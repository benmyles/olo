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
//! asked for its password (Ben, 2026-11-14: "when i first login after a
//! restart i'm being asked for my keyring password to unlock it twice").
//!
//! It waits for the session's olo-secrets to take the name (it may be
//! asking for the password: as long as that takes), and ends; what asked
//! then gets its answer from Olo's. If Olo's keyring is locked (the
//! person said Not Now, or it couldn't be opened), it asks olo-secrets to
//! unlock it (`locked.rs`): the password is asked for again, unless the
//! person said Not Now in the last two minutes, when this request goes
//! unanswered (the notification that it's locked stays, with Unlock).
//! With no olo-secrets in the session for 15 s (olo-session restarts it
//! when it ends), it ends unanswered too. It never starts GNOME Keyring:
//! nothing of GNOME's is (Ben, 2026-11-15: "we should be 100%
//! independent of gnome").

use std::{process::ExitCode, sync::mpsc, time::Duration};

use crate::gnome::NAME;

/// How long the session may have no olo-secrets before this request goes
/// unanswered (olo-session starts it again within a second or two).
const ALONE: Duration = Duration::from_secs(15);

pub fn run() -> ExitCode {
    let Ok(conn) = zbus::blocking::Connection::session() else { return ExitCode::FAILURE };
    let (tx, rx) = mpsc::channel();
    // (Listened for before looking, so the moment it's taken isn't missed.)
    if let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&conn)
        && let Ok(changes) = dbus.receive_name_owner_changed_with_args(&[(0, NAME)])
    {
        std::thread::spawn(move || {
            for c in changes {
                if c.args().is_ok_and(|a| a.new_owner().is_some()) {
                    let _ = tx.send(());
                    return;
                }
            }
        });
    }
    if crate::gnome::owner(&conn).is_some() {
        return ExitCode::SUCCESS;
    }
    let mut alone = Duration::ZERO;
    let step = Duration::from_millis(500);
    loop {
        match rx.recv_timeout(step) {
            Ok(()) => return ExitCode::SUCCESS,
            Err(mpsc::RecvTimeoutError::Disconnected) => return ExitCode::FAILURE,
            // (Only while something waits for the keyring: a few seconds
            // at login, a moment at an upgrade.)
            Err(mpsc::RecvTimeoutError::Timeout) => match ours() {
                Ours::Coming => alone = Duration::ZERO,
                // Locked: olo-secrets asked to open it, as a program asked
                // (the password asked for, unless the person said Not Now a
                // moment ago); it takes the name once it's open. Never
                // ended in silence: while it stays locked, a notification
                // says so (locked.rs).
                Ours::Declined => match crate::locked::ask_to_unlock(&conn, false) {
                    Ok(true) => return ExitCode::SUCCESS,
                    Ok(false) => {
                        tracing::info!("secrets: Olo's keyring stays locked (Not Now); this request goes unanswered");
                        return ExitCode::FAILURE;
                    }
                    Err(e) => {
                        tracing::warn!("secrets: couldn't ask olo-secrets to unlock: {e}");
                        return ExitCode::FAILURE;
                    }
                },
                Ours::Gone => {
                    alone += step;
                    if alone >= ALONE {
                        tracing::warn!("secrets: no olo-secrets in the session to answer for the keyring; this request goes unanswered");
                        return ExitCode::FAILURE;
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
