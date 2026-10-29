//! olo-sudo: sudo in Terminal's shells, asking for its password in a
//! window. Terminal puts a `sudo` that is this program first on its
//! shells' PATH (sudo.rs). A coding agent's commands have no terminal to
//! type a password into (Claude Code runs each in a session of its own,
//! reading /dev/null), so there sudo alone can only fail.
//!
//! When sudo needs a password, we ask Terminal for a window and have sudo
//! check it (`sudo -S -v`: sudo's own PAM, so Touch ID first where PAM
//! has it, then the password), passing on what sudo says and the answer.
//! Then the command runs on the credential sudo just cached: the
//! terminal's, or with no terminal, its parent process's (so we stay the
//! parent). Whatever we can't help with (no Terminal, `-n`, `-S`, `-A`,
//! `-k`, …) goes straight to sudo. The password only ever goes to a sudo
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
    process::{Child, Command, ExitCode, ExitStatus, Stdio},
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
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let Some(sudo) = real_sudo() else {
        eprintln!("sudo: command not found");
        return ExitCode::from(127);
    };
    let Some(socket) = std::env::var_os(SOCKET_ENV).filter(|_| !passes_through(&args)) else { return exec(&sudo, &args) };
    // Nothing to ask: a credential still cached, or none needed.
    if validates_quietly(&sudo) {
        return run(&sudo, &args);
    }
    let Ok(stream) = UnixStream::connect(&socket) else { return exec(&sudo, &args) };
    match ask(&sudo, stream, &args) {
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
    std::env::split_paths(&path).chain(["/usr/bin", "/bin"].map(PathBuf::from)).map(|d| d.join("sudo")).find(|p| {
        std::fs::metadata(p).is_ok_and(|md| md.is_file() && md.uid() == 0 && md.mode() & libc::S_ISUID != 0) && p.canonicalize().ok() != me
    })
}

/// sudo reads its password some other way with these options, or asks
/// for none (scanning the options before the command).
fn passes_through(args: &[OsString]) -> bool {
    if args.len() == 1 && args[0] == "-h" {
        return true;
    }
    let mut value_next = false;
    for a in args {
        if std::mem::take(&mut value_next) {
            continue;
        }
        let Some(a) = a.to_str() else { return false };
        if a == "--" || !a.starts_with('-') || a == "-" {
            return false;
        }
        if let Some(long) = a.strip_prefix("--") {
            let name = long.split('=').next().unwrap_or_default();
            if matches!(name, "non-interactive" | "stdin" | "askpass" | "reset-timestamp" | "remove-timestamp" | "help" | "version") {
                return true;
            }
            value_next = !long.contains('=')
                && matches!(name, "close-from" | "chdir" | "group" | "host" | "prompt" | "chroot" | "role" | "type" | "command-timeout" | "other-user" | "user");
            continue;
        }
        for (i, ch) in a[1..].char_indices() {
            match ch {
                'n' | 'S' | 'A' | 'k' | 'K' | 'V' => return true,
                // The rest of the word, else the next, is its value.
                'C' | 'D' | 'g' | 'h' | 'p' | 'R' | 'r' | 't' | 'T' | 'U' | 'u' => {
                    value_next = i + 2 == a.len();
                    break;
                }
                _ => {}
            }
        }
    }
    false
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
    Info(String),
    Error(String),
    Prompt(String),
    Reply(Reply),
    /// Terminal went away.
    Gone,
    /// `sudo -v` is done: whether it said yes.
    Exit(bool),
}

/// What sudo wrote to its error output, so far: whole lines are messages;
/// what's left once it stops writing is a prompt (it waits for the
/// answer).
fn split(pending: &mut Vec<u8>) -> Vec<Event> {
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
        out.push(Event::Prompt(if s == PROMPT.trim() { "Password:".to_owned() } else { s }));
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

/// Ask in a window: Terminal shows it while `sudo -v` checks.
fn ask(sudo: &Path, stream: UnixStream, args: &[OsString]) -> Outcome {
    let Ok(mut out) = stream.try_clone() else { return Outcome::Pass };
    let mut say = move |a: Ask| {
        let _ = out.write_all(&sudo_wire::line(&a));
    };
    let mut replies = BufReader::new(stream);
    let begin = Ask::Begin {
        args: args.iter().map(|a| a.to_string_lossy().into_owned()).collect(),
        cwd: std::env::current_dir().ok(),
        tty: has_tty(),
    };
    say(begin);
    if read_reply(&mut replies) != Some(Reply::Go) {
        return Outcome::Pass;
    }
    let spawned = sudo_command(sudo)
        .args(["-S", "-v", "-p", PROMPT])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = spawned else { return Outcome::Pass };
    CHILD.store(child.id() as i32, Ordering::Relaxed);
    pass_signals_on();
    let mut stdin = child.stdin.take();
    let (tx, rx) = mpsc::channel();
    let readers = [
        child.stdout.take().map(|o| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for l in BufReader::new(o).lines().map_while(Result::ok) {
                    if !l.trim().is_empty() {
                        let _ = tx.send(Event::Info(l.trim().to_owned()));
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
                    for ev in split(&mut pending) {
                        let _ = tx.send(ev);
                    }
                }
            })
        }),
    ];
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
    std::thread::spawn(move || {
        let ok = child.wait().is_ok_and(|s| s.success());
        // Everything sudo said comes before its end.
        for r in readers.into_iter().flatten() {
            let _ = r.join();
        }
        let _ = tx.send(Event::Exit(ok));
    });
    loop {
        let Ok(e) = rx.recv() else { return Outcome::No(1) };
        match e {
            Event::Info(t) => say(Ask::Info(t)),
            Event::Error(t) => {
                eprintln!("{t}");
                say(Ask::Error(t));
            }
            Event::Prompt(t) => say(Ask::Prompt(t)),
            Event::Reply(Reply::Answer(mut a)) => {
                a.push('\n');
                if let Some(i) = stdin.as_mut() {
                    let _ = i.write_all(a.as_bytes());
                }
                // SAFETY: zero bytes keep the string valid UTF-8.
                unsafe { a.as_bytes_mut().fill(0) };
            }
            Event::Reply(Reply::Cancel) | Event::Gone => {
                // sudo can't be stopped (it is root's now), but with no
                // answer coming it gives up.
                drop(stdin.take());
                // Plain enough for an agent to tell the person said no.
                if matches!(e, Event::Gone) {
                    eprintln!("sudo: not run: Terminal closed before a password was given");
                } else {
                    eprintln!("sudo: not run: cancelled in Terminal's password window");
                }
                return Outcome::No(1);
            }
            Event::Reply(_) => {}
            Event::Exit(ok) => {
                say(Ask::Done(ok));
                return if ok { Outcome::Yes } else { Outcome::No(1) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn through(args: &[&str]) -> bool {
        passes_through(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn options_sudo_answers_itself() {
        assert!(through(&["-n", "true"]));
        assert!(through(&["-S", "ls"]));
        assert!(through(&["-nS", "ls"]));
        assert!(through(&["--non-interactive", "ls"]));
        assert!(through(&["-k"]));
        assert!(through(&["-h"]));
        assert!(through(&["-u", "root", "-n", "ls"]));
        assert!(!through(&["ls", "-n"]));
        assert!(!through(&["dnf", "install", "-y", "foo"]));
        assert!(!through(&["-u", "postgres", "psql"]));
        assert!(!through(&["-unobody", "ls", "-k"]));
        assert!(!through(&["--user", "nobody", "ls"]));
        assert!(!through(&["-v"]));
        assert!(!through(&["-i"]));
        assert!(!through(&["--", "-n"]));
    }

    #[test]
    fn prompts_and_messages() {
        let mut p = b"Place your finger\nSorry, try again.\n[olo-sudo] Password: ".to_vec();
        let ev = split(&mut p);
        assert!(matches!(&ev[..], [Event::Error(a), Event::Error(b), Event::Prompt(c)] if a == "Place your finger" && b == "Sorry, try again." && c == "Password:"));
        assert!(p.is_empty());
        let mut p = b"Verification code: ".to_vec();
        assert!(matches!(&split(&mut p)[..], [Event::Prompt(c)] if c == "Verification code:"));
        let mut p = b"\n".to_vec();
        assert!(split(&mut p).is_empty());
    }
}
