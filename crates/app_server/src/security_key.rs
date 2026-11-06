//! A security key's typing (a YubiKey's one-time code: a touch types 44
//! characters and Enter through a USB keyboard of its own) is taken here
//! and passed to nobody, so a stray touch types nothing where it lands.
//! When a password window waits for the key, a touch goes to it instead
//! (FIDO2's user presence, `olo_fido`) and types nothing at all. Three
//! touches in a row (each within `ROW` of the last) type the third's code
//! into the focused window as the key would have
//! (`SecurityKeySettings::three_touches_type`, on by default), for a web
//! form Olo knows nothing of. A key is told from a keyboard by the
//! device (`olo_protocol::keyboards::security_key`), never by timing or
//! the text.

use std::time::{Duration, Instant};

/// Touches this close together are in a row.
pub const ROW: Duration = Duration::from_secs(3);
/// A key's characters come milliseconds apart: a pause this long ends a
/// touch's code (a key set to type no Enter after it).
const PAUSE: Duration = Duration::from_millis(400);
const KEY_ENTER: u32 = 28;
const KEY_KPENTER: u32 = 96;

/// The touches seen, and the one being typed.
#[derive(Default, Debug)]
pub struct Taps {
    /// The keys of the touch being typed: (code, pressed).
    keys: Vec<(u32, bool)>,
    last: Option<Instant>,
    /// When each touch in this row ended.
    row: Vec<Instant>,
}

impl Taps {
    /// A key from a security key at `now`: the keys of a touch to type
    /// into the focused window, once a third in a row has ended (when
    /// `typing` allows it); otherwise nothing.
    pub fn feed(&mut self, key: u32, pressed: bool, now: Instant, typing: bool) -> Option<Vec<(u32, bool)>> {
        let mut out = None;
        // A pause: what came before was a touch of its own.
        if self.last.is_some_and(|l| now.duration_since(l) > PAUSE) && !self.keys.is_empty() {
            out = self.touched(self.last.unwrap_or(now), typing);
        }
        self.last = Some(now);
        self.keys.push((key, pressed));
        if matches!(key, KEY_ENTER | KEY_KPENTER) && !pressed {
            out = self.touched(now, typing).or(out);
        }
        out
    }

    /// A touch's code ended at `at`.
    fn touched(&mut self, at: Instant, typing: bool) -> Option<Vec<(u32, bool)>> {
        let keys = std::mem::take(&mut self.keys);
        if self.row.last().is_some_and(|&l| at.duration_since(l) > ROW) {
            self.row.clear();
        }
        self.row.push(at);
        tracing::info!(touches = self.row.len(), "a security key's touch typed nothing");
        if self.row.len() < 3 || !typing {
            return None;
        }
        self.row.clear();
        // Every key let go that it pressed (a code cut short by a pause
        // mustn't leave one down).
        Some(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A touch's code: a few letters and Enter, pressed and let go.
    fn code(taps: &mut Taps, at: Instant, typing: bool) -> Option<Vec<(u32, bool)>> {
        let mut out = None;
        for (i, k) in [46u32, 48, 32, 18, KEY_ENTER].into_iter().enumerate() {
            let t = at + Duration::from_millis(i as u64 * 8);
            for pressed in [true, false] {
                if let Some(keys) = taps.feed(k, pressed, t + Duration::from_millis(if pressed { 0 } else { 4 }), typing) {
                    out = Some(keys);
                }
            }
        }
        out
    }

    #[test]
    fn a_touch_alone_types_nothing() {
        let (mut t, now) = (Taps::default(), Instant::now());
        assert!(code(&mut t, now, true).is_none());
        // Two in a row: still nothing.
        assert!(code(&mut t, now + Duration::from_secs(1), true).is_none());
    }

    #[test]
    fn three_in_a_row_type_the_third() {
        let (mut t, now) = (Taps::default(), Instant::now());
        assert!(code(&mut t, now, true).is_none());
        assert!(code(&mut t, now + Duration::from_millis(1500), true).is_none());
        let typed = code(&mut t, now + Duration::from_millis(3000), true).expect("the third types");
        assert_eq!(typed.len(), 10);
        assert_eq!(typed.last(), Some(&(KEY_ENTER, false)));
        // And the row starts again.
        assert!(code(&mut t, now + Duration::from_millis(4000), true).is_none());
    }

    #[test]
    fn touches_too_far_apart_are_no_row() {
        let (mut t, now) = (Taps::default(), Instant::now());
        assert!(code(&mut t, now, true).is_none());
        assert!(code(&mut t, now + Duration::from_secs(5), true).is_none());
        assert!(code(&mut t, now + Duration::from_secs(10), true).is_none());
        // Off: three in a row type nothing either.
        let mut off = Taps::default();
        for i in 0..3 {
            assert!(code(&mut off, now + Duration::from_secs(i), false).is_none());
        }
    }
}
