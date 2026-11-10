//! olo-sudo: sudo in Terminal's shells, asking for its password in a
//! window. Terminal puts a `sudo` that is this program first on its
//! shells' PATH (sudo.rs). A coding agent's commands have no terminal to
//! type a password into (Claude Code runs each in a session of its own,
//! reading /dev/null), so there sudo alone can only fail.
//!
//! When sudo needs a password, we ask Terminal for a window and have sudo
//! check it (`sudo -S -v`: sudo's own PAM, the face and Touch ID where PAM
//! has them, and the password at the same time, `ask`), passing on what
//! sudo says and the answer.
//! Then the command runs on the credential sudo just cached: the
//! terminal's, or with no terminal, its parent process's (so we stay the
//! parent). Whatever we can't help with (no Terminal, `-S`, `-A`, `-k`,
//! …) goes straight to sudo; `-n` (never ask) is let in only while
//! Deskbar approves without asking, with no window either way. The password only ever goes to a sudo
//! that is root's and set-user-ID.

mod sudo_wire;

use std::{
    ffi::OsString,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt},
        net::UnixStream,
        process::{CommandExt, ExitStatusExt},
    },
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, ExitCode, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicI32, Ordering},
        mpsc,
    },
};

use sudo_wire::{Ask, Reply, SOCKET_ENV};

/// Our prompt, so we know sudo is asking for the password.
const PROMPT: &str = "[olo-sudo] Password: ";

/// The sudo we wait for, to pass signals on to.
static CHILD: AtomicI32 = AtomicI32::new(0);

fn main() -> ExitCode {
    olo_crash::install();
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let Some(sudo) = real_sudo() else {
        eprintln!("sudo: command not found");
        return ExitCode::from(127);
    };
    let through = passes_through(&args);
    let Some(socket) = std::env::var_os(SOCKET_ENV).filter(|_| through != Through::Yes) else { return exec(&sudo, &args) };
    // Nothing to ask: a credential still cached, or none needed.
    if validates_quietly(&sudo) {
        return run(&sudo, &args);
    }
    let Ok(stream) = UnixStream::connect(&socket) else { return exec(&sudo, &args) };
    match ask(&sudo, stream, &args, through == Through::NeverAsk) {
        Outcome::Yes => run(&sudo, &args),
        Outcome::Pass => exec(&sudo, &args),
        Outcome::No(code) => ExitCode::from(code),
    }
}

/// The real sudo: the first on PATH that isn't us and is root's and
/// set-user-ID, so a password never goes to a stand-in.
fn real_sudo() -> Option<PathBuf> {
    // Tests stand in a sudo of their own (debug builds only).
    #[cfg(debug_assertions)]
    if let Some(p) = std::env::var_os("OLO_SUDO_REAL") {
        return Some(p.into());
    }
    let me = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok());
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/usr/bin", "/bin"].map(PathBuf::from))
        .map(|d| d.join("sudo"))
        .find(|p| std::fs::metadata(p).is_ok_and(|md| md.is_file() && md.uid() == 0 && md.mode() & libc::S_ISUID != 0) && p.canonicalize().ok() != me)
}

/// What sudo's options (those before the command) leave to us.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Through {
    /// Nothing: we may ask in a window.
    No,
    /// `-n`: never ask anyone. Only approval without asking lets it in.
    NeverAsk,
    /// sudo reads its password some other way, or asks for none.
    Yes,
}

fn passes_through(args: &[OsString]) -> Through {
    if args.len() == 1 && args[0] == "-h" {
        return Through::Yes;
    }
    let (mut value_next, mut never_ask) = (false, false);
    let found = |never_ask: bool| if never_ask { Through::NeverAsk } else { Through::No };
    for a in args {
        if std::mem::take(&mut value_next) {
            continue;
        }
        let Some(a) = a.to_str() else { return found(never_ask) };
        if a == "--" || !a.starts_with('-') || a == "-" {
            return found(never_ask);
        }
        if let Some(long) = a.strip_prefix("--") {
            let name = long.split('=').next().unwrap_or_default();
            if name == "non-interactive" {
                never_ask = true;
                continue;
            }
            if matches!(name, "stdin" | "askpass" | "reset-timestamp" | "remove-timestamp" | "help" | "version") {
                return Through::Yes;
            }
            value_next = !long.contains('=') && matches!(name, "close-from" | "chdir" | "group" | "host" | "prompt" | "chroot" | "role" | "type" | "command-timeout" | "other-user" | "user");
            continue;
        }
        for (i, ch) in a[1..].char_indices() {
            match ch {
                'n' => never_ask = true,
                'S' | 'A' | 'k' | 'K' | 'V' => return Through::Yes,
                // The rest of the word, else the next, is its value.
                'C' | 'D' | 'g' | 'h' | 'p' | 'R' | 'r' | 't' | 'T' | 'U' | 'u' => {
                    value_next = i + 2 == a.len();
                    break;
                }
                _ => {}
            }
        }
    }
    found(never_ask)
}

fn has_tty() -> bool {
    std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NOCTTY).open("/dev/tty").is_ok()
}

fn sudo_command(sudo: &Path) -> Command {
    let mut c = Command::new(sudo);
    c.arg0("sudo");
    c
}

/// Become sudo.
fn exec(sudo: &Path, args: &[OsString]) -> ExitCode {
    let e = sudo_command(sudo).args(args).exec();
    eprintln!("sudo: {e}");
    ExitCode::FAILURE
}

/// Whether sudo needs nothing from anyone (`sudo -n -v`).
fn validates_quietly(sudo: &Path) -> bool {
    sudo_command(sudo).args(["-n", "-v"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// Run the command, now that sudo needn't ask.
fn run(sudo: &Path, args: &[OsString]) -> ExitCode {
    // With a terminal, sudo's credential is the terminal's.
    if has_tty() {
        return exec(sudo, args);
    }
    // Without, its parent's: ours.
    match sudo_command(sudo).args(args).spawn() {
        Ok(child) => wait(child),
        Err(e) => {
            eprintln!("sudo: {e}");
            ExitCode::FAILURE
        }
    }
}

fn wait(mut child: Child) -> ExitCode {
    CHILD.store(child.id() as i32, Ordering::Relaxed);
    pass_signals_on();
    let status = child.wait();
    CHILD.store(0, Ordering::Relaxed);
    status.map_or(ExitCode::FAILURE, exit_code)
}

fn exit_code(s: ExitStatus) -> ExitCode {
    ExitCode::from(s.code().map(|c| c as u8).or_else(|| s.signal().map(|n| 128 + n as u8)).unwrap_or(1))
}

extern "C" fn pass_on(sig: libc::c_int) {
    let pid = CHILD.load(Ordering::Relaxed);
    if pid > 0 {
        // SAFETY: kill is async-signal-safe.
        unsafe { libc::kill(pid, sig) };
    }
}

fn pass_signals_on() {
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
        // SAFETY: the handler only calls kill.
        unsafe { libc::signal(sig, pass_on as extern "C" fn(libc::c_int) as libc::sighandler_t) };
    }
}

enum Outcome {
    /// sudo has a credential now.
    Yes,
    /// Leave it to sudo.
    Pass,
    /// Not allowed, with this exit status.
    No(u8),
}

enum Event {
    Info(usize, String),
    Error(String),
    Prompt(usize, String),
    Reply(Reply),
    /// Terminal went away.
    Gone,
    /// A `sudo -v` is done: whether it said yes.
    Exit(usize, bool),
}

/// What sudo wrote to its error output, so far: whole lines are messages;
/// what's left once it stops writing is a prompt (it waits for the
/// answer).
fn split(run: usize, pending: &mut Vec<u8>) -> Vec<Event> {
    let mut out = Vec::new();
    while let Some(i) = pending.iter().position(|&b| b == b'\n') {
        let line: Vec<u8> = pending.drain(..=i).collect();
        let s = String::from_utf8_lossy(&line).trim().to_owned();
        if !s.is_empty() {
            out.push(Event::Error(s));
        }
    }
    if !pending.is_empty() {
        let s = String::from_utf8_lossy(pending).trim().to_owned();
        pending.clear();
        out.push(Event::Prompt(run, if s == PROMPT.trim() { "Password:".to_owned() } else { s }));
    }
    out
}

fn read_reply(r: &mut impl BufRead) -> Option<Reply> {
    let mut line = String::new();
    loop {
        line.clear();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if let Ok(reply) = serde_json::from_str(&line) {
            // SAFETY: zero bytes keep the string valid UTF-8.
            unsafe { line.as_bytes_mut().fill(0) };
            return Some(reply);
        }
    }
}

/// Start a `sudo -S -v` checking (run `run`), what it says going to `tx`.
fn check(sudo: &Path, run: usize, tx: &mpsc::Sender<Event>) -> Option<ChildStdin> {
    let mut child = sudo_command(sudo).args(["-S", "-v", "-p", PROMPT]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    if run == 0 {
        CHILD.store(child.id() as i32, Ordering::Relaxed);
    }
    let stdin = child.stdin.take();
    let readers = [
        child.stdout.take().map(|o| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for l in BufReader::new(o).lines().map_while(Result::ok) {
                    if !l.trim().is_empty() {
                        let _ = tx.send(Event::Info(run, l.trim().to_owned()));
                    }
                }
            })
        }),
        child.stderr.take().map(|mut e| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let (mut pending, mut buf) = (Vec::new(), [0u8; 4096]);
                while let Ok(n @ 1..) = e.read(&mut buf) {
                    pending.extend_from_slice(&buf[..n]);
                    for ev in split(run, &mut pending) {
                        let _ = tx.send(ev);
                    }
                }
            })
        }),
    ];
    let tx = tx.clone();
    std::thread::spawn(move || {
        let ok = child.wait().is_ok_and(|s| s.success());
        // Everything sudo said comes before its end.
        for r in readers.into_iter().flatten() {
            let _ = r.join();
        }
        let _ = tx.send(Event::Exit(run, ok));
    });
    stdin
}

/// Ask in a window: Terminal shows it while `sudo -v` checks. `quiet`
/// (`-n`): only if approved without asking, never in a window.
///
/// Every way in at once: sudo's PAM goes one way at a time (the face,
/// then the fingerprint, then the password), so a security key's
/// password (or one typed) would wait until the fingerprint sensor gave
/// up. Once the first check says it listens for a finger (pam_fprintd
/// has the sensor), a second starts: its pam_fprintd finds the sensor
/// taken and asks for the password at once. Answers go to whichever asks;
/// the first to say yes counts. The other is left without input: a check
/// at the password gives up at once; one still listening for the finger
/// (root's: it can't be stopped) gives up when pam_fprintd does (30 s),
/// or a finger in that time only validates sudo's credential again.
fn ask(sudo: &Path, stream: UnixStream, args: &[OsString], quiet: bool) -> Outcome {
    let Ok(mut out) = stream.try_clone() else { return Outcome::Pass };
    let mut say = move |a: Ask| {
        let _ = out.write_all(&sudo_wire::line(&a));
    };
    let mut replies = BufReader::new(stream);
    let begin = Ask::Begin { args: args.iter().map(|a| a.to_string_lossy().into_owned()).collect(), cwd: std::env::current_dir().ok(), tty: has_tty(), quiet };
    say(begin);
    if read_reply(&mut replies) != Some(Reply::Go) {
        return Outcome::Pass;
    }
    let (tx, rx) = mpsc::channel();
    // Each check's input, while it may still ask.
    let mut stdins: Vec<Option<ChildStdin>> = vec![check(sudo, 0, &tx)];
    if stdins[0].is_none() {
        return Outcome::Pass;
    }
    pass_signals_on();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            loop {
                match read_reply(&mut replies) {
                    Some(r) => {
                        if tx.send(Event::Reply(r)).is_err() {
                            return;
                        }
                    }
                    None => {
                        let _ = tx.send(Event::Gone);
                        return;
                    }
                }
            }
        });
    }
    // Which check's prompt an answer goes to; whether the first listens
    // for a finger while the second asks; which have ended.
    let (mut asking, mut finger, mut ended) = (0, false, [false; 2]);
    loop {
        let Ok(e) = rx.recv() else { return Outcome::No(1) };
        match e {
            Event::Info(run, t) => {
                let second = run == 0 && stdins.len() == 1 && olo_system::fingerprint::listening(&t);
                say(Ask::Info(t));
                if second {
                    stdins.push(check(sudo, 1, &tx));
                    if stdins[1].is_some() {
                        finger = true;
                        say(Ask::Finger(true));
                    }
                }
            }
            Event::Error(t) => {
                eprintln!("{t}");
                say(Ask::Error(t));
            }
            // The first is past the finger and asks itself: the second
            // asks instead, so the first goes without.
            Event::Prompt(0, _) if stdins.len() > 1 => {
                drop(stdins[0].take());
                if std::mem::take(&mut finger) {
                    say(Ask::Finger(false));
                }
            }
            Event::Prompt(run, t) => {
                asking = run;
                say(Ask::Prompt(t));
            }
            Event::Reply(Reply::Answer(mut a)) => {
                a.push('\n');
                if let Some(i) = stdins.get_mut(asking).and_then(Option::as_mut) {
                    let _ = i.write_all(a.as_bytes());
                }
                // SAFETY: zero bytes keep the string valid UTF-8.
                unsafe { a.as_bytes_mut().fill(0) };
            }
            Event::Reply(Reply::Cancel) | Event::Gone => {
                // sudo can't be stopped (it is root's now), but with no
                // answer coming it gives up.
                stdins.clear();
                // Plain enough for an agent to tell the person said no.
                if matches!(e, Event::Gone) {
                    eprintln!("sudo: not run: Terminal closed before a password was given");
                } else {
                    eprintln!("sudo: not run: cancelled in Terminal's password window");
                }
                return Outcome::No(1);
            }
            Event::Reply(_) => {}
            Event::Exit(_, true) => {
                stdins.clear();
                say(Ask::Done(true));
                return Outcome::Yes;
            }
            Event::Exit(run, false) => {
                ended[run.min(1)] = true;
                // The first's finger is over (it matched nothing, or the
                // sensor gave up); the second asks on.
                if run == 0 && stdins.len() > 1 && !ended[1] {
                    drop(stdins[0].take());
                    if std::mem::take(&mut finger) {
                        say(Ask::Finger(false));
                    }
                    continue;
                }
                stdins.clear();
                say(Ask::Done(false));
                return Outcome::No(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn through(args: &[&str]) -> bool {
        passes_through(&args.iter().map(OsString::from).collect::<Vec<_>>()) == Through::Yes
    }

    fn never_ask(args: &[&str]) -> bool {
        passes_through(&args.iter().map(OsString::from).collect::<Vec<_>>()) == Through::NeverAsk
    }

    #[test]
    fn non_interactive_waits_on_approval() {
        assert!(never_ask(&["-n", "true"]));
        assert!(never_ask(&["--non-interactive", "ls"]));
        assert!(never_ask(&["-u", "root", "-n", "ls"]));
        assert!(never_ask(&["-n", "-u", "root", "ls"]));
        // With -S as well, sudo reads the password itself.
        assert!(through(&["-nS", "ls"]));
        assert!(through(&["-n", "-k"]));
        assert!(!never_ask(&["ls", "-n"]));
        assert!(!never_ask(&["--", "-n"]));
    }

    #[test]
    fn options_sudo_answers_itself() {
        assert!(through(&["-S", "ls"]));
        assert!(through(&["-nS", "ls"]));
        assert!(through(&["-k"]));
        assert!(through(&["-h"]));
        assert!(!through(&["ls", "-n"]));
        assert!(!through(&["dnf", "install", "-y", "foo"]));
        assert!(!through(&["-u", "postgres", "psql"]));
        assert!(!through(&["-unobody", "ls", "-k"]));
        assert!(!through(&["--user", "nobody", "ls"]));
        assert!(!through(&["-v"]));
        assert!(!through(&["-i"]));
        assert!(!through(&["--", "-n"]));
    }

    /// A stand-in for `sudo -S -v`, as sudo's PAM goes with Touch ID:
    /// the sensor (a folder in `$OLO_TEST_DIR`) free, it takes it, says
    /// so and listens (a finger: `finger` there) for `$OLO_TEST_LISTEN`
    /// tenths of a second, then the password; taken, or no finger
    /// enrolled (`$OLO_TEST_NO_FINGER`), the password at once ("right"),
    /// three tries.
    const STAND_IN: &str = r#"#!/bin/bash
D="$OLO_TEST_DIR"
if [ -z "$OLO_TEST_NO_FINGER" ] && mkdir "$D/sensor" 2>/dev/null; then
  echo "Place your finger on the fingerprint reader"
  for i in $(seq 1 "$OLO_TEST_LISTEN"); do
    if [ -e "$D/finger" ]; then rmdir "$D/sensor"; exit 0; fi
    sleep 0.1
  done
  rmdir "$D/sensor"
fi
for try in 1 2 3; do
  printf '%s' "$4" >&2
  read -r pw || exit 1
  [ "$pw" = right ] && exit 0
  echo "Sorry, try again." >&2
done
exit 1
"#;

    /// Terminal's side of a conversation, as a test drives it.
    struct Window {
        lines: std::io::Lines<BufReader<UnixStream>>,
        out: UnixStream,
        dir: PathBuf,
        asked: std::thread::JoinHandle<bool>,
    }

    impl Window {
        fn open(name: &str, env: &[(&str, &str)]) -> Window {
            let dir = std::env::temp_dir().join(format!("olo-sudo-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let sudo = dir.join("sudo");
            std::fs::write(&sudo, STAND_IN).unwrap();
            std::fs::set_permissions(&sudo, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
            // (The stand-in reads these; tests in one process share them,
            // so each sets its own before its checks start.)
            // SAFETY: the tests that set them run one at a time (TESTS).
            unsafe {
                std::env::set_var("OLO_TEST_DIR", &dir);
                std::env::set_var("OLO_TEST_LISTEN", "40");
                std::env::remove_var("OLO_TEST_NO_FINGER");
                for (k, v) in env {
                    std::env::set_var(k, v);
                }
            }
            let (ours, theirs) = UnixStream::pair().unwrap();
            let asked = std::thread::spawn(move || matches!(ask(&sudo, theirs, &[OsString::from("true")], false), Outcome::Yes));
            let mut w = Window { lines: BufReader::new(ours.try_clone().unwrap()).lines(), out: ours, dir, asked };
            assert!(matches!(w.next(), Ask::Begin { .. }));
            w.reply(Reply::Go);
            w
        }

        fn next(&mut self) -> Ask {
            serde_json::from_str(&self.lines.next().unwrap().unwrap()).unwrap()
        }

        fn reply(&mut self, r: Reply) {
            self.out.write_all(&sudo_wire::line(&r)).unwrap();
        }

        /// What it says until `until` matches (that included).
        fn until(&mut self, until: impl Fn(&Ask) -> bool) -> Vec<Ask> {
            let mut seen = Vec::new();
            loop {
                let a = self.next();
                let done = until(&a);
                seen.push(a);
                if done {
                    return seen;
                }
            }
        }

        fn finish(self) -> bool {
            let ok = self.asked.join().unwrap();
            // The first check, left listening, gives up on its own.
            std::thread::sleep(std::time::Duration::from_millis(4500));
            let _ = std::fs::remove_dir_all(&self.dir);
            ok
        }
    }

    /// The stand-in's environment is the process's: one test at a time.
    static TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn every_way_in_at_once() {
        let _one = TESTS.lock().unwrap_or_else(|e| e.into_inner());
        // A finger: the first check, while the second asks for the
        // password.
        let mut w = Window::open("finger", &[]);
        let said = w.until(|a| matches!(a, Ask::Prompt(_)));
        assert_eq!(said, [Ask::Info("Place your finger on the fingerprint reader".into()), Ask::Finger(true), Ask::Prompt("Password:".into())]);
        std::fs::write(w.dir.join("finger"), "").unwrap();
        assert_eq!(w.next(), Ask::Done(true));
        assert!(w.finish());

        // The password (a security key's, or typed) while the finger is
        // listened for: wrong, then right.
        let mut w = Window::open("password", &[]);
        w.until(|a| matches!(a, Ask::Prompt(_)));
        w.reply(Reply::Answer("wrong".into()));
        assert_eq!(w.until(|a| matches!(a, Ask::Prompt(_))), [Ask::Error("Sorry, try again.".into()), Ask::Prompt("Password:".into())]);
        w.reply(Reply::Answer("right".into()));
        assert_eq!(w.next(), Ask::Done(true));
        assert!(w.finish());

        // The finger gives up: the second asks on, alone.
        let mut w = Window::open("gives-up", &[("OLO_TEST_LISTEN", "5")]);
        w.until(|a| matches!(a, Ask::Prompt(_)));
        assert_eq!(w.next(), Ask::Finger(false));
        w.reply(Reply::Answer("right".into()));
        assert_eq!(w.next(), Ask::Done(true));
        assert!(w.finish());

        // No finger enrolled: one check, the password at once.
        let mut w = Window::open("no-finger", &[("OLO_TEST_NO_FINGER", "1")]);
        assert_eq!(w.next(), Ask::Prompt("Password:".into()));
        w.reply(Reply::Answer("right".into()));
        assert_eq!(w.next(), Ask::Done(true));
        assert!(w.finish());
    }

    #[test]
    fn prompts_and_messages() {
        let mut p = b"Place your finger\nSorry, try again.\n[olo-sudo] Password: ".to_vec();
        let ev = split(0, &mut p);
        assert!(matches!(&ev[..], [Event::Error(a), Event::Error(b), Event::Prompt(0, c)] if a == "Place your finger" && b == "Sorry, try again." && c == "Password:"));
        assert!(p.is_empty());
        let mut p = b"Verification code: ".to_vec();
        assert!(matches!(&split(1, &mut p)[..], [Event::Prompt(1, c)] if c == "Verification code:"));
        let mut p = b"\n".to_vec();
        assert!(split(0, &mut p).is_empty());
    }
}
