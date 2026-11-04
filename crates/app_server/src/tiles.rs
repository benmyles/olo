//! Monitors that take their picture in tiles: a 5K or 6K display over
//! two DisplayPort streams (Apple's Studio Display and Pro Display XDR,
//! LG's UltraFine 5K, Dell's UP3218K), each stream a connector of its
//! own with the kernel's `TILE` property saying which part it is. Most
//! also offer the whole picture through one of them (with DSC, as a
//! Thunderbolt 4 or 5 port can): then that one is the display, and the
//! others are left off. Set up one by one, each tile was a display of
//! its own (the Studio Display was D0 at 5120 × 2880 and D2 at
//! 2560 × 2880, the second a copy of its right half).

use smithay::reexports::drm::control::{Device as ControlDevice, connector};

/// One connector's part of a tiled monitor (DRM's `TILE` blob:
/// "group:flags:across:down:x:y:width:height").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tile {
    /// The monitor's tiles share it (on one DRM device).
    pub group: u32,
    /// Its tiles are one monitor's (not a wall of monitors).
    pub one_monitor: bool,
    pub across: u32,
    pub down: u32,
    /// Its place among them, in tiles.
    pub x: u32,
    pub y: u32,
    /// Its size, in pixels.
    pub width: u32,
    pub height: u32,
}

impl Tile {
    pub fn parse(blob: &[u8]) -> Option<Tile> {
        let text = std::str::from_utf8(blob).ok()?.trim_end_matches('\0').trim();
        let n: Vec<u32> = text.split(':').map(|s| s.trim().parse().ok()).collect::<Option<_>>()?;
        let [group, flags, across, down, x, y, width, height] = n[..] else { return None };
        (across * down > 1 && x < across && y < down && width > 0 && height > 0).then_some(Tile { group, one_monitor: flags & 1 == 1, across, down, x, y, width, height })
    }

    /// The whole picture's size, in pixels.
    pub fn whole(&self) -> (u32, u32) {
        (self.across * self.width, self.down * self.height)
    }
}

/// `connector`'s tile, if it's part of a tiled monitor.
pub fn of(drm: &impl ControlDevice, connector: connector::Handle) -> Option<Tile> {
    let props = drm.get_properties(connector).ok()?;
    let (ids, values) = props.as_props_and_values();
    let blob = ids.iter().zip(values).find_map(|(id, value)| (drm.get_property(*id).ok()?.name().to_str() == Ok("TILE")).then_some(*value))?;
    if blob == 0 {
        return None;
    }
    Tile::parse(&drm.get_property_blob(blob).ok()?)
}

/// Whether `info` offers `tile`'s whole picture as one mode.
pub fn offers_whole(info: &connector::Info, tile: &Tile) -> bool {
    let (w, h) = tile.whole();
    info.modes().iter().any(|m| {
        let (mw, mh) = m.size();
        (u32::from(mw), u32::from(mh)) == (w, h)
    })
}

/// Of a monitor's connected tiles (connector, its tile, whether it
/// offers the whole picture), the one to show it whole through: the
/// first tile offering it (top left first). None when none does: each
/// is then set up as a display of its own.
pub fn whole_through<C: Copy>(tiles: &[(C, Tile, bool)]) -> Option<C> {
    tiles.iter().filter(|(_, _, whole)| *whole).min_by_key(|(_, t, _)| (t.y, t.x)).map(|(c, _, _)| *c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_studio_displays_tiles() {
        // As the kernel gives them for an Apple Studio Display over
        // Thunderbolt to a ThinkPad P1 Gen 9.
        let left = Tile::parse(b"1:1:2:1:0:0:2560:2880\0").unwrap();
        let right = Tile::parse(b"1:1:2:1:1:0:2560:2880\0").unwrap();
        assert_eq!((left.group, left.one_monitor, left.across, left.x, right.x), (1, true, 2, 0, 1));
        assert_eq!(left.whole(), (5120, 2880));
        // Not a tile: one by one, or nonsense.
        assert_eq!(Tile::parse(b"1:1:1:1:0:0:2560:2880"), None);
        assert_eq!(Tile::parse(b"1:1:2:1:2:0:2560:2880"), None);
        assert_eq!(Tile::parse(b"garbage"), None);

        // The left one offers 5120 × 2880: the display is shown through it
        // alone, whichever came first.
        assert_eq!(whole_through(&[("DP-1", right, false), ("DP-2", left, true)]), Some("DP-2"));
        assert_eq!(whole_through(&[("DP-2", left, true), ("DP-1", right, false)]), Some("DP-2"));
        // Both do: the top left one. Neither: each a display.
        assert_eq!(whole_through(&[("DP-1", right, true), ("DP-2", left, true)]), Some("DP-2"));
        assert_eq!(whole_through(&[("DP-1", right, false), ("DP-2", left, false)]), None);
        // Only the right one's plugged in yet, offering nothing whole.
        assert_eq!(whole_through(&[("DP-1", right, false)]), None);
    }

    /// This computer's tiled monitors, as app_server reads them
    /// (`cargo test -p app_server --lib tiles::tests::live -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn live() {
        use smithay::reexports::drm::{Device, control::Device as _};
        use std::os::fd::{AsFd, BorrowedFd};
        struct Card(std::fs::File);
        impl AsFd for Card {
            fn as_fd(&self) -> BorrowedFd<'_> {
                self.0.as_fd()
            }
        }
        impl Device for Card {}
        impl ControlDevice for Card {}
        for path in ["/dev/dri/card0", "/dev/dri/card1"] {
            // (Opening a sleeping GPU wakes it.)
            let sysfs = format!("/sys/class/drm/{}/device/power/runtime_status", &path[9..]);
            if std::fs::read_to_string(sysfs).is_ok_and(|s| s.trim() == "suspended") {
                eprintln!("{path}: asleep, left so");
                continue;
            }
            let Ok(file) = std::fs::File::open(path) else { continue };
            let card = Card(file);
            let Ok(res) = card.resource_handles() else { continue };
            let mut tiles = Vec::new();
            for c in res.connectors() {
                let Ok(info) = card.get_connector(*c, false) else { continue };
                if let Some(t) = of(&card, *c) {
                    let name = format!("{}-{}", info.interface().as_str(), info.interface_id());
                    eprintln!("{path} {name}: {t:?}, offers the whole {:?}: {}", t.whole(), offers_whole(&info, &t));
                    tiles.push((name, t, offers_whole(&info, &t)));
                }
            }
            let named: Vec<(&str, Tile, bool)> = tiles.iter().map(|(n, t, w)| (n.as_str(), *t, *w)).collect();
            eprintln!("{path}: shown whole through {:?}", whole_through(&named));
        }
    }
}
