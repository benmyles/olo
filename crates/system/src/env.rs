//! Export the session's environment to D-Bus activation and the systemd
//! user manager, so services started on demand (portals, …) find our
//! display.

use std::collections::HashMap;

use crate::{Result, bus};

pub fn update_activation_environment(vars: &[(&str, &str)]) -> Result<()> {
    let conn = bus::session()?;
    let map: HashMap<&str, &str> = vars.iter().copied().collect();
    bus::call::<_, ()>(conn, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "UpdateActivationEnvironment", &(map,))?;
    let assignments: Vec<String> = vars.iter().map(|(k, v)| format!("{k}={v}")).collect();
    bus::call::<_, ()>(conn, "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager", "SetEnvironment", &(assignments,))
}

/// Have the systemd user manager re-read its unit files (after writing one).
pub fn reload_units() -> Result<()> {
    bus::call::<_, ()>(bus::session()?, "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager", "Reload", &())
}

/// Start (`start` true) or stop a systemd user unit, without waiting for
/// the job to finish.
pub fn start_unit(name: &str, start: bool) -> Result<()> {
    let method = if start { "StartUnit" } else { "StopUnit" };
    bus::call::<_, zvariant::OwnedObjectPath>(bus::session()?, "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager", method, &(name, "replace")).map(|_| ())
}

/// Restart a systemd user unit if it is running (to read new settings),
/// without waiting for the job to finish.
pub fn try_restart_unit(name: &str) -> Result<()> {
    bus::call::<_, zvariant::OwnedObjectPath>(bus::session()?, "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager", "TryRestartUnit", &(name, "replace"))
        .map(|_| ())
}
