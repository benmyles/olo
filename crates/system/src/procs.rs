//! Each process and what it uses, read from /proc (Activity Monitor's
//! list, and Deskbar's widgets' tips: the top five while one's tip is
//! up): its CPU and memory, disk reads and writes, TCP traffic (its
//! sockets' bytes from netlink, `netdiag`, by who has each open) and each
//! GPU's use (its DRM descriptors' fdinfo, NVIDIA's library while it's
//! awake). Counters come back with each look, to make rates of at the
//! next.

use std::{collections::HashMap, path::Path, time::Instant};

use serde::{Deserialize, Serialize};

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn num(s: &str) -> u64 {
    s.trim().parse().unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub name: String,
    pub user: String,
    pub uid: u32,
    /// Of one core (so up to 100 × cores).
    pub cpu: f64,
    pub memory: u64,
    pub threads: u32,
    pub state: char,
    pub nice: i32,
    /// CPU time used so far (seconds).
    pub cpu_time: f64,
    /// When it started (seconds since boot).
    pub started: f64,
    /// Bytes read and written each second, when they can be read (its
    /// own user's processes, everyone's with full access).
    pub read: Option<f64>,
    pub written: Option<f64>,
    /// Bytes its TCP sockets sent and received each second, when its open
    /// files can be seen (as `read`).
    #[serde(default)]
    pub sent: Option<f64>,
    #[serde(default)]
    pub received: Option<f64>,
    /// Each GPU's (in `System::gpus`' order): None, not using it; Some(%)
    /// of it busy for this process (0 where the driver doesn't say).
    #[serde(default)]
    pub gpus: Vec<Option<f64>>,
    /// Its command line, arguments spaced.
    pub command: String,
}

/// A program using a GPU, from its DRM file descriptors' fdinfo (and
/// NVIDIA's library for NVIDIA's).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GpuClient {
    pub pid: i32,
    pub name: String,
    /// Which GPU (in `System::gpus`' order).
    #[serde(default)]
    pub gpu: usize,
    /// Memory it has on the GPU (resident), in bytes.
    pub memory: u64,
    /// GPU time used each second, as a share of one engine, where the
    /// driver reports engine time.
    pub busy: Option<f64>,
}

pub fn users() -> HashMap<u32, String> {
    read("/etc/passwd")
        .lines()
        .filter_map(|l| {
            let mut f = l.split(':');
            let name = f.next()?;
            let uid = f.nth(1)?.parse().ok()?;
            Some((uid, name.to_owned()))
        })
        .collect()
}

/// What `processes` needs to turn counters into rates.
pub struct Before<'a> {
    pub ticks: &'a HashMap<i32, u64>,
    pub io: &'a HashMap<i32, (u64, u64)>,
    /// Each one's disk counters read (`Proc::read`, `written`): a second
    /// file a process, so only when they're wanted.
    pub with_io: bool,
}

/// What doesn't change in a process's life (its name, command line and
/// owner), kept by pid with when it started, so a list of hundreds reads
/// only their stat and io each time.
#[derive(Default)]
pub struct Known {
    by_pid: HashMap<i32, (u64, String, String, u32)>,
}

/// Processes, and their counters now: CPU ticks, and bytes read and
/// written, by pid.
pub type Counted = (Vec<Proc>, HashMap<i32, u64>, HashMap<i32, (u64, u64)>);

/// Every process: its counters now, and how it looked.
pub fn processes(before: Before, elapsed: f64, users: &HashMap<u32, String>, page: u64, hz: f64, known: &mut Known) -> Counted {
    let mut list = Vec::new();
    let mut ticks = HashMap::new();
    let mut io = HashMap::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else { continue };
        let dir = e.path();
        let stat = read(dir.join("stat"));
        // "pid (comm) state ..." — comm may hold spaces and parentheses.
        let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else { continue };
        let comm = &stat[open + 1..close];
        let f: Vec<&str> = stat[close + 1..].split_whitespace().collect();
        if f.len() < 22 {
            continue;
        }
        let state = f[0].chars().next().unwrap_or('?');
        let ppid = f[1].parse().unwrap_or(0);
        let t = num(f[11]) + num(f[12]);
        let nice = f[16].parse().unwrap_or(0);
        let threads = num(f[17]) as u32;
        let started = num(f[19]) as f64 / hz;
        let rss = num(f[21]) * page;
        ticks.insert(pid, t);
        let cpu = before.ticks.get(&pid).map_or(0.0, |&p| t.saturating_sub(p) as f64 / hz / elapsed * 100.0);
        // Its I/O, where it may be read.
        let counted = before.with_io.then(|| std::fs::read_to_string(dir.join("io")).ok()).flatten().map(|s| {
            let field = |k: &str| s.lines().find_map(|l| l.strip_prefix(k)).map_or(0, num);
            (field("read_bytes:"), field("write_bytes:"))
        });
        let (read_rate, write_rate) = match (counted, before.io.get(&pid)) {
            (Some((r, w)), Some(&(r0, w0))) => (Some(r.saturating_sub(r0) as f64 / elapsed), Some(w.saturating_sub(w0) as f64 / elapsed)),
            (Some(_), None) => (Some(0.0), Some(0.0)),
            _ => (None, None),
        };
        if let Some(c) = counted {
            io.insert(pid, c);
        }
        let start_ticks = num(f[19]);
        let (name, command, uid) = match known.by_pid.get(&pid) {
            // (A program that runs another keeps its pid but changes its
            // name: comm says so.)
            Some((start, name, command, uid)) if *start == start_ticks && name.starts_with(comm) => (name.clone(), command.clone(), *uid),
            _ => {
                let cmd = read(dir.join("cmdline"));
                let command = cmd.trim_end_matches('\0').replace('\0', " ");
                // comm is cut at 15 bytes; the program's file name isn't.
                let mut name = comm.to_owned();
                if name.len() >= 15
                    && let Some(arg0) = cmd.split('\0').next()
                    && let Some(base) = Path::new(arg0).file_name().map(|b| b.to_string_lossy().into_owned())
                    && base.starts_with(&name)
                {
                    name = base;
                }
                let uid = std::fs::metadata(&dir).map_or(0, |m| std::os::unix::fs::MetadataExt::uid(&m));
                known.by_pid.insert(pid, (start_ticks, name.clone(), command.clone(), uid));
                (name, command, uid)
            }
        };
        let user = users.get(&uid).cloned().unwrap_or_else(|| uid.to_string());
        list.push(Proc {
            pid,
            ppid,
            name,
            user,
            uid,
            cpu,
            memory: rss,
            threads,
            state,
            nice,
            cpu_time: t as f64 / hz,
            started,
            read: read_rate,
            written: write_rate,
            // (From its sockets, after: `traffic`.)
            sent: None,
            received: None,
            command,
            gpus: Vec::new(),
        });
    }
    known.by_pid.retain(|pid, _| ticks.contains_key(pid));
    (list, ticks, io)
}

/// Who has each GPU open: each DRM client's memory and, where the driver
/// says, engine time (`drm-engine-*`, nanoseconds; xe's `drm-cycles-*`
/// of `drm-total-cycles-*`), by the render or card node it opened (which
/// GPU); NVIDIA's from its library while it's awake. A program's
/// descriptors sharing a client count once.
pub fn gpu_clients(gpus: &[crate::gpu_load::Probe], pids: &[(i32, String)], before: &HashMap<(u64, u64), (u64, u64)>, elapsed: f64) -> (Vec<GpuClient>, HashMap<(u64, u64), (u64, u64)>) {
    use std::os::unix::fs::MetadataExt as _;
    // Each GPU's device numbers (its card and render nodes).
    let mut nodes: HashMap<u64, usize> = HashMap::new();
    for (i, g) in gpus.iter().enumerate() {
        for e in std::fs::read_dir(g.device.join("drm")).into_iter().flatten().flatten() {
            if let Ok(m) = std::fs::metadata(format!("/dev/dri/{}", e.file_name().to_string_lossy())) {
                nodes.insert(m.rdev(), i);
            }
        }
    }
    let mut out: Vec<GpuClient> = Vec::new();
    let mut busy = HashMap::new();
    for (pid, name) in pids {
        let Ok(rd) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
        let mut seen = std::collections::HashSet::new();
        let mut clients: HashMap<usize, GpuClient> = HashMap::new();
        for fd in rd.flatten() {
            let Ok(target) = std::fs::read_link(fd.path()) else { continue };
            if !target.starts_with("/dev/dri/") {
                continue;
            }
            let Some(&gpu) = std::fs::metadata(&target).ok().and_then(|m| nodes.get(&m.rdev())) else { continue };
            let info = read(format!("/proc/{pid}/fdinfo/{}", fd.file_name().to_string_lossy()));
            let field = |k: &str| info.lines().find_map(|l| l.strip_prefix(k)).map(str::trim);
            let Some(id) = field("drm-client-id:").and_then(|v| v.parse::<u64>().ok()) else { continue };
            if !seen.insert((gpu, id)) {
                continue;
            }
            let client = clients.entry(gpu).or_insert_with(|| GpuClient { pid: *pid, name: name.clone(), gpu, ..Default::default() });
            let (mut ns, mut cycles, mut total) = (None, 0u64, 0u64);
            for l in info.lines() {
                let Some((k, v)) = l.split_once(':') else { continue };
                let mut words = v.split_whitespace();
                let n = words.next().map_or(0, num);
                let scale = match words.next() {
                    Some("KiB") => 1024,
                    Some("MiB") => 1024 * 1024,
                    Some("GiB") => 1024 * 1024 * 1024,
                    _ => 1,
                };
                if k.starts_with("drm-resident-") {
                    client.memory += n * scale;
                } else if k.starts_with("drm-engine-") && !k.starts_with("drm-engine-capacity") {
                    *ns.get_or_insert(0u64) += n;
                } else if k.starts_with("drm-total-cycles-") {
                    total = total.max(n);
                } else if k.starts_with("drm-cycles-") {
                    cycles += n;
                }
            }
            let key = (gpu as u64, id);
            let now = match ns {
                Some(ns) => (ns, 0),
                None if total > 0 => (cycles, total),
                None => continue,
            };
            busy.insert(key, now);
            let share = match before.get(&key) {
                // Nanoseconds of engine time a second.
                Some(&(was, 0)) if now.1 == 0 => now.0.saturating_sub(was) as f64 / (elapsed * 1e9) * 100.0,
                // Cycles busy of the cycles there were.
                Some(&(was, was_total)) if now.1 > was_total => now.0.saturating_sub(was) as f64 / (now.1 - was_total) as f64 * 100.0,
                _ => 0.0,
            };
            client.busy = Some(client.busy.unwrap_or(0.0) + share);
        }
        out.extend(clients.into_values());
    }
    // NVIDIA's (its driver's descriptors say no engine time): its library's
    // users, while it's awake.
    for (i, g) in gpus.iter().enumerate() {
        let crate::gpu_load::Kind::Nvidia { bus } = &g.kind else { continue };
        // (Asleep, or idle when last asked: its users aren't asked for,
        // which would keep it awake.)
        if crate::gpu_load::asleep(&g.device) || !crate::gpu_load::nvidia_busy(bus) {
            continue;
        }
        let Some(d) = crate::gpu_load::nvml().and_then(|n| n.device_by_pci_bus_id(bus.as_str()).ok()) else { continue };
        let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |t| t.as_micros() as u64).saturating_sub((elapsed * 1e6) as u64);
        let util: HashMap<u32, u32> = d.process_utilization_stats(since).map(|v| v.into_iter().map(|u| (u.pid, u.sm_util.max(u.enc_util).max(u.dec_util))).collect()).unwrap_or_default();
        let procs = d.running_graphics_processes().unwrap_or_default().into_iter().chain(d.running_compute_processes().unwrap_or_default());
        for pr in procs {
            let pid = pr.pid as i32;
            let memory = match pr.used_gpu_memory {
                nvml_wrapper::enums::device::UsedGpuMemory::Used(b) => b,
                _ => 0,
            };
            let name = pids.iter().find(|(p, _)| *p == pid).map(|(_, n)| n.clone()).unwrap_or_default();
            match out.iter_mut().find(|c| c.pid == pid && c.gpu == i) {
                Some(c) => {
                    c.memory = c.memory.max(memory);
                    c.busy = Some(util.get(&pr.pid).copied().unwrap_or(0) as f64);
                }
                None => out.push(GpuClient { pid, name, gpu: i, memory, busy: Some(util.get(&pr.pid).copied().unwrap_or(0) as f64) }),
            }
        }
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.memory));
    (out, busy)
}

/// Every TCP and UDP connection, and who has it (where their open files
/// can be seen).
/// Socket inode → the process that has it open, and the processes whose
/// open files could be seen.
pub fn socket_owners(procs: &[Proc]) -> (HashMap<u64, i32>, std::collections::HashSet<i32>) {
    let (mut owner, mut seen) = (HashMap::new(), std::collections::HashSet::new());
    for p in procs {
        let Ok(rd) = std::fs::read_dir(format!("/proc/{}/fd", p.pid)) else { continue };
        seen.insert(p.pid);
        for fd in rd.flatten() {
            if let Ok(target) = std::fs::read_link(fd.path())
                && let Some(inode) = target.to_str().and_then(|t| t.strip_prefix("socket:[")).and_then(|t| t.strip_suffix(']')).and_then(|t| t.parse().ok())
            {
                owner.insert(inode, p.pid);
            }
        }
    }
    (owner, seen)
}

/// Each process's TCP bytes sent and received each second, from its
/// sockets' counters now and at the last look (`before`): a socket new
/// since counts in full, one gone counts no more. Those whose files can't
/// be seen get none.
pub fn traffic(
    procs: &mut [Proc],
    sockets: &[crate::netdiag::Socket],
    owner: &HashMap<u64, i32>,
    seen: &std::collections::HashSet<i32>,
    before: Option<&HashMap<u64, (u64, u64)>>,
    elapsed: f64,
) -> HashMap<u64, (u64, u64)> {
    let mut by_pid: HashMap<i32, (u64, u64)> = HashMap::new();
    let mut now = HashMap::new();
    for s in sockets.iter().filter(|s| s.tcp) {
        now.insert(s.inode, (s.sent, s.received));
        let (Some(pid), Some(before)) = (owner.get(&s.inode), before) else { continue };
        let (was_sent, was_received) = before.get(&s.inode).copied().unwrap_or((0, 0));
        let e = by_pid.entry(*pid).or_default();
        e.0 += s.sent.saturating_sub(was_sent);
        e.1 += s.received.saturating_sub(was_received);
    }
    let per = elapsed.max(1e-3);
    for p in procs.iter_mut() {
        let known = before.is_some() && seen.contains(&p.pid);
        let (sent, received) = by_pid.get(&p.pid).copied().unwrap_or_default();
        p.sent = known.then(|| sent as f64 / per);
        p.received = known.then(|| received as f64 / per);
    }
    now
}

/// The least time between reading every process's open files for who
/// has each socket (`Sampler`).
const OWNERS_EVERY: std::time::Duration = std::time::Duration::from_secs(3);

/// What a look at every process ranks them by (Deskbar's widgets' tips).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rank {
    /// Of one core, %.
    Cpu,
    /// Resident, bytes.
    Memory,
    /// Read and written, bytes a second.
    Disk,
    /// TCP sent and received, bytes a second.
    Network,
    /// Busy, % of the GPU at this PCI address (None: the one the
    /// computer started on).
    Gpu(Option<String>),
}

/// Looks at every process, each one's rates since the look before (a
/// first look's are nothing yet: `top` says so). What it reads follows
/// `Rank`: network and GPU use read every process's open files, so
/// they're read only for those.
pub struct Sampler {
    users: HashMap<u32, String>,
    page: u64,
    hz: f64,
    known: Known,
    last: Option<Instant>,
    ticks: HashMap<i32, u64>,
    io: HashMap<i32, (u64, u64)>,
    sockets: Option<HashMap<u64, (u64, u64)>>,
    /// Who has each socket, and when every process's open files were last
    /// read for it.
    owners: HashMap<u64, i32>,
    owners_at: Option<Instant>,
    gpus: Option<Vec<crate::gpu_load::Probe>>,
    gpu_busy: HashMap<(u64, u64), (u64, u64)>,
}

impl Default for Sampler {
    fn default() -> Self {
        Sampler::new()
    }
}

impl Sampler {
    pub fn new() -> Sampler {
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
        // SAFETY: as above.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
        Sampler {
            users: users(),
            page,
            hz,
            known: Known::default(),
            last: None,
            ticks: HashMap::new(),
            io: HashMap::new(),
            sockets: None,
            owners: HashMap::new(),
            owners_at: None,
            gpus: None,
            gpu_busy: HashMap::new(),
        }
    }

    /// The `n` processes using most by `rank` now, most first: each one's
    /// name and its use (in `Rank`'s unit), those using nothing left out.
    /// None until there's a look before this one to measure from (memory
    /// needs none).
    pub fn top(&mut self, rank: &Rank, n: usize) -> Option<Vec<(String, f64)>> {
        let now = Instant::now();
        let elapsed = self.last.map_or(1.0, |t| now.duration_since(t).as_secs_f64().max(0.001));
        let first = self.last.is_none();
        let with_io = *rank == Rank::Disk;
        let (mut list, ticks, io) = processes(Before { ticks: &self.ticks, io: &self.io, with_io }, elapsed, &self.users, self.page, self.hz, &mut self.known);
        (self.last, self.ticks, self.io) = (Some(now), ticks, io);
        let uses: Vec<(String, f64)> = match rank {
            Rank::Cpu => list.iter().map(|p| (p.name.clone(), p.cpu)).collect(),
            Rank::Memory => list.iter().map(|p| (p.name.clone(), p.memory as f64)).collect(),
            Rank::Disk => list.iter().map(|p| (p.name.clone(), p.read.unwrap_or(0.0) + p.written.unwrap_or(0.0))).collect(),
            Rank::Network => {
                let sockets = crate::netdiag::sockets().ok()?;
                // Who has each socket, read again only when one nobody is
                // known to have moves bytes (a socket keeps its owner), and
                // then at most every few seconds: a browser opens sockets
                // all the time, and reading every process's open files is
                // most of a look (2.3% of a core a look a second, 0.8%
                // without it).
                let before = self.sockets.as_ref();
                let unknown = sockets.iter().any(|s| s.tcp && s.inode != 0 && !self.owners.contains_key(&s.inode) && before.is_some_and(|b| b.get(&s.inode) != Some(&(s.sent, s.received))));
                if self.owners_at.is_none_or(|t| unknown && t.elapsed() >= OWNERS_EVERY) {
                    self.owners = socket_owners(&list).0;
                    self.owners_at = Some(now);
                }
                // (Whose open files can be seen: this user's, everyone's as
                // root.)
                // SAFETY: geteuid has no preconditions.
                let me = unsafe { libc::geteuid() };
                let seen: std::collections::HashSet<i32> = list.iter().filter(|p| me == 0 || p.uid == me).map(|p| p.pid).collect();
                let first_sockets = self.sockets.is_none();
                self.sockets = Some(traffic(&mut list, &sockets, &self.owners, &seen, before, elapsed));
                if first_sockets {
                    return None;
                }
                list.iter().map(|p| (p.name.clone(), p.sent.unwrap_or(0.0) + p.received.unwrap_or(0.0))).collect()
            }
            Rank::Gpu(pci) => {
                let gpus = self.gpus.get_or_insert_with(crate::gpu_load::find);
                let pci_of = |g: &crate::gpu_load::Probe| g.device.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let which = match pci {
                    Some(pci) => gpus.iter().position(|g| pci_of(g) == *pci)?,
                    None => 0,
                };
                let pids: Vec<(i32, String)> = list.iter().filter(|p| p.memory > 0).map(|p| (p.pid, p.name.clone())).collect();
                let (clients, busy) = gpu_clients(gpus, &pids, &self.gpu_busy, elapsed);
                let first_gpu = self.gpu_busy.is_empty();
                self.gpu_busy = busy;
                if first_gpu {
                    return None;
                }
                clients.into_iter().filter(|c| c.gpu == which).map(|c| (c.name, c.busy.unwrap_or(0.0))).collect()
            }
        };
        if first && *rank != Rank::Memory {
            return None;
        }
        let mut uses: Vec<(String, f64)> = uses.into_iter().filter(|(_, v)| v.is_finite() && *v > 0.0).collect();
        uses.sort_by(|a, b| b.1.total_cmp(&a.1));
        uses.truncate(n);
        Some(uses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This test's own process is among them, a look at memory needs no
    /// look before it, and a second look at the CPU measures from the
    /// first.
    #[test]
    fn the_top_processes() {
        let mut s = Sampler::new();
        let memory = s.top(&Rank::Memory, 5).unwrap();
        assert!(!memory.is_empty() && memory.len() <= 5);
        assert!(memory.windows(2).all(|w| w[0].1 >= w[1].1));
        assert!(s.top(&Rank::Cpu, 5).is_some(), "measured from the look before");
        let mut fresh = Sampler::new();
        assert!(fresh.top(&Rank::Cpu, 5).is_none(), "nothing to measure from");
        // Spin a little: this process uses some CPU.
        let t = Instant::now();
        while t.elapsed().as_millis() < 120 {
            std::hint::black_box(0u64.wrapping_add(1));
        }
        let cpu = fresh.top(&Rank::Cpu, 50).unwrap();
        assert!(!cpu.is_empty(), "something used the CPU");
    }
}

#[cfg(test)]
mod timing {
    use super::*;

    /// What each part of a look costs here (`--ignored --nocapture`).
    #[test]
    #[ignore]
    fn a_looks_cost() {
        let t = Instant::now();
        let (list, ..) = processes(Before { ticks: &HashMap::new(), io: &HashMap::new(), with_io: true }, 1.0, &users(), 4096, 100.0, &mut Known::default());
        eprintln!("processes, first: {:?} ({} of them)", t.elapsed(), list.len());
        let mut known = Known::default();
        let _ = processes(Before { ticks: &HashMap::new(), io: &HashMap::new(), with_io: true }, 1.0, &users(), 4096, 100.0, &mut known);
        let t = Instant::now();
        let _ = processes(Before { ticks: &HashMap::new(), io: &HashMap::new(), with_io: true }, 1.0, &users(), 4096, 100.0, &mut known);
        eprintln!("processes, known: {:?}", t.elapsed());
        let t = Instant::now();
        let sockets = crate::netdiag::sockets().unwrap();
        eprintln!("sockets: {:?} ({} of them)", t.elapsed(), sockets.len());
        let t = Instant::now();
        let (owners, _) = socket_owners(&list);
        eprintln!("owners: {:?} ({} sockets owned)", t.elapsed(), owners.len());
        let t = Instant::now();
        let _ = users();
        eprintln!("users: {:?}", t.elapsed());
    }
}
