//! Memory pressure, told as it happens.
//!
//! The kernel counts the time programs are held because memory is short
//! (pressure stall information, `/proc/pressure/memory`) and wakes
//! whoever asked when that passes a mark (a trigger): [`watch`] waits in
//! the kernel until then, on a thread of its own. Root's here (a window
//! that isn't a multiple of two seconds is; one of two seconds an
//! ordinary program may set too).
//!
//! What the kernel does for it, measured in a guest on Linux 7.2.15: its
//! own monitor thread (`psimon`) runs about five times a second while
//! programs are being held and sleeps otherwise (3 wakes in four calm
//! seconds, 54 under pressure); it tells at most once a window, and
//! what wasn't read is one telling.
//!
//! Nothing of Olo's uses it yet but the measuring tool (`kbench
//! pressure`). It was made to keep an NVIDIA card from suspending while
//! memory is short (2026-11-20), by writing "on" to the card's
//! `power/control` at each telling: withdrawn before it ever ran, on
//! review. That write is the kernel's `pm_runtime_forbid`, which
//! resumes the device then and there: a card that had gone to sleep
//! between Olo's look and its write was woken, under the very pressure
//! it was to be spared, and one in the middle of suspending was waited
//! for without limit (reproduced on this kernel with a USB hub in a
//! guest). User space has no way to keep a device from suspending
//! without that; the place for it is the driver's own suspend
//! (docs/TODO.md, "The NVIDIA card can never again be left stuck").

use std::{io::Write as _, time::Duration};

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
            // (An error, a hang-up or a file that is no more: the trigger
            // is gone, and waiting on would spin.)
            if fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 || (fd.revents & libc::POLLPRI != 0 && !told()) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_can_be_had_of_memory() {
        let text = "MemTotal:       65263568 kB\nMemFree:         9075576 kB\nMemAvailable:    5912000 kB\nBuffers:              16 kB\n";
        let share = available_share(text).unwrap();
        assert!((share - 0.0906).abs() < 0.001, "{share}");
        assert_eq!(available_share("MemTotal: 0 kB\nMemAvailable: 0 kB\n"), None);
        assert_eq!(available_share("nothing"), None);
    }
}
