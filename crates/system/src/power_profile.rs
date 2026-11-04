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

use serde::{Deserialize, Serialize};
use zvariant::OwnedValue;

use crate::{Result, bus};

/// Where the daemon is: its name, object and interface, newest first.
const PLACES: &[(&str, &str, &str)] = &[
    ("org.freedesktop.UPower.PowerProfiles", "/org/freedesktop/UPower/PowerProfiles", "org.freedesktop.UPower.PowerProfiles"),
    ("net.hadess.PowerProfiles", "/net/hadess/PowerProfiles", "net.hadess.PowerProfiles"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    PowerSaver,
    Balanced,
    Performance,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Performance, Mode::Balanced, Mode::PowerSaver];

    /// The daemon's name for it.
    pub fn id(self) -> &'static str {
        match self {
            Mode::PowerSaver => "power-saver",
            Mode::Balanced => "balanced",
            Mode::Performance => "performance",
        }
    }

    pub fn from_id(id: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::PowerSaver => "Power Saver",
            Mode::Balanced => "Balanced",
            Mode::Performance => "Performance",
        }
    }

    /// What it does, in a sentence.
    pub fn about(self) -> &'static str {
        match self {
            Mode::Performance => "As fast as the computer goes: the processors kept quick and the firmware's power limits raised, the fans as needed. The battery runs down sooner.",
            Mode::Balanced => "Fast when there is work, frugal when there isn't. The usual choice.",
            Mode::PowerSaver => "Slower, cooler and quieter: the processors favour saving energy and the firmware lowers its power limits. The battery lasts longest.",
        }
    }
}

/// The modes the daemon offers, the one in use, and what it acts on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Modes {
    pub active: Mode,
    pub available: Vec<Mode>,
    /// Why Performance is held back now, if it is: the computer on a lap,
    /// running hot (the daemon's words: "lap-detected",
    /// "high-operating-temperature").
    pub degraded: Option<String>,
    /// What the daemon drives: the processors' ("intel_pstate") and the
    /// platform's ("platform_profile") drivers, each once.
    pub drivers: Vec<String>,
}

impl Modes {
    /// Why Performance is held back, said plainly.
    pub fn degraded_text(&self) -> Option<String> {
        let why = self.degraded.as_deref()?;
        Some(match why {
            "lap-detected" => "Performance is held back while the computer is on a lap.".into(),
            "high-operating-temperature" => "Performance is held back while the computer runs hot.".into(),
            other => format!("Performance is held back ({other})."),
        })
    }
}

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

/// Calls `changed` with the modes now and whenever they change (a mode
/// chosen here or anywhere else, a ThinkPad's Fn keys, Performance held
/// back), on threads of its own, by the daemon's signals: nothing polls.
pub fn watch(changed: impl Fn(Option<Modes>) + Send + 'static) {
    let _ = std::thread::Builder::new().name("power-modes".into()).spawn(move || {
        let mut last = modes().ok().flatten();
        changed(last.clone());
        let Some((dest, path, _)) = place() else { return };
        let Ok(conn) = bus::system() else { return };
        let Some(signals) = (|| {
            let rule = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .sender(dest)
                .ok()?
                .path(path)
                .ok()?
                .interface("org.freedesktop.DBus.Properties")
                .ok()?
                .member("PropertiesChanged")
                .ok()?
                .build();
            zbus::blocking::MessageIterator::for_match_rule(rule, conn, Some(16)).ok()
        })() else {
            return;
        };
        for _ in signals.flatten() {
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
        assert_eq!(held.degraded_text().as_deref(), Some("Performance is held back while the computer is on a lap."));
    }

    /// The machine's own daemon, if it has one (`cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn live() {
        eprintln!("{:?}", modes());
    }
}
