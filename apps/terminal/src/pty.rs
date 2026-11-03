//! The pseudo-terminal a shell runs in (olo-termd's). Unlike
//! alacritty's own `tty::Pty`, ours can also adopt a PTY and shell this
//! process inherited: when olo-termd upgrades itself in place the
//! shells keep running and the new version picks them up.

use std::{
    ffi::CString,
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    path::Path,
};

use alacritty_terminal::event::WindowSize;

fn winsize(size: WindowSize) -> libc::winsize {
    libc::winsize { ws_row: size.num_lines, ws_col: size.num_cols, ws_xpixel: size.num_cols.saturating_mul(size.cell_width), ws_ypixel: size.num_lines.saturating_mul(size.cell_height) }
}

pub fn set_cloexec(fd: RawFd, on: bool) {
    // SAFETY: fcntl on an fd we own.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        let flags = if on { flags | libc::FD_CLOEXEC } else { flags & !libc::FD_CLOEXEC };
        libc::fcntl(fd, libc::F_SETFD, flags);
    }
}

/// The user's login shell as the account has it now (one changed since
/// login, Olo Suggestions' brush, is the next window's at once), else
/// `$SHELL` (what it was at login).
fn shell(env: &[(String, String)]) -> String {
    // SAFETY: getpwuid returns a pointer into static storage or null.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_shell.is_null() {
            let s = std::ffi::CStr::from_ptr((*pw).pw_shell).to_string_lossy().into_owned();
            if !s.is_empty() {
                return s;
            }
        }
    }
    if let Some((_, s)) = env.iter().find(|(k, _)| k == "SHELL")
        && !s.is_empty()
    {
        return s.clone();
    }
    "/bin/sh".into()
}

/// A shell in a PTY: the master side and the shell's pid.
pub struct Pty {
    pub master: OwnedFd,
    pub pid: libc::pid_t,
}

impl Pty {
    /// Start the user's shell (or `command`) in a new PTY, with `env`.
    pub fn spawn(command: Option<&[String]>, cwd: Option<&Path>, size: WindowSize, env: &[(String, String)]) -> io::Result<Pty> {
        let mut master: RawFd = -1;
        let mut slave: RawFd = -1;
        let ws = winsize(size);
        // SAFETY: openpty fills in two fds.
        if unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), &ws) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openpty just gave us these.
        let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        set_cloexec(master.as_raw_fd(), true);
        set_cloexec(slave.as_raw_fd(), true);
        // UTF-8 input processing (erase whole characters).
        // SAFETY: termios calls on our slave fd.
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(slave.as_raw_fd(), &mut t) == 0 {
                t.c_iflag |= libc::IUTF8;
                libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &t);
            }
        }

        let shell = shell(env);
        let (program, args): (String, Vec<String>) = match command {
            Some([p, rest @ ..]) => (p.clone(), rest.to_vec()),
            _ => (shell.clone(), Vec::new()),
        };
        // A login shell when starting the default shell, like BeOS
        // Terminal (argv[0] starting with '-').
        let argv0 = if command.is_none() { format!("-{}", Path::new(&shell).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(shell.clone())) } else { program.clone() };
        let c_program = CString::new(program.clone()).map_err(io::Error::other)?;
        let c_args: Vec<CString> = std::iter::once(argv0).chain(args).map(|a| CString::new(a).map_err(io::Error::other)).collect::<io::Result<_>>()?;
        let mut c_argv: Vec<*const libc::c_char> = c_args.iter().map(|a| a.as_ptr()).collect();
        c_argv.push(std::ptr::null());

        let mut env: Vec<(String, String)> = env.iter().filter(|(k, _)| !matches!(k.as_str(), "XDG_ACTIVATION_TOKEN" | "DESKTOP_STARTUP_ID" | "OLO_RESTORE")).cloned().collect();
        for (k, v) in [("TERM", "xterm-256color"), ("COLORTERM", "truecolor"), ("TERM_PROGRAM", "Olo-Terminal")] {
            env.retain(|(ek, _)| ek != k);
            env.push((k.into(), v.into()));
        }
        let c_env: Vec<CString> = env.iter().filter_map(|(k, v)| CString::new(format!("{k}={v}")).ok()).collect();
        let mut c_envp: Vec<*const libc::c_char> = c_env.iter().map(|e| e.as_ptr()).collect();
        c_envp.push(std::ptr::null());
        let c_cwd = cwd.and_then(|d| CString::new(d.as_os_str().as_encoded_bytes()).ok());

        let (m, s) = (master.as_raw_fd(), slave.as_raw_fd());
        // SAFETY: between fork and exec the child only calls
        // async-signal-safe functions on pre-built buffers.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(io::Error::last_os_error());
        }
        if pid == 0 {
            // SAFETY: in the child; see above.
            unsafe {
                libc::setsid();
                libc::ioctl(s, libc::TIOCSCTTY as _, 0);
                libc::dup2(s, 0);
                libc::dup2(s, 1);
                libc::dup2(s, 2);
                if s > 2 {
                    libc::close(s);
                }
                libc::close(m);
                if let Some(d) = &c_cwd {
                    libc::chdir(d.as_ptr());
                }
                for sig in [libc::SIGCHLD, libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM, libc::SIGALRM, libc::SIGPIPE] {
                    libc::signal(sig, libc::SIG_DFL);
                }
                let mut none: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut none);
                libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
                libc::execve(c_program.as_ptr(), c_argv.as_ptr(), c_envp.as_ptr());
                // Not a path: search $PATH.
                libc::execvpe(c_program.as_ptr(), c_argv.as_ptr(), c_envp.as_ptr());
                libc::_exit(127);
            }
        }
        drop(slave);
        Ok(Pty { master, pid })
    }

    /// Take over a PTY master and shell inherited from our previous self.
    pub fn adopt(fd: RawFd, pid: libc::pid_t) -> io::Result<Pty> {
        // SAFETY: the previous version passed this fd to us alone.
        if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
            return Err(io::Error::last_os_error());
        }
        set_cloexec(fd, true);
        // SAFETY: as above; it is ours now.
        Ok(Pty { master: unsafe { OwnedFd::from_raw_fd(fd) }, pid })
    }
}

/// Tell the PTY (and so the program) the new size.
pub fn resize(fd: RawFd, size: WindowSize) {
    let ws = winsize(size);
    // SAFETY: TIOCSWINSZ on the PTY master.
    unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) };
}
