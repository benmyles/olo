//! Apple's Magic Keyboards (2021 and 2024: with Touch ID, a keypad, or
//! neither): their top row and their Globe (Fn) key, which the kernel's
//! hid-apple gives as `magic_keyboard_2021_and_2024_fn_keys` has it. With
//! its default (fnmode 1) the row types its printed features (brightness,
//! Mission Control, Spotlight, dictation, Do Not Disturb, media, volume)
//! and F1–F12 with Fn held; `KeyboardSettings::fkeys_first` turns that
//! round here, per keyboard, with no root (hid-apple's parameter is the
//! whole machine's, and root's). The row's Do Not Disturb key comes as
//! `KEY_SLEEP`, which logind would take to suspend the computer: Olo
//! holds that from logind (power.rs) and makes it Do Not Disturb.

/// Apple's vendor ids: USB, and Bluetooth's.
const APPLE: [u32; 2] = [0x05ac, 0x004c];
/// Their product ids (hid-ids.h): 2021's, then 2024's (plain, with Touch
/// ID, with a keypad).
const MAGIC: [u32; 6] = [0x029c, 0x029a, 0x029f, 0x0320, 0x0321, 0x0322];

/// The row: each F-key and what it's printed with.
pub const ROW: [(u32, u32); 12] = [
    (59, 224), // F1: brightness down
    (60, 225), // F2: brightness up
    (61, 120), // F3: Mission Control (KEY_SCALE)
    (62, 217), // F4: Spotlight (KEY_SEARCH)
    (63, 248), // F5: dictation, as the microphone's mute
    (64, 142), // F6: Do Not Disturb (KEY_SLEEP)
    (65, 165), // F7: previous
    (66, 164), // F8: play or pause
    (67, 163), // F9: next
    (68, 113), // F10: mute
    (87, 114), // F11: volume down
    (88, 115), // F12: volume up
];

pub const KEY_FN: u32 = 464;
pub const KEY_SLEEP: u32 = 142;

/// Whether a device (its USB product and vendor ids, as smithay gives
/// them) is a 2021 or 2024 Magic Keyboard.
pub fn is_magic_keyboard((product, vendor): (u32, u32)) -> bool {
    APPLE.contains(&vendor) && MAGIC.contains(&product)
}

/// `key` from the row with F-keys first: an F-key is its feature, a
/// feature its F-key (Fn held gives the other, as the kernel sends it).
pub fn fkeys_first(key: u32) -> u32 {
    ROW.iter()
        .find_map(|&(f, m)| {
            if key == m {
                Some(f)
            } else if key == f {
                Some(m)
            } else {
                None
            }
        })
        .unwrap_or(key)
}

/// The Globe key pressed and let go with nothing between.
#[derive(Debug, Default)]
pub struct Globe {
    down: Option<(std::time::Instant, bool)>,
}

impl Globe {
    /// A key from the keyboard: true when the Globe key was pressed and
    /// let go alone, quickly (not held for Fn+key, nor long).
    pub fn feed(&mut self, key: u32, pressed: bool, now: std::time::Instant) -> bool {
        match (key == KEY_FN, pressed) {
            (true, true) => {
                self.down = Some((now, true));
                false
            }
            (true, false) => self.down.take().is_some_and(|(at, alone)| alone && now.duration_since(at) < std::time::Duration::from_millis(800)),
            (false, true) => {
                if let Some((_, alone)) = &mut self.down {
                    *alone = false;
                }
                false
            }
            (false, false) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn magic_keyboards_by_their_ids() {
        // Ben's: USB 05ac:029f (smithay gives product, vendor).
        assert!(is_magic_keyboard((0x029f, 0x05ac)));
        assert!(is_magic_keyboard((0x0321, 0x004c)));
        assert!(!is_magic_keyboard((0x0267, 0x05ac)));
        assert!(!is_magic_keyboard((0x029f, 0x1050)));
    }

    #[test]
    fn the_row_turned_round() {
        assert_eq!(fkeys_first(224), 59);
        assert_eq!(fkeys_first(59), 224);
        assert_eq!(fkeys_first(KEY_SLEEP), 64);
        assert_eq!(fkeys_first(88), 115);
        assert_eq!(fkeys_first(30), 30);
        for (f, m) in ROW {
            assert_eq!(fkeys_first(fkeys_first(f)), f);
            assert_eq!(fkeys_first(fkeys_first(m)), m);
        }
    }

    #[test]
    fn globe_alone_only() {
        let (mut g, t) = (Globe::default(), Instant::now());
        assert!(!g.feed(KEY_FN, true, t));
        assert!(g.feed(KEY_FN, false, t + Duration::from_millis(120)));
        // Fn held for F1: not alone.
        assert!(!g.feed(KEY_FN, true, t));
        assert!(!g.feed(59, true, t + Duration::from_millis(50)));
        assert!(!g.feed(59, false, t + Duration::from_millis(90)));
        assert!(!g.feed(KEY_FN, false, t + Duration::from_millis(150)));
        // Held a long time: not a press.
        assert!(!g.feed(KEY_FN, true, t));
        assert!(!g.feed(KEY_FN, false, t + Duration::from_secs(2)));
    }
}
