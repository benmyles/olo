//! Times said relative to now that follow the clock (Ben, 2026-11-15:
//! "when i have multiple active agent screens and i click on one of the
//! other tabs ... it often then updates the last activity timer ... it
//! seems to only update when i click on it. i've noticed the same thing
//! happen in other apps too ... please root cause and fix universally").
//!
//! An app says an age in its own words (`say`: "now", "5 min", "Updated
//! 3 minutes ago"); [`ago`] says it, works out when those words next
//! change (the first whole second, minute, hour or day of the age at
//! which `say` says something else), and tells the view saying it
//! (`olo_interface::holds_until`), which says what it shows again then:
//! one timer a view, for the soonest, and none while nothing said would
//! change. No app keeps a timer for its times, and none goes stale until
//! something else draws the window. Say it in `body`.
//!
//! ```ignore
//! row.value(olo_kit::ago::ago_ms(last_used, olo_kit::ago::short))
//! ```

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Now, in milliseconds since 1970.
fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The age of a time `at_ms` (milliseconds since 1970) in `say`'s words,
/// the view saying it said again as they change.
pub fn ago_ms(at_ms: u64, say: impl Fn(Duration) -> String) -> String {
    age(Duration::from_millis(now_ms().saturating_sub(at_ms)), say)
}

/// [`ago_ms`] for a time in seconds since 1970.
pub fn ago_secs(at: i64, say: impl Fn(Duration) -> String) -> String {
    ago_ms((at.max(0) as u64).saturating_mul(1000), say)
}

/// [`ago_ms`] for a `SystemTime`.
pub fn ago(at: SystemTime, say: impl Fn(Duration) -> String) -> String {
    age(SystemTime::now().duration_since(at).unwrap_or_default(), say)
}

/// An age now `age`, in `say`'s words, the view saying it said again as
/// they change.
pub fn age(age: Duration, say: impl Fn(Duration) -> String) -> String {
    let words = say(age);
    olo_interface::holds_until(Instant::now() + until_change(age, &say, &words));
    words
}

/// How long until `say` says something other than `words` of an age now
/// `age`: at the first whole second or minute of it where it does; where
/// that's further on, an hour before the whole hour where it does (then
/// looked at again to the minute: words that change with the calendar,
/// not the age); a day on at most. Words, once they've changed, don't come
/// back as the age grows, so each unit's boundaries are searched by
/// halves: a handful of `say`s.
pub fn until_change(age: Duration, say: &impl Fn(Duration) -> String, words: &str) -> Duration {
    const SECOND: u64 = 1000;
    const MINUTE: u64 = 60 * SECOND;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    let now = age.as_millis() as u64;
    let differs = |at: u64| say(Duration::from_millis(at)) != words;
    let mut from = now;
    // Each unit's boundaries up to the next of the unit above it.
    for (unit, upto) in [(SECOND, MINUTE), (MINUTE, HOUR), (HOUR, DAY)] {
        let first = (from / unit + 1) * unit;
        let last = (from / upto + 1) * upto;
        let n = (last - first) / unit + 1;
        if differs(first + (n - 1) * unit) {
            let (mut lo, mut hi) = (0, n - 1);
            while lo < hi {
                let mid = (lo + hi) / 2;
                if differs(first + mid * unit) {
                    hi = mid;
                } else {
                    lo = mid + 1;
                }
            }
            // (Among the hours, the hour before it: a change that isn't on
            // a whole hour of the age, "yesterday" at midnight, is then
            // found to the minute.)
            let at = if unit == HOUR { first + lo * unit - unit } else { first + lo * unit };
            return Duration::from_millis(at.max(now + 1) - now);
        }
        from = last;
    }
    // No change within the day: look again at its end (`from`, the next
    // whole day of the age).
    Duration::from_millis(from - now)
}

/// A sidebar's short age: "now", "5 min", "3 h", "2 d".
pub fn short(age: Duration) -> String {
    let s = age.as_secs();
    match s {
        0..60 => crate::words::tr("now").into(),
        60..3600 => crate::tr!("{} min", s / 60),
        3600..86_400 => crate::tr!("{} h", s / 3600),
        _ => crate::tr!("{} d", s / 86_400),
    }
}

/// An age in a sentence: "just now", "12 s ago", "5 min ago", "3 h ago",
/// "2 days ago".
pub fn long(age: Duration) -> String {
    let s = age.as_secs();
    match s {
        0..10 => crate::words::tr("just now").into(),
        10..60 => crate::tr!("{} s ago", s),
        60..3600 => crate::tr!("{} min ago", s / 60),
        3600..86_400 => crate::tr!("{} h ago", s / 3600),
        86_400..172_800 => crate::words::tr("a day ago").into(),
        _ => crate::words::count(s / 86_400, "1 day ago", "{n} days ago"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    /// The words' next change, for each kind of age, as each phrasing
    /// has them.
    #[test]
    fn the_next_change_is_when_the_words_change() {
        let next = |age: Duration, say: fn(Duration) -> String| until_change(age, &say, &say(age));
        // "now" holds for the first minute: 45 s in, 15 s on.
        assert_eq!(next(at(45), short), at(15));
        // "5 min" until 6: from 5 min 20 s, 40 s on.
        assert_eq!(next(at(320), short), at(40));
        // "3 h" until 4: looked at again an hour before the hour it
        // changes, then to the minute.
        assert_eq!(next(at(3 * 3600 + 100), short), at(3600 - 100));
        assert_eq!(
            next(at(3 * 3600 + 30), |a| if a.as_secs() < 5 * 3600 + 600 { "early".into() } else { "late".into() }),
            at(3600 - 30 + 3600),
            "a change at 5 h 10 min: woken at 5 h, then to the minute"
        );
        assert_eq!(next(at(5 * 3600), |a| if a.as_secs() < 5 * 3600 + 600 { "early".into() } else { "late".into() }), at(600));
        // "2 d" until 3: an hour before, then to the minute.
        assert_eq!(next(at(2 * 86_400 + 5), short), at(86_400 - 5 - 3600));
        // "just now" until 10 s; then each second; then each minute.
        assert_eq!(next(at(3), long), at(7));
        assert_eq!(next(at(12), long), at(1));
        assert_eq!(next(Duration::from_millis(12_400), long), Duration::from_millis(600));
        assert_eq!(next(at(61), long), at(59));
        // "a day ago" for the whole second day: looked at again an hour
        // before it ends.
        assert_eq!(next(at(86_400 + 3600), long), at(86_400 - 3600 - 3600));
        // Words that never change: a day on, at most.
        assert_eq!(until_change(at(30), &|_| "always".to_owned(), "always"), at(86_400 - 30));
    }

    #[test]
    fn phrasings() {
        assert_eq!((short(at(5)), short(at(125)), short(at(7300)), short(at(200_000))), ("now".into(), "2 min".into(), "2 h".into(), "2 d".into()));
        assert_eq!((long(at(5)), long(at(30)), long(at(125)), long(at(90_000))), ("just now".into(), "30 s ago".into(), "2 min ago".into(), "a day ago".into()));
    }
}
