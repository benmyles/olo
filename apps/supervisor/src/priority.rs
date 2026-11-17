//! Lowering background work, and putting it back. Every thread of the
//! process (nice and scheduling are each thread's on Linux; threads and
//! children made later take the lowered ones' from them): nice raised to
//! the setting's (never lowered: without privileges it can't come back
//! down), the batch scheduling policy (no preempting on wakeup; a
//! compiler's waits are short), and idle I/O priority (its reads and
//! writes only when nobody else's are waiting). Batch and the I/O
//! priority are put back when the supervisor stops or is turned off;
//! nice stays where it went until the process ends, which for builds is
//! soon. A realtime thread (a program's sound) is never touched, lowering
//! or putting back: only threads at batch are put back, so one made
//! realtime meanwhile stays so.
//!
//! Nice orders threads only inside their cgroup: the processor is shared
//! out between cgroups first, by their weights. A build run as a service
//! of the person's own manager (`user@UID.service/…`) is in a cgroup of
//! its own, beside Olo's login session rather than inside it, and Fedora's
//! uresourced gives the user manager five times the session's weight; so
//! the unit itself is lowered as well (`unit_lowerable`, main.rs:
//! `CPUWeight=idle`, its cgroup's `cpu.idle`), and put back with it.

use std::path::Path;

const IOPRIO_CLASS_SHIFT: i32 = 13;
const IOPRIO_CLASS_BE: i32 = 2;
const IOPRIO_CLASS_IDLE: i32 = 3;
const IOPRIO_WHO_PROCESS: i32 = 1;

fn threads(pid: i32) -> Vec<i32> {
    std::fs::read_dir(Path::new("/proc").join(pid.to_string()).join("task")).map(|d| d.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect()).unwrap_or_default()
}

fn ioprio_set(tid: i32, class: i32, level: i32) -> bool {
    // SAFETY: the ioprio_set syscall on one of our own threads.
    unsafe { libc::syscall(libc::SYS_ioprio_set, IOPRIO_WHO_PROCESS, tid, (class << IOPRIO_CLASS_SHIFT) | level) == 0 }
}

/// Thread `tid` in class `policy`, its mark kept: a thread registered
/// as one that may be realtime (`olo_app::realtime`: a window's looper)
/// has SCHED_RESET_ON_FORK on its class, which the kernel lets nobody
/// without privilege take off (EPERM), so set without it a marked thread
/// was neither lowered nor, lowered before it was marked, put back.
fn policy(tid: i32, policy: i32) -> bool {
    let param = libc::sched_param { sched_priority: 0 };
    // SAFETY: sched_getscheduler only reads; sched_setscheduler on one
    // of our own threads.
    unsafe {
        let mark = libc::sched_getscheduler(tid).max(0) & libc::SCHED_RESET_ON_FORK;
        libc::sched_setscheduler(tid, policy | mark, &param) == 0
    }
}

fn nice_of(tid: i32) -> Option<i32> {
    // SAFETY: getpriority after clearing errno (it may return -1).
    unsafe {
        *libc::__errno_location() = 0;
        let n = libc::getpriority(libc::PRIO_PROCESS, tid as libc::id_t);
        (*libc::__errno_location() == 0).then_some(n)
    }
}

/// Lower `pid` to `nice` (only ever up), batch scheduling and idle I/O.
/// Its nice before, if it could be read; false if it couldn't be lowered
/// at all (it went, or isn't ours).
pub fn lower(pid: i32, nice: i32) -> Option<i32> {
    let before = nice_of(pid)?;
    let mut any = false;
    for tid in threads(pid) {
        if let Some(n) = nice_of(tid)
            && n < nice
        {
            // SAFETY: setpriority on one of our own threads.
            unsafe { libc::setpriority(libc::PRIO_PROCESS, tid as libc::id_t, nice) };
        }
        // (Realtime: what it's for is never waiting.)
        if olo_system::realtime::is_realtime(tid as u32) {
            continue;
        }
        any |= policy(tid, libc::SCHED_BATCH);
        ioprio_set(tid, IOPRIO_CLASS_IDLE, 0);
    }
    any.then_some(before)
}

/// Whether the user manager's unit `unit` may be lowered whole: a
/// service, or a scope `systemd-run` made (`run-…`); not an app the
/// person started (`app-…`), nor the manager's own.
pub fn unit_lowerable(unit: &str) -> bool {
    (unit.ends_with(".service") || (unit.starts_with("run-") && unit.ends_with(".scope"))) && !unit.starts_with("app-") && unit != "init.scope"
}

/// Put `pid`'s scheduling and I/O priority back (its nice can't come
/// down without privileges).
pub fn restore(pid: i32) {
    for tid in threads(pid) {
        // (Only what was lowered: one made realtime since stays so.)
        // SAFETY: sched_getscheduler only reads.
        if unsafe { libc::sched_getscheduler(tid) } & !libc::SCHED_RESET_ON_FORK == libc::SCHED_BATCH {
            policy(tid, libc::SCHED_OTHER);
        }
        ioprio_set(tid, IOPRIO_CLASS_BE, 4);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests themselves run in the idle class (`chrt -i`): nothing
    /// leaves it without privilege, so there is no lowering to batch and
    /// no putting back to see.
    fn idle() -> bool {
        // SAFETY: sched_getscheduler only reads.
        unsafe { libc::sched_getscheduler(0) & !libc::SCHED_RESET_ON_FORK == libc::SCHED_IDLE }
    }

    #[test]
    fn lowers_and_puts_back_a_child() {
        if idle() {
            return;
        }
        // A process of our own to lower: `sleep`, found by its pid.
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id() as i32;
        let before = lower(pid, 12).expect("not lowered");
        assert_eq!(before, nice_of(pid).map(|_| before).unwrap());
        assert_eq!(nice_of(pid), Some(12.max(before)));
        // SAFETY: plain sched_getscheduler.
        assert_eq!(unsafe { libc::sched_getscheduler(pid) }, libc::SCHED_BATCH);
        restore(pid);
        // SAFETY: as above.
        assert_eq!(unsafe { libc::sched_getscheduler(pid) }, libc::SCHED_OTHER);
        let _ = child.kill();
        let _ = child.wait();
    }

    /// A marked thread (a window's looper: SCHED_RESET_ON_FORK on its
    /// class) is lowered and put back as any, its mark kept.
    #[test]
    fn a_marked_thread_is_lowered_and_put_back() {
        if idle() {
            return;
        }
        std::thread::spawn(|| {
            let param = libc::sched_param { sched_priority: 0 };
            // SAFETY: plain scheduler calls on this thread; setting the
            // mark needs no privilege.
            unsafe {
                let me = libc::gettid();
                assert_eq!(libc::sched_setscheduler(me, libc::SCHED_OTHER | libc::SCHED_RESET_ON_FORK, &param), 0);
                assert!(policy(me, libc::SCHED_BATCH));
                assert_eq!(libc::sched_getscheduler(me), libc::SCHED_BATCH | libc::SCHED_RESET_ON_FORK);
                assert!(policy(me, libc::SCHED_OTHER));
                assert_eq!(libc::sched_getscheduler(me), libc::SCHED_OTHER | libc::SCHED_RESET_ON_FORK);
            }
        })
        .join()
        .unwrap();
    }
}
