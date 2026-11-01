//! The system's systemd manager: what a unit is doing, and starting,
//! stopping, enabling and disabling it (as root, or as whoever polkit
//! lets). The session's own (user) manager is in env.rs.

use std::time::{Duration, Instant};

use crate::{Result, bus};

const SYSTEMD: &str = "org.freedesktop.systemd1";
const MANAGER: &str = "/org/freedesktop/systemd1";
const MANAGER_IFACE: &str = "org.freedesktop.systemd1.Manager";
const UNIT_IFACE: &str = "org.freedesktop.systemd1.Unit";

/// A system unit as systemd sees it.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UnitState {
    /// There is such a unit (its LoadState isn't "not-found").
    pub loaded: bool,
    pub active: bool,
    /// Starts at boot, or with something that does (enabled, linked,
    /// static).
    pub enabled: bool,
    /// Its unit file.
    pub path: Option<String>,
}

/// What a unit is doing (one that isn't there comes back `loaded:
/// false`); Err only when systemd can't be asked.
pub fn unit_state(name: &str) -> Result<UnitState> {
    let conn = bus::system()?;
    let path: zvariant::OwnedObjectPath = bus::call(conn, SYSTEMD, MANAGER, MANAGER_IFACE, "LoadUnit", &(name,))?;
    let prop = |p: &str| bus::property::<String>(conn, SYSTEMD, path.as_str(), UNIT_IFACE, p);
    if prop("LoadState")? == "not-found" {
        return Ok(UnitState::default());
    }
    let active = prop("ActiveState")?;
    let file = prop("UnitFileState")?;
    Ok(UnitState {
        loaded: true,
        active: matches!(active.as_str(), "active" | "activating" | "reloading"),
        enabled: matches!(file.as_str(), "enabled" | "enabled-runtime" | "linked" | "linked-runtime" | "alias" | "indirect" | "static"),
        path: prop("FragmentPath").ok().filter(|p| !p.is_empty()),
    })
}

/// Start (`start` true) or stop a unit, and wait (up to five seconds)
/// for it to get there, so what is asked straight after sees it.
pub fn start_stop(name: &str, start: bool) -> Result<()> {
    let method = if start { "StartUnit" } else { "StopUnit" };
    bus::call::<_, zvariant::OwnedObjectPath>(bus::system()?, SYSTEMD, MANAGER, MANAGER_IFACE, method, &(name, "replace"))?;
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        if unit_state(name).is_ok_and(|s| s.active == start) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// Have a unit start at boot (`enable` true), or not.
pub fn enable(name: &str, enable: bool) -> Result<()> {
    let conn = bus::system()?;
    if enable {
        bus::call::<_, (bool, Vec<(String, String, String)>)>(conn, SYSTEMD, MANAGER, MANAGER_IFACE, "EnableUnitFiles", &(vec![name], false, true))?;
    } else {
        bus::call::<_, Vec<(String, String, String)>>(conn, SYSTEMD, MANAGER, MANAGER_IFACE, "DisableUnitFiles", &(vec![name], false))?;
    }
    Ok(())
}

/// Have systemd read its unit files again (after one is removed).
pub fn reload() -> Result<()> {
    bus::call::<_, ()>(bus::system()?, SYSTEMD, MANAGER, MANAGER_IFACE, "Reload", &())
}
