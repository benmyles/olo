//! Memory pressure, told as it happens, and what waits it out.
//!
//! The kernel counts the time programs are held because memory is short
//! (pressure stall information, `/proc/pressure/memory`) and wakes
//! whoever asked when that passes a mark (a trigger): [`watch`] sleeps
//! in the kernel until then, so nothing is polled. Root's here (a
//! window that isn't a multiple of two seconds is; Olo's follower,
//! `olo-gpu-tune follow`, is root).
//!
//! What it is for: **an NVIDIA card isn't let suspend while memory is
//! short.** Its runtime suspend allocates memory (the firmware's saved
//! image, in 2 MB pages first) and runs the whole way against one
//! deadline of four seconds; held for memory on the way, it comes to
//! its last check with the deadline spent, calls the suspend failed,
//! and leaves the card unusable until the computer restarts
//! (2026-11-20: HACKING, "A graphics card that stopped answering").
//! So while programs are being held for memory, a card that is awake
//! stays awake ([`Held`]), and sleeps once memory has been calm a
//! while. One that is asleep is left asleep: waking it allocates too.

use std::{
    io::Write as _,
    time::{Duration, Instant},
};

/// Programs held for memory this long within [`WINDOW`] count as memory
/// being short: a twentieth of the time. (The minute the card's suspend
/// failed had them held 39,000 times; an ordinary busy minute of
/// builds, a few hundred to a few thousand, about a hundredth of the
/// time.)
pub const STALL: Duration = Duration::from_millis(100);
pub const WINDOW: Duration = Duration::from_secs(2);
/// How long memory has to have been calm before what was held is let
/// go: long enough that the builds that made the pressure are over,
/// not a pause between two of their steps.
pub const CALM: Duration = Duration::from_secs(120);
/// With less than this share of memory to be had, memory is short
/// whatever the stalls say (the failing suspend's minute had a tenth).
pub const SHORT: f64 = 0.15;

/// Calls `told` each time programs have been held for memory for
/// `stall` or more within `window` (the kernel says so once a window at
/// the most), on a thread of its own that sleeps in the kernel between.
/// It ends when `told` says false. Err where the kernel has no pressure
/// information, or doesn't take the trigger (not root).
pub fn watch(stall: Duration, window: Duration, told: impl Fn() -> bool + Send + 'static) -> std::io::Result<()> {
    use std::os::{fd::AsRawFd as _, unix::fs::OpenOptionsExt as _};
    let mut file = std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NONBLOCK).open("/proc/pressure/memory")?;
    file.write_all(format!("some {} {}\0", stall.as_micros(), window.as_micros()).as_bytes())?;
    std::thread::Builder::new().name("memory-pressure".into()).spawn(move || {
        loop {
            let mut fd = libc::pollfd { fd: file.as_raw_fd(), events: libc::POLLPRI, revents: 0 };
            // SAFETY: one pollfd of ours, for as long as the call.
            let n = unsafe { libc::poll(&mut fd, 1, -1) };
            if n < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            // (POLLERR: the trigger is gone with the file.)
            if fd.revents & libc::POLLERR != 0 || (fd.revents & libc::POLLPRI != 0 && !told()) {
                break;
            }
        }
    })?;
    Ok(())
}

/// The share of memory that can be had without swapping, of
/// `/proc/meminfo`'s text (`MemAvailable` of `MemTotal`).
pub fn available_share(meminfo: &str) -> Option<f64> {
    let kb = |name: &str| meminfo.lines().find_map(|l| l.strip_prefix(name)?.trim().strip_suffix("kB")?.trim().parse::<f64>().ok());
    let (total, available) = (kb("MemTotal:")?, kb("MemAvailable:")?);
    (total > 0.0).then(|| available / total)
}

/// Whether memory is short now, by what can be had ([`SHORT`]).
pub fn short_now() -> bool {
    std::fs::read_to_string("/proc/meminfo").ok().and_then(|m| available_share(&m)).is_some_and(|share| share < SHORT)
}

/// Something held while memory is short, and when it may be let go:
/// [`CALM`] after the last time the kernel said programs were held for
/// memory, and not while there is little to be had.
#[derive(Debug, Default)]
pub struct Held {
    until: Option<Instant>,
}

impl Held {
    /// Programs were held for memory just now: held from now (true if
    /// it wasn't), for [`CALM`] from now.
    pub fn pressed(&mut self, now: Instant) -> bool {
        let was = self.until.is_some();
        self.until = Some(now + CALM);
        !was
    }

    /// When to look again (`due`), while something is held.
    pub fn deadline(&self) -> Option<Instant> {
        self.until
    }

    /// At the deadline: whether what was held is let go now. Not while
    /// memory is `short` still (held for another [`CALM`]), nor before
    /// the deadline.
    pub fn due(&mut self, now: Instant, short: bool) -> bool {
        match self.until {
            Some(until) if now >= until && short => {
                self.until = Some(now + CALM);
                false
            }
            Some(until) if now >= until => {
                self.until = None;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_can_be_had_of_memory() {
        let text = "MemTotal:       65263568 kB\nMemFree:         9075576 kB\nMemAvailable:    5912000 kB\nBuffers:              16 kB\n";
        let share = available_share(text).unwrap();
        assert!((share - 0.0906).abs() < 0.001, "{share}");
        assert!(share < SHORT);
        assert_eq!(available_share("MemTotal: 0 kB\nMemAvailable: 0 kB\n"), None);
        assert_eq!(available_share("nothing"), None);
    }

    /// Held from the first stall, let go a while after the last, and
    /// not while there is still little memory to be had.
    #[test]
    fn held_until_memory_has_been_calm() {
        let t0 = Instant::now();
        let mut held = Held::default();
        assert_eq!(held.deadline(), None);
        assert!(!held.due(t0, false));
        assert!(held.pressed(t0), "the first stall holds");
        assert!(!held.pressed(t0 + Duration::from_secs(30)), "held already");
        assert_eq!(held.deadline(), Some(t0 + Duration::from_secs(30) + CALM));
        // Before the deadline: still held.
        assert!(!held.due(t0 + CALM, false));
        // At it, with little memory to be had: held for as long again.
        let at = t0 + Duration::from_secs(30) + CALM;
        assert!(!held.due(at, true));
        assert_eq!(held.deadline(), Some(at + CALM));
        // Calm, and memory to be had: let go.
        assert!(held.due(at + CALM, false));
        assert_eq!(held.deadline(), None);
        assert!(held.pressed(at + CALM + Duration::from_secs(1)), "and held again by the next stall");
    }
}
