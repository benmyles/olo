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

/// Have the session bus read its configuration again, its services'
/// activation files among it (a user's own in
/// `~/.local/share/dbus-1/services`, which it otherwise reads only as it
/// starts).
pub fn reload_session_bus() -> Result<()> {
    bus::call::<_, ()>(bus::session()?, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ReloadConfig", &())
}

/// The systemd user manager's environment (what the services and scopes
/// it starts get: `environment.d` read at its start, and what's been set
/// since), as `KEY=VALUE` pairs.
pub fn manager_environment() -> Result<Vec<(String, String)>> {
    let env: Vec<String> = bus::property(bus::session()?, "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager", "Environment")?;
    Ok(env.into_iter().filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_owned(), v.to_owned()))).collect())
}

/// Whether this program runs in the person's own session, in front of
/// the computer: a real one (`OLO_REAL_SESSION`: olo-session on a seat)
/// and not an agent's screen (`OLO_SCREEN`). Only there does a program
/// go across to root through polkit (which asks the person), or use what
/// is the person's to touch: their fingerprint reader, security key and
/// camera. A test session or an agent's screen has nobody in front of it.
pub fn at_the_computer() -> bool {
    std::env::var_os("OLO_REAL_SESSION").is_some() && std::env::var_os("OLO_SCREEN").is_none()
}

/// Whether the person's fingerprint reader, security key and camera are
/// for this program to wait on: only in front of the computer
/// ([`at_the_computer`], asked once). A test session's or an agent
/// screen's lock, or Passwords there, leaves them alone (an agent's
/// screen that locked itself looked for the person's face whenever the
/// agent moved its pointer, 2026-11-07). A test's own fake key or face
/// service counts anywhere: [`key_here`], [`face_here`].
pub fn readers_here() -> bool {
    static HERE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *HERE.get_or_init(at_the_computer)
}

/// Whether to wait for a security key's touch: [`readers_here`], or a
/// test's soft key (`OLO_FIDO_SOFT`, debug builds' only).
pub fn key_here() -> bool {
    readers_here() || std::env::var_os("OLO_FIDO_SOFT").is_some()
}

/// Whether to look for the person's face: [`readers_here`], or a test's
/// own face service (`OLO_FACE_BUS`).
pub fn face_here() -> bool {
    readers_here() || std::env::var_os("OLO_FACE_BUS").is_some()
}

/// Whether to listen for a finger: [`readers_here`], unless
/// `OLO_NO_FINGERPRINT`; or (debug builds) a test's own fprintd
/// (`OLO_FPRINT_BUS`).
pub fn finger_here() -> bool {
    readers_here() && std::env::var_os("OLO_NO_FINGERPRINT").is_none() || cfg!(debug_assertions) && std::env::var_os("OLO_FPRINT_BUS").is_some()
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

/// The `TMPDIR` Olo's file (`TMPDIR_FILE`, under the config folder
/// `config`) gives the session, if it's there: `TMPDIR=/var/tmp`, quotes
/// and spaces round `=` allowed, as environment.d reads it.
pub fn tmpdir_in_file(config: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(config.join(TMPDIR_FILE)).ok()?;
    text.lines().filter(|l| !l.trim_start().starts_with('#')).find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == "TMPDIR").then(|| v.trim().trim_matches('"').to_owned()).filter(|v| !v.is_empty())
    })
}

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

/// Have the session bus read its service files again (one written or
/// taken away).
pub fn reload_bus_services() -> Result<()> {
    bus::call::<_, ()>(bus::session()?, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ReloadConfig", &())
}

/// Whether the user's systemd unit `name` is active or on its way
/// (`activating`, `reloading`).
pub fn user_unit_active(name: &str) -> bool {
    let Ok(conn) = bus::session() else { return false };
    let Ok(path) = bus::call::<_, zvariant::OwnedObjectPath>(conn, "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager", "GetUnit", &(name,)) else {
        return false;
    };
    let state: Result<zvariant::OwnedValue> = bus::call(conn, "org.freedesktop.systemd1", path.as_str(), "org.freedesktop.DBus.Properties", "Get", &("org.freedesktop.systemd1.Unit", "ActiveState"));
    state.ok().and_then(|v| String::try_from(v).ok()).is_some_and(|s| matches!(s.as_str(), "active" | "activating" | "reloading"))
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
