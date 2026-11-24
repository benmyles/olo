//! The session's services, as the session tells of them and is asked to
//! restart, stop or start one (Preferences ▸ Services; `olo-session
//! services`). Ben, 2026-11-23, on asking whether Passwords' SSH agent had
//! to be restarted by hand to take a fix: "a Preferences -> Services app
//! that shows all of the olo services and their status and lets me
//! manage/restart them".
//!
//! Over a socket of the session's own ([`socket_path`]): one request a
//! line ([`Ask`], JSON), one answer a line ([`Said`]). [`Ask::Watch`]
//! keeps the connection and is sent the list again each time it changes,
//! so a window showing it asks nothing while nothing happens.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How a service stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Running,
    /// Stopped as the person asked (until they start it, or the next
    /// login).
    Stopped,
    /// It ended saying it isn't wanted (turned off in its settings:
    /// `olo_xdg::SERVICE_NOT_WANTED`), so it isn't started again.
    NotWanted,
    /// It ended too often in a minute, and was left stopped.
    GaveUp,
}

/// One service of the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Service {
    /// Its program's name (`olo-pool`), which names it to [`Ask`].
    pub program: String,
    /// What it was started with, after the program.
    #[serde(default)]
    pub args: Vec<String>,
    /// Its process, while it runs (0 when it doesn't).
    pub pid: i32,
    /// When it was started last, in seconds since the epoch (0: never).
    #[serde(default)]
    pub since: u64,
    /// How often it ended and was started again by the session this
    /// login (not counting a restart asked for).
    #[serde(default)]
    pub restarts: u32,
    pub state: State,
    /// The window server: restarting it is the session's reload (every
    /// app reconnects), and it can't be stopped.
    #[serde(default)]
    pub window_server: bool,
}

/// What the session is asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Ask {
    /// The list, once.
    List,
    /// The list now and each time it changes, on this connection.
    Watch,
    /// End it and start it again (a stopped one: start it).
    Restart { program: String },
    /// End it, and don't start it again until asked.
    Stop { program: String },
    /// Start it (one stopped, not wanted or given up on).
    Start { program: String },
}

/// What the session answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Said {
    Services { services: Vec<Service> },
    Error { message: String },
}

/// The session's socket for these, in its runtime folder, for its
/// display (`wayland-1`).
pub fn socket_path(runtime: &Path, display: &str) -> PathBuf {
    runtime.join(format!("olo-session-{display}.sock"))
}

/// A service as a person knows it: its name, and what it's for in a
/// line (words of the words layer). One not known here is named by its
/// program.
pub fn about(program: &str) -> (&str, &'static str) {
    match program {
        "olo-app-server" => ("Window Server", "Draws every window and takes the keyboard and pointer."),
        "olo-deskbar" => ("Deskbar", "The menu, the running apps, the tray and the clock."),
        "olo-tracker" => ("Desktop", "The icons on the desktop (Tracker)."),
        "olo-polkit-agent" => ("Authentication", "Asks for your password when a program needs more rights."),
        "olo-notifications" => ("Notifications", "Shows notifications and keeps them for later."),
        "olo-portal" => ("Portal", "Open and save panels, and screen sharing, for other apps."),
        "olo-pool" => ("Token Pool", "Shares the coding agents’ accounts between them."),
        "olo-supervisor" => ("Supervisor", "Keeps background work from slowing what you use."),
        "olo-doctor" => ("Doctor", "Tells of problems it finds, at login and as they happen."),
        "olo-airpods" => ("AirPods", "Your AirPods’ battery, modes and ear detection."),
        "olo-phoned" => ("Phone", "Your phone’s calls, messages and one-time codes."),
        "olo-voice" => ("Voice", "Dictation and the voice assistant."),
        "olo-diskbackup" => ("DiskBackup", "Backs up this computer’s disk when one is due."),
        "olo-calendar" => ("Calendar Reminders", "Reminds you of your events."),
        "olo-todo" => ("To-Do Reminders", "Reminds you of your to-dos."),
        "olo-passwords" => ("SSH Agent", "Your SSH keys from Passwords, for ssh and git."),
        "olo-secrets" => ("Login Keyring", "Keeps other apps’ passwords, unlocked as you log in."),
        "olo-terminal" => ("Agent Tab Titles", "Lets coding agents title their Terminal tab."),
        "olo-bus" => ("Message Bus", "This screen’s own bus, for the programs on it."),
        "olo-greeter" => ("Login Window", "Asks who is logging in."),
        other => (other, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_and_answers_as_lines() {
        let ask: Ask = serde_json::from_str(r#"{"do":"restart","program":"olo-pool"}"#).unwrap();
        assert_eq!(ask, Ask::Restart { program: "olo-pool".into() });
        assert_eq!(serde_json::to_string(&Ask::Watch).unwrap(), r#"{"do":"watch"}"#);
        let said =
            Said::Services { services: vec![Service { program: "olo-pool".into(), args: vec!["--service".into()], pid: 42, since: 1, restarts: 0, state: State::Running, window_server: false }] };
        let line = serde_json::to_string(&said).unwrap();
        assert!(line.starts_with(r#"{"type":"services","services":[{"program":"olo-pool""#), "{line}");
        assert_eq!(serde_json::from_str::<Said>(&line).unwrap(), said);
    }

    #[test]
    fn each_has_a_name() {
        assert_eq!(about("olo-passwords").0, "SSH Agent");
        assert_eq!(about("something-else"), ("something-else", ""));
        assert_eq!(socket_path(Path::new("/run/user/1000"), "wayland-1"), PathBuf::from("/run/user/1000/olo-session-wayland-1.sock"));
    }
}
