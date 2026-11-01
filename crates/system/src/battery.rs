//! The battery and mains power, from sysfs (`/sys/class/power_supply`).
//! Cheap reads, but still file system work: call off the UI thread.
//! [`watch`] reads it again only when UPower says it changed.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Charging,
    Discharging,
    Full,
    /// Plugged in but holding (at a charge limit, or too warm).
    NotCharging,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Battery {
    pub percent: u32,
    pub state: State,
    /// What the hardware estimates, when it does.
    pub time_to_empty: Option<Duration>,
    pub time_to_full: Option<Duration>,
    pub cycle_count: Option<u32>,
    /// Full capacity now as a share of what it was new (0–100).
    pub health: Option<u32>,
    /// Charging stops here (macOS's charge limit), when supported.
    pub charge_limit: Option<u32>,
    /// Watts flowing in or out, when reported.
    pub power_watts: Option<f32>,
}

impl Battery {
    pub fn charging(&self) -> bool {
        matches!(self.state, State::Charging | State::Full)
    }
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

fn num(dir: &Path, name: &str) -> Option<u64> {
    read(&dir.join(name))?.parse().ok()
}

fn supplies() -> Vec<PathBuf> {
    std::fs::read_dir("/sys/class/power_supply")
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default()
}

/// The system battery (not a mouse's or a USB device's), if there is one.
pub fn battery() -> Option<Battery> {
    let dir = supplies().into_iter().find(|p| {
        read(&p.join("type")).as_deref() == Some("Battery")
            && read(&p.join("scope")).is_none_or(|s| s == "System")
            && num(p, "present").is_none_or(|n| n == 1)
    })?;
    from_dir(&dir)
}

fn from_dir(dir: &Path) -> Option<Battery> {
    let percent = num(dir, "capacity")? as u32;
    let state = match read(&dir.join("status")).as_deref() {
        Some("Charging") => State::Charging,
        Some("Discharging") => State::Discharging,
        Some("Full") => State::Full,
        Some("Not charging") => State::NotCharging,
        _ => State::Unknown,
    };
    let secs = |name| num(dir, name).filter(|&s| s > 0).map(Duration::from_secs);
    let health = match (num(dir, "energy_full"), num(dir, "energy_full_design")) {
        (Some(f), Some(d)) if d > 0 => Some(((f * 100 + d / 2) / d).min(100) as u32),
        _ => match (num(dir, "charge_full"), num(dir, "charge_full_design")) {
            (Some(f), Some(d)) if d > 0 => Some(((f * 100 + d / 2) / d).min(100) as u32),
            _ => None,
        },
    };
    Some(Battery {
        percent: percent.min(100),
        state,
        time_to_empty: secs("time_to_empty_now"),
        time_to_full: secs("time_to_full_now"),
        cycle_count: num(dir, "cycle_count").map(|n| n as u32),
        health,
        charge_limit: num(dir, "charge_control_end_threshold").map(|n| n as u32),
        power_watts: num(dir, "power_now").filter(|&p| p > 0).map(|p| p as f32 / 1e6),
    })
}

const UPOWER: &str = "org.freedesktop.UPower";
/// UPower's battery as a whole (what a desktop shows).
const DISPLAY_DEVICE: &str = "/org/freedesktop/UPower/devices/DisplayDevice";

/// Calls `changed` with the battery ([`battery`]) now and whenever it
/// changes, on threads of its own: read again when UPower's display
/// device says something changed (UPower follows the kernel's events
/// and polls what doesn't send them, so nothing here wakes in between).
/// With no UPower, once a minute. Quick runs of changes come together.
pub fn watch(changed: impl Fn(Option<Battery>) + Send + 'static) {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let signals = crate::bus::system().ok().and_then(|c| {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender(UPOWER)
            .ok()?
            .path(DISPLAY_DEVICE)
            .ok()?
            .interface("org.freedesktop.DBus.Properties")
            .ok()?
            .member("PropertiesChanged")
            .ok()?
            .build();
        zbus::blocking::MessageIterator::for_match_rule(rule, c, Some(64)).ok()
    });
    // Is UPower there to say?
    let upower = crate::bus::system().is_ok_and(|c| {
        crate::bus::call::<_, bool>(c, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "NameHasOwner", &(UPOWER,)).unwrap_or(false)
    });
    match signals {
        Some(signals) if upower => {
            let _ = std::thread::Builder::new().name("battery-signals".into()).spawn(move || {
                for _ in signals.flatten() {
                    if tx.send(()).is_err() {
                        return;
                    }
                }
            });
        }
        _ => {
            let _ = std::thread::Builder::new().name("battery-poll".into()).spawn(move || {
                while tx.send(()).is_ok() {
                    std::thread::sleep(Duration::from_secs(60));
                }
            });
        }
    }
    let _ = std::thread::Builder::new().name("battery".into()).spawn(move || {
        let mut last = battery();
        changed(last.clone());
        while rx.recv().is_ok() {
            // (UPower sends a few properties apart.)
            std::thread::sleep(Duration::from_millis(50));
            while rx.try_recv().is_ok() {}
            let now = battery();
            if now != last {
                changed(now.clone());
                last = now;
            }
        }
    });
}

/// Plugged into mains power (true on machines without a battery).
pub fn on_ac() -> bool {
    let mains: Vec<PathBuf> = supplies().into_iter().filter(|p| read(&p.join("type")).as_deref() == Some("Mains")).collect();
    if mains.is_empty() {
        return battery().is_none_or(|b| b.state != State::Discharging);
    }
    mains.iter().any(|p| num(p, "online") == Some(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_fake_battery() {
        let dir = std::env::temp_dir().join(format!("olo-battery-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (k, v) in [
            ("capacity", "57"),
            ("status", "Discharging"),
            ("time_to_empty_now", "5400"),
            ("energy_full", "79036200"),
            ("energy_full_design", "99111600"),
            ("cycle_count", "465"),
            ("power_now", "12500000"),
        ] {
            std::fs::write(dir.join(k), v).unwrap();
        }
        let b = from_dir(&dir).unwrap();
        assert_eq!(b.percent, 57);
        assert_eq!(b.state, State::Discharging);
        assert_eq!(b.time_to_empty, Some(Duration::from_secs(5400)));
        assert_eq!(b.time_to_full, None);
        assert_eq!(b.health, Some(80));
        assert_eq!(b.cycle_count, Some(465));
        assert_eq!(b.power_watts, Some(12.5));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
