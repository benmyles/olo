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

/// Thread `tid` of `pid` made realtime: SCHED_RR at `priority`, set
/// directly where the account lets it (RLIMIT_RTPRIO, Olo Doctor's Allow
/// Realtime), with no limit on how long it may run: never through rtkit,
/// whose rule (a hard limit of 200 ms on realtime time without sleeping)
/// ends the program at that. The process must be the caller's user's.
pub fn realtime(pid: u32, tid: u32, priority: u32) -> Result<()> {
    make_realtime(pid, tid, priority)
}

/// [`realtime`] (`olo_xdg::realtime`'s, where the kit's programs reach
/// it).
pub fn make_realtime(pid: u32, tid: u32, priority: u32) -> Result<()> {
    olo_xdg::realtime::make_realtime(pid, tid, priority).map_err(zbus::Error::Failure)
}

pub use olo_xdg::realtime::is_realtime;

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

/// Why this program isn't raised above others (`raise_self`,
/// `util_min_self`), if it isn't: it runs on an agent's screen
/// (`OLO_SCREEN`), background work as far as the person's desktop is
/// concerned, which three agent screens' compositors at app_server's own
/// nice (and on fast cores at full clock) competed with, under load.
pub fn not_the_persons() -> Option<&'static str> {
    std::env::var_os("OLO_SCREEN").is_some().then_some("on an agent's screen: left at its nice")
}

/// The calling thread to `nice` (clamped to what rtkit gives). What it
/// is at now, if that could be read, and the error if it couldn't be
/// raised.
pub fn raise_self(nice: i32) -> std::result::Result<i32, String> {
    if let Some(why) = not_the_persons() {
        return Err(why.into());
    }
    // SAFETY: gettid has no preconditions.
    let tid = unsafe { libc::gettid() } as u32;
    raise_thread(std::process::id(), tid, nice)
}

/// [`raise_self`] for a thread that has its own work to get on with and
/// can't ask again itself: where rtkit says no, a helper thread of this
/// program asks for it again, every few seconds for a minute, until it
/// is raised or gone. rtkit gives so many raises in a burst and no more
/// (25 in 20 s for all of an account's programs, as it comes: "Device or
/// resource busy" after that), and a session coming back starts more
/// threads than that at once: Terminal's daemon two for every shell,
/// Terminal three more. One refused stayed at nice 0 for as long as it
/// lived (five of olo-termd's, seen 2026-11-20). The helper sleeps
/// between rounds and ends when nothing is left to ask for.
pub fn raise_self_patiently(nice: i32) {
    if raise_self(nice).is_ok() || not_the_persons().is_some() {
        return;
    }
    // SAFETY: gettid has no preconditions.
    let tid = unsafe { libc::gettid() } as u32;
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    waiting.list.push(Asked { tid, nice, rounds: 0 });
    if !waiting.helper {
        waiting.helper = std::thread::Builder::new().name("raise-again".into()).spawn(ask_again).is_ok();
    }
}

/// A thread rtkit refused, to be asked for again.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Asked {
    tid: u32,
    nice: i32,
    rounds: u32,
}

struct Waiting {
    list: Vec<Asked>,
    /// The helper is running (it clears this, under the lock, as it ends).
    helper: bool,
}

static WAITING: std::sync::Mutex<Waiting> = std::sync::Mutex::new(Waiting { list: Vec::new(), helper: false });
/// Between rounds, and how many: a little over rtkit's burst three times.
const AGAIN_EVERY: std::time::Duration = std::time::Duration::from_secs(7);
const ROUNDS: u32 = 9;

fn ask_again() {
    let pid = std::process::id();
    loop {
        std::thread::sleep(AGAIN_EVERY);
        let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
        let list = std::mem::take(&mut waiting.list);
        waiting.list = round(list, |tid| std::path::Path::new(&format!("/proc/{pid}/task/{tid}")).exists(), |tid, nice| raise_thread(pid, tid, nice).is_ok());
        if waiting.list.is_empty() {
            waiting.helper = false;
            return;
        }
    }
}

/// One round of asking again: who is still to be asked for after it. A
/// thread that is gone (`there`) is dropped, as is one asked for
/// [`ROUNDS`] times; once rtkit says no (`raise`) the burst is spent and
/// the rest wait for the next round unasked.
fn round(list: Vec<Asked>, there: impl Fn(u32) -> bool, mut raise: impl FnMut(u32, i32) -> bool) -> Vec<Asked> {
    let mut spent = false;
    list.into_iter()
        .filter(|a| there(a.tid))
        .filter_map(|mut a| {
            if !spent {
                if raise(a.tid, a.nice) {
                    return None;
                }
                spent = true;
                a.rounds += 1;
            }
            (a.rounds < ROUNDS).then_some(a)
        })
        .collect()
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
    set_util_min(0, min)
}

/// Set the scheduler hint on a selected thread without changing its class,
/// nice, priority or maximum utilization. Zero means the calling thread.
pub fn set_util_min(tid: i32, min: u32) -> std::io::Result<()> {
    if let Some(why) = not_the_persons() {
        return Err(std::io::Error::other(why));
    }
    const KEEP_POLICY: u64 = 0x08;
    const KEEP_PARAMS: u64 = 0x10;
    const UTIL_CLAMP_MIN: u64 = 0x20;
    let attr = SchedAttr { size: size_of::<SchedAttr>() as u32, flags: KEEP_POLICY | KEEP_PARAMS | UTIL_CLAMP_MIN, util_min: min.min(1024), ..Default::default() };
    // SAFETY: sched_setattr reads `attr`, of the size it's told, for this
    // thread (0).
    if unsafe { libc::syscall(libc::SYS_sched_setattr, tid, &attr as *const SchedAttr, 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// The minimum utilisation thread `tid` is taken as (`util_min_self`):
/// 0 for none, None if it can't be read.
pub fn util_min_of(tid: u32) -> Option<u32> {
    let mut attr = SchedAttr::default();
    // SAFETY: sched_getattr writes at most `size` bytes into `attr`.
    if unsafe { libc::syscall(libc::SYS_sched_getattr, tid as libc::pid_t, &mut attr as *mut SchedAttr, size_of::<SchedAttr>() as u32, 0) } != 0 {
        return None;
    }
    Some(attr.util_min)
}

/// The kernel's `struct sched_attr` (sched_setattr(2)).
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

    /// Asking again: the raised and the gone leave the list, a refusal
    /// ends the round (the burst is spent: nobody after it is asked, nor
    /// counted a round), and one refused nine rounds is given up.
    #[test]
    fn a_refused_raise_is_asked_for_again_until_it_is_given() {
        let asked = |tid| Asked { tid, nice: -5, rounds: 0 };
        let list = vec![asked(1), asked(2), asked(3), asked(4)];
        // 1 is raised, 2 is gone, 3 is refused: 4 isn't asked.
        let mut tried = Vec::new();
        let left = round(
            list,
            |tid| tid != 2,
            |tid, nice| {
                tried.push((tid, nice));
                tid == 1
            },
        );
        assert_eq!(tried, [(1, -5), (3, -5)]);
        assert_eq!(left, [Asked { tid: 3, nice: -5, rounds: 1 }, asked(4)]);
        // The next round both are given.
        assert!(round(left, |_| true, |_, _| true).is_empty());
        // Refused every time: given up after the last round.
        let mut one = vec![asked(7)];
        for n in 1..ROUNDS {
            one = round(one, |_| true, |_, _| false);
            assert_eq!(one, [Asked { tid: 7, nice: -5, rounds: n }]);
        }
        assert!(round(one, |_| true, |_, _| false).is_empty());
    }

    /// The helper itself, with rtkit: a thread put on the list as one
    /// refused is raised at the next round, and the helper ends
    /// (`cargo test -p olo-system the_helper_raises -- --ignored`; it
    /// waits a round, seven seconds).
    #[test]
    #[ignore = "asks rtkit to raise a thread of this test"]
    fn the_helper_raises_a_thread_left_waiting() {
        let (tell, told) = std::sync::mpsc::channel();
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let worker = std::thread::spawn(move || {
            // SAFETY: gettid has no preconditions.
            tell.send(unsafe { libc::gettid() } as u32).unwrap();
            let _ = stopped.recv();
        });
        let tid = told.recv().unwrap();
        assert_eq!(nice_of(tid), Some(nice_of(std::process::id()).unwrap()));
        {
            let mut waiting = WAITING.lock().unwrap();
            waiting.list.push(Asked { tid, nice: -1, rounds: 0 });
            assert!(!waiting.helper);
            waiting.helper = std::thread::Builder::new().name("raise-again".into()).spawn(ask_again).is_ok();
        }
        std::thread::sleep(AGAIN_EVERY + std::time::Duration::from_secs(2));
        assert_eq!(nice_of(tid), Some(-1), "rtkit didn't raise it (is rtkit running?)");
        let waiting = WAITING.lock().unwrap();
        assert!(waiting.list.is_empty() && !waiting.helper, "the helper is still there with nothing to ask for");
        drop(waiting);
        drop(stop);
        worker.join().unwrap();
    }

    #[test]
    fn reads_its_own_nice() {
        let me = std::process::id();
        let n = nice_of(me).expect("own nice");
        assert!((-20..=19).contains(&n));
        // Already there (or below): nothing asked of rtkit.
        assert_eq!(raise_thread(me, me, n), Ok(n));
    }

    /// On a thread of this test's own, where the account lets it.
    #[test]
    #[ignore = "needs the account's leave for realtime (Olo Doctor's Allow Realtime)"]
    fn makes_a_thread_realtime() {
        let (tx, rx) = std::sync::mpsc::channel();
        let t = std::thread::spawn(move || {
            // SAFETY: gettid has no preconditions.
            let tid = unsafe { libc::gettid() } as u32;
            tx.send(tid).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(300));
        });
        let tid = rx.recv().unwrap();
        assert!(!is_realtime(tid));
        realtime(std::process::id(), tid, 5).expect("the account's leave");
        assert!(is_realtime(tid));
        t.join().unwrap();
    }

    #[test]
    fn a_process_that_is_gone_is_an_error() {
        let (n, e) = raise_process(u32::MAX - 7, -1);
        assert_eq!(n, 0);
        assert!(e.is_some());
    }
}
