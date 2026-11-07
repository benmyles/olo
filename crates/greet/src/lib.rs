//! Olo's login screen, in GDM's place: what its parts say to each other.
//!
//! * `olo-login --daemon` (root, the display manager) owns the seat. It
//!   starts the greeter's session (app_server and `olo-greeter`, as the
//!   greeter's own user) and answers it on a socket
//!   ([`socket_path`]: [`Request`] in, [`Event`] out, a JSON object a
//!   line).
//! * For each login it starts workers, one process a way in
//!   (`olo-login --worker SERVICE USER`: the password, a fingerprint, a
//!   face), as GDM does, each with its own PAM conversation
//!   ([`pam::Pam`]). It relays their prompts and takes the first that
//!   says yes, ending the others.
//! * That worker opens the person's PAM and logind session, becomes
//!   them and starts their desktop's session. When it ends, the daemon
//!   shows the greeter again.
//!
//! The worker and the daemon speak [`ToWorker`] and [`FromWorker`] over
//! the worker's standard input and output.

pub mod pam;
pub mod sessions;
pub mod users;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use sessions::Session;
pub use users::User;

/// What the greeter asks of the daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Who can log in ([`Event::Users`]).
    Users,
    /// The desktops a session may be ([`Event::Sessions`]).
    Sessions,
    /// Log `user` in: every way in that person has is tried at once.
    Begin { user: String },
    /// The answer to the password worker's question (None: given up).
    Answer { text: Option<String> },
    /// Authenticated: start session `session` (a [`Session::id`]).
    Start { session: String },
    /// Stop logging in (another person chosen, Escape).
    Cancel,
    /// Restart, power off or sleep the computer.
    Power { action: Power },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Power {
    Restart,
    PowerOff,
    Suspend,
}

/// What the daemon tells the greeter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Users {
        users: Vec<User>,
    },
    Sessions {
        sessions: Vec<Session>,
    },
    /// A way in says something: a question for the password field (a
    /// secret one or not), or words to show ("Place your finger on the
    /// reader").
    Prompt {
        by: Way,
        kind: PromptKind,
        text: String,
    },
    /// A way in gave up or was refused ("Sorry, that didn't work").
    Failed {
        by: Way,
        reason: String,
    },
    /// The person is in: [`Request::Start`] starts their session.
    Authenticated {
        by: Way,
    },
    /// Their session is starting: the greeter goes.
    Started,
    /// Something went wrong outside the person's answers.
    Error {
        reason: String,
    },
}

/// A way in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Way {
    Password,
    Fingerprint,
    Face,
}

impl Way {
    /// Its PAM service, as `data/pam` installs it.
    pub fn service(self) -> &'static str {
        match self {
            Way::Password => "olo-login",
            Way::Fingerprint => "olo-login-fingerprint",
            Way::Face => "olo-login-face",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptKind {
    /// A question whose answer isn't shown (a password).
    Secret,
    /// A question whose answer is shown.
    Visible,
    /// Words to show.
    Info,
    /// Words saying something went wrong.
    Error,
}

/// What the daemon tells a worker (a line of JSON on its input).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToWorker {
    /// The answer to its question (None: give up).
    Answer { text: Option<String> },
    /// Start the person's session: `command` with `env` added (the
    /// session's own: XDG_SESSION_DESKTOP and the rest).
    Start { command: Vec<String>, env: Vec<(String, String)> },
}

/// What a worker tells the daemon (a line of JSON on its output).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromWorker {
    Prompt {
        kind: PromptKind,
        text: String,
    },
    Authenticated,
    Failed {
        reason: String,
    },
    /// The session started (its process).
    Started {
        pid: i32,
    },
    /// The session ended (the person logged out).
    Ended,
}

/// Where the daemon listens for the greeter: `OLO_LOGIN_SOCKET` (given
/// to the greeter's session), else the system's place for it.
pub fn socket_path() -> PathBuf {
    std::env::var_os("OLO_LOGIN_SOCKET").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/run/olo-login/greeter.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The greeter's and the workers' words, as a line of JSON each.
    #[test]
    fn words_are_a_line_of_json() {
        let r = Request::Begin { user: "ben".into() };
        let line = serde_json::to_string(&r).unwrap();
        assert_eq!(line, r#"{"type":"begin","user":"ben"}"#);
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), r);
        let e = Event::Prompt { by: Way::Fingerprint, kind: PromptKind::Info, text: "Place your finger on the reader".into() };
        assert_eq!(serde_json::from_str::<Event>(&serde_json::to_string(&e).unwrap()).unwrap(), e);
        let w = ToWorker::Start { command: vec!["olo-session".into(), "--tty-udev".into()], env: vec![("XDG_SESSION_DESKTOP".into(), "olo".into())] };
        assert_eq!(serde_json::from_str::<ToWorker>(&serde_json::to_string(&w).unwrap()).unwrap(), w);
        assert_eq!(Way::Fingerprint.service(), "olo-login-fingerprint");
    }
}
