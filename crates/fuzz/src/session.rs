//! A headless session (`tools/dev/tsession`'s): its environment, a
//! connection to its app_server, starting programs in it and reloading
//! it. Starting and stopping a whole session stay tsession's.

use std::{
    collections::HashMap,
    os::unix::process::CommandExt as _,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

use olo_protocol::{Client, ClientError, Reply, Request};

/// Where tsession keeps each session (its logs, env.json, pid files).
pub const BASE: &str = "/var/tmp/olo-dev";

pub struct Session {
    pub name: String,
    pub dir: PathBuf,
    pub env: HashMap<String, String>,
    pub socket: PathBuf,
    /// The build its programs come from: the session's own
    /// (`target/release`, or `target/debug` for a debug session).
    pub target: PathBuf,
}

impl Session {
    pub fn open(name: &str) -> Result<Session, String> {
        let dir = Path::new(BASE).join(name);
        let text = std::fs::read_to_string(dir.join("env.json")).map_err(|e| format!("no session {name:?} ({e}): tsession start {name} …"))?;
        let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{name}'s env.json: {e}"))?;
        let env: HashMap<String, String> =
            json.as_object().map(|o| o.iter().filter(|(k, _)| *k != "pid" && *k != "sockets").filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect()).unwrap_or_default();
        let socket = PathBuf::from(env.get("OLO_SOCKET").ok_or("no OLO_SOCKET in env.json")?);
        // (The build its session runs from: olo-session's, or app_server's.)
        let leader = env.get("OLO_SESSION_PID").and_then(|p| p.parse::<u32>().ok()).or_else(|| json.get("pid").and_then(serde_json::Value::as_u64).map(|p| p as u32));
        let debug = leader.and_then(|p| std::fs::read_link(format!("/proc/{p}/exe")).ok()).is_some_and(|exe| exe.starts_with(root().join("target/debug")));
        Ok(Session { name: name.to_owned(), dir, env, socket, target: root().join(if debug { "target/debug" } else { "target/release" }) })
    }

    /// Start `program` in the session, as `tsession run` does: the
    /// session's environment, its log in the session's folder, a pid file
    /// for `tsession stop`, in a process group of its own. Doesn't wait.
    pub fn spawn(&self, program: &str, args: &[&str]) -> Result<(), String> {
        self.spawn_with(program, args, &[])
    }

    /// `spawn`, with `extra` in its environment too.
    pub fn spawn_with(&self, program: &str, args: &[&str], extra: &[(&str, &str)]) -> Result<(), String> {
        let exe = self.target.join(program);
        let exe = if exe.exists() { exe } else { PathBuf::from(program) };
        let log = std::fs::File::create(self.dir.join(format!("{program}.log"))).map_err(|e| e.to_string())?;
        let mute = self.dir.join("no-sound");
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(&mute);
        let mut cmd = Command::new(&exe);
        cmd.args(args)
            .envs(&self.env)
            .env("OLO_PASSWORDS_KEYFILE", self.dir.join("passwords.key"))
            .env("HISTFILE", self.dir.join("bash_history"))
            .env("RUST_LOG", std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
            .env("RUST_LIB_BACKTRACE", "1")
            .env("PULSE_SERVER", format!("unix:{}", mute.display()))
            .env("PATH", format!("{}:{}", self.target.display(), std::env::var("PATH").unwrap_or_default()))
            .envs(extra.iter().copied())
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log)
            .process_group(0);
        reap();
        let child = cmd.spawn().map_err(|e| format!("{}: {e}", exe.display()))?;
        let _ = std::fs::write(self.dir.join(format!("{program}.{}.pid", child.id())), child.id().to_string());
        CHILDREN.lock().unwrap_or_else(|e| e.into_inner()).push((self.dir.clone(), child));
        Ok(())
    }

    /// app_server restarted in place, as an upgrade does (olo-session's
    /// SIGHUP: `tsession reload`).
    pub fn reload(&self) -> Result<(), String> {
        let pid: i32 = self.env.get("OLO_SESSION_PID").and_then(|p| p.parse().ok()).ok_or("not a --session session: nothing to reload")?;
        // SAFETY: a signal to the session's olo-session.
        if unsafe { libc::kill(pid, libc::SIGHUP) } != 0 {
            return Err(format!("olo-session {pid}: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }

    /// Bytes of app_server's log so far, and the log (it outlives a restart).
    pub fn log_path(&self) -> PathBuf {
        self.dir.join("app_server.log")
    }
}

/// The programs the fuzzers started, each with its session's folder, kept
/// to be waited for once they've quit (else each would linger as a zombie
/// until this process ends).
static CHILDREN: Mutex<Vec<(PathBuf, Child)>> = Mutex::new(Vec::new());

/// Those of the programs started that have quit, waited for.
pub fn reap() {
    CHILDREN.lock().unwrap_or_else(|e| e.into_inner()).retain_mut(|(_, c)| matches!(c.try_wait(), Ok(None)));
}

/// The programs started in the session at `dir`, ended with it (once
/// tsession has stopped it, they've gone): any still running are told to
/// go, killed after 2 s, and all waited for.
pub fn end_children(dir: &Path) {
    let mine: Vec<Child> = {
        let mut kids = CHILDREN.lock().unwrap_or_else(|e| e.into_inner());
        let (mine, rest) = std::mem::take(&mut *kids).into_iter().partition(|(d, _)| d == dir);
        *kids = rest;
        mine.into_iter().map(|(_, c)| c).collect()
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    for mut c in mine {
        if matches!(c.try_wait(), Ok(None)) {
            // SAFETY: SIGTERM to the process group of a program we started
            // (each in a group of its own).
            unsafe { libc::kill(-(c.id() as i32), libc::SIGTERM) };
            while matches!(c.try_wait(), Ok(None)) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            if matches!(c.try_wait(), Ok(None)) {
                // SAFETY: as above, SIGKILL.
                unsafe { libc::kill(-(c.id() as i32), libc::SIGKILL) };
            }
        }
        let _ = c.wait();
    }
}

/// The repository (this crate is `crates/fuzz`).
pub fn root() -> PathBuf {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    here.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_else(|| here.to_path_buf())
}

/// A connection to a session's app_server, kept (it answers requests in
/// order on one), made again when it goes (app_server restarted).
pub struct Ipc {
    socket: PathBuf,
    client: Option<Client>,
}

impl Ipc {
    pub fn new(socket: &Path) -> Ipc {
        Ipc { socket: socket.to_owned(), client: None }
    }

    /// Drop the connection: the next request makes a new one (after a
    /// reload: the old app_server answers on the old one a moment longer).
    pub fn reset(&mut self) {
        self.client = None;
    }

    /// `req`'s reply, an error reply too (`Reply::Error`); Err if
    /// app_server can't be reached.
    pub fn call(&mut self, req: &Request) -> Result<Reply, String> {
        for attempt in 0..2 {
            if self.client.is_none() {
                self.client = Some(Client::connect_to(&self.socket).map_err(|e| format!("app_server didn't answer {}: {e}", kind(req)))?);
            }
            let client = self.client.as_mut().expect("connected");
            let sent = client.send(req).and_then(|()| {
                loop {
                    match client.read()? {
                        Reply::Event { .. } => continue,
                        reply => return Ok(reply),
                    }
                }
            });
            match sent {
                Ok(reply) => return Ok(reply),
                // (Gone: app_server restarted. Once more, on a new one.)
                Err(ClientError::Closed | ClientError::Io(_)) if attempt == 0 => self.client = None,
                Err(e) => {
                    self.client = None;
                    return Err(format!("app_server didn't answer {}: {e}", kind(req)));
                }
            }
        }
        Err(format!("app_server closed the connection on {}", kind(req)))
    }
}

/// A request's name, for messages.
pub fn kind(req: &Request) -> String {
    serde_json::to_value(req).ok().and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_owned)).unwrap_or_default()
}
