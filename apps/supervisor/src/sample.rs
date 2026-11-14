//! Looking at the person's own processes and the system's memory, from
//! `/proc` (nothing it reads needs privileges): each process's state,
//! parent, CPU time, voluntary context switches, I/O and anonymous
//! memory; the memory available, its stalls (PSI) and swapping. A
//! command line (costly to read: it is in the process's memory) is read
//! once a process, by the caller (`command`).

use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use crate::detect::{Sample, System};

#[derive(Debug, Clone)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub name: String,
    pub state: char,
    /// When it started (clock ticks after boot): with the pid, which
    /// process this is (pids are used again).
    pub start: u64,
    pub sample: Sample,
}

fn ticks() -> f64 {
    // SAFETY: plain sysconf.
    (unsafe { libc::sysconf(libc::_SC_CLK_TCK) }).max(1) as f64
}

fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(':')).map(str::trim)
}

fn kb(text: &str, key: &str) -> u64 {
    field(text, key).and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok()).unwrap_or(0) * 1024
}

/// Whose each process is that its folder alone doesn't say: kept by pid
/// with the folder's change time (a pid used again is a new folder).
///
/// A process's folder in /proc is its effective user's (the kernel keeps
/// it so for this, even for one that can't be looked into), so most are
/// told apart by that alone: a `stat`, where `status` is a text the
/// kernel writes, ten times the cost, for ~750 processes every two
/// seconds. But the person's real user may be running as another (a
/// setuid program: sudo, pkexec), whose folder is root's: those, and
/// root's own, have their `status` read once.
#[derive(Default)]
pub struct Owners {
    by_pid: HashMap<i32, ((i64, i64), bool)>,
}

/// The processes of user `uid`, looked at now (`t`, seconds).
pub fn processes(uid: u32, t: f64, owners: &mut Owners) -> Vec<Proc> {
    use std::os::unix::fs::MetadataExt as _;
    let hz = ticks();
    let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
    let (mut status_buf, mut stat_buf, mut io_buf) = (Vec::new(), Vec::new(), Vec::new());
    let mut seen = HashSet::new();
    let out = dir
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
        .filter_map(|pid| {
            let base = Path::new("/proc").join(pid.to_string());
            let meta = std::fs::metadata(&base).ok()?;
            seen.insert(pid);
            // Its user's, someone else's, or root's (root's own, or one of
            // the person's running as root): its status says, once.
            if meta.uid() != uid && (meta.uid() != 0 || uid == 0) {
                return None;
            }
            let read_status = |buf: &mut Vec<u8>| -> Option<usize> { olo_system::procs::read_record(&base.join("status"), buf) };
            if meta.uid() != uid {
                let at = (meta.ctime(), meta.ctime_nsec());
                match owners.by_pid.get(&pid) {
                    Some((when, mine)) if *when == at && !mine => return None,
                    Some((when, _)) if *when == at => {}
                    _ => {
                        let n = read_status(&mut status_buf)?;
                        let mine = owner(std::str::from_utf8(&status_buf[..n]).ok()?) == Some(uid);
                        owners.by_pid.insert(pid, (at, mine));
                        if !mine {
                            return None;
                        }
                    }
                }
            }
            let n = read_status(&mut status_buf)?;
            let status = std::str::from_utf8(&status_buf[..n]).ok()?;
            // (Its folder its user's, but its real user may not be.)
            if owner(status) != Some(uid) {
                return None;
            }
            let n = olo_system::procs::read_record(&base.join("stat"), &mut stat_buf)?;
            let stat = String::from_utf8_lossy(&stat_buf[..n]);
            let (left, rest) = stat.rsplit_once(')')?;
            let name = left.split_once('(')?.1.to_owned();
            let f: Vec<&str> = rest.split_whitespace().collect();
            // After the name: state is field 3, so f[0]; ppid f[1]; utime
            // f[11], stime f[12]; starttime f[19].
            let state = f.first()?.chars().next()?;
            let cpu = (f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?) / hz;
            let io_text = olo_system::procs::read_record(&base.join("io"), &mut io_buf).and_then(|n| std::str::from_utf8(&io_buf[..n]).ok()).unwrap_or_default();
            let io = io_text.lines().filter_map(|l| l.strip_prefix("rchar: ").or_else(|| l.strip_prefix("wchar: "))).filter_map(|v| v.trim().parse::<u64>().ok()).sum();
            Some(Proc {
                pid,
                ppid: f.get(1)?.parse().ok()?,
                name,
                state,
                start: f.get(19)?.parse().ok()?,
                sample: Sample { t, cpu, switches: field(status, "voluntary_ctxt_switches").and_then(|v| v.parse().ok()).unwrap_or(0), io, memory: kb(status, "RssAnon") + kb(status, "VmSwap") },
            })
        })
        .collect();
    owners.by_pid.retain(|pid, _| seen.contains(pid));
    out
}

/// A process's real user, from its status.
fn owner(status: &str) -> Option<u32> {
    field(status, "Uid")?.split_whitespace().next()?.parse().ok()
}

/// `pid`'s command line, its arguments a space apart.
pub fn command(pid: i32) -> String {
    std::fs::read(format!("/proc/{pid}/cmdline")).map(|b| String::from_utf8_lossy(&b).replace('\0', " ").trim().to_owned()).unwrap_or_default()
}

/// Voluntary context switches of all of `pid`'s threads (the process's
/// own count is only its first thread's): for one that may spin.
pub fn all_switches(pid: i32) -> Option<u64> {
    let dir = std::fs::read_dir(format!("/proc/{pid}/task")).ok()?;
    Some(dir.flatten().filter_map(|e| std::fs::read_to_string(e.path().join("status")).ok()).filter_map(|s| field(&s, "voluntary_ctxt_switches")?.parse::<u64>().ok()).sum())
}

/// Pages swapped out so far.
pub fn swapped_out() -> u64 {
    std::fs::read_to_string("/proc/vmstat").ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("pswpout ")?.trim().parse().ok())).unwrap_or(0)
}

/// The system's memory now; `swapping` from `pages` swapped out over
/// `dt` seconds (the caller keeps the last count).
pub fn system(pages_per_s: f32) -> System {
    let mem = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let psi = std::fs::read_to_string("/proc/pressure/memory").unwrap_or_default();
    let avg10 = |kind: &str| -> f32 { psi.lines().find(|l| l.starts_with(kind)).and_then(|l| l.split_whitespace().find_map(|w| w.strip_prefix("avg10="))).and_then(|v| v.parse().ok()).unwrap_or(0.0) };
    System { total: kb(&mem, "MemTotal"), available: kb(&mem, "MemAvailable"), stall_some: avg10("some"), stall_full: avg10("full"), swapping: pages_per_s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sees_itself() {
        // SAFETY: plain getuid.
        let uid = unsafe { libc::getuid() };
        let me = std::process::id() as i32;
        let all = processes(uid, 1.0, &mut Owners::default());
        let p = all.iter().find(|p| p.pid == me).expect("not found");
        assert!(p.sample.cpu >= 0.0 && p.sample.memory > 0 && p.state != 'Z');
        assert!(all_switches(me).is_some());
        assert!(!command(me).is_empty());
        let s = system(0.0);
        assert!(s.total > 0 && s.available > 0 && s.available <= s.total);
    }

    /// Root's processes have their status read once, and are kept as
    /// not the person's; one of the person's that can't be looked into
    /// (not dumpable) is still found.
    #[test]
    fn root_read_once_and_one_not_dumpable_found() {
        // SAFETY: plain getuid.
        let uid = unsafe { libc::getuid() };
        if uid == 0 {
            return;
        }
        let me = std::process::id() as i32;
        // SAFETY: prctl on this process; put back below.
        unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) };
        let mut owners = Owners::default();
        let first = processes(uid, 1.0, &mut owners).iter().any(|p| p.pid == me);
        let init = owners.by_pid.get(&1).map(|(_, mine)| *mine);
        let again = processes(uid, 1.0, &mut owners).iter().any(|p| p.pid == me);
        // SAFETY: as above.
        unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 1, 0, 0, 0) };
        assert!(first && again, "found both times");
        assert_eq!(init, Some(false), "pid 1 is root's: read once, kept as not the person's");
    }

    fn cpu_ms() -> f64 {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: clock_gettime fills the struct it's given.
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
        t.tv_sec as f64 * 1e3 + t.tv_nsec as f64 / 1e6
    }

    /// What a look at the person's processes costs here, after the first
    /// (`--ignored --nocapture`).
    #[test]
    #[ignore]
    fn a_looks_cost() {
        // SAFETY: plain getuid.
        let uid = unsafe { libc::getuid() };
        let mut owners = Owners::default();
        let mut n = processes(uid, 1.0, &mut owners).len();
        let t = cpu_ms();
        for _ in 0..20 {
            n = processes(uid, 1.0, &mut owners).len();
        }
        eprintln!("a look: {:.2} ms of CPU ({n} processes of the person's)", (cpu_ms() - t) / 20.0);
    }
}
