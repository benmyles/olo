//! Tuning each GPU: cooler and quieter, or faster and hotter. What each
//! allows ([`Caps`]: read without root), the tuning kept for each
//! (`/etc/olo/gpus.json`, [`Tuning`]: the computer's, as the hardware
//! is) and applying it ([`apply`]: root, by `olo-gpu-tune`, which
//! Preferences ▸ GPUs runs through polkit and udev runs as each GPU
//! appears, at every start).
//!
//! - Intel's xe: each GT's lowest and highest clock (`freq0/min_freq`,
//!   `max_freq`, within `rpn_freq`..`rp0_freq`) and its power profile
//!   (`power_profile`: base or power_saving).
//! - NVIDIA's, through NVML (its library, in process): the power limit
//!   (within its constraints), the highest GPU clock it may run at
//!   (locked clocks: lowest to that), and persistence (the driver kept
//!   ready: it then never sleeps). Asking NVIDIA's library wakes the GPU,
//!   so its [`Caps`] are read only when asked for ([`caps`] with
//!   `wake`), and kept ([`Caps::load_cached`]).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where the tuning is kept (root's: it's the computer's).
pub const FILE: &str = "/etc/olo/gpus.json";

/// A tuning chosen at once: each GPU's knobs set for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Preset {
    /// Slower, cooler and quieter, the battery lasting longer.
    Cooler,
    /// As it came: the driver's own.
    #[default]
    Balanced,
    /// As fast as it goes, hotter and louder.
    Fastest,
    /// Knobs set by hand.
    Custom,
}

impl Preset {
    pub const CHOSEN: [Preset; 3] = [Preset::Cooler, Preset::Balanced, Preset::Fastest];

    pub fn name(self) -> &'static str {
        match self {
            Preset::Cooler => "Cooler and Quieter",
            Preset::Balanced => "Balanced",
            Preset::Fastest => "Fastest",
            Preset::Custom => "Custom",
        }
    }
}

/// One GPU's tuning: None for a knob left as the driver has it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GpuTune {
    /// Its PCI address: the same from one start to the next.
    pub pci: String,
    pub preset: Preset,
    /// xe: the render GT's lowest and highest clock (MHz).
    pub min_mhz: Option<u32>,
    pub max_mhz: Option<u32>,
    /// xe: every GT's power profile is power_saving.
    pub power_saving: Option<bool>,
    /// NVIDIA: its power limit (W).
    pub power_limit_w: Option<u32>,
    /// NVIDIA: the highest GPU clock (MHz), lowest to it locked.
    pub max_gpu_mhz: Option<u32>,
    /// NVIDIA: its driver kept ready (never sleeps).
    pub persistent: Option<bool>,
}

impl GpuTune {
    /// Nothing changed from the driver's own.
    pub fn is_default(&self) -> bool {
        GpuTune { pci: self.pci.clone(), preset: self.preset, ..Default::default() } == *self
    }
}

/// Every GPU's tuning.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Tuning {
    pub gpus: Vec<GpuTune>,
    /// Each GPU's limits as its driver had them, noted before it was
    /// first tuned (`olo-gpu-tune set`): what Balanced puts back. (xe
    /// keeps what's written to it, and its own lowest clock isn't one it
    /// says: not its efficient clock, which moves.)
    pub driver: Vec<DriverLimits>,
}

/// The preset a power mode puts every GPU on (the power mode is the
/// computer's one setting for speed against battery: the processors,
/// the firmware and the GPUs, integrated and discrete, all follow it;
/// `olo-gpu-tune follow`): Power Saver's cooler and quieter, Balanced's
/// the driver's own, Performance's fastest.
pub fn for_mode(mode: crate::power_profile::Mode) -> Preset {
    use crate::power_profile::Mode;
    match mode {
        Mode::PowerSaver => Preset::Cooler,
        Mode::Balanced => Preset::Balanced,
        Mode::Performance => Preset::Fastest,
    }
}

/// `kept` with every GPU (its PCI address and what it allows) on
/// `preset`, one set by hand too (it stays so only till the mode next
/// changes); None if that changes nothing.
pub fn following(kept: &Tuning, gpus: &[(String, Caps)], preset: Preset) -> Option<Tuning> {
    let mut new = kept.clone();
    for (pci, caps) in gpus {
        new.set(self::preset(pci, preset, caps));
    }
    (new != *kept).then_some(new)
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DriverLimits {
    pub pci: String,
    pub min_mhz: Option<u32>,
    pub max_mhz: Option<u32>,
}

impl Tuning {
    /// The tuning kept (none: every GPU as its driver has it).
    pub fn load() -> Tuning {
        Self::load_from(Path::new(FILE))
    }

    pub fn load_from(path: &Path) -> Tuning {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn of(&self, pci: &str) -> GpuTune {
        self.gpus.iter().find(|g| g.pci == pci).cloned().unwrap_or_else(|| GpuTune { pci: pci.to_owned(), ..Default::default() })
    }

    pub fn set(&mut self, tune: GpuTune) {
        self.gpus.retain(|g| g.pci != tune.pci);
        if !(tune.is_default() && tune.preset == Preset::Balanced) {
            self.gpus.push(tune);
        }
        self.gpus.sort_by(|a, b| a.pci.cmp(&b.pci));
    }
}

/// What a GPU allows, and how it's set now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Caps {
    Xe(XeCaps),
    Nvidia(NvidiaCaps),
    /// Nothing Olo can tune (yet).
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XeCaps {
    /// The lowest clock it can run at, its efficient one and its highest
    /// (MHz: `rpn`, `rpe`, `rp0`).
    pub lowest: u32,
    pub efficient: u32,
    pub highest: u32,
    /// The render GT's limits now.
    pub min_now: u32,
    pub max_now: u32,
    /// Whether the power profiles can be chosen, and power_saving is now.
    pub profiles: bool,
    pub power_saving_now: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvidiaCaps {
    /// Power limits (W): least, most, the driver's, now.
    pub power: Option<(u32, u32, u32, u32)>,
    /// Its GPU clock at rest and at its highest (MHz).
    pub min_gpu_mhz: u32,
    pub max_gpu_mhz: u32,
    pub persistent_now: Option<bool>,
}

fn read(p: impl AsRef<Path>) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

fn num(p: impl AsRef<Path>) -> Option<u32> {
    read(p).trim().parse().ok()
}

/// An xe GPU's GTs' frequency folders (render first).
fn xe_gts(device: &Path) -> Vec<PathBuf> {
    let mut gts: Vec<PathBuf> = std::fs::read_dir(device.join("tile0")).into_iter().flatten().flatten().map(|e| e.path().join("freq0")).filter(|p| p.join("max_freq").exists()).collect();
    gts.sort();
    gts
}

fn xe_caps(device: &Path) -> Option<XeCaps> {
    let gt = xe_gts(device).into_iter().next()?;
    let profile = read(gt.join("power_profile"));
    Some(XeCaps {
        lowest: num(gt.join("rpn_freq"))?,
        efficient: num(gt.join("rpe_freq")).unwrap_or(0),
        highest: num(gt.join("rp0_freq"))?,
        min_now: num(gt.join("min_freq"))?,
        max_now: num(gt.join("max_freq"))?,
        profiles: !profile.trim().is_empty(),
        power_saving_now: profile.contains("[power_saving]"),
    })
}

/// NVIDIA's library, loaded once (None without NVIDIA's driver).
fn nvml() -> Option<&'static nvml_wrapper::Nvml> {
    static NVML: std::sync::OnceLock<Option<nvml_wrapper::Nvml>> = std::sync::OnceLock::new();
    NVML.get_or_init(|| nvml_wrapper::Nvml::init().ok()).as_ref()
}

/// Its PCI address as NVML has it ("00000000:01:00.0").
fn nvml_bus(pci: &str) -> String {
    format!("0000{pci}")
}

fn nvidia_caps(pci: &str) -> Option<NvidiaCaps> {
    use nvml_wrapper::enum_wrappers::device::Clock;
    let d = nvml()?.device_by_pci_bus_id(nvml_bus(pci)).ok()?;
    let power = (|| {
        let c = d.power_management_limit_constraints().ok()?;
        let w = |mw: u32| (mw + 500) / 1000;
        // (A laptop's tells a range but can't be given a limit, the
        // platform's to set: then its limit can't be read either.)
        let now = d.power_management_limit().ok()?;
        Some((w(c.min_limit), w(c.max_limit), w(d.power_management_limit_default().ok()?), w(now)))
    })();
    Some(NvidiaCaps { power, min_gpu_mhz: nvidia_rest_mhz(&d), max_gpu_mhz: d.max_clock_info(Clock::Graphics).unwrap_or(0), persistent_now: d.is_in_persistent_mode().ok() })
}

/// An NVIDIA GPU's clock at rest (its lowest performance state's; the
/// supported clocks' list is `[0]` on a laptop's).
fn nvidia_rest_mhz(d: &nvml_wrapper::Device) -> u32 {
    use nvml_wrapper::enum_wrappers::device::{Clock, PerformanceState};
    [PerformanceState::Eight, PerformanceState::Twelve, PerformanceState::Five]
        .into_iter()
        .find_map(|p| d.min_max_clock_of_pstate(Clock::Graphics, p).ok().map(|(min, _)| min).filter(|m| *m > 0))
        .unwrap_or(210)
}

/// Whether a GPU is asleep (runtime-suspended): asking NVIDIA's would
/// wake it.
pub fn asleep(gpu: &crate::gpus::Gpu) -> bool {
    read(gpu.sysfs.join("power/runtime_status")).trim() == "suspended"
}

/// What `gpu` allows. NVIDIA's only with `wake` or while it's awake
/// (asking wakes it); else what was last read ([`Caps::load_cached`]).
pub fn caps(gpu: &crate::gpus::Gpu, wake: bool) -> Option<Caps> {
    match gpu.driver.as_str() {
        "xe" => xe_caps(&gpu.sysfs).map(Caps::Xe),
        "nvidia" if wake || !asleep(gpu) => {
            let caps = nvidia_caps(&gpu.pci).map(Caps::Nvidia);
            if let Some(c) = &caps {
                c.cache(&gpu.pci);
            }
            caps
        }
        "nvidia" => Caps::load_cached(&gpu.pci),
        _ => Some(Caps::None),
    }
}

impl Caps {
    fn cache_file(pci: &str) -> Option<PathBuf> {
        Some(olo_xdg_cache()?.join("olo/gpus").join(format!("{}.json", pci.replace(':', "_"))))
    }

    fn cache(&self, pci: &str) {
        if let Some(f) = Self::cache_file(pci) {
            let _ = f.parent().map(std::fs::create_dir_all);
            let _ = serde_json::to_vec(self).map(|j| std::fs::write(f, j));
        }
    }

    /// What was read last time the GPU was asked (it sleeps now).
    pub fn load_cached(pci: &str) -> Option<Caps> {
        serde_json::from_slice(&std::fs::read(Self::cache_file(pci)?).ok()?).ok()
    }
}

fn olo_xdg_cache() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
}

/// Rounded to 15 MHz (NVIDIA's clock steps).
pub fn round15(mhz: u32) -> u32 {
    (mhz + 7) / 15 * 15
}

/// Rounded to 50 MHz (xe's steps are 50/3; it rounds what it's given).
fn round50(mhz: u32) -> u32 {
    (mhz + 25) / 50 * 50
}

/// A preset's knobs for a GPU that allows `caps`.
pub fn preset(pci: &str, preset: Preset, caps: &Caps) -> GpuTune {
    let mut t = GpuTune { pci: pci.to_owned(), preset, ..Default::default() };
    match (preset, caps) {
        (Preset::Cooler, Caps::Xe(x)) => {
            // Up to halfway between its efficient clock and its highest,
            // and the power-saving profile.
            t.max_mhz = Some(round50(x.efficient.max(x.lowest) + (x.highest.saturating_sub(x.efficient.max(x.lowest))) / 2));
            t.power_saving = x.profiles.then_some(true);
        }
        (Preset::Fastest, Caps::Xe(x)) => {
            // Never below halfway up; as high as it goes.
            t.min_mhz = Some(round50(x.efficient.max(x.lowest) + (x.highest.saturating_sub(x.efficient.max(x.lowest))) / 2));
            t.max_mhz = Some(x.highest);
            t.power_saving = x.profiles.then_some(false);
        }
        (Preset::Cooler, Caps::Nvidia(n)) => {
            // Its least power, and 70 % of its clock.
            // Its power limit two thirds of the driver's (no lower than
            // it allows), and 70 % of its highest clock.
            t.power_limit_w = n.power.map(|(least, _, default, _)| (default * 2 / 3).max(least));
            if n.max_gpu_mhz > 0 {
                t.max_gpu_mhz = Some(round15(n.max_gpu_mhz * 7 / 10).max(n.min_gpu_mhz));
            }
        }
        (Preset::Fastest, Caps::Nvidia(n)) => {
            t.power_limit_w = n.power.map(|(_, most, ..)| most);
        }
        _ => {}
    }
    t
}

/// What applying went like, a line a knob.
pub type Applied = Vec<Result<String, String>>;

/// Apply `tune` to `gpu` (root). Knobs left as the driver has them are
/// put back to the driver's own (`driver`, as noted before it was first
/// tuned).
pub fn apply(gpu: &crate::gpus::Gpu, tune: &GpuTune, driver: Option<&DriverLimits>) -> Applied {
    match gpu.driver.as_str() {
        "xe" => apply_xe(&gpu.sysfs, tune, driver),
        "nvidia" => apply_nvidia(&gpu.pci, tune),
        _ => Vec::new(),
    }
}

fn write(path: &Path, value: &str) -> Result<String, String> {
    std::fs::write(path, value).map(|()| format!("{} = {value}", path.display())).map_err(|e| format!("{}: {e}", path.display()))
}

/// The limits a GPU's driver has now (xe's render GT's), to note before
/// it's first tuned.
pub fn driver_limits(gpu: &crate::gpus::Gpu) -> DriverLimits {
    let gt = (gpu.driver == "xe").then(|| xe_gts(&gpu.sysfs).into_iter().next()).flatten();
    DriverLimits { pci: gpu.pci.clone(), min_mhz: gt.as_ref().and_then(|g| num(g.join("min_freq"))), max_mhz: gt.as_ref().and_then(|g| num(g.join("max_freq"))) }
}

fn apply_xe(device: &Path, tune: &GpuTune, driver: Option<&DriverLimits>) -> Applied {
    let mut out = Vec::new();
    let gts = xe_gts(device);
    if let Some(gt) = gts.first() {
        let (lowest, efficient, highest) = (num(gt.join("rpn_freq")).unwrap_or(0), num(gt.join("rpe_freq")).unwrap_or(0), num(gt.join("rp0_freq")).unwrap_or(u32::MAX));
        // The driver's own: as it had them before it was tuned (else
        // from its efficient clock up to its highest).
        let min = tune.min_mhz.or(driver.and_then(|d| d.min_mhz)).unwrap_or(efficient.max(lowest)).clamp(lowest, highest);
        let max = tune.max_mhz.or(driver.and_then(|d| d.max_mhz)).unwrap_or(highest).clamp(min, highest);
        // (Each written so that min ≤ max holds between the two writes.)
        let now_max = num(gt.join("max_freq")).unwrap_or(highest);
        if min > now_max {
            out.push(write(&gt.join("max_freq"), &max.to_string()));
            out.push(write(&gt.join("min_freq"), &min.to_string()));
        } else {
            out.push(write(&gt.join("min_freq"), &min.to_string()));
            out.push(write(&gt.join("max_freq"), &max.to_string()));
        }
    }
    for gt in &gts {
        if gt.join("power_profile").exists() {
            out.push(write(&gt.join("power_profile"), if tune.power_saving == Some(true) { "power_saving" } else { "base" }));
        }
    }
    out
}

fn apply_nvidia(pci: &str, tune: &GpuTune) -> Applied {
    use nvml_wrapper::enums::device::GpuLockedClocksSetting;
    let Some(nvml) = nvml() else { return vec![Err("NVIDIA's library isn't there".into())] };
    let mut d = match nvml.device_by_pci_bus_id(nvml_bus(pci)) {
        Ok(d) => d,
        Err(e) => return vec![Err(format!("{pci}: {e}"))],
    };
    let mut out = Vec::new();
    let said = |what: String, r: Result<(), nvml_wrapper::error::NvmlError>| r.map(|()| what.clone()).map_err(|e| format!("{what}: {e}"));
    // (Only where it can be given one: a laptop's can't, `nvidia_caps`.)
    let limit = match tune.power_limit_w {
        Some(w) => Some(w * 1000),
        None => d.power_management_limit_default().ok(),
    };
    if let Some(mw) = limit.filter(|_| d.power_management_limit().is_ok()) {
        out.push(said(format!("power limit {} W", mw / 1000), d.set_power_management_limit(mw)));
    }
    match tune.max_gpu_mhz {
        Some(max) => {
            let min = nvidia_rest_mhz(&d).min(max);
            out.push(said(format!("GPU clock {min}–{max} MHz"), d.set_gpu_locked_clocks(GpuLockedClocksSetting::Numeric { min_clock_mhz: min, max_clock_mhz: max })));
        }
        None => out.push(said("GPU clock as the driver has it".into(), d.reset_gpu_locked_clocks())),
    }
    out.push(said(format!("persistence {}", if tune.persistent == Some(true) { "on" } else { "off" }), d.set_persistent(tune.persistent == Some(true))));
    out
}

/// Apply the tuning kept to every GPU (or the one whose card is `card`,
/// "card1", as udev names it): root, at start and as it's changed. At
/// start (`untuned` false) a GPU nobody tuned is left as its driver
/// started it; as the tuning changes, one no longer tuned is put back to
/// its driver's own.
pub fn apply_kept(card: Option<&str>, untuned: bool) -> Vec<(String, Applied)> {
    let tuning = Tuning::load();
    crate::gpus::list()
        .into_iter()
        .filter(|g| card.is_none_or(|c| g.card.as_ref().and_then(|p| p.file_name()).is_some_and(|n| n == c)))
        .filter_map(|g| {
            let tune = tuning.gpus.iter().find(|t| t.pci == g.pci).cloned();
            let tune = tune.or_else(|| untuned.then(|| tuning.of(&g.pci)))?;
            let driver = tuning.driver.iter().find(|d| d.pci == g.pci);
            Some((g.name.clone(), apply(&g, &tune, driver)))
        })
        .collect()
}

/// `new` to keep, with each GPU's driver limits as `kept` noted them, or
/// as they are now for a GPU tuned for the first time.
pub fn with_driver_limits(mut new: Tuning, kept: &Tuning) -> Tuning {
    new.driver =
        crate::gpus::list().iter().map(|g| kept.driver.iter().find(|d| d.pci == g.pci).cloned().unwrap_or_else(|| driver_limits(g))).filter(|d| d.min_mhz.is_some() || d.max_mhz.is_some()).collect();
    new
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xe() -> Caps {
        Caps::Xe(XeCaps { lowest: 100, efficient: 700, highest: 2450, min_now: 700, max_now: 2450, profiles: true, power_saving_now: false })
    }

    /// The power mode's preset on every GPU, integrated and discrete, one
    /// set by hand too; nothing when they're on it already.
    #[test]
    fn the_power_mode_sets_the_gpus() {
        use crate::power_profile::Mode;
        let nv = Caps::Nvidia(NvidiaCaps { power: Some((5, 75, 60, 60)), min_gpu_mhz: 180, max_gpu_mhz: 3090, persistent_now: Some(false) });
        let gpus = vec![("0000:00:02.0".to_owned(), xe()), ("0000:01:00.0".to_owned(), nv)];
        let none = Tuning::default();
        let saving = following(&none, &gpus, for_mode(Mode::PowerSaver)).unwrap();
        assert!(saving.gpus.iter().all(|g| g.preset == Preset::Cooler) && saving.gpus.len() == 2, "{saving:?}");
        assert_eq!(saving.of("0000:01:00.0").power_limit_w, Some(40));
        assert_eq!(following(&saving, &gpus, Preset::Cooler), None, "on it already");
        // Back to Balanced: as the drivers have it (nothing kept).
        let balanced = following(&saving, &gpus, for_mode(Mode::Balanced)).unwrap();
        assert!(balanced.gpus.is_empty(), "{balanced:?}");
        // One set by hand goes with the mode too.
        let mut by_hand = none.clone();
        by_hand.set(GpuTune { pci: "0000:01:00.0".into(), preset: Preset::Custom, power_limit_w: Some(30), ..Default::default() });
        let fast = following(&by_hand, &gpus, for_mode(Mode::Performance)).unwrap();
        assert!(fast.gpus.iter().all(|g| g.preset == Preset::Fastest) && fast.gpus.len() == 2, "{fast:?}");
        assert_eq!(fast.of("0000:01:00.0").power_limit_w, Some(75));
    }

    #[test]
    fn presets() {
        let c = preset("0000:00:02.0", Preset::Cooler, &xe());
        assert_eq!((c.min_mhz, c.max_mhz, c.power_saving), (None, Some(1600), Some(true)));
        let f = preset("0000:00:02.0", Preset::Fastest, &xe());
        assert_eq!((f.min_mhz, f.max_mhz, f.power_saving), (Some(1600), Some(2450), Some(false)));
        assert!(preset("0000:00:02.0", Preset::Balanced, &xe()).is_default());
        // (The ThinkPad P1 Gen 9's RTX PRO 2000: 5–75 W, 60 by default.)
        let n = Caps::Nvidia(NvidiaCaps { power: Some((5, 75, 60, 60)), min_gpu_mhz: 210, max_gpu_mhz: 3090, persistent_now: Some(false) });
        let c = preset("0000:01:00.0", Preset::Cooler, &n);
        assert_eq!((c.power_limit_w, c.max_gpu_mhz), (Some(40), Some(2160)));
        assert_eq!(preset("0000:01:00.0", Preset::Fastest, &n).power_limit_w, Some(75));
    }

    #[test]
    fn kept_only_when_changed() {
        let mut t = Tuning::default();
        t.set(GpuTune { pci: "a".into(), ..Default::default() });
        assert!(t.gpus.is_empty());
        t.set(GpuTune { pci: "a".into(), preset: Preset::Cooler, max_mhz: Some(1600), ..Default::default() });
        assert_eq!(t.of("a").max_mhz, Some(1600));
        assert_eq!(t.of("b").preset, Preset::Balanced);
    }

    /// This computer's GPUs, what they allow (`cargo test -p olo-system
    /// gpu_tuning -- --ignored --nocapture`; wakes NVIDIA's).
    #[test]
    #[ignore]
    fn live_nvidia_detail() {
        use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureThreshold as T};
        let Some(nvml) = nvml() else { return };
        for g in crate::gpus::list().into_iter().filter(|g| g.driver == "nvidia") {
            let d = nvml.device_by_pci_bus_id(nvml_bus(&g.pci)).unwrap();
            eprintln!(
                "limit {:?} enforced {:?} default {:?} constraints {:?}",
                d.power_management_limit(),
                d.enforced_power_limit(),
                d.power_management_limit_default(),
                d.power_management_limit_constraints()
            );
            eprintln!("mem clocks {:?}", d.supported_memory_clocks());
            eprintln!("max clocks gfx {:?} mem {:?} sm {:?}", d.max_clock_info(Clock::Graphics), d.max_clock_info(Clock::Memory), d.max_clock_info(Clock::SM));
            for t in [T::Shutdown, T::Slowdown, T::GpuMax, T::AcousticMin, T::AcousticCurr, T::AcousticMax] {
                eprintln!("threshold {t:?}: {:?}", d.temperature_threshold(t));
            }
            eprintln!("pstate {:?} persistence {:?}", d.performance_state(), d.is_in_persistent_mode());
        }
    }

    #[test]
    #[ignore]
    fn live() {
        for g in crate::gpus::list() {
            eprintln!("{} ({}): {:?}", g.name, g.driver, caps(&g, true));
        }
    }
}
