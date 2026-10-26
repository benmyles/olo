//! logind: restart, shut down, sleep, and inhibitor locks (what keeps the
//! machine awake while caffeinated).

use std::os::fd::OwnedFd;

use crate::{Result, bus};

const DEST: &str = "org.freedesktop.login1";
const PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";

fn manager(method: &str) -> Result<()> {
    // `interactive`: let polkit ask if the policy wants a password.
    bus::call::<_, ()>(bus::system()?, DEST, PATH, MANAGER, method, &(true,))
}

pub fn reboot() -> Result<()> {
    manager("Reboot")
}

pub fn power_off() -> Result<()> {
    manager("PowerOff")
}

pub fn suspend() -> Result<()> {
    manager("Suspend")
}

/// An inhibitor lock; dropping it releases the lock.
#[derive(Debug)]
pub struct Inhibitor(#[allow(dead_code)] OwnedFd);

/// Take an inhibitor lock. `what` is a colon-separated list from
/// "shutdown", "sleep", "idle", "handle-lid-switch", …; `block` stops the
/// action outright, otherwise it is only delayed.
pub fn inhibit(what: &str, who: &str, why: &str, block: bool) -> Result<Inhibitor> {
    let mode = if block { "block" } else { "delay" };
    let fd: zvariant::OwnedFd = bus::call(bus::system()?, DEST, PATH, MANAGER, "Inhibit", &(what, who, why, mode))?;
    Ok(Inhibitor(fd.into()))
}

/// What logind tells the session.
#[derive(Debug)]
pub enum LoginEvent {
    /// The machine is about to sleep. Suspend waits (up to logind's
    /// InhibitDelayMaxSec) until the guard is dropped: lock the screen first.
    Sleeping(Option<Inhibitor>),
    /// Back from sleep.
    Woke,
    /// `loginctl lock-session` and friends. (Unlock requests are ignored:
    /// only the password or a fingerprint unlocks.)
    Lock,
}

/// Our logind session: the one this process is in, else the user's
/// graphical one.
fn session_path(conn: &zbus::blocking::Connection) -> Result<zvariant::OwnedObjectPath> {
    bus::call(conn, DEST, PATH, MANAGER, "GetSessionByPID", &(std::process::id(),)).or_else(|_| {
        let (_, path): (String, zvariant::OwnedObjectPath) =
            bus::property(conn, DEST, "/org/freedesktop/login1/user/self", "org.freedesktop.login1.User", "Display")?;
        Ok(path)
    })
}

/// Watch logind on two threads (sleep, and the session's lock requests),
/// calling `on` for each event. Holds a sleep delay lock between sleeps.
pub fn watch_login(on: impl Fn(LoginEvent) + Send + Sync + 'static) -> Result<()> {
    let conn = bus::system()?;
    let on = std::sync::Arc::new(on);
    let manager = zbus::blocking::Proxy::new(conn, DEST, PATH, MANAGER)?;
    let mut sleeps = manager.receive_signal("PrepareForSleep")?;
    let delay = || inhibit("sleep", "Tabula", "Locking the screen", false).ok();
    let mut guard = delay();
    let sleep_on = on.clone();
    std::thread::Builder::new().name("logind-sleep".into()).spawn(move || {
        for msg in sleeps.by_ref() {
            let Ok(start) = msg.body().deserialize::<bool>() else { continue };
            if start {
                sleep_on(LoginEvent::Sleeping(guard.take()));
            } else {
                guard = delay();
                sleep_on(LoginEvent::Woke);
            }
        }
    })?;
    let session = session_path(conn)?;
    let proxy = zbus::blocking::Proxy::new(conn, DEST, session, "org.freedesktop.login1.Session")?;
    let mut locks = proxy.receive_signal("Lock")?;
    std::thread::Builder::new().name("logind-lock".into()).spawn(move || {
        for _ in locks.by_ref() {
            on(LoginEvent::Lock);
        }
    })?;
    Ok(())
}
