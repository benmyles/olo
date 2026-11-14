//! Live readings for Deskbar's widgets (`olo_protocol::deskbar`): what
//! there is to measure (`sources`, for Preferences ▸ Deskbar) and each
//! widget's reading (`Reader`), from the kernel's own counters (`/proc`
//! and `/sys`) and the GPUs' (`gpu_load`). Nothing here starts a program
//! or wakes a GPU: one asleep reads as asleep, and an idle NVIDIA GPU is
//! asked only every 20 s (`gpu_load::nvidia_now`).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Instant,
};

use olo_protocol::{
    deskbar::{Measure, PoolView},
    pool::{self, PoolMeters},
};

use crate::gpu_load::{self, Kind, Probe};

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// What this computer has to measure.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sources {
    /// How many processor cores (threads) it has.
    pub cores: usize,
    /// Its GPUs, by PCI address, and their names (the one it started on
    /// first).
    pub gpus: Vec<(String, String)>,
    /// Its disks ("nvme0n1"), and what to call each.
    pub disks: Vec<(String, String)>,
    /// Its volumes' mount points, and what to call each.
    pub volumes: Vec<(String, String)>,
    /// Its network interfaces ("wlp0s20f3"), and what to call each.
    pub interfaces: Vec<(String, String)>,
    /// Its temperature sensors ("coretemp/Package id 0"), and what to
    /// call each.
    pub sensors: Vec<(String, String)>,
    /// Whether it has a battery.
    pub battery: bool,
    /// Token Pool's providers and the quotas their accounts have, as it
    /// last said (none while it isn't running).
    pub pool: Vec<(String, Vec<String>)>,
}

/// What there is to measure here (a few milliseconds: sysfs and procfs
/// only; read it off the UI thread).
pub fn sources() -> Sources {
    Sources {
        cores: cpu_lines(&read("/proc/stat")).len().saturating_sub(1),
        gpus: gpu_load::find().iter().map(|p| (pci_of(p), p.name.clone())).collect(),
        disks: disks(),
        volumes: volumes(),
        interfaces: interfaces(),
        sensors: sensors().into_iter().map(|s| (s.key, s.name)).collect(),
        battery: battery_dir().is_some(),
        pool: PoolMeters::load().map(|m| pool::PROVIDERS.iter().map(|p| (p.to_string(), m.quotas(p))).filter(|(_, q)| !q.is_empty()).collect()).unwrap_or_default(),
    }
}

fn pci_of(p: &Probe) -> String {
    p.device.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// What a reading is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Percent,
    BytesPerSecond,
    Celsius,
    Watts,
}

/// A widget's reading: a value for each of its series (all the
/// processors, or each core; reads and writes; down and up), in `unit`.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub values: Vec<f64>,
    /// Each series' name, for a widget's tip ("Read", "Core 3").
    pub names: Vec<String>,
    pub unit: Unit,
    /// Said instead of, or beside, the value: "asleep", "not found",
    /// "11.2 of 62 GB".
    pub note: Option<String>,
    /// Its value in words, where the unit's own won't do ("62% left").
    pub words: Option<String>,
    /// Its tip's lines after what it measures (a Token Pool widget's
    /// accounts), instead of each series' value.
    pub detail: Vec<String>,
    /// What's left of what it measures, 0 to 1, for a widget that warns
    /// as it runs low (a quota).
    pub left: Option<f64>,
}

impl Reading {
    fn of(values: Vec<f64>, names: Vec<String>, unit: Unit) -> Reading {
        Reading { values, names, unit, note: None, words: None, detail: Vec::new(), left: None }
    }

    fn missing(unit: Unit, why: &str) -> Reading {
        Reading { values: Vec::new(), names: Vec::new(), unit, note: Some(why.to_owned()), words: None, detail: Vec::new(), left: None }
    }

    /// The level a series is drawn at, 0 to 1, against `top` (the
    /// highest a rate has been in the widget's history: rates have no
    /// top of their own).
    pub fn level(&self, value: f64, top: f64) -> f32 {
        let full = match self.unit {
            Unit::Percent => 100.0,
            // (20 °C to 100 °C: a computer's range.)
            Unit::Celsius => return ((value - 20.0) / 80.0).clamp(0.0, 1.0) as f32,
            Unit::BytesPerSecond | Unit::Watts => top.max(1.0),
        };
        (value / full).clamp(0.0, 1.0) as f32
    }
}

/// A rate in bytes a second, short: "980 B/s", "2.1 MB/s".
pub fn rate_text(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["B/s", "KB/s", "MB/s", "GB/s", "TB/s"];
    let mut v = bytes.max(0.0);
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 || v >= 100.0 { format!("{v:.0} {}", UNITS[i]) } else { format!("{v:.1} {}", UNITS[i]) }
}

/// One value in `unit`, in words: "23%", "2.1 MB/s", "54 °C", "8.4 W".
pub fn unit_text(unit: Unit, v: f64) -> String {
    match unit {
        Unit::Percent => format!("{v:.0}%"),
        Unit::BytesPerSecond => rate_text(v),
        Unit::Celsius => format!("{v:.0} °C"),
        Unit::Watts => format!("{v:.1} W"),
    }
}

/// A reading's value in words, for a widget's row and tip: "23%",
/// "2.1 MB/s", "↓ 1.2 MB/s ↑ 40 KB/s", "54 °C", "8.4 W", or its note.
pub fn value_text(measure: &Measure, r: &Reading) -> String {
    if let Some(words) = &r.words {
        return words.clone();
    }
    if r.values.is_empty() {
        return r.note.clone().unwrap_or_default();
    }
    let one = |v: f64| unit_text(r.unit, v);
    match measure {
        // All the cores together: their mean.
        Measure::Cpu { cores: true } => one(r.values.iter().sum::<f64>() / r.values.len() as f64),
        Measure::Network { down: true, up: true, .. } if r.values.len() == 2 => format!("↓ {} ↑ {}", one(r.values[0]), one(r.values[1])),
        Measure::Disk { read: true, write: true, .. } if r.values.len() == 2 => format!("R {} W {}", one(r.values[0]), one(r.values[1])),
        _ => one(r.values[0]),
    }
}

/// What a widget measures, said in full ("CPU, each core", "Network:
/// wlp0s20f3, down and up"), as Preferences lists it.
pub fn describe(measure: &Measure, sources: &Sources) -> String {
    let named = |list: &[(String, String)], key: &str| list.iter().find(|(k, _)| k == key).map_or_else(|| key.to_owned(), |(_, n)| n.clone());
    match measure {
        Measure::Cpu { cores: false } => "CPU".into(),
        Measure::Cpu { cores: true } => "CPU, each core".into(),
        Measure::Memory { swap: false } => "Memory".into(),
        Measure::Memory { swap: true } => "Memory and swap".into(),
        Measure::Gpu { pci: None } => sources.gpus.first().map_or_else(|| "GPU".into(), |(_, n)| n.clone()),
        Measure::Gpu { pci: Some(p) } => named(&sources.gpus, p),
        Measure::Disk { device, read, write } => {
            let which = device.as_deref().map_or_else(|| "every disk".to_owned(), |d| named(&sources.disks, d));
            let what = match (read, write) {
                (true, false) => ", reads",
                (false, true) => ", writes",
                _ => "",
            };
            format!("Disk: {which}{what}")
        }
        Measure::Space { mount } => format!("Disk Space: {}", named(&sources.volumes, mount)),
        Measure::Network { interface, down, up } => {
            let which = interface.as_deref().map_or_else(|| "every interface".to_owned(), |i| named(&sources.interfaces, i));
            let what = match (down, up) {
                (true, false) => ", down",
                (false, true) => ", up",
                _ => "",
            };
            format!("Network: {which}{what}")
        }
        Measure::Temperature { sensor: None } => "Temperature: the processor".into(),
        Measure::Temperature { sensor: Some(s) } => format!("Temperature: {}", named(&sources.sensors, s)),
        Measure::Power => "Power from the battery".into(),
        Measure::Pool { provider, quotas, view, .. } => {
            let what = match quotas.as_slice() {
                [] => format!("{}, what holds each account back", pool::provider_name(provider)),
                [q] => format!("{}, {q}", pool::provider_name(provider)),
                [first @ .., last] => format!("{}, {} and {last}", pool::provider_name(provider), first.join(", ")),
            };
            let how = match view {
                PoolView::Together => "",
                PoolView::Each => ", each account",
                PoolView::Serving => ", the account answering now",
            };
            format!("Token Pool: {what}{how}")
        }
    }
}

/// What a Token Pool widget shows at `now` (seconds since the epoch):
/// from the meters the pool last wrote (`olo_protocol::pool`). A series
/// for each quota chosen (each account's in Each, as "Work 5-hour" when
/// there are several quotas); `left` is the least left of them, what
/// warns and tells.
pub fn pool_reading(m: Option<&PoolMeters>, measure: &Measure, now: i64) -> Reading {
    let Measure::Pool { provider, quotas, view, used, count_off, .. } = measure else { return Reading::missing(Unit::Percent, "not the pool") };
    let Some(m) = m else { return Reading::missing(Unit::Percent, "Token Pool isn't running") };
    if !m.loaded {
        return Reading::missing(Unit::Percent, "Token Pool is opening its accounts");
    }
    let shown = |left: f64| if *used { 100.0 - left } else { left };
    let word = if *used { "used" } else { "left" };
    let chosen = |label: &str| quotas.is_empty() || quotas.iter().any(|q| q == label);
    if *view == PoolView::Serving {
        let Some(a) = m.serving(provider) else { return Reading::missing(Unit::Percent, "no account has answered yet") };
        let windows: Vec<&olo_protocol::pool::PoolWindow> = a.windows.iter().filter(|w| chosen(&w.label)).collect();
        if windows.is_empty() {
            return Reading::missing(Unit::Percent, "its quotas aren't known yet");
        }
        let left = |w: &olo_protocol::pool::PoolWindow| (100.0 - w.used_at(now)).clamp(0.0, 100.0);
        // (Its least of those for every request: what holds it back.)
        let least = windows.iter().filter(|w| w.models.is_empty() || !quotas.is_empty()).map(|w| left(w)).fold(100.0, f64::min);
        let mut r = Reading::of(windows.iter().map(|w| shown(left(w))).collect(), windows.iter().map(|w| w.label.clone()).collect(), Unit::Percent);
        r.words = Some(format!("{}: {:.0}% {word}", a.name, shown(least)));
        r.detail = std::iter::once(format!("{}{}", a.name, if a.in_flight > 0 { ", answering now" } else { ", answered last" }))
            .chain(windows.iter().map(|w| match w.resets_at.filter(|t| *t > now) {
                Some(t) => format!("{}: {:.0}% {word}, resets in {}", w.label, shown(left(w)), pool::duration_text(t - now)),
                None => format!("{}: {:.0}% {word}", w.label, shown(left(w))),
            }))
            .collect();
        r.left = Some(least / 100.0);
        return r;
    }
    // Each quota chosen across the accounts (none chosen: what holds
    // each back).
    let asked: Vec<Option<&str>> = if quotas.is_empty() { vec![None] } else { quotas.iter().map(|q| Some(q.as_str())).collect() };
    let found: Vec<(Option<&str>, pool::Availability)> = asked.iter().filter_map(|q| Some((*q, pool::availability(m, provider, *q, *count_off, now)?))).collect();
    if found.is_empty() {
        let name = pool::provider_name(provider);
        return Reading::missing(
            Unit::Percent,
            &match quotas.as_slice() {
                [] => format!("no {name} accounts"),
                [q] => format!("no {name} account has a {q} quota"),
                _ => format!("no {name} account has those quotas"),
            },
        );
    }
    let several = found.len() > 1;
    let (mut values, mut names) = (Vec::new(), Vec::new());
    for (q, a) in &found {
        match view {
            PoolView::Each => {
                for c in a.accounts.iter().filter(|c| c.counted) {
                    values.push(shown(c.left.unwrap_or(0.0)));
                    names.push(match q {
                        Some(q) if several => format!("{} {q}", c.name),
                        _ => c.name.clone(),
                    });
                }
            }
            _ => {
                values.push(shown(a.fraction() * 100.0));
                names.push(if several { q.unwrap_or_default().to_owned() } else { a.title() });
            }
        }
    }
    let mut r = Reading::of(values, names, Unit::Percent);
    let least = found.iter().map(|(_, a)| a.fraction()).fold(1.0, f64::min);
    r.words = Some(if several {
        format!("{} {word}", found.iter().map(|(_, a)| format!("{:.0}%", shown(a.fraction() * 100.0))).collect::<Vec<_>>().join(" · "))
    } else {
        format!("{:.0}% {word}", shown(least * 100.0))
    });
    for (q, a) in &found {
        let mut lines = a.detail(now);
        if several && let (Some(q), Some(first)) = (q, lines.first_mut()) {
            *first = format!("{q}: {first}");
        }
        r.detail.extend(lines);
    }
    // Out on every account: when it's back (which quota, of several).
    if let Some((q, a)) = found.iter().find(|(_, a)| a.exhausted())
        && let Some((at, who)) = &a.back
    {
        r.note = Some(match q {
            Some(q) if several => format!("{q}: {who} back in {}", pool::duration_text(at - now)),
            _ => format!("{who} back in {}", pool::duration_text(at - now)),
        });
    }
    r.left = Some(least);
    r
}

/// What a widget remembers between readings, to make rates of the
/// kernel's running counters.
#[derive(Default)]
pub struct Reader {
    at: Option<Instant>,
    cpu: Vec<(u64, u64)>,
    counters: HashMap<String, (u64, u64)>,
    gpu: Option<Probe>,
    /// An xe GPU's idle time when last read, each of its GTs'.
    gpu_idle: Option<Vec<u64>>,
}

impl Reader {
    pub fn new() -> Reader {
        Reader::default()
    }

    /// `measure` now. A rate's first reading has nothing to compare with:
    /// None, and its next is the first.
    pub fn read(&mut self, measure: &Measure) -> Option<Reading> {
        let now = Instant::now();
        let elapsed = self.at.map(|t| now.duration_since(t).as_secs_f64()).filter(|s| *s > 0.0);
        self.at = Some(now);
        match measure {
            Measure::Cpu { cores } => {
                let lines = cpu_lines(&read("/proc/stat"));
                let before = std::mem::replace(&mut self.cpu, lines.clone());
                if before.len() != lines.len() {
                    return None;
                }
                let busy = |(b, t): (&(u64, u64), &(u64, u64))| {
                    let (db, dt) = (t.0.saturating_sub(b.0), t.1.saturating_sub(b.1));
                    if dt == 0 { 0.0 } else { 100.0 * db as f64 / dt as f64 }
                };
                let all: Vec<f64> = before.iter().zip(lines.iter()).map(busy).collect();
                if *cores {
                    let names = (1..all.len()).map(|i| format!("Core {i}")).collect();
                    Some(Reading::of(all[1..].to_vec(), names, Unit::Percent))
                } else {
                    Some(Reading::of(vec![all[0]], vec!["CPU".into()], Unit::Percent))
                }
            }
            Measure::Memory { swap } => {
                let info = read("/proc/meminfo");
                let kb = |key: &str| info.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(':')?.split_whitespace().next()?.parse::<u64>().ok()).unwrap_or(0);
                let (total, available) = (kb("MemTotal"), kb("MemAvailable"));
                if total == 0 {
                    return Some(Reading::missing(Unit::Percent, "not found"));
                }
                let used = total.saturating_sub(available);
                let mut r = Reading::of(vec![100.0 * used as f64 / total as f64], vec!["Memory".into()], Unit::Percent);
                r.note = Some(format!("{} of {}", size_text(used * 1024), size_text(total * 1024)));
                if *swap {
                    let (st, sf) = (kb("SwapTotal"), kb("SwapFree"));
                    r.values.push(if st == 0 { 0.0 } else { 100.0 * st.saturating_sub(sf) as f64 / st as f64 });
                    r.names.push("Swap".into());
                }
                Some(r)
            }
            Measure::Gpu { pci } => self.gpu(pci.as_deref(), elapsed),
            Measure::Disk { device, read: reads, write: writes } => {
                let stats = read("/proc/diskstats");
                let wanted: Vec<String> = match device {
                    Some(d) => vec![d.clone()],
                    None => disks().into_iter().map(|(d, _)| d).collect(),
                };
                // Sectors read and written (512 bytes each, whatever the disk's).
                let (mut r, mut w) = (0u64, 0u64);
                let mut found = false;
                for l in stats.lines() {
                    let f: Vec<&str> = l.split_whitespace().collect();
                    if f.len() > 9 && wanted.iter().any(|d| d == f[2]) {
                        found = true;
                        r += f[5].parse::<u64>().unwrap_or(0);
                        w += f[9].parse::<u64>().unwrap_or(0);
                    }
                }
                if !found {
                    return Some(Reading::missing(Unit::BytesPerSecond, "not found"));
                }
                let (dr, dw) = self.rates("disk", (r * 512, w * 512), elapsed)?;
                let mut out = Reading::of(Vec::new(), Vec::new(), Unit::BytesPerSecond);
                if *reads || !*writes {
                    out.values.push(dr);
                    out.names.push("Read".into());
                }
                if *writes {
                    out.values.push(dw);
                    out.names.push("Write".into());
                }
                Some(out)
            }
            Measure::Space { mount } => {
                let Some((total, free)) = statvfs(mount) else { return Some(Reading::missing(Unit::Percent, "not mounted")) };
                if total == 0 {
                    return Some(Reading::missing(Unit::Percent, "not mounted"));
                }
                let mut r = Reading::of(vec![100.0 * (total - free.min(total)) as f64 / total as f64], vec!["Used".into()], Unit::Percent);
                r.note = Some(format!("{} free of {}", size_text(free), size_text(total)));
                Some(r)
            }
            Measure::Network { interface, down, up } => {
                let dev = read("/proc/net/dev");
                let (mut rx, mut tx) = (0u64, 0u64);
                let mut found = false;
                for l in dev.lines().skip(2) {
                    let Some((name, rest)) = l.split_once(':') else { continue };
                    let name = name.trim();
                    let take = match interface {
                        Some(i) => name == i,
                        None => name != "lo",
                    };
                    if !take {
                        continue;
                    }
                    let f: Vec<u64> = rest.split_whitespace().filter_map(|v| v.parse().ok()).collect();
                    if f.len() > 8 {
                        found = true;
                        rx += f[0];
                        tx += f[8];
                    }
                }
                if !found {
                    return Some(Reading::missing(Unit::BytesPerSecond, "not found"));
                }
                let (drx, dtx) = self.rates("net", (rx, tx), elapsed)?;
                let mut out = Reading::of(Vec::new(), Vec::new(), Unit::BytesPerSecond);
                if *down || !*up {
                    out.values.push(drx);
                    out.names.push("Down".into());
                }
                if *up {
                    out.values.push(dtx);
                    out.names.push("Up".into());
                }
                Some(out)
            }
            Measure::Temperature { sensor } => {
                let all = sensors();
                let s = match sensor {
                    Some(k) => all.iter().find(|s| &s.key == k),
                    None => processor_sensor(&all),
                };
                let Some(s) = s else { return Some(Reading::missing(Unit::Celsius, "not found")) };
                let milli: f64 = read(&s.input).trim().parse().unwrap_or(f64::NAN);
                if milli.is_nan() {
                    return Some(Reading::missing(Unit::Celsius, "not answering"));
                }
                Some(Reading::of(vec![milli / 1000.0], vec![s.name.clone()], Unit::Celsius))
            }
            Measure::Pool { .. } => {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
                Some(pool_reading(PoolMeters::load().as_ref(), measure, now))
            }
            Measure::Power => {
                let Some(bat) = battery_dir() else { return Some(Reading::missing(Unit::Watts, "no battery")) };
                let status = read(bat.join("status"));
                let microwatts = match read(bat.join("power_now")).trim().parse::<f64>() {
                    Ok(p) => p,
                    // (A battery that says its current and voltage instead.)
                    Err(_) => read(bat.join("current_now")).trim().parse::<f64>().unwrap_or(0.0) * read(bat.join("voltage_now")).trim().parse::<f64>().unwrap_or(0.0) / 1e6,
                };
                let mut r = Reading::of(vec![microwatts / 1e6], vec!["Power".into()], Unit::Watts);
                if status.trim() != "Discharging" {
                    r.note = Some(if status.trim() == "Charging" { "charging".into() } else { "on power".into() });
                }
                Some(r)
            }
        }
    }

    /// A pair of running counters' rates a second since the last reading
    /// (None for the first).
    fn rates(&mut self, key: &str, now: (u64, u64), elapsed: Option<f64>) -> Option<(f64, f64)> {
        let before = self.counters.insert(key.to_owned(), now);
        let (b, s) = (before?, elapsed?);
        Some((now.0.saturating_sub(b.0) as f64 / s, now.1.saturating_sub(b.1) as f64 / s))
    }

    fn gpu(&mut self, pci: Option<&str>, elapsed: Option<f64>) -> Option<Reading> {
        if self.gpu.as_ref().is_none_or(|g| pci.is_some_and(|p| pci_of(g) != p)) {
            let found = gpu_load::find();
            self.gpu = match pci {
                Some(p) => found.into_iter().find(|g| pci_of(g) == p),
                None => found.into_iter().next(),
            };
            self.gpu_idle = None;
        }
        let Some(g) = self.gpu.clone() else { return Some(Reading::missing(Unit::Percent, "not found")) };
        let busy = |v: f64| Some(Reading::of(vec![v.clamp(0.0, 100.0)], vec![g.name.clone()], Unit::Percent));
        match &g.kind {
            // (Asleep: never asked, which would wake it.)
            _ if gpu_load::asleep(&g.device) => Some(Reading { note: Some("asleep".into()), ..Reading::of(vec![0.0], vec![g.name.clone()], Unit::Percent) }),
            Kind::Xe { gts } => {
                // Each GT's idle time against the time gone by (a GT
                // asleep counts as idle): the render one's and the media
                // one's, as busy as the busier (a video decoding on the
                // media GT is the GPU at work too).
                let idle: Vec<u64> = gts.iter().map(|gt| gpu_load::xe_idle_ms(gt)).collect();
                let before = self.gpu_idle.replace(idle.clone());
                let (b, s) = (before.filter(|b| b.len() == idle.len())?, elapsed?);
                let each: Vec<f64> = idle.iter().zip(&b).map(|(now, was)| 100.0 - 100.0 * now.saturating_sub(*was) as f64 / (s * 1000.0)).collect();
                busy(gpu_load::busiest(&each))
            }
            // (Its graphics engine, its encoder and its decoder: as
            // busy as the busiest.)
            Kind::Nvidia { bus } => gpu_load::nvidia_now(&g.device, bus).map_or_else(|| Some(Reading::missing(Unit::Percent, "not answering")), |n| busy(n.total())),
            Kind::Amd => read(g.device.join("gpu_busy_percent")).trim().parse::<f64>().ok().map_or_else(|| Some(Reading::missing(Unit::Percent, "not answering")), busy),
            Kind::Agx => Some(Reading::missing(Unit::Percent, "not measured")),
        }
    }
}

/// Each processor line's busy and total time ("cpu", then each core).
fn cpu_lines(stat: &str) -> Vec<(u64, u64)> {
    stat.lines()
        .filter(|l| l.starts_with("cpu"))
        .map(|l| {
            let f: Vec<u64> = l.split_whitespace().skip(1).filter_map(|v| v.parse().ok()).collect();
            // user nice system idle iowait irq softirq steal (guest time is
            // in user's already).
            let total: u64 = f.iter().take(8).sum();
            let idle = f.get(3).copied().unwrap_or(0) + f.get(4).copied().unwrap_or(0);
            (total.saturating_sub(idle), total)
        })
        .collect()
}

/// A size in bytes, short: "11.2 GB".
fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 || v >= 100.0 { format!("{v:.0} {}", UNITS[i]) } else { format!("{v:.1} {}", UNITS[i]) }
}

/// The whole disks (not their partitions, loop, RAM or mapped devices),
/// and what to call each: its model, else its name.
fn disks() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::fs::read_dir("/sys/block")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !["loop", "ram", "zram", "dm-", "md", "sr", "fd"].iter().any(|p| n.starts_with(p)))
        .map(|n| {
            let model = read(format!("/sys/block/{n}/device/model")).trim().to_owned();
            let label = if model.is_empty() { n.clone() } else { format!("{model} ({n})") };
            (n, label)
        })
        .collect();
    out.sort();
    out
}

/// The volumes mounted from disks (each device once, at its first mount
/// point), and what to call each.
fn volumes() -> Vec<(String, String)> {
    const REAL: [&str; 11] = ["ext2", "ext3", "ext4", "btrfs", "xfs", "vfat", "exfat", "ntfs3", "f2fs", "bcachefs", "zfs"];
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for l in read("/proc/mounts").lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 3 || !REAL.contains(&f[2]) || seen.iter().any(|d| d == f[0]) {
            continue;
        }
        seen.push(f[0].to_owned());
        let mount = f[1].replace("\\040", " ");
        let label = match mount.as_str() {
            "/" => "System (/)".to_owned(),
            m => m.rsplit('/').next().filter(|n| !n.is_empty()).map_or_else(|| m.to_owned(), |n| format!("{n} ({m})")),
        };
        out.push((mount, label));
    }
    out
}

/// The network interfaces but loopback, and what to call each.
fn interfaces() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::fs::read_dir("/sys/class/net")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "lo")
        .map(|n| {
            let base = Path::new("/sys/class/net").join(&n);
            let kind = if base.join("wireless").exists() || base.join("phy80211").exists() {
                "Wi-Fi"
            } else if base.join("device").exists() {
                "Ethernet"
            } else {
                "virtual"
            };
            (n.clone(), format!("{n} ({kind})"))
        })
        .collect();
    out.sort();
    out
}

struct Sensor {
    key: String,
    name: String,
    input: PathBuf,
}

/// Every hwmon temperature: its key ("coretemp/Package id 0"), what to
/// call it, and where it's read.
fn sensors() -> Vec<Sensor> {
    let mut out = Vec::new();
    let mut chips: Vec<PathBuf> = std::fs::read_dir("/sys/class/hwmon").into_iter().flatten().flatten().map(|e| e.path()).collect();
    chips.sort();
    for chip in chips {
        let name = read(chip.join("name")).trim().to_owned();
        let mut inputs: Vec<PathBuf> = std::fs::read_dir(&chip)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("temp") && n.to_string_lossy().ends_with("_input")))
            .collect();
        inputs.sort_by_key(|p| p.file_name().and_then(|n| n.to_string_lossy().trim_start_matches("temp").trim_end_matches("_input").parse::<u32>().ok()).unwrap_or(0));
        for input in inputs {
            let stem = input.file_name().map(|n| n.to_string_lossy().trim_end_matches("_input").to_owned()).unwrap_or_default();
            let label = read(chip.join(format!("{stem}_label"))).trim().to_owned();
            let label = if label.is_empty() { stem.clone() } else { label };
            let shown = match name.as_str() {
                "coretemp" | "k10temp" | "zenpower" => format!("Processor: {label}"),
                "nvme" => format!("NVMe disk: {label}"),
                "amdgpu" | "nouveau" | "xe" | "i915" => format!("GPU: {label}"),
                _ => format!("{name}: {label}"),
            };
            out.push(Sensor { key: format!("{name}/{label}"), name: shown, input });
        }
    }
    out
}

/// The processor's temperature: its package (Intel), its control
/// temperature (AMD), else the first of its chip's.
fn processor_sensor(all: &[Sensor]) -> Option<&Sensor> {
    ["coretemp/Package id 0", "k10temp/Tctl", "zenpower/Tdie"]
        .iter()
        .find_map(|k| all.iter().find(|s| s.key == *k))
        .or_else(|| all.iter().find(|s| s.key.starts_with("coretemp/") || s.key.starts_with("k10temp/")))
}

fn battery_dir() -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir("/sys/class/power_supply")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| read(p.join("type")).trim() == "Battery" && read(p.join("scope")).trim() != "Device")
        .collect();
    found.sort();
    found.into_iter().next()
}

/// A volume's size and free space, in bytes.
fn statvfs(path: &str) -> Option<(u64, u64)> {
    let c = std::ffi::CString::new(path).ok()?;
    // SAFETY: zeroed is a valid statvfs to fill.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: a NUL-terminated path and a statvfs to fill.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let frag = st.f_frsize as u64;
    Some((st.f_blocks as u64 * frag, st.f_bavail as u64 * frag))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Token Pool widget's reading: two Claude accounts' 5-hour quota
    /// together, each, used rather than left, the one answering, out on
    /// both (when one's back), and nothing while the pool isn't running.
    #[test]
    fn a_pool_reading() {
        use olo_protocol::pool::{PoolAccount, PoolWindow};
        let now = 10_000;
        let account = |name: &str, used: f64, resets: i64, in_flight: u32| PoolAccount {
            id: name.to_lowercase(),
            name: name.into(),
            provider: "claude".into(),
            enabled: true,
            in_flight,
            last_used: Some(now - 60),
            windows: vec![PoolWindow { name: "five_hour".into(), label: "5-hour".into(), used, resets_at: Some(now + resets), ..Default::default() }],
            ..Default::default()
        };
        let mut m = PoolMeters { pid: 1, loaded: true, accounts: vec![account("Work", 30.0, 3_600, 1), account("Home", 100.0, 7_200, 0)], paused: Vec::new() };
        let mut measure = Measure::pool("claude", &["5-hour"]);
        let r = pool_reading(Some(&m), &measure, now);
        assert_eq!((r.values.len(), r.words.as_deref(), r.left.map(|l| (l * 100.0).round())), (1, Some("35% left"), Some(35.0)));
        assert!(!r.detail.is_empty() && r.note.is_none(), "{r:?}");
        if let Measure::Pool { view, used, .. } = &mut measure {
            (*view, *used) = (PoolView::Each, true);
        }
        let r = pool_reading(Some(&m), &measure, now);
        assert_eq!((r.names.clone(), r.values.clone(), r.words.as_deref()), (vec!["Work".to_owned(), "Home".to_owned()], vec![30.0, 100.0], Some("65% used")));
        if let Measure::Pool { view, used, .. } = &mut measure {
            (*view, *used) = (PoolView::Serving, false);
        }
        assert_eq!(pool_reading(Some(&m), &measure, now).words.as_deref(), Some("Work: 70% left"));
        // Out on both: when the first is back.
        m.accounts[0].windows[0].used = 100.0;
        measure = Measure::pool("claude", &["5-hour"]);
        let r = pool_reading(Some(&m), &measure, now);
        assert_eq!((r.left, r.note.as_deref()), (Some(0.0), Some("Work back in 1 h")));
        // Two quotas: a series each, the least left warning.
        m.accounts[0].windows[0].used = 30.0;
        for (a, used) in m.accounts.iter_mut().zip([40.0, 60.0]) {
            a.windows.push(PoolWindow { name: "seven_day".into(), label: "Weekly".into(), used, resets_at: Some(now + 100_000), ..Default::default() });
        }
        let measure = Measure::pool("claude", &["5-hour", "Weekly"]);
        let r = pool_reading(Some(&m), &measure, now);
        assert_eq!((r.names.clone(), r.values.clone(), r.words.as_deref()), (vec!["5-hour".to_owned(), "Weekly".to_owned()], vec![35.0, 50.0], Some("35% · 50% left")));
        assert_eq!(r.left.map(|l| (l * 100.0).round()), Some(35.0));
        assert!(r.detail[0].starts_with("5-hour: 35% left"), "{:?}", r.detail);
        // Not running.
        let r = pool_reading(None, &measure, now);
        assert!(r.values.is_empty() && r.note.as_deref() == Some("Token Pool isn't running"));
    }

    #[test]
    fn processor_lines() {
        let stat = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 60 0 20 400 20 0 0 0 0 0\ncpu1 40 0 30 400 30 0 0 0 0 0\nintr 1 2 3\n";
        // Busy: user, nice, system, irq, softirq, steal; not idle or iowait.
        assert_eq!(cpu_lines(stat), vec![(150, 1000), (80, 500), (70, 500)]);
    }

    #[test]
    fn values_in_words() {
        assert_eq!(rate_text(980.0), "980 B/s");
        assert_eq!(rate_text(2_140_000.0), "2.1 MB/s");
        assert_eq!(size_text(11_200_000_000), "11.2 GB");
        let net = Measure::Network { interface: None, down: true, up: true };
        let r = Reading::of(vec![1_200_000.0, 40_000.0], vec!["Down".into(), "Up".into()], Unit::BytesPerSecond);
        assert_eq!(value_text(&net, &r), "↓ 1.2 MB/s ↑ 40.0 KB/s");
        let cores = Measure::Cpu { cores: true };
        assert_eq!(value_text(&cores, &Reading::of(vec![10.0, 30.0], vec![], Unit::Percent)), "20%");
        let gpu = Measure::Gpu { pci: None };
        assert_eq!(value_text(&gpu, &Reading::missing(Unit::Percent, "asleep")), "asleep");
        // (Levels: percent of 100, rates of the history's top, degrees
        // from 20 to 100.)
        assert_eq!(r.level(600_000.0, 1_200_000.0), 0.5);
        assert_eq!(Reading::of(vec![], vec![], Unit::Celsius).level(60.0, 0.0), 0.5);
    }

    /// This computer's sources, and a reading of each kind
    /// (`cargo test -p olo-system meters -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn live() {
        let s = sources();
        eprintln!("{s:#?}");
        let mut readers: Vec<(Measure, Reader)> = Measure::kinds().into_iter().map(|m| (m, Reader::new())).collect();
        for _ in 0..2 {
            for (m, r) in &mut readers {
                let got = r.read(m);
                eprintln!("{}: {:?} {}", describe(m, &s), got.as_ref().map(|g| &g.values), got.as_ref().map(|g| value_text(m, g)).unwrap_or_default());
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }
}
