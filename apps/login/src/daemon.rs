//! The display manager: the seat's terminal, the greeter's session, the
//! workers of a login, and the person's session once they're in.
//!
//! One loop, fed by a thread for each thing that talks (the greeter's
//! connection, each worker's output), so nothing waits on another.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
};

use olo_greet::{Event, FromWorker, PromptKind, Request, ToWorker, Way};

/// How the daemon runs.
pub struct Config {
    /// A test (`--test DIR`): no seat, terminal or greeter session of its
    /// own (the test runs the greeter); PAM's stack from `DIR/pam`, the
    /// people and sessions from `DIR/root`, the socket `DIR/greeter.sock`.
    pub test: Option<PathBuf>,
    /// Where Olo's programs are (the greeter's session runs
    /// `olo-session` from there, as the greeter's user).
    pub programs: PathBuf,
    /// The seat's terminal.
    pub vt: u32,
}

/// A session to start: its worker, its command and its environment.
type Launch = (u64, Vec<String>, Vec<(String, String)>);

/// The greeter's own user (data/sysusers).
const GREETER_USER: &str = "olo-greeter";
/// The longest Plymouth's boot splash waits for the login screen before
/// it gives the console back (`plymouth.rs`).
const SPLASH_WAITS: std::time::Duration = std::time::Duration::from_secs(30);

enum Msg {
    Greeter(UnixStream),
    Asked(u64, Request),
    GreeterGone(u64),
    Worker(u64, FromWorker),
    WorkerGone(u64),
    /// The login screen hasn't come since Plymouth let go of the screen.
    SplashLate,
}

/// Plymouth's boot splash, as the login screen takes over from it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Splash {
    /// Gone (or never there: a test, a restart of the daemon).
    Gone,
    /// Showing the boot.
    Showing,
    /// Told to let go of the screen for the login screen's session.
    LetGo,
}

/// What a worker is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    /// The greeter's own session.
    GreeterSession,
    /// A way in for the person logging in.
    Login(Way),
}

struct Worker {
    role: Role,
    child: Child,
    input: std::process::ChildStdin,
    /// It said the person is in.
    authenticated: bool,
    /// Its session started.
    started: bool,
}

struct Daemon {
    config: Config,
    tx: mpsc::Sender<Msg>,
    workers: HashMap<u64, Worker>,
    next: u64,
    /// The greeter connected now (its number and where to write).
    greeter: Option<(u64, UnixStream)>,
    /// The person logging in.
    user: Option<String>,
    /// The session to start once the greeter has gone: the winning
    /// worker, the command and its environment.
    pending: Option<Launch>,
    /// The greeter's session, to start once its worker is in.
    pending_greeter: Option<Launch>,
    splash: Splash,
    /// The greeter's sessions in a row that ended with no greeter ever
    /// connecting (Olo can't draw here: no GPU it can render on).
    failed_greeters: u32,
    /// A greeter connected in the session now.
    greeter_seen: bool,
}

/// After this many greeter sessions in a row that never showed, the seat
/// goes to another login screen for this boot (`hand_over`).
const GREETER_TRIES: u32 = 3;
/// What olo-login exits with having handed over: systemd doesn't start it
/// again for it (`RestartPreventExitStatus`, data/systemd/olo-login.service).
pub const HANDED_OVER: i32 = 75;
/// Where it says why, for Olo Doctor.
pub const HANDED_OVER_NOTE: &str = "/run/olo-login/handed-over";

pub fn run(config: Config) -> i32 {
    let socket = match &config.test {
        Some(dir) => dir.join("greeter.sock"),
        None => olo_greet::socket_path(),
    };
    if let Some(dir) = socket.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::remove_file(&socket);
    let listener = match UnixListener::bind(&socket) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("olo-login: {}: {e}", socket.display());
            return 1;
        }
    };
    if config.test.is_none() {
        // Only the greeter's user may connect.
        give_to_greeter(&socket);
        // What an earlier run left of the greeter's sessions (they're
        // logind's, not this service's: restarted, olo-login started a
        // second greeter beside the first, both drawing): ended.
        if let Some(uid) = greeter_uid()
            && olo_system::power::terminate_user(uid).is_ok()
        {
            eprintln!("olo-login: ended the login screen's sessions left from before");
        }
    }
    let (tx, rx) = mpsc::channel();
    let accept = tx.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if accept.send(Msg::Greeter(stream)).is_err() {
                break;
            }
        }
    });
    let splash = if config.test.is_none() && crate::plymouth::Splash::system().running() { Splash::Showing } else { Splash::Gone };
    let mut d = Daemon { config, tx, workers: HashMap::new(), next: 1, greeter: None, user: None, pending: None, pending_greeter: None, splash, failed_greeters: 0, greeter_seen: false };
    d.greeter_session();
    for msg in rx {
        d.handle(msg);
    }
    0
}

/// The socket the greeter's user's (it alone may connect).
fn give_to_greeter(path: &Path) {
    let Ok(name) = std::ffi::CString::new(GREETER_USER) else { return };
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else { return };
    // SAFETY: getpwnam's entry read at once; chown and chmod on our path.
    unsafe {
        let pw = libc::getpwnam(name.as_ptr());
        if !pw.is_null() {
            libc::chown(c.as_ptr(), (*pw).pw_uid, (*pw).pw_gid);
        }
        libc::chmod(c.as_ptr(), 0o600);
    }
}

impl Daemon {
    fn id(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    fn tell(&mut self, e: &Event) {
        if let Some((_, s)) = &mut self.greeter {
            let line = serde_json::to_string(e).unwrap_or_default();
            if writeln!(s, "{line}").and_then(|_| s.flush()).is_err() {
                self.greeter = None;
            }
        }
    }

    /// A worker started: `--worker` with its way (or the greeter's
    /// session) and user, its output read on a thread.
    fn spawn(&mut self, role: Role, user: &str) -> Option<u64> {
        let way = match role {
            Role::GreeterSession => "greeter",
            Role::Login(Way::Password) => "password",
            Role::Login(Way::Fingerprint) => "fingerprint",
            Role::Login(Way::Face) => "face",
        };
        // (Upgraded since it started: the program in its place now.)
        let exe = std::env::current_exe().ok()?;
        let exe = exe.to_str().and_then(|s| s.strip_suffix(" (deleted)")).map(PathBuf::from).unwrap_or(exe);
        let mut cmd = Command::new(exe);
        cmd.args(["--worker", way, user, "--vt", &self.config.vt.to_string()]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
        if let Some(dir) = &self.config.test {
            cmd.arg("--test").arg(dir);
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("olo-login: a worker couldn't start: {e}");
                return None;
            }
        };
        let id = self.id();
        let (input, output) = (child.stdin.take()?, child.stdout.take()?);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                if let Ok(m) = serde_json::from_str(&line)
                    && tx.send(Msg::Worker(id, m)).is_err()
                {
                    return;
                }
            }
            let _ = tx.send(Msg::WorkerGone(id));
        });
        self.workers.insert(id, Worker { role, child, input, authenticated: false, started: false });
        Some(id)
    }

    fn send(&mut self, id: u64, m: &ToWorker) {
        if let Some(w) = self.workers.get_mut(&id) {
            let _ = writeln!(w.input, "{}", serde_json::to_string(m).unwrap_or_default()).and_then(|_| w.input.flush());
        }
    }

    /// The workers of the login under way stopped (another chosen, given
    /// up, or one of them won: `but`).
    fn stop_logins(&mut self, but: Option<u64>) {
        let ids: Vec<u64> = self.workers.iter().filter(|(id, w)| matches!(w.role, Role::Login(_)) && Some(**id) != but && !w.started).map(|(id, _)| *id).collect();
        for id in ids {
            if let Some(mut w) = self.workers.remove(&id) {
                // SAFETY: our child.
                unsafe { libc::kill(w.child.id() as i32, libc::SIGTERM) };
                let _ = w.child.wait();
            }
        }
    }

    /// The greeter's session (not in a test: the test runs the greeter),
    /// on the seat's terminal, unless a person's session is on it.
    fn greeter_session(&mut self) {
        if self.config.test.is_some() || self.workers.values().any(|w| w.role == Role::GreeterSession || w.started) {
            return;
        }
        if self.splash == Splash::Showing {
            // Plymouth lets go of the screen first; it quits once the login
            // screen shows (or gives the console back if it doesn't).
            crate::plymouth::Splash::system().deactivate();
            self.splash = Splash::LetGo;
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                std::thread::sleep(SPLASH_WAITS);
                let _ = tx.send(Msg::SplashLate);
            });
        }
        crate::vt::activate(self.config.vt);
        let Some(id) = self.spawn(Role::GreeterSession, GREETER_USER) else {
            self.splash_gone(false);
            return;
        };
        let session = self.config.programs.join("olo-session");
        let socket = olo_greet::socket_path();
        let env = vec![
            ("OLO_LOGIN_SOCKET".into(), socket.display().to_string()),
            ("XDG_SESSION_DESKTOP".into(), "olo-greeter".into()),
            ("XDG_CURRENT_DESKTOP".into(), "Olo".into()),
            ("PATH".into(), format!("{}:/usr/local/bin:/usr/bin:/bin", self.config.programs.display())),
        ];
        // (Its PAM lets it in without a question.)
        self.pending_greeter = Some((id, vec![session.display().to_string(), "--tty-udev".into(), "--greeter".into()], env));
    }

    /// Plymouth quit, its last picture left for the login screen
    /// (`keep`), else the console given back.
    fn splash_gone(&mut self, keep: bool) {
        if self.splash != Splash::Gone {
            crate::plymouth::Splash::system().quit(keep);
            self.splash = Splash::Gone;
        }
    }

    fn root(&self) -> PathBuf {
        self.config.test.as_ref().map_or_else(|| PathBuf::from("/"), |d| d.join("root"))
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Greeter(stream) => {
                self.greeter_seen = true;
                self.failed_greeters = 0;
                let id = self.id();
                let Ok(reader) = stream.try_clone() else { return };
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    for line in BufReader::new(reader).lines().map_while(Result::ok) {
                        if let Ok(r) = serde_json::from_str(&line)
                            && tx.send(Msg::Asked(id, r)).is_err()
                        {
                            return;
                        }
                    }
                    let _ = tx.send(Msg::GreeterGone(id));
                });
                self.greeter = Some((id, stream));
                // The login screen is up: the boot splash makes way.
                self.splash_gone(true);
            }
            Msg::GreeterGone(id) => {
                if self.greeter.as_ref().is_some_and(|(g, _)| *g == id) {
                    self.greeter = None;
                }
            }
            Msg::Asked(id, request) => {
                if self.greeter.as_ref().is_none_or(|(g, _)| *g != id) {
                    return;
                }
                self.asked(request);
            }
            Msg::Worker(id, m) => self.heard(id, m),
            Msg::WorkerGone(id) => self.gone(id),
            Msg::SplashLate => {
                if self.splash == Splash::LetGo {
                    eprintln!("olo-login: the login screen didn't show; the console is back");
                    self.splash_gone(false);
                }
            }
        }
    }

    fn asked(&mut self, request: Request) {
        match request {
            Request::Users => {
                let users = olo_greet::users::all(&self.root());
                self.tell(&Event::Users { users });
            }
            Request::Sessions => {
                let sessions = olo_greet::sessions::all(&self.root());
                self.tell(&Event::Sessions { sessions });
            }
            Request::Begin { user } => {
                self.stop_logins(None);
                self.user = Some(user.clone());
                for way in [Way::Password, Way::Fingerprint, Way::Face] {
                    // (Not in a test: the person's own sensor and camera.)
                    if way != Way::Password && (self.config.test.is_some() || !way_here(way)) {
                        continue;
                    }
                    self.spawn(Role::Login(way), &user);
                }
            }
            Request::Answer { text } => {
                let id = self.workers.iter().find(|(_, w)| w.role == Role::Login(Way::Password)).map(|(id, _)| *id);
                if let Some(id) = id {
                    self.send(id, &ToWorker::Answer { text });
                }
            }
            Request::Cancel => {
                self.stop_logins(None);
                self.user = None;
            }
            Request::Start { session } => {
                let Some(winner) = self.workers.iter().find(|(_, w)| w.authenticated && matches!(w.role, Role::Login(_))).map(|(id, _)| *id) else {
                    self.tell(&Event::Error { reason: "nobody is logged in yet".into() });
                    return;
                };
                let Some(s) = olo_greet::sessions::all(&self.root()).into_iter().find(|s| s.id == session) else {
                    self.tell(&Event::Error { reason: format!("there's no session called {session}") });
                    return;
                };
                let env = vec![("XDG_SESSION_DESKTOP".into(), s.id.clone()), ("DESKTOP_SESSION".into(), s.id.clone()), ("XDG_CURRENT_DESKTOP".into(), s.desktop_names.join(":"))];
                self.tell(&Event::Started);
                let greeter_on = self.workers.values().any(|w| w.role == Role::GreeterSession);
                if greeter_on {
                    // (Started once the greeter's session has gone, which
                    // it does as the greeter quits.)
                    self.pending = Some((winner, s.command, env));
                } else {
                    self.send(winner, &ToWorker::Start { command: s.command, env });
                }
            }
            Request::Power { action } => {
                if self.config.test.is_some() {
                    eprintln!("olo-login (test): would {action:?}");
                    return;
                }
                let r = match action {
                    olo_greet::Power::Restart => olo_system::power::reboot(),
                    olo_greet::Power::PowerOff => olo_system::power::power_off(),
                    olo_greet::Power::Suspend => olo_system::power::suspend(),
                };
                if let Err(e) = r {
                    self.tell(&Event::Error { reason: e.to_string() });
                }
            }
        }
    }

    fn heard(&mut self, id: u64, m: FromWorker) {
        let Some(role) = self.workers.get(&id).map(|w| w.role) else { return };
        match (role, m) {
            // The greeter's session: in without a question, then started.
            (Role::GreeterSession, FromWorker::Authenticated) => {
                if let Some((gid, command, env)) = self.pending_greeter.take().filter(|(g, ..)| *g == id) {
                    self.send(gid, &ToWorker::Start { command, env });
                }
            }
            (Role::GreeterSession, FromWorker::Failed { reason }) => eprintln!("olo-login: the greeter's session: {reason}"),
            (Role::GreeterSession, _) => {}
            (Role::Login(way), FromWorker::Prompt { kind, text }) => {
                // (Words from a way the person hasn't set up stay quiet.)
                if way == Way::Password || kind == PromptKind::Info {
                    self.tell(&Event::Prompt { by: way, kind, text });
                }
            }
            (Role::Login(way), FromWorker::Failed { reason }) => {
                // (Once in, the greeter has gone: said here.)
                if let Some(w) = self.workers.get(&id)
                    && (w.authenticated || w.started)
                {
                    eprintln!("olo-login: {reason}");
                }
                self.tell(&Event::Failed { by: way, reason });
            }
            (Role::Login(way), FromWorker::Authenticated) => {
                if self.workers.values().any(|w| w.authenticated) {
                    return;
                }
                if let Some(w) = self.workers.get_mut(&id) {
                    w.authenticated = true;
                }
                self.stop_logins(Some(id));
                self.tell(&Event::Authenticated { by: way });
            }
            (Role::Login(_), FromWorker::Started { pid }) => {
                if let Some(w) = self.workers.get_mut(&id) {
                    w.started = true;
                }
                if let Some(dir) = &self.config.test {
                    let _ = std::fs::write(dir.join("started"), format!("{pid}\n"));
                }
            }
            (Role::Login(_), FromWorker::Ended) => {}
        }
    }

    fn gone(&mut self, id: u64) {
        let Some(mut w) = self.workers.remove(&id) else { return };
        let _ = w.child.wait();
        match w.role {
            Role::GreeterSession => {
                // The greeter quit for a session to start: it starts now.
                if let Some((winner, command, env)) = self.pending.take() {
                    self.send(winner, &ToWorker::Start { command, env });
                } else {
                    // It ended (it crashed, or was ended): again, the boot
                    // splash gone so its console shows why if this fails too;
                    // but never showing, time after time, the seat goes to
                    // a login screen that can (`hand_over`).
                    self.splash_gone(false);
                    if !std::mem::take(&mut self.greeter_seen) {
                        self.failed_greeters += 1;
                    }
                    if self.failed_greeters >= GREETER_TRIES && self.config.test.is_none() {
                        hand_over(self.failed_greeters);
                    }
                    self.greeter_session();
                }
            }
            Role::Login(_) if w.started => {
                // Logged out: the greeter again.
                self.user = None;
                if let Some(dir) = &self.config.test {
                    let _ = std::fs::write(dir.join("ended"), "");
                }
                self.greeter_session();
            }
            Role::Login(_) => {}
        }
    }
}

/// The greeter's user's uid.
fn greeter_uid() -> Option<u32> {
    let name = std::ffi::CString::new(GREETER_USER).ok()?;
    // SAFETY: getpwnam's static entry, read at once.
    unsafe {
        let pw = libc::getpwnam(name.as_ptr());
        (!pw.is_null()).then(|| (*pw).pw_uid)
    }
}

/// Whether this computer can log people in `way`: a fingerprint where
/// Fedora's fingerprint stack is set up (fprintd's module in it; when it
/// isn't, authselect leaves `pam_debug.so auth=authinfo_unavail`, which
/// says just that through the conversation: the login screen showed
/// "auth=authinfo_unavail", a fresh Fedora 45), a face where Olo's face
/// module is installed.
fn way_here(way: Way) -> bool {
    match way {
        Way::Password => true,
        Way::Fingerprint => std::fs::read_to_string("/etc/pam.d/fingerprint-auth").is_ok_and(|t| t.lines().any(|l| !l.trim_start().starts_with('#') && l.contains("pam_fprintd"))),
        Way::Face => ["/usr/lib64/security/pam_olo_face.so", "/usr/lib/security/pam_olo_face.so"].iter().any(|p| Path::new(p).exists()),
    }
}

/// Olo's login screen can't show here (its session ended `tries` times
/// in a row before its greeter ever came: no GPU Olo can render on, a
/// broken install), so nobody would get in: the seat goes to the
/// computer's other login screen for this boot (GDM, else SDDM, else the
/// text console's login), and olo-login steps aside, saying why
/// (`HANDED_OVER_NOTE`, which Olo Doctor reads). The next boot tries
/// Olo's again. Never returns.
fn hand_over(tries: u32) -> ! {
    let why = format!("Olo's login screen didn't show {tries} times in a row; the login screen was handed to another for this boot.");
    eprintln!("olo-login: {why}");
    let _ = std::fs::create_dir_all("/run/olo-login");
    let _ = std::fs::write(HANDED_OVER_NOTE, format!("{why}\n"));
    for other in ["gdm.service", "sddm.service", "getty@tty1.service"] {
        if Path::new("/usr/lib/systemd/system").join(other.replace("@tty1", "@")).exists() && olo_system::systemd::start_stop(other, true).is_ok() {
            eprintln!("olo-login: {other} has the seat");
            break;
        }
    }
    std::process::exit(HANDED_OVER)
}
