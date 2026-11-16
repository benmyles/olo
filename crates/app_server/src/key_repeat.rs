//! Key repeat, made by app_server for the clients that take it.
//!
//! A key held down repeats. Until wl_keyboard version 10 each client did
//! that itself, from a timer started at the press it was sent: and a
//! client's timer can't know of a release its compositor hasn't sent
//! yet. When app_server's loop was held 316 ms just after a press (in
//! the graphics driver, 2026-11-16), the release reached the client past
//! its 300 ms repeat delay, and the letter was typed twice (Ben:
//! "sunbursst"). The hold's cause is gone (HACKING, "Windows' buffers
//! stay in memory"); this takes away what any hold could do.
//!
//! From version 10 the compositor may do the repeating: it tells such a
//! keyboard a repeat rate of 0, and sends key events in the "repeated"
//! state (`KeyboardHandle::make_repeats`, Olo's in smithay). app_server
//! does, for every client whose keyboard is of that version (Olo's kit;
//! the others repeat by their own timers as before).
//!
//! What repeats is the last key that repeats sent to a client and still
//! down there (`KeyboardHandle::repeatable`, kept where every key
//! reaches a client, whatever grab sent it), and a repeat is sent only
//! at the end of a turn of the loop in which its timer came due
//! (`insert_idle`): after whatever input had waited with it. A release
//! that waited behind a hold is therefore always handled first, and
//! ends the repeating before a repeat is sent, however late the timer
//! fired; and a key still down after a hold repeats on from then, with
//! no burst for the time missed.

use std::time::{Duration, Instant};

use smithay::{
    input::keyboard::Keycode,
    reexports::calloop::{
        RegistrationToken,
        timer::{TimeoutAction, Timer},
    },
    utils::SERIAL_COUNTER,
};

use crate::state::{AnvilState, Backend};

/// The key repeating, if one is: its code and its press's time (the
/// keyboard's clock, milliseconds), which tell one press of a key from
/// its next.
type Key = (Keycode, u32);

#[derive(Debug, Default)]
pub struct KeyRepeat {
    armed: Option<(Key, RegistrationToken)>,
    /// When the next repeat is due.
    due: Option<Instant>,
    /// How many repeats were sent (the tests', and `{"type":"timings"}`'s).
    pub sent: u64,
}

/// When the repeat after the one due at `due` is due, `every` on; from
/// now, where that is past already (the loop was held): no burst for the
/// time missed.
pub fn next_due(due: Instant, now: Instant, every: Duration) -> Instant {
    let next = due + every;
    if next > now { next } else { now + every }
}

impl<BackendData: Backend> AnvilState<BackendData> {
    /// The key that would repeat changed (smithay's
    /// `SeatHandler::repeatable_key_changed`: asked with the keyboard's
    /// state locked, so nothing of the keyboard but `repeatable` here):
    /// its timer started for its first repeat, the repeat delay after
    /// its press was sent; the one before stopped.
    pub fn key_repeat_changed(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else { return };
        let now = keyboard.repeatable();
        if now.map(|(code, time, _)| (code, time)) == self.key_repeat.armed.as_ref().map(|(key, _)| *key) {
            return;
        }
        if let Some((_, timer)) = self.key_repeat.armed.take() {
            self.handle.remove(timer);
        }
        let (rate, delay) = (self.settings.keyboard.repeat_rate_hz, self.settings.keyboard.repeat_delay_ms);
        let Some((code, time, at)) = now.filter(|_| rate > 0) else { return };
        let key = (code, time);
        let every = Duration::from_secs(1) / rate as u32;
        let due = at + Duration::from_millis(delay.max(0) as u64);
        self.key_repeat.due = Some(due);
        let timer = self.handle.insert_source(Timer::from_deadline(due), move |_, _, state| {
            // Sent at the end of this turn of the loop, after whatever
            // input waited with it: a release among it ends the
            // repeating first.
            state.handle.insert_idle(move |state| state.key_repeat_send(key));
            let now = Instant::now();
            let next = next_due(state.key_repeat.due.unwrap_or(now), now, every);
            state.key_repeat.due = Some(next);
            TimeoutAction::ToInstant(next)
        });
        match timer {
            Ok(timer) => self.key_repeat.armed = Some((key, timer)),
            Err(e) => tracing::warn!("no key repeat: its timer couldn't be made: {e}"),
        }
    }

    /// One repeat of `key`, if it is still the key repeating; its timer
    /// stopped where there's no one left to repeat it to.
    fn key_repeat_send(&mut self, key: Key) {
        if self.key_repeat.armed.as_ref().map(|(armed, _)| *armed) != Some(key) {
            return;
        }
        let Some(keyboard) = self.seat.get_keyboard() else { return };
        let sent = keyboard
            .repeatable()
            .filter(|(code, time, _)| (*code, *time) == key)
            // (Its time on the keyboard's clock: its press's, and what has passed since.)
            .is_some_and(|(_, time, at)| keyboard.send_repeat(SERIAL_COUNTER.next_serial(), time.wrapping_add(at.elapsed().as_millis() as u32)));
        if sent {
            self.key_repeat.sent += 1;
        } else if let Some((_, timer)) = self.key_repeat.armed.take() {
            self.handle.remove(timer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Repeats keep their beat (the next one `every` after the last was
    /// due, however late its turn came), and after a hold go on from now
    /// with no burst for the time missed.
    #[test]
    fn repeats_keep_their_beat_and_never_burst() {
        let (start, every) = (Instant::now(), Duration::from_millis(28));
        // On time, or a little late: the beat kept.
        assert_eq!(next_due(start, start, every), start + every);
        assert_eq!(next_due(start, start + Duration::from_millis(9), every), start + every);
        // Held past several: one from now, not one for each missed.
        let after_hold = start + Duration::from_millis(500);
        assert_eq!(next_due(start, after_hold, every), after_hold + every);
        // (Exactly a beat late counts as missed too.)
        assert_eq!(next_due(start, start + every, every), start + every + every);
    }
}
