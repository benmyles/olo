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
    bus::call::<_, ()>(
        conn,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        "SetEnvironment",
        &(assignments,),
    )
}
