//! The guard on realtime threads, in every Olo program that has threads
//! someone waits on (a window's looper, the display thread, a sound
//! thread): what makes it safe for app_server, or the program itself
//! (`olo_system::realtime`), to make them realtime through rtkit.
//!
//! A realtime thread that spun would take a core for good (Fedora's
//! kernel doesn't throttle realtime threads), and rtkit's hard limit on
//! realtime CPU time without sleeping (`RLIMIT_RTTIME`, 200 ms) ends the
//! whole process; without a handler, SIGXCPU at the soft limit ends it
//! too. So the soft limit here is lower ([`SOFT_LIMIT_US`]), and SIGXCPU
//! puts every thread registered here back to the normal class at once
//! (its nice as it was), before the hard limit: a realtime thread that
//! runs long is slowed, never ended. app_server makes a program's threads
//! realtime only when its limits say it has this guard (`guarded`).
//!
//! Registering doesn't make a thread realtime; it only says which threads
//! may be, and puts them back when they run over. (Plain system calls,
//! no D-Bus: every Olo program has it.)

use std::sync::{
    OnceLock,
    atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering},
};

/// Realtime CPU time without sleeping before the registered threads are
/// put back: no frame, batch of events or buffer of sound takes this long,
/// and it is well under rtkit's hard limit.
pub const SOFT_LIMIT_US: u64 = 50_000;
/// rtkit's most (`RTTimeUSecMax`), the hard limit: rtkit refuses a
/// process whose hard limit is larger.
pub const HARD_LIMIT_US: u64 = 200_000;
const SLOTS: usize = 32;

static TIDS: [AtomicI32; SLOTS] = [const { AtomicI32::new(0) }; SLOTS];
static PRIORITIES: [AtomicU32; SLOTS] = [const { AtomicU32::new(0) }; SLOTS];
/// Put back by the guard, not yet made realtime again.
static PUT_BACK: [AtomicBool; SLOTS] = [const { AtomicBool::new(false) }; SLOTS];
/// A pipe told when the threads were put back (-1: none): the keeper
/// that makes them realtime again, if the program has one.
static NOTIFY: AtomicI32 = AtomicI32::new(-1);
static GUARD: OnceLock<Result<(), String>> = OnceLock::new();

/// The limits, and the handler (once for the process). The limits are
/// set again each time: something else in the process may have set its
/// own since (libpipewire's realtime module sets soft and hard both to
/// 200 ms, where a thread that ran over would be ended, not slowed).
pub fn guard() -> Result<(), String> {
    let limit = libc::rlimit { rlim_cur: SOFT_LIMIT_US, rlim_max: HARD_LIMIT_US };
    // SAFETY: setrlimit reads `limit`; lowering a limit needs no privilege.
    if unsafe { libc::setrlimit(libc::RLIMIT_RTTIME, &limit) } != 0 {
        return Err(format!("its realtime limit: {}", std::io::Error::last_os_error()));
    }
    GUARD
        .get_or_init(|| {
            // SAFETY: a zeroed sigaction, then its handler set; the handler
            // makes only async-signal-safe calls and reads atomics.
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = on_xcpu as *const () as usize;
                action.sa_flags = libc::SA_RESTART;
                libc::sigemptyset(&mut action.sa_mask);
                if libc::sigaction(libc::SIGXCPU, &action, std::ptr::null_mut()) != 0 {
                    return Err(format!("its signal: {}", std::io::Error::last_os_error()));
                }
            }
            Ok(())
        })
        .clone()
}

/// SIGXCPU: a realtime thread used its soft limit without sleeping. The
/// kernel sends it to the process but, in practice, to the thread that ran
/// over (it is the one running): that one goes back to the normal class
/// if registered; if the signal landed on another, every registered one
/// that is realtime does (which one ran over isn't known). Each put back
/// is marked for the keeper, which hears of it.
///
/// The kernel raises the soft limit by a second each time it sends
/// SIGXCPU (so it comes once a second, not every tick): over the hard
/// one, the next thread to run over would meet the hard limit, and
/// SIGKILL. So the limits go back as they were first.
extern "C" fn on_xcpu(_: libc::c_int) {
    let limit = libc::rlimit { rlim_cur: SOFT_LIMIT_US, rlim_max: HARD_LIMIT_US };
    // SAFETY: setrlimit is one system call (prlimit64), async-signal-safe
    // in practice; lowering a limit needs no privilege.
    unsafe { libc::setrlimit(libc::RLIMIT_RTTIME, &limit) };
    // SAFETY: gettid is one system call.
    let me = unsafe { libc::gettid() };
    let mine = TIDS.iter().position(|t| t.load(Ordering::Relaxed) == me);
    for (slot, t) in TIDS.iter().enumerate() {
        let tid = t.load(Ordering::Relaxed);
        if tid > 0 && mine.is_none_or(|m| m == slot) && is_realtime(tid) {
            demote(tid);
            PUT_BACK[slot].store(true, Ordering::Relaxed);
        }
    }
    let fd = NOTIFY.load(Ordering::Relaxed);
    if fd >= 0 {
        // SAFETY: async-signal-safe; the keeper's pipe is non-blocking.
        unsafe { libc::write(fd, b"!".as_ptr().cast(), 1) };
    }
}

/// Whether thread `tid` runs realtime (SCHED_FIFO or SCHED_RR; rtkit's
/// SCHED_RESET_ON_FORK comes back OR'd in, and read as it was every
/// thread rtkit made realtime looked not to be).
pub fn is_realtime(tid: i32) -> bool {
    // SAFETY: sched_getscheduler only reads (one system call).
    realtime_policy(unsafe { libc::sched_getscheduler(tid) })
}

fn realtime_policy(policy: libc::c_int) -> bool {
    policy >= 0 && matches!(policy & !libc::SCHED_RESET_ON_FORK, libc::SCHED_FIFO | libc::SCHED_RR)
}

/// The threads the guard put back since last asked, and their
/// priorities: what the keeper makes realtime again. (A thread made
/// realtime by app_server, a window's looper, too: it was realtime when
/// it ran over.)
pub fn take_put_back() -> Vec<(i32, u32)> {
    TIDS.iter()
        .zip(&PRIORITIES)
        .zip(&PUT_BACK)
        .filter_map(|((t, p), back)| {
            let tid = t.load(Ordering::Relaxed);
            (back.swap(false, Ordering::AcqRel) && tid > 0).then(|| (tid, p.load(Ordering::Relaxed)))
        })
        .collect()
}

/// The calling thread's id.
pub fn tid() -> i32 {
    // SAFETY: gettid has no preconditions.
    unsafe { libc::gettid() }
}

/// Forgets the thread's registration when the thread ends (a thread
/// local's destructor): a window's looper ends with its last window, a
/// player's thread with its sound, and the kernel gives their ids to new
/// threads.
struct Registered(i32);

impl Drop for Registered {
    fn drop(&mut self) {
        forget(self.0);
    }
}

thread_local! {
    static REGISTERED: std::cell::RefCell<Option<Registered>> = const { std::cell::RefCell::new(None) };
}

/// Register the calling thread as one that may be realtime, at
/// `priority` when it is, the guard installed first; forgotten when the
/// thread ends. Its id.
pub fn register_self(priority: u32) -> Result<i32, String> {
    guard()?;
    let tid = tid();
    let _ = REGISTERED.try_with(|r| {
        let mut r = r.borrow_mut();
        if r.is_none() {
            *r = Some(Registered(tid));
        }
    });
    let slot = TIDS
        .iter()
        .position(|t| t.load(Ordering::Relaxed) == tid)
        .or_else(|| TIDS.iter().position(|t| t.compare_exchange(0, tid, Ordering::AcqRel, Ordering::Relaxed).is_ok()))
        .ok_or("too many realtime threads in this program")?;
    PRIORITIES[slot].store(priority, Ordering::Relaxed);
    Ok(tid)
}

/// Forget thread `tid` (it ended, or is never to be realtime again), and
/// put it back to the normal class.
pub fn forget(tid: i32) {
    for t in &TIDS {
        if t.compare_exchange(tid, 0, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            demote(tid);
        }
    }
    for (t, back) in TIDS.iter().zip(&PUT_BACK) {
        if t.load(Ordering::Relaxed) == 0 {
            back.store(false, Ordering::Relaxed);
        }
    }
}

/// Thread `tid` back to the normal class, its nice as it was. With
/// SCHED_RESET_ON_FORK, as rtkit made it: the kernel refuses (EPERM) an
/// unprivileged caller that would clear the flag, so without it a thread
/// rtkit made realtime could never be put back. One system call,
/// async-signal-safe (the guard's handler calls it). Any thread of the
/// caller's user (app_server puts back a window's looper).
pub fn demote(tid: i32) -> bool {
    let param = libc::sched_param { sched_priority: 0 };
    // SAFETY: sched_setscheduler reads `param`; lowering a thread of our
    // own user to the normal class needs no privilege.
    unsafe { libc::sched_setscheduler(tid, libc::SCHED_OTHER | libc::SCHED_RESET_ON_FORK, &param) == 0 }
}

/// The threads registered, and their priorities.
pub fn registered() -> Vec<(i32, u32)> {
    TIDS.iter().zip(&PRIORITIES).filter_map(|(t, p)| Some((t.load(Ordering::Relaxed), p.load(Ordering::Relaxed))).filter(|(t, _)| *t > 0)).collect()
}

/// The pipe the handler tells when it put the threads back.
pub fn set_notify(fd: i32) {
    NOTIFY.store(fd, Ordering::Release);
}

/// Whether process `pid`'s limits are this guard's (its soft realtime
/// limit set, under its hard one: SIGXCPU is handled, not fatal). Read
/// from /proc: for app_server, before it makes another program's threads
/// realtime.
pub fn guarded(pid: u32) -> bool {
    let Ok(limits) = std::fs::read_to_string(format!("/proc/{pid}/limits")) else { return false };
    limits.lines().find(|l| l.starts_with("Max realtime timeout")).is_some_and(|l| {
        let mut n = l["Max realtime timeout".len()..].split_whitespace();
        let (soft, hard) = (n.next().and_then(|v| v.parse::<u64>().ok()), n.next().and_then(|v| v.parse::<u64>().ok()));
        matches!((soft, hard), (Some(s), Some(h)) if s <= SOFT_LIMIT_US && s < h && h <= HARD_LIMIT_US)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guarded_program_says_so_in_its_limits() {
        // (This test process: guarded once registered.)
        let tid = register_self(5).unwrap();
        assert!(registered().contains(&(tid, 5)));
        assert!(guarded(std::process::id()));
        forget(tid);
        assert!(!registered().iter().any(|(t, _)| *t == tid));
        assert!(SOFT_LIMIT_US * 2 <= HARD_LIMIT_US);
        assert!(!guarded(1), "init isn't ours, or unguarded");
    }

    #[test]
    fn a_thread_that_ends_is_forgotten() {
        let tid = std::thread::spawn(|| register_self(3).unwrap()).join().unwrap();
        assert!(!registered().iter().any(|(t, _)| *t == tid));
    }

    #[test]
    fn rtkits_reset_on_fork_is_still_realtime() {
        assert!(realtime_policy(libc::SCHED_RR | libc::SCHED_RESET_ON_FORK));
        assert!(realtime_policy(libc::SCHED_FIFO));
        assert!(!realtime_policy(libc::SCHED_OTHER | libc::SCHED_RESET_ON_FORK));
        assert!(!realtime_policy(libc::SCHED_IDLE));
        assert!(!realtime_policy(-1));
    }
}
