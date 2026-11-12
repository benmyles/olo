//! Every GPU the computer has, and how busy each is (Activity Monitor's
//! GPU section and Deskbar's GPU widgets): Apple's AGX
//! (Asahi's `agx_stats`), AMD's (`gpu_busy_percent`, VRAM), Intel's xe
//! (each GT's idle residency, its clock) and NVIDIA's (NVML, its own
//! library, in process: busy, memory, power, temperature, clock). A GPU
//! asleep (runtime-suspended, as a hybrid laptop's NVIDIA one is while
//! nothing uses it) is shown asleep and never asked, which would wake it.

use std::path::{Path, PathBuf};

/// What kind of GPU, and what it is read through.
#[derive(Debug, Clone)]
pub enum Kind {
    Agx,
    Amd,
    /// Its GTs' idle residency files (render, then media).
    Xe {
        gts: Vec<PathBuf>,
    },
    /// Its PCI address as NVML names it ("00000000:01:00.0").
    Nvidia {
        bus: String,
    },
}

/// A GPU found, and the sources its samples go to: `prefix` is "gpu" for
/// the first (as before there were more), "gpu1", "gpu2" after.
#[derive(Debug, Clone)]
pub struct Probe {
    pub device: PathBuf,
    pub kind: Kind,
    pub prefix: String,
    pub name: String,
    pub driver: String,
}

fn read(p: impl AsRef<Path>) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

/// The GPUs, the one the computer started on first.
pub fn find() -> Vec<Probe> {
    let mut found: Vec<(bool, Probe)> = Vec::new();
    let mut cards: Vec<PathBuf> = std::fs::read_dir("/sys/class/drm")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("card") && !n.contains('-')))
        .collect();
    cards.sort();
    for card in cards {
        let device = card.join("device");
        let Ok(device) = device.canonicalize() else { continue };
        if found.iter().any(|(_, p)| p.device == device) {
            continue;
        }
        let driver = std::fs::read_link(device.join("driver")).ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
        let kind = if device.join("agx_stats").exists() {
            Kind::Agx
        } else if device.join("gpu_busy_percent").exists() {
            Kind::Amd
        } else if driver == "xe" {
            let mut gts: Vec<PathBuf> = std::fs::read_dir(device.join("tile0")).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.join("gtidle/idle_residency_ms").exists()).collect();
            gts.sort();
            Kind::Xe { gts }
        } else if driver == "nvidia" {
            let bus = device.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            Kind::Nvidia { bus: format!("0000{bus}") }
        } else {
            continue;
        };
        let boot = read(device.join("boot_vga")).trim() == "1";
        let name = name_of(&device, &kind, &driver);
        found.push((boot, Probe { device, kind, prefix: String::new(), name, driver }));
    }
    found.sort_by_key(|(boot, _)| !*boot);
    found
        .into_iter()
        .enumerate()
        .map(|(i, (_, mut p))| {
            p.prefix = if i == 0 { "gpu".into() } else { format!("gpu{i}") };
            p
        })
        .collect()
}

/// What to call a GPU: its maker and the name its driver or the PCI
/// database gives, else the driver's name.
fn name_of(device: &Path, kind: &Kind, driver: &str) -> String {
    let compatible = read(device.join("uevent")).lines().find_map(|l| l.strip_prefix("OF_COMPATIBLE_0=").map(str::to_owned)).unwrap_or_default();
    // (Never NVIDIA's library here: asking a GPU asleep wakes it.)
    let maker = match kind {
        Kind::Nvidia { .. } => "NVIDIA",
        Kind::Xe { .. } => "Intel",
        Kind::Amd => "AMD",
        Kind::Agx => "Apple",
    };
    match kind {
        Kind::Agx if !compatible.is_empty() => compatible,
        Kind::Agx => driver.to_owned(),
        _ => {
            let id = |f: &str| u16::from_str_radix(read(device.join(f)).trim().trim_start_matches("0x"), 16).unwrap_or(0);
            crate::gpus::pci_name(id("vendor"), id("device")).map_or_else(|| format!("{maker} GPU"), |model| if model.starts_with(maker) { model } else { format!("{maker} {model}") })
        }
    }
}

/// NVIDIA's library, the process's one (`gpu_tuning`'s: started again
/// at each ask until it starts once; None without NVIDIA's driver).
pub fn nvml() -> Option<&'static nvml_wrapper::Nvml> {
    crate::gpu_tuning::nvml().ok()
}

/// Whether the GPU is asleep (runtime-suspended): asking it would wake it.
pub fn asleep(device: &Path) -> bool {
    read(device.join("power/runtime_status")).trim() == "suspended"
}

/// How busy a GPU is, of how busy each of its engines is (%): as busy as
/// its busiest (they are separate hardware: a card encoding video with
/// its 3D engine idle is busy, and two at 60% aren't 120%). NaN where
/// none of them says.
pub fn busiest(engines: &[f64]) -> f64 {
    engines.iter().copied().filter(|v| v.is_finite()).fold(f64::NAN, f64::max)
}

/// An NVIDIA GPU's figures now: how busy its graphics and compute engine
/// is (%), and its video encoder and decoder, which its library counts
/// apart (a screen recording or a call encoding on the card read as an
/// idle GPU while only the first was asked: Ben, 2026-11-10); memory
/// used and in all, watts, °C, MHz. None if it's asleep or its library
/// can't say.
#[derive(Clone, Copy)]
pub struct NvidiaNow {
    pub busy: f64,
    pub encode: f64,
    pub decode: f64,
    pub memory_used: u64,
    pub memory_total: u64,
    pub watts: f64,
    pub celsius: f64,
    pub mhz: f64,
}

impl NvidiaNow {
    /// How busy the card is: its busiest engine ([`busiest`]).
    pub fn total(&self) -> f64 {
        busiest(&[self.busy, self.encode, self.decode])
    }
}

/// How long an idle NVIDIA GPU is left unasked: each question counts as
/// use, and one asked every second never sleeps (it slept about ten
/// seconds after the last). While it's busy it's asked every time.
const IDLE_ASK_EVERY: std::time::Duration = std::time::Duration::from_secs(20);
/// How long after it was last seen asleep it's asked every time.
const JUST_WOKEN: std::time::Duration = std::time::Duration::from_secs(8);

pub fn nvidia_now(device: &Path, bus: &str) -> Option<NvidiaNow> {
    use std::{collections::HashMap, sync::Mutex, time::Instant};
    static LAST: Mutex<Option<HashMap<String, (Instant, NvidiaNow)>>> = Mutex::new(None);
    static SLEPT: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
    let mut slept = SLEPT.lock().unwrap_or_else(|e| e.into_inner());
    let slept = slept.get_or_insert_with(HashMap::new);
    if asleep(device) {
        slept.insert(bus.to_owned(), Instant::now());
        return None;
    }
    // Just woken: something has begun to use it, and what it does shows
    // a moment after it wakes. Asked each time for a while, though it
    // says it's idle (a recording begun on a sleeping card read as
    // nothing for its first twenty seconds otherwise).
    let just_woken = slept.get(bus).is_some_and(|at| at.elapsed() < JUST_WOKEN);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let last = last.get_or_insert_with(HashMap::new);
    // Idle when last asked: what it said then, until it's time to ask
    // again (it may well be asleep by then).
    if !just_woken
        && let Some((at, was)) = last.get(bus)
        && was.total() < 1.0
        && at.elapsed() < IDLE_ASK_EVERY
    {
        return Some(NvidiaNow { ..*was });
    }
    let now = ask_nvidia(bus)?;
    NVIDIA_BUSY.lock().unwrap_or_else(|e| e.into_inner()).insert(bus.to_owned(), now.total() >= 1.0);
    last.insert(bus.to_owned(), (Instant::now(), NvidiaNow { ..now }));
    Some(now)
}

/// Whether the NVIDIA GPU was busy when last asked ([`nvidia_now`]):
/// only then are its users asked for (each question keeps it awake).
pub fn nvidia_busy(bus: &str) -> bool {
    NVIDIA_BUSY.lock().unwrap_or_else(|e| e.into_inner()).get(bus).copied().unwrap_or(false)
}

static NVIDIA_BUSY: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, bool>>> = std::sync::LazyLock::new(Default::default);

fn ask_nvidia(bus: &str) -> Option<NvidiaNow> {
    use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
    let d = nvml()?.device_by_pci_bus_id(bus).ok()?;
    let memory = d.memory_info().ok();
    Some(NvidiaNow {
        busy: d.utilization_rates().map_or(0.0, |u| u.gpu as f64),
        encode: d.encoder_utilization().map_or(0.0, |u| u.utilization as f64),
        decode: d.decoder_utilization().map_or(0.0, |u| u.utilization as f64),
        memory_used: memory.as_ref().map_or(0, |m| m.used),
        memory_total: memory.as_ref().map_or(0, |m| m.total),
        watts: d.power_usage().map_or(0.0, |mw| mw as f64 / 1000.0),
        celsius: d.temperature(TemperatureSensor::Gpu).map_or(0.0, |c| c as f64),
        mhz: d.clock_info(Clock::Graphics).map_or(0.0, |c| c as f64),
    })
}

/// An xe GT's idle time so far (ms), and its clock now (MHz).
pub fn xe_idle_ms(gt: &Path) -> u64 {
    read(gt.join("gtidle/idle_residency_ms")).trim().parse().unwrap_or(0)
}

pub fn xe_mhz(gt: &Path) -> f64 {
    read(gt.join("freq0/act_freq")).trim().parse().unwrap_or(0.0)
}

/// What a GT does, by its name ("gt0-rc": render and compute; "gt1-mc":
/// media).
pub fn xe_gt_name(gt: &Path) -> &'static str {
    if read(gt.join("gtidle/name")).trim().ends_with("-mc") { "media" } else { "render" }
}

#[cfg(test)]
mod tests {
    /// A GPU is as busy as its busiest engine: one encoding with its 3D
    /// engine idle is busy, and what an engine doesn't say isn't counted.
    #[test]
    fn as_busy_as_its_busiest_engine() {
        use super::{NvidiaNow, busiest};
        assert_eq!(busiest(&[3.0, 41.0, 0.0]), 41.0);
        assert_eq!(busiest(&[60.0, 60.0]), 60.0);
        assert_eq!(busiest(&[f64::NAN, 12.0]), 12.0);
        assert!(busiest(&[f64::NAN]).is_nan() && busiest(&[]).is_nan());
        let recording = NvidiaNow { busy: 2.0, encode: 38.0, decode: 0.0, memory_used: 0, memory_total: 0, watts: 0.0, celsius: 0.0, mhz: 0.0 };
        assert_eq!(recording.total(), 38.0);
    }

    /// This computer's GPUs (`cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn live() {
        for g in super::find() {
            eprintln!("{} {} ({}) {:?} asleep={}", g.prefix, g.name, g.driver, g.kind, super::asleep(&g.device));
        }
    }

    /// An NVIDIA GPU's engines, a line a second for `OLO_GPU_WATCH`
    /// seconds (10), while it's awake: run it beside something that
    /// encodes or decodes on the card (olo-video's ignored NVENC tests)
    /// to see the encoder's and decoder's figures move. It never wakes
    /// a sleeping card (`--ignored --nocapture`).
    #[test]
    #[ignore]
    fn watch_nvidia() {
        let seconds: u32 = std::env::var("OLO_GPU_WATCH").ok().and_then(|s| s.parse().ok()).unwrap_or(10);
        let Some((g, bus)) = super::find().into_iter().find_map(|g| match &g.kind {
            super::Kind::Nvidia { bus } => Some((g.clone(), bus.clone())),
            _ => None,
        }) else {
            return eprintln!("no NVIDIA GPU here");
        };
        for _ in 0..seconds {
            // (Asked every time, as `nvidia_now` asks a busy card: it
            // leaves an idle one alone for twenty seconds, which here
            // would be the whole watch.)
            match Some(()).filter(|()| !super::asleep(&g.device)).and_then(|()| super::ask_nvidia(&bus)) {
                Some(n) => eprintln!("graphics {:3.0}%  encoder {:3.0}%  decoder {:3.0}%  as busy as {:3.0}%  ({:.0} W)", n.busy, n.encode, n.decode, n.total(), n.watts),
                None => eprintln!("asleep (or its library doesn't answer)"),
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
}
