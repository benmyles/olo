//! Agent Setup (Back Up to Google Drive ▸ Let an Agent Do It…): an agent
//! does Google Cloud's steps for the person (Ben, 2026-11-08: "an "Agent
//! Setup" feature to disk backup where an agent can do the google drive
//! oauth/app setup"). The person chooses the Google account's login in
//! Passwords; DiskBackup starts an agent screen of its own and shares that
//! login with it (Passwords' sharing with agent screens), then opens a
//! Terminal window running Claude Code (Preferences ▸ AI's model and
//! effort) with the task: the screen, how to sign in with the login there
//! without seeing or typing its password, the steps, and how to finish:
//! `olo-diskbackup --agent-setup-done CLIENT_FILE --screen NAME`, which
//! keeps the client in Passwords, signs DiskBackup in (Google's consent
//! page on the agent's screen, for it to allow), sets it up, and tells
//! DiskBackup's window. The person follows along in Terminal and can talk
//! to the agent there.

use std::process::Command;

use olo_keychain::{Request, Response, ScreenShares, Shareable};

use crate::{drive_setup::STEPS, target};

/// The agent screen DiskBackup makes for it (`OLO_DISKBACKUP_AGENT_SCREEN`:
/// another, for tests).
pub const SCREEN: &str = "diskbackup-setup";

pub fn screen_name() -> String {
    std::env::var("OLO_DISKBACKUP_AGENT_SCREEN").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| SCREEN.to_owned())
}

/// The person's Google logins in Passwords (what may be said of them: no
/// secrets): those that say Google, else every login. Blocks (Passwords
/// started and unlocked if it asks).
pub fn google_logins() -> Result<Vec<Shareable>, String> {
    // (A Response zeroes its secrets as it goes: what's needed is cloned.)
    let r = target::passwords(&Request::Shares)?;
    let Response::Shares { items, .. } = &r else { return Err("Passwords didn't list its items".into()) };
    Ok(google_of(items.clone()))
}

/// [`google_logins`] of `items`.
pub fn google_of(items: Vec<Shareable>) -> Vec<Shareable> {
    let logins: Vec<Shareable> = items.into_iter().filter(|i| i.kind == "Login").collect();
    let google: Vec<Shareable> = logins.iter().filter(|i| is_google(i)).cloned().collect();
    if google.is_empty() { logins } else { google }
}

fn is_google(i: &Shareable) -> bool {
    let words = format!("{} {}", i.title, i.subtitle).to_lowercase();
    ["google", "gmail", "workspace"].iter().any(|w| words.contains(w))
}

/// What a login is called in the list: its title, and its site or username.
pub fn label(i: &Shareable) -> String {
    if i.subtitle.is_empty() || i.subtitle == i.title { i.title.clone() } else { format!("{} — {}", i.title, i.subtitle) }
}

/// Its agent screen running (started if it isn't), `login` shared with it,
/// and Claude Code started in a Terminal window with the task. The screen's
/// name. Blocks.
pub fn start(login: &Shareable) -> Result<String, String> {
    let screen = screen_name();
    running(&screen)?;
    share(&screen, login)?;
    let settings = olo_ui::settings::current().map(|s| s.ai.claude_code).unwrap_or_default();
    claude(&brief(&screen, login), &settings)?;
    tracing::info!(screen, login = login.title, model = settings.model, "diskbackup: an agent is setting up Google Drive");
    Ok(screen)
}

/// Olo's own program `name`: beside us, else on PATH.
fn program(name: &str) -> String {
    olo_xdg::find_program(name).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| name.to_owned())
}

/// Screen `name` running: as it is, else started (made, or a stopped or
/// archived one started again).
fn running(name: &str) -> Result<(), String> {
    let out = Command::new(program("olo-screen")).arg("list").output().map_err(|e| format!("olo-screen: {e}"))?;
    let list: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap_or_default();
    if list.iter().any(|s| s["name"] == name && s["state"] == "running") {
        return Ok(());
    }
    let out = Command::new(program("olo-screen")).args(["start", name]).output().map_err(|e| format!("olo-screen: {e}"))?;
    if !out.status.success() {
        return Err(format!("its agent screen didn't start: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(())
}

/// `login` shared with screen `screen`, besides what it has (Passwords'
/// sharing, as Agent Screens' Passwords tab does it).
fn share(screen: &str, login: &Shareable) -> Result<(), String> {
    let r = target::passwords(&Request::Shares)?;
    let Response::Shares { everywhere, screens, .. } = &r else { return Err("Passwords didn't say what it shares".into()) };
    let mut mine = screens.get(screen).cloned().unwrap_or_default();
    if let Some(shares) = shared_too(&mine, everywhere, &login.id) {
        mine = shares;
        match target::passwords(&Request::SetShares { everywhere: None, screen: Some((screen.to_owned(), mine)) })? {
            Response::Shared => {}
            other => return Err(format!("Passwords didn't share it: {other:?}")),
        }
    }
    Ok(())
}

/// What screen `shares` become with item `id` shared too (None: it has it
/// already, from every screen's or its own).
pub fn shared_too(shares: &ScreenShares, everywhere: &[String], id: &str) -> Option<ScreenShares> {
    let has = (everywhere.iter().any(|i| i == id) && !shares.remove.iter().any(|i| i == id)) || shares.add.iter().any(|i| i == id);
    if has {
        return None;
    }
    let mut s = shares.clone();
    s.remove.retain(|i| i != id);
    if !everywhere.iter().any(|i| i == id) {
        s.add.push(id.to_owned());
    }
    Some(s)
}

/// The task, as Claude Code is given it.
pub fn brief(screen: &str, login: &Shareable) -> String {
    let steps: String = STEPS.iter().enumerate().map(|(i, (title, how, url))| format!("{}. {title}: {how} ({url})\n", i + 1)).collect();
    format!(
        "Set up Olo's DiskBackup to back up to Google Drive, for the person at this computer. Google needs each person's own OAuth client for an app like DiskBackup: make it in Google Cloud, signed in as the Google account the backups go in, then hand it to DiskBackup.

Your screen: Olo agent screen \"{screen}\", made for this (the olo-screens skill; `olo-screen --help`). Work only there, never on the person's own display. Its browser: `olo-screen run {screen} olo-chrome URL`.

The Google account: \"{title}\" ({subtitle}), the person's login in Olo Passwords, shared with your screen. Sign in with it there without seeing or typing its password: right-click the email or password field in Olo's Chrome and choose Olo ▸ Autofill with Passwords (or Passwords ▸ the login ▸ Autofill); or put a value on the screen's clipboard with `( eval \"$(olo-screen env {screen})\"; op item get \"{title}\" --fields username | pbcopy )` (password the same way) and paste it with `olo-screen key {screen} Cmd+V`. Never print, type or echo the password. A one-time code: `op item get \"{title}\" --otp` the same way, if the login keeps one; if Google asks for something only the person has (a code sent to their phone, a security key), ask them here and wait.

The steps (Google Cloud Console, as that account):
{steps}
Download JSON at the end saves the client's file to ~/Downloads (client_secret_….json).

Then hand it to DiskBackup, running this in the background (it waits for you on the next step):
  olo-diskbackup --agent-setup-done ~/Downloads/client_secret_….json --screen {screen}
It keeps the client in Olo Passwords (as \"{client}\"; Passwords may ask the person to unlock it), then opens Google's consent page on your screen: choose the account (sign in again if asked), and Continue or Allow. It prints \"Backing up to Google Drive (…)\" once DiskBackup is set up, or what went wrong. Then tell the person it's done, and that they can stop the agent screen in Agent Screens.

A Google Workspace account (its domain isn't gmail.com or googlemail.com): choose Audience Internal at step 3, which needs no publishing and whose sign-in never expires, and skip step 5. A Gmail account: External; publishing needs a homepage and a privacy-policy address first, so ask the person for them, or leave it in Testing with them added as a test user (Google then signs DiskBackup out every 7 days) if they'd rather.

Keep to what's needed: one project, the Drive API, the sign-in's consent screen, the drive.file scope only, one Desktop app client. Say what you're doing as you go; the person is following along here and may tell you something.",
        title = login.title,
        subtitle = login.subtitle,
        client = target::CLIENT_TITLE,
    )
}

/// Where Claude Code works: a folder of its own (Claude Code asks once
/// whether to trust the folder it starts in; the home folder would be far
/// more than the task needs), the task kept there too.
pub fn work_dir() -> std::path::PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| target::home().join(".local/state"));
    state.join("olo").join("diskbackup-agent-setup")
}

/// Claude Code started in a new Terminal window with `prompt`, with
/// `settings`' model and effort, let use olo-screen and olo-diskbackup
/// without asking each time (the person sees them, and can stop it).
fn claude(prompt: &str, settings: &olo_protocol::ClaudeCode) -> Result<(), String> {
    let dir = work_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let _ = std::fs::write(dir.join("TASK.md"), prompt);
    let mut c = Command::new(program("olo-terminal"));
    c.arg("-w").arg(&dir).arg("-e").args(claude_args(prompt, settings));
    // (Its own session: not one a Claude Code session started DiskBackup
    // from would make it, which turns its transcript off.)
    for (k, _) in std::env::vars_os() {
        if k.to_str().is_some_and(|k| k.starts_with("CLAUDE_CODE") || k == "CLAUDECODE") {
            c.env_remove(k);
        }
    }
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null());
    c.spawn().map(drop).map_err(|e| format!("Terminal didn't start: {e}"))
}

/// The command Terminal runs: `claude` and its arguments.
pub fn claude_args(prompt: &str, settings: &olo_protocol::ClaudeCode) -> Vec<String> {
    let command = std::env::var("OLO_DISKBACKUP_AGENT_COMMAND").unwrap_or_else(|_| "claude".to_owned());
    vec![
        command,
        // (One value each, before the prompt: a list option takes the
        // words after it.)
        "--allowedTools".into(),
        "Bash(olo-screen *),Bash(olo-diskbackup --agent-setup-done *)".into(),
        // The screen's pictures (`olo-screen look`) and the client's file,
        // read without asking; the person's own Chrome never (its screen's
        // Chrome only).
        "--add-dir".into(),
        olo_xdg::runtime_dir().join("olo-screens").to_string_lossy().into_owned(),
        "--add-dir".into(),
        target::home().join("Downloads").to_string_lossy().into_owned(),
        "--no-chrome".into(),
        "--model".into(),
        settings.model.clone(),
        "--effort".into(),
        settings.effort.clone(),
        "--name".into(),
        "DiskBackup Agent Setup".into(),
        prompt.to_owned(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, title: &str, subtitle: &str, kind: &str) -> Shareable {
        Shareable { vault: "v".into(), vault_title: "Personal".into(), id: id.into(), title: title.into(), subtitle: subtitle.into(), kind: kind.into() }
    }

    #[test]
    fn google_logins_first() {
        let items = vec![item("1", "GitHub", "github.com", "Login"), item("2", "Work", "accounts.google.com", "Login"), item("3", "Google note", "", "Secure Note")];
        assert_eq!(google_of(items.clone()).iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["2"]);
        // None says Google: every login.
        assert_eq!(google_of(vec![items[0].clone(), items[2].clone()]).iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["1"]);
        assert_eq!(label(&items[1]), "Work — accounts.google.com");
    }

    #[test]
    fn shared_once() {
        let none = ScreenShares::default();
        assert_eq!(shared_too(&none, &[], "a"), Some(ScreenShares { add: vec!["a".into()], remove: vec![] }));
        // Shared with every screen already, or with this one: as it is.
        assert_eq!(shared_too(&none, &["a".into()], "a"), None);
        assert_eq!(shared_too(&ScreenShares { add: vec!["a".into()], remove: vec![] }, &[], "a"), None);
        // Every screen's, but kept from this one: given back.
        assert_eq!(shared_too(&ScreenShares { add: vec![], remove: vec!["a".into()] }, &["a".into()], "a"), Some(ScreenShares::default()));
    }

    #[test]
    fn the_brief_says_where_what_and_how() {
        let b = brief("diskbackup-setup", &item("2", "Work Google", "ben@example.com", "Login"));
        for want in [
            "Audience Internal",
            "agent screen \"diskbackup-setup\"",
            "\"Work Google\" (ben@example.com)",
            "Autofill with Passwords",
            "Never print, type or echo the password",
            "olo-diskbackup --agent-setup-done ~/Downloads/client_secret_….json --screen diskbackup-setup",
            "drive.file",
            "DiskBackup Google Client",
        ] {
            assert!(b.contains(want), "{want}");
        }
        assert_eq!(b.matches("console.cloud.google.com").count(), STEPS.len());
        let args = claude_args("do it", &olo_protocol::ClaudeCode::default());
        assert_eq!(args[args.len() - 1], "do it", "the prompt last");
        assert!(args.windows(2).any(|w| w == ["--model", "claude-opus-5-5"]) && args.windows(2).any(|w| w == ["--effort", "medium"]));
        assert!(args.contains(&"--no-chrome".to_owned()) && args.iter().any(|a| a.ends_with("/olo-screens")));
    }
}
