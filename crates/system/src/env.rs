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

/// The systemd user manager's environment (what the services and scopes
/// it starts get: `environment.d` read at its start, and what's been set
/// since), as `KEY=VALUE` pairs.
pub fn manager_environment() -> Result<Vec<(String, String)>> {
    let env: Vec<String> = bus::property(bus::session()?, "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager", "Environment")?;
    Ok(env.into_iter().filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_owned(), v.to_owned()))).collect())
}

/// Where programs keep their temporary files (`TMPDIR`, and `TMP` and
/// `TEMP` some read instead): on disk, not /tmp (Ben, 2026-11-05: "/tmp is
/// often in memory and not a good place for apps to use as temp"). Olo
/// Doctor's fix writes them for the session in `TMPDIR_FILE`.
pub const TMP_VARS: [&str; 3] = ["TMPDIR", "TMP", "TEMP"];
pub const DISK_TMP: &str = "/var/tmp";
/// Olo's file for systemd's user manager (`environment.d`, read as it
/// starts at each login), under the config folder.
pub const TMPDIR_FILE: &str = "environment.d/60-olo-tmpdir.conf";

/// Is `path` on a file system held in memory (tmpfs, ramfs)?
pub fn in_memory(path: &std::path::Path) -> bool {
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else { return false };
    // SAFETY: statfs into a zeroed struct of ours.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return false;
    }
    const TMPFS: i64 = 0x0102_1994;
    const RAMFS: i64 = 0x8584_58f6;
    matches!(st.f_type as i64, TMPFS | RAMFS)
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
