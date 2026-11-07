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

enum Msg {
    Greeter(UnixStream),
    Asked(u64, Request),
    GreeterGone(u64),
    Worker(u64, FromWorker),
    WorkerGone(u64),
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
}

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
    let mut d = Daemon { config, tx, workers: HashMap::new(), next: 1, greeter: None, user: None, pending: None, pending_greeter: None };
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
        crate::vt::activate(self.config.vt);
        let Some(id) = self.spawn(Role::GreeterSession, GREETER_USER) else { return };
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

    fn root(&self) -> PathBuf {
        self.config.test.as_ref().map_or_else(|| PathBuf::from("/"), |d| d.join("root"))
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Greeter(stream) => {
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
                    if way != Way::Password && self.config.test.is_some() {
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
                    // It ended (it crashed, or was ended): again.
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
