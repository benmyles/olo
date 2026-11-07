//! Tuning each GPU: cooler and quieter, or faster and hotter. What each
//! allows ([`Caps`]: read without root), the tuning kept for each
//! (`/etc/olo/gpus.json`, [`Tuning`]: the computer's, as the hardware
//! is) and applying it ([`apply`]: root, by `olo-gpu-tune`, which
//! Preferences ▸ Power runs through polkit and udev runs as each GPU
//! appears, at every start).
//!
//! - Intel's xe: each GT's lowest and highest clock (`freq0/min_freq`,
//!   `max_freq`, within `rpn_freq`..`rp0_freq`) and its power profile
//!   (`power_profile`: base or power_saving).
//! What each power mode, and running on battery, puts each GPU (and the
//! processors, `cpu_tuning`) on is the [`Plan`] (Preferences ▸ Power),
//! which `olo-gpu-tune follow` applies as the mode or the supply changes.
//!
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
    pub const ALL: [Preset; 4] = [Preset::Cooler, Preset::Balanced, Preset::Fastest, Preset::Custom];

    pub fn name(self) -> &'static str {
        match self {
            Preset::Cooler => "Cooler and Quieter",
            Preset::Balanced => "Balanced",
            Preset::Fastest => "Fastest",
            Preset::Custom => "Custom",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Preset::Cooler => "cooler",
            Preset::Balanced => "balanced",
            Preset::Fastest => "fastest",
            Preset::Custom => "custom",
        }
    }

    pub fn parse(s: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.id() == s)
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
    /// What each power mode, and running on battery, put the processors
    /// and each GPU on (Preferences ▸ Power).
    pub plan: Plan,
    /// Each GPU's knobs set by hand: what its Custom preset is.
    pub custom: Vec<GpuTune>,
}

/// When a preset is chosen for: a power mode, or running on battery
/// (whatever the mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    Mode(crate::power_profile::Mode),
    Battery,
}

impl When {
    pub const ALL: [When; 4] =
        [When::Mode(crate::power_profile::Mode::PowerSaver), When::Mode(crate::power_profile::Mode::Balanced), When::Mode(crate::power_profile::Mode::Performance), When::Battery];

    fn key(self) -> &'static str {
        match self {
            When::Mode(m) => m.id(),
            When::Battery => "battery",
        }
    }
}

/// What the power mode and the power supply put the processors and each
/// GPU on (Preferences ▸ Power ▸ Power Modes; `olo-gpu-tune follow`
/// applies it as either changes). Only choices that differ from the
/// defaults are kept (`chosen`: "MODE/cpu" or "MODE/PCI" to a preset's
/// id, MODE a mode's id or "battery"), so a GPU added later, or a
/// default changed, takes its default. The defaults: each mode's own
/// (Power Saver saving power, Balanced balanced, Performance fastest),
/// and on battery the mode's, but NVIDIA's GPU on Cooler and Quieter
/// (Ben, 2026-11-06: video wakes it on battery too, then saving power).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Plan {
    pub chosen: std::collections::BTreeMap<String, String>,
}

/// The processors' preset a power mode is, as the daemon has it.
pub fn cpu_for_mode(mode: crate::power_profile::Mode) -> crate::cpu_tuning::CpuPreset {
    use crate::{cpu_tuning::CpuPreset, power_profile::Mode};
    match mode {
        Mode::PowerSaver => CpuPreset::Saving,
        Mode::Balanced => CpuPreset::Balanced,
        Mode::Performance => CpuPreset::Fastest,
    }
}

impl Plan {
    /// The processors' preset `when` (on battery, None: the mode's).
    pub fn cpu(&self, when: When) -> Option<crate::cpu_tuning::CpuPreset> {
        match self.chosen.get(&format!("{}/cpu", when.key())) {
            Some(id) => crate::cpu_tuning::CpuPreset::parse(id),
            None => match when {
                When::Mode(m) => Some(cpu_for_mode(m)),
                When::Battery => None,
            },
        }
    }

    /// GPU `pci`'s preset `when` (on battery, None: the mode's).
    pub fn gpu(&self, when: When, pci: &str, nvidia: bool) -> Option<Preset> {
        match self.chosen.get(&format!("{}/{pci}", when.key())) {
            Some(id) => Preset::parse(id),
            None => Self::gpu_default(when, nvidia),
        }
    }

    fn gpu_default(when: When, nvidia: bool) -> Option<Preset> {
        match when {
            When::Mode(m) => Some(for_mode(m)),
            When::Battery => nvidia.then_some(Preset::Cooler),
        }
    }

    /// The processors' preset chosen `when` (None on battery: the mode's).
    pub fn set_cpu(&mut self, when: When, preset: Option<crate::cpu_tuning::CpuPreset>) {
        let key = format!("{}/cpu", when.key());
        let default = match when {
            When::Mode(m) => Some(cpu_for_mode(m)),
            When::Battery => None,
        };
        self.choose(key, preset == default, preset.map_or("mode", |p| p.id()));
    }

    /// GPU `pci`'s preset chosen `when` (None on battery: the mode's).
    pub fn set_gpu(&mut self, when: When, pci: &str, nvidia: bool, preset: Option<Preset>) {
        let key = format!("{}/{pci}", when.key());
        self.choose(key, preset == Self::gpu_default(when, nvidia), preset.map_or("mode", |p| p.id()));
    }

    fn choose(&mut self, key: String, default: bool, id: &str) {
        if default {
            self.chosen.remove(&key);
        } else {
            self.chosen.insert(key, id.to_owned());
        }
    }

    /// The processors' preset in force in `mode`, on battery or not.
    pub fn cpu_now(&self, mode: crate::power_profile::Mode, on_battery: bool) -> crate::cpu_tuning::CpuPreset {
        on_battery.then(|| self.cpu(When::Battery)).flatten().or_else(|| self.cpu(When::Mode(mode))).unwrap_or_else(|| cpu_for_mode(mode))
    }

    /// GPU `pci`'s preset in force in `mode`, on battery or not.
    pub fn gpu_now(&self, mode: crate::power_profile::Mode, on_battery: bool, pci: &str, nvidia: bool) -> Preset {
        on_battery.then(|| self.gpu(When::Battery, pci, nvidia)).flatten().or_else(|| self.gpu(When::Mode(mode), pci, nvidia)).unwrap_or_else(|| for_mode(mode))
    }
}

/// A GPU as the plan takes it: its PCI address, what it allows, and
/// whether it's NVIDIA's (its battery default).
pub struct PlanGpu {
    pub pci: String,
    pub caps: Caps,
    pub nvidia: bool,
}

/// `kept` with every GPU on the preset the plan has for `mode` and the
/// power supply (Custom: its knobs as set by hand); None if that changes
/// nothing.
pub fn scenario(kept: &Tuning, gpus: &[PlanGpu], mode: crate::power_profile::Mode, on_battery: bool) -> Option<Tuning> {
    let mut new = kept.clone();
    for g in gpus {
        let tune = match kept.plan.gpu_now(mode, on_battery, &g.pci, g.nvidia) {
            Preset::Custom => GpuTune { preset: Preset::Custom, ..kept.custom_of(&g.pci) },
            p => self::preset(&g.pci, p, &g.caps),
        };
        new.set(tune);
    }
    (new != *kept).then_some(new)
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

    /// GPU `pci`'s knobs as set by hand (its Custom preset).
    pub fn custom_of(&self, pci: &str) -> GpuTune {
        self.custom.iter().find(|g| g.pci == pci).cloned().unwrap_or_else(|| GpuTune { pci: pci.to_owned(), preset: Preset::Custom, ..Default::default() })
    }

    /// GPU `pci`'s Custom knobs set.
    pub fn set_custom(&mut self, tune: GpuTune) {
        self.custom.retain(|g| g.pci != tune.pci);
        self.custom.push(GpuTune { preset: Preset::Custom, ..tune });
        self.custom.sort_by(|a, b| a.pci.cmp(&b.pci));
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
/// NVML, opened once and kept open for the life of the process. That is
/// what keeps an NVIDIA GPU's tuning: with persistence off, when the last
/// program holding the GPU closes it the driver shuts the GPU down
/// whole, and a clock lock or power limit set on it goes with it
/// (open-gpu-kernel-modules' `nv_close_device` → `nv_stop_device` →
/// `rm_shutdown_adapter`), so a lock set and let go of lasted only while
/// something else had the GPU open. `olo-gpu-tune follow`, which runs
/// for the whole boot, holds it this way from its first change; the GPU
/// still sleeps (fine-grained runtime D3: held, a 615 driver's laptop
/// GPU suspended 25 s after its last use, video memory off), so holding
/// it costs nothing idle. Never shut down early.
/// NVIDIA's library, started once it starts. A start that failed isn't
/// kept: the driver may answer later (loaded, or back from an error),
/// and Preferences, open for days, asked it once and never again.
pub(crate) fn nvml() -> Result<&'static nvml_wrapper::Nvml, String> {
    static NVML: std::sync::OnceLock<nvml_wrapper::Nvml> = std::sync::OnceLock::new();
    if let Some(n) = NVML.get() {
        return Ok(n);
    }
    let n = nvml_wrapper::Nvml::init().map_err(|e| format!("NVIDIA's library didn't start ({e})"))?;
    Ok(NVML.get_or_init(|| n))
}

/// Its PCI address as NVML has it ("00000000:01:00.0").
fn nvml_bus(pci: &str) -> String {
    format!("0000{pci}")
}

/// What NVIDIA's GPU `pci` allows, or what its driver said instead.
fn nvidia_caps(pci: &str) -> Result<NvidiaCaps, String> {
    use nvml_wrapper::enum_wrappers::device::Clock;
    let d = nvml()?.device_by_pci_bus_id(nvml_bus(pci)).map_err(|e| format!("its driver didn't answer ({e})"))?;
    let power = (|| {
        let c = d.power_management_limit_constraints().ok()?;
        let w = |mw: u32| (mw + 500) / 1000;
        // (A laptop's tells a range but can't be given a limit, the
        // platform's to set: then its limit can't be read either.)
        let now = d.power_management_limit().ok()?;
        Some((w(c.min_limit), w(c.max_limit), w(d.power_management_limit_default().ok()?), w(now)))
    })();
    Ok(NvidiaCaps { power, min_gpu_mhz: nvidia_rest_mhz(&d), max_gpu_mhz: d.max_clock_info(Clock::Graphics).unwrap_or(0), persistent_now: d.is_in_persistent_mode().ok() })
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

/// Whether the kernel has a GPU's power management in an error state
/// (`runtime_status` "error": its driver failed to wake or suspend it,
/// as NVIDIA's did when memory ran out on 2026-11-07). It stays so, its
/// driver not answering, until the computer starts again.
pub fn power_failed(gpu: &crate::gpus::Gpu) -> bool {
    read(gpu.sysfs.join("power/runtime_status")).trim() == "error"
}

/// What `gpu` allows. NVIDIA's only with `wake` or while it's awake
/// (asking wakes it); else what was last read ([`Caps::load_cached`]).
pub fn caps(gpu: &crate::gpus::Gpu, wake: bool) -> Option<Caps> {
    caps_or_why(gpu, wake).0
}

/// [`caps`], and when NVIDIA's driver was asked and didn't answer, what
/// it said ("its driver didn't answer (GPU is lost)"), for the person.
pub fn caps_or_why(gpu: &crate::gpus::Gpu, wake: bool) -> (Option<Caps>, Option<String>) {
    match gpu.driver.as_str() {
        "xe" => (xe_caps(&gpu.sysfs).map(Caps::Xe), None),
        "nvidia" if wake || !asleep(gpu) => match nvidia_caps(&gpu.pci) {
            Ok(c) => {
                let c = Caps::Nvidia(c);
                c.cache(&gpu.pci);
                (Some(c), None)
            }
            Err(why) => (None, Some(why)),
        },
        "nvidia" => (Caps::load_cached(&gpu.pci), None),
        _ => (Some(Caps::None), None),
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
    let nvml = match nvml() {
        Ok(n) => n,
        Err(e) => return vec![Err(e)],
    };
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

    /// The plan's defaults: each mode's own preset for the processors and
    /// every GPU; on battery the mode's, but NVIDIA's GPU on Cooler and
    /// Quieter. A choice back on its default leaves nothing kept.
    #[test]
    fn the_plan_s_defaults_and_choices() {
        use crate::{cpu_tuning::CpuPreset, power_profile::Mode};
        let mut plan = Plan::default();
        let (intel, nvidia) = ("0000:00:02.0", "0000:01:00.0");
        assert_eq!(plan.cpu(When::Mode(Mode::PowerSaver)), Some(CpuPreset::Saving));
        assert_eq!(plan.cpu(When::Battery), None);
        assert_eq!(plan.gpu(When::Mode(Mode::Performance), intel, false), Some(Preset::Fastest));
        assert_eq!(plan.gpu(When::Battery, intel, false), None);
        assert_eq!(plan.gpu(When::Battery, nvidia, true), Some(Preset::Cooler));
        // In force: Performance on battery leaves the processors and
        // Intel's GPU fastest, NVIDIA's saving power.
        assert_eq!(plan.cpu_now(Mode::Performance, true), CpuPreset::Fastest);
        assert_eq!(plan.gpu_now(Mode::Performance, true, intel, false), Preset::Fastest);
        assert_eq!(plan.gpu_now(Mode::Performance, true, nvidia, true), Preset::Cooler);
        assert_eq!(plan.gpu_now(Mode::Performance, false, nvidia, true), Preset::Fastest);
        // Chosen otherwise: kept; back on the default: gone.
        plan.set_cpu(When::Battery, Some(CpuPreset::Saving));
        plan.set_gpu(When::Battery, nvidia, true, None);
        plan.set_gpu(When::Mode(Mode::Balanced), intel, false, Some(Preset::Cooler));
        assert_eq!(plan.chosen.len(), 3, "{plan:?}");
        assert_eq!(plan.cpu_now(Mode::Performance, true), CpuPreset::Saving);
        assert_eq!(plan.gpu_now(Mode::Performance, true, nvidia, true), Preset::Fastest, "NVIDIA's as the mode on battery");
        assert_eq!(plan.gpu_now(Mode::Balanced, false, intel, false), Preset::Cooler);
        plan.set_cpu(When::Battery, None);
        plan.set_gpu(When::Battery, nvidia, true, Some(Preset::Cooler));
        plan.set_gpu(When::Mode(Mode::Balanced), intel, false, Some(Preset::Balanced));
        assert!(plan.chosen.is_empty(), "{plan:?}");
        // Kept as JSON and read back.
        plan.set_cpu(When::Mode(Mode::PowerSaver), Some(CpuPreset::Balanced));
        let back: Plan = serde_json::from_str(&serde_json::to_string(&plan).unwrap()).unwrap();
        assert_eq!(back, plan);
    }

    /// The plan applied: each GPU on what it says for the mode and the
    /// power supply, Custom its knobs as set by hand.
    #[test]
    fn the_plan_sets_the_gpus() {
        use crate::power_profile::Mode;
        let nv = Caps::Nvidia(NvidiaCaps { power: Some((5, 75, 60, 60)), min_gpu_mhz: 180, max_gpu_mhz: 3090, persistent_now: Some(false) });
        let gpus = vec![PlanGpu { pci: "0000:00:02.0".into(), caps: xe(), nvidia: false }, PlanGpu { pci: "0000:01:00.0".into(), caps: nv, nvidia: true }];
        let kept = Tuning::default();
        let mains = scenario(&kept, &gpus, Mode::Performance, false).unwrap();
        assert!(mains.gpus.iter().all(|g| g.preset == Preset::Fastest), "{mains:?}");
        let battery = scenario(&mains, &gpus, Mode::Performance, true).unwrap();
        assert_eq!((battery.of("0000:00:02.0").preset, battery.of("0000:01:00.0").preset), (Preset::Fastest, Preset::Cooler));
        assert_eq!(scenario(&battery, &gpus, Mode::Performance, true), None, "on it already");
        // Custom: the knobs set by hand.
        let mut custom = battery.clone();
        custom.set_custom(GpuTune { pci: "0000:01:00.0".into(), max_gpu_mhz: Some(1200), ..Default::default() });
        custom.plan.set_gpu(When::Battery, "0000:01:00.0", true, Some(Preset::Custom));
        let set = scenario(&custom, &gpus, Mode::Performance, true).unwrap();
        assert_eq!((set.of("0000:01:00.0").preset, set.of("0000:01:00.0").max_gpu_mhz), (Preset::Custom, Some(1200)));
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
        let Ok(nvml) = nvml() else { return };
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
