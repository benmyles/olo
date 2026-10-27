//! Notices an app_server that has stopped answering (stuck in a loop or a
//! deadlock, the whole desktop frozen) and ends it, so it is started again
//! like any crash: apps reconnect, windows come back, and a locked screen
//! comes back locked. It is asked to dump core first (SIGABRT), so
//! `coredumpctl` has the stack that hung.

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
    /// The app_server watched, and since when.
    pid: i32,
    since: Instant,
    /// Asked to dump core, at.
    aborted: Option<Instant>,
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
        Watchdog { epoch, answered, pid: 0, since: epoch, aborted: None }
    }

    /// Check on app_server `pid` (0: none running); ends it if it's stuck.
    pub fn check(&mut self, pid: i32) {
        let now = Instant::now();
        if pid != self.pid {
            self.pid = pid;
            self.since = now;
            self.aborted = None;
        }
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
        if quiet > HANG {
            crate::log(format!("app_server {pid} has not answered for {}s; ending it (it will be restarted)", quiet.as_secs()));
            // SAFETY: as above.
            unsafe { libc::kill(pid, libc::SIGABRT) };
            self.aborted = Some(now);
        }
    }
}
