//! The guard on realtime threads, in every Olo program that has threads
//! someone waits on (a window's looper, the display thread, a sound
//! thread): what makes it safe for app_server, or the program itself
//! (`olo_system::realtime`), to make them realtime.
//!
//! A realtime thread runs before anything else wanting the processor,
//! for as long as it has work: busy isn't stuck, and it is never put back
//! to the normal class for running long, nor ended for it (Ben,
//! 2026-11-15: "there should never be short kill rule and we should
//! NEVER drop priority down"; app_server was ended that day by rtkit's
//! rule, at 200 ms without sleeping, inside a GPU driver's wait). Only a
//! thread truly stuck, [`STUCK_LIMIT_US`] (30 s) of processor without
//! once sleeping, ends its program: RLIMIT_RTTIME's soft limit, whose
//! SIGXCPU the guard turns into an abort (a crash report, as any crash
//! has), and the hard limit a second on, the kernel's SIGKILL should the
//! thread be inside the kernel all that while. So threads are made
//! realtime only where the account lets a program do it itself
//! (RLIMIT_RTPRIO; Olo Doctor's Allow Realtime), never through rtkit,
//! which gives it only to a process whose hard limit is 200 ms at most.
//! app_server makes a program's threads realtime only when its limits say
//! it has this guard (`guarded`).
//!
//! Registering doesn't make a thread realtime; it says which threads may
//! be. (Plain system calls, no D-Bus: every Olo program has it.)

use std::sync::{
    OnceLock,
    atomic::{AtomicI32, AtomicU32, Ordering},
};

/// Processor time a realtime thread may take without once sleeping
/// before it's taken for stuck and its program ended (with a report).
pub const STUCK_LIMIT_US: u64 = 30_000_000;
/// The kernel's own end, a second after: for a thread stuck inside the
/// kernel, where the guard's signal can't be taken.
pub const HARD_LIMIT_US: u64 = 31_000_000;
const SLOTS: usize = 32;

static TIDS: [AtomicI32; SLOTS] = [const { AtomicI32::new(0) }; SLOTS];
static PRIORITIES: [AtomicU32; SLOTS] = [const { AtomicU32::new(0) }; SLOTS];
static GUARD: OnceLock<Result<(), String>> = OnceLock::new();

/// The limits, and the handler (once for the process). The limits are
/// set again each time: something else in the process may have lowered
/// them since (libpipewire's realtime module, where it went through
/// rtkit, sets both to 200 ms). A hard limit lowered below ours can't be
/// raised again: then no thread here is made realtime, rather than one
/// ended at that.
pub fn guard() -> Result<(), String> {
    let limit = libc::rlimit { rlim_cur: STUCK_LIMIT_US, rlim_max: HARD_LIMIT_US };
    // SAFETY: setrlimit reads `limit`; lowering a limit needs no privilege
    // (raising the hard one would, and fails).
    if unsafe { libc::setrlimit(libc::RLIMIT_RTTIME, &limit) } != 0 {
        let mut now = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: getrlimit writes `now`.
        unsafe { libc::getrlimit(libc::RLIMIT_RTTIME, &mut now) };
        return Err(format!("its realtime time limit is {} ms (set by something else in it): a thread made realtime would be ended at that", now.rlim_max / 1000));
    }
    GUARD
        .get_or_init(|| {
            // SAFETY: a zeroed sigaction, then its handler set; the handler
            // makes only async-signal-safe calls.
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

/// SIGXCPU: a realtime thread has run [`STUCK_LIMIT_US`] without once
/// sleeping. Stuck: the program is ended with a report (SIGABRT, as a
/// crash), said first on stderr (the journal).
extern "C" fn on_xcpu(_: libc::c_int) {
    const SAID: &[u8] = b"olo: a realtime thread ran 30 s without once sleeping: stuck, ending the program\n";
    // SAFETY: write and abort are async-signal-safe.
    unsafe {
        libc::write(2, SAID.as_ptr().cast(), SAID.len());
        libc::abort();
    }
}

/// Thread `tid` marked as registered, where app_server can see it:
/// SCHED_RESET_ON_FORK set on its normal class (its nice kept), which the
/// threads it starts don't have (the kernel clears it in them). A thread
/// a window's looper starts takes the looper's name (`w>…`), and
/// app_server makes realtime only the named threads that are marked
/// ([`marked`]), not those (Agent Screens' export, encoding video,
/// 2026-11-15). Realtime already (made so with the flag), left as it is.
fn mark(tid: i32) {
    // SAFETY: sched_getscheduler only reads.
    let policy = unsafe { libc::sched_getscheduler(tid) };
    if policy == libc::SCHED_OTHER {
        let param = libc::sched_param { sched_priority: 0 };
        // SAFETY: sched_setscheduler reads `param`; setting the flag on a
        // thread of our own (never clearing it) needs no privilege, and the
        // normal class keeps the thread's nice.
        unsafe { libc::sched_setscheduler(tid, libc::SCHED_OTHER | libc::SCHED_RESET_ON_FORK, &param) };
    }
}

/// Whether thread `tid` is marked as one that may be made realtime (a
/// registered thread's flag, see `mark`; one made realtime has it too). Read by app_server before it makes another program's threads
/// realtime.
pub fn marked(tid: i32) -> bool {
    // SAFETY: sched_getscheduler only reads.
    let policy = unsafe { libc::sched_getscheduler(tid) };
    policy >= 0 && policy & libc::SCHED_RESET_ON_FORK != 0
}

/// Whether thread `tid` runs realtime (SCHED_FIFO or SCHED_RR; the
/// SCHED_RESET_ON_FORK it's made with comes back OR'd in).
pub fn is_realtime(tid: i32) -> bool {
    // SAFETY: sched_getscheduler only reads (one system call).
    realtime_policy(unsafe { libc::sched_getscheduler(tid) })
}

fn realtime_policy(policy: libc::c_int) -> bool {
    policy >= 0 && matches!(policy & !libc::SCHED_RESET_ON_FORK, libc::SCHED_FIFO | libc::SCHED_RR)
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
    mark(tid);
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
}

/// Thread `tid` back to the normal class, its nice as it was (a thread
/// ending, or a window whose program no longer has the keyboard: never
/// for running long). With SCHED_RESET_ON_FORK, the mark it keeps: the
/// kernel refuses (EPERM) an unprivileged caller that would clear it. Any
/// thread of the caller's user (app_server puts back a window's looper).
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

/// Keep this program's memory in RAM, for good: every page it has mapped
/// and whatever it maps later, its code and its data, each locked from
/// when it's first touched (`mlockall` with MCL_ONFAULT: nothing is read
/// in for it, and room only reserved takes none). What's locked is never
/// moved to swap, nor dropped to be read back from disk: a program
/// someone waits on mustn't wait for its own pages (Ben, 2026-11-16:
/// "keep all those core "responsiveness" processes out of swap always";
/// app_server had taken 7,669 major page faults in 2 h 20 min).
///
/// Only where the account lets a program lock all it has
/// (RLIMIT_MEMLOCK unlimited: Olo Doctor's Keep the Core in Memory). With
/// a limit, whatever the program mapped past it would fail to map, and
/// it would end for want of memory that was there. The error says why
/// it isn't locked. (Not inherited: a program it starts isn't locked.)
pub fn lock_memory() -> Result<(), String> {
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: getrlimit writes `limit`.
    if unsafe { libc::getrlimit(libc::RLIMIT_MEMLOCK, &mut limit) } != 0 {
        return Err(format!("its limit can't be read: {}", std::io::Error::last_os_error()));
    }
    if limit.rlim_max != libc::RLIM_INFINITY {
        return Err(format!("the account may lock {} MB, not all a program has", limit.rlim_max >> 20));
    }
    if limit.rlim_cur != libc::RLIM_INFINITY {
        let all = libc::rlimit { rlim_cur: libc::RLIM_INFINITY, rlim_max: libc::RLIM_INFINITY };
        // SAFETY: setrlimit reads `all`; the soft limit raised to the hard one.
        if unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &all) } != 0 {
            return Err(format!("its limit can't be raised: {}", std::io::Error::last_os_error()));
        }
    }
    // SAFETY: mlockall takes flags only.
    if unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE | libc::MCL_ONFAULT) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

/// How much of this program is locked in memory, in bytes (the
/// kernel's count, `VmLck`: what's mapped under the lock).
pub fn locked_memory() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("VmLck:")).and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok()))
        .map_or(0, |kb| kb * 1024)
}

/// Whether process `pid`'s limits are this guard's (a thread there ends
/// only stuck, 30 s, with a report). Read from /proc: for app_server,
/// before it makes another program's threads realtime.
pub fn guarded(pid: u32) -> bool {
    let Ok(limits) = std::fs::read_to_string(format!("/proc/{pid}/limits")) else { return false };
    limits.lines().find(|l| l.starts_with("Max realtime timeout")).is_some_and(|l| {
        let mut n = l["Max realtime timeout".len()..].split_whitespace();
        let (soft, hard) = (n.next().and_then(|v| v.parse::<u64>().ok()), n.next().and_then(|v| v.parse::<u64>().ok()));
        (soft, hard) == (Some(STUCK_LIMIT_US), Some(HARD_LIMIT_US))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Memory is locked only where the account's limit is none; with
    /// one, it's left as it is and the limit said. (Whichever this test's
    /// own limit is: under a login's 8 MB it's refused; in a session Olo
    /// Doctor has let lock, everything mapped is under the lock.)
    #[test]
    fn memory_is_locked_only_without_a_limit() {
        let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: getrlimit writes `limit`.
        unsafe { libc::getrlimit(libc::RLIMIT_MEMLOCK, &mut limit) };
        match lock_memory() {
            Ok(()) => {
                assert_eq!(limit.rlim_max, libc::RLIM_INFINITY);
                assert!(locked_memory() > 1 << 20, "{} bytes under the lock", locked_memory());
            }
            Err(e) => {
                assert_ne!(limit.rlim_max, libc::RLIM_INFINITY, "{e}");
                assert_eq!(e, format!("the account may lock {} MB, not all a program has", limit.rlim_max >> 20));
                assert_eq!(locked_memory(), 0);
            }
        }
    }

    #[test]
    fn a_guarded_program_says_so_in_its_limits() {
        // (This test process: guarded once registered.)
        let tid = register_self(5).unwrap();
        assert!(registered().contains(&(tid, 5)));
        assert!(guarded(std::process::id()));
        forget(tid);
        assert!(!registered().iter().any(|(t, _)| *t == tid));
        // Stuck is half a minute, never a frame's or a buffer's worth.
        assert!(STUCK_LIMIT_US >= 30_000_000 && HARD_LIMIT_US > STUCK_LIMIT_US);
        assert!(!guarded(1), "init isn't ours, or unguarded");
    }

    /// A registered thread is marked; one it starts isn't (though it
    /// would take its name), and a registered one's nice is kept.
    #[test]
    fn a_registered_thread_is_marked_and_what_it_starts_isnt() {
        std::thread::spawn(|| {
            // SAFETY: setpriority on this thread (0 with PRIO_PROCESS is
            // the calling thread on Linux); raising a nice needs no privilege.
            unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 3) };
            register_self(8).unwrap();
            let me = tid();
            assert!(marked(me));
            // SAFETY: getpriority only reads.
            assert_eq!(unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) }, 3, "its nice kept");
            let child = std::thread::spawn(|| marked(tid())).join().unwrap();
            assert!(!child, "a thread it starts isn't marked");
            forget(me);
        })
        .join()
        .unwrap();
    }

    /// Stuck (the stuck limit's SIGXCPU): the program ends with an abort,
    /// as a crash (a report), said on stderr. (In a child process: it
    /// ends.)
    #[test]
    fn a_stuck_thread_ends_its_program_with_a_report() {
        if std::env::var_os("OLO_RT_STUCK_CHILD").is_some() {
            guard().unwrap();
            // SAFETY: raise sends this process a signal.
            unsafe { libc::raise(libc::SIGXCPU) };
            unreachable!("it went on");
        }
        use std::os::unix::process::ExitStatusExt;
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["realtime::tests::a_stuck_thread_ends_its_program_with_a_report", "--exact", "--nocapture"])
            .env("OLO_RT_STUCK_CHILD", "1")
            .output()
            .unwrap();
        assert_eq!(out.status.signal(), Some(libc::SIGABRT), "{out:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("stuck"), "{out:?}");
    }

    #[test]
    fn a_thread_that_ends_is_forgotten() {
        let tid = std::thread::spawn(|| register_self(3).unwrap()).join().unwrap();
        assert!(!registered().iter().any(|(t, _)| *t == tid));
    }

    #[test]
    fn reset_on_fork_is_still_realtime() {
        assert!(realtime_policy(libc::SCHED_RR | libc::SCHED_RESET_ON_FORK));
        assert!(realtime_policy(libc::SCHED_FIFO));
        assert!(!realtime_policy(libc::SCHED_OTHER | libc::SCHED_RESET_ON_FORK));
        assert!(!realtime_policy(libc::SCHED_IDLE));
        assert!(!realtime_policy(-1));
    }
}
