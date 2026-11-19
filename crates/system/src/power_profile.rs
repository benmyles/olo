//! Power modes: Power Saver, Balanced, Performance, through the power
//! profiles daemon's D-Bus API (`org.freedesktop.UPower.PowerProfiles`,
//! served by power-profiles-daemon and by Fedora's tuned-ppd; older
//! daemons as `net.hadess.PowerProfiles`). The daemon turns a mode into
//! what the machine has: the processors' energy/performance preference
//! and governor (intel_pstate, amd-pstate), the firmware's platform
//! profile (a ThinkPad's power limits and fans), tuned's other settings.
//! A ThinkPad's own Fn+L, Fn+M and Fn+H change the platform profile, and
//! the daemon follows: [`watch`] tells of it. No daemon: no modes.

use std::collections::HashMap;

use zvariant::OwnedValue;

use crate::{Result, bus};

/// Where the daemon is: its name, object and interface, newest first.
const PLACES: &[(&str, &str, &str)] = &[
    ("org.freedesktop.UPower.PowerProfiles", "/org/freedesktop/UPower/PowerProfiles", "org.freedesktop.UPower.PowerProfiles"),
    ("net.hadess.PowerProfiles", "/net/hadess/PowerProfiles", "net.hadess.PowerProfiles"),
];

pub use olo_protocol::power_mode::{Mode, Modes};

/// The daemon's place on the system bus, if one is running.
fn place() -> Option<(&'static str, &'static str, &'static str)> {
    let conn = bus::system().ok()?;
    PLACES.iter().copied().find(|(dest, ..)| bus::call::<_, bool>(conn, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "NameHasOwner", &(*dest,)).unwrap_or(false))
}

/// The modes now; None with no daemon.
pub fn modes() -> Result<Option<Modes>> {
    let Some((dest, path, iface)) = place() else { return Ok(None) };
    let conn = bus::system()?;
    let active: String = bus::property(conn, dest, path, iface, "ActiveProfile")?;
    let profiles: Vec<HashMap<String, OwnedValue>> = bus::property(conn, dest, path, iface, "Profiles")?;
    let degraded: String = bus::property(conn, dest, path, iface, "PerformanceDegraded").unwrap_or_default();
    let text = |d: &HashMap<String, OwnedValue>, key: &str| d.get(key).and_then(|v| String::try_from(v.clone()).ok());
    let mut available = Vec::new();
    let mut drivers = Vec::new();
    for p in &profiles {
        if let Some(m) = text(p, "Profile").as_deref().and_then(Mode::from_id) {
            available.push(m);
        }
        for key in ["CpuDriver", "PlatformDriver", "Driver"] {
            if let Some(d) = text(p, key).filter(|d| !d.is_empty() && d != "multiple" && !drivers.contains(d)) {
                drivers.push(d);
            }
        }
    }
    available.sort_by_key(|m| Mode::ALL.iter().position(|a| a == m));
    let Some(active) = Mode::from_id(&active) else { return Ok(None) };
    Ok(Some(Modes { active, available, degraded: Some(degraded).filter(|d| !d.is_empty()), drivers }))
}

/// Use `mode` (polkit's `switch-profile`, allowed to the person at the
/// computer).
pub fn set(mode: Mode) -> Result<()> {
    let Some((dest, path, iface)) = place() else { return Err(zbus::Error::Failure("no power profiles daemon is running".into())) };
    bus::set_property(bus::system()?, dest, path, iface, "ActiveProfile", mode.id())
}

/// Return once a power profiles daemon is on the system bus (at once if
/// one is), as the bus says names come (`NameOwnerChanged`): nothing
/// polls. For a service started before the daemon is.
pub fn wait_for_daemon() -> Result<()> {
    let conn = bus::system()?;
    let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).sender("org.freedesktop.DBus")?.interface("org.freedesktop.DBus")?.member("NameOwnerChanged")?.build();
    let signals = zbus::blocking::MessageIterator::for_match_rule(rule, conn, Some(64))?;
    // (Looked for once listening, so one coming in between isn't missed.)
    if place().is_some() {
        return Ok(());
    }
    for m in signals.flatten() {
        let Ok((name, _, new)) = m.body().deserialize::<(String, String, String)>() else { continue };
        if !new.is_empty() && PLACES.iter().any(|(dest, ..)| *dest == name) {
            return Ok(());
        }
    }
    Err(zbus::Error::Failure("the system bus went away".into()))
}

/// TuneD applies its profile on a worker after PPD publishes ActiveProfile.
/// Subscribe before the initial hardware read. Other PPD implementations
/// complete the transition themselves.
pub fn watch_tuned_applied(changed: impl Fn(String) + Send + 'static) -> Result<()> {
    let conn = bus::system()?;
    let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).sender("com.redhat.tuned")?.path("/Tuned")?.interface("com.redhat.tuned.control")?.member("profile_changed")?.build();
    let signals = zbus::blocking::MessageIterator::for_match_rule(rule, conn, Some(16))?;
    std::thread::Builder::new()
        .name("tuned-applied".into())
        .spawn(move || {
            for m in signals.flatten() {
                if let Ok((profile, true, _)) = m.body().deserialize::<(String, bool, String)>() {
                    changed(profile);
                }
            }
        })
        .expect("tuned profile watcher");
    Ok(())
}

pub fn tuned_profile() -> Option<String> {
    bus::call(bus::system().ok()?, "com.redhat.tuned", "/Tuned", "com.redhat.tuned.control", "active_profile", &()).ok()
}

/// Calls `changed` with the modes now and whenever they change (a mode
/// chosen here or anywhere else, a ThinkPad's Fn keys, Performance held
/// back), on threads of its own, by the daemon's signals: nothing polls.
pub fn watch(changed: impl Fn(Option<Modes>) + Send + 'static) {
    let _ = std::thread::Builder::new().name("power-modes".into()).spawn(move || {
        // A private connection has only these signals. Subscribe before
        // reading, including name changes so a daemon can appear or restart.
        let Ok(conn) = zbus::blocking::Connection::system() else {
            changed(None);
            return;
        };
        let signals = zbus::blocking::MessageIterator::from(&conn);
        for (dest, path, _) in PLACES {
            for rule in [
                format!("type='signal',sender='{dest}',path='{path}',interface='org.freedesktop.DBus.Properties',member='PropertiesChanged'"),
                format!("type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged',arg0='{dest}'"),
            ] {
                if let Err(e) = bus::call::<_, ()>(&conn, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "AddMatch", &(rule,)) {
                    tracing::warn!(?e, "power profile subscription");
                }
            }
        }
        let mut last = modes().ok().flatten();
        changed(last.clone());
        for msg in signals.flatten() {
            if msg.message_type() != zbus::message::Type::Signal {
                continue;
            }
            let now = modes().ok().flatten();
            if now != last {
                changed(now.clone());
                last = now;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for m in Mode::ALL {
            assert_eq!(Mode::from_id(m.id()), Some(m));
        }
        assert_eq!(Mode::from_id("turbo"), None);
        let held = Modes { active: Mode::Balanced, available: Mode::ALL.to_vec(), degraded: Some("lap-detected".into()), drivers: vec![] };
        assert_eq!(held.degraded_text().as_deref(), Some("Low Latency is limited while the computer is on a lap."));
    }

    /// The machine's own daemon, if it has one (`cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn live() {
        eprintln!("{:?}", modes());
    }
}
