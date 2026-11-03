//! Lowering background work, and putting it back. Every thread of the
//! process (nice and scheduling are each thread's on Linux; threads and
//! children made later take the lowered ones' from them): nice raised to
//! the setting's (never lowered: without privileges it can't come back
//! down), the batch scheduling policy (no preempting on wakeup; a
//! compiler's waits are short), and idle I/O priority (its reads and
//! writes only when nobody else's are waiting). Batch and the I/O
//! priority are put back when the supervisor stops or is turned off;
//! nice stays where it went until the process ends, which for builds is
//! soon.

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

fn policy(tid: i32, policy: i32) -> bool {
    let param = libc::sched_param { sched_priority: 0 };
    // SAFETY: sched_setscheduler on one of our own threads.
    unsafe { libc::sched_setscheduler(tid, policy, &param) == 0 }
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
        any |= policy(tid, libc::SCHED_BATCH);
        ioprio_set(tid, IOPRIO_CLASS_IDLE, 0);
    }
    any.then_some(before)
}

/// Put `pid`'s scheduling and I/O priority back (its nice can't come
/// down without privileges).
pub fn restore(pid: i32) {
    for tid in threads(pid) {
        policy(tid, libc::SCHED_OTHER);
        ioprio_set(tid, IOPRIO_CLASS_BE, 4);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowers_and_puts_back_a_child() {
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
}
