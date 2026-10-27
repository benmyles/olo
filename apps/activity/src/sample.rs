//! What Activity Monitor measures, read from /proc and /sys on a worker
//! thread at the update interval: CPU (total and per core), memory, swap,
//! network, disk, faults, context switches, processes and threads, CPU
//! frequency, battery, and every temperature, power and fan sensor the
//! hardware reports (the Mac's SMC included). Processes are listed only
//! while someone is looking at them.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq)]
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
}

impl Unit {
    pub fn format(self, v: f64) -> String {
        match self {
            Unit::Percent => format!("{v:.1}%"),
            Unit::Bytes => olo_kit::files::format_size(v.max(0.0) as u64),
            Unit::Rate => format!("{}/s", olo_kit::files::format_size(v.max(0.0) as u64)),
            Unit::Count => format!("{v:.0}"),
            Unit::PerSecond => format!("{v:.0}/s"),
            Unit::Watts => format!("{v:.1} W"),
            Unit::Celsius => format!("{v:.1} °C"),
            Unit::Rpm => format!("{v:.0} rpm"),
            Unit::Megahertz => format!("{v:.0} MHz"),
        }
    }
}

/// Where a source sits in the graph's context menu.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Group {
    Main,
    Cpus,
    Sensors,
}

#[derive(Debug, Clone)]
pub struct Source {
    pub key: String,
    pub name: String,
    pub unit: Unit,
    pub color: [u8; 3],
    /// A fixed top of the scale; None scales to what the graph shows.
    pub max: Option<f64>,
    pub group: Group,
}

#[derive(Debug, Clone)]
pub struct Proc {
    pub pid: i32,
    pub name: String,
    pub user: String,
    pub uid: u32,
    /// Of one core (so up to 100 × cores).
    pub cpu: f64,
    pub memory: u64,
    pub threads: u32,
    pub state: char,
}

pub struct Sample {
    /// One value per source, in order (NaN until it can be known, e.g. a
    /// rate's first sample).
    pub values: Vec<f64>,
    pub processes: Option<Vec<Proc>>,
}

/// Shared with the worker.
#[derive(Clone)]
pub struct Control {
    pub interval_ms: Arc<AtomicU64>,
    pub want_processes: Arc<AtomicBool>,
}

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn num(s: &str) -> u64 {
    s.trim().parse().unwrap_or(0)
}

// Colours for the sources: the reference hues, readable on light and dark.
const BLUE: [u8; 3] = [40, 110, 230];
const RED: [u8; 3] = [220, 50, 40];
const GREEN: [u8; 3] = [40, 160, 60];
const ORANGE: [u8; 3] = [235, 130, 20];
const PURPLE: [u8; 3] = [150, 70, 200];
const TEAL: [u8; 3] = [20, 160, 170];
const BROWN: [u8; 3] = [160, 100, 50];
const PINK: [u8; 3] = [220, 70, 150];
const OLIVE: [u8; 3] = [130, 140, 30];
const GREY: [u8; 3] = [128, 128, 128];
const SPREAD: [[u8; 3]; 10] = [BLUE, RED, GREEN, ORANGE, PURPLE, TEAL, BROWN, PINK, OLIVE, GREY];

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
}

fn probe() -> Probe {
    let mut sources = Vec::new();
    let mut add = |key: &str, name: &str, unit: Unit, color: [u8; 3], max: Option<f64>, group: Group| {
        sources.push(Source { key: key.into(), name: name.into(), unit, color, max, group });
    };
    let mem_total = mem_kb(&read("/proc/meminfo"), "MemTotal:") as f64 * 1024.0;
    let swap_total = mem_kb(&read("/proc/meminfo"), "SwapTotal:") as f64 * 1024.0;
    add("cpu", "CPU usage", Unit::Percent, BLUE, Some(100.0), Group::Main);
    add("memory", "Used memory", Unit::Bytes, RED, Some(mem_total), Group::Main);
    add("cached", "Cached memory", Unit::Bytes, GREEN, Some(mem_total), Group::Main);
    add("swap", "Swap space", Unit::Bytes, ORANGE, Some(swap_total.max(1.0)), Group::Main);
    add("net-rx", "Network receive", Unit::Rate, TEAL, None, Group::Main);
    add("net-tx", "Network send", Unit::Rate, PURPLE, None, Group::Main);
    add("disk-read", "Disk read", Unit::Rate, BROWN, None, Group::Main);
    add("disk-write", "Disk write", Unit::Rate, PINK, None, Group::Main);
    add("faults", "Page faults", Unit::PerSecond, OLIVE, None, Group::Main);
    add("switches", "Context switches", Unit::PerSecond, GREY, None, Group::Main);
    add("processes", "Processes", Unit::Count, RED, None, Group::Main);
    add("threads", "Threads", Unit::Count, BLUE, None, Group::Main);

    let freq: Vec<PathBuf> = (0..1024)
        .map(|i| PathBuf::from(format!("/sys/devices/system/cpu/cpu{i}/cpufreq/scaling_cur_freq")))
        .take_while(|p| p.exists())
        .collect();
    if !freq.is_empty() {
        add("cpu-freq", "CPU frequency", Unit::Megahertz, ORANGE, None, Group::Main);
    }
    let battery = std::fs::read_dir("/sys/class/power_supply")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| read(p.join("type")).trim() == "Battery" && p.join("capacity").exists());
    if battery.is_some() {
        add("battery", "Battery charge", Unit::Percent, GREEN, Some(100.0), Group::Main);
    }

    let cpus = read("/proc/stat").lines().filter(|l| l.starts_with("cpu") && !l.starts_with("cpu ")).count();
    for i in 0..cpus {
        add(&format!("cpu{i}"), &format!("CPU {}", i + 1), Unit::Percent, SPREAD[i % SPREAD.len()], Some(100.0), Group::Cpus);
    }

    let mut sensors = Vec::new();
    let mut hwmons: Vec<PathBuf> = std::fs::read_dir("/sys/class/hwmon").into_iter().flatten().flatten().map(|e| e.path()).collect();
    hwmons.sort_by_key(|h| h.file_name().and_then(|n| n.to_str()?.strip_prefix("hwmon")?.parse::<u32>().ok()).unwrap_or(u32::MAX));
    let mut seen: HashMap<String, usize> = HashMap::new();
    for h in hwmons {
        let chip = read(h.join("name")).trim().to_owned();
        let mut inputs: Vec<PathBuf> = std::fs::read_dir(&h)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().ends_with("_input")))
            .collect();
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

    // Whole physical disks (not partitions, loop or RAM devices).
    let disks = std::fs::read_dir("/sys/block")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("device").exists())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    Probe { sources, cpus, sensors, battery, freq, disks }
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
        "tas2764" | "tas2770" => format!("Speaker amplifier {kind}"),
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
    cpu: Vec<(u64, u64)>,
    rx: u64,
    tx: u64,
    read: u64,
    written: u64,
    faults: u64,
    switches: u64,
    /// Per process: (utime + stime) ticks.
    ticks: HashMap<i32, u64>,
}

/// (busy, total) jiffies for "cpu" and each "cpuN".
fn cpu_times(stat: &str) -> Vec<(u64, u64)> {
    stat.lines()
        .filter(|l| l.starts_with("cpu"))
        .map(|l| {
            let f: Vec<u64> = l.split_whitespace().skip(1).map(num).collect();
            let total: u64 = f.iter().take(8).sum();
            let idle = f.get(3).copied().unwrap_or(0) + f.get(4).copied().unwrap_or(0);
            (total - idle.min(total), total)
        })
        .collect()
}

fn users() -> HashMap<u32, String> {
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

/// Every process: its counters now, and how it looked.
fn processes(prev: &HashMap<i32, u64>, elapsed: f64, users: &HashMap<u32, String>, page: u64, hz: f64) -> (Vec<Proc>, HashMap<i32, u64>) {
    let mut list = Vec::new();
    let mut ticks = HashMap::new();
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
        let t = num(f[11]) + num(f[12]);
        let threads = num(f[17]) as u32;
        let rss = num(f[21]) * page;
        ticks.insert(pid, t);
        let cpu = prev.get(&pid).map_or(0.0, |&p| t.saturating_sub(p) as f64 / hz / elapsed * 100.0);
        // comm is cut at 15 bytes; the program's file name isn't.
        let mut name = comm.to_owned();
        if name.len() >= 15 {
            let cmd = read(dir.join("cmdline"));
            if let Some(arg0) = cmd.split('\0').next()
                && let Some(base) = Path::new(arg0).file_name().map(|b| b.to_string_lossy().into_owned())
                && base.starts_with(&name)
            {
                name = base;
            }
        }
        let uid = std::fs::metadata(&dir).map_or(0, |m| std::os::unix::fs::MetadataExt::uid(&m));
        let user = users.get(&uid).cloned().unwrap_or_else(|| uid.to_string());
        list.push(Proc { pid, name, user, uid, cpu, memory: rss, threads, state });
    }
    (list, ticks)
}

/// Start sampling; `send` gets the source list once, then each sample.
pub fn start(control: Control, send_sources: impl FnOnce(Vec<Source>) + Send + 'static, send: impl Fn(Sample) + Send + 'static) {
    std::thread::spawn(move || {
        let probe = probe();
        send_sources(probe.sources.clone());
        let users = users();
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
        // SAFETY: as above.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
        let mut prev: Option<(Instant, Counters)> = None;
        loop {
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
            c.cpu = cpu_times(&stat);
            let usage = |i: usize| match p.and_then(|p| p.cpu.get(i)) {
                Some(&(b0, t0)) => {
                    let (b1, t1) = c.cpu.get(i).copied().unwrap_or((b0, t0));
                    let dt = t1.saturating_sub(t0);
                    if dt == 0 { 0.0 } else { b1.saturating_sub(b0) as f64 / dt as f64 * 100.0 }
                }
                None => f64::NAN,
            };
            values.push(usage(0));

            let mi = read("/proc/meminfo");
            let kb = |k| mem_kb(&mi, k) as f64 * 1024.0;
            let cached = kb("Cached:") + kb("Buffers:") + kb("SReclaimable:") - kb("Shmem:").min(kb("Cached:"));
            values.push(kb("MemTotal:") - kb("MemAvailable:"));
            values.push(cached.max(0.0));
            values.push(kb("SwapTotal:") - kb("SwapFree:"));

            for l in read("/proc/net/dev").lines().skip(2) {
                let Some((iface, rest)) = l.split_once(':') else { continue };
                if iface.trim() == "lo" {
                    continue;
                }
                let f: Vec<u64> = rest.split_whitespace().map(num).collect();
                c.rx += f.first().copied().unwrap_or(0);
                c.tx += f.get(8).copied().unwrap_or(0);
            }
            values.push(rate(c.rx, p.map(|p| p.rx)));
            values.push(rate(c.tx, p.map(|p| p.tx)));

            for l in read("/proc/diskstats").lines() {
                let f: Vec<&str> = l.split_whitespace().collect();
                if f.len() > 9 && probe.disks.iter().any(|d| d == f[2]) {
                    c.read += num(f[5]) * 512;
                    c.written += num(f[9]) * 512;
                }
            }
            values.push(rate(c.read, p.map(|p| p.read)));
            values.push(rate(c.written, p.map(|p| p.written)));

            c.faults = read("/proc/vmstat").lines().find_map(|l| l.strip_prefix("pgfault ")).map_or(0, num);
            values.push(rate(c.faults, p.map(|p| p.faults)));
            c.switches = stat.lines().find_map(|l| l.strip_prefix("ctxt ")).map_or(0, num);
            values.push(rate(c.switches, p.map(|p| p.switches)));

            let procs = std::fs::read_dir("/proc")
                .map(|rd| rd.flatten().filter(|e| e.file_name().to_str().is_some_and(|s| s.bytes().all(|b| b.is_ascii_digit()))).count())
                .unwrap_or(0);
            values.push(procs as f64);
            // "running/total" scheduling entities: the total is threads.
            let threads = read("/proc/loadavg").split_whitespace().nth(3).and_then(|s| s.split_once('/')).map_or(0, |(_, t)| num(t));
            values.push(threads as f64);

            if !probe.freq.is_empty() {
                let sum: u64 = probe.freq.iter().map(|f| num(&read(f))).sum();
                values.push(sum as f64 / probe.freq.len() as f64 / 1000.0);
            }
            if let Some(b) = &probe.battery {
                values.push(num(&read(b.join("capacity"))) as f64);
            }
            for i in 0..probe.cpus {
                values.push(usage(i + 1));
            }
            for s in &probe.sensors {
                let raw = read(&s.path);
                values.push(raw.trim().parse::<f64>().map_or(f64::NAN, |v| v * s.scale));
            }

            let processes = if control.want_processes.load(Ordering::Relaxed) {
                let before = p.map(|p| p.ticks.clone()).unwrap_or_default();
                let (list, ticks) = processes(&before, elapsed.unwrap_or(1.0), &users, page, hz);
                c.ticks = ticks;
                Some(list)
            } else {
                None
            };
            send(Sample { values, processes });
            prev = Some((now, c));
            let ms = control.interval_ms.load(Ordering::Relaxed).clamp(100, 10_000);
            std::thread::sleep(Duration::from_millis(ms).saturating_sub(now.elapsed()));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_lines() {
        let stat = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 60 0 20 400 20 0 0 0 0 0\nintr 1\n";
        assert_eq!(cpu_times(stat), [(150, 1000), (80, 500)]);
    }

    #[test]
    fn sensor_names() {
        assert_eq!(sensor_name("nvme", "temp1", "Composite"), "SSD temperature");
        assert_eq!(sensor_name("macsmc_battery", "power1", ""), "Battery power");
        assert_eq!(sensor_name("macsmc_hwmon", "fan1", "Fan 1"), "Fan 1");
        assert_eq!(sensor_name("k10temp", "temp1", ""), "k10temp temp1");
    }

    #[test]
    fn units() {
        assert_eq!(Unit::Percent.format(12.345), "12.3%");
        assert_eq!(Unit::Rate.format(2048.0), "2.0 KB/s");
        assert_eq!(Unit::Celsius.format(41.25), "41.2 °C");
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
        let (list, ticks) = processes(&HashMap::new(), 1.0, &users(), 4096, 100.0);
        let me = std::process::id() as i32;
        assert!(list.iter().any(|p| p.pid == me && p.threads >= 1));
        assert!(ticks.contains_key(&me));
    }
}
