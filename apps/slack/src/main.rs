//! olo-slack: Slack, Olo-fied, as olo-chrome is Chrome. Slack's web app
//! in Olo's Chrome as an app window of its own (`--app`, a Chrome profile
//! of its own, so its sign-in and data are Slack's alone), which Olo
//! knows as `olo.slack` (`olo_xdg::apps::WEB_APPS`). The Olo extension
//! and its host (apps/chrome: slack.js, slack-page.js, slack.rs) join it
//! to the desktop: its unread count on its Deskbar entry, its messages as
//! Olo notifications, its links in Olo's default apps, pictures in the
//! Viewer, its channels in the palette, and agents' trees, as Chrome's.
//!
//! `olo-slack [LINK]`: with Slack open, its window comes forward (and goes
//! to LINK: a `slack://` link, or a Slack page); else Chrome is started on
//! Slack's profile (olo-chrome: it offers to install Chrome if it isn't
//! yet), LINK opened once Slack is in.

use std::{os::unix::process::CommandExt, path::PathBuf};

use olo_protocol::{Client, Reply, Request};

/// Where Slack's web app is (`OLO_SLACK_URL` for tests: a page standing in
/// for it).
const SLACK: &str = "https://app.slack.com/client";

/// The link to open once Slack is in, in its profile folder
/// (apps/chrome's `slack::open_file`).
const LINK_FILE: &str = "Olo Slack Link";

/// Our app id (its window's, as app_server reports it).
const APP_ID: &str = "olo.slack";

fn slack_url() -> String {
    std::env::var("OLO_SLACK_URL").ok().filter(|u| u.starts_with("http")).unwrap_or_else(|| SLACK.to_owned())
}

/// Slack's own Chrome profile (user data folder): beside Chrome's own
/// (`$CHROME_CONFIG_HOME`, an agent screen's, or the config folder), so
/// each screen has its own Slack too.
fn user_data_dir() -> Option<PathBuf> {
    let config = std::env::var_os("CHROME_CONFIG_HOME")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))?;
    Some(config.join("olo-slack"))
}

/// A link to open in Slack, as a page of Slack's web app at `base` (its
/// client's address): `slack://channel?team=T&id=C` (and `open`, `user`,
/// `file`), or a Slack page itself. None for anything else.
fn page_for(link: &str, base: &str) -> Option<String> {
    if let Some(rest) = link.strip_prefix("slack://") {
        let (what, query) = rest.split_once('?').unwrap_or((rest, ""));
        let param = |key: &str| query.split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('=')).filter(|v| v.chars().all(|c| c.is_ascii_alphanumeric()) && !v.is_empty());
        let team = param("team")?;
        return Some(match (what.trim_end_matches('/'), param("id")) {
            ("channel" | "user", Some(id)) => format!("{base}/{team}/{id}"),
            _ => format!("{base}/{team}"),
        });
    }
    let (scheme, rest) = link.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    let slack = scheme == "https" && (host == "slack.com" || host.ends_with(".slack.com"));
    let at = format!("{scheme}://{host}");
    let stand_in = base == at || base.starts_with(&format!("{at}/"));
    (slack || stand_in).then(|| link.to_owned())
}

/// Slack's window, if it is open: its id and its process (Chrome's).
fn slack_window() -> Option<(u64, u32)> {
    let mut client = Client::connect().ok()?;
    let Ok(Reply::Windows { windows }) = client.request(&Request::ListWindows) else { return None };
    // The one used last first (the list is front to back).
    let w = windows.into_iter().find(|w| w.app_id == APP_ID)?;
    Some((w.id, u32::try_from(w.pid?).ok()?))
}

fn main() {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let base = slack_url();
    let open = args.iter().find(|a| !a.starts_with("--")).and_then(|l| page_for(l, &base));
    // Slack open already: forward, and to the link.
    if let Some((id, pid)) = slack_window() {
        if let Some(page) = &open
            && !olo_kit_base::commands::invoke(pid, &format!("open {page}"))
        {
            eprintln!("olo-slack: Slack didn't take the link {page}");
        }
        if let Ok(mut c) = Client::connect() {
            let _ = c.request(&Request::Activate { id });
        }
        return;
    }
    let Some(dir) = user_data_dir() else {
        eprintln!("olo-slack: no home folder");
        std::process::exit(1);
    };
    let Some(chrome) = olo_xdg::find_program("olo-chrome") else {
        eprintln!("olo-slack: olo-chrome isn't installed");
        std::process::exit(1);
    };
    // Always the same page (the window's app id comes from it), then the
    // link once Slack is in (the host tells the extension: slack.rs).
    let mut cmd = std::process::Command::new(&chrome);
    cmd.arg(format!("--user-data-dir={}", dir.display())).arg(format!("--app={base}"));
    // No first-run dialog: its "Make Google Chrome the default browser",
    // ticked, would make Slack's Chrome the person's browser.
    cmd.args(["--no-first-run", "--no-default-browser-check"]);
    cmd.env("OLO_SLACK_URL", &base).env("OLO_SLACK_PROFILE", &dir);
    // (Left for the extension's host to take once: apps/chrome's slack.rs.)
    if let Some(page) = &open
        && let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(LINK_FILE), page))
    {
        eprintln!("olo-slack: couldn't keep the link to open: {e}");
    }
    let e = cmd.exec();
    eprintln!("olo-slack: couldn't start {}: {e}", chrome.display());
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_become_slack_pages() {
        let b = SLACK;
        assert_eq!(page_for("slack://channel?team=T0123&id=C0456", b).as_deref(), Some("https://app.slack.com/client/T0123/C0456"));
        assert_eq!(page_for("slack://user?team=T1&id=U9", b).as_deref(), Some("https://app.slack.com/client/T1/U9"));
        assert_eq!(page_for("slack://open?team=T1", b).as_deref(), Some("https://app.slack.com/client/T1"));
        assert_eq!(page_for("slack://file?team=T1&id=F2", b).as_deref(), Some("https://app.slack.com/client/T1"));
        // No team, or one that isn't an id: nothing.
        assert_eq!(page_for("slack://open", b), None);
        assert_eq!(page_for("slack://channel?team=../x&id=C1", b), None);
        // Slack's pages as they are; anyone else's not.
        assert_eq!(page_for("https://acme.slack.com/archives/C1/p123", b).as_deref(), Some("https://acme.slack.com/archives/C1/p123"));
        assert_eq!(page_for("https://app.slack.com/client/T1/C2", b).as_deref(), Some("https://app.slack.com/client/T1/C2"));
        assert_eq!(page_for("https://example.com/slack.com", b), None);
        assert_eq!(page_for("https://evilslack.com/", b), None);
        // A stand-in's own pages.
        assert_eq!(page_for("http://127.0.0.1:8123/client/T1", "http://127.0.0.1:8123/client").as_deref(), Some("http://127.0.0.1:8123/client/T1"));
        assert_eq!(page_for("http://127.0.0.1:8124/client/T1", "http://127.0.0.1:8123/client"), None);
        assert_eq!(page_for("http://acme.slack.com/archives/C1", b), None);
    }
}
