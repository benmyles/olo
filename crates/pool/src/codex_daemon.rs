//! Codex's app-server daemon, kept on the pool's settings. Codex's
//! interactive sessions run their turns in a daemon of its own that lives
//! for days (`codex app-server`, its pid in
//! `~/.codex/app-server-daemon/daemon.pid`), and that reads the `.env` the
//! pool edits (`clients`) only as it starts: a daemon from before the pool
//! was turned on goes straight to OpenAI, and one from before it was
//! turned off still goes through the pool. So a daemon started before
//! `.env` last changed is restarted, once none of its sessions is in the
//! middle of a turn (asked over its control socket, the app-server's
//! JSON-RPC over a WebSocket), with Codex's own `codex app-server daemon
//! restart` (the daemon's start is Codex's to make: its pid file, locks
//! and updater), run as the daemon was (its directory and environment,
//! which its sessions' commands get theirs from). Codex's windows
//! reconnect to the new one by themselves, as after an update of Codex's.

use std::{
    ffi::OsStr,
    io,
    os::unix::{ffi::OsStrExt, net::UnixStream},
    path::PathBuf,
    time::{Duration, SystemTime},
};

use serde_json::{Value, json};

use crate::clients::codex_dir;

/// A daemon this close after the change may have read it.
const SLACK: Duration = Duration::from_secs(2);

/// How long the daemon has to answer.
const PATIENCE: Duration = Duration::from_secs(5);

/// How long Codex has to restart it.
const RESTART: Duration = Duration::from_secs(60);

/// The running daemon.
#[derive(Debug, Clone, PartialEq)]
pub struct Daemon {
    pub pid: i32,
    /// When it started, from its start in clock ticks after boot (which
    /// the pid file records too, so a pid used again isn't taken for it).
    ticks: u64,
    pub started: SystemTime,
}

fn pid_file() -> PathBuf {
    codex_dir().join("app-server-daemon").join("daemon.pid")
}

fn control_socket() -> PathBuf {
    codex_dir().join("app-server-control").join("app-server-control.sock")
}

/// When process `pid` started: its start in ticks after boot, and the
/// time (None once it has exited, if not yet reaped).
fn start_of(pid: i32) -> Option<(u64, SystemTime)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // After the command's name (in parentheses, maybe with spaces), the
    // state is field 3; the start time field 22.
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    if *fields.first()? == "Z" {
        return None;
    }
    let ticks: u64 = fields.get(19)?.parse().ok()?;
    let boot: u64 = std::fs::read_to_string("/proc/stat").ok()?.lines().find_map(|l| l.strip_prefix("btime ")?.trim().parse().ok())?;
    // SAFETY: plain sysconf.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    Some((ticks, SystemTime::UNIX_EPOCH + Duration::from_secs(boot) + Duration::from_millis(ticks * 1000 / hz)))
}

/// Codex's daemon, if one is running.
pub fn running() -> Option<Daemon> {
    daemon_in(&pid_file())
}

/// The daemon `pid_file` names, if it's still that process and Codex's.
fn daemon_in(pid_file: &std::path::Path) -> Option<Daemon> {
    let v: Value = serde_json::from_slice(&std::fs::read(pid_file).ok()?).ok()?;
    let pid = i32::try_from(v["pid"].as_i64()?).ok()?;
    let ticks = v["processIdentity"]["startTicks"].as_u64()?;
    let (now_ticks, started) = start_of(pid)?;
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let codex = cmdline.split(|b| *b == 0).any(|a| a == b"app-server");
    (now_ticks == ticks && codex).then_some(Daemon { pid, ticks, started })
}

/// Whether `d` is still running (its pid not taken by another since).
fn alive(d: &Daemon) -> bool {
    start_of(d.pid).is_some_and(|(t, _)| t == d.ticks)
}

/// When Codex's settings for the pool last changed: its `.env`, else (the
/// pool took out the whole file) the pool's record of what it changed.
fn settings_changed() -> Option<SystemTime> {
    let modified = |p: PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    modified(codex_dir().join(".env")).or_else(|| modified(crate::clients::manifest_path()))
}

/// Whether `d` started before Codex's settings last changed, so hasn't
/// read them.
pub fn stale(d: &Daemon) -> bool {
    started_before(d, settings_changed())
}

fn started_before(d: &Daemon, changed: Option<SystemTime>) -> bool {
    changed.is_some_and(|changed| changed > d.started + SLACK)
}

/// A WebSocket to the daemon's control socket, initialized as a client.
pub struct Control {
    ws: tungstenite::WebSocket<UnixStream>,
    next: u64,
}

impl Control {
    pub fn open() -> io::Result<Control> {
        let stream = UnixStream::connect(control_socket())?;
        stream.set_read_timeout(Some(PATIENCE))?;
        stream.set_write_timeout(Some(PATIENCE))?;
        let (ws, _) = tungstenite::client::client("ws://localhost/", stream).map_err(|e| io::Error::other(format!("Codex's daemon: {e}")))?;
        let mut c = Control { ws, next: 1 };
        let info = json!({"clientInfo": {"name": "olo-pool", "title": "Olo Token Pool", "version": env!("CARGO_PKG_VERSION")}});
        c.call("initialize", info)?;
        c.send(json!({"method": "initialized"}))?;
        Ok(c)
    }

    fn send(&mut self, v: Value) -> io::Result<()> {
        self.ws.send(tungstenite::Message::text(v.to_string())).map_err(io::Error::other)
    }

    /// Ask `method`, and wait for its answer (notifications on the way
    /// passed over).
    pub fn call(&mut self, method: &str, params: Value) -> io::Result<Value> {
        let id = self.next;
        self.next += 1;
        self.send(json!({"id": id, "method": method, "params": params}))?;
        loop {
            let msg = self.ws.read().map_err(io::Error::other)?;
            let Ok(text) = msg.to_text() else { continue };
            let Ok(v) = serde_json::from_str::<Value>(text) else { continue };
            if v["id"] != id || v.get("method").is_some() {
                continue;
            }
            if let Some(e) = v.get("error") {
                return Err(io::Error::other(format!("Codex's daemon: {method}: {}", e["message"].as_str().unwrap_or("failed"))));
            }
            return Ok(v["result"].clone());
        }
    }

    /// Wait up to `within` for notification `method`, and its parameters.
    pub fn wait_for(&mut self, method: &str, within: Duration) -> io::Result<Value> {
        let until = std::time::Instant::now() + within;
        loop {
            let left = until.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, format!("no {method} from Codex's daemon")));
            }
            self.ws.get_ref().set_read_timeout(Some(left))?;
            let msg = self.ws.read().map_err(io::Error::other)?;
            let Ok(v) = msg.to_text().map(serde_json::from_str::<Value>) else { continue };
            if let Ok(v) = v
                && v["method"] == method
            {
                return Ok(v["params"].clone());
            }
        }
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        let _ = self.ws.close(None);
        let _ = self.ws.flush();
    }
}

/// Whether any of the daemon's sessions is in the middle of a turn.
pub fn busy() -> io::Result<bool> {
    let mut c = Control::open()?;
    let mut cursor: Option<String> = None;
    loop {
        let page = c.call("thread/loaded/list", json!({"cursor": cursor}))?;
        for id in page["data"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            let read = c.call("thread/read", json!({"threadId": id, "includeTurns": false}))?;
            if read["thread"]["status"]["type"] == "active" {
                return Ok(true);
            }
        }
        match page["nextCursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => return Ok(false),
        }
    }
}

/// Stop `d` (still the same process), and wait for it to go.
pub fn stop(d: &Daemon) -> io::Result<()> {
    if !alive(d) {
        return Ok(());
    }
    // SAFETY: plain kill(2) of the daemon, checked above to be it.
    if unsafe { libc::kill(d.pid, libc::SIGTERM) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < until {
        if !alive(d) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(io::Error::other("Codex's daemon didn't stop"))
}

/// Whether `key=value` in the daemon's environment came from the pool's
/// settings in `.env` (read by the Codex that started it), so is left for
/// the new one to read from `.env` as it is now.
fn from_the_pool(key: &[u8], value: &[u8]) -> bool {
    crate::clients::codex_env(crate::intercept::port()).iter().any(|(k, v)| k.as_bytes() == key && v.as_bytes() == value)
}

/// Restart `d` with Codex's own command, from the daemon's directory and
/// with its environment, and wait for the new one.
pub fn restart(d: &Daemon) -> io::Result<()> {
    if !alive(d) {
        return Ok(());
    }
    // The copy of Codex that runs it (kept by Codex's updater), else the
    // daemon's own program.
    let program = [Some(codex_dir().join("packages/app-server-daemon/current/bin/codex")), std::fs::read_link(format!("/proc/{}/exe", d.pid)).ok()]
        .into_iter()
        .flatten()
        .find(|p| p.is_file())
        .ok_or_else(|| io::Error::other("Codex's program for its daemon isn't there"))?;
    let environ = std::fs::read(format!("/proc/{}/environ", d.pid))?;
    let cwd = std::fs::read_link(format!("/proc/{}/cwd", d.pid)).ok().filter(|p| p.is_dir());
    let mut c = std::process::Command::new(program);
    c.args(["app-server", "daemon", "restart"])
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let mut has_home = false;
    for var in environ.split(|b| *b == 0) {
        let Some(eq) = var.iter().position(|b| *b == b'=') else { continue };
        let (k, v) = (&var[..eq], &var[eq + 1..]);
        if !from_the_pool(k, v) {
            has_home |= k == b"CODEX_HOME";
            c.env(OsStr::from_bytes(k), OsStr::from_bytes(v));
        }
    }
    if !has_home && let Some(home) = std::env::var_os("CODEX_HOME") {
        c.env("CODEX_HOME", home);
    }
    if let Some(cwd) = cwd {
        c.current_dir(cwd);
    }
    let child = c.spawn()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let out = rx.recv_timeout(RESTART).map_err(|_| io::Error::other("Codex didn't restart its daemon in time"))??;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        let why = why.lines().rfind(|l| !l.trim().is_empty() && !l.starts_with('{')).unwrap_or("").trim().to_owned();
        return Err(io::Error::other(format!("Codex couldn't restart its daemon ({}){}", out.status, if why.is_empty() { String::new() } else { format!(": {why}") })));
    }
    match running() {
        Some(new) if new.pid != d.pid => Ok(()),
        _ => Err(io::Error::other("Codex's daemon didn't come back")),
    }
}

/// What `follow` did.
#[derive(Debug, PartialEq)]
pub enum Followed {
    /// No daemon, or one on the current settings.
    Current,
    /// A daemon from before them, in the middle of a turn: later.
    Waiting,
    /// A daemon from before them (this pid), restarted.
    Restarted(i32),
}

/// Restart Codex's daemon if it started before its settings last changed
/// and isn't in the middle of a turn. Blocks (on the daemon's answer, at
/// most a few seconds).
pub fn follow() -> io::Result<Followed> {
    let Some(d) = running().filter(stale) else { return Ok(Followed::Current) };
    if busy()? {
        return Ok(Followed::Waiting);
    }
    restart(&d)?;
    Ok(Followed::Restarted(d.pid))
}

/// Keep Codex's daemon on the pool's settings, checking every `every`
/// (on a thread of its own).
pub fn keep_following(every: Duration) {
    std::thread::spawn(move || {
        let mut said = None;
        loop {
            match follow() {
                Ok(Followed::Restarted(pid)) => tracing::info!("pool: restarted Codex’s daemon ({pid}), which started before its settings for the pool"),
                Ok(Followed::Waiting) if said != Some(true) => {
                    tracing::info!("pool: Codex’s daemon started before its settings for the pool; restarting it when it’s between turns");
                    said = Some(true);
                }
                Err(e) if said != Some(false) => {
                    tracing::warn!("pool: Codex’s daemon started before its settings for the pool, and couldn’t be restarted: {e}");
                    said = Some(false);
                }
                Ok(Followed::Current) => said = None,
                _ => {}
            }
            std::thread::sleep(every);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_the_daemon_by_its_start() {
        let dir = std::env::temp_dir().join(format!("olo-codex-daemon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("daemon.pid");
        // A stand-in for Codex's daemon: a program called with `app-server`.
        let mut child = std::process::Command::new("/bin/sh").args(["-c", "sleep 30", "app-server"]).spawn().unwrap();
        let pid = child.id() as i32;
        let (ticks, started) = start_of(pid).unwrap();
        let age = SystemTime::now().duration_since(started).unwrap_or_default();
        assert!(age < Duration::from_secs(5), "{age:?}");
        let write = |ticks: u64| std::fs::write(&file, json!({"pid": pid, "processIdentity": {"startTicks": ticks}}).to_string()).unwrap();
        write(ticks);
        let d = daemon_in(&file).expect("the daemon");
        assert_eq!((d.pid, d.started), (pid, started));
        // Another process with its pid isn't it; nor is a program that isn't Codex's.
        write(ticks + 1);
        assert!(daemon_in(&file).is_none());
        std::fs::write(&file, json!({"pid": std::process::id(), "processIdentity": {"startTicks": start_of(std::process::id() as i32).unwrap().0}}).to_string()).unwrap();
        assert!(daemon_in(&file).is_none());
        // Settings changed just as it started may have been read; later ones not.
        assert!(!started_before(&d, None));
        assert!(!started_before(&d, Some(started + Duration::from_secs(1))));
        assert!(!started_before(&d, Some(started - Duration::from_secs(60))));
        assert!(started_before(&d, Some(started + Duration::from_secs(3))));
        stop(&d).unwrap();
        assert!(!alive(&d));
        let _ = child.wait();
        // Stopping one that's gone is done already.
        stop(&d).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
