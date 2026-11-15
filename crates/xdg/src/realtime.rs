//! Realtime threads: what the person is waiting on runs before anything
//! else wanting the processor, under any scheduler and any load (Ben,
//! 2026-11-15: "nothing should ever be able to impact the responsiveness
//! of olo"; "any audio i/o and other stuff like that should be realtime
//! too").
//!
//! A nice isn't enough: the CPU is shared between cgroups before nice
//! counts (each app is a scope of its own), and a scheduler loaded over
//! the kernel's (scx_lavd) gave nice little weight. Under other programs'
//! builds and a GPU job, app_server's loop at nice −10 waited on a run
//! queue 100–300 ms of every second and the display stalled for up to
//! 2.5 s; made realtime, it waited 0 ms in a minute of the same load. The
//! realtime class is above every normal and sched_ext one.
//!
//! SCHED_RR (with SCHED_RESET_ON_FORK: the threads a realtime one starts
//! are normal), set by the program itself ([`make_realtime`]), where the
//! account lets it (RLIMIT_RTPRIO, from the login's limits: Olo Doctor's
//! Allow Realtime gives the person's account [`MOST_PRIORITY`]). Each
//! thread registered with the guard in `olo_app::realtime` first: a
//! realtime thread is never put back for running long, and ends its
//! program only stuck, 30 s without once sleeping, with a report. Never
//! through rtkit: it gives realtime only to a process whose hard limit on
//! realtime time without sleeping is 200 ms at most, ended at that, and
//! app_server was (2026-11-15, inside a GPU driver's wait). Without the
//! account's leave, threads keep their nice (rtkit still gives that, with
//! no such limit).

pub use olo_app::realtime::{HARD_LIMIT_US, STUCK_LIMIT_US, demote, forget, guarded, marked, registered, tid};

/// The highest realtime priority Olo's threads and PipeWire's in its
/// programs use, and what Olo Doctor's Allow Realtime lets the account
/// set (Fedora gives its `pipewire` group the same).
pub const MOST_PRIORITY: u32 = 70;

/// Whether thread `tid` runs realtime (SCHED_FIFO or SCHED_RR;
/// SCHED_RESET_ON_FORK comes back OR'd in).
pub fn is_realtime(tid: u32) -> bool {
    olo_app::realtime::is_realtime(tid as i32)
}

/// The realtime priority this process may set (its RLIMIT_RTPRIO: the
/// login's limits; 0 without Olo Doctor's Allow Realtime).
pub fn allowed_priority() -> u32 {
    let mut l = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: getrlimit writes `l`.
    if unsafe { libc::getrlimit(libc::RLIMIT_RTPRIO, &mut l) } != 0 {
        return 0;
    }
    l.rlim_cur.min(99) as u32
}

/// Thread `tid` of `pid` made realtime: SCHED_RR at `priority` (at most
/// what the account allows), SCHED_RESET_ON_FORK. The process must be the
/// caller's user's, and the account let it (RLIMIT_RTPRIO): the error
/// says so where it isn't.
pub fn make_realtime(pid: u32, tid: u32, priority: u32) -> Result<(), String> {
    let _ = pid;
    let param = libc::sched_param { sched_priority: priority.clamp(1, 99) as i32 };
    // SAFETY: sched_setscheduler reads `param`, for a thread of our user.
    if unsafe { libc::sched_setscheduler(tid as i32, libc::SCHED_RR | libc::SCHED_RESET_ON_FORK, &param) } == 0 {
        return Ok(());
    }
    let e = std::io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::EPERM) {
        return Err(format!("the account may not make threads realtime at {priority} (Olo Doctor's Allow Realtime lets it)"));
    }
    Err(e.to_string())
}

/// A thread moving sound between a program and the sound server (a
/// player's, a recorder's, a call's): above app_server's (a gap in sound
/// is heard where a late frame is barely seen; such a thread does little
/// each time it wakes), below PipeWire's own data loops.
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

/// Make the calling thread realtime at `priority`, guarded (stuck 30 s,
/// its program ended with a report; never put back for running long).
/// The error says why it wasn't (the account doesn't allow it, or
/// something in the process lowered the guard's limit): it stays as it
/// was.
pub fn promote_self(priority: u32) -> Result<(), String> {
    let tid = olo_app::realtime::register_self(priority)?;
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
    use std::time::Duration;

    use super::*;

    fn spin(ms: u64) {
        let end = std::time::Instant::now() + Duration::from_millis(ms);
        let mut x = 0u64;
        while std::time::Instant::now() < end {
            x = std::hint::black_box(x.wrapping_add(1));
        }
    }

    /// A realtime thread busy for longer than any frame stays realtime and
    /// its program runs on: busy isn't stuck (rtkit's rule ended a
    /// program at 200 ms; the guard put a thread back at 50).
    #[test]
    #[ignore = "needs the account's leave for realtime (Olo Doctor's Allow Realtime)"]
    fn a_busy_thread_stays_realtime() {
        promote_self(5).expect("the account's leave");
        let me = tid() as u32;
        assert!(is_realtime(me));
        spin(1_500);
        assert!(is_realtime(me), "still realtime after 1.5 s of work");
        demote_self();
        assert!(!is_realtime(me));
    }

    /// Without the account's leave, a thread isn't made realtime, and
    /// it's said why; it carries on as it was.
    #[test]
    fn without_leave_it_says_so() {
        if allowed_priority() >= 5 {
            return;
        }
        let e = std::thread::spawn(|| promote_self(5)).join().unwrap().unwrap_err();
        assert!(e.contains("Allow Realtime"), "{e}");
    }
}
