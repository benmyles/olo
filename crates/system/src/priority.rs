//! Scheduling priority for what keeps the desktop responsive. An
//! unprivileged process can only raise its nice (lower its priority);
//! the sanctioned way down is rtkit (`org.freedesktop.RealtimeKit1`, what
//! PipeWire uses), which gives a thread of the caller's own user a
//! negative nice down to its `MinNiceLevel` (−15 on Fedora), per thread.
//! Olo's tiers are `olo_protocol::performance::tier`: app_server (input
//! and every frame) highest, then the session, Deskbar and the proxy;
//! each asks for itself as it starts (`raise_self`), and the supervisor
//! re-asserts them and shows them.

use std::io::Read as _;

use crate::Result;

const RTKIT: &str = "org.freedesktop.RealtimeKit1";
const PATH: &str = "/org/freedesktop/RealtimeKit1";

/// Thread `tid` of `pid` to `nice` (negative: a higher priority), through
/// rtkit. The process must be the caller's user's.
pub fn high_priority(pid: u32, tid: u32, nice: i32) -> Result<()> {
    let conn = crate::bus::system()?;
    conn.call_method(Some(RTKIT), PATH, Some(RTKIT), "MakeThreadHighPriorityWithPID", &(pid as u64, tid as u64, nice))?;
    Ok(())
}

/// The lowest nice rtkit gives (−15 by default), or None without it.
pub fn min_nice() -> Option<i32> {
    let conn = crate::bus::system().ok()?;
    let reply = conn.call_method(Some(RTKIT), PATH, Some("org.freedesktop.DBus.Properties"), "Get", &(RTKIT, "MinNiceLevel")).ok()?;
    let v: zvariant::OwnedValue = reply.body().deserialize().ok()?;
    i32::try_from(v).ok()
}

/// The nice of thread `tid` (its own process's for a pid), from /proc.
pub fn nice_of(tid: u32) -> Option<i32> {
    let mut stat = String::new();
    std::fs::File::open(format!("/proc/{tid}/stat")).ok()?.read_to_string(&mut stat).ok()?;
    let after = &stat[stat.rfind(')')? + 1..];
    after.split_whitespace().nth(16)?.parse().ok()
}

/// The calling thread to `nice` (clamped to what rtkit gives). What it
/// is at now, if that could be read, and the error if it couldn't be
/// raised.
pub fn raise_self(nice: i32) -> std::result::Result<i32, String> {
    // SAFETY: gettid has no preconditions.
    let tid = unsafe { libc::gettid() } as u32;
    raise_thread(std::process::id(), tid, nice)
}

/// Thread `tid` of `pid` to `nice`, if it is above that.
pub fn raise_thread(pid: u32, tid: u32, nice: i32) -> std::result::Result<i32, String> {
    let nice = nice.max(min_nice().unwrap_or(-15));
    if let Some(now) = nice_of(tid)
        && now <= nice
    {
        return Ok(now);
    }
    high_priority(pid, tid, nice).map_err(|e| e.to_string())?;
    Ok(nice_of(tid).unwrap_or(nice))
}

/// The calling thread taken by the scheduler as at least `min` busy (of
/// 1024) while it runs, its uclamp.min (no privilege needed): it runs on
/// a fast core, at a high clock, not where a mostly idle thread is put
/// (`olo_protocol::performance::CORE_UTIL_MIN`). The threads it starts
/// afterwards inherit it, as they do its nice.
pub fn util_min_self(min: u32) -> std::io::Result<()> {
    #[repr(C)]
    #[derive(Default)]
    struct SchedAttr {
        size: u32,
        policy: u32,
        flags: u64,
        nice: i32,
        priority: u32,
        runtime: u64,
        deadline: u64,
        period: u64,
        util_min: u32,
        util_max: u32,
    }
    const KEEP_POLICY: u64 = 0x08;
    const KEEP_PARAMS: u64 = 0x10;
    const UTIL_CLAMP_MIN: u64 = 0x20;
    let attr = SchedAttr { size: size_of::<SchedAttr>() as u32, flags: KEEP_POLICY | KEEP_PARAMS | UTIL_CLAMP_MIN, util_min: min.min(1024), ..Default::default() };
    // SAFETY: sched_setattr reads `attr`, of the size it's told, for this
    // thread (0).
    if unsafe { libc::syscall(libc::SYS_sched_setattr, 0, &attr as *const SchedAttr, 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Every thread of `pid` to `nice`, those above it; how many were raised,
/// and the first error.
pub fn raise_process(pid: u32, nice: i32) -> (usize, Option<String>) {
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else { return (0, Some("no such process".into())) };
    let mut raised = 0;
    let mut error = None;
    for t in tasks.flatten() {
        let Ok(tid) = t.file_name().to_string_lossy().parse::<u32>() else { continue };
        if nice_of(tid).is_some_and(|n| n <= nice) {
            continue;
        }
        match raise_thread(pid, tid, nice) {
            Ok(_) => raised += 1,
            Err(e) => {
                error.get_or_insert(e);
            }
        }
    }
    (raised, error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_its_own_nice() {
        let me = std::process::id();
        let n = nice_of(me).expect("own nice");
        assert!((-20..=19).contains(&n));
        // Already there (or below): nothing asked of rtkit.
        assert_eq!(raise_thread(me, me, n), Ok(n));
    }

    #[test]
    fn a_process_that_is_gone_is_an_error() {
        let (n, e) = raise_process(u32::MAX - 7, -1);
        assert_eq!(n, 0);
        assert!(e.is_some());
    }
}
