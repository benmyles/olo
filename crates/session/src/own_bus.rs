//! An agent screen's own session bus (`olo-session --own-bus ADDRESS`,
//! which `olo-screen start` passes): `olo-bus` at ADDRESS, started before
//! anything else, and every program the session starts given it
//! (`DBUS_SESSION_BUS_ADDRESS`). On the person's bus, a GTK app on the
//! screen found the person's copy by its name and opened its window on
//! their display, and a portal's dialog asked from the screen showed
//! there too.
//!
//! The person's accessibility bus stays the one the screen's programs
//! use (`OLO_A11Y_BUS`, which `olo-screen` asks theirs for: given as
//! `AT_SPI_BUS_ADDRESS`, and answered by the bus's `org.a11y.Bus`), so
//! `olo-screen tree` reads GTK and Qt programs there as before. With it
//! come the services the person's session runs on their bus, for the
//! screen's programs (`services`): its notification centre (shown on the
//! screen) and Olo's portal, behind
//! xdg-desktop-portal, whose permission store and document portal
//! `olo-bus` stands in for.

use std::{
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use crate::log;

pub struct Bus {
    pub address: String,
    pub argv: Vec<String>,
    pub pid: i32,
}

/// The variable `olo-screen` names the person's accessibility bus in.
const A11Y_ENV: &str = "OLO_A11Y_BUS";

/// Start the bus and point this process's environment (so everything it
/// starts) at it. None if it couldn't be: the screen goes on with the
/// person's.
pub fn start(address: &str) -> Option<Bus> {
    let a11y = std::env::var(A11Y_ENV).ok().filter(|a| !a.is_empty());
    let mut argv = vec!["olo-bus".to_owned(), address.to_owned()];
    if let Some(a) = &a11y {
        argv.extend(["--a11y".to_owned(), a.clone()]);
    }
    // The portal's backends, which the session starts beside the
    // frontend (`services`): waited for when it asks.
    argv.extend(["--expect".to_owned(), "org.freedesktop.impl.portal.desktop.olo".to_owned()]);
    let program = crate::exe().parent().map(|d| d.join("olo-bus")).filter(|p| p.exists());
    let Some(program) = program else {
        log("olo-bus isn't installed: the screen shares the person's session bus");
        return None;
    };
    let mut cmd = Command::new(program);
    cmd.args(&argv[1..]).args(["--ready", "--with-parent"]).stdin(Stdio::null()).stdout(Stdio::piped());
    // SAFETY: async-signal-safe calls only, between fork and exec: the
    // session's blocked signals (`block_signals`) not passed on.
    unsafe {
        use std::os::unix::process::CommandExt as _;
        cmd.pre_exec(|| {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
            Ok(())
        });
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            log(format!("the screen's bus couldn't be started ({e}): it shares the person's"));
            return None;
        }
    };
    let pid = child.id() as i32;
    // Listening once it says so (a few milliseconds).
    let out = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(out).read_line(&mut line);
        let _ = tx.send(line);
    });
    let until = Instant::now() + Duration::from_secs(3);
    let ready = rx.recv_timeout(until.saturating_duration_since(Instant::now())).is_ok_and(|l| l.trim() == "ready");
    // Reaped by the session's `reap`, as the shell's are.
    std::mem::forget(child);
    if !ready {
        log("the screen's bus didn't start in time: it shares the person's");
        // SAFETY: our own child.
        unsafe { libc::kill(pid, libc::SIGTERM) };
        return None;
    }
    // SAFETY: single-threaded start-up (the thread above has ended).
    unsafe {
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", address);
        match &a11y {
            Some(a) => std::env::set_var("AT_SPI_BUS_ADDRESS", a),
            None => std::env::remove_var("AT_SPI_BUS_ADDRESS"),
        }
        std::env::remove_var(A11Y_ENV);
    }
    log(format!("the screen's own session bus at {address} (pid {pid})"));
    Some(Bus { address: address.to_owned(), argv, pid })
}

/// What xdg-desktop-portal is started with on a screen's bus: the
/// desktop is Olo alone, so it reads Olo's portal configuration and not
/// GNOME's too (`graphical::PORTAL_DROP_IN` does the same for a real
/// session's).
pub const PORTAL_FRONTEND_ENV: (&str, &str) = ("XDG_CURRENT_DESKTOP", "Olo");

/// The services a screen with a bus of its own runs on it: Olo's
/// notification centre and portal backend (each told the bus, as they
/// otherwise serve only a real session), and xdg-desktop-portal in front
/// of the portal, where it's installed.
pub fn services(address: &str) -> Vec<Vec<String>> {
    let mut out = vec![vec!["olo-notifications".to_owned(), "--bus".to_owned(), address.to_owned()], vec!["olo-portal".to_owned(), "--bus".to_owned(), address.to_owned()]];
    let found = |name: &str| ["/usr/libexec", "/usr/lib"].iter().map(|d| std::path::Path::new(d).join(name)).find(|p| p.exists());
    // (Olo's backend serves every interface the screen's programs ask
    // for, Settings too: the frontend is told the desktop is Olo alone,
    // `PORTAL_FRONTEND_ENV`, as a real session's is.)
    if let Some(frontend) = found("xdg-desktop-portal") {
        out.push(vec![frontend.display().to_string()]);
    }
    out
}
