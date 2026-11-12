//! What Activity Monitor measures, read from /proc and /sys on a worker
//! thread at the update interval (never on the UI thread): CPU (total,
//! by kind of time, per core, each core's frequency and governor), memory
//! and swap (with zram and every /proc/meminfo field), pressure stall
//! information, each disk's and network interface's traffic, every GPU
//! (gpus.rs: Asahi's, amdgpu's, Intel's xe, NVIDIA's), the battery, every
//! temperature, power and fan sensor (the Mac's SMC included), the
//! kernel's tunables, and processes (with their parents, priorities and
//! I/O) and network connections while someone looks at them.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use olo_system::procs::{Before, Known, gpu_clients, processes, socket_owners, traffic};
pub use olo_system::procs::{GpuClient, Proc, users};
use olo_theme::Hue;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Unit {
    Percent,
    Bytes,
    /// Bytes per second.
    Rate,
    Count,
    PerSecond,
    Watts,
    Celsius,
    Rpm,
    Megahertz,
    /// A load average (runnable tasks).
    Load,
}

impl Unit {
    pub fn format(self, v: f64) -> String {
        match self {
            Unit::Percent => format!("{v:.1}%"),
            Unit::Bytes => olo_kit::files::format_size(v.max(0.0) as u64),
            // Under a kilobyte reads as part of one, not in bytes.
            Unit::Rate if v <= 0.0 => "0 KB/s".into(),
            Unit::Rate if v < 1024.0 => "<1 KB/s".into(),
            Unit::Rate => format!("{}/s", olo_kit::files::format_size(v.max(0.0) as u64)),
            Unit::Count => format!("{v:.0}"),
            Unit::PerSecond => format!("{v:.0}/s"),
            Unit::Watts => format!("{v:.1} W"),
            Unit::Celsius => format!("{v:.1} °C"),
            Unit::Rpm => format!("{v:.0} rpm"),
            Unit::Megahertz => format!("{v:.0} MHz"),
            Unit::Load => format!("{v:.2}"),
        }
    }
}

/// What a source is about (the sections and the graph menus group by it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Group {
    Main,
    Cpus,
    Frequencies,
    Disks,
    Network,
    Gpu,
    Pressure,
    Sensors,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub key: String,
    pub name: String,
    pub unit: Unit,
    /// Its line and swatch: one of the look's hues.
    pub color: Hue,
    /// A fixed top of the scale; None scales to what the graph shows.
    pub max: Option<f64>,
    pub group: Group,
}

/// A network connection (TCP or UDP, v4 or v6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    pub proto: String,
    pub local: String,
    pub remote: String,
    pub state: String,
    /// Who has it open, if that can be seen.
    pub pid: Option<i32>,
    pub process: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Zram {
    pub name: String,
    pub algorithm: String,
    /// What is stored, as it was and as it is compressed, and the memory
    /// it takes in all.
    pub original: u64,
    pub compressed: u64,
    pub used: u64,
    pub disk_size: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Pressure {
    pub resource: String,
    /// Some tasks stalled: the share of the last 10, 60, 300 seconds.
    pub some: [f64; 3],
    /// All of them at once (not for the CPU).
    pub full: Option<[f64; 3]>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Disk {
    pub name: String,
    pub model: String,
    pub size: u64,
    pub rotational: bool,
    pub removable: bool,
    /// Requests each second, and how long one takes (ms).
    pub iops: f64,
    pub latency_ms: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Interface {
    pub name: String,
    pub up: bool,
    pub mac: String,
    pub mtu: u64,
    pub addresses: Vec<String>,
    pub received: u64,
    pub sent: u64,
    pub errors: u64,
    pub drops: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Battery {
    pub status: String,
    pub capacity: f64,
    pub health: String,
    pub cycles: u64,
    /// Watt-hours now, when full, and when new.
    pub energy: f64,
    pub full: f64,
    pub design: f64,
    pub watts: f64,
    pub volts: f64,
    pub model: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Gpu {
    /// Its series' names start so: "gpu" (the first), "gpu1", …
    #[serde(default)]
    pub prefix: String,
    pub driver: String,
    pub name: String,
    pub memory_used: u64,
    pub memory_limit: u64,
    pub jobs: u64,
    /// Asleep (runtime-suspended): nothing is using it, and it isn't asked.
    #[serde(default)]
    pub asleep: bool,
}

/// A cpufreq policy: cores that change frequency together (a cluster:
/// Apple's efficiency and performance cores).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Policy {
    pub id: usize,
    pub cores: Vec<usize>,
    pub governor: String,
    pub governors: Vec<String>,
    /// MHz.
    pub min: f64,
    pub max: f64,
    pub now: f64,
}

/// A CPU idle state (cpuidle): how much of all cores' time was spent in
/// it since the last sample.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IdleState {
    pub name: String,
    pub what: String,
    /// Of all cores' time (%).
    pub share: f64,
    /// How long waking from it takes (µs).
    pub latency_us: u64,
}

/// An NVMe controller and its health log (None: it needs full access).
pub type Health = (String, Option<crate::smart::Health>);

/// A mounted file system.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Volume {
    pub mount: String,
    pub device: String,
    pub kind: String,
    /// Bytes in all and free; None for a network or FUSE file system
    /// (asking those can hang).
    pub size: Option<u64>,
    pub free: Option<u64>,
    pub read_only: bool,
}

/// Who keeps the fans (Olo's `olo-fans`, if it's here) and how, and
/// whether the processor was held back for heat.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Cooling {
    /// The fan profile chosen (`/etc/olo/fans`: "cool", or "auto", the
    /// firmware's own curve); empty where there's no such choice.
    pub profile: String,
    /// The level the service holds the fans at ("auto": left to the
    /// firmware; "3" to "7"; "full-speed"), and what asked for it;
    /// empty: Cool isn't chosen, or isn't in force (`amiss`).
    pub level: String,
    pub why: String,
    /// Why a chosen Cool isn't what the fans are doing, for people ("the
    /// fan service isn't running", "the fans can't be set until the
    /// computer's next start"); empty: it is.
    #[serde(default)]
    pub amiss: String,
    /// How often the processor's package was throttled for heat since
    /// the computer started, and for how long in all (None: the
    /// processor doesn't say).
    pub throttled: Option<u64>,
    pub throttled_ms: Option<u64>,
}

/// The fans' keeper as `olo_system::fans` finds it (the one reader of
/// the service's files: what Preferences ▸ Power and Olo Doctor go by).
/// A service that's running and has said nothing holds "auto": this
/// read the missing line as "isn't running" once, of a service that was
/// (Ben, 2026-11-12).
fn fan_state(state: Option<&olo_system::fans::State>) -> (String, String, String, String) {
    let Some(state) = state.filter(|s| s.installed) else { return Default::default() };
    let (level, why) = state.level().map(|(l, w)| (l.to_owned(), w.to_owned())).unwrap_or_default();
    (state.profile.word().to_owned(), level, why, state.amiss().map(|a| a.words().to_owned()).unwrap_or_default())
}

fn cooling() -> Cooling {
    let (profile, level, why, amiss) = fan_state(olo_system::fans::look(&olo_system::fans::Places::here()).as_ref());
    let count = |name: &str| read(format!("/sys/devices/system/cpu/cpu0/thermal_throttle/{name}")).trim().parse().ok();
    Cooling { profile, level, why, amiss, throttled: count("package_throttle_count"), throttled_ms: count("package_throttle_total_time_ms") }
}

/// What isn't a series: the machine, and the state of things now.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct System {
    /// The fans' keeper and the processor's throttling.
    #[serde(default)]
    pub cooling: Cooling,
    pub host: String,
    pub kernel: String,
    pub model: String,
    pub uptime: f64,
    /// When it started (seconds since 1970).
    pub boot: u64,
    pub load: [f64; 3],
    pub cores: usize,
    pub policies: Vec<Policy>,
    /// What the CPU is ("Apple M2 Pro", or the device tree's names).
    pub cpu_model: String,
    pub memory: Vec<(String, u64)>,
    pub zram: Vec<Zram>,
    pub pressure: Vec<Pressure>,
    pub disks: Vec<Disk>,
    pub interfaces: Vec<Interface>,
    pub battery: Option<Battery>,
    /// Every GPU, the one the computer started on first.
    #[serde(default)]
    pub gpus: Vec<Gpu>,
    /// The kernel's tunables Olo shows (sysctl name, value).
    pub tunables: Vec<(String, String)>,
    pub open_files: (u64, u64),
    pub volumes: Vec<Volume>,
    pub idle_states: Vec<IdleState>,
    /// Each NVMe controller's health log (None: it needs full access).
    pub health: Vec<Health>,
    /// Who uses the GPU (while the GPU section shows).
    pub gpu_clients: Option<Vec<GpuClient>>,
    /// Whether this was measured with full access (as root).
    pub privileged: bool,
}

#[derive(Serialize, Deserialize)]
pub struct Sample {
    /// When (seconds since the sampler started).
    pub at: f64,
    /// One value per source, in order (NaN until it can be known, e.g. a
    /// rate's first sample).
    #[serde(with = "nan_as_null")]
    pub values: Vec<f64>,
    pub system: Box<System>,
    pub processes: Option<Vec<Proc>>,
    pub connections: Option<Vec<Connection>>,
}

/// NaN (not known yet) as JSON's null, which serde_json can't read back
/// into an f64.
mod nan_as_null {
    use serde::{Deserialize, Deserializer, Serializer, ser::SerializeSeq};

    pub fn serialize<S: Serializer>(v: &[f64], s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(v.len()))?;
        for x in v {
            seq.serialize_element(&x.is_finite().then_some(*x))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<f64>, D::Error> {
        Ok(Vec::<Option<f64>>::deserialize(d)?.into_iter().map(|x| x.unwrap_or(f64::NAN)).collect())
    }
}

/// What the sampler is asked for, shared with its thread (or sent to the
/// privileged helper's).
#[derive(Clone, Default)]
pub struct Control {
    pub interval_ms: Arc<AtomicU64>,
    pub want_processes: Arc<AtomicBool>,
    pub want_connections: Arc<AtomicBool>,
    pub want_gpu_clients: Arc<AtomicBool>,
    /// Stop sampling here (the helper measures instead).
    pub paused: Arc<AtomicBool>,
}

/// [`Control`]'s settings as a value (what the helper is sent).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Wants {
    pub interval_ms: u64,
    pub processes: bool,
    pub connections: bool,
    pub gpu_clients: bool,
}

impl Control {
    pub fn new(interval_ms: u64) -> Control {
        let c = Control::default();
        c.interval_ms.store(interval_ms, Ordering::Relaxed);
        c
    }

    pub fn wants(&self) -> Wants {
        Wants {
            interval_ms: self.interval_ms.load(Ordering::Relaxed),
            processes: self.want_processes.load(Ordering::Relaxed),
            connections: self.want_connections.load(Ordering::Relaxed),
            gpu_clients: self.want_gpu_clients.load(Ordering::Relaxed),
        }
    }

    pub fn set(&self, w: Wants) {
        self.interval_ms.store(w.interval_ms, Ordering::Relaxed);
        self.want_processes.store(w.processes, Ordering::Relaxed);
        self.want_connections.store(w.connections, Ordering::Relaxed);
        self.want_gpu_clients.store(w.gpu_clients, Ordering::Relaxed);
    }
}

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn num(s: &str) -> u64 {
    s.trim().parse().unwrap_or(0)
}

/// Colours for many sources of a kind (CPUs, sensors), in turn.
const SPREAD: [Hue; 10] = [Hue::Blue, Hue::Red, Hue::Green, Hue::Orange, Hue::Purple, Hue::Teal, Hue::Brown, Hue::Pink, Hue::Yellow, Hue::Gray];

/// The kernel tunables shown under System: (name, what it does).
pub const TUNABLES: &[(&str, &str)] = &[
    ("vm.swappiness", "How readily memory is swapped out (0–200)"),
    ("vm.vfs_cache_pressure", "How readily file metadata caches are freed"),
    ("vm.dirty_ratio", "Memory that may wait to be written before writers wait (%)"),
    ("vm.dirty_background_ratio", "Memory that may wait before writing starts (%)"),
    ("vm.overcommit_memory", "Memory overcommit (0 guess, 1 always, 2 never)"),
    ("vm.page-cluster", "Pages read from swap at once (2ⁿ)"),
    ("vm.min_free_kbytes", "Memory kept free (KB)"),
    ("kernel.sched_autogroup_enabled", "Group a session's tasks for fairness"),
    ("kernel.pid_max", "The highest process ID"),
    ("kernel.threads-max", "The most threads at once"),
    ("fs.file-max", "The most open files at once"),
    ("fs.inotify.max_user_watches", "Files a user may watch for changes"),
    ("net.core.somaxconn", "Connections waiting to be accepted, at most"),
    ("net.ipv4.tcp_congestion_control", "TCP congestion control"),
];

pub fn sysctl_path(name: &str) -> PathBuf {
    Path::new("/proc/sys").join(name.replace('.', "/"))
}

/// A sensor in /sys/class/hwmon.
struct Sensor {
    path: PathBuf,
    /// Multiply the raw reading by this.
    scale: f64,
}

/// Everything the machine can report, found once at start.
struct Probe {
    sources: Vec<Source>,
    cpus: usize,
    sensors: Vec<Sensor>,
    battery: Option<PathBuf>,
    freq: Vec<PathBuf>,
    disks: Vec<String>,
    interfaces: Vec<String>,
    gpus: Vec<crate::gpus::Probe>,
}

/// Network interfaces worth showing: not loopback or virtual bridges.
fn interfaces() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir("/sys/class/net")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "lo" && !n.starts_with("veth") && !n.starts_with("docker") && !n.starts_with("virbr"))
        .collect();
    v.sort();
    v
}

fn probe() -> Probe {
    let mut sources = Vec::new();
    let mut add = |key: &str, name: &str, unit: Unit, color: Hue, max: Option<f64>, group: Group| {
        sources.push(Source { key: key.into(), name: name.into(), unit, color, max, group });
    };
    let mem_total = mem_kb(&read("/proc/meminfo"), "MemTotal:") as f64 * 1024.0;
    let swap_total = mem_kb(&read("/proc/meminfo"), "SwapTotal:") as f64 * 1024.0;
    add("cpu", "CPU usage", Unit::Percent, Hue::Blue, Some(100.0), Group::Main);
    add("cpu-user", "User time", Unit::Percent, Hue::Green, Some(100.0), Group::Main);
    add("cpu-system", "System time", Unit::Percent, Hue::Red, Some(100.0), Group::Main);
    add("cpu-iowait", "Waiting for I/O", Unit::Percent, Hue::Orange, Some(100.0), Group::Main);
    add("memory", "Used memory", Unit::Bytes, Hue::Red, Some(mem_total), Group::Main);
    add("cached", "Cached memory", Unit::Bytes, Hue::Green, Some(mem_total), Group::Main);
    add("swap", "Swap space", Unit::Bytes, Hue::Orange, Some(swap_total.max(1.0)), Group::Main);
    add("net-rx", "Network receive", Unit::Rate, Hue::Teal, None, Group::Main);
    add("net-tx", "Network send", Unit::Rate, Hue::Purple, None, Group::Main);
    add("disk-read", "Disk read", Unit::Rate, Hue::Brown, None, Group::Main);
    add("disk-write", "Disk write", Unit::Rate, Hue::Pink, None, Group::Main);
    add("faults", "Page faults", Unit::PerSecond, Hue::Yellow, None, Group::Main);
    add("switches", "Context switches", Unit::PerSecond, Hue::Gray, None, Group::Main);
    add("interrupts", "Interrupts", Unit::PerSecond, Hue::Teal, None, Group::Main);
    add("processes", "Processes", Unit::Count, Hue::Red, None, Group::Main);
    add("threads", "Threads", Unit::Count, Hue::Blue, None, Group::Main);
    add("load", "Load average", Unit::Load, Hue::Purple, None, Group::Main);

    let freq: Vec<PathBuf> = (0..1024).map(|i| PathBuf::from(format!("/sys/devices/system/cpu/cpu{i}/cpufreq/scaling_cur_freq"))).take_while(|p| p.exists()).collect();
    if !freq.is_empty() {
        add("cpu-freq", "CPU frequency", Unit::Megahertz, Hue::Orange, None, Group::Main);
    }
    let battery = std::fs::read_dir("/sys/class/power_supply")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| read(p.join("type")).trim() == "Battery" && p.join("capacity").exists() && read(p.join("scope")).trim() != "Device");
    if battery.is_some() {
        add("battery", "Battery charge", Unit::Percent, Hue::Green, Some(100.0), Group::Main);
        add("battery-power", "Battery power", Unit::Watts, Hue::Yellow, None, Group::Main);
    }

    let cpus = read("/proc/stat").lines().filter(|l| l.starts_with("cpu") && !l.starts_with("cpu ")).count();
    for i in 0..cpus {
        add(&format!("cpu{i}"), &format!("CPU {}", i + 1), Unit::Percent, SPREAD[i % SPREAD.len()], Some(100.0), Group::Cpus);
    }
    for i in 0..freq.len() {
        add(&format!("freq{i}"), &format!("CPU {} frequency", i + 1), Unit::Megahertz, SPREAD[i % SPREAD.len()], None, Group::Frequencies);
    }
    for (res, name, hue) in [("cpu", "CPU pressure", Hue::Blue), ("memory", "Memory pressure", Hue::Red), ("io", "I/O pressure", Hue::Orange)] {
        if Path::new("/proc/pressure").join(res).exists() {
            add(&format!("psi-{res}"), name, Unit::Percent, hue, Some(100.0), Group::Pressure);
        }
    }
    for (res, name, hue) in [("memory", "Memory stalled (all)", Hue::Purple), ("io", "I/O stalled (all)", Hue::Brown)] {
        if Path::new("/proc/pressure").join(res).exists() {
            add(&format!("psi-{res}-full"), name, Unit::Percent, hue, Some(100.0), Group::Pressure);
        }
    }

    // Whole physical disks (not partitions, loop or RAM devices; zram is
    // in Memory).
    let mut disks: Vec<String> = std::fs::read_dir("/sys/block")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("device").exists())
        // Not a controller's own tiny namespaces (Apple's SSD has two).
        .filter(|e| num(&read(e.path().join("size"))) * 512 >= 1 << 30)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    disks.sort();
    for (i, d) in disks.iter().enumerate() {
        add(&format!("disk:{d}:read"), &format!("{d} read"), Unit::Rate, SPREAD[(2 * i) % SPREAD.len()], None, Group::Disks);
        add(&format!("disk:{d}:write"), &format!("{d} write"), Unit::Rate, SPREAD[(2 * i + 1) % SPREAD.len()], None, Group::Disks);
        add(&format!("disk:{d}:busy"), &format!("{d} busy"), Unit::Percent, SPREAD[(2 * i + 2) % SPREAD.len()], Some(100.0), Group::Disks);
    }
    let ifaces = interfaces();
    for (i, n) in ifaces.iter().enumerate() {
        add(&format!("net:{n}:rx"), &format!("{n} receive"), Unit::Rate, SPREAD[(2 * i + 5) % SPREAD.len()], None, Group::Network);
        add(&format!("net:{n}:tx"), &format!("{n} send"), Unit::Rate, SPREAD[(2 * i + 6) % SPREAD.len()], None, Group::Network);
    }
    // Every GPU (gpus.rs), the one the computer started on first: its
    // series named by its prefix ("gpu-busy", "gpu1-busy"), each kind's.
    let gpus = crate::gpus::find();
    let several = gpus.len() > 1;
    for g in &gpus {
        use crate::gpus::Kind;
        let (x, label) = (&g.prefix, |what: &str| if several { format!("{} {what}", g.name) } else { format!("GPU {what}") });
        add(&format!("{x}-busy"), &label("busy"), Unit::Percent, Hue::Purple, Some(100.0), Group::Gpu);
        match &g.kind {
            Kind::Agx => {
                add(&format!("{x}-jobs"), &label("jobs"), Unit::PerSecond, Hue::Teal, None, Group::Gpu);
                add(&format!("{x}-power"), &label("power"), Unit::Watts, Hue::Orange, None, Group::Gpu);
            }
            // (A GPU of several engines: "busy" is its busiest, and each
            // is a series of its own.)
            Kind::Xe { gts } => {
                if gts.len() > 1 {
                    add(&format!("{x}-render"), &label("render busy"), Unit::Percent, Hue::Purple, Some(100.0), Group::Gpu);
                    add(&format!("{x}-media"), &label("media busy"), Unit::Percent, Hue::Teal, Some(100.0), Group::Gpu);
                }
                add(&format!("{x}-freq"), &label("clock"), Unit::Megahertz, Hue::Blue, None, Group::Gpu);
            }
            Kind::Nvidia { .. } => {
                add(&format!("{x}-render"), &label("graphics busy"), Unit::Percent, Hue::Purple, Some(100.0), Group::Gpu);
                add(&format!("{x}-encode"), &label("encoder busy"), Unit::Percent, Hue::Orange, Some(100.0), Group::Gpu);
                add(&format!("{x}-decode"), &label("decoder busy"), Unit::Percent, Hue::Teal, Some(100.0), Group::Gpu);
                add(&format!("{x}-power"), &label("power"), Unit::Watts, Hue::Orange, None, Group::Gpu);
                add(&format!("{x}-temp"), &label("temperature"), Unit::Celsius, Hue::Red, None, Group::Gpu);
                add(&format!("{x}-freq"), &label("clock"), Unit::Megahertz, Hue::Blue, None, Group::Gpu);
            }
            Kind::Amd => {}
        }
        if matches!(g.kind, Kind::Agx | Kind::Nvidia { .. }) || g.device.join("mem_info_vram_used").exists() {
            add(&format!("{x}-memory"), &label("memory"), Unit::Bytes, Hue::Red, None, Group::Gpu);
        }
    }

    let mut sensors = Vec::new();
    let mut hwmons: Vec<PathBuf> = std::fs::read_dir("/sys/class/hwmon").into_iter().flatten().flatten().map(|e| e.path()).collect();
    hwmons.sort_by_key(|h| h.file_name().and_then(|n| n.to_str()?.strip_prefix("hwmon")?.parse::<u32>().ok()).unwrap_or(u32::MAX));
    let mut seen: HashMap<String, usize> = HashMap::new();
    for h in hwmons {
        let chip = read(h.join("name")).trim().to_owned();
        let mut inputs: Vec<PathBuf> =
            std::fs::read_dir(&h).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().ends_with("_input"))).collect();
        inputs.sort();
        for input in inputs {
            let file = input.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let stem = file.trim_end_matches("_input");
            let (unit, scale) = if stem.starts_with("temp") {
                (Unit::Celsius, 0.001)
            } else if stem.starts_with("power") {
                (Unit::Watts, 0.000_001)
            } else if stem.starts_with("fan") {
                (Unit::Rpm, 1.0)
            } else {
                continue;
            };
            let label = read(h.join(format!("{stem}_label"))).trim().to_owned();
            let mut name = sensor_name(&chip, stem, &label);
            // Several chips of a kind (speaker amps) report the same names.
            let n = seen.entry(name.clone()).or_default();
            *n += 1;
            if *n > 1 {
                name = format!("{name} {n}");
            }
            let key = format!("hwmon:{chip}:{}:{stem}", h.file_name().unwrap_or_default().to_string_lossy());
            let color = SPREAD[sensors.len() % SPREAD.len()];
            sources.push(Source { key, name, unit, color, max: None, group: Group::Sensors });
            sensors.push(Sensor { path: input, scale });
        }
    }
    Probe { sources, cpus, sensors, battery, freq, disks, interfaces: ifaces, gpus }
}

/// A sensor's name for people: the driver's label, or what the chip is.
fn sensor_name(chip: &str, stem: &str, label: &str) -> String {
    let kind = if stem.starts_with("temp") {
        "temperature"
    } else if stem.starts_with("power") {
        "power"
    } else {
        "fan"
    };
    match chip {
        "nvme" if label == "Composite" || label.is_empty() => "SSD temperature".into(),
        "nvme" => format!("SSD {label}"),
        "macsmc_battery" => format!("Battery {kind}"),
        "tas2764" | "tas2770" => format!("Speaker amp {kind}"),
        _ if !label.is_empty() => label.into(),
        _ => format!("{chip} {stem}"),
    }
}

fn mem_kb(meminfo: &str, key: &str) -> u64 {
    meminfo.lines().find_map(|l| l.strip_prefix(key)).and_then(|v| v.split_whitespace().next()).map_or(0, num)
}

/// Counters that become rates.
#[derive(Default, Clone)]
struct Counters {
    cpu: Vec<CpuTimes>,
    rx: u64,
    tx: u64,
    read: u64,
    written: u64,
    faults: u64,
    switches: u64,
    interrupts: u64,
    /// Per disk: sectors read, written, ms busy, requests, ms waiting.
    disks: HashMap<String, [u64; 5]>,
    /// Per interface: bytes in, out.
    ifaces: HashMap<String, (u64, u64)>,
    /// Per GPU (by prefix): Asahi's busy nanoseconds and jobs.
    gpu_busy_ns: HashMap<String, u64>,
    gpu_jobs: HashMap<String, u64>,
    /// Per xe GT: its idle milliseconds so far.
    gt_idle: HashMap<PathBuf, u64>,
    /// Per process: (utime + stime) ticks, and bytes read and written.
    ticks: HashMap<i32, u64>,
    io: HashMap<i32, (u64, u64)>,
    /// Per GPU client (by device and client id): engine nanoseconds, or
    /// engine cycles and the cycles there were (xe).
    gpu_busy: HashMap<(u64, u64), (u64, u64)>,
    /// Per TCP socket (by inode): bytes sent and received, for processes'
    /// network rates; None until a first look.
    sockets: Option<HashMap<u64, (u64, u64)>>,
    /// Per idle state (by its index): µs spent in it, all cores.
    idle: Vec<u64>,
}

/// A CPU line of /proc/stat: busy, total, and the kinds of time.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct CpuTimes {
    busy: u64,
    total: u64,
    user: u64,
    system: u64,
    iowait: u64,
}

/// (busy, total) jiffies for "cpu" and each "cpuN".
#[cfg(test)]
fn cpu_times(stat: &str) -> Vec<(u64, u64)> {
    cpu_lines(stat).into_iter().map(|c| (c.busy, c.total)).collect()
}

fn cpu_lines(stat: &str) -> Vec<CpuTimes> {
    stat.lines()
        .filter(|l| l.starts_with("cpu"))
        .map(|l| {
            let f: Vec<u64> = l.split_whitespace().skip(1).map(num).collect();
            let g = |i: usize| f.get(i).copied().unwrap_or(0);
            let total: u64 = f.iter().take(8).sum();
            let idle = g(3) + g(4);
            CpuTimes { busy: total - idle.min(total), total, user: g(0) + g(1), system: g(2) + g(5) + g(6), iowait: g(4) }
        })
        .collect()
}

/// `\040`-style escapes in /proc/self/mounts.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|d| (b'0'..=b'7').contains(d)) {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// File systems on devices and the network (not the kernel's own: proc,
/// sysfs, cgroups, tmpfs), one a device (Fedora mounts one btrfs at / and
/// /home: it is listed once, at both).
fn volumes(mounts: &str) -> Vec<Volume> {
    let network = ["nfs", "nfs4", "cifs", "smb3", "sshfs", "9p", "ceph", "glusterfs", "davfs"];
    let mut out: Vec<Volume> = Vec::new();
    for l in mounts.lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 4 {
            continue;
        }
        let (device, mount, kind) = (unescape(f[0]), unescape(f[1]), f[2]);
        let remote = network.contains(&kind) || kind.starts_with("fuse");
        if !device.starts_with("/dev/") && !remote {
            continue;
        }
        if device.starts_with("/dev/loop") || mount.starts_with("/var/lib/snapd") {
            continue;
        }
        if let Some(v) = out.iter_mut().find(|v| v.device == device) {
            v.mount = format!("{}, {mount}", v.mount);
            continue;
        }
        let (size, free) = if remote { (None, None) } else { statvfs(&mount).map_or((None, None), |(s, f)| (Some(s), Some(f))) };
        out.push(Volume { mount, device, kind: kind.into(), size, free, read_only: f[3].split(',').any(|o| o == "ro") });
    }
    out
}

/// A file system's size and what is free of it for users.
fn statvfs(path: &str) -> Option<(u64, u64)> {
    let c = std::ffi::CString::new(path).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: a NUL-terminated path and a statvfs to fill.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let frag = st.f_frsize as u64;
    Some((st.f_blocks as u64 * frag, st.f_bavail as u64 * frag))
}

/// "1.2.3.4:80" from /proc/net's hex for v4 or v6.
pub fn socket_address(hex: &str) -> String {
    let Some((addr, port)) = hex.split_once(':') else { return hex.into() };
    let port = u16::from_str_radix(port, 16).unwrap_or(0);
    let bytes: Vec<u8> = (0..addr.len() / 2).filter_map(|i| u8::from_str_radix(&addr[2 * i..2 * i + 2], 16).ok()).collect();
    match bytes.len() {
        4 => format!("{}.{}.{}.{}:{port}", bytes[3], bytes[2], bytes[1], bytes[0]),
        16 => {
            // Four little-endian words.
            let mut b = [0u8; 16];
            for w in 0..4 {
                for k in 0..4 {
                    b[w * 4 + k] = bytes[w * 4 + 3 - k];
                }
            }
            let ip = std::net::Ipv6Addr::from(b);
            match ip.to_ipv4_mapped() {
                Some(v4) => format!("{v4}:{port}"),
                None => format!("[{ip}]:{port}"),
            }
        }
        _ => hex.into(),
    }
}

fn tcp_state(code: &str) -> &'static str {
    match code {
        "01" => "Established",
        "02" => "SYN sent",
        "03" => "SYN received",
        "04" => "FIN wait 1",
        "05" => "FIN wait 2",
        "06" => "Time wait",
        "07" => "Closed",
        "08" => "Close wait",
        "09" => "Last ACK",
        "0A" => "Listening",
        "0B" => "Closing",
        _ => "",
    }
}

/// TCP's state by number (netlink's), as the list says it.
fn tcp_state_of(n: u8) -> &'static str {
    tcp_state(&format!("{n:02X}"))
}

fn endpoint((ip, port): (std::net::IpAddr, u16)) -> String {
    match ip {
        std::net::IpAddr::V6(v6) => format!("[{v6}]:{port}"),
        v4 => format!("{v4}:{port}"),
    }
}

/// Every connection from netlink's sockets (`netdiag::sockets`), with
/// who has it.
fn connections_of(sockets: &[crate::netdiag::Socket], owner: &HashMap<u64, i32>, procs: &[Proc]) -> Vec<Connection> {
    let names: HashMap<i32, &str> = procs.iter().map(|p| (p.pid, p.name.as_str())).collect();
    sockets
        .iter()
        .map(|s| {
            let pid = owner.get(&s.inode).copied();
            let state = if s.tcp {
                tcp_state_of(s.state)
            } else if s.state == 7 {
                ""
            } else {
                "Connected"
            };
            Connection {
                proto: if s.tcp { "TCP" } else { "UDP" }.into(),
                local: endpoint(s.local),
                remote: endpoint(s.remote),
                state: state.into(),
                pid,
                process: pid.and_then(|p| names.get(&p).map(|n| (*n).to_owned())).unwrap_or_default(),
            }
        })
        .collect()
}

/// Each interface's counters: from netlink, else /proc/net/dev.
fn links() -> Vec<crate::netdiag::Link> {
    if let Ok(l) = crate::netdiag::interfaces() {
        return l;
    }
    read("/proc/net/dev")
        .lines()
        .skip(2)
        .filter_map(|l| {
            let (iface, rest) = l.split_once(':')?;
            let f: Vec<u64> = rest.split_whitespace().map(num).collect();
            let g = |i: usize| f.get(i).copied().unwrap_or(0);
            Some(crate::netdiag::Link { name: iface.trim().to_owned(), received: g(0), sent: g(8), errors: g(2) + g(10), drops: g(3) + g(11) })
        })
        .collect()
}

/// Every connection read from /proc/net, if netlink can't be had.
fn connections(procs: &[Proc]) -> Vec<Connection> {
    let (owner, _) = socket_owners(procs);
    let names: HashMap<i32, &str> = procs.iter().map(|p| (p.pid, p.name.as_str())).collect();
    let mut out = Vec::new();
    for (file, proto) in [("tcp", "TCP"), ("tcp6", "TCP"), ("udp", "UDP"), ("udp6", "UDP")] {
        for l in read(format!("/proc/net/{file}")).lines().skip(1) {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 10 {
                continue;
            }
            let inode: u64 = f[9].parse().unwrap_or(0);
            let pid = owner.get(&inode).copied();
            let state = if proto == "UDP" { if f[3] == "07" { "" } else { "Connected" } } else { tcp_state(f[3]) };
            out.push(Connection {
                proto: proto.into(),
                local: socket_address(f[1]),
                remote: socket_address(f[2]),
                state: state.into(),
                pid,
                process: pid.and_then(|p| names.get(&p).map(|n| (*n).to_owned())).unwrap_or_default(),
            });
        }
    }
    out
}

/// Each interface's addresses (getifaddrs).
fn addresses() -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `head` with a list we free below.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return out;
    }
    let mut cur = head;
    while !cur.is_null() {
        // SAFETY: `cur` is a node of the list getifaddrs gave.
        let ifa = unsafe { &*cur };
        cur = ifa.ifa_next;
        if ifa.ifa_addr.is_null() || ifa.ifa_name.is_null() {
            continue;
        }
        // SAFETY: a NUL-terminated name from the kernel.
        let name = unsafe { std::ffi::CStr::from_ptr(ifa.ifa_name) }.to_string_lossy().into_owned();
        // SAFETY: the address's family says which struct it is.
        let family = unsafe { (*ifa.ifa_addr).sa_family } as i32;
        let text = match family {
            libc::AF_INET => {
                // SAFETY: an AF_INET address is a sockaddr_in.
                let a = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
                Some(std::net::Ipv4Addr::from(u32::from_be(a.sin_addr.s_addr)).to_string())
            }
            libc::AF_INET6 => {
                // SAFETY: an AF_INET6 address is a sockaddr_in6.
                let a = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in6) };
                Some(std::net::Ipv6Addr::from(a.sin6_addr.s6_addr).to_string())
            }
            _ => None,
        };
        if let Some(t) = text {
            out.entry(name).or_default().push(t);
        }
    }
    // SAFETY: the list getifaddrs made.
    unsafe { libc::freeifaddrs(head) };
    out
}

/// "some avg10=0.02 avg60=… total=…" lines of a pressure file.
fn pressure(text: &str) -> (Option<[f64; 3]>, Option<[f64; 3]>) {
    let line = |kind: &str| {
        let l = text.lines().find(|l| l.starts_with(kind))?;
        let v = |k: &str| l.split_whitespace().find_map(|f| f.strip_prefix(k)).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        Some([v("avg10="), v("avg60="), v("avg300=")])
    };
    (line("some"), line("full"))
}

fn policies() -> Vec<Policy> {
    let mut v: Vec<Policy> = std::fs::read_dir("/sys/devices/system/cpu/cpufreq")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().to_str()?.strip_prefix("policy")?.parse().ok()?;
            let d = e.path();
            let khz = |f: &str| num(&read(d.join(f))) as f64 / 1000.0;
            Some(Policy {
                id,
                cores: read(d.join("related_cpus")).split_whitespace().filter_map(|c| c.parse().ok()).collect(),
                governor: read(d.join("scaling_governor")).trim().into(),
                governors: read(d.join("scaling_available_governors")).split_whitespace().map(str::to_owned).collect(),
                min: khz("cpuinfo_min_freq"),
                max: khz("cpuinfo_max_freq"),
                now: khz("scaling_cur_freq"),
            })
        })
        .collect();
    v.sort_by_key(|p| p.id);
    v
}

/// Apple's chip, from the machine's name ("M2 Max" in "MacBook Pro
/// (16-inch, M2 Max, 2023)").
pub fn apple_chip(model: &str) -> Option<String> {
    let words: Vec<&str> = model.split([',', '(', ')']).map(str::trim).collect();
    words.into_iter().find(|w| w.len() >= 2 && w.starts_with('M') && w[1..].chars().next().is_some_and(|c| c.is_ascii_digit())).map(str::to_owned)
}

/// The CPU's name: /proc/cpuinfo's model name, else (Apple's) the chip
/// and its kinds of core, from the device tree ("Apple M2 Max: 4
/// Blizzard and 8 Avalanche cores").
fn cpu_model() -> String {
    let info = read("/proc/cpuinfo");
    if let Some(m) = info.lines().find_map(|l| l.strip_prefix("model name").and_then(|r| r.split_once(':')).map(|(_, v)| v.trim().to_owned())) {
        return m;
    }
    let mut kinds: Vec<(String, usize)> = Vec::new();
    let mut nodes: Vec<PathBuf> =
        std::fs::read_dir("/sys/firmware/devicetree/base/cpus").into_iter().flatten().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("cpu@")).map(|e| e.path()).collect();
    nodes.sort();
    for n in nodes {
        let c = read(n.join("compatible"));
        if let Some(first) = c.split('\0').next().filter(|s| !s.is_empty()) {
            let mut name = first.split_once(',').map_or(first, |(_, n)| n).to_owned();
            if let Some(f) = name.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            match kinds.iter_mut().find(|(k, _)| *k == name) {
                Some((_, count)) => *count += 1,
                None => kinds.push((name, 1)),
            }
        }
    }
    let cores = kinds.iter().map(|(k, n)| format!("{n} {k}")).collect::<Vec<_>>().join(" and ");
    match apple_chip(&machine_model()) {
        Some(chip) if !cores.is_empty() => format!("Apple {chip}: {cores} cores"),
        Some(chip) => format!("Apple {chip}"),
        None => cores,
    }
}

/// The machine's name, as the device tree or DMI says.
fn machine_model() -> String {
    let dt = read("/sys/firmware/devicetree/base/model").trim_end_matches('\0').trim().to_owned();
    if !dt.is_empty() {
        return dt;
    }
    let vendor = read("/sys/class/dmi/id/sys_vendor").trim().to_owned();
    let product = read("/sys/class/dmi/id/product_name").trim().to_owned();
    [vendor, product].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ")
}

/// What isn't a series, read now.
fn system(probe: &Probe, mi: &str, static_disks: &[Disk], disk_rates: &HashMap<String, (f64, f64)>, links: &[crate::netdiag::Link]) -> System {
    let mut s = System {
        // SAFETY: geteuid has no preconditions.
        privileged: unsafe { libc::geteuid() } == 0,
        host: read("/proc/sys/kernel/hostname").trim().into(),
        kernel: read("/proc/sys/kernel/osrelease").trim().into(),
        model: machine_model(),
        uptime: read("/proc/uptime").split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0.0),
        boot: read("/proc/stat").lines().find_map(|l| l.strip_prefix("btime ")).map_or(0, num),
        cores: probe.cpus,
        ..Default::default()
    };
    let load: Vec<f64> = read("/proc/loadavg").split_whitespace().take(3).filter_map(|v| v.parse().ok()).collect();
    if load.len() == 3 {
        s.load = [load[0], load[1], load[2]];
    }
    s.policies = policies();
    s.cpu_model = cpu_model();
    s.memory = mi
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            let kb = v.split_whitespace().next()?.parse::<u64>().ok()?;
            Some((k.trim().to_owned(), if v.contains("kB") { kb * 1024 } else { kb }))
        })
        .collect();
    for e in std::fs::read_dir("/sys/block").into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("zram") {
            continue;
        }
        let mm: Vec<u64> = read(e.path().join("mm_stat")).split_whitespace().map(num).collect();
        let algorithm = read(e.path().join("comp_algorithm")).split_whitespace().find(|a| a.starts_with('[')).map(|a| a.trim_matches(['[', ']']).to_owned()).unwrap_or_default();
        s.zram.push(Zram {
            name,
            algorithm,
            original: mm.first().copied().unwrap_or(0),
            compressed: mm.get(1).copied().unwrap_or(0),
            used: mm.get(2).copied().unwrap_or(0),
            disk_size: num(&read(e.path().join("disksize"))),
        });
    }
    for res in ["cpu", "memory", "io", "irq"] {
        let text = read(format!("/proc/pressure/{res}"));
        if text.is_empty() {
            continue;
        }
        let (some, full) = pressure(&text);
        // (The CPU's "full" line is all zero by definition.)
        s.pressure.push(Pressure { resource: res.into(), some: some.unwrap_or_default(), full: if res == "cpu" { None } else { full } });
    }
    s.disks = static_disks
        .iter()
        .map(|d| {
            let (iops, latency_ms) = disk_rates.get(&d.name).copied().unwrap_or_default();
            Disk { iops, latency_ms, ..d.clone() }
        })
        .collect();
    let addrs = addresses();
    for n in &probe.interfaces {
        let d = Path::new("/sys/class/net").join(n);
        let l = links.iter().find(|l| l.name == *n).cloned().unwrap_or_default();
        s.interfaces.push(Interface {
            name: n.clone(),
            up: read(d.join("operstate")).trim() == "up",
            mac: read(d.join("address")).trim().into(),
            mtu: num(&read(d.join("mtu"))),
            addresses: addrs.get(n).cloned().unwrap_or_default(),
            received: l.received,
            sent: l.sent,
            errors: l.errors,
            drops: l.drops,
        });
    }
    if let Some(b) = &probe.battery {
        let f = |k: &str| num(&read(b.join(k))) as f64;
        s.battery = Some(Battery {
            status: read(b.join("status")).trim().into(),
            capacity: f("capacity"),
            health: read(b.join("health")).trim().into(),
            cycles: f("cycle_count") as u64,
            energy: f("energy_now") / 1e6,
            full: f("energy_full") / 1e6,
            design: f("energy_full_design") / 1e6,
            watts: f("power_now") / 1e6,
            volts: f("voltage_now") / 1e6,
            model: read(b.join("model_name")).trim().into(),
        });
    }
    for g in &probe.gpus {
        use crate::gpus::Kind;
        let asleep = crate::gpus::asleep(&g.device);
        let (mut used, mut limit, mut jobs) = (0, 0, 0);
        match &g.kind {
            Kind::Agx => {
                let mem = read(g.device.join("gpu_memory"));
                let field = |k: &str| mem.lines().find_map(|l| l.strip_prefix(k)).map_or(0, num);
                (used, limit) = (field("user_resident ") + field("kernel_resident "), field("limit "));
                jobs = read(g.device.join("agx_stats")).lines().find_map(|l| l.strip_prefix("jobs ")).map_or(0, num);
            }
            Kind::Amd => (used, limit) = (num(&read(g.device.join("mem_info_vram_used"))), num(&read(g.device.join("mem_info_vram_total")))),
            Kind::Nvidia { bus } => {
                if let Some(n) = crate::gpus::nvidia_now(&g.device, bus) {
                    (used, limit) = (n.memory_used, n.memory_total);
                }
            }
            Kind::Xe { .. } => {}
        }
        // (Asahi's GPU by the Mac's chip.)
        let name = match (&g.kind, apple_chip(&s.model)) {
            (Kind::Agx, Some(chip)) => format!("Apple {chip} GPU"),
            _ => g.name.clone(),
        };
        s.gpus.push(Gpu { prefix: g.prefix.clone(), name, driver: g.driver.clone(), memory_used: used, memory_limit: limit, jobs, asleep });
    }
    s.cooling = cooling();
    s.tunables = TUNABLES.iter().map(|(k, _)| ((*k).to_owned(), read(sysctl_path(k)).trim().to_owned())).filter(|(_, v)| !v.is_empty()).collect();
    let nr: Vec<u64> = read("/proc/sys/fs/file-nr").split_whitespace().map(num).collect();
    s.open_files = (nr.first().copied().unwrap_or(0), nr.get(2).copied().unwrap_or(0));
    s
}

/// The idle states cpu0 has: (index, name, what it is, wake latency µs).
fn idle_states(cpus: usize) -> Vec<(usize, String, String, u64)> {
    if cpus == 0 {
        return Vec::new();
    }
    (0..16)
        .map(|i| (i, PathBuf::from(format!("/sys/devices/system/cpu/cpu0/cpuidle/state{i}"))))
        .take_while(|(_, d)| d.exists())
        .map(|(i, d)| (i, read(d.join("name")).trim().to_owned(), read(d.join("desc")).trim().to_owned(), num(&read(d.join("latency")))))
        .collect()
}

/// µs all cores spent in idle state `i`.
fn idle_time(cpus: usize, i: usize) -> u64 {
    (0..cpus).map(|c| num(&read(format!("/sys/devices/system/cpu/cpu{c}/cpuidle/state{i}/time")))).sum()
}

/// The disks' fixed facts (model, size, kind).
fn disk_facts(disks: &[String]) -> Vec<Disk> {
    disks
        .iter()
        .map(|d| {
            let b = Path::new("/sys/block").join(d);
            Disk {
                name: d.clone(),
                model: read(b.join("device/model")).trim().into(),
                size: num(&read(b.join("size"))) * 512,
                rotational: read(b.join("queue/rotational")).trim() == "1",
                removable: read(b.join("removable")).trim() == "1",
                ..Default::default()
            }
        })
        .collect()
}

/// Start sampling; `send` gets the source list once, then each sample.
pub fn start(control: Control, send_sources: impl FnOnce(Vec<Source>) + Send + 'static, send: impl Fn(Sample) + Send + 'static) {
    std::thread::Builder::new()
        .name("activity-sampler".into())
        .spawn(move || {
            let probe = probe();
            send_sources(probe.sources.clone());
            let users = users();
            let facts = disk_facts(&probe.disks);
            // SAFETY: sysconf has no preconditions.
            let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
            // SAFETY: as above.
            let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
            let begun = Instant::now();
            let mut prev: Option<(Instant, Counters)> = None;
            let mut known = Known::default();
            let mut volumes_at: Option<(Instant, Vec<Volume>)> = None;
            // Socket inode → its process, and when every process's open
            // files were last read for it.
            let mut owners: (HashMap<u64, i32>, Option<Instant>) = (HashMap::new(), None);
            let mut health_at: Option<(Instant, Vec<Health>)> = None;
            let idle_names = idle_states(probe.cpus);
            loop {
                if control.paused.load(Ordering::Relaxed) {
                    // The helper measures; start afresh when it stops.
                    prev = None;
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                let now = Instant::now();
                let mut c = Counters::default();
                let mut values = Vec::with_capacity(probe.sources.len());
                let elapsed = prev.as_ref().map(|(t, _)| now.duration_since(*t).as_secs_f64().max(0.001));
                let p = prev.as_ref().map(|(_, c)| c);
                let rate = |now: u64, before: Option<u64>| match (before, elapsed) {
                    (Some(b), Some(e)) => now.saturating_sub(b) as f64 / e,
                    _ => f64::NAN,
                };

                let stat = read("/proc/stat");
                c.cpu = cpu_lines(&stat);
                let share = |i: usize, f: fn(&CpuTimes) -> u64| match p.and_then(|p| p.cpu.get(i)) {
                    Some(b) => {
                        let a = c.cpu.get(i).copied().unwrap_or(*b);
                        let dt = a.total.saturating_sub(b.total);
                        if dt == 0 { 0.0 } else { f(&a).saturating_sub(f(b)) as f64 / dt as f64 * 100.0 }
                    }
                    None => f64::NAN,
                };
                values.push(share(0, |t| t.busy));
                values.push(share(0, |t| t.user));
                values.push(share(0, |t| t.system));
                values.push(share(0, |t| t.iowait));

                let mi = read("/proc/meminfo");
                let kb = |k| mem_kb(&mi, k) as f64 * 1024.0;
                let cached = kb("Cached:") + kb("Buffers:") + kb("SReclaimable:") - kb("Shmem:").min(kb("Cached:"));
                values.push(kb("MemTotal:") - kb("MemAvailable:"));
                values.push(cached.max(0.0));
                values.push(kb("SwapTotal:") - kb("SwapFree:"));

                let links = links();
                for l in links.iter().filter(|l| l.name != "lo") {
                    c.rx += l.received;
                    c.tx += l.sent;
                    c.ifaces.insert(l.name.clone(), (l.received, l.sent));
                }
                values.push(rate(c.rx, p.map(|p| p.rx)));
                values.push(rate(c.tx, p.map(|p| p.tx)));

                for l in read("/proc/diskstats").lines() {
                    let f: Vec<&str> = l.split_whitespace().collect();
                    if f.len() > 13 && probe.disks.iter().any(|d| d == f[2]) {
                        c.read += num(f[5]) * 512;
                        c.written += num(f[9]) * 512;
                        // Sectors read and written, ms busy, requests, ms they waited.
                        c.disks.insert(f[2].to_owned(), [num(f[5]), num(f[9]), num(f[12]), num(f[3]) + num(f[7]), num(f[6]) + num(f[10])]);
                    }
                }
                values.push(rate(c.read, p.map(|p| p.read)));
                values.push(rate(c.written, p.map(|p| p.written)));

                c.faults = read("/proc/vmstat").lines().find_map(|l| l.strip_prefix("pgfault ")).map_or(0, num);
                values.push(rate(c.faults, p.map(|p| p.faults)));
                c.switches = stat.lines().find_map(|l| l.strip_prefix("ctxt ")).map_or(0, num);
                values.push(rate(c.switches, p.map(|p| p.switches)));
                c.interrupts = stat.lines().find_map(|l| l.strip_prefix("intr ")).and_then(|r| r.split_whitespace().next()).map_or(0, num);
                values.push(rate(c.interrupts, p.map(|p| p.interrupts)));

                let procs = std::fs::read_dir("/proc").map(|rd| rd.flatten().filter(|e| e.file_name().to_str().is_some_and(|s| s.bytes().all(|b| b.is_ascii_digit()))).count()).unwrap_or(0);
                values.push(procs as f64);
                // "running/total" scheduling entities: the total is threads.
                let loadavg = read("/proc/loadavg");
                let threads = loadavg.split_whitespace().nth(3).and_then(|s| s.split_once('/')).map_or(0, |(_, t)| num(t));
                values.push(threads as f64);
                values.push(loadavg.split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(f64::NAN));

                let freqs: Vec<f64> = probe.freq.iter().map(|f| num(&read(f)) as f64 / 1000.0).collect();
                if !freqs.is_empty() {
                    values.push(freqs.iter().sum::<f64>() / freqs.len() as f64);
                }
                if let Some(b) = &probe.battery {
                    values.push(num(&read(b.join("capacity"))) as f64);
                    values.push(num(&read(b.join("power_now"))) as f64 / 1e6);
                }
                for i in 0..probe.cpus {
                    values.push(share(i + 1, |t| t.busy));
                }
                values.extend(freqs.iter().copied());
                for res in ["cpu", "memory", "io"] {
                    if Path::new("/proc/pressure").join(res).exists() {
                        values.push(pressure(&read(format!("/proc/pressure/{res}"))).0.map_or(f64::NAN, |v| v[0]));
                    }
                }
                for res in ["memory", "io"] {
                    if Path::new("/proc/pressure").join(res).exists() {
                        values.push(pressure(&read(format!("/proc/pressure/{res}"))).1.map_or(f64::NAN, |v| v[0]));
                    }
                }
                let mut disk_rates = HashMap::new();
                for d in &probe.disks {
                    let now_d = c.disks.get(d).copied().unwrap_or_default();
                    let was = p.and_then(|p| p.disks.get(d).copied());
                    values.push(rate(now_d[0] * 512, was.map(|w| w[0] * 512)));
                    values.push(rate(now_d[1] * 512, was.map(|w| w[1] * 512)));
                    values.push(match (was, elapsed) {
                        (Some(w), Some(e)) => (now_d[2].saturating_sub(w[2]) as f64 / (e * 1000.0) * 100.0).min(100.0),
                        _ => f64::NAN,
                    });
                    if let (Some(w), Some(e)) = (was, elapsed) {
                        let requests = now_d[3].saturating_sub(w[3]) as f64;
                        let waited = now_d[4].saturating_sub(w[4]) as f64;
                        disk_rates.insert(d.clone(), (requests / e, if requests > 0.0 { waited / requests } else { 0.0 }));
                    }
                }
                for n in &probe.interfaces {
                    let (rx, tx) = c.ifaces.get(n).copied().unwrap_or_default();
                    let was = p.and_then(|p| p.ifaces.get(n).copied());
                    values.push(rate(rx, was.map(|w| w.0)));
                    values.push(rate(tx, was.map(|w| w.1)));
                }
                for g in &probe.gpus {
                    use crate::gpus::Kind;
                    let x = &g.prefix;
                    match &g.kind {
                        Kind::Agx => {
                            let stats = read(g.device.join("agx_stats"));
                            let field = |k: &str| stats.lines().find_map(|l| l.strip_prefix(k)).map_or(0, num);
                            let (busy, jobs) = (field("busy_ns "), field("jobs "));
                            c.gpu_busy_ns.insert(x.clone(), busy);
                            c.gpu_jobs.insert(x.clone(), jobs);
                            values.push(match (p.and_then(|p| p.gpu_busy_ns.get(x)), elapsed) {
                                (Some(was), Some(e)) => (busy.saturating_sub(*was) as f64 / (e * 1e9) * 100.0).min(100.0),
                                _ => f64::NAN,
                            });
                            values.push(rate(jobs, p.and_then(|p| p.gpu_jobs.get(x).copied())));
                            values.push(field("power_mw ") as f64 / 1000.0);
                            let mem = read(g.device.join("gpu_memory"));
                            let field = |k: &str| mem.lines().find_map(|l| l.strip_prefix(k)).map_or(0, num);
                            values.push((field("user_resident ") + field("kernel_resident ")) as f64);
                        }
                        Kind::Amd => {
                            values.push(num(&read(g.device.join("gpu_busy_percent"))) as f64);
                            if g.device.join("mem_info_vram_used").exists() {
                                values.push(num(&read(g.device.join("mem_info_vram_used"))) as f64);
                            }
                        }
                        Kind::Xe { gts } => {
                            // Busy: what of the time since it wasn't idle (a
                            // GT asleep counts as idle).
                            let mut busy = |gt: &PathBuf| {
                                let idle = crate::gpus::xe_idle_ms(gt);
                                c.gt_idle.insert(gt.clone(), idle);
                                match (p.and_then(|p| p.gt_idle.get(gt)), elapsed) {
                                    (Some(was), Some(e)) => (100.0 - idle.saturating_sub(*was) as f64 / (e * 1000.0) * 100.0).clamp(0.0, 100.0),
                                    _ => f64::NAN,
                                }
                            };
                            let render = gts.iter().find(|gt| crate::gpus::xe_gt_name(gt) == "render").or(gts.first());
                            let rendering = render.map_or(f64::NAN, &mut busy);
                            if gts.len() > 1 {
                                // As busy as the busier of the two, and each.
                                let media = gts.iter().find(|gt| crate::gpus::xe_gt_name(gt) == "media").or(gts.get(1));
                                let playing = media.map_or(f64::NAN, &mut busy);
                                values.extend([crate::gpus::busiest(&[rendering, playing]), rendering, playing]);
                            } else {
                                values.push(rendering);
                            }
                            values.push(render.map_or(f64::NAN, |gt| crate::gpus::xe_mhz(gt)));
                        }
                        Kind::Nvidia { bus } => match crate::gpus::nvidia_now(&g.device, bus) {
                            Some(n) => values.extend([n.total(), n.busy, n.encode, n.decode, n.watts, n.celsius, n.mhz, n.memory_used as f64]),
                            // Asleep: nothing runs on it, and it draws next to nothing.
                            None => values.extend([0.0, 0.0, 0.0, 0.0, 0.0, f64::NAN, 0.0, 0.0]),
                        },
                    }
                }
                for s in &probe.sensors {
                    let raw = read(&s.path);
                    values.push(raw.trim().parse::<f64>().map_or(f64::NAN, |v| v * s.scale));
                }
                debug_assert_eq!(values.len(), probe.sources.len());

                let mut processes = if control.want_processes.load(Ordering::Relaxed) {
                    let (t0, i0) = (p.map(|p| p.ticks.clone()).unwrap_or_default(), p.map(|p| p.io.clone()).unwrap_or_default());
                    let (list, ticks, io) = processes(Before { ticks: &t0, io: &i0, with_io: true }, elapsed.unwrap_or(1.0), &users, page, hz, &mut known);
                    c.ticks = ticks;
                    c.io = io;
                    Some(list)
                } else {
                    None
                };
                // The sockets (netlink), whose owners give each process's
                // traffic and each connection's program.
                let want_connections = control.want_connections.load(Ordering::Relaxed);
                let sockets = processes.as_ref().and_then(|_| crate::netdiag::sockets().ok());
                let mut connections = None;
                if let (Some(list), Some(sockets)) = (processes.as_mut(), &sockets) {
                    // Who has each socket: every process's open files are
                    // read again only when a socket nobody is known to
                    // have is moving bytes (or, for the list of
                    // connections, now and then): a socket keeps its owner.
                    let before = p.and_then(|p| p.sockets.as_ref());
                    let active_unknown = sockets.iter().any(|s| s.tcp && s.inode != 0 && !owners.0.contains_key(&s.inode) && before.is_some_and(|b| b.get(&s.inode) != Some(&(s.sent, s.received))));
                    let stale = want_connections && owners.1.is_none_or(|t: Instant| t.elapsed() > Duration::from_secs(5));
                    if active_unknown || stale || owners.1.is_none() {
                        owners = (socket_owners(list).0, Some(now));
                    }
                    // (Whose files can be seen: as for its disk counters.)
                    let seen: std::collections::HashSet<i32> = list.iter().filter(|p| p.read.is_some()).map(|p| p.pid).collect();
                    c.sockets = Some(traffic(list, sockets, &owners.0, &seen, before, elapsed.unwrap_or(1.0)));
                    if want_connections {
                        connections = Some(connections_of(sockets, &owners.0, list));
                    }
                }
                if want_connections && connections.is_none() {
                    connections = processes.as_deref().map(self::connections);
                }
                let mut system = Box::new(system(&probe, &mi, &facts, &disk_rates, &links));
                // Volumes every few seconds: they change rarely, and a
                // slow disk can make statvfs wait.
                if volumes_at.as_ref().is_none_or(|(t, _)| t.elapsed() > Duration::from_secs(5)) {
                    volumes_at = Some((now, volumes(&read("/proc/self/mounts"))));
                }
                system.volumes = volumes_at.as_ref().map(|(_, v)| v.clone()).unwrap_or_default();
                // Disks' health once a minute: it changes slowly.
                if health_at.as_ref().is_none_or(|(t, _)| t.elapsed() > Duration::from_secs(60)) {
                    let mut ctrls: Vec<String> = probe.disks.iter().filter_map(|d| crate::smart::controller_of(d)).collect();
                    ctrls.dedup();
                    health_at = Some((
                        now,
                        ctrls
                            .into_iter()
                            .map(|c| {
                                let h = crate::smart::read(&c).ok();
                                (c, h)
                            })
                            .collect(),
                    ));
                }
                system.health = health_at.as_ref().map(|(_, h)| h.clone()).unwrap_or_default();
                c.idle = idle_names.iter().map(|(i, ..)| idle_time(probe.cpus, *i)).collect();
                if let (Some(p), Some(e)) = (p, elapsed) {
                    let all = e * 1e6 * probe.cpus.max(1) as f64;
                    system.idle_states = idle_names
                        .iter()
                        .zip(&c.idle)
                        .zip(&p.idle)
                        .map(|(((_, name, what, latency), now), was)| IdleState {
                            name: name.clone(),
                            what: what.clone(),
                            share: (now.saturating_sub(*was) as f64 / all * 100.0).min(100.0),
                            latency_us: *latency,
                        })
                        .collect();
                }
                if control.want_gpu_clients.load(Ordering::Relaxed)
                    && !probe.gpus.is_empty()
                    && let Some(pids) = processes.as_ref().map(|list| list.iter().filter(|p| p.memory > 0).map(|p| (p.pid, p.name.clone())).collect::<Vec<(i32, String)>>())
                {
                    let before = p.map(|p| p.gpu_busy.clone()).unwrap_or_default();
                    let (clients, busy) = gpu_clients(&probe.gpus, &pids, &before, elapsed.unwrap_or(1.0));
                    c.gpu_busy = busy;
                    // Each process's use of each GPU, for its columns.
                    if let Some(list) = processes.as_mut() {
                        let n = probe.gpus.len();
                        for pr in list.iter_mut() {
                            pr.gpus = vec![None; n];
                        }
                        let at: HashMap<i32, usize> = list.iter().enumerate().map(|(i, pr)| (pr.pid, i)).collect();
                        for cl in &clients {
                            if let Some(&i) = at.get(&cl.pid)
                                && let Some(slot) = list[i].gpus.get_mut(cl.gpu)
                            {
                                *slot = Some(slot.unwrap_or(0.0) + cl.busy.unwrap_or(0.0));
                            }
                        }
                    }
                    system.gpu_clients = Some(clients);
                }
                send(Sample { at: now.duration_since(begun).as_secs_f64(), values, system, processes, connections });
                prev = Some((now, c));
                let ms = control.interval_ms.load(Ordering::Relaxed).clamp(100, 10_000);
                std::thread::sleep(Duration::from_millis(ms).saturating_sub(now.elapsed()));
            }
        })
        .expect("the sampler thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_lines_read() {
        let stat = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 60 0 20 400 20 0 0 0 0 0\nintr 1\n";
        assert_eq!(cpu_times(stat), [(150, 1000), (80, 500)]);
        let all = cpu_lines(stat);
        assert_eq!((all[0].user, all[0].system, all[0].iowait), (100, 50, 50));
    }

    #[test]
    fn fan_service_state() {
        use olo_system::fans::{Profile, State};
        let own = |(a, b, c, d): (&str, &str, &str, &str)| (a.to_owned(), b.to_owned(), c.to_owned(), d.to_owned());
        let mut s = State { profile: Profile::Cool, installed: true, control: true, control_kept: true, running: true, held: Some(("6".into(), "coretemp Package id 0 74° (+4 rising)".into())) };
        assert_eq!(fan_state(Some(&s)), own(("cool", "6", "coretemp Package id 0 74° (+4 rising)", "")));
        // Running and silent (an older service before its first change):
        // the firmware's for now, not "isn't running".
        s.held = None;
        assert_eq!(fan_state(Some(&s)), own(("cool", "auto", "", "")));
        s.running = false;
        assert_eq!(fan_state(Some(&s)), own(("cool", "", "", "the fan service isn't running")));
        // A driver that takes no levels: whatever the service says.
        (s.running, s.control, s.held) = (true, false, Some(("auto".into(), String::new())));
        assert_eq!(fan_state(Some(&s)), own(("cool", "", "", "the fans can't be set until the computer's next start")));
        s.control_kept = false;
        assert_eq!(fan_state(Some(&s)), own(("cool", "", "", "the fans' driver doesn't let them be set")));
        s.profile = Profile::Auto;
        assert_eq!(fan_state(Some(&s)), own(("auto", "", "", "")));
        // Not installed, or no fans to set: no profile to speak of.
        s.installed = false;
        assert_eq!(fan_state(Some(&s)), own(("", "", "", "")));
        assert_eq!(fan_state(None), own(("", "", "", "")));
    }

    #[test]
    fn sensor_names() {
        assert_eq!(sensor_name("nvme", "temp1", "Composite"), "SSD temperature");
        assert_eq!(sensor_name("macsmc_battery", "power1", ""), "Battery power");
        assert_eq!(sensor_name("macsmc_hwmon", "fan1", "Fan 1"), "Fan 1");
        assert_eq!(sensor_name("k10temp", "temp1", ""), "k10temp temp1");
    }

    #[test]
    fn apple_chips() {
        assert_eq!(apple_chip("Apple MacBook Pro (16-inch, M2 Max, 2023)").as_deref(), Some("M2 Max"));
        assert_eq!(apple_chip("Apple MacBook Air (M1, 2020)").as_deref(), Some("M1"));
        assert_eq!(apple_chip("LENOVO ThinkPad X1"), None);
    }

    #[test]
    fn units() {
        assert_eq!(Unit::Percent.format(12.345), "12.3%");
        assert_eq!(Unit::Rate.format(2048.0), "2.0 KB/s");
        assert_eq!(Unit::Rate.format(0.0), "0 KB/s");
        assert_eq!(Unit::Rate.format(300.0), "<1 KB/s");
        assert_eq!(Unit::Celsius.format(41.25), "41.2 °C");
        assert_eq!(Unit::Load.format(1.5), "1.50");
    }

    #[test]
    fn probes_this_machine() {
        let p = probe();
        assert!(p.cpus >= 1);
        assert_eq!(p.sources.iter().filter(|s| s.group == Group::Cpus).count(), p.cpus);
        // Every key is unique (settings refer to sources by key).
        let mut keys: Vec<&str> = p.sources.iter().map(|s| s.key.as_str()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), p.sources.len());
    }

    #[test]
    fn lists_processes() {
        let (list, ticks, _) = processes(Before { ticks: &HashMap::new(), io: &HashMap::new(), with_io: true }, 1.0, &users(), 4096, 100.0, &mut Known::default());
        let me = std::process::id() as i32;
        let mine = list.iter().find(|p| p.pid == me).expect("this test's process");
        assert!(mine.threads >= 1 && mine.ppid > 0);
        // Its own I/O can be read.
        assert!(mine.read.is_some());
        assert!(ticks.contains_key(&me));
    }

    #[test]
    fn mounts() {
        assert_eq!(unescape("/run/media/a\\040b"), "/run/media/a b");
        let v = volumes("/dev/nvme0n1p6 / btrfs rw,subvol=root 0 0\nproc /proc proc rw 0 0\n/dev/nvme0n1p6 /home btrfs rw 0 0\nhost:/x /mnt/x nfs4 ro 0 0\n");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].mount, "/, /home");
        assert!(v[0].size.is_some());
        assert!(v[1].size.is_none() && v[1].read_only);
    }

    #[test]
    fn addresses_and_pressure() {
        assert_eq!(socket_address("0100007F:0050"), "127.0.0.1:80");
        assert_eq!(socket_address("00000000000000000000000001000000:01BB"), "[::1]:443");
        assert_eq!(socket_address("0000000000000000FFFF00000100007F:0016"), "127.0.0.1:22");
        let (some, full) = pressure("some avg10=1.50 avg60=0.20 avg300=0.10 total=5\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n");
        assert_eq!(some, Some([1.5, 0.2, 0.1]));
        assert_eq!(full, Some([0.0, 0.0, 0.0]));
        assert_eq!(tcp_state("0A"), "Listening");
    }

    #[test]
    fn samples_add_up() {
        // Every source gets a value each sample.
        let control = Control::new(100);
        control.set(Wants { interval_ms: 100, processes: true, connections: true, gpu_clients: true });
        let (tx, rx) = std::sync::mpsc::channel();
        let (stx, srx) = std::sync::mpsc::channel();
        start(
            control,
            move |s| stx.send(s).unwrap(),
            move |s| {
                let _ = tx.send(s);
            },
        );
        let sources = srx.recv_timeout(Duration::from_secs(5)).unwrap();
        let _first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let second = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(second.values.len(), sources.len());
        assert!(second.values[0].is_finite());
        assert!(second.processes.is_some() && second.connections.is_some());
        assert!(second.system.cores >= 1 && !second.system.kernel.is_empty());
        assert!(second.system.gpu_clients.is_some() || second.system.gpus.is_empty());
        assert!(!second.system.volumes.is_empty());
        // It goes over the helper's pipe and back.
        let text = serde_json::to_string(&second).unwrap();
        let back: Sample = serde_json::from_str(&text).unwrap();
        assert_eq!(back.values.len(), second.values.len());
    }
}
