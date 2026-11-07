//! A worker: one way in for one person (`olo-login --worker WAY USER`),
//! as GDM's session workers are. Its PAM conversation goes through the
//! daemon (questions out on standard output, answers in on standard
//! input, a line of JSON each). Once in, it waits to be told to start
//! the session; then it opens the person's PAM and logind session,
//! becomes them on the seat's terminal, starts their desktop and waits
//! for it to end. One thread, so forking is safe.
//!
//! The greeter's own session is a worker too (`WAY` greeter): its user
//! is the system's `olo-greeter`, its PAM service lets it in without a
//! question, and its session is `logind`'s greeter class.

use std::{
    ffi::CString,
    io::{BufRead, Write},
    path::PathBuf,
};

use olo_greet::{FromWorker, PromptKind, ToWorker, pam};

/// Where this worker is.
pub struct Setup {
    /// The PAM service (`olo-login`, `olo-login-fingerprint`, ...).
    pub service: String,
    pub user: String,
    /// Logind's class for the session: "user", or "greeter".
    pub class: &'static str,
    /// The seat's terminal (`1`: /dev/tty1).
    pub vt: u32,
    /// A test (`OLO_LOGIN_TEST`): PAM's stack from this folder, and the
    /// session run as whoever runs the test, without a terminal.
    pub test: Option<PathBuf>,
}

fn say(m: &FromWorker) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", serde_json::to_string(m).unwrap_or_default());
    let _ = out.flush();
}

fn hear() -> Option<ToWorker> {
    let mut line = String::new();
    loop {
        line.clear();
        if std::io::stdin().lock().read_line(&mut line).ok()? == 0 {
            return None;
        }
        if let Ok(m) = serde_json::from_str(line.trim()) {
            return Some(m);
        }
    }
}

pub fn run(setup: Setup) -> i32 {
    let converse: pam::Converse = Box::new(|kind, text: &str| {
        say(&FromWorker::Prompt { kind, text: text.to_owned() });
        match kind {
            PromptKind::Secret | PromptKind::Visible => match hear() {
                Some(ToWorker::Answer { text: Some(t) }) => pam::Answer::Said(Some(t)),
                _ => pam::Answer::GiveUp,
            },
            PromptKind::Info | PromptKind::Error => pam::Answer::Said(None),
        }
    });
    let confdir = setup.test.as_ref().map(|t| t.join("pam"));
    let mut p = match pam::Pam::start(&setup.service, &setup.user, confdir.as_deref(), converse) {
        Ok(p) => p,
        Err(reason) => {
            say(&FromWorker::Failed { reason });
            return 1;
        }
    };
    if let Err(reason) = p.authenticate() {
        say(&FromWorker::Failed { reason });
        return 1;
    }
    say(&FromWorker::Authenticated);
    // Told to start (or the daemon gone: nothing to do).
    let Some(ToWorker::Start { command, env }) = std::iter::from_fn(hear).find(|m| matches!(m, ToWorker::Start { .. })) else { return 0 };
    let tty = format!("tty{}", setup.vt);
    for (k, v) in [("XDG_SESSION_TYPE", "wayland"), ("XDG_SESSION_CLASS", setup.class), ("XDG_SEAT", "seat0")] {
        let _ = p.putenv(k, v);
    }
    if setup.test.is_none() {
        let _ = p.putenv("XDG_VTNR", &setup.vt.to_string());
        let _ = p.set_tty(&tty);
    }
    if let Err(reason) = p.open_session() {
        say(&FromWorker::Failed { reason: format!("the session couldn't open: {reason}") });
        return 1;
    }
    let account = match account(&setup.user) {
        Some(a) => a,
        None => {
            say(&FromWorker::Failed { reason: format!("{} has no account", setup.user) });
            return 1;
        }
    };
    // The session's environment: PAM's (XDG_SESSION_ID, XDG_RUNTIME_DIR
    // and the rest), the person's, the session's own.
    let mut vars: Vec<(String, String)> = p.env();
    vars.extend([
        ("HOME".into(), account.home.clone()),
        ("USER".into(), setup.user.clone()),
        ("LOGNAME".into(), setup.user.clone()),
        ("SHELL".into(), account.shell.clone()),
        ("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into()),
        ("XDG_SESSION_TYPE".into(), "wayland".into()),
        ("XDG_SESSION_CLASS".into(), setup.class.into()),
        ("XDG_SEAT".into(), "seat0".into()),
    ]);
    if setup.test.is_none() {
        vars.push(("XDG_VTNR".into(), setup.vt.to_string()));
    }
    vars.extend(env);
    // Through the system's olo-session beside us (`--exec`): SELinux moves
    // the session into the person's domain as it starts a system program,
    // as GDM's does through gdm-wayland-session (a fresh Fedora 45: starting
    // ~/.local/bin/olo-session from the login screen's domain was denied).
    let command: Vec<String> = match std::env::current_exe().ok().and_then(|e| Some(e.parent()?.join("olo-session"))) {
        Some(trampoline) if setup.test.is_none() && trampoline.is_file() => std::iter::once(trampoline.display().to_string()).chain(std::iter::once("--exec".to_owned())).chain(command).collect(),
        _ => command,
    };
    // SAFETY: one thread (the worker never starts another), so the child
    // may do anything before it execs.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        say(&FromWorker::Failed { reason: "fork failed".into() });
        return 1;
    }
    if pid == 0 {
        let why = become_and_exec(&setup, &account, &tty, &command, &vars);
        eprintln!("olo-login: the session couldn't start: {why}");
        // SAFETY: the child's own end, without the parent's destructors.
        unsafe { libc::_exit(127) };
    }
    say(&FromWorker::Started { pid });
    let mut status = 0;
    // SAFETY: our child.
    while unsafe { libc::waitpid(pid, &mut status, 0) } < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {}
    say(&FromWorker::Ended);
    // (Dropping the transaction closes the session and its credentials.)
    drop(p);
    0
}

/// A stream into the journal (systemd's stdout socket, its header said:
/// `identifier`, info, no level prefixes, nothing forwarded), as a file
/// descriptor; None without one.
fn journal_stream(identifier: &str) -> Option<i32> {
    use std::{io::Write as _, os::fd::IntoRawFd as _};
    let mut s = std::os::unix::net::UnixStream::connect("/run/systemd/journal/stdout").ok()?;
    s.write_all(format!("{identifier}\n\n6\n0\n0\n0\n0\n").as_bytes()).ok()?;
    let _ = s.shutdown(std::net::Shutdown::Read);
    Some(s.into_raw_fd())
}

struct Account {
    uid: libc::uid_t,
    gid: libc::gid_t,
    home: String,
    shell: String,
}

fn account(user: &str) -> Option<Account> {
    let c = CString::new(user).ok()?;
    // SAFETY: getpwnam's static entry, copied at once.
    unsafe {
        let pw = libc::getpwnam(c.as_ptr());
        if pw.is_null() {
            return None;
        }
        let s = |p: *const libc::c_char| if p.is_null() { String::new() } else { std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned() };
        Some(Account { uid: (*pw).pw_uid, gid: (*pw).pw_gid, home: s((*pw).pw_dir), shell: s((*pw).pw_shell) })
    }
}

/// In the child: a session of its own on the seat's terminal, the
/// person's groups and ids, their home, then the desktop's command with
/// `vars` only. Returns only if something failed (why).
fn become_and_exec(setup: &Setup, account: &Account, tty: &str, command: &[String], vars: &[(String, String)]) -> String {
    let argv: Vec<CString> = command.iter().filter_map(|a| CString::new(a.as_str()).ok()).collect();
    if argv.is_empty() {
        return "no command".into();
    }
    let envp: Vec<CString> = vars.iter().filter_map(|(k, v)| CString::new(format!("{k}={v}")).ok()).collect();
    // SAFETY: plain system calls in the forked child, on our own values.
    unsafe {
        if setup.test.is_none() {
            libc::setsid();
            let path = CString::new(format!("/dev/{tty}")).unwrap_or_default();
            let fd = libc::open(path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
            if fd < 0 {
                return format!("/dev/{tty}: {}", std::io::Error::last_os_error());
            }
            libc::ioctl(fd, libc::TIOCSCTTY, 1);
            for to in 0..3 {
                libc::dup2(fd, to);
            }
            if fd > 2 {
                libc::close(fd);
            }
            // What it says to the journal, as GDM's sessions do, not to
            // the terminal: on a terminal another took (Fedora 45's
            // kmscon), writing failed and Olo's programs ended; and the
            // journal keeps it (`journalctl -t olo-session`).
            let program = if command.get(1).is_some_and(|a| a == "--exec") { command.get(2) } else { command.first() };
            if let Some(out) = journal_stream(program.and_then(|p| p.rsplit('/').next()).unwrap_or("olo-session")) {
                libc::dup2(out, 1);
                libc::dup2(out, 2);
                libc::close(out);
            }
            let name = CString::new(setup.user.as_str()).unwrap_or_default();
            if libc::initgroups(name.as_ptr(), account.gid) != 0 || libc::setgid(account.gid) != 0 || libc::setuid(account.uid) != 0 {
                return format!("becoming {}: {}", setup.user, std::io::Error::last_os_error());
            }
            // (No way back.)
            if libc::setuid(0) == 0 {
                return "root could be taken back".into();
            }
        }
        let home = CString::new(account.home.as_str()).unwrap_or_default();
        if libc::chdir(home.as_ptr()) != 0 {
            libc::chdir(c"/".as_ptr());
        }
        let mut a: Vec<*const libc::c_char> = argv.iter().map(|s| s.as_ptr()).collect();
        a.push(std::ptr::null());
        let mut e: Vec<*const libc::c_char> = envp.iter().map(|s| s.as_ptr()).collect();
        e.push(std::ptr::null());
        // (execvpe: the command found on the session's PATH.)
        libc::execvpe(a[0], a.as_ptr(), e.as_ptr());
    }
    format!("{}: {}", command[0], std::io::Error::last_os_error())
}
