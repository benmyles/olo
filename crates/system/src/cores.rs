//! A core kept for Olo's most important threads (Ben, 2026-11-15: "an
//! option -- enabled by default -- to reserve a cpu core just for olo's
//! most important threads"): app_server's loop (every input event, every
//! frame) and its input thread run there alone, and nothing else of the
//! person's does: builds, agents, browsers and services run on the other
//! cores. Realtime already puts those threads first wherever they wake;
//! the kept core is what they find free, at its full clock and with its
//! caches theirs, whatever else is running.
//!
//! Which one ([`kept`]): the fastest (ACPI CPPC's `highest_perf`, else the
//! highest clock), all of its hardware threads, never CPU 0 (the boot
//! processor, with the most of the kernel's own work), and only where at
//! least four are online. On the ThinkPad: CPU 2, the favoured P-core.
//!
//! How, with no privilege for the person's own programs: each thread's
//! CPU affinity, which a program's children inherit ([`sweep`], once
//! when it's turned on, of every thread of the person's: from then on
//! every program starts from a parent kept off it). Threads already
//! realtime are left as they are (PipeWire's, Olo's other realtime
//! threads: what they are for is never waiting), as is an affinity a
//! program chose for itself, unless it's the kept core alone: that's
//! inherited from a thread homed there. The homed threads' own children
//! would inherit the kept core alone (the kernel copies a thread's
//! affinity to the threads and programs it starts), so every program
//! app_server starts begins where the person's run ([`start_off`]), and
//! its other threads are swept off it as it goes. Before, Palette,
//! started by app_server, started Terminal, whose shells and every build
//! and agent in them ran on the kept core alone: one processor for all
//! of it, the desktop's own (Ben, 2026-11-15: "wait why is the shell
//! limited to one [cpu]???? that should never happen!"). The system's services (root's) through
//! systemd's own `CPUAffinity=` ([`SYSTEM_CONF`]: what systemd starts
//! inherits it) and a sweep of what runs already, which Olo Doctor does
//! as root. Affinity, not a cgroup's cpuset: Intel's low-power daemon
//! (intel_lpmd) moves the slices' cpusets itself, and the two combine
//! (a thread runs where both allow).

use std::path::{Path, PathBuf};

/// Where the processors are described.
pub const CPU_DIR: &str = "/sys/devices/system/cpu";
/// systemd's drop-in for its own affinity, and every service's: written
/// by Olo Doctor (`olo-doctor --keep-core`).
pub const SYSTEM_CONF: &str = "/etc/systemd/system.conf.d/50-olo-kept-core.conf";
/// At least this many processors online for one to be kept.
pub const FEWEST: usize = 4;

/// A list of processors as the kernel and systemd write it ("0-1,3-15").
pub fn parse_list(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for part in text.trim().split([',', ' ']).filter(|p| !p.is_empty()) {
        match part.split_once('-') {
            Some((a, b)) => {
                if let (Ok(a), Ok(b)) = (a.parse::<usize>(), b.parse::<usize>())
                    && a <= b
                {
                    out.extend(a..=b);
                }
            }
            None => out.extend(part.parse::<usize>().ok()),
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// `cpus` (sorted) as the kernel writes a list: runs as ranges.
pub fn format_list(cpus: &[usize]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < cpus.len() {
        let mut j = i;
        while j + 1 < cpus.len() && cpus[j + 1] == cpus[j] + 1 {
            j += 1;
        }
        parts.push(if i == j { cpus[i].to_string() } else { format!("{}-{}", cpus[i], cpus[j]) });
        i = j + 1;
    }
    parts.join(",")
}

/// The processors online, from `dir` (`CPU_DIR`, or a test's tree).
pub fn online_in(dir: &Path) -> Vec<usize> {
    std::fs::read_to_string(dir.join("online")).map(|t| parse_list(&t)).unwrap_or_default()
}

pub fn online() -> Vec<usize> {
    online_in(Path::new(CPU_DIR))
}

fn read_u64(path: PathBuf) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// How fast processor `cpu` can go: CPPC's highest performance (what
/// Intel's favoured cores are told apart by), else its highest clock.
fn speed(dir: &Path, cpu: usize) -> (u64, u64) {
    let at = dir.join(format!("cpu{cpu}"));
    (read_u64(at.join("acpi_cppc/highest_perf")).unwrap_or(0), read_u64(at.join("cpufreq/cpuinfo_max_freq")).unwrap_or(0))
}

/// The processors kept, from `dir`: the fastest core's hardware threads
/// (ties: the highest numbered), never one with CPU 0; None with fewer
/// than [`FEWEST`] online. `OLO_KEPT_CORE` (a list) chooses instead.
pub fn kept_in(dir: &Path) -> Option<Vec<usize>> {
    let online = online_in(dir);
    if online.len() < FEWEST {
        return None;
    }
    if let Ok(chosen) = std::env::var("OLO_KEPT_CORE") {
        let chosen: Vec<usize> = parse_list(&chosen).into_iter().filter(|c| online.contains(c) && *c != 0).collect();
        return (!chosen.is_empty() && chosen.len() < online.len()).then_some(chosen);
    }
    let mut best: Option<((u64, u64), Vec<usize>)> = None;
    for &cpu in &online {
        let siblings = std::fs::read_to_string(dir.join(format!("cpu{cpu}/topology/thread_siblings_list"))).map(|t| parse_list(&t)).unwrap_or_else(|_| vec![cpu]);
        let siblings: Vec<usize> = siblings.into_iter().filter(|s| online.contains(s)).collect();
        // (Each core once: from its first thread.)
        if siblings.first() != Some(&cpu) || siblings.contains(&0) {
            continue;
        }
        let fastest = siblings.iter().map(|&s| speed(dir, s)).max().unwrap_or_default();
        if best.as_ref().is_none_or(|(b, _)| fastest >= *b) {
            best = Some((fastest, siblings));
        }
    }
    best.map(|(_, cpus)| cpus).filter(|c| c.len() < online.len())
}

pub fn kept() -> Option<Vec<usize>> {
    kept_in(Path::new(CPU_DIR))
}

/// The processors online but `kept`.
pub fn others(online: &[usize], kept: &[usize]) -> Vec<usize> {
    online.iter().copied().filter(|c| !kept.contains(c)).collect()
}

/// Where thread `tid` may run (any process's, as anyone may read it).
pub fn affinity(tid: i32) -> Option<Vec<usize>> {
    // SAFETY: a zeroed cpu_set_t is an empty set; sched_getaffinity
    // writes at most its size.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        if libc::sched_getaffinity(tid, size_of::<libc::cpu_set_t>(), &mut set) != 0 {
            return None;
        }
        Some((0..libc::CPU_SETSIZE as usize).filter(|&c| libc::CPU_ISSET(c, &set)).collect())
    }
}

/// Thread `tid` to run on `cpus` alone (one of the caller's user's; any,
/// as root). The kernel keeps it as asked and runs it where that and its
/// cgroup's cpuset both allow.
pub fn set_affinity(tid: i32, cpus: &[usize]) -> std::io::Result<()> {
    // SAFETY: as in `affinity`; sched_setaffinity reads the set.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for &c in cpus.iter().filter(|&&c| c < libc::CPU_SETSIZE as usize) {
            libc::CPU_SET(c, &mut set);
        }
        if libc::sched_setaffinity(tid, size_of::<libc::cpu_set_t>(), &set) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// What a sweep does to each thread.
#[derive(Clone, Copy, Debug)]
pub enum Sweep<'a> {
    /// Keep it off these processors: taken out of where it may run (an
    /// affinity of its program's own included, unless it was these
    /// alone); one that can run only where its cgroup allows (`plain`,
    /// all of those: under intel_lpmd's low-power mode the kept core
    /// isn't among them, and no affinity can be told from none) is given
    /// every processor online but these.
    KeepOff { kept: &'a [usize], plain: &'a [usize], online: &'a [usize] },
    /// Let it back on: one kept off them (running everywhere else, or
    /// where its cgroup allows) is given every processor online.
    GiveBack { kept: &'a [usize], plain: &'a [usize], online: &'a [usize] },
}

impl Sweep<'_> {
    /// Thread `tid`'s affinity, `now`, as this sweep wants it; None:
    /// left as it is.
    pub fn wanted(&self, now: &[usize]) -> Option<Vec<usize>> {
        match *self {
            Sweep::KeepOff { kept, plain, online } => {
                if now.iter().any(|c| kept.contains(c)) {
                    let rest = others(now, kept);
                    // (The kept core alone: inherited from a thread homed
                    // there, so given where the person's programs run.)
                    Some(if rest.is_empty() { off(kept, plain, online) } else { rest })
                } else if now == plain && plain != others(online, kept) {
                    Some(others(online, kept))
                } else {
                    None
                }
            }
            Sweep::GiveBack { kept, plain, online } => (now == others(online, kept) || (now == plain && now != online)).then(|| online.to_vec()),
        }
    }
}

/// Where the person's programs run while `kept` is kept: where their
/// cgroup lets them (`plain`) but the kept core, else every processor
/// online but it.
pub fn off(kept: &[usize], plain: &[usize], online: &[usize]) -> Vec<usize> {
    let rest = others(plain, kept);
    if rest.is_empty() { others(online, kept) } else { rest }
}

/// `cmd`'s program begins on `cpus` (where the person's programs run),
/// whatever the thread starting it may run on: the kernel copies a
/// thread's affinity to what it starts, and app_server's loop is homed
/// on the kept core alone.
pub fn start_off(cmd: &mut std::process::Command, cpus: &[usize]) {
    use std::os::unix::process::CommandExt as _;
    // SAFETY: a zeroed cpu_set_t is an empty set, filled here, before the
    // fork; the child only makes the one system call with it, which is
    // async-signal-safe.
    let set = unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for &c in cpus.iter().filter(|&&c| c < libc::CPU_SETSIZE as usize) {
            libc::CPU_SET(c, &mut set);
        }
        set
    };
    if cpus.is_empty() {
        return;
    }
    // SAFETY: as above.
    unsafe {
        cmd.pre_exec(move || {
            // (Where it may not, it starts where it would have.)
            libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &set);
            Ok(())
        });
    }
}

/// The processes of the caller's user (their /proc directories its).
pub fn mine() -> Vec<i32> {
    use std::os::unix::fs::MetadataExt as _;
    // SAFETY: getuid has no preconditions.
    let me = unsafe { libc::getuid() };
    let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
    dir.flatten().filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok().filter(|_| e.metadata().is_ok_and(|m| m.uid() == me))).collect()
}

/// Whether process `pid` is one of the kernel's own threads (PF_KTHREAD:
/// their affinity is the kernel's).
pub fn kernel_thread(pid: i32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { return true };
    // (After the name, which may hold spaces and parentheses: its flags
    // are the seventh field on.)
    let Some(rest) = stat.rsplit_once(')').map(|(_, r)| r) else { return true };
    rest.split_whitespace().nth(6).and_then(|f| f.parse::<u64>().ok()).is_none_or(|flags| flags & 0x0020_0000 != 0)
}

/// What a sweep did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Swept {
    pub processes: usize,
    pub threads: usize,
    /// Threads it couldn't change (they went, or aren't ours).
    pub failed: usize,
}

/// Every thread of `pids` as `how` wants it, but threads `spare` (the
/// kept core's own) and those already realtime. Kernel threads never.
pub fn sweep(pids: &[i32], how: Sweep, spare: &[i32]) -> Swept {
    let mut swept = Swept::default();
    for &pid in pids {
        if kernel_thread(pid) {
            continue;
        }
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else { continue };
        let mut any = false;
        for tid in tasks.flatten().filter_map(|t| t.file_name().to_str()?.parse::<i32>().ok()) {
            if spare.contains(&tid) || crate::realtime::is_realtime(tid as u32) {
                continue;
            }
            let Some(now) = affinity(tid) else { continue };
            if let Some(want) = how.wanted(&now) {
                match set_affinity(tid, &want) {
                    Ok(()) => {
                        swept.threads += 1;
                        any = true;
                    }
                    Err(_) => swept.failed += 1,
                }
            }
        }
        swept.processes += any as usize;
    }
    swept
}

/// Where a thread may run with no affinity of its own: its cgroup's
/// cpuset as it is now (intel_lpmd changes it), read through the calling
/// thread's own (`plain` for [`Sweep`]); every processor online without
/// one.
pub fn plain() -> Vec<usize> {
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("0::").map(str::to_owned)));
    if let Some(path) = cgroup {
        let mut at = PathBuf::from("/sys/fs/cgroup").join(path.trim_start_matches('/'));
        loop {
            if let Ok(t) = std::fs::read_to_string(at.join("cpuset.cpus.effective")) {
                let cpus = parse_list(&t);
                if !cpus.is_empty() {
                    return cpus;
                }
            }
            if !at.pop() || at == Path::new("/sys/fs") {
                break;
            }
        }
    }
    online()
}

/// The systemd drop-in that keeps the system's services off `kept`
/// (`online` the rest), as Olo Doctor writes it.
pub fn system_conf(online: &[usize], kept: &[usize]) -> String {
    format!(
        "# Olo: processor {} kept for Olo's most important threads (Preferences ▸ Performance ▸ Scheduler; olo-doctor --keep-core).\n# What systemd starts runs on the others.\n[Manager]\nCPUAffinity={}\n",
        format_list(kept),
        format_list(&others(online, kept)).replace(',', " ")
    )
}

/// The processors the system's drop-in keeps off, if it's there.
pub fn system_kept() -> Option<Vec<usize>> {
    let text = std::fs::read_to_string(SYSTEM_CONF).ok()?;
    let line = text.lines().find_map(|l| l.strip_prefix("CPUAffinity="))?;
    let allowed = parse_list(line);
    let online = online();
    Some(others(&online, &allowed)).filter(|k| !k.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_read_and_write_as_the_kernel_does() {
        assert_eq!(parse_list("0-1,3-15\n"), [0, 1, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
        assert_eq!(parse_list("0 2 4-5"), [0, 2, 4, 5]);
        assert_eq!(format_list(&[0, 1, 3, 4, 5, 9]), "0-1,3-5,9");
        assert_eq!(format_list(&[2]), "2");
        assert_eq!(format_list(&[]), "");
    }

    fn put(path: PathBuf, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// The ThinkPad's: four P-cores (CPU 2 favoured), eight E-cores, four
    /// low-power ones; no SMT.
    #[test]
    fn the_fastest_core_is_kept_never_cpu_0() {
        let dir = PathBuf::from(format!("/var/tmp/olo-cores-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        put(dir.join("online"), "0-15\n");
        for c in 0..16 {
            let perf = match c {
                2 => 53,
                0..=3 => 52,
                4..=11 => 37,
                _ => 33,
            };
            put(dir.join(format!("cpu{c}/acpi_cppc/highest_perf")), &format!("{perf}\n"));
            put(dir.join(format!("cpu{c}/topology/thread_siblings_list")), &format!("{c}\n"));
        }
        assert_eq!(kept_in(&dir), Some(vec![2]));
        // CPU 0 the fastest: the next.
        put(dir.join("cpu0/acpi_cppc/highest_perf"), "60\n");
        put(dir.join("cpu2/acpi_cppc/highest_perf"), "52\n");
        assert_eq!(kept_in(&dir), Some(vec![3]), "ties: the highest numbered");
        // SMT: a core's two threads together.
        put(dir.join("cpu3/topology/thread_siblings_list"), "3,7\n");
        put(dir.join("cpu7/topology/thread_siblings_list"), "3,7\n");
        assert_eq!(kept_in(&dir), Some(vec![3, 7]));
        // Too few to keep one.
        put(dir.join("online"), "0-2\n");
        assert_eq!(kept_in(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A program started from a thread pinned to one processor (as
    /// app_server's loop is, on the kept core) begins where it's told,
    /// not on that one processor (it did: Terminal's shells and every
    /// build in them ran on the kept core alone, 2026-11-15).
    #[test]
    fn a_program_started_from_a_pinned_thread_begins_off_it() {
        let mine = affinity(0).unwrap();
        if mine.len() < 2 {
            eprintln!("one processor here: nothing to tell apart");
            return;
        }
        let (pin, rest) = (mine[0], mine[1..].to_vec());
        let child = |start: bool| {
            let rest = rest.clone();
            std::thread::spawn(move || {
                set_affinity(0, &[pin]).unwrap();
                let mut cmd = std::process::Command::new("/bin/sh");
                cmd.args(["-c", "while read -r k v; do [ \"$k\" = Cpus_allowed_list: ] && echo \"$v\"; done < /proc/self/status"]);
                if start {
                    start_off(&mut cmd, &rest);
                }
                String::from_utf8(cmd.output().unwrap().stdout).unwrap().trim().to_owned()
            })
            .join()
            .unwrap()
        };
        assert_eq!(child(false), pin.to_string(), "inherited, without it");
        assert_eq!(parse_list(&child(true)), rest);
    }

    #[test]
    fn a_sweep_keeps_off_and_gives_back_only_what_it_should() {
        let online: Vec<usize> = (0..16).collect();
        let (kept, rest) = (vec![2], others(&online, &[2]));
        let off = Sweep::KeepOff { kept: &kept, plain: &online, online: &online };
        assert_eq!(off.wanted(&online), Some(rest.clone()), "no affinity of its own");
        assert_eq!(off.wanted(&rest), None, "already off");
        assert_eq!(off.wanted(&[0, 1, 2, 3]), Some(vec![0, 1, 3]), "its own, less the kept core");
        assert_eq!(off.wanted(&[2]), Some(rest.clone()), "the kept core alone, inherited: moved off");
        assert_eq!(off.wanted(&[5]), None);
        // Under intel_lpmd's low-power mode: the cgroup allows 12–15.
        let lp: Vec<usize> = (12..16).collect();
        let off_lp = Sweep::KeepOff { kept: &kept, plain: &lp, online: &online };
        assert_eq!(off_lp.wanted(&lp), Some(rest.clone()));
        let back = Sweep::GiveBack { kept: &kept, plain: &online, online: &online };
        assert_eq!(back.wanted(&rest), Some(online.clone()));
        assert_eq!(back.wanted(&online), None);
        assert_eq!(back.wanted(&[0, 1, 3]), None, "its program's own");
    }

    /// On a child of this test's (never anyone else's processes).
    #[test]
    fn a_child_is_kept_off_and_given_back() {
        let online = online();
        let Some(kept) = kept().or_else(|| (online.len() >= 2).then(|| vec![*online.last().unwrap()])) else { return };
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id() as i32;
        // (Every processor, whatever this test was started on: a shell
        // kept off the kept core starts its children off it already.)
        set_affinity(pid, &online).unwrap();
        let plain = online.clone();
        let swept = sweep(&[pid], Sweep::KeepOff { kept: &kept, plain: &plain, online: &online }, &[]);
        assert_eq!(swept, Swept { processes: 1, threads: 1, failed: 0 });
        assert!(!affinity(pid).unwrap().iter().any(|c| kept.contains(c)));
        let back = sweep(&[pid], Sweep::GiveBack { kept: &kept, plain: &plain, online: &online }, &[]);
        assert_eq!(back.threads, 1);
        assert_eq!(affinity(pid).unwrap(), plain);
        let _ = child.kill();
        let _ = child.wait();
        assert!(kernel_thread(2), "kthreadd");
        assert!(!kernel_thread(std::process::id() as i32));
    }

    #[test]
    fn the_system_drop_in_says_the_others() {
        let online: Vec<usize> = (0..16).collect();
        let conf = system_conf(&online, &[2]);
        assert!(conf.contains("CPUAffinity=0-1 3-15\n"), "{conf}");
    }
}
