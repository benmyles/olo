//! Looking at the person's own processes and the system's memory, from
//! `/proc` (nothing it reads needs privileges): each process's state,
//! parent, CPU time, voluntary context switches, I/O and anonymous
//! memory; the memory available, its stalls (PSI) and swapping. A
//! command line (costly to read: it is in the process's memory) is read
//! once a process, by the caller (`command`).

use std::path::Path;

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

/// The processes of user `uid`, looked at now (`t`, seconds).
pub fn processes(uid: u32, t: f64) -> Vec<Proc> {
    let hz = ticks();
    let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
    dir.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
        .filter_map(|pid| {
            let base = Path::new("/proc").join(pid.to_string());
            let status = std::fs::read_to_string(base.join("status")).ok()?;
            let owner: u32 = field(&status, "Uid")?.split_whitespace().next()?.parse().ok()?;
            if owner != uid {
                return None;
            }
            let stat = std::fs::read_to_string(base.join("stat")).ok()?;
            let (left, rest) = stat.rsplit_once(')')?;
            let name = left.split_once('(')?.1.to_owned();
            let f: Vec<&str> = rest.split_whitespace().collect();
            // After the name: state is field 3, so f[0]; ppid f[1]; utime
            // f[11], stime f[12]; starttime f[19].
            let state = f.first()?.chars().next()?;
            let cpu = (f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?) / hz;
            let io_text = std::fs::read_to_string(base.join("io")).unwrap_or_default();
            let io = io_text.lines().filter_map(|l| l.strip_prefix("rchar: ").or_else(|| l.strip_prefix("wchar: "))).filter_map(|v| v.trim().parse::<u64>().ok()).sum();
            Some(Proc {
                pid,
                ppid: f.get(1)?.parse().ok()?,
                name,
                state,
                start: f.get(19)?.parse().ok()?,
                sample: Sample {
                    t,
                    cpu,
                    switches: field(&status, "voluntary_ctxt_switches").and_then(|v| v.parse().ok()).unwrap_or(0),
                    io,
                    memory: kb(&status, "RssAnon") + kb(&status, "VmSwap"),
                },
            })
        })
        .collect()
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
    let avg10 = |kind: &str| -> f32 {
        psi.lines().find(|l| l.starts_with(kind)).and_then(|l| l.split_whitespace().find_map(|w| w.strip_prefix("avg10="))).and_then(|v| v.parse().ok()).unwrap_or(0.0)
    };
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
        let all = processes(uid, 1.0);
        let p = all.iter().find(|p| p.pid == me).expect("not found");
        assert!(p.sample.cpu >= 0.0 && p.sample.memory > 0 && p.state != 'Z');
        assert!(all_switches(me).is_some());
        assert!(!command(me).is_empty());
        let s = system(0.0);
        assert!(s.total > 0 && s.available > 0 && s.available <= s.total);
    }
}
