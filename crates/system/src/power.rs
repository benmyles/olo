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
