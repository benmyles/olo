//! The embedded controller flooding the kernel with events: seen, and
//! measured.
//!
//! A laptop's embedded controller (EC) tells the kernel of each thing
//! that happens to it (a key of the top row, the lid, the charger) with
//! an interrupt, and the kernel queues a handler for each, without limit
//! (drivers/acpi/ec.c). A controller that reports the same thing over
//! and over fills that queue faster than its handlers run: a ThinkPad
//! P1 Gen 9's reported its charger coming and going 23 times a second
//! for two hours (2026-11-17), Lenovo's handler for that takes 0.6 s,
//! and 195,778 handlers stood in line with the brightness keys' and the
//! USB-C ports' behind them, so none of those worked, while the battery
//! ran down on its charger (docs/AGENT-NOTES.md, "The embedded
//! controller's event storm").
//!
//! From outside the kernel it shows as two things at once, neither
//! alone: the ACPI interrupt arriving many times a second for seconds on
//! end ([`interrupts`]), and kernel workers of the EC's query queue
//! standing blocked ([`waiting`]). [`Watch`] is told when the power
//! supply changes (which costs nothing while it doesn't) and says when a
//! run of changes is worth measuring; [`judge`] says whether two
//! readings a few seconds apart are a storm.

use std::{
    collections::VecDeque,
    path::Path,
    time::{Duration, Instant},
};

/// ACPI interrupts a second, held between two readings, that go with a
/// storm (the P1's: 46, two for each coming and going of the charger).
/// Not enough alone: a key of the top row held down makes interrupts
/// too, and its handlers keep up.
pub const RATE: f32 = 8.0;
/// Workers of the EC's query queue blocked at once, that only a queue
/// not keeping up has (the P1's storm: hundreds; a key, the lid or the
/// charger in ordinary use: one, for a moment).
pub const WAITING: u32 = 6;

/// What a moment looks like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// ACPI interrupts since the system started.
    pub interrupts: u64,
    /// Kernel workers running an EC query's handler, blocked.
    pub waiting: u32,
    pub at: Instant,
}

/// A storm, as measured between two readings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Storm {
    /// ACPI interrupts a second.
    pub events_per_second: f32,
    /// Handlers standing blocked.
    pub waiting: u32,
}

/// ACPI interrupts since the system started: the `acpi` line of
/// `/proc/interrupts` (`text`), every processor's count.
pub fn interrupts(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.split_whitespace().last() == Some("acpi"))?;
    Some(line.split_whitespace().skip(1).map_while(|n| n.parse::<u64>().ok()).sum())
}

/// Whether a line of `/proc/PID/stat` is a kernel worker running one of
/// the EC's queries and blocked there (state D: Lenovo's adapter handler
/// sleeps under a firmware mutex): "12 (kworker/3:2+kec_query) D 2 …".
fn waiting_worker(stat: &str) -> bool {
    let Some((name, rest)) = stat.split_once('(').and_then(|(_, r)| r.rsplit_once(')')) else { return false };
    name.starts_with("kworker/") && name.contains("+kec_query") && rest.split_whitespace().next() == Some("D")
}

/// How many kernel workers are running an EC query's handler and blocked
/// (`proc`: /proc). A read of each process's `stat`: a few milliseconds,
/// so only when there is reason to look.
pub fn waiting(proc: &Path) -> u32 {
    let Ok(dir) = std::fs::read_dir(proc) else { return 0 };
    dir.flatten()
        .filter(|e| e.file_name().as_encoded_bytes().first().is_some_and(u8::is_ascii_digit))
        .filter(|e| std::fs::read_to_string(e.path().join("stat")).is_ok_and(|s| waiting_worker(&s)))
        .count() as u32
}

/// This moment, read from /proc.
pub fn read() -> Option<Reading> {
    let interrupts = interrupts(&std::fs::read_to_string("/proc/interrupts").ok()?)?;
    Some(Reading { interrupts, waiting: waiting(Path::new("/proc")), at: Instant::now() })
}

/// Whether `before` and `after` (a few seconds apart) are a storm: the
/// interrupt held at [`RATE`] or more between them, and handlers blocked
/// at both ends ([`WAITING`] or more at the second).
pub fn judge(before: &Reading, after: &Reading) -> Option<Storm> {
    let secs = after.at.saturating_duration_since(before.at).as_secs_f32();
    if secs < 1.0 {
        return None;
    }
    let rate = after.interrupts.saturating_sub(before.interrupts) as f32 / secs;
    (rate >= RATE && before.waiting > 0 && after.waiting >= WAITING).then_some(Storm { events_per_second: rate, waiting: after.waiting })
}

/// Follows how often the power supply changes, to say when it's worth
/// measuring: told of each change (UPower's signals, as they come), it
/// costs nothing while there are none.
#[derive(Debug, Default)]
pub struct Watch {
    changes: VecDeque<Instant>,
    /// Looked and found nothing: not again before then.
    quiet_until: Option<Instant>,
}

impl Watch {
    /// This many changes of the supply within [`Watch::WINDOW`] are worth
    /// a look (a charger plugged in makes three or four; a battery being
    /// used, one every half minute or so).
    pub const CHANGES: usize = 10;
    pub const WINDOW: Duration = Duration::from_secs(15);
    /// After a look that found nothing, how long before another.
    pub const REST: Duration = Duration::from_secs(120);

    /// The supply changed at `now`; whether to measure.
    pub fn changed(&mut self, now: Instant) -> bool {
        self.changes.push_back(now);
        while self.changes.len() > Self::CHANGES {
            self.changes.pop_front();
        }
        let many = self.changes.len() == Self::CHANGES && self.changes.front().is_some_and(|t| now.saturating_duration_since(*t) <= Self::WINDOW);
        many && self.quiet_until.is_none_or(|t| now >= t)
    }

    /// A look found nothing (a charger being fiddled with): rest.
    pub fn found_nothing(&mut self, now: Instant) {
        self.quiet_until = Some(now + Self::REST);
        self.changes.clear();
    }
}

/// Between two readings that judge a storm, and, in one, between looks
/// at whether it is over.
const LOOK: Duration = Duration::from_secs(4);
const AGAIN: Duration = Duration::from_secs(20);
/// Looks in a row that find it gone before it is over.
const CALM: u32 = 3;

/// Calls `changed` with a storm when one begins (as measured), and with
/// `None` when it's over, on threads of its own. Asleep until UPower
/// says the supply changed (nothing to follow without it: `false`), and
/// measuring only when it changes over and over ([`Watch`]); in a storm,
/// a look every [`AGAIN`].
pub fn follow(changed: impl Fn(Option<Storm>) + Send + 'static) -> bool {
    let Some(signals) = crate::bus::system().ok().and_then(|c| {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender("org.freedesktop.UPower")
            .ok()?
            .interface("org.freedesktop.DBus.Properties")
            .ok()?
            .member("PropertiesChanged")
            .ok()?
            .build();
        zbus::blocking::MessageIterator::for_match_rule(rule, c, Some(64)).ok()
    }) else {
        return false;
    };
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let _ = std::thread::Builder::new().name("ec-storm-signals".into()).spawn(move || {
        for _ in signals.flatten() {
            if tx.send(()).is_err() {
                return;
            }
        }
    });
    let _ = std::thread::Builder::new().name("ec-storm".into()).spawn(move || {
        use std::sync::mpsc::RecvTimeoutError;
        let mut watch = Watch::default();
        let mut storm = false;
        let mut calm = 0;
        loop {
            // Until the supply changes; in a storm, until it's time to
            // look again (its changes are no news then).
            let look = if storm {
                std::thread::sleep(AGAIN);
                true
            } else {
                match rx.recv() {
                    Ok(()) => watch.changed(Instant::now()),
                    Err(_) => return,
                }
            };
            // (What came meanwhile: each a change, when not in a storm.)
            let mut look = look;
            loop {
                match rx.recv_timeout(Duration::ZERO) {
                    Ok(()) => look |= !storm && watch.changed(Instant::now()),
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            if !look {
                continue;
            }
            let Some(before) = read() else { continue };
            std::thread::sleep(LOOK);
            let Some(after) = read() else { continue };
            match (storm, judge(&before, &after)) {
                (false, Some(found)) => {
                    (storm, calm) = (true, 0);
                    changed(Some(found));
                }
                (false, None) => watch.found_nothing(Instant::now()),
                (true, Some(_)) => calm = 0,
                (true, None) => {
                    calm += 1;
                    if calm >= CALM {
                        storm = false;
                        watch = Watch::default();
                        changed(None);
                    }
                }
            }
        }
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_acpi_interrupts_count() {
        let text = "            CPU0       CPU1       CPU2\n   1:          9          0          0  IR-IO-APIC    1-edge      i8042\n   9:     245210         12          3  IR-IO-APIC    9-fasteoi   acpi\n  14:          0          0          0  INTEL-GPIO   32  ELAN0001:00\n";
        assert_eq!(interrupts(text), Some(245_225));
        assert_eq!(interrupts("   1:   9   IR-IO-APIC 1-edge i8042\n"), None, "a computer with no such line");
    }

    #[test]
    fn workers_of_the_ecs_queue_blocked() {
        assert!(waiting_worker("4242 (kworker/3:2+kec_query) D 2 0 0 0 -1 69238880 0 0"));
        assert!(!waiting_worker("4242 (kworker/3:2+kec_query) I 2 0 0"), "idle between two");
        assert!(!waiting_worker("4242 (kworker/3:2-kec_query) D 2 0 0"), "its last work, not this one");
        assert!(!waiting_worker("4242 (kworker/u64:1+events_unbound) D 2 0 0"));
        assert!(!waiting_worker("77 (kec_query) D 1 0 0"), "a program of that name");
        assert!(!waiting_worker("nonsense"));
        // Counted from a folder laid out as /proc is.
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-ec-storm-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (pid, stat) in [("10", "10 (kworker/1:1+kec_query) D 2"), ("11", "11 (kworker/2:1+kec_query) D 2"), ("12", "12 (bash) S 1"), ("self", "13 (kworker/9:9+kec_query) D 2")] {
            std::fs::create_dir_all(dir.join(pid)).unwrap();
            std::fs::write(dir.join(pid).join("stat"), stat).unwrap();
        }
        assert_eq!(waiting(&dir), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A storm is both at once, held: the interrupt many times a second
    /// and handlers standing blocked. Either alone isn't one.
    #[test]
    fn a_storm_is_the_interrupt_held_and_handlers_blocked() {
        let t = Instant::now();
        let at = |secs: u64, interrupts: u64, waiting: u32| Reading { interrupts, waiting, at: t + Duration::from_secs(secs) };
        // The P1's: 46 interrupts a second, every worker blocked.
        let storm = judge(&at(0, 1000, 352), &at(5, 1230, 352)).expect("a storm");
        assert_eq!((storm.events_per_second, storm.waiting), (46.0, 352));
        // A top-row key held down: the interrupt, and its handlers keeping up.
        assert_eq!(judge(&at(0, 1000, 0), &at(5, 1050, 1)), None);
        // Blocked workers with no interrupts (a slow firmware call): no.
        assert_eq!(judge(&at(0, 1000, 8), &at(5, 1004, 8)), None);
        // Just begun: blocked now, but not a moment ago. Looked at again, it is.
        assert_eq!(judge(&at(0, 1000, 0), &at(5, 1230, 40)), None);
        assert!(judge(&at(5, 1230, 40), &at(10, 1460, 90)).is_some());
        // Readings too close to tell a rate from.
        assert_eq!(judge(&at(0, 1000, 352), &Reading { interrupts: 1040, waiting: 352, at: t + Duration::from_millis(500) }), None);
    }

    /// The supply changing over and over is what makes it worth a look;
    /// a charger plugged in isn't, nor a battery in use; and after a look
    /// that found nothing it rests.
    #[test]
    fn worth_a_look() {
        let t = Instant::now();
        let ms = |n: u64| t + Duration::from_millis(n);
        let mut w = Watch::default();
        // A charger plugged in: a handful of changes.
        assert!((0..4).all(|i| !w.changed(ms(i * 300))));
        // A battery in use: one every half minute.
        let mut w = Watch::default();
        assert!((0..40).all(|i| !w.changed(ms(i * 30_000))));
        // The controller repeating itself: ten in a few seconds.
        let mut w = Watch::default();
        let looks: Vec<bool> = (0..12).map(|i| w.changed(ms(i * 200))).collect();
        assert_eq!(looks.iter().position(|l| *l), Some(Watch::CHANGES - 1));
        assert!(looks[Watch::CHANGES..].iter().all(|l| *l), "and still, while it goes on");
        // Nothing found: not again for a while, however it flaps; then again.
        w.found_nothing(ms(3000));
        assert!((0..30).all(|i| !w.changed(ms(3000 + i * 100))));
        let later = 3000 + Watch::REST.as_millis() as u64;
        assert!((0..12).any(|i| w.changed(ms(later + i * 100))));
    }
}
