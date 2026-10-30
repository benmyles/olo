//! Everything about one process, for its inspector: read from /proc on a
//! worker thread (or by the privileged helper, for other users'), never
//! on the UI thread. What can't be read without full access comes back
//! as None, so the inspector can say so rather than show nothing.

use std::{collections::HashMap, path::Path};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Thread {
    pub tid: i32,
    pub name: String,
    pub state: char,
    /// CPU time so far, in clock ticks (a rate needs two readings).
    pub ticks: u64,
    pub nice: i32,
    /// The core it last ran on.
    pub core: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenFile {
    pub fd: i32,
    /// File, Folder, Socket, Pipe, Device, Event, Other.
    pub kind: String,
    pub target: String,
    /// How it was opened: "r", "w" or "rw".
    pub mode: String,
}

/// A process's mappings of one file (or of anonymous memory, the heap,
/// the stack), summed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Region {
    pub path: String,
    pub count: usize,
    pub size: u64,
    /// Read, write, execute, as any of its mappings allow.
    pub perms: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Limit {
    pub name: String,
    /// The `RLIMIT_*` it is (for changing it), when Olo knows it.
    pub resource: Option<String>,
    pub soft: String,
    pub hard: String,
    pub units: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Details {
    pub pid: i32,
    pub exe: Option<String>,
    pub cwd: Option<String>,
    pub command: Vec<String>,
    pub cgroup: String,
    /// /proc/PID/status, as (field, value).
    pub status: Vec<(String, String)>,
    /// smaps_rollup's totals (Rss, Pss, Shared and Private, Swap…).
    pub memory: Option<Vec<(String, u64)>>,
    pub io: Option<Vec<(String, u64)>>,
    pub threads: Vec<Thread>,
    pub files: Option<Vec<OpenFile>>,
    pub regions: Option<Vec<Region>>,
    pub environment: Option<Vec<(String, String)>>,
    pub limits: Vec<Limit>,
    /// The cores it may run on.
    pub affinity: Option<Vec<usize>>,
    pub oom_score: Option<i32>,
    pub oom_adjust: Option<i32>,
    /// Its scheduling policy (Normal, Batch, Idle, FIFO, Round-robin).
    pub policy: String,
    /// Namespaces it is in, by kind, when they differ from init's.
    pub namespaces: Vec<(String, String)>,
}

fn read(p: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(p).ok()
}

fn link(p: impl AsRef<Path>) -> Option<String> {
    std::fs::read_link(p).ok().map(|t| t.to_string_lossy().into_owned())
}

/// The limits /proc/PID/limits names, and the resource each is.
const LIMITS: &[(&str, &str)] = &[
    ("Max cpu time", "cpu"),
    ("Max file size", "fsize"),
    ("Max data size", "data"),
    ("Max stack size", "stack"),
    ("Max core file size", "core"),
    ("Max resident set", "rss"),
    ("Max processes", "nproc"),
    ("Max open files", "nofile"),
    ("Max locked memory", "memlock"),
    ("Max address space", "as"),
    ("Max file locks", "locks"),
    ("Max pending signals", "sigpending"),
    ("Max msgqueue size", "msgqueue"),
    ("Max nice priority", "nice"),
    ("Max realtime priority", "rtprio"),
    ("Max realtime timeout", "rttime"),
];

/// A limit's resource number, for prlimit.
pub fn resource(name: &str) -> Option<libc::__rlimit_resource_t> {
    Some(match name {
        "cpu" => libc::RLIMIT_CPU,
        "fsize" => libc::RLIMIT_FSIZE,
        "data" => libc::RLIMIT_DATA,
        "stack" => libc::RLIMIT_STACK,
        "core" => libc::RLIMIT_CORE,
        "rss" => libc::RLIMIT_RSS,
        "nproc" => libc::RLIMIT_NPROC,
        "nofile" => libc::RLIMIT_NOFILE,
        "memlock" => libc::RLIMIT_MEMLOCK,
        "as" => libc::RLIMIT_AS,
        "locks" => libc::RLIMIT_LOCKS,
        "sigpending" => libc::RLIMIT_SIGPENDING,
        "msgqueue" => libc::RLIMIT_MSGQUEUE,
        "nice" => libc::RLIMIT_NICE,
        "rtprio" => libc::RLIMIT_RTPRIO,
        "rttime" => libc::RLIMIT_RTTIME,
        _ => return None,
    })
}

/// /proc/PID/limits: fixed columns (26, 21, 21 wide), "unlimited" or a
/// number, then units.
fn limits(text: &str) -> Vec<Limit> {
    text.lines()
        .skip(1)
        .filter(|l| l.len() > 47)
        .map(|l| {
            let at = |a: usize, b: usize| l.get(a..b.min(l.len())).unwrap_or("").trim().to_owned();
            let name = at(0, 26);
            let resource = LIMITS.iter().find(|(n, _)| *n == name).map(|(_, r)| (*r).to_owned());
            Limit { name, resource, soft: at(26, 47), hard: at(47, 68), units: at(68, 200) }
        })
        .collect()
}

/// (kind, target) of a descriptor's link.
fn file_kind(target: &str, sockets: &HashMap<u64, String>) -> (String, String) {
    if let Some(inode) = target.strip_prefix("socket:[").and_then(|t| t.strip_suffix(']')).and_then(|t| t.parse::<u64>().ok()) {
        return ("Socket".into(), sockets.get(&inode).cloned().unwrap_or_else(|| "Local socket".into()));
    }
    if target.starts_with("pipe:[") {
        return ("Pipe".into(), target.into());
    }
    if let Some(rest) = target.strip_prefix("anon_inode:") {
        let kind = match rest.trim_matches(['[', ']']) {
            "eventfd" | "eventpoll" | "timerfd" | "signalfd" | "inotify" => "Event",
            _ => "Other",
        };
        return (kind.into(), rest.trim_matches(['[', ']']).into());
    }
    if target.starts_with("/dev/") {
        return ("Device".into(), target.into());
    }
    if Path::new(target).is_dir() {
        return ("Folder".into(), target.into());
    }
    ("File".into(), target.into())
}

/// Socket inode → what it is ("TCP 127.0.0.1:80 → 1.2.3.4:5000").
fn sockets() -> HashMap<u64, String> {
    let mut out = HashMap::new();
    for (file, proto) in [("tcp", "TCP"), ("tcp6", "TCP"), ("udp", "UDP"), ("udp6", "UDP")] {
        for l in read(format!("/proc/net/{file}")).unwrap_or_default().lines().skip(1) {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 10 {
                continue;
            }
            let Ok(inode) = f[9].parse::<u64>() else { continue };
            let local = crate::sample::socket_address(f[1]);
            let remote = crate::sample::socket_address(f[2]);
            let text = if remote.ends_with(":0") { format!("{proto} {local} (listening)") } else { format!("{proto} {local} → {remote}") };
            out.insert(inode, text);
        }
    }
    for l in read("/proc/net/unix").unwrap_or_default().lines().skip(1) {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() >= 7
            && let Ok(inode) = f[6].parse::<u64>()
        {
            let path = f.get(7).copied().unwrap_or("");
            out.insert(inode, if path.is_empty() { "Unix socket".into() } else { format!("Unix socket {path}") });
        }
    }
    out
}

/// The mappings in /proc/PID/maps, summed by what they map.
fn regions(maps: &str) -> Vec<Region> {
    let mut by: HashMap<String, Region> = HashMap::new();
    for l in maps.lines() {
        let f: Vec<&str> = l.splitn(6, ' ').collect();
        if f.len() < 5 {
            continue;
        }
        let Some((a, b)) = f[0].split_once('-') else { continue };
        let (Ok(a), Ok(b)) = (u64::from_str_radix(a, 16), u64::from_str_radix(b, 16)) else { continue };
        let path = f.get(5).map(|p| p.trim()).filter(|p| !p.is_empty()).unwrap_or("[anonymous]").to_owned();
        let r = by.entry(path.clone()).or_insert_with(|| Region { path, ..Default::default() });
        r.count += 1;
        r.size += b.saturating_sub(a);
        let perms = f[1].as_bytes();
        let mut have: Vec<char> = r.perms.chars().collect();
        for (i, ch) in ['r', 'w', 'x'].into_iter().enumerate() {
            if perms.get(i) == Some(&(ch as u8)) && !have.contains(&ch) {
                have.push(ch);
            }
        }
        have.sort_by_key(|c| ['r', 'w', 'x'].iter().position(|x| x == c));
        r.perms = have.into_iter().collect();
    }
    let mut v: Vec<Region> = by.into_values().collect();
    v.sort_by_key(|r| std::cmp::Reverse(r.size));
    v
}

/// The cores `pid` may run on.
pub fn affinity(pid: i32) -> Option<Vec<usize>> {
    // SAFETY: a zeroed cpu_set_t is an empty set.
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    // SAFETY: the set is ours and its size is given.
    if unsafe { libc::sched_getaffinity(pid, std::mem::size_of::<libc::cpu_set_t>(), &mut set) } != 0 {
        return None;
    }
    // SAFETY: CPU_ISSET reads the set within its size.
    Some((0..libc::CPU_SETSIZE as usize).filter(|&i| unsafe { libc::CPU_ISSET(i, &set) }).collect())
}

fn policy(pid: i32) -> String {
    // SAFETY: sched_getscheduler takes a pid and has no memory effects.
    match unsafe { libc::sched_getscheduler(pid) } {
        libc::SCHED_OTHER => "Normal",
        libc::SCHED_BATCH => "Batch",
        libc::SCHED_IDLE => "Idle",
        libc::SCHED_FIFO => "Real-time (first in, first out)",
        libc::SCHED_RR => "Real-time (round-robin)",
        -1 => "",
        _ => "Other",
    }
    .into()
}

/// Everything about `pid` this process may see.
pub fn read_details(pid: i32) -> Result<Details, String> {
    let dir = Path::new("/proc").join(pid.to_string());
    let status = read(dir.join("status")).ok_or_else(|| "The process has quit.".to_owned())?;
    let mut d = Details {
        pid,
        exe: link(dir.join("exe")),
        cwd: link(dir.join("cwd")),
        command: read(dir.join("cmdline")).unwrap_or_default().split('\0').filter(|a| !a.is_empty()).map(str::to_owned).collect(),
        cgroup: read(dir.join("cgroup")).unwrap_or_default().lines().find_map(|l| l.strip_prefix("0::")).unwrap_or("").to_owned(),
        status: status.lines().filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned())).collect(),
        limits: limits(&read(dir.join("limits")).unwrap_or_default()),
        affinity: affinity(pid),
        oom_score: read(dir.join("oom_score")).and_then(|v| v.trim().parse().ok()),
        oom_adjust: read(dir.join("oom_score_adj")).and_then(|v| v.trim().parse().ok()),
        policy: policy(pid),
        ..Default::default()
    };
    d.memory = read(dir.join("smaps_rollup")).map(|t| {
        t.lines()
            .skip(1)
            .filter_map(|l| {
                let (k, v) = l.split_once(':')?;
                Some((k.trim().to_owned(), v.split_whitespace().next()?.parse::<u64>().ok()? * 1024))
            })
            .collect()
    });
    d.io = read(dir.join("io")).map(|t| t.lines().filter_map(|l| l.split_once(':')).filter_map(|(k, v)| Some((k.trim().to_owned(), v.trim().parse().ok()?))).collect());
    for e in std::fs::read_dir(dir.join("task")).into_iter().flatten().flatten() {
        let Some(tid) = e.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else { continue };
        let stat = read(e.path().join("stat")).unwrap_or_default();
        let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else { continue };
        let f: Vec<&str> = stat[close + 1..].split_whitespace().collect();
        if f.len() < 37 {
            continue;
        }
        let num = |i: usize| f[i].parse::<u64>().unwrap_or(0);
        d.threads.push(Thread {
            tid,
            name: stat[open + 1..close].to_owned(),
            state: f[0].chars().next().unwrap_or('?'),
            ticks: num(11) + num(12),
            nice: f[16].parse().unwrap_or(0),
            core: f[36].parse().unwrap_or(-1),
        });
    }
    d.threads.sort_by_key(|t| t.tid);
    if let Ok(rd) = std::fs::read_dir(dir.join("fd")) {
        let socks = sockets();
        let mut files: Vec<OpenFile> = rd
            .flatten()
            .filter_map(|e| {
                let fd: i32 = e.file_name().to_str()?.parse().ok()?;
                let target = link(e.path())?;
                let flags = read(dir.join("fdinfo").join(fd.to_string()))
                    .and_then(|t| t.lines().find_map(|l| l.strip_prefix("flags:")).and_then(|v| u32::from_str_radix(v.trim(), 8).ok()))
                    .unwrap_or(0);
                let mode = match flags & 3 {
                    0 => "r",
                    1 => "w",
                    _ => "rw",
                };
                let (kind, target) = file_kind(&target, &socks);
                Some(OpenFile { fd, kind, target, mode: mode.into() })
            })
            .collect();
        files.sort_by_key(|f| f.fd);
        d.files = Some(files);
    }
    d.regions = read(dir.join("maps")).filter(|m| !m.is_empty() || d.exe.is_some()).map(|m| regions(&m));
    d.environment = std::fs::read(dir.join("environ")).ok().map(|b| {
        let mut env: Vec<(String, String)> = String::from_utf8_lossy(&b)
            .split('\0')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        env.sort();
        env
    });
    // Namespaces that differ from this process's own (a container, a
    // sandbox).
    let ns = |p: &Path, kind: &str| link(p.join("ns").join(kind));
    let me = Path::new("/proc/self");
    for kind in ["mnt", "net", "pid", "user", "uts", "ipc", "cgroup"] {
        if let (Some(theirs), Some(mine)) = (ns(&dir, kind), ns(me, kind))
            && theirs != mine
        {
            d.namespaces.push((kind.into(), theirs));
        }
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_itself() {
        let d = read_details(std::process::id() as i32).unwrap();
        assert!(d.exe.is_some() && d.cwd.is_some());
        assert!(!d.threads.is_empty());
        assert!(d.files.as_ref().is_some_and(|f| !f.is_empty()));
        assert!(d.environment.is_some());
        assert!(d.limits.iter().any(|l| l.resource.as_deref() == Some("nofile")));
        assert!(d.affinity.as_ref().is_some_and(|a| !a.is_empty()));
        assert_eq!(d.policy, "Normal");
        assert!(read_details(i32::MAX).is_err());
    }

    #[test]
    fn regions_sum() {
        let maps = "aaaa0000-aaaa1000 r--p 00000000 00:1f 1 /usr/bin/x\naaaa1000-aaaa3000 r-xp 00001000 00:1f 1 /usr/bin/x\nffff0000-ffff1000 rw-p 00000000 00:00 0 \n";
        let r = regions(maps);
        assert_eq!(r[0].path, "/usr/bin/x");
        assert_eq!((r[0].count, r[0].size, r[0].perms.as_str()), (2, 0x3000, "rx"));
        assert_eq!(r[1].path, "[anonymous]");
        assert_eq!(r[1].perms, "rw");
    }

    #[test]
    fn limits_parse() {
        let text = "Limit                     Soft Limit           Hard Limit           Units     \nMax open files            1024                 524288               files     \n";
        let l = limits(text);
        assert_eq!(l[0].soft, "1024");
        assert_eq!(l[0].hard, "524288");
        assert_eq!(l[0].resource.as_deref(), Some("nofile"));
        assert_eq!(l[0].units, "files");
    }
}
