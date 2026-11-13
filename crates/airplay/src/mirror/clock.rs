//! The time a mirrored screen's pictures and sound are stamped with, and
//! how the receiver learns it.
//!
//! The receiver plays each picture and each piece of sound at its stamp
//! plus the lead it was told (`latencyMs`, `latencyMax`), on its own
//! clock: so it has to know how this computer's clock stands to its own.
//! There are two ways, and the receiver says which it takes:
//!
//! - *NTP*: it asks this computer the time now and then (the timing
//!   port, `play::timing`'s answers), and works the offset out itself.
//!   The answers are this clock with NTP's epoch (1900) added; a
//!   picture's stamp is the same clock without it (as Apple's senders
//!   do, and UxPlay reads: the receiver takes the epoch off one and not
//!   the other), sound's sync with it.
//! - *PTP*: today's Apple TVs keep a network clock of their own, and
//!   stamps are on that one's timeline. No PTP is spoken here: each
//!   answer on the control connection says when the receiver had the
//!   request, in milliseconds of that clock (`X-Apple-RequestReceived
//!   Timestamp`, with `X-Apple-ProcessingTime` after it), and this
//!   computer's clock is laid against that ([`Timeline`]), again at each
//!   `/feedback` (never back: a late answer looks older than it is).
//!
//! The clock itself is the computer's monotonic one (what `Instant`
//! counts), so a capture's moment converts exactly.

use std::time::{Duration, Instant};

/// NTP's epoch (1900) from Unix's, in seconds.
pub(crate) const EPOCH: u64 = 2_208_988_800;

/// The monotonic clock now, from boot (CLOCK_MONOTONIC: what `Instant`
/// counts).
pub(crate) fn monotonic() -> Duration {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime into a timespec of ours.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

/// This computer's clock: an `Instant`'s moment as time from boot.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Clock {
    at: Instant,
    since_boot: Duration,
}

impl Clock {
    pub fn new() -> Clock {
        Clock { at: Instant::now(), since_boot: monotonic() }
    }

    /// `t` on this clock.
    pub fn of(&self, t: Instant) -> Duration {
        if t >= self.at { self.since_boot + (t - self.at) } else { self.since_boot.saturating_sub(self.at - t) }
    }

    pub fn now(&self) -> Duration {
        self.of(Instant::now())
    }
}

/// NTP's fixed point: whole seconds in the high half, the fraction in the
/// low.
pub(crate) fn compact(d: Duration) -> u64 {
    (d.as_secs() << 32) | ((u64::from(d.subsec_nanos()) << 32) / 1_000_000_000)
}

/// The same, with NTP's epoch: what the timing port answers.
pub(crate) fn ntp(d: Duration) -> u64 {
    compact(d) + (EPOCH << 32)
}

/// Nanoseconds of a fixed-point time (PTP's sync wants them).
pub(crate) fn nanoseconds(t: u64) -> u64 {
    (t >> 32) * 1_000_000_000 + (((t & 0xffff_ffff) * 1_000_000_000) >> 32)
}

/// A fixed-point time moved by `by` (back too), None past its ends.
pub(crate) fn moved(t: u64, by: Duration, back: bool) -> Option<u64> {
    if back { t.checked_sub(compact(by)) } else { t.checked_add(compact(by)) }
}

/// The receiver's network clock (PTP), laid against this computer's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Timeline {
    /// Its clock's identity, as SETUP's answer named it (each stamp says
    /// which timeline it's on).
    pub id: u64,
    /// When here, and what its clock said then (fixed point).
    local: Option<(Instant, u64)>,
}

/// What an answer says of the receiver's clock when it had the request:
/// milliseconds when it came, plus how long it took over it.
pub(crate) fn stamped(received: Option<&str>, processing: Option<&str>) -> Option<u64> {
    let (received, processing) = (received?.trim().parse::<u64>().ok()?, processing.unwrap_or("0").trim().parse::<u64>().ok()?);
    Some(compact(Duration::from_millis(received.checked_add(processing)?)))
}

impl Timeline {
    pub fn new(id: u64) -> Timeline {
        Timeline { id, local: None }
    }

    /// Its clock read `theirs` at `at` (when its answer came): laid
    /// against ours, never moving back from where the last reading put
    /// it.
    pub fn anchor(&mut self, at: Instant, theirs: u64) {
        let theirs = match self.local {
            Some((then, was)) if at >= then => theirs.max(was.saturating_add(compact(at - then))),
            _ => theirs,
        };
        self.local = Some((at, theirs));
    }

    /// `t` here, plus `lead`, on its clock.
    pub fn at(&self, t: Instant, lead: Duration) -> Option<u64> {
        let (then, theirs) = self.local?;
        let base = if t >= then { moved(theirs, t - then, false)? } else { moved(theirs, then - t, true)? };
        moved(base, lead, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_point_and_epoch() {
        assert_eq!(compact(Duration::from_millis(1500)), 1 << 32 | 1 << 31);
        assert_eq!(ntp(Duration::ZERO) >> 32, EPOCH);
        assert!(nanoseconds(compact(Duration::from_nanos(2_250_000_123))).abs_diff(2_250_000_123) <= 1);
        assert_eq!(moved(5 << 32, Duration::from_secs(6), true), None);
        assert_eq!(moved(5 << 32, Duration::from_secs(2), true), Some(3 << 32));
    }

    #[test]
    fn an_instant_on_the_clock() {
        let clock = Clock::new();
        let now = Instant::now();
        let later = now + Duration::from_millis(250);
        assert_eq!(clock.of(later) - clock.of(now), Duration::from_millis(250));
        // The monotonic clock and Instant agree (to within the call).
        assert!(clock.now().abs_diff(monotonic()) < Duration::from_millis(50));
    }

    #[test]
    fn the_receivers_clock_laid_against_ours() {
        assert_eq!(stamped(Some("1000"), Some("12")), Some(compact(Duration::from_millis(1012))));
        assert_eq!(stamped(Some("x"), Some("1")), None);
        assert_eq!(stamped(None, Some("1")), None);
        let mut tl = Timeline::new(0xabc);
        let t0 = Instant::now();
        assert_eq!(tl.at(t0, Duration::ZERO), None);
        tl.anchor(t0, 10 << 32);
        assert_eq!(tl.at(t0 + Duration::from_secs(2), Duration::from_millis(500)), Some(12 << 32 | 1 << 31));
        assert_eq!(tl.at(t0 - Duration::from_secs(1), Duration::ZERO), Some(9 << 32));
        // A later answer that seems older (it was on its way a while)
        // doesn't move it back; a newer one moves it on.
        tl.anchor(t0 + Duration::from_secs(1), 10 << 32);
        assert_eq!(tl.at(t0 + Duration::from_secs(1), Duration::ZERO), Some(11 << 32));
        tl.anchor(t0 + Duration::from_secs(1), 20 << 32);
        assert_eq!(tl.at(t0 + Duration::from_secs(1), Duration::ZERO), Some(20 << 32));
    }
}
