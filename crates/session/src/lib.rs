//! olo-session: starts an Olo session and keeps it running.
//!
//! ```text
//! olo-session [--tty-udev | --winit | --headless WxH] [--no-shell]
//! olo-session reload     restart app_server in place (picks up upgrades)
//! olo-session upgrade    install newer programs from the build tree, then reload
//! olo-session quit       end the session
//! ```
//!
//! The session owns the Wayland and IPC listening sockets and hands them to
//! app_server, so they outlive it. `reload` stops app_server (it saves the
//! window layout), starts the new binary on the same sockets, and every
//! client reconnects by itself and recreates its windows — or, if its own
//! binary was upgraded, re-executes the new version with its saved state
//! (see `olo_kit::run`). Nobody has to log out. If olo-session itself
//! was upgraded it re-executes first, keeping its children and sockets.
//!
//! An installed session also watches the tree it was built in and offers
//! newer builds through Deskbar (upgrade.rs).
//!
//! It also restarts app_server and the shell (Deskbar, the Tracker desktop,
//! and in a real session the polkit agent)
//! if they crash, ends and restarts an app_server that stops answering
//! (watchdog.rs), and exports the session environment to systemd/D-Bus.
//! A real session has keyd (if it runs) leave the modifier keys to
//! app_server (keyd.rs), and gives coding agents Olo's skills
//! (skills.rs).

mod graphical;
mod keyd;
mod keyring;
mod skills;
mod upgrade;
mod watchdog;

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

const USAGE: &str = "usage: olo-session [--tty-udev | --winit | --headless WxH] [--no-shell]
       olo-session reload     restart app_server in place (picks up upgrades)
       olo-session upgrade    install newer programs from the build tree, then reload
       olo-session quit       end the session";

const SHELL: &[&[&str]] = &[&["olo-deskbar"], &["olo-tracker", "--desktop"]];
/// Services only the real (logged-in) session runs: the polkit agent
/// answers for the login session and the notification centre and the
/// portal backend (screen sharing) serve the session bus, which a nested
/// or headless one must never take over; Token Pool's gateway listens on
/// the computer's one loopback port.
const REAL_SHELL: &[&[&str]] = &[&["olo-polkit-agent"], &["olo-notifications"], &["olo-portal", "--service"], &["olo-pool", "--service"]];
/// Give up restarting something that crashes this often.
const CRASH_LIMIT: usize = 5;
const CRASH_WINDOW: Duration = Duration::from_secs(60);

fn log(msg: impl AsRef<str>) {
    eprintln!("olo-session: {}", msg.as_ref());
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// `name` in the runtime directory, or what a session that started on the
/// old build (Tabula) called it there.
fn run_file(name: String) -> PathBuf {
    olo_xdg::legacy::runtime_file(&runtime_dir(), &name)
}

pub fn run() {
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
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
    let pid = std::env::var("OLO_SESSION_PID").ok().and_then(|p| p.parse::<i32>().ok()).or_else(|| {
        let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
        std::fs::read_to_string(run_file(format!("olo-session-{display}.pid"))).ok()?.trim().parse().ok()
    });
    let Some(pid) = pid else {
        eprintln!("olo-session: no running session found (is $OLO_SESSION_PID set?)");
        std::process::exit(1);
    };
    // SAFETY: sending a signal.
    if unsafe { libc::kill(pid, sig) } != 0 {
        eprintln!("olo-session: could not signal session {pid}: {}", std::io::Error::last_os_error());
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
    #[serde(default)]
    proxy: Option<Proxy>,
    /// Then nudge the programs whose files were replaced (see `bridge`).
    #[serde(default)]
    nudge: bool,
}

/// olo-wlproxy and app_server's two other sockets: one for Olo's own
/// apps (which reconnect by themselves), one the proxy connects to for
/// everything else (which it keeps alive across app_server restarts).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Proxy {
    direct_fd: RawFd,
    proxied_fd: RawFd,
    child: Child,
    /// What it was started from (None: by a session from before this was
    /// noted, so another version).
    #[serde(default)]
    binary: Option<(u64, i64, i64)>,
    /// Older proxies still serving the programs they had (`renew_proxy`).
    #[serde(default)]
    retired: Vec<i32>,
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
    /// Our socket and lock files as made (device, inode): removed at the
    /// end only while still ours, and made again if they vanish.
    files: Vec<(PathBuf, Option<(u64, u64)>)>,
    watchdog: watchdog::Watchdog,
    /// The direct and proxied listeners and the proxy, once started.
    proxy: Option<(UnixListener, UnixListener, Child)>,
    /// The program the proxy was started from: when it's another, the
    /// proxy is renewed (`renew_proxy`).
    proxy_binary: Option<(u64, i64, i64)>,
    /// Proxies that gave the socket to a newer one and serve the programs
    /// they had until those close (those this version started then exit).
    retired: Vec<i32>,
    /// The proxy kept crashing: app_server takes the socket itself.
    no_proxy: bool,
}

const RESUME_ENV: &str = "OLO_SESSION_RESUME";

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
        let ipc_path = runtime_dir().join(format!("olo-{display}.sock"));
        let _ = std::fs::remove_file(&ipc_path);
        // Window places carry over a reload, never into a new session.
        let _ = std::fs::remove_file(layout_path(&display));
        let ipc = UnixListener::bind(&ipc_path).unwrap_or_else(|e| {
            log(format!("could not create the IPC socket: {e}"));
            std::process::exit(1);
        });
        set_session_env(&display, &ipc_path);
        let _ = std::fs::write(runtime_dir().join(format!("olo-session-{display}.pid")), std::process::id().to_string());
        // Machine-readable, for tools/dev/tsession.
        println!("session: WAYLAND_DISPLAY={display} OLO_SOCKET={}", ipc_path.display());
        log(format!("session on {display}, pid {}", std::process::id()));

        let mut s = Session {
            watchdog: watchdog::Watchdog::start(ipc_path.clone()),
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
            files: Vec::new(),
            proxy: None,
            proxy_binary: None,
            retired: Vec::new(),
            no_proxy: std::env::var_os("OLO_NO_WAYLAND_PROXY").is_some(),
        };
        s.own_files();
        s.start_server();
        // Only a real session owns the user's environment: a headless or
        // nested one (tests, development) must not point the user's portals
        // and services at itself.
        let real = s.backend.iter().any(|b| b == "--tty-udev");
        // SAFETY: single-threaded start-up.
        unsafe {
            if real {
                std::env::set_var("OLO_REAL_SESSION", "1");
            } else {
                // Not inherited from a real session we were started in.
                std::env::remove_var("OLO_REAL_SESSION");
            }
        }
        s.real = real;
        if real {
            keyring::start();
            export_environment();
            keyd::pass_through();
            skills::start();
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

    fn resume(mut r: Resume) -> Session {
        // Children a Tabula session started go by their new names.
        for c in std::iter::once(&mut r.server).chain(r.shell.iter_mut()).chain(r.proxy.iter_mut().map(|p| &mut p.child)) {
            if let Some(name) = c.argv.first_mut() {
                *name = olo_xdg::legacy::new_program(name);
            }
        }
        // SAFETY: these fds were ours before exec (CLOEXEC cleared for it).
        let (wayland, ipc, lock) = unsafe {
            (UnixListener::from_raw_fd(r.wayland_fd), UnixListener::from_raw_fd(r.ipc_fd), std::fs::File::from_raw_fd(r.lock_fd))
        };
        for fd in [r.wayland_fd, r.ipc_fd, r.lock_fd] {
            set_cloexec(fd, true);
        }
        let (proxy_binary, retired) = r.proxy.as_ref().map_or((None, Vec::new()), |p| (p.binary, p.retired.clone()));
        // SAFETY: as above.
        let proxy = r.proxy.map(|p| unsafe {
            set_cloexec(p.direct_fd, true);
            set_cloexec(p.proxied_fd, true);
            (UnixListener::from_raw_fd(p.direct_fd), UnixListener::from_raw_fd(p.proxied_fd), p.child)
        });
        let ipc_path = run_file(format!("olo-{}.sock", r.display));
        let real = r.backend.iter().any(|b| b == "--tty-udev");
        let mut s = Session {
            watchdog: watchdog::Watchdog::start(ipc_path.clone()),
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
            files: Vec::new(),
            proxy,
            proxy_binary,
            retired,
            no_proxy: std::env::var_os("OLO_NO_WAYLAND_PROXY").is_some(),
        };
        s.own_files();
        s.upgrades = exe().parent().and_then(|d| upgrade::Watcher::new(d, &s.display));
        // A new proxy (this upgrade's) takes new programs from now on.
        s.renew_proxy_if_upgraded();
        if real {
            // This version's skills, for agents run from now on.
            skills::start();
            // What the session says it is may have changed with the
            // upgrade (olo_xdg::upgrade_env): for services started on
            // demand from now on too.
            std::thread::spawn(|| {
                let desktop = [("XDG_CURRENT_DESKTOP", olo_xdg::CURRENT_DESKTOP)];
                if let Err(e) = olo_system::env::update_activation_environment(&desktop) {
                    log(format!("could not export the environment: {e}"));
                }
            });
        }
        // A service this version adds starts now, not at the next login (a
        // session with a shell has Deskbar in it; one without stays so).
        let has_shell = s.shell.iter().any(|c| c.argv == ["olo-deskbar"]);
        if real && has_shell {
            for argv in REAL_SHELL {
                let argv: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
                if !s.shell.iter().any(|c| c.argv == argv) {
                    log(format!("starting {} (new in this version)", argv[0]));
                    let pid = spawn(&argv, &[]).unwrap_or(0);
                    s.shell.push(Child { argv, pid, crashes: VecDeque::new() });
                }
            }
        }
        if exe().file_name().is_some_and(|n| n == "tabula-session") {
            s.bridge();
        }
        if r.reloading {
            s.reload();
        }
        if r.nudge
            && let Some(w) = &s.upgrades
        {
            w.nudge(&[std::process::id() as i32, s.server.pid]);
        }
        s
    }

    /// We were a Tabula session, upgraded to this program under its old
    /// name (the only one such a session offers): install the rest of Olo
    /// from the build tree, leave the old names as links to the new
    /// programs (the login screen's entry runs `tabula-session`, and
    /// programs still running re-execute themselves by the names they were
    /// started by), and become `olo-session`, reloading into the new
    /// app_server. Our apps become their new versions as that restarts,
    /// and the rest are nudged once it has: so it is the new app_server,
    /// which knows their old app ids, that puts their windows back. Returns
    /// only if that fails.
    fn bridge(&mut self) {
        log("upgraded from Tabula; installing Olo");
        let Some(w) = &mut self.upgrades else {
            log("nothing to install Olo from (not an installed session)");
            return;
        };
        let done = w.install();
        log(format!("installed {} programs", done.len()));
        if let Some(dir) = exe().parent() {
            upgrade::link_old_names(dir);
        }
        self.exec_self(true, true);
    }

    fn start_server(&mut self) {
        let mut argv = vec!["olo-app-server".to_owned()];
        argv.extend(self.backend.iter().cloned());
        self.start_proxy();
        let mut fds = vec![self.wayland.as_raw_fd(), self.ipc.as_raw_fd()];
        if let Some((direct, proxied, _)) = &self.proxy {
            fds[0] = direct.as_raw_fd();
            fds.push(proxied.as_raw_fd());
        }
        let mut env = vec![("OLO_WAYLAND_FD", fds[0].to_string()), ("OLO_IPC_FD", fds[1].to_string())];
        if let Some(fd) = fds.get(2) {
            env.push(("OLO_WAYLAND_PROXY_FD", fd.to_string()));
        }
        // The listening sockets must survive into app_server.
        for &fd in &fds {
            set_cloexec(fd, false);
        }
        self.server.argv = argv.clone();
        self.server.pid = spawn(&argv, &env).unwrap_or(0);
        for &fd in &fds {
            set_cloexec(fd, true);
        }
        log(format!("app_server started (pid {})", self.server.pid));
    }

    /// Where Olo's own apps connect, and where the proxy does.
    fn direct_path(&self) -> PathBuf {
        run_file(format!("olo-{}-direct", self.display))
    }

    fn proxied_path(&self) -> PathBuf {
        run_file(format!("olo-{}-proxied", self.display))
    }

    /// Put olo-wlproxy on the session's socket, if it isn't there.
    fn start_proxy(&mut self) {
        if self.no_proxy {
            if let Some((_, _, child)) = self.proxy.take()
                && child.pid > 0
            {
                // SAFETY: signalling our own child.
                unsafe { libc::kill(child.pid, libc::SIGTERM) };
            }
            return;
        }
        if self.proxy.is_none() {
            let bind = |p: &Path| {
                let _ = std::fs::remove_file(p);
                UnixListener::bind(p)
            };
            match (bind(&self.direct_path()), bind(&self.proxied_path())) {
                (Ok(direct), Ok(proxied)) => {
                    self.proxy = Some((direct, proxied, Child { argv: vec!["olo-wlproxy".into()], pid: 0, crashes: VecDeque::new() }));
                    self.own_files();
                }
                (Err(e), _) | (_, Err(e)) => {
                    log(format!("could not make the proxy's sockets ({e}); programs connect to app_server directly"));
                    self.no_proxy = true;
                    return;
                }
            }
        }
        let upstream = self.proxied_path();
        let fd = self.wayland.as_raw_fd();
        let Some((_, _, child)) = &mut self.proxy else { return };
        if child.pid > 0 {
            return;
        }
        set_cloexec(fd, false);
        child.pid = spawn(&child.argv, &[("OLO_WAYLAND_FD", fd.to_string()), ("OLO_WAYLAND_UPSTREAM", upstream.display().to_string())]).unwrap_or(0);
        set_cloexec(fd, true);
        self.proxy_binary = proxy_binary();
        log(format!("olo-wlproxy started (pid {})", child.pid));
    }

    /// Renew the proxy if its program was upgraded since it started.
    fn renew_proxy_if_upgraded(&mut self) {
        let running = self.proxy.as_ref().is_some_and(|p| p.2.pid > 0);
        if running && proxy_binary().is_some() && proxy_binary() != self.proxy_binary {
            self.renew_proxy("olo-wlproxy was upgraded");
        }
    }

    /// Give new programs a new proxy, without taking any from the old
    /// one: a new listening socket goes in the old one's place (renamed
    /// over it, so a program connecting meanwhile finds one or the other),
    /// the new proxy takes it, and the old one serves the programs it has
    /// until they close (one this version started then exits: SIGWINCH;
    /// an older one never notices, and stays until the session ends).
    fn renew_proxy(&mut self, why: &str) {
        let path = self.wayland_path();
        let fresh = path.with_extension("new");
        let _ = std::fs::remove_file(&fresh);
        let listener = match UnixListener::bind(&fresh).and_then(|l| std::fs::rename(&fresh, &path).map(|()| l)) {
            Ok(l) => l,
            Err(e) => {
                log(format!("{why}, but its new socket couldn't be made ({e}); keeping the old proxy"));
                let _ = std::fs::remove_file(&fresh);
                return;
            }
        };
        self.wayland = listener;
        self.hand_over_proxy(why);
        self.own_files();
    }

    /// Start a new proxy on the session's socket (`self.wayland`, made
    /// again), retiring the running one.
    fn hand_over_proxy(&mut self, why: &str) {
        let Some((_, _, child)) = &mut self.proxy else { return };
        let old = std::mem::replace(&mut child.pid, 0);
        if old > 0 {
            // SAFETY: signalling our own child (ignored by one too old to know it).
            unsafe { libc::kill(old, libc::SIGWINCH) };
            self.retired.push(old);
        }
        log(format!("{why}: a new olo-wlproxy takes new programs; the old one (pid {old}) keeps those it has"));
        self.start_proxy();
    }

    fn run(mut self) -> ! {
        loop {
            match wait_signal(Duration::from_secs(2)) {
                Some(libc::SIGHUP | libc::SIGUSR1) => self.reload(),
                Some(libc::SIGUSR2) => self.upgrade(),
                Some(libc::SIGTERM | libc::SIGINT) => self.shutdown(0),
                _ => {}
            }
            self.reap();
            if !self.stopping {
                self.watchdog.check(self.server.pid);
            }
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

    /// Install what's newer in the build tree and switch to it. Only a new
    /// app_server needs a reload (which closes apps that can't reconnect);
    /// otherwise each upgraded Olo program re-executes in place and
    /// everything else carries on.
    fn upgrade(&mut self) {
        let Some(w) = &mut self.upgrades else {
            log("nothing to upgrade from (not an installed session)");
            return;
        };
        let done = w.install();
        if done.is_empty() {
            return;
        }
        w.nudge(&[std::process::id() as i32, self.server.pid]);
        if done.iter().any(|p| p == "olo-app-server") {
            self.reload();
        } else if own_binary().is_some() && own_binary() != self.binary {
            self.exec_self(false, false);
        }
        // (A new session does this as it resumes.)
        self.renew_proxy_if_upgraded();
    }

    fn wayland_path(&self) -> PathBuf {
        runtime_dir().join(&self.display)
    }

    fn own_files(&mut self) {
        let lock = runtime_dir().join(format!("{}.lock", self.display));
        let mut paths = vec![self.wayland_path(), lock, self.ipc_path.clone()];
        if self.proxy.is_some() {
            paths.extend([self.direct_path(), self.proxied_path()]);
        }
        self.files = paths.into_iter().map(|p| {
            let id = identity(&p);
            (p, id)
        }).collect();
    }

    /// Make our socket files again if they've gone (something removed
    /// them: the runtime directory emptied, or a careless program), so new
    /// programs can reach us; app_server gets the new sockets when it next
    /// starts.
    fn mend_sockets(&mut self) {
        let wayland = self.wayland_path();
        let lock_path = runtime_dir().join(format!("{}.lock", self.display));
        let mut mended = false;
        if !lock_path.exists()
            && let Ok(lock) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&lock_path)
        {
            // SAFETY: flock on an fd we own.
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                self.lock = lock;
                mended = true;
            }
        }
        if !wayland.exists() {
            match UnixListener::bind(&wayland) {
                Ok(l) => {
                    self.wayland = l;
                    mended = true;
                    // The proxy listens on the socket that went.
                    self.hand_over_proxy("the Wayland socket was made again");
                }
                Err(e) => log(format!("could not make {} again: {e}", wayland.display())),
            }
        }
        if !self.ipc_path.exists() {
            match UnixListener::bind(&self.ipc_path) {
                Ok(l) => {
                    self.ipc = l;
                    mended = true;
                }
                Err(e) => log(format!("could not make {} again: {e}", self.ipc_path.display())),
            }
        }
        if mended {
            log("our sockets had been removed; made them again");
            self.own_files();
        }
    }

    /// Restart app_server in place (clients reconnect on their own).
    fn reload(&mut self) {
        log("reloading");
        self.mend_sockets();
        if own_binary().is_some() && own_binary() != self.binary {
            self.exec_self(true, false);
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

    /// Become the upgraded olo-session, keeping children and sockets
    /// (and then reload, if `reloading`, and nudge upgraded programs, if
    /// `nudge`).
    fn exec_self(&mut self, reloading: bool, nudge: bool) {
        let mut fds = vec![self.wayland.as_raw_fd(), self.ipc.as_raw_fd(), self.lock.as_raw_fd()];
        let proxy = self.proxy.as_ref().map(|(direct, proxied, child)| Proxy {
            direct_fd: direct.as_raw_fd(),
            proxied_fd: proxied.as_raw_fd(),
            child: child.clone(),
            binary: self.proxy_binary,
            retired: self.retired.clone(),
        });
        if let Some(p) = &proxy {
            fds.extend([p.direct_fd, p.proxied_fd]);
        }
        let resume = Resume {
            display: self.display.clone(),
            wayland_fd: fds[0],
            ipc_fd: fds[1],
            lock_fd: fds[2],
            backend: self.backend.clone(),
            server: self.server.clone(),
            shell: self.shell.clone(),
            reloading,
            proxy,
            nudge,
        };
        let Ok(state) = serde_json::to_string(&resume) else { return };
        for &fd in &fds {
            set_cloexec(fd, false);
        }
        log("olo-session was upgraded; re-executing");
        // The signal mask is inherited across exec, which is what we want.
        let mut cmd = Command::new(exe());
        olo_xdg::upgrade_env(&mut cmd);
        let e = cmd.env(RESUME_ENV, state).exec();
        log(format!("re-exec failed ({e}); carrying on"));
        for &fd in &fds {
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
            } else if let Some(i) = self.retired.iter().position(|&p| p == pid) {
                self.retired.remove(i);
                log(format!("the old olo-wlproxy (pid {pid}) is done: its programs have all closed"));
            } else if let Some((_, _, c)) = self.proxy.as_mut().filter(|p| p.2.pid == pid) {
                c.pid = 0;
                if self.stopping {
                    continue;
                }
                log(format!("olo-wlproxy exited ({}); other programs lost their windows", describe(status)));
                if note_crash(c) {
                    self.start_proxy();
                } else {
                    log("olo-wlproxy keeps exiting; app_server takes the socket itself from now on");
                    self.no_proxy = true;
                    self.reload();
                }
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
        let proxy = self.proxy.as_ref().map_or(0, |p| p.2.pid);
        let pids: Vec<i32> = self.shell.iter().map(|c| c.pid).chain([self.server.pid, proxy]).chain(self.retired.iter().copied()).filter(|&p| p > 0).collect();
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
            graphical::stop();
        }
        for (path, id) in &self.files {
            if id.is_some() && identity(path) == *id {
                let _ = std::fs::remove_file(path);
            }
        }
        let _ = std::fs::remove_file(layout_path(&self.display));
        let pid_file = run_file(format!("olo-session-{}.pid", self.display));
        if std::fs::read_to_string(&pid_file).is_ok_and(|p| p.trim() == std::process::id().to_string()) {
            let _ = std::fs::remove_file(pid_file);
        }
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
    EXE.get_or_init(|| std::env::current_exe().unwrap_or_else(|_| "olo-session".into()))
}

/// The proxy's program as installed beside us: (inode, mtime).
fn proxy_binary() -> Option<(u64, i64, i64)> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(exe().parent()?.join("olo-wlproxy")).ok()?;
    Some((m.ino(), m.mtime(), m.mtime_nsec()))
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
/// A file by identity (device, inode).
fn identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

/// Where app_server leaves window places for its successor (see
/// app_server's layout.rs).
fn layout_path(display: &str) -> PathBuf {
    run_file(format!("olo-layout-{display}.json"))
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
        ("OLO_SOCKET", ipc.display().to_string()),
        ("OLO_SESSION", "1".into()),
        ("OLO_SESSION_PID", std::process::id().to_string()),
        ("XDG_CURRENT_DESKTOP", olo_xdg::CURRENT_DESKTOP.into()),
        ("XDG_SESSION_DESKTOP", "olo".into()),
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
    let vars: Vec<(&str, String)> = ["WAYLAND_DISPLAY", "XDG_CURRENT_DESKTOP", "XDG_SESSION_TYPE", "OLO_SOCKET"]
        .into_iter()
        .filter_map(|k| Some((k, std::env::var(k).ok()?)))
        .collect();
    // D-Bus calls can stall if a bus is slow to answer; don't hold up start-up.
    std::thread::spawn(move || {
        let pairs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        if let Err(e) = olo_system::env::update_activation_environment(&pairs) {
            log(format!("could not export the environment: {e}"));
        }
        graphical::start();
    });
}
