//! The person's processes, read from /proc on a thread of their own every
//! two seconds while Rescue is up (it's up only while something's
//! wrong), the busiest first, for ending one.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use olo_ui::Proxy;

const EVERY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq)]
pub struct Proc {
    pub pid: i32,
    pub name: String,
    /// Of one CPU, since the last look (100 is one CPU all the time).
    pub cpu: f32,
    /// Resident memory, bytes.
    pub memory: u64,
}

/// What the thread found: every process of ours, busiest first.
pub struct Found(pub Vec<Proc>);

/// Look every two seconds, from now on.
pub fn watch(proxy: Proxy) {
    let spawned = std::thread::Builder::new().name("rescue processes".into()).spawn(move || {
        let mut last: HashMap<i32, u64> = HashMap::new();
        let mut at = Instant::now();
        loop {
            let now = Instant::now();
            let secs = now.duration_since(at).as_secs_f32().max(0.001);
            at = now;
            let mut ticks = HashMap::new();
            let mut found = Vec::new();
            for (pid, name, t, memory) in read_all() {
                let cpu = last.get(&pid).map_or(0.0, |&before| t.saturating_sub(before) as f32 / clock_ticks() / secs * 100.0);
                ticks.insert(pid, t);
                found.push(Proc { pid, name, cpu, memory });
            }
            last = ticks;
            found.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.memory.cmp(&a.memory)));
            proxy.send(Found(found));
            std::thread::sleep(EVERY);
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("Rescue's process list: {e}");
    }
}

fn clock_ticks() -> f32 {
    // SAFETY: sysconf only reads.
    (unsafe { libc::sysconf(libc::_SC_CLK_TCK) }).max(1) as f32
}

fn page_size() -> u64 {
    // SAFETY: sysconf only reads.
    (unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).max(1) as u64
}

/// Every process of this user: pid, name, CPU ticks so far, resident bytes.
fn read_all() -> Vec<(i32, String, u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    // SAFETY: getuid has no preconditions.
    let me = unsafe { libc::getuid() };
    let mine = std::process::id() as i32;
    let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
    dir.flatten()
        .filter_map(|e| {
            let pid: i32 = e.file_name().to_str()?.parse().ok()?;
            if pid == mine || e.metadata().ok()?.uid() != me {
                return None;
            }
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            let (name, ticks) = parse_stat(&stat)?;
            let rss = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok().and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok()).unwrap_or(0) * page_size();
            // A kernel thread or a zombie has no memory: nothing to end.
            (rss > 0).then_some((pid, name, ticks, rss))
        })
        .collect()
}

/// A /proc/PID/stat's name (in parentheses, which it may hold itself)
/// and its user and system CPU ticks.
fn parse_stat(stat: &str) -> Option<(String, u64)> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat.get(open + 1..close)?.to_owned();
    let fields: Vec<&str> = stat.get(close + 2..)?.split_whitespace().collect();
    // After the name: state (3), …, utime (14) and stime (15).
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some((name, utime + stime))
}

/// End `pid`: asked (SIGTERM), or at once (`force`: SIGKILL).
pub fn end(pid: i32, force: bool) -> Result<(), String> {
    // SAFETY: kill only sends a signal.
    if unsafe { libc::kill(pid, if force { libc::SIGKILL } else { libc::SIGTERM }) } == 0 { Ok(()) } else { Err(std::io::Error::last_os_error().to_string()) }
}

pub fn memory(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 { format!("{:.1} GB", mb / 1024.0) } else { format!("{mb:.0} MB") }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_name_with_parentheses_and_spaces() {
        let stat = "1234 (Web Content (x)) S 1 1234 1234 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 1 0 100 1000 10";
        assert_eq!(super::parse_stat(stat), Some(("Web Content (x)".into(), 300)));
    }

    #[test]
    fn this_users_processes_are_found() {
        // (Ourselves left out, our parent, the test harness, not.)
        assert!(!super::read_all().is_empty());
    }
}
