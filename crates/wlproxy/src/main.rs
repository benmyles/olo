//! olo-wlproxy: the Wayland socket other programs connect to.
//!
//! olo-session hands it the session's `wayland-N` listening socket and
//! the path of app_server's socket for proxied connections. Each program
//! that connects gets its own connection to app_server, opened with a
//! hello naming the program's pid (app_server would otherwise see ours).
//! When app_server restarts (an upgrade, a crash), the program's state is
//! replayed on the new one and the program carries on: see `client`.
//! Olo's own apps don't come through here; they reconnect by
//! themselves.
//!
//! When the proxy itself is upgraded, olo-session puts a new listening
//! socket in the old one's place and starts the new proxy on it, then
//! sends this one SIGWINCH (ignored by default, so a proxy from before
//! this carries on rather than dying): it stops accepting and serves the
//! programs it has until the last one closes, then exits.

mod client;
mod interfaces;
mod wire;

use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use client::{Client, Phase};
use wire::Conn;

/// How long a program waits for app_server to come back.
const PATIENCE: Duration = Duration::from_secs(30);
const RETRY: Duration = Duration::from_millis(100);
/// Requests a program may send while it waits.
const MAX_BACKLOG: usize = 64 << 20;

fn log(msg: impl AsRef<str>) {
    eprintln!("olo-wlproxy: {}", msg.as_ref());
}

/// A newer proxy has the socket now (SIGWINCH).
static RETIRE: AtomicBool = AtomicBool::new(false);

extern "C" fn retire(_: libc::c_int) {
    RETIRE.store(true, Ordering::Relaxed);
}

fn main() {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    // Every outside program's Wayland traffic goes through here: with the
    // shell, above apps (olo_protocol::performance::tier; -5).
    let _ = olo_system::priority::raise_self(-5);
    let (Some(fd), Some(upstream)) = (std::env::var("OLO_WAYLAND_FD").ok().and_then(|f| f.parse::<i32>().ok()), std::env::var_os("OLO_WAYLAND_UPSTREAM")) else {
        eprintln!("olo-wlproxy is started by olo-session (OLO_WAYLAND_FD, OLO_WAYLAND_UPSTREAM)");
        std::process::exit(2);
    };
    // SAFETY: olo-session passes this listening socket to us alone.
    let mut listener = Some(unsafe { UnixListener::from_raw_fd(fd) });
    // SAFETY: fcntl on our own descriptor.
    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    listener.as_ref().unwrap().set_nonblocking(true).expect("non-blocking listener");
    raise_fd_limit();
    // SAFETY: the handler only stores to an atomic; no SA_RESTART, so it
    // interrupts the poll.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = retire as *const () as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGWINCH, &sa, std::ptr::null_mut());
    }
    let upstream = PathBuf::from(upstream);
    log(format!("listening; app_server at {}", upstream.display()));

    let mut clients: Vec<Client> = Vec::new();
    loop {
        if RETIRE.load(Ordering::Relaxed) && listener.take().is_some() {
            log(format!("a newer proxy has the socket; serving {} program(s) until they close", clients.len()));
        }
        if listener.is_none() && clients.is_empty() {
            log("retired, and its last program has closed");
            std::process::exit(0);
        }
        let listening = listener.as_ref().map_or(-1, |l| l.as_raw_fd());
        let mut fds = vec![libc::pollfd { fd: listening, events: libc::POLLIN, revents: 0 }];
        for c in &clients {
            fds.push(libc::pollfd { fd: c.down.fd(), events: libc::POLLIN | if c.down.backlog() > 0 { libc::POLLOUT } else { 0 }, revents: 0 });
            let up = c.up.as_ref().map(|u| (u.fd(), u.backlog() > 0));
            fds.push(libc::pollfd { fd: up.map_or(-1, |u| u.0), events: libc::POLLIN | if up.is_some_and(|u| u.1) { libc::POLLOUT } else { 0 }, revents: 0 });
        }
        let waiting = clients.iter().any(|c| c.phase == Phase::Down);
        let timeout = if waiting { RETRY.as_millis() as i32 } else { -1 };
        // SAFETY: polling our own descriptors.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout) };
        if n < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            log(format!("poll: {}", std::io::Error::last_os_error()));
            std::process::exit(1);
        }

        let mut gone = vec![false; clients.len()];
        for (i, c) in clients.iter_mut().enumerate() {
            let (down, up) = (fds[1 + 2 * i].revents, fds[2 + 2 * i].revents);
            if let Err(e) = step(c, down, up, &upstream) {
                log(format!("pid {}: {e}; closing", c.pid));
                gone[i] = true;
            }
        }
        let mut i = 0;
        clients.retain(|_| {
            i += 1;
            !gone[i - 1]
        });

        if fds[0].revents & libc::POLLIN != 0
            && let Some(listener) = &listener
        {
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let (pid, uid) = peer(&stream);
                        match Conn::new(stream) {
                            Ok(down) => {
                                let mut c = Client::new(down, pid, uid);
                                let _ = step(&mut c, 0, 0, &upstream);
                                clients.push(c);
                            }
                            Err(e) => log(format!("new connection: {e}")),
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => {
                        log(format!("accept: {e}"));
                        break;
                    }
                }
            }
        }
    }
}

/// Handle one program's sockets.
fn step(c: &mut Client, down: i16, up: i16, upstream: &std::path::Path) -> Result<(), String> {
    let hangup = libc::POLLHUP | libc::POLLERR;
    if down & (libc::POLLIN | hangup) != 0 {
        let open = c.down.fill().map_err(|e| e.to_string())?;
        c.read_client()?;
        if !open {
            return Err("closed".into());
        }
    }
    if up & (libc::POLLIN | hangup) != 0 {
        let open = match c.up.as_mut().map(|u| u.fill()) {
            Some(Ok(open)) => open,
            Some(Err(_)) => false,
            None => true,
        };
        c.read_server()?;
        if !open && !c.lost() {
            return Err("app_server ended the connection".into());
        }
    }
    if c.phase == Phase::Down {
        let since = c.down_since.unwrap_or_else(Instant::now);
        if since.elapsed() > PATIENCE {
            return Err("app_server didn't come back".into());
        }
        if c.last_try.is_none_or(|t| t.elapsed() >= RETRY) {
            c.last_try = Some(Instant::now());
            if let Ok(s) = UnixStream::connect(upstream)
                && let Ok(conn) = Conn::new(s)
            {
                c.connected(conn);
            }
        }
        if c.down.backlog() > MAX_BACKLOG || c.waiting() > 200_000 {
            return Err("too much waiting while app_server was away".into());
        }
    }
    if let Some(u) = c.up.as_mut()
        && u.flush().is_err()
        && !c.lost()
    {
        return Err("app_server ended the connection".into());
    }
    c.down.flush().map_err(|e| e.to_string())?;
    if c.down.backlog() > MAX_BACKLOG {
        return Err("not reading its messages".into());
    }
    Ok(())
}

fn peer(s: &UnixStream) -> (i32, u32) {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: getsockopt into a ucred of the right size.
    unsafe { libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) };
    (cred.pid, cred.uid)
}

/// Buffers' descriptors are kept to make them again: allow plenty.
fn raise_fd_limit() {
    let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: get/setrlimit on our own process.
    unsafe {
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) == 0 {
            lim.rlim_cur = lim.rlim_max;
            libc::setrlimit(libc::RLIMIT_NOFILE, &lim);
        }
    }
}
