//! Realtime threads, guarded: what the person is waiting on runs before
//! anything else wanting the processor, under any scheduler and any load
//! (Ben, 2026-11-15: "nothing should ever be able to impact the
//! responsiveness of olo"; "any audio i/o and other stuff like that
//! should be realtime too").
//!
//! A nice isn't enough: the CPU is shared between cgroups before nice
//! counts (each app is a scope of its own), and a scheduler loaded over
//! the kernel's (scx_lavd) gave nice little weight. Under other programs'
//! builds and a GPU job, app_server's loop at nice −10 waited on a run
//! queue 100–300 ms of every second and the display stalled for up to
//! 2.5 s; made realtime, it waited 0 ms in a minute of the same load. The
//! realtime class is above every normal and sched_ext one.
//!
//! Through rtkit ([`make_realtime`]), SCHED_RR, each thread
//! registered with the guard in `olo_app::realtime` first (a soft limit of
//! 50 ms of realtime CPU without sleeping, where SIGXCPU puts the threads
//! back to their nice, under rtkit's hard 200 ms that would end the
//! process). Here a keeper thread, asleep on a pipe the guard's handler
//! writes to, makes them realtime again [`AGAIN_AFTER`] later (and logs
//! it). Nothing runs meanwhile: the keeper is blocked in a read.
//!
//! Threads a realtime one starts are normal (rtkit sets
//! SCHED_RESET_ON_FORK). [`demote_self`] puts one back for good (a window
//! that lost the keyboard).

use std::time::Duration;

pub use olo_app::realtime::{SOFT_LIMIT_US, demote, forget, guarded, registered, tid};

const RTKIT: &str = "org.freedesktop.RealtimeKit1";
const PATH: &str = "/org/freedesktop/RealtimeKit1";

/// Whether thread `tid` runs realtime (SCHED_FIFO or SCHED_RR; rtkit's
/// SCHED_RESET_ON_FORK comes back OR'd in).
pub fn is_realtime(tid: u32) -> bool {
    olo_app::realtime::is_realtime(tid as i32)
}

/// Thread `tid` of `pid` made realtime through rtkit (SCHED_RR at
/// `priority`, at most its `MaxRealtimePriority`). rtkit refuses a process
/// whose hard RLIMIT_RTTIME isn't set at most its 200 ms: the guard sets
/// it. The process must be the caller's user's.
pub fn make_realtime(pid: u32, tid: u32, priority: u32) -> Result<(), String> {
    // One connection, and rtkit's most asked once (each focus change asks
    // again for a window's threads).
    static BUS: std::sync::OnceLock<(zbus::blocking::Connection, u32)> = std::sync::OnceLock::new();
    let (conn, most) = match BUS.get() {
        Some(b) => b,
        None => {
            let conn = zbus::blocking::Connection::system().map_err(|e| e.to_string())?;
            let most = conn
                .call_method(Some(RTKIT), PATH, Some("org.freedesktop.DBus.Properties"), "Get", &(RTKIT, "MaxRealtimePriority"))
                .ok()
                .and_then(|r| r.body().deserialize::<zbus::zvariant::OwnedValue>().ok())
                .and_then(|v| i32::try_from(v).ok())
                .unwrap_or(20)
                .max(1) as u32;
            BUS.get_or_init(|| (conn, most))
        }
    };
    conn.call_method(Some(RTKIT), PATH, Some(RTKIT), "MakeThreadRealtimeWithPID", &(pid as u64, tid as u64, priority.min(*most))).map_err(|e| e.to_string())?;
    Ok(())
}

/// A thread moving sound between a program and the sound server (a
/// player's, a recorder's, a call's): above app_server's (a gap in sound
/// is heard where a late frame is barely seen; such a thread does little
/// each time it wakes), below PipeWire's own data loops (20).
pub const SOUND_PRIORITY: u32 = 15;

/// The calling thread made realtime at `priority` if realtime is
/// [`wanted`] here; why not, logged (it carries on as it was). Forgotten,
/// and so never made realtime again, when the thread ends. For a thread
/// someone listens to or waits on: `what` names it in the log.
pub fn promote_if_wanted(priority: u32, what: &str) {
    if !wanted() {
        return;
    }
    match promote_self(priority) {
        Ok(()) => tracing::debug!(priority, "{what}: realtime"),
        Err(e) => tracing::info!("{what} stays at its nice, not realtime: {e}"),
    }
}

/// How long a thread put back stays in the normal class before the
/// keeper makes it realtime again.
pub const AGAIN_AFTER: Duration = Duration::from_secs(2);

static KEEPER: std::sync::OnceLock<()> = std::sync::OnceLock::new();

/// The keeper, once: asleep on a pipe the guard's handler writes to;
/// woken, it makes the threads the guard put back realtime again after
/// [`AGAIN_AFTER`].
fn keeper() {
    KEEPER.get_or_init(|| {
        let mut fds = [0; 2];
        // SAFETY: pipe2 writes two descriptors into `fds`.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            return;
        }
        // (The handler's end non-blocking: a full pipe already says it.)
        // SAFETY: fcntl on a descriptor of ours.
        unsafe { libc::fcntl(fds[1], libc::F_SETFL, libc::O_NONBLOCK) };
        olo_app::realtime::set_notify(fds[1]);
        let read = fds[0];
        let _ = std::thread::Builder::new().name("realtime-keeper".into()).spawn(move || {
            let mut buf = [0u8; 64];
            loop {
                // SAFETY: reads at most 64 bytes into `buf` from our pipe.
                let n = unsafe { libc::read(read, buf.as_mut_ptr().cast(), buf.len()) };
                if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                if n <= 0 {
                    return;
                }
                tracing::warn!(ms = SOFT_LIMIT_US / 1000, "a realtime thread ran this long without sleeping: back to its nice, realtime again in {} s", AGAIN_AFTER.as_secs());
                std::thread::sleep(AGAIN_AFTER);
                for (tid, priority) in olo_app::realtime::take_put_back() {
                    if !is_realtime(tid as u32)
                        && let Err(e) = make_realtime(std::process::id(), tid as u32, priority)
                    {
                        tracing::warn!(tid, "couldn't be made realtime again: {e}");
                    }
                }
            }
        });
    });
}

/// Make the calling thread realtime at `priority` (rtkit, at most its
/// `MaxRealtimePriority`), guarded, made realtime again by the keeper
/// should the guard put it back. Its nice stays as set before, for when
/// it is put back. The error says why it wasn't (rtkit missing or
/// refusing): it stays as it was.
pub fn promote_self(priority: u32) -> Result<(), String> {
    let tid = olo_app::realtime::register_self(priority)?;
    keeper();
    if is_realtime(tid as u32) {
        return Ok(());
    }
    make_realtime(std::process::id(), tid as u32, priority)
}

/// Put the calling thread back to the normal class for good, and forget
/// it.
pub fn demote_self() {
    forget(tid());
}

/// Whether realtime threads are wanted in this process: not on an
/// agent's screen (background work beside the person's desktop), and not
/// turned off (`OLO_REALTIME=0`; `=1` asks for them anywhere, to measure).
pub fn wanted() -> bool {
    match std::env::var("OLO_REALTIME").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        // (Not on an agent's screen: background work beside the person's
        // desktop.)
        _ => std::env::var_os("OLO_SCREEN").is_none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spin(ms: u64) {
        let end = std::time::Instant::now() + Duration::from_millis(ms);
        let mut x = 0u64;
        while std::time::Instant::now() < end {
            x = std::hint::black_box(x.wrapping_add(1));
        }
    }

    /// A realtime thread that never sleeps is put back at the soft limit,
    /// made realtime again, and put back again: never ended by the hard
    /// one (the kernel raises the soft limit a second each time; without
    /// the guard setting it back, the second time was SIGKILL).
    #[test]
    #[ignore = "asks the system's rtkit"]
    fn a_thread_that_runs_over_is_slowed_never_ended() {
        promote_self(5).expect("rtkit");
        let me = tid() as u32;
        assert!(is_realtime(me));
        spin(300);
        assert!(!is_realtime(me), "put back after 50 ms");
        // The keeper, two seconds on (asleep meanwhile, as this thread is).
        std::thread::sleep(AGAIN_AFTER + Duration::from_millis(300));
        assert!(is_realtime(me), "realtime again");
        spin(300);
        assert!(!is_realtime(me), "put back again, not ended");
        demote_self();
    }
}
