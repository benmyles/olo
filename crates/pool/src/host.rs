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
}

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
    }
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
            let (said, stop) = match serde_json::from_str::<Ask>(line.trim()) {
                Ok(ask) => {
                    let stop = matches!(ask, Ask::Stop);
                    (answer(&shared, ask), stop)
                }
                Err(e) => (json!({"error": format!("not understood: {e}")}), false),
            };
            let _ = stream.write_all(format!("{said}\n").as_bytes());
            let _ = stream.flush();
            if stop {
                let _ = std::fs::remove_file(socket());
                std::process::exit(0);
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
