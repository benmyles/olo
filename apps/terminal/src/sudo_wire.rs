//! How `olo-sudo` (sudo in Terminal's shells, olo_sudo.rs) and Terminal
//! (sudo.rs) talk: one JSON value per line over a Unix socket, the path
//! in the shells' `OLO_TERMINAL_SUDO`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Where the socket's path is, in the shells' environment.
pub const SOCKET_ENV: &str = "OLO_TERMINAL_SUDO";

/// olo-sudo → Terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ask {
    /// sudo needs a password to run `args` (what followed `sudo`) from
    /// `cwd`. `tty`: it has a terminal, so a person could type it there.
    /// `quiet`: `sudo -n`, never to ask anyone: go ahead only if
    /// approved without asking, else `Reply::Pass` (and sudo fails, as
    /// `-n` does). Answered with `Reply::Go` or `Reply::Pass`.
    Begin {
        args: Vec<String>,
        cwd: Option<PathBuf>,
        tty: bool,
        #[serde(default)]
        quiet: bool,
    },
    /// What PAM said ("Place your finger on the fingerprint reader").
    Info(String),
    /// What went wrong ("Sorry, try again.").
    Error(String),
    /// sudo waits for this answer (a password unless it says otherwise).
    Prompt(String),
    /// sudo said yes, or no.
    Done(bool),
}

/// Terminal → olo-sudo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reply {
    /// Asking in a window.
    Go,
    /// Ask in the terminal, as sudo does.
    Pass,
    /// The answer to the prompt.
    Answer(String),
    /// The person said no.
    Cancel,
}

/// One message as its line.
pub fn line(value: &impl Serialize) -> Vec<u8> {
    let mut v = serde_json::to_vec(value).unwrap_or_default();
    v.push(b'\n');
    v
}
