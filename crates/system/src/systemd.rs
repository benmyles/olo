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

/// Restart a unit (start it if it isn't running), and wait (up to five
/// seconds) for it to be running.
pub fn restart(name: &str) -> Result<()> {
    let job = bus::call::<_, zvariant::OwnedObjectPath>(bus::system()?, SYSTEMD, MANAGER, MANAGER_IFACE, "RestartUnit", &(name, "replace"))?;
    // Its job done (a one-shot that stays active, such as
    // systemd-sysusers, is active before it runs again: Olo Setup made a
    // folder for a user not made yet, 2026-11-07).
    wait_for_job(&job, Duration::from_secs(60));
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        if unit_state(name).is_ok_and(|s| s.active) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// Wait (at most `limit`) for systemd's job `job` to be done: its object
/// is gone once it has.
fn wait_for_job(job: &zvariant::OwnedObjectPath, limit: Duration) {
    let Ok(conn) = bus::system() else { return };
    let until = Instant::now() + limit;
    while Instant::now() < until {
        let there: Result<zvariant::OwnedValue> = bus::call(conn, SYSTEMD, job.as_str(), "org.freedesktop.DBus.Properties", "Get", &("org.freedesktop.systemd1.Job", "State"));
        if there.is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
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

/// The unit of the person's own (user) manager process `pid` runs in,
/// from its cgroup ("…/user@1000.service/app.slice/build.service":
/// `build.service`); None for one outside it (the login session's scope,
/// where Olo runs, or the system's).
pub fn user_unit_of(pid: i32) -> Option<String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    user_unit_in(text.lines().find_map(|l| l.strip_prefix("0::"))?)
}

/// `user_unit_of`, from a cgroup's path.
pub fn user_unit_in(cgroup: &str) -> Option<String> {
    let (_, inside) = cgroup.split_once(".service/").filter(|(manager, _)| manager.rsplit('/').next().is_some_and(|m| m.starts_with("user@")))?;
    inside.split('/').rev().find(|part| part.ends_with(".service") || part.ends_with(".scope")).map(str::to_owned)
}

/// The user manager's unit `unit`'s CPU weight as set (None: not set,
/// its parent's default; Some(0): idle).
pub fn user_cpu_weight(unit: &str) -> Result<Option<u64>> {
    let conn = bus::session()?;
    let path: zvariant::OwnedObjectPath = bus::call(conn, SYSTEMD, MANAGER, MANAGER_IFACE, "GetUnit", &(unit,))?;
    let iface = if unit.ends_with(".scope") { "org.freedesktop.systemd1.Scope" } else { "org.freedesktop.systemd1.Service" };
    let value: zvariant::OwnedValue = bus::call(conn, SYSTEMD, path.as_str(), "org.freedesktop.DBus.Properties", "Get", &(iface, "CPUWeight"))?;
    let weight = u64::try_from(value).map_err(|e| zbus::Error::Failure(e.to_string()))?;
    Ok((weight != u64::MAX).then_some(weight))
}

/// Set the user manager's unit `unit`'s CPU weight until it ends (or the
/// manager restarts): Some(0) idle (its cgroup's `cpu.idle`: it runs only
/// when no other cgroup beside it wants the processor), None back to its
/// default. The person's own manager: no privilege asked.
pub fn set_user_cpu_weight(unit: &str, weight: Option<u64>) -> Result<()> {
    let props = vec![("CPUWeight", zvariant::Value::U64(weight.unwrap_or(u64::MAX)))];
    bus::call::<_, ()>(bus::session()?, SYSTEMD, MANAGER, MANAGER_IFACE, "SetUnitProperties", &(unit, true, props))
}

#[cfg(test)]
mod user_unit_tests {
    use super::*;

    #[test]
    fn a_processs_unit_is_read_from_its_cgroup() {
        assert_eq!(user_unit_in("/user.slice/user-1000.slice/user@1000.service/app.slice/shiftup-nightly.service").as_deref(), Some("shiftup-nightly.service"));
        assert_eq!(user_unit_in("/user.slice/user-1000.slice/user@1000.service/app.slice/app-com.google.Chrome-1967770.scope").as_deref(), Some("app-com.google.Chrome-1967770.scope"));
        assert_eq!(user_unit_in("/user.slice/user-1000.slice/user@1000.service/app.slice/kache.service/sub").as_deref(), Some("kache.service"));
        assert_eq!(user_unit_in("/user.slice/user-1000.slice/session-3.scope"), None, "the login session's: Olo's");
        assert_eq!(user_unit_in("/system.slice/dnf.service"), None, "the system's");
        assert_eq!(user_unit_in("/user.slice/user-1000.slice/user@1000.service/init.scope").as_deref(), Some("init.scope"));
    }

    /// On the person's own manager: a scope of this test's, holding a
    /// child of its own, made idle and put back, then gone with it.
    #[test]
    #[ignore = "asks the session's systemd"]
    fn a_units_weight_goes_idle_and_back() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let unit = format!("olo-test-weight-{}.scope", child.id());
        let props = vec![("PIDs", zvariant::Value::from(vec![child.id()]))];
        let aux: Vec<(&str, Vec<(&str, zvariant::Value)>)> = Vec::new();
        let _: zvariant::OwnedObjectPath = bus::call(bus::session().unwrap(), SYSTEMD, MANAGER, MANAGER_IFACE, "StartTransientUnit", &(unit.as_str(), "fail", props, aux)).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(user_unit_of(child.id() as i32).as_deref(), Some(unit.as_str()));
        assert_eq!(user_cpu_weight(&unit).unwrap(), None);
        set_user_cpu_weight(&unit, Some(0)).unwrap();
        assert_eq!(user_cpu_weight(&unit).unwrap(), Some(0));
        let cgroup = std::fs::read_to_string(format!("/proc/{}/cgroup", child.id())).unwrap();
        let dir = format!("/sys/fs/cgroup{}", cgroup.trim().strip_prefix("0::").unwrap());
        assert_eq!(std::fs::read_to_string(format!("{dir}/cpu.idle")).unwrap().trim(), "1");
        set_user_cpu_weight(&unit, None).unwrap();
        assert_eq!(user_cpu_weight(&unit).unwrap(), None);
        assert_eq!(std::fs::read_to_string(format!("{dir}/cpu.idle")).unwrap().trim(), "0");
        let _ = child.kill();
        let _ = child.wait();
    }
}
