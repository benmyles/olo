//! An account's own app made for it in Google Cloud, by an agent
//! (Preferences ▸ Integrations ▸ Google ▸ Add a Google Account…; it was
//! DiskBackup's Agent Setup, for its Drive alone: Ben, 2026-11-18: "we
//! need to move that ai account setup thing from diskbackup to be in
//! the prefs -> integrations for each google account that someone wants
//! to connect, and then that app must be used").
//!
//! The person chooses the Google account's login in Passwords. An agent
//! screen is started for it and the login shared with that screen
//! (Passwords' sharing with agent screens); Claude Code is started in a
//! Terminal window (Preferences ▸ AI's model and effort) with the task:
//! the screen, how to sign in with the login there without seeing or
//! typing its password, the steps in Google Cloud (a project, the APIs
//! Olo's services use, the sign-in's consent screen named "Olo
//! (address)", a Desktop app client), and how to finish:
//! `olo-preferences --google-setup-done CLIENT_FILE --screen NAME`,
//! which signs Olo in with the new client (Google's consent page on the
//! agent's screen, for it to allow) and lists the account. The person
//! follows along in Terminal and can talk to the agent there.
//!
//! An account whose app is there already (one DiskBackup's own set-up
//! made, before this) isn't given another: the files DiskBackup wrote
//! to its Drive are seen only by that project's app (`drive.file`), so
//! the agent brings that app up to date instead ([`Task::Extend`]).

use std::process::Command;

use olo_keychain::{Request, Response, ScreenShares, Shareable};

use crate::accounts::{Account, Service};

/// The agent screen made for it (`OLO_GOOGLE_AGENT_SCREEN`: another, for
/// tests).
pub const SCREEN: &str = "google-setup";

pub fn screen_name() -> String {
    std::env::var("OLO_GOOGLE_AGENT_SCREEN").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| SCREEN.to_owned())
}

/// `request` to Passwords, starting it first if it isn't running.
pub fn passwords(request: &Request) -> Result<Response, String> {
    let mut started = false;
    loop {
        match olo_keychain::call(request) {
            Ok(Response::Error { ref message }) => return Err(message.clone()),
            Ok(r) => return Ok(r),
            Err(olo_keychain::CallError::NotRunning) if !started => {
                started = true;
                let _ = olo_xdg::spawn_detached("olo-passwords");
                for _ in 0..50 {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    if olo_keychain::socket_path().exists() {
                        break;
                    }
                }
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// The person's Google logins in Passwords (what may be said of them: no
/// secrets): those that say Google, else every login. Blocks (Passwords
/// started and unlocked if it asks).
pub fn google_logins() -> Result<Vec<Shareable>, String> {
    // (A Response zeroes its secrets as it goes: what's needed is cloned.)
    let r = passwords(&Request::Shares)?;
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

/// What the agent is to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Task {
    /// Make the account's app: a project, its APIs, its sign-in, a client.
    Make,
    /// The account `address` has an app already (the client with this
    /// id, which Olo keeps): rename it, and turn on what it lacks.
    Extend { address: String, client_id: String },
}

/// Its agent screen running (started if it isn't), `login` shared with
/// it, and Claude Code started in a Terminal window with the task. The
/// screen's name. Blocks.
pub fn start(login: &Shareable, task: &Task, settings: &olo_protocol::ClaudeCode) -> Result<String, String> {
    let screen = screen_name();
    running(&screen)?;
    share(&screen, login)?;
    claude(&brief(&screen, login, task), settings)?;
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

/// `url` opened in Olo's Chrome on screen `screen`: whether it was.
pub fn open_on_screen(screen: &str, url: &str) -> bool {
    Command::new(program("olo-screen")).args(["run", screen, "olo-chrome", url]).stdout(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
}

/// `login` shared with screen `screen`, besides what it has (Passwords'
/// sharing, as Agent Screens' Passwords tab does it).
fn share(screen: &str, login: &Shareable) -> Result<(), String> {
    let r = passwords(&Request::Shares)?;
    let Response::Shares { everywhere, screens, .. } = &r else { return Err("Passwords didn't say what it shares".into()) };
    let mut mine = screens.get(screen).cloned().unwrap_or_default();
    if let Some(shares) = shared_too(&mine, everywhere, &login.id) {
        mine = shares;
        match passwords(&Request::SetShares { everywhere: None, screen: Some((screen.to_owned(), mine)) })? {
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

/// The page that turns on every API Olo's services use, in the project
/// chosen.
pub const ENABLE_APIS: &str = "https://console.cloud.google.com/flows/enableapi?apiid=gmail.googleapis.com,caldav.googleapis.com,drive.googleapis.com";
pub const NEW_PROJECT: &str = "https://console.cloud.google.com/projectcreate";
pub const SIGN_IN_SCREEN: &str = "https://console.cloud.google.com/auth/overview";
pub const BRANDING: &str = "https://console.cloud.google.com/auth/branding";
pub const SCOPES_PAGE: &str = "https://console.cloud.google.com/auth/scopes";
pub const AUDIENCE: &str = "https://console.cloud.google.com/auth/audience";
pub const NEW_CLIENT: &str = "https://console.cloud.google.com/auth/clients/create";
pub const CLIENTS: &str = "https://console.cloud.google.com/auth/clients";

/// The steps that make an account's app, for a person doing them by
/// hand and for the agent: what, how, and the page that does it.
/// `app` is the app's name ("Olo (ada@example.com)").
pub fn steps(app: &str) -> Vec<(&'static str, String, &'static str)> {
    let scopes: Vec<&str> = Service::ALL.iter().map(|s| s.scope()).collect();
    vec![
        ("Make a project", "Signed in as the account being connected: name it “Olo” (a project's name takes no @ or brackets), then Create, and choose it at the top of the page.".to_owned(), NEW_PROJECT),
        ("Turn on its APIs", "With that project chosen: Enable (the Gmail API, the CalDAV API and the Google Drive API, all three).".to_owned(), ENABLE_APIS),
        (
            "Set up its sign-in",
            format!("Get Started: App name “{app}” and the account's email; Audience: Internal for a Google Workspace account (no publishing, no warning, never signed out), External for a Gmail account; the email again; agree, then Create."),
            SIGN_IN_SCREEN,
        ),
        (
            "Say what it may ask for",
            format!("Data Access ▸ Add or Remove Scopes: under “Manually add scopes” paste {} (one a line), Add to Table, Update, then Save.", scopes.join(" , ")),
            SCOPES_PAGE,
        ),
        (
            "Publish it (External only)",
            "Audience ▸ Publish App, and Confirm (Branding may first want a homepage and a privacy-policy address). It isn't sent for Google's review: signing in shows “Google hasn’t verified this app” once (Advanced ▸ Go to Olo), which is right for an app that is the person's own. Left in Testing instead, Google signs Olo out every 7 days. Internal: nothing to do.".to_owned(),
            AUDIENCE,
        ),
        ("Make its desktop client", "Create Client: Application type Desktop app, name “Olo”, then Create; Download JSON.".to_owned(), NEW_CLIENT),
    ]
}

/// The command that finishes: signs Olo in with the client and lists
/// the account.
pub const DONE: &str = "olo-preferences --google-setup-done";

/// The task, as Claude Code is given it.
pub fn brief(screen: &str, login: &Shareable, task: &Task) -> String {
    let head = format!(
        "Connect a Google account to Olo, for the person at this computer. Google lets only a registered app sign someone in, so each account Olo connects has an app of its own in its own Google Cloud project, called “Olo (the account's address)”; Olo's Mail, Calendar and DiskBackup all sign in through it.

Your screen: Olo agent screen \"{screen}\", made for this (the olo-screens skill; `olo-screen --help`). Work only there, never on the person's own display. Its browser: `olo-screen run {screen} olo-chrome URL`.

The Google account: \"{title}\" ({subtitle}), the person's login in Olo Passwords, shared with your screen. Sign in with it there without seeing or typing its password: right-click the email or password field in Olo's Chrome and choose Olo ▸ Autofill with Passwords (or the key in the address bar); a code sent to the person's phone or mail is put in by Olo, or the person is asked. Never print, type or echo the password. The account's address is what Google shows once signed in (the login's username, usually): use exactly that in the app's name.
",
        title = login.title,
        subtitle = login.subtitle,
    );
    let rest = match task {
        Task::Make => {
            let listed: String = steps("Olo (the account's address)").iter().enumerate().map(|(i, (title, how, url))| format!("{}. {title}: {how} ({url})\n", i + 1)).collect();
            format!(
                "
The steps (Google Cloud Console, as that account). First look whether the account already has a project named “Olo” with a Desktop app client (a set-up begun before): then finish that one instead of making another.
{listed}
Download JSON at the end saves the client's file to ~/Downloads (client_secret_….json).

Then hand it to Olo, running this in the background (it waits for you on the next step):
  {DONE} ~/Downloads/client_secret_….json --screen {screen}
It opens Google's consent page on your screen: choose the account (sign in again if asked), allow everything it asks (tick every box if Google lists them apart), and Continue or Allow; an app that isn't Internal shows “Google hasn’t verified this app” first: Advanced, then Go to Olo. It prints \"Connected …\" with what each of Mail, Calendar and DiskBackup said once the account is connected, or what went wrong (an API not turned on: turn it on and run it again). Then tell the person it's done, that the account is in Preferences ▸ Integrations ▸ Google where they tick what it's used for, and that they can stop the agent screen in Agent Screens.

A Google Workspace account (its domain isn't gmail.com or googlemail.com): Audience Internal at step 3, and skip step 5. If its organisation doesn't let it make projects, say so and stop: its administrator must allow that, or make the client. A Gmail account: External, published at step 5 (ask the person for a homepage and privacy-policy address if Google wants them; any page of theirs does).

Keep to what's needed: one project, the three APIs, the sign-in's consent screen, the three scopes, one Desktop app client."
            )
        }
        Task::Extend { address, client_id } => format!(
            "
This account ({address}) has an app already: Olo's DiskBackup made it (probably a project named “Olo DiskBackup”), with the client whose ID is
  {client_id}
Do NOT make another project or client: the backups DiskBackup wrote to this account's Drive are visible only to that project's app, and Olo keeps that client. Bring that app up to date instead (Google Cloud Console, as that account, with the project that holds that client chosen at the top of the page: find it under {CLIENTS} in each project if there are several):
1. Rename the app: Branding ▸ App name “{app}”, Save ({BRANDING}). (The project itself may be renamed “Olo” too, in IAM & Admin ▸ Settings; it doesn't matter if not.)
2. Turn on its APIs: Enable the Gmail API and the CalDAV API (the Google Drive API is on already) ({ENABLE_APIS}).
3. Say what it may ask for: Data Access ▸ Add or Remove Scopes: under “Manually add scopes” paste {mail} and {calendar}, Add to Table, Update, then Save; leave …/auth/drive.file as it is ({SCOPES_PAGE}).

Then sign Olo in again with the same client, running this in the background (it waits for you on the next step):
  {DONE} --account {address} --screen {screen}
It opens Google's consent page on your screen: choose {address} (sign in again if asked), allow everything it asks (tick every box if Google lists them apart), and Continue or Allow; an app that isn't Internal shows “Google hasn’t verified this app” first: Advanced, then go on. It prints \"Connected …\" with what each of Mail, Calendar and DiskBackup said, or what went wrong (an API not turned on: turn it on and run it again). DiskBackup goes on backing up to the same place throughout: nothing of its files or its state changes. Then tell the person it's done, and that they can stop the agent screen in Agent Screens.

Change nothing else in that project.",
            app = Account::app_name(address),
            mail = Service::Mail.scope(),
            calendar = Service::Calendar.scope(),
        ),
    };
    format!("{head}{rest} Say what you're doing as you go; the person is following along here and may tell you something.")
}

/// Where Claude Code works: a folder of its own (Claude Code asks once
/// whether to trust the folder it starts in; the home folder would be far
/// more than the task needs), the task kept there too.
pub fn work_dir() -> std::path::PathBuf {
    olo_xdg::state_dir().join("google-agent-setup")
}

/// Claude Code started in a new Terminal window with `prompt`, with
/// `settings`' model and effort, let use olo-screen and the finishing
/// command without asking each time (the person sees them, and can stop
/// it).
fn claude(prompt: &str, settings: &olo_protocol::ClaudeCode) -> Result<(), String> {
    let dir = work_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let _ = std::fs::write(dir.join("TASK.md"), prompt);
    let mut c = Command::new(program("olo-terminal"));
    c.arg("-w").arg(&dir).arg("-e").args(claude_args(prompt, settings));
    // (Its own session: not one a Claude Code session started this from
    // would make it, which turns its transcript off.)
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
    let command = std::env::var("OLO_GOOGLE_AGENT_COMMAND").unwrap_or_else(|_| "claude".to_owned());
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    vec![
        command,
        // (One value each, before the prompt: a list option takes the
        // words after it.)
        "--allowedTools".into(),
        format!("Bash(olo-screen *),Bash({DONE} *)"),
        // The screen's pictures (`olo-screen look`) and the client's file,
        // read without asking; the person's own Chrome never (its screen's
        // Chrome only).
        "--add-dir".into(),
        olo_xdg::runtime_dir().join("olo-screens").to_string_lossy().into_owned(),
        "--add-dir".into(),
        home.join("Downloads").to_string_lossy().into_owned(),
        "--no-chrome".into(),
        "--model".into(),
        settings.model.clone(),
        "--effort".into(),
        settings.effort.clone(),
        "--name".into(),
        "Google Account Setup".into(),
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

    /// The task says where, who, what and how to finish; an account
    /// with an app already is brought up to date, never given another.
    #[test]
    fn the_brief_says_where_what_and_how() {
        let login = item("2", "Work Google", "ada@example.com", "Login");
        let b = brief("google-setup", &login, &Task::Make);
        for want in [
            "Audience Internal",
            "agent screen \"google-setup\"",
            "\"Work Google\" (ada@example.com)",
            "Autofill with Passwords",
            "Never print, type or echo the password",
            "olo-preferences --google-setup-done ~/Downloads/client_secret_….json --screen google-setup",
            "App name “Olo (the account's address)”",
            "https://mail.google.com/",
            "…/auth/calendar".trim_start_matches('…'),
            "/auth/drive.file",
            "gmail.googleapis.com,caldav.googleapis.com,drive.googleapis.com",
            "Desktop app",
        ] {
            assert!(b.contains(want), "{want}\n{b}");
        }
        let e = brief("google-setup", &login, &Task::Extend { address: "ada@example.com".into(), client_id: "123.apps.googleusercontent.com".into() });
        for want in [
            "Do NOT make another project or client",
            "123.apps.googleusercontent.com",
            "App name “Olo (ada@example.com)”",
            "olo-preferences --google-setup-done --account ada@example.com --screen google-setup",
            "nothing of its files or its state changes",
        ] {
            assert!(e.contains(want), "{want}\n{e}");
        }
        assert!(!e.contains("Make a project"));
        let args = claude_args("do it", &olo_protocol::ClaudeCode::default());
        assert_eq!(args[args.len() - 1], "do it", "the prompt last");
        assert!(args.windows(2).any(|w| w[0] == "--allowedTools" && w[1] == "Bash(olo-screen *),Bash(olo-preferences --google-setup-done *)"));
        assert!(args.contains(&"--no-chrome".to_owned()) && args.iter().any(|a| a.ends_with("/olo-screens")));
    }
}
