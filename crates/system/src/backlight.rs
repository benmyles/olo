//! The built-in display's backlight. Read from sysfs; written through
//! logind's `Session.SetBrightness`, which lets the session's own user
//! change it without root.

use std::path::{Path, PathBuf};

use crate::{Result, bus};

#[derive(Debug, Clone, PartialEq)]
pub struct Backlight {
    /// The sysfs device name ("apple-panel-bl", "intel_backlight").
    pub name: String,
    pub max: u32,
    dir: PathBuf,
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}

/// The panel's backlight: firmware and platform interfaces before raw
/// ones, as the kernel recommends.
pub fn internal() -> Option<Backlight> {
    let mut all: Vec<(u8, Backlight)> = std::fs::read_dir("/sys/class/backlight")
        .ok()?
        .flatten()
        .filter_map(|e| {
            let dir = e.path();
            let max: u32 = read(&dir.join("max_brightness"))?.parse().ok().filter(|&m| m > 0)?;
            let rank = match read(&dir.join("type")).as_deref() {
                Some("firmware") => 0,
                Some("platform") => 1,
                _ => 2,
            };
            Some((rank, Backlight { name: e.file_name().to_string_lossy().into_owned(), max, dir }))
        })
        .collect();
    all.sort_by_key(|(rank, b)| (*rank, b.name.clone()));
    all.into_iter().next().map(|(_, b)| b)
}

/// Perceived brightness (0–1) to a raw value: the eye's response is far
/// from linear, so the low end gets most of the range.
pub fn to_raw(level: f32, max: u32) -> u32 {
    let level = level.clamp(0.0, 1.0);
    if level == 0.0 {
        return 0;
    }
    ((level * level * max as f32).round() as u32).clamp(1, max)
}

pub fn to_level(raw: u32, max: u32) -> f32 {
    if max == 0 { 0.0 } else { (raw.min(max) as f32 / max as f32).sqrt() }
}

impl Backlight {
    pub fn raw(&self) -> Option<u32> {
        read(&self.dir.join("brightness"))?.parse().ok()
    }

    /// Perceived brightness, 0–1.
    pub fn level(&self) -> Option<f32> {
        Some(to_level(self.raw()?, self.max))
    }

    pub fn set_raw(&self, raw: u32) -> Result<()> {
        let conn = bus::system()?;
        let set = |session: &str| {
            bus::call::<_, ()>(
                conn,
                "org.freedesktop.login1",
                session,
                "org.freedesktop.login1.Session",
                "SetBrightness",
                &("backlight", self.name.as_str(), raw.min(self.max)),
            )
        };
        // Our own session if we are in one, else the user's graphical one.
        set("/org/freedesktop/login1/session/auto").or_else(|_| {
            let (_, path): (String, zvariant::OwnedObjectPath) = bus::property(
                conn,
                "org.freedesktop.login1",
                "/org/freedesktop/login1/user/self",
                "org.freedesktop.login1.User",
                "Display",
            )?;
            set(path.as_str())
        })
    }

    pub fn set_level(&self, level: f32) -> Result<()> {
        self.set_raw(to_raw(level, self.max))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perceptual_curve() {
        assert_eq!(to_raw(0.0, 500), 0);
        assert_eq!(to_raw(1.0, 500), 500);
        assert_eq!(to_raw(0.01, 500), 1);
        assert_eq!(to_raw(0.5, 500), 125);
        for raw in [1, 58, 125, 333, 500] {
            assert_eq!(to_raw(to_level(raw, 500), 500), raw);
        }
    }

    /// Writes the current brightness back (no visible change).
    #[test]
    #[ignore]
    fn live_rewrite() {
        let b = internal().expect("a backlight");
        let raw = b.raw().unwrap();
        b.set_raw(raw).expect("logind SetBrightness");
        assert_eq!(b.raw(), Some(raw));
    }

    #[test]
    #[ignore]
    fn live_read() {
        let b = internal().expect("a backlight");
        eprintln!("{} raw {:?} of {} = {:?}", b.name, b.raw(), b.max, b.level());
    }
}
