//! `olo-secrets --activated`: what D-Bus starts for
//! `org.freedesktop.secrets` while nobody answers for it, in an Olo
//! session (olo-session describes it so in the runtime folder's
//! services, which the bus reads before the system's: graphical.rs's
//! `name_keyring`).
//!
//! Without it, a program asking for a secret at login before olo-secrets
//! had the name (Token Pool's service and DiskBackup's look up their
//! keys as they start), or in the moment olo-secrets became a new
//! version, had D-Bus start GNOME Keyring in Olo's place, locked, which
//! asked for its password; then olo-secrets, finding it running, read
//! its keyrings, asking again for each (Ben, 2026-11-14: "when i first
//! login after a restart i'm being asked for my keyring password to
//! unlock it twice. it should only need it once to unlock").
//!
//! With Olo's keyring on, this waits for the session's olo-secrets to
//! take the name (it may be asking for the password: as long as that
//! takes), and ends; what asked then gets its answer from Olo's. With
//! Olo's turned off (Preferences ▸ Security), or no olo-secrets in the
//! session to answer for 15 s (olo-session gave up on it), it becomes
//! GNOME Keyring, as the system's service would have started it, so
//! programs still have a keyring. If olo-secrets couldn't take over (the
//! person said Not Now to the password), it ends without an answer: the
//! keyring stays locked, and nothing else asks.

use std::{process::ExitCode, sync::mpsc, time::Duration};

use crate::gnome::NAME;

/// What ends the wait.
enum Then {
    /// Someone answers for the keyring (olo-secrets).
    Taken,
    /// Olo's keyring was turned off.
    Off,
}

/// How long the session may have no olo-secrets before GNOME Keyring is
/// started instead (olo-session starts it again within a second or two).
const ALONE: Duration = Duration::from_secs(15);

pub fn run() -> ExitCode {
    let Ok(conn) = zbus::blocking::Connection::session() else { return ExitCode::FAILURE };
    let (tx, rx) = mpsc::channel();
    // (Listened for before looking, so the moment it's taken isn't missed.)
    if let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&conn)
        && let Ok(changes) = dbus.receive_name_owner_changed_with_args(&[(0, NAME)])
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for c in changes {
                if c.args().is_ok_and(|a| a.new_owner().is_some()) {
                    let _ = tx.send(Then::Taken);
                    return;
                }
            }
        });
    }
    if crate::gnome::owner(&conn).is_some() {
        return ExitCode::SUCCESS;
    }
    if watch_setting(tx) == Some(false) {
        return become_gnome_keyring();
    }
    let mut alone = Duration::ZERO;
    let step = Duration::from_millis(500);
    loop {
        match rx.recv_timeout(step) {
            Ok(Then::Taken) => return ExitCode::SUCCESS,
            Ok(Then::Off) => return become_gnome_keyring(),
            Err(mpsc::RecvTimeoutError::Disconnected) => return ExitCode::FAILURE,
            // (Only while something waits for the keyring: a few seconds
            // at login, a moment at an upgrade.)
            Err(mpsc::RecvTimeoutError::Timeout) => match ours() {
                Ours::Coming => alone = Duration::ZERO,
                Ours::Declined => {
                    tracing::info!("secrets: Olo's keyring stays locked; nothing answers");
                    return ExitCode::FAILURE;
                }
                Ours::Gone => {
                    alone += step;
                    if alone >= ALONE {
                        tracing::warn!("secrets: no olo-secrets in the session to answer for the keyring: GNOME Keyring instead");
                        return become_gnome_keyring();
                    }
                }
            },
        }
    }
}

/// The session's olo-secrets, as its status says.
enum Ours {
    /// Running, taking over (asking for the password, opening the store).
    Coming,
    /// Running, but it couldn't take over (the password not given).
    Declined,
    /// Not running.
    Gone,
}

fn ours() -> Ours {
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Seen {
        pid: u32,
        serving: bool,
        error: Option<String>,
    }
    let seen: Seen = std::fs::read(crate::status_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let running = seen.pid != 0 && seen.pid != std::process::id() && std::fs::read_to_string(format!("/proc/{}/comm", seen.pid)).is_ok_and(|c| c.trim() == "olo-secrets");
    match (running, seen.serving, seen.error) {
        (false, ..) => Ours::Gone,
        (true, false, Some(_)) => Ours::Declined,
        _ => Ours::Coming,
    }
}

/// Whether Olo's keyring is on, as app_server's settings say now (None
/// if they can't be read), and `Off` sent on `tx` if it's turned off
/// while this waits.
fn watch_setting(tx: mpsc::Sender<Then>) -> Option<bool> {
    olo_protocol::socket_path()?;
    let wanted = |s: &olo_protocol::Settings| s.security.passwords.secret_service;
    let now = olo_protocol::Client::connect().ok().and_then(|mut c| match c.request(&olo_protocol::Request::GetSettings) {
        Ok(olo_protocol::Reply::Settings { settings }) => Some(wanted(&settings)),
        _ => None,
    });
    if let Ok(events) = olo_protocol::Client::connect().and_then(|c| c.into_events_of(&[olo_protocol::EventKind::Settings])) {
        std::thread::spawn(move || {
            for event in events {
                if let Ok(olo_protocol::Event::SettingsChanged { settings }) = event
                    && !wanted(&settings)
                {
                    let _ = tx.send(Then::Off);
                    return;
                }
            }
        });
    }
    now
}

/// GNOME Keyring, started as the system's description of the service
/// says (the first in the data folders, never the runtime folder's,
/// which is this): this process becomes it.
fn become_gnome_keyring() -> ExitCode {
    let Some(exec) = system_exec() else {
        tracing::warn!("secrets: no other keyring service is described on this computer");
        return ExitCode::FAILURE;
    };
    let mut words = exec.split_whitespace();
    let Some(program) = words.next() else { return ExitCode::FAILURE };
    use std::os::unix::process::CommandExt as _;
    let e = std::process::Command::new(program).args(words).exec();
    tracing::warn!("secrets: couldn't start {program}: {e}");
    ExitCode::FAILURE
}

/// The `Exec` of the keyring service the computer describes for D-Bus:
/// `$XDG_DATA_HOME`'s, then each of `$XDG_DATA_DIRS`' (by default
/// `/usr/local/share` and `/usr/share`), not this one's.
fn system_exec() -> Option<String> {
    let home = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()).map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")));
    let dirs = std::env::var("XDG_DATA_DIRS").ok().filter(|d| !d.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let mine = std::env::current_exe().ok();
    home.into_iter().chain(dirs.split(':').map(std::path::PathBuf::from)).find_map(|dir| {
        let text = std::fs::read_to_string(dir.join("dbus-1/services").join(format!("{NAME}.service"))).ok()?;
        exec_of(&text).filter(|exec| mine.as_ref().is_none_or(|m| exec.split_whitespace().next() != m.to_str()) && !exec.contains("olo-secrets"))
    })
}

/// A D-Bus service description's `Exec`, if it's for the keyring.
fn exec_of(text: &str) -> Option<String> {
    let names = text.lines().any(|l| l.trim() == format!("Name={NAME}"));
    let exec = text.lines().find_map(|l| l.trim().strip_prefix("Exec=")).map(str::to_owned);
    exec.filter(|_| names)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The system's description gives GNOME Keyring's command; Olo's own
    /// (this) isn't taken for it.
    #[test]
    fn the_systems_keyring_is_read_from_its_description() {
        let fedora = "[D-BUS Service]\nName=org.freedesktop.secrets\nExec=/usr/bin/gnome-keyring-daemon --start --foreground --components=secrets\n";
        assert_eq!(exec_of(fedora).as_deref(), Some("/usr/bin/gnome-keyring-daemon --start --foreground --components=secrets"));
        assert_eq!(exec_of("[D-BUS Service]\nName=org.example.Other\nExec=/usr/bin/other\n"), None);
        let dir = std::env::temp_dir().join(format!("olo-secrets-activated-{}", std::process::id()));
        let services = dir.join("share/dbus-1/services");
        std::fs::create_dir_all(&services).unwrap();
        std::fs::write(services.join("org.freedesktop.secrets.service"), fedora).unwrap();
        let ours = dir.join("home/dbus-1/services");
        std::fs::create_dir_all(&ours).unwrap();
        std::fs::write(ours.join("org.freedesktop.secrets.service"), "[D-BUS Service]\nName=org.freedesktop.secrets\nExec=/home/me/.local/bin/olo-secrets --activated\n").unwrap();
        // SAFETY: this test alone reads these.
        unsafe {
            std::env::set_var("XDG_DATA_HOME", dir.join("home"));
            std::env::set_var("XDG_DATA_DIRS", dir.join("share"));
        }
        assert_eq!(system_exec().as_deref(), Some("/usr/bin/gnome-keyring-daemon --start --foreground --components=secrets"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
