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

/// The program that started `pid`, if it isn't one of Olo's (nor the
/// session's service manager, nor init, which adopt what's started
/// detached): a coding agent running an MCP server of ours, a browser
/// its messaging host, a shell a command (`olo-sudo` running someone's
/// script, an agent's `olo-screen wait`). Such a process isn't the
/// session's: the session didn't start it and won't start it again, it
/// ends with what it's doing, and stopping it breaks what that program
/// had open with it (an agent's tool was gone for the rest of its
/// session). So an upgrade leaves it on the version it started as
/// (olo-session's `check_stale`), and Olo Doctor doesn't offer to quit
/// it (its `stale`): one rule for both, here.
pub fn started_by_another(pid: i32) -> Option<String> {
    started_by_another_in(Path::new("/"), pid)
}

/// [`started_by_another`], with `/proc` under `root` (a test's own).
pub fn started_by_another_in(root: &Path, pid: i32) -> Option<String> {
    let proc = root.join("proc");
    let stat = std::fs::read_to_string(proc.join(pid.to_string()).join("stat")).ok()?;
    let ppid: i32 = stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()?;
    if ppid <= 1 {
        return None;
    }
    let parent = proc.join(ppid.to_string());
    let comm = std::fs::read_to_string(parent.join("comm")).ok()?.trim().to_owned();
    let exe = std::fs::read_link(parent.join("exe")).ok()?;
    let name = exe.file_name()?.to_str()?.trim_end_matches(" (deleted)");
    let ours = name.starts_with("olo-") || name.starts_with("tabula-") || name == "app_server";
    (!ours && comm != "systemd").then_some(comm)
}

/// A process's `stat` or `io` into `buf` (kept from file to file), and
/// how long it is: one read, where `read_to_string` starts with a probe
/// of 32 bytes and grows, five or six reads for a `stat`. (procfs gives
/// such a file's one record whole to a read with room for it: a read
/// short of the room is all of it.) None if it can't be opened or read
/// (another user's `io`). (The supervisor reads its looks so too.)
pub fn read_record(path: &Path, buf: &mut Vec<u8>) -> Option<usize> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut len = 0;
    loop {
        if buf.len() < len + 1024 {
            buf.resize(len + 4096, 0);
        }
        match f.read(&mut buf[len..]) {
            Ok(0) => return Some(len),
            Ok(n) => {
                len += n;
                if len < buf.len() {
                    return Some(len);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
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
    /// Memory it has on the GPU (resident), in bytes: its own and, but
    /// for the display server, what it shares ([`DISPLAY_SERVER`]).
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
/// owner, and whether its io can be read), kept by pid with when it
/// started, so a list of hundreds reads only their stat and io each time,
/// and only the io it can.
#[derive(Default)]
pub struct Known {
    by_pid: HashMap<i32, Seen>,
}

struct Seen {
    start: u64,
    name: String,
    command: String,
    uid: u32,
    /// Its io couldn't be read (another user's process, or one that
    /// can't be looked into): not tried again each look, ~400 of them
    /// here.
    io_hidden: bool,
}

/// Processes, and their counters now: CPU ticks, and bytes read and
/// written, by pid.
pub type Counted = (Vec<Proc>, HashMap<i32, u64>, HashMap<i32, (u64, u64)>);

/// Every process: its counters now, and how it looked.
pub fn processes(before: Before, elapsed: f64, users: &HashMap<u32, String>, page: u64, hz: f64, known: &mut Known) -> Counted {
    let mut list = Vec::new();
    let mut ticks = HashMap::new();
    let mut io = HashMap::new();
    let (mut stat_buf, mut io_buf) = (Vec::new(), Vec::new());
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else { continue };
        let dir = e.path();
        let Some(n) = read_record(&dir.join("stat"), &mut stat_buf) else { continue };
        // (A name needn't be UTF-8.)
        let stat = String::from_utf8_lossy(&stat_buf[..n]);
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
        let start_ticks = num(f[19]);
        // As it was when last seen: the same process (a program that
        // runs another keeps its pid but changes its name: comm says so).
        let kept = known.by_pid.get_mut(&pid).filter(|k| k.start == start_ticks && k.name.starts_with(comm));
        // Its I/O, where it may be read.
        let io_tried = before.with_io && !kept.as_ref().is_some_and(|k| k.io_hidden);
        let counted = io_tried.then(|| read_record(&dir.join("io"), &mut io_buf)).flatten().map(|n| {
            let s = std::str::from_utf8(&io_buf[..n]).unwrap_or_default();
            let field = |k: &str| s.lines().find_map(|l| l.strip_prefix(k)).map_or(0, num);
            (field("read_bytes:"), field("write_bytes:"))
        });
        let io_hidden = io_tried && counted.is_none();
        let (read_rate, write_rate) = match (counted, before.io.get(&pid)) {
            (Some((r, w)), Some(&(r0, w0))) => (Some(r.saturating_sub(r0) as f64 / elapsed), Some(w.saturating_sub(w0) as f64 / elapsed)),
            (Some(_), None) => (Some(0.0), Some(0.0)),
            _ => (None, None),
        };
        if let Some(c) = counted {
            io.insert(pid, c);
        }
        let (name, command, uid) = match kept {
            Some(k) => {
                k.io_hidden |= io_hidden;
                (k.name.clone(), k.command.clone(), k.uid)
            }
            None => {
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
                known.by_pid.insert(pid, Seen { start: start_ticks, name: name.clone(), command: command.clone(), uid, io_hidden });
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

/// Olo's display server, by its process's name. What it shares on the
/// GPU isn't its own: every window's buffers, which it holds only to
/// show them (through its render node and again through its display
/// node), counted for the app that draws them. (Ben, 2026-11-16, of
/// Activity's 6.3 GB beside it: "is there a leak or something?"; its own
/// were the displays' frames, 0.35 GB.)
pub const DISPLAY_SERVER: &str = "olo-app-server";

/// What a DRM client's fdinfo says it has on the GPU, in bytes: what's
/// resident (`drm-resident-*`, every kind of memory), and how much of
/// what it holds another client holds too (`drm-shared-*`).
fn drm_memory(info: &str) -> (u64, u64) {
    let (mut resident, mut shared) = (0u64, 0u64);
    for l in info.lines() {
        let Some((k, v)) = l.split_once(':') else { continue };
        let to = if k.starts_with("drm-resident-") {
            &mut resident
        } else if k.starts_with("drm-shared-") {
            &mut shared
        } else {
            continue;
        };
        let mut words = v.split_whitespace();
        let n = words.next().map_or(0, num);
        *to += n * match words.next() {
            Some("KiB") => 1024,
            Some("MiB") => 1024 * 1024,
            Some("GiB") => 1024 * 1024 * 1024,
            _ => 1,
        };
    }
    (resident, shared)
}

/// Who has each GPU open: each DRM client's memory and, where the driver
/// says, engine time (`drm-engine-*`, nanoseconds; xe's `drm-cycles-*`
/// of `drm-total-cycles-*`), by the render or card node it opened (which
/// GPU); NVIDIA's from its library while it's awake. A program's
/// descriptors sharing a client count once, and the display server's
/// memory is its own alone ([`DISPLAY_SERVER`]).
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
            let (resident, shared) = drm_memory(&info);
            client.memory += if name == DISPLAY_SERVER { resident.saturating_sub(shared) } else { resident };
            let (mut ns, mut cycles, mut total) = (None, 0u64, 0u64);
            for l in info.lines() {
                let Some((k, v)) = l.split_once(':') else { continue };
                let n = v.split_whitespace().next().map_or(0, num);
                if k.starts_with("drm-engine-") && !k.starts_with("drm-engine-capacity") {
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
        if crate::gpu_load::asleep(&g.device) || crate::gpu_load::failed(&g.device) || !crate::gpu_load::nvidia_busy(bus) {
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

    /// A DRM client's memory, as xe's fdinfo gives it: app_server's render
    /// node held 2.9 GiB resident, all but 316 KiB of it windows' buffers
    /// shared with their apps (Ben's session, 2026-11-16).
    #[test]
    fn a_drm_clients_memory() {
        let info = "pos:\t0\nflags:\t02100002\ndrm-driver:\txe\ndrm-client-id:\t20914\ndrm-pdev:\t0000:00:02.0\n\
                    drm-total-system:\t276 KiB\ndrm-shared-system:\t0\ndrm-active-system:\t0\ndrm-resident-system:\t0\ndrm-purgeable-system:\t0\n\
                    drm-total-gtt:\t3025052 KiB\ndrm-shared-gtt:\t3024736 KiB\ndrm-active-gtt:\t0\ndrm-resident-gtt:\t3025052 KiB\n\
                    drm-total-stolen:\t0\ndrm-shared-stolen:\t0\ndrm-resident-stolen:\t0\ndrm-total-cycles-rcs:\t1452840405258\n";
        let (resident, shared) = drm_memory(info);
        assert_eq!((resident, shared), (3025052 * 1024, 3024736 * 1024));
        assert_eq!(resident - shared, 316 * 1024);
        assert_eq!(drm_memory("drm-resident-vram0:\t2 MiB\ndrm-resident-system:\t4096\n"), (2 * 1024 * 1024 + 4096, 0));
    }

    /// Its own process's disk counters are read at every look; another
    /// user's, which can't be, are tried once.
    #[test]
    fn io_read_where_it_can_be() {
        let mut known = Known::default();
        let me = std::process::id() as i32;
        for look in 0..2 {
            let (list, _, io) = processes(Before { ticks: &HashMap::new(), io: &HashMap::new(), with_io: true }, 1.0, &users(), 4096, 100.0, &mut known);
            let mine = list.iter().find(|p| p.pid == me).expect("this test's process");
            assert!(mine.read.is_some() && io.contains_key(&me), "look {look}");
            assert!(!known.by_pid[&me].io_hidden);
        }
        // (pid 1 is root's: hidden unless the test runs as root.)
        // SAFETY: getuid has no preconditions.
        if unsafe { libc::getuid() } != 0 {
            assert!(known.by_pid.get(&1).is_none_or(|k| k.io_hidden));
        }
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
