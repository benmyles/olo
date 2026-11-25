//! The Token Pool on a Rex host: the pool's own gateway and proxy, run
//! for one account there by `olo-pool-host` (`src/bin`), with no
//! desktop under it (the crate without its `desktop` feature), and
//! what the desktop's pool says to it.
//!
//! They speak over a socket in the account's own state folder
//! (`pool-host.sock`, the account's alone: what may speak to it is
//! whoever may read the account's files), a line of JSON asked and a
//! line answered ([`Ask`]). The desktop reaches it over SSH, through
//! the host's own `olo-pool-host ctl`, which passes its input to the
//! socket and the answer back: no port, no token.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::gateway::{Carried, Shared};

/// What the host's program speaks: raised when the desktop must put a
/// newer one there before speaking to it.
pub const VERSION: u32 = 1;

/// Where the service listens for the desktop's pool.
pub fn socket() -> PathBuf {
    olo_xdg::state_dir().join("pool-host.sock")
}

/// One thing asked of a host's pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Ask {
    /// How it stands ([`Shared::hosted_status`]).
    Status,
    /// The accounts as they are to be there ([`Shared::carried`]).
    Carry { carried: Carried },
    /// An account's sign-in for a borrower ([`Shared::lend`]).
    Lend { id: String, at_least: i64 },
    /// An account's sign-in given back whole ([`Shared::give_back`]).
    GiveBack { id: String, to: String },
    /// The service ends (before it is removed, or replaced).
    Stop,
    /// The version the account's folder now links takes over, beside this
    /// one, with nothing under way cut ([`hand_over`]): answered
    /// `{"upgrading": true}`, or an error where it couldn't start.
    Upgrade,
}

/// Set for a copy started by [`hand_over`]: when it hands over in turn it
/// ends once it has finished, the service's own process (its keeper)
/// following the newer copy.
pub const KEPT_ENV: &str = "OLO_POOL_HOST_KEPT";

/// Answers one asking.
fn answer(shared: &Arc<Shared>, ask: Ask) -> Value {
    let failed = |why: String| json!({"error": why});
    match ask {
        Ask::Status => shared.hosted_status(),
        Ask::Carry { carried } => shared.carried(carried).unwrap_or_else(failed),
        Ask::Lend { id, at_least } => {
            let s = shared.clone();
            match shared.wait(async move { s.lend(&id, at_least).await }) {
                Some(Ok(credential)) => json!({"credential": credential}),
                Some(Err(why)) => failed(why),
                None => failed("the pool there didn’t answer in time".into()),
            }
        }
        Ask::GiveBack { id, to } => {
            let s = shared.clone();
            match shared.wait(async move { s.give_back(&id, &to).await }) {
                Some(Ok(credential)) => json!({"credential": credential}),
                Some(Err(why)) => failed(why),
                None => failed("the pool there didn’t answer in time".into()),
            }
        }
        Ask::Stop => {
            shared.flush();
            json!({"stopping": true})
        }
        // (Done once answered: `serve`'s loop.)
        Ask::Upgrade => json!({"upgrading": true}),
    }
}

/// The new version put beside this one (`Ask::Upgrade`, as the desktop's
/// pool upgrades the host's): started from the program the account's
/// folder now links (`olo-pool-host` beside this one's file, which the
/// upgrade removed), handed the listening sockets, the gateway's and the
/// proxy's (`Drain::for_a_copy`), and this socket's name, so it answers
/// every new request and asking at once; this one retires and finishes
/// what it has under way, streams too, then ends, or, where it is the
/// service's own process, becomes its keeper (`olo-pool-host keep`):
/// systemd follows that process, and the copy is its child. Every stream
/// through a host was cut at each upgrade of the desktop's pool, which
/// stopped the program and restarted its service ("API Error: Connection
/// lost mid-response", Ben, 2026-11-25). The new copy's pid.
fn hand_over(shared: &Arc<Shared>) -> Result<u32, String> {
    use std::os::{fd::AsRawFd as _, unix::process::CommandExt as _};
    let here = std::env::current_exe().map_err(|e| e.to_string())?;
    let next = PathBuf::from(here.to_string_lossy().trim_end_matches(" (deleted)")).with_file_name("olo-pool-host");
    let listeners = shared.drain.for_a_copy().ok_or("nothing listens to hand over")?;
    // (The asking socket's name is the copy's from now: it won't start
    // while something answers there.)
    let _ = std::fs::remove_file(socket());
    let finishing = shared.drain.finishing();
    shared.flush();
    let mut cmd = std::process::Command::new(&next);
    cmd.arg("serve").env(crate::drain::LISTENERS_ENV, listeners).env(KEPT_ENV, "1");
    if let Some(fd) = &finishing {
        cmd.env(crate::drain::FINISHING_ENV, fd.as_raw_fd().to_string());
    }
    let started = cmd.spawn();
    drop(finishing);
    shared.drain.sealed();
    let child = match started {
        Ok(child) => child,
        Err(e) => {
            shared.drain.finished();
            // (Asked here again, as before.)
            let _ = serve(shared.clone());
            return Err(format!("the new version couldn’t start: {e}"));
        }
    };
    shared.drain.retire();
    let (shared, kept) = (shared.clone(), std::env::var_os(KEPT_ENV).is_some());
    let _ = std::thread::Builder::new().name("pool-host-handed-over".into()).spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        while shared.drain.busy() {
            std::thread::sleep(Duration::from_millis(250));
        }
        shared.drain.finished();
        if kept {
            std::process::exit(0);
        }
        let e = std::process::Command::new(&next).arg("keep").exec();
        eprintln!("olo-pool-host: couldn’t become the keeper ({e}); ending");
        std::process::exit(0);
    });
    Ok(child.id())
}

/// Listens on [`socket`] and answers the desktop's pool, on a thread of
/// its own, for as long as the service runs. An asking to stop ends the
/// process once it is answered.
pub fn serve(shared: Arc<Shared>) -> std::io::Result<()> {
    let path = socket();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // (One left by a service that ended badly: nobody listens on it.)
    if UnixStream::connect(&path).is_ok() {
        return Err(std::io::Error::new(std::io::ErrorKind::AddrInUse, "a Token Pool already serves this account"));
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    std::thread::Builder::new().name("pool-host".into()).spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            let mut line = String::new();
            if BufReader::new(&stream).read_line(&mut line).is_err() {
                continue;
            }
            let (said, stop, upgrade) = match serde_json::from_str::<Ask>(line.trim()) {
                Ok(ask) => {
                    let (stop, upgrade) = (matches!(ask, Ask::Stop), matches!(ask, Ask::Upgrade));
                    (answer(&shared, ask), stop, upgrade)
                }
                Err(e) => (json!({"error": format!("not understood: {e}")}), false, false),
            };
            // (An upgrade is said to have begun only once the new version
            // has: the asker restarts the service otherwise.)
            let said = if upgrade {
                match hand_over(&shared) {
                    Ok(pid) => json!({"upgrading": true, "pid": pid}),
                    Err(why) => json!({"error": why}),
                }
            } else {
                said
            };
            let _ = stream.write_all(format!("{said}\n").as_bytes());
            let _ = stream.flush();
            if stop {
                let _ = std::fs::remove_file(socket());
                std::process::exit(0);
            }
            if upgrade {
                // (This socket is the copy's now: nothing more is heard here.)
                return;
            }
        }
    })?;
    Ok(())
}

/// One asking put to the service on this machine, and its answer: what
/// `olo-pool-host ctl` does with its input.
pub fn ask_here(line: &str) -> Result<String, String> {
    let mut stream = UnixStream::connect(socket()).map_err(|e| format!("The Token Pool isn’t running here ({e})."))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(120)));
    stream.write_all(format!("{}\n", line.trim()).as_bytes()).map_err(|e| e.to_string())?;
    let mut said = String::new();
    BufReader::new(&stream).read_line(&mut said).map_err(|e| e.to_string())?;
    if said.trim().is_empty() { Err("The Token Pool here ended before it answered.".into()) } else { Ok(said) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the desktop's pool asks over SSH reads as written: the upgrade
    /// it asks first (a program from before answers "not understood", and
    /// the service is stopped and restarted as it was), and a stop.
    #[test]
    fn the_desktops_askings_read_as_written() {
        assert!(matches!(serde_json::from_str::<Ask>(r#"{"do":"upgrade"}"#), Ok(Ask::Upgrade)));
        assert!(matches!(serde_json::from_str::<Ask>(r#"{"do":"stop"}"#), Ok(Ask::Stop)));
        assert!(serde_json::from_str::<Ask>(r#"{"do":"reboot"}"#).is_err());
    }
}
