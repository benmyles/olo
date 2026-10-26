//! tabula-session: starts a Tabula session and keeps it running.
//!
//! ```text
//! tabula-session [--tty-udev | --winit | --headless WxH] [--no-shell]
//! tabula-session reload     restart app_server in place (picks up upgrades)
//! tabula-session upgrade    install newer programs from the build tree, then reload
//! tabula-session quit       end the session
//! ```
//!
//! The session owns the Wayland and IPC listening sockets and hands them to
//! app_server, so they outlive it. `reload` stops app_server (it saves the
//! window layout), starts the new binary on the same sockets, and every
//! client reconnects by itself and recreates its windows — or, if its own
//! binary was upgraded, re-executes the new version with its saved state
//! (see `tabula_kit::run`). Nobody has to log out. If tabula-session itself
//! was upgraded it re-executes first, keeping its children and sockets.
//!
//! An installed session also watches the tree it was built in and offers
//! newer builds through Deskbar (upgrade.rs).
//!
//! It also restarts app_server and the shell (Deskbar, the Tracker desktop,
//! and in a real session the polkit agent)
//! if they crash, and exports the session environment to systemd/D-Bus.
//! A real session has keyd (if it runs) leave the modifier keys to
//! app_server (keyd.rs).

mod keyd;
mod upgrade;

use std::{
    collections::VecDeque,
    ffi::OsString,
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::{net::UnixListener, process::CommandExt},
    },
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant, SystemTime},
};

use serde::{Deserialize, Serialize};

const USAGE: &str = "usage: tabula-session [--tty-udev | --winit | --headless WxH] [--no-shell]
       tabula-session reload     restart app_server in place (picks up upgrades)
       tabula-session upgrade    install newer programs from the build tree, then reload
       tabula-session quit       end the session";

const SHELL: &[&[&str]] = &[&["tabula-deskbar"], &["tabula-tracker", "--desktop"]];
/// Services only the real (logged-in) session runs: the polkit agent
/// answers for the login session, which a nested or headless one must
/// never take over.
const REAL_SHELL: &[&[&str]] = &[&["tabula-polkit-agent"]];
/// Give up restarting something that crashes this often.
const CRASH_LIMIT: usize = 5;
const CRASH_WINDOW: Duration = Duration::from_secs(60);

fn log(msg: impl AsRef<str>) {
    eprintln!("tabula-session: {}", msg.as_ref());
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

fn main() {
    exe();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("reload") => signal_session(libc::SIGHUP),
        Some("upgrade") => signal_session(libc::SIGUSR2),
        Some("quit") => signal_session(libc::SIGTERM),
        Some("-h" | "--help") => println!("{USAGE}"),
        _ => Session::start(args).run(),
    }
}

// ---- the control commands ---------------------------------------------------

fn signal_session(sig: i32) {
    let pid = std::env::var("TABULA_SESSION_PID").ok().and_then(|p| p.parse::<i32>().ok()).or_else(|| {
        let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
        std::fs::read_to_string(runtime_dir().join(format!("tabula-session-{display}.pid"))).ok()?.trim().parse().ok()
    });
    let Some(pid) = pid else {
        eprintln!("tabula-session: no running session found (is $TABULA_SESSION_PID set?)");
        std::process::exit(1);
    };
    // SAFETY: sending a signal.
    if unsafe { libc::kill(pid, sig) } != 0 {
        eprintln!("tabula-session: could not signal session {pid}: {}", std::io::Error::last_os_error());
        std::process::exit(1);
    }
}

// ---- the session ------------------------------------------------------------

/// A supervised child.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Child {
    argv: Vec<String>,
    pid: i32,
    #[serde(skip)]
    crashes: VecDeque<Option<Instant>>,
}

/// Everything needed to carry on after re-executing ourselves.
#[derive(Debug, Serialize, Deserialize)]
struct Resume {
    display: String,
    wayland_fd: RawFd,
    ipc_fd: RawFd,
    lock_fd: RawFd,
    backend: Vec<String>,
    server: Child,
    shell: Vec<Child>,
    /// Re-executed in the middle of a reload, which the new self finishes.
    #[serde(default)]
    reloading: bool,
}

struct Session {
    display: String,
    wayland: UnixListener,
    ipc: UnixListener,
    ipc_path: PathBuf,
    /// Held for the session's lifetime: the wayland-N name is ours.
    lock: std::fs::File,
    backend: Vec<String>,
    server: Child,
    shell: Vec<Child>,
    /// Our own binary when we started, to notice upgrades.
    binary: Option<(u64, SystemTime)>,
    stopping: bool,
    /// On the machine's own screens (not nested or headless).
    real: bool,
    upgrades: Option<upgrade::Watcher>,
    /// When keyd was last asked to leave our keys alone.
    keyd_at: Instant,
}

const RESUME_ENV: &str = "TABULA_SESSION_RESUME";

impl Session {
    fn start(args: Vec<String>) -> Session {
        block_signals();
        if let Some(state) = std::env::var_os(RESUME_ENV) {
            // SAFETY: single-threaded.
            unsafe { std::env::remove_var(RESUME_ENV) };
            if let Ok(r) = serde_json::from_str::<Resume>(&state.to_string_lossy()) {
                log(format!("upgraded in place; resuming session on {}", r.display));
                return Session::resume(r);
            }
        }
        let no_shell = args.iter().any(|a| a == "--no-shell");
        let mut backend: Vec<String> = args.into_iter().filter(|a| a != "--no-shell").collect();
        if backend.is_empty() {
            backend.push("--tty-udev".into());
        }
        let (display, wayland, lock) = bind_wayland().unwrap_or_else(|e| {
            log(format!("could not create a Wayland socket: {e}"));
            std::process::exit(1);
        });
        let ipc_path = runtime_dir().join(format!("tabula-{display}.sock"));
        let _ = std::fs::remove_file(&ipc_path);
        // Window places carry over a reload, never into a new session.
        let _ = std::fs::remove_file(layout_path(&display));
        let ipc = UnixListener::bind(&ipc_path).unwrap_or_else(|e| {
            log(format!("could not create the IPC socket: {e}"));
            std::process::exit(1);
        });
        set_session_env(&display, &ipc_path);
        let _ = std::fs::write(runtime_dir().join(format!("tabula-session-{display}.pid")), std::process::id().to_string());
        // Machine-readable, for tools/dev/tsession.
        println!("session: WAYLAND_DISPLAY={display} TABULA_SOCKET={}", ipc_path.display());
        log(format!("session on {display}, pid {}", std::process::id()));

        let mut s = Session {
            display,
            wayland,
            ipc,
            ipc_path,
            lock,
            backend,
            server: Child { argv: Vec::new(), pid: 0, crashes: VecDeque::new() },
            shell: Vec::new(),
            binary: own_binary(),
            stopping: false,
            real: false,
            upgrades: None,
            keyd_at: Instant::now(),
        };
        s.start_server();
        // Only a real session owns the user's environment: a headless or
        // nested one (tests, development) must not point the user's portals
        // and services at itself.
        let real = s.backend.iter().any(|b| b == "--tty-udev");
        // SAFETY: single-threaded start-up.
        unsafe {
            if real {
                std::env::set_var("TABULA_REAL_SESSION", "1");
            } else {
                // Not inherited from a real session we were started in.
                std::env::remove_var("TABULA_REAL_SESSION");
            }
        }
        s.real = real;
        if real {
            export_environment();
            keyd::pass_through();
        }
        s.upgrades = exe().parent().and_then(|d| upgrade::Watcher::new(d, &s.display));
        if !no_shell {
            let extra: &[&[&str]] = if real { REAL_SHELL } else { &[] };
            for argv in SHELL.iter().chain(extra) {
                let argv: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
                let pid = spawn(&argv, &[]).unwrap_or(0);
                s.shell.push(Child { argv, pid, crashes: VecDeque::new() });
            }
        }
        s
    }

    fn resume(r: Resume) -> Session {
        // SAFETY: these fds were ours before exec (CLOEXEC cleared for it).
        let (wayland, ipc, lock) = unsafe {
            (UnixListener::from_raw_fd(r.wayland_fd), UnixListener::from_raw_fd(r.ipc_fd), std::fs::File::from_raw_fd(r.lock_fd))
        };
        for fd in [r.wayland_fd, r.ipc_fd, r.lock_fd] {
            set_cloexec(fd, true);
        }
        let ipc_path = runtime_dir().join(format!("tabula-{}.sock", r.display));
        let real = r.backend.iter().any(|b| b == "--tty-udev");
        let mut s = Session {
            display: r.display,
            wayland,
            ipc,
            ipc_path,
            lock,
            backend: r.backend,
            server: r.server,
            shell: r.shell,
            binary: own_binary(),
            stopping: false,
            real,
            upgrades: None,
            keyd_at: Instant::now(),
        };
        s.upgrades = exe().parent().and_then(|d| upgrade::Watcher::new(d, &s.display));
        if r.reloading {
            s.reload();
        }
        s
    }

    fn start_server(&mut self) {
        let mut argv = vec!["tabula-app-server".to_owned()];
        argv.extend(self.backend.iter().cloned());
        let fds = [self.wayland.as_raw_fd(), self.ipc.as_raw_fd()];
        let env = [
            ("TABULA_WAYLAND_FD", fds[0].to_string()),
            ("TABULA_IPC_FD", fds[1].to_string()),
        ];
        // The listening sockets must survive into app_server.
        for fd in fds {
            set_cloexec(fd, false);
        }
        self.server.argv = argv.clone();
        self.server.pid = spawn(&argv, &env).unwrap_or(0);
        for fd in fds {
            set_cloexec(fd, true);
        }
        log(format!("app_server started (pid {})", self.server.pid));
    }

    fn run(mut self) -> ! {
        loop {
            match wait_signal(Duration::from_secs(5)) {
                Some(libc::SIGHUP | libc::SIGUSR1) => self.reload(),
                Some(libc::SIGUSR2) => self.upgrade(),
                Some(libc::SIGTERM | libc::SIGINT) => self.shutdown(0),
                _ => {}
            }
            self.reap();
            if let Some(w) = &mut self.upgrades {
                w.check();
            }
            // `keyd reload` forgets what we asked.
            if self.real && self.keyd_at.elapsed() > Duration::from_secs(60) {
                self.keyd_at = Instant::now();
                keyd::pass_through();
            }
        }
    }

    /// Install what's newer in the build tree and reload with it.
    fn upgrade(&mut self) {
        let Some(w) = &mut self.upgrades else {
            log("nothing to upgrade from (not an installed session)");
            return;
        };
        if w.install() > 0 {
            self.reload();
        }
    }

    /// Restart app_server in place (clients reconnect on their own).
    fn reload(&mut self) {
        log("reloading");
        if own_binary().is_some() && own_binary() != self.binary {
            self.exec_self();
        }
        let old = self.server.pid;
        if old > 0 {
            // SAFETY: signalling our own child.
            unsafe { libc::kill(old, libc::SIGTERM) };
            if !wait_for(old, Duration::from_secs(5)) {
                log("app_server did not stop; killing it");
                // SAFETY: as above.
                unsafe { libc::kill(old, libc::SIGKILL) };
                wait_for(old, Duration::from_secs(2));
            }
        }
        self.start_server();
    }

    /// Become the upgraded tabula-session, keeping children and sockets.
    fn exec_self(&mut self) {
        let fds = [self.wayland.as_raw_fd(), self.ipc.as_raw_fd(), self.lock.as_raw_fd()];
        let resume = Resume {
            display: self.display.clone(),
            wayland_fd: fds[0],
            ipc_fd: fds[1],
            lock_fd: fds[2],
            backend: self.backend.clone(),
            server: self.server.clone(),
            shell: self.shell.clone(),
            reloading: true,
        };
        let Ok(state) = serde_json::to_string(&resume) else { return };
        for fd in fds {
            set_cloexec(fd, false);
        }
        log("tabula-session was upgraded; re-executing");
        // The signal mask is inherited across exec, which is what we want.
        let e = Command::new(exe()).env(RESUME_ENV, state).exec();
        log(format!("re-exec failed ({e}); carrying on"));
        for fd in fds {
            set_cloexec(fd, true);
        }
    }

    /// Collect exited children; restart what should be running.
    fn reap(&mut self) {
        loop {
            let mut status = 0;
            // SAFETY: plain waitpid.
            let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
            if pid <= 0 {
                return;
            }
            let clean = libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0;
            if pid == self.server.pid {
                self.server.pid = 0;
                if self.stopping {
                    continue;
                }
                if clean {
                    log("app_server exited; ending the session");
                    self.shutdown(0);
                }
                log(format!("app_server died ({}); restarting it", describe(status)));
                if !note_crash(&mut self.server) {
                    log("app_server keeps crashing; giving up");
                    self.shutdown(1);
                }
                self.start_server();
            } else if let Some(i) = self.shell.iter().position(|c| c.pid == pid) {
                if self.stopping {
                    continue;
                }
                let c = &mut self.shell[i];
                log(format!("{} exited ({}); restarting it", c.argv[0], describe(status)));
                if note_crash(c) {
                    c.pid = spawn(&c.argv, &[]).unwrap_or(0);
                } else {
                    log(format!("{} keeps exiting; leaving it stopped", c.argv[0]));
                    c.pid = 0;
                }
            }
        }
    }

    fn shutdown(&mut self, code: i32) -> ! {
        self.stopping = true;
        log("ending the session");
        let pids: Vec<i32> = self.shell.iter().map(|c| c.pid).chain([self.server.pid]).filter(|&p| p > 0).collect();
        for &p in &pids {
            // SAFETY: signalling our own children.
            unsafe { libc::kill(p, libc::SIGTERM) };
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        for &p in &pids {
            wait_for(p, deadline.saturating_duration_since(Instant::now()));
        }
        if let Some(w) = &self.upgrades {
            w.forget();
        }
        if self.real {
            keyd::restore();
        }
        let _ = std::fs::remove_file(&self.ipc_path);
        let _ = std::fs::remove_file(layout_path(&self.display));
        let _ = std::fs::remove_file(runtime_dir().join(&self.display));
        let _ = std::fs::remove_file(runtime_dir().join(format!("tabula-session-{}.pid", self.display)));
        std::process::exit(code);
    }
}

/// Record a crash; false once it has crashed too often recently.
fn note_crash(c: &mut Child) -> bool {
    let now = Instant::now();
    c.crashes.push_back(Some(now));
    c.crashes.retain(|t| t.is_some_and(|t| now.duration_since(t) < CRASH_WINDOW));
    c.crashes.len() <= CRASH_LIMIT
}

fn describe(status: i32) -> String {
    if libc::WIFEXITED(status) {
        format!("status {}", libc::WEXITSTATUS(status))
    } else if libc::WIFSIGNALED(status) {
        format!("signal {}", libc::WTERMSIG(status))
    } else {
        "unknown".into()
    }
}

// ---- processes --------------------------------------------------------------

/// Start `argv[0]` (preferring the binary next to ours, so a development
/// build runs its own components).
fn spawn(argv: &[String], env: &[(&str, String)]) -> Option<i32> {
    let sibling = exe().parent().map(|d| d.join(&argv[0]));
    let program: OsString = match sibling {
        Some(p) if p.exists() => p.into_os_string(),
        _ => argv[0].clone().into(),
    };
    let mut cmd = Command::new(program);
    cmd.args(&argv[1..]);
    for (k, v) in env {
        cmd.env(k, v);
    }
    // SAFETY: async-signal-safe calls only, between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            // Children start with no signals blocked (we block ours).
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
            Ok(())
        });
    }
    match cmd.spawn() {
        Ok(child) => {
            let pid = child.id() as i32;
            // Reaped by waitpid in `reap`, not through `Child`.
            std::mem::forget(child);
            Some(pid)
        }
        Err(e) => {
            log(format!("could not start {}: {e}", argv[0]));
            None
        }
    }
}

/// Wait up to `timeout` for `pid` to exit (reaping it).
fn wait_for(pid: i32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let mut status = 0;
        // SAFETY: plain waitpid.
        let r = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        if r == pid || r < 0 {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The path we were started from. Asked once, at start-up: when the binary
/// is replaced, /proc/self/exe points at the deleted old file.
fn exe() -> &'static Path {
    static EXE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    EXE.get_or_init(|| std::env::current_exe().unwrap_or_else(|_| "tabula-session".into()))
}

fn own_binary() -> Option<(u64, SystemTime)> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(exe()).ok()?;
    Some((m.ino(), m.modified().ok()?))
}

// ---- signals ----------------------------------------------------------------

const SIGNALS: [i32; 6] = [libc::SIGCHLD, libc::SIGHUP, libc::SIGUSR1, libc::SIGUSR2, libc::SIGTERM, libc::SIGINT];

fn signal_set() -> libc::sigset_t {
    // SAFETY: building a sigset.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for s in SIGNALS {
            libc::sigaddset(&mut set, s);
        }
        set
    }
}

/// Block our signals; `wait_signal` takes them synchronously.
fn block_signals() {
    let set = signal_set();
    // SAFETY: single-threaded process.
    unsafe { libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) };
}

fn wait_signal(timeout: Duration) -> Option<i32> {
    let set = signal_set();
    let ts = libc::timespec { tv_sec: timeout.as_secs() as _, tv_nsec: timeout.subsec_nanos() as _ };
    // SAFETY: valid set and timespec.
    let sig = unsafe { libc::sigtimedwait(&set, std::ptr::null_mut(), &ts) };
    (sig > 0).then_some(sig)
}

// ---- sockets and environment -------------------------------------------------

fn set_cloexec(fd: RawFd, on: bool) {
    // SAFETY: fcntl on an fd we own.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags >= 0 {
            let flags = if on { flags | libc::FD_CLOEXEC } else { flags & !libc::FD_CLOEXEC };
            libc::fcntl(fd, libc::F_SETFD, flags);
        }
    }
}

/// Claim the first free `wayland-N` (with its lock file, as libwayland does).
/// Where app_server leaves window places for its successor (see
/// app_server's layout.rs).
fn layout_path(display: &str) -> PathBuf {
    runtime_dir().join(format!("tabula-layout-{display}.json"))
}

fn bind_wayland() -> std::io::Result<(String, UnixListener, std::fs::File)> {
    let dir = runtime_dir();
    for n in 1..33 {
        let name = format!("wayland-{n}");
        let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(format!("{name}.lock")))?;
        // SAFETY: flock on an fd we own.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            continue;
        }
        let path = dir.join(&name);
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        return Ok((name, listener, lock));
    }
    Err(std::io::Error::other("no free wayland-N socket name"))
}

fn set_session_env(display: &str, ipc: &Path) {
    let vars = [
        ("WAYLAND_DISPLAY", display.to_owned()),
        ("TABULA_SOCKET", ipc.display().to_string()),
        ("TABULA_SESSION", "1".into()),
        ("TABULA_SESSION_PID", std::process::id().to_string()),
        ("XDG_CURRENT_DESKTOP", "Tabula".into()),
        ("XDG_SESSION_DESKTOP", "tabula".into()),
        ("XDG_SESSION_TYPE", "wayland".into()),
        ("MOZ_ENABLE_WAYLAND", "1".into()),
        ("QT_QPA_PLATFORM", "wayland;xcb".into()),
        ("SDL_VIDEODRIVER", "wayland".into()),
        ("_JAVA_AWT_WM_NONREPARENTING", "1".into()),
    ];
    for (k, v) in vars {
        // SAFETY: single-threaded start-up.
        unsafe { std::env::set_var(k, v) };
    }
    // The login screen's PATH needn't include where we're installed
    // (~/.local/bin): put our directory first, for everything we start.
    if let Some(dir) = exe().parent() {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).filter(|d| d != dir).collect();
        dirs.insert(0, dir.to_owned());
        if let Ok(path) = std::env::join_paths(dirs) {
            // SAFETY: single-threaded start-up.
            unsafe { std::env::set_var("PATH", path) };
        }
    }
}

/// Tell systemd and D-Bus activation about the session, so portals and
/// services started on demand find our display.
fn export_environment() {
    let vars: Vec<(&str, String)> = ["WAYLAND_DISPLAY", "XDG_CURRENT_DESKTOP", "XDG_SESSION_TYPE", "TABULA_SOCKET"]
        .into_iter()
        .filter_map(|k| Some((k, std::env::var(k).ok()?)))
        .collect();
    // D-Bus calls can stall if a bus is slow to answer; don't hold up start-up.
    std::thread::spawn(move || {
        let pairs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        if let Err(e) = tabula_system::env::update_activation_environment(&pairs) {
            log(format!("could not export the environment: {e}"));
        }
    });
}
