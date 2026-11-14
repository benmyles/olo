//! Notices an app_server that has stopped answering (stuck in a loop or a
//! deadlock, the whole desktop frozen) and ends it, so it is started again
//! like any crash: apps reconnect, windows come back, and a locked screen
//! comes back locked. It is asked to dump core first (SIGABRT), so
//! `coredumpctl` has the stack that hung.
//!
//! Silence around a sleep isn't a hang: the graphics driver holds every
//! submission from when a sleep is being prepared until it has woken
//! (xe's `xe_pm_block_on_suspend`), app_server's frame with it, and that
//! time before the sleep and after it once added up to `HANG`: a healthy
//! app_server was ended 20 ms after the lid opened (Ben, 2026-11-13), and
//! every app mid-frame with it waited for good for its images. So silence
//! counts from the last wake (`asleep`: CLOCK_BOOTTIME less
//! CLOCK_MONOTONIC grows by each sleep), and an app_server the kernel
//! holds for a sleep (`held_for_sleep`, its main thread's wchan) isn't
//! ended.
//!
//! And a shell program that didn't come back: once app_server has
//! started (or restarted), Deskbar and the desktop's Tracker should be
//! on it with their surfaces; one still running without any
//! `SHELL_GRACE` later is stuck (that day's Deskbar, its drawing thread
//! waiting for an image the ended app_server would never give back:
//! "my deskbar is gone why isn't supervisor bringing it back?"), and is
//! ended so it's started again like any crash ([`layer_clients`],
//! `Session::check_shell`).

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

/// How often app_server is asked whether it's there.
const EVERY: Duration = Duration::from_secs(2);
/// Silence this long means it's stuck (it answers within a frame).
const HANG: Duration = Duration::from_secs(10);
/// How long a core dump may take before it's killed outright.
const DUMP: Duration = Duration::from_secs(10);

pub struct Watchdog {
    epoch: Instant,
    /// When app_server last answered (ms after `epoch`).
    answered: Arc<AtomicU64>,
    /// The app_server watched, and since when (or since the computer
    /// last woke, or the kernel last held it for a sleep).
    pid: i32,
    since: Instant,
    /// Asked to dump core, at.
    aborted: Option<Instant>,
    /// How long the computer had slept, as last seen (`asleep`).
    asleep: Duration,
    /// Said that the kernel holds app_server for a sleep (said once).
    said_held: bool,
}

/// How long the computer has spent asleep since it started:
/// CLOCK_BOOTTIME counts that time and CLOCK_MONOTONIC (`Instant`)
/// doesn't, so this grows at each sleep.
fn asleep() -> Duration {
    let read = |clock| {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: `t` is a valid timespec to write into.
        unsafe { libc::clock_gettime(clock, &mut t) };
        Duration::new(t.tv_sec.max(0) as u64, t.tv_nsec.clamp(0, 999_999_999) as u32)
    };
    read(libc::CLOCK_BOOTTIME).saturating_sub(read(libc::CLOCK_MONOTONIC))
}

/// Whether process `pid`'s main thread is waiting in the kernel on the
/// computer's going to sleep or waking (or frozen for it), not on
/// anything of its own: a graphics driver holds every submission from
/// when a sleep is being prepared until it has woken (xe's
/// `xe_pm_block_on_suspend`), app_server's frame with it.
fn held_for_sleep(pid: i32) -> Option<String> {
    let wchan = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/wchan")).ok()?;
    let held = ["suspend", "pm_block", "refrigerator", "freezer"].iter().any(|w| wchan.contains(w));
    held.then_some(wchan)
}

/// How long after app_server starts, and after Deskbar does, Deskbar must
/// have its surfaces (it has them within a second or two).
pub const SHELL_GRACE: Duration = Duration::from_secs(20);

/// Each layer surface's process and namespace, as app_server says
/// (`LayerClients`; None: it didn't answer).
pub fn layer_clients(path: &PathBuf) -> Option<Vec<(i32, String)>> {
    let mut s = UnixStream::connect(path).ok()?;
    let _ = s.set_read_timeout(Some(EVERY));
    let _ = s.set_write_timeout(Some(EVERY));
    s.write_all(b"{\"type\":\"layer_clients\"}\n").ok()?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).ok()?;
    let reply: serde_json::Value = serde_json::from_str(&line).ok()?;
    let clients = reply.get("clients")?.as_array()?;
    Some(clients.iter().filter_map(|c| Some((c.get(0)?.as_i64()? as i32, c.get(1)?.as_str()?.to_owned()))).collect())
}

/// How long process `pid` has been running.
pub fn age(pid: i32) -> Option<Duration> {
    let started: f64 = crate::upgrade::start_time(pid)? as f64;
    // SAFETY: sysconf has no preconditions.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
    let up: f64 = std::fs::read_to_string("/proc/uptime").ok()?.split_whitespace().next()?.parse().ok()?;
    Some(Duration::from_secs_f64((up - started / hz).max(0.0)))
}

/// Ask over the IPC socket; true if it answered.
fn ping(path: &PathBuf) -> bool {
    let Ok(mut s) = UnixStream::connect(path) else { return false };
    let _ = s.set_read_timeout(Some(EVERY));
    let _ = s.set_write_timeout(Some(EVERY));
    if s.write_all(b"{\"type\":\"ping\"}\n").is_err() {
        return false;
    }
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).is_ok_and(|_| line.contains("pong"))
}

impl Watchdog {
    pub fn start(path: PathBuf) -> Watchdog {
        let epoch = Instant::now();
        let answered = Arc::new(AtomicU64::new(0));
        let a = answered.clone();
        std::thread::Builder::new()
            .name("watchdog".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(EVERY);
                    if ping(&path) {
                        a.store(epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
                    }
                }
            })
            .ok();
        Watchdog { epoch, answered, pid: 0, since: epoch, aborted: None, asleep: asleep(), said_held: false }
    }

    /// Check on app_server `pid` (0: none running); ends it if it's stuck.
    pub fn check(&mut self, pid: i32) {
        let now = Instant::now();
        if pid != self.pid {
            self.pid = pid;
            self.since = now;
            self.aborted = None;
        }
        // Woken: silence before the sleep isn't app_server's, nor is the
        // time it takes to wake; it's given `HANG` from now.
        let slept = asleep();
        if slept > self.asleep + Duration::from_millis(500) {
            if pid > 0 && self.aborted.is_none() {
                crate::log(format!("the computer slept {} s: app_server's silence counts from now", (slept - self.asleep).as_secs()));
            }
            self.since = now;
        }
        self.asleep = self.asleep.max(slept);
        if pid <= 0 {
            return;
        }
        if let Some(at) = self.aborted {
            if now.duration_since(at) > DUMP {
                crate::log(format!("app_server {pid} is still there after SIGABRT; killing it"));
                // SAFETY: signalling our own child.
                unsafe { libc::kill(pid, libc::SIGKILL) };
                self.aborted = Some(now);
            }
            return;
        }
        let answered = self.epoch + Duration::from_millis(self.answered.load(Ordering::Relaxed));
        let quiet = now.duration_since(answered.max(self.since));
        if quiet > HANG
            && let Some(wchan) = held_for_sleep(pid)
        {
            // The kernel's, for a sleep: not stuck; ending it wouldn't help
            // (another would be held the same) and lost every app's frame.
            if !self.said_held {
                crate::log(format!("app_server {pid} hasn't answered for {} s, held by the kernel for a sleep ({}): waiting", quiet.as_secs(), wchan.trim()));
                self.said_held = true;
            }
            self.since = now;
            return;
        }
        if answered > self.since {
            self.said_held = false;
        }
        if quiet > HANG {
            crate::log(format!("app_server {pid} has not answered for {}s; ending it (it will be restarted)", quiet.as_secs()));
            // SAFETY: as above.
            unsafe { libc::kill(pid, libc::SIGABRT) };
            self.aborted = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_asleep_only_grows() {
        let a = asleep();
        let b = asleep();
        // (Read a moment apart, the two clocks can differ by a little.)
        assert!(b + Duration::from_millis(5) >= a, "{a:?} then {b:?}");
    }

    #[test]
    fn a_running_process_isnt_held_for_a_sleep() {
        assert_eq!(held_for_sleep(std::process::id() as i32), None);
        assert_eq!(held_for_sleep(i32::MAX), None);
    }

    #[test]
    fn a_process_has_an_age() {
        let age = age(std::process::id() as i32).unwrap();
        assert!(age < Duration::from_secs(600), "{age:?}");
    }
}
