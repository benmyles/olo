//! The processors' power: three presets, made of what the power profiles
//! daemon sets for its modes, so each can be chosen for a mode or for
//! running on battery (`gpu_tuning::Plan`, Preferences ▸ Power): each
//! processor's governor and energy preference (cpufreq's
//! `scaling_governor` and `energy_performance_preference`), turbo
//! (intel_pstate's `no_turbo`, else cpufreq's `boost`) and the firmware's
//! platform profile (its power limits and fans). Read without root
//! ([`caps`]); applied by `olo-gpu-tune` (root) as the power mode or the
//! power supply changes ([`apply`]).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The processors' preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CpuPreset {
    /// Slower, favouring energy: no turbo, the firmware's low-power limits.
    Saving,
    /// Fast when there's work, frugal when there isn't.
    #[default]
    Balanced,
    /// As fast as they go.
    Fastest,
}

impl CpuPreset {
    pub const ALL: [CpuPreset; 3] = [CpuPreset::Saving, CpuPreset::Balanced, CpuPreset::Fastest];

    pub fn name(self) -> &'static str {
        match self {
            CpuPreset::Saving => "Saving Power",
            CpuPreset::Balanced => "Balanced",
            CpuPreset::Fastest => "Fastest",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            CpuPreset::Saving => "saving",
            CpuPreset::Balanced => "balanced",
            CpuPreset::Fastest => "fastest",
        }
    }

    pub fn parse(s: &str) -> Option<CpuPreset> {
        CpuPreset::ALL.into_iter().find(|p| p.id() == s)
    }

    /// The governor it asks for.
    fn governor(self) -> &'static str {
        match self {
            CpuPreset::Fastest => "performance",
            _ => "powersave",
        }
    }

    /// The energy preferences it asks for, best first (the first the
    /// processors offer is taken).
    fn energy(self) -> &'static [&'static str] {
        match self {
            CpuPreset::Saving => &["power", "balance_power"],
            CpuPreset::Balanced => &["balance_performance", "default"],
            CpuPreset::Fastest => &["performance"],
        }
    }

    fn turbo(self) -> bool {
        self != CpuPreset::Saving
    }

    /// The firmware's platform profile it asks for.
    fn platform(self) -> &'static [&'static str] {
        match self {
            CpuPreset::Saving => &["low-power", "quiet", "cool"],
            CpuPreset::Balanced => &["balanced"],
            CpuPreset::Fastest => &["performance"],
        }
    }
}

/// What the processors allow, and how they're set now.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CpuCaps {
    /// cpufreq's driver ("intel_pstate", "amd-pstate-epp").
    pub driver: String,
    pub governor: String,
    /// The energy preferences offered (none: no energy preference).
    pub preferences: Vec<String>,
    pub preference: String,
    /// Turbo can be switched, and whether it's on.
    pub turbo_switch: bool,
    pub turbo: bool,
    /// The firmware's platform profiles offered, and the one now.
    pub platforms: Vec<String>,
    pub platform: String,
}

/// Where the processors' and firmware's knobs are (sysfs's), for tests.
pub struct Sys {
    pub cpu: PathBuf,
    pub firmware: PathBuf,
}

impl Default for Sys {
    fn default() -> Sys {
        Sys { cpu: PathBuf::from("/sys/devices/system/cpu"), firmware: PathBuf::from("/sys/firmware/acpi") }
    }
}

fn read(p: impl AsRef<Path>) -> String {
    std::fs::read_to_string(p).map(|s| s.trim().to_owned()).unwrap_or_default()
}

impl Sys {
    /// Each processor's cpufreq folder.
    fn policies(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(self.cpu.join("cpufreq"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("policy")))
            .collect();
        v.sort();
        v
    }

    fn turbo_file(&self) -> Option<(PathBuf, bool)> {
        // (intel_pstate's says "no turbo"; cpufreq's says "boost".)
        let no_turbo = self.cpu.join("intel_pstate/no_turbo");
        if no_turbo.exists() {
            return Some((no_turbo, true));
        }
        let boost = self.cpu.join("cpufreq/boost");
        boost.exists().then_some((boost, false))
    }

    /// What they allow (None: no cpufreq here).
    pub fn caps(&self) -> Option<CpuCaps> {
        let first = self.policies().into_iter().next()?;
        let (turbo_switch, turbo) = match self.turbo_file() {
            Some((f, inverted)) => (true, (read(&f) == "1") != inverted),
            None => (false, true),
        };
        let words = |p: PathBuf| read(p).split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        Some(CpuCaps {
            driver: read(first.join("scaling_driver")),
            governor: read(first.join("scaling_governor")),
            preferences: words(first.join("energy_performance_available_preferences")),
            preference: read(first.join("energy_performance_preference")),
            turbo_switch,
            turbo,
            platforms: words(self.firmware.join("platform_profile_choices")),
            platform: read(self.firmware.join("platform_profile")),
        })
    }

    /// `preset` set on every processor and the firmware (root): what
    /// each write did, or why it couldn't.
    pub fn apply(&self, preset: CpuPreset) -> Vec<Result<String, String>> {
        let mut out = Vec::new();
        let Some(caps) = self.caps() else {
            return vec![Err("no cpufreq here".into())];
        };
        let write = |path: &Path, value: &str| std::fs::write(path, value).map_err(|e| format!("{}: {e}", path.display()));
        let policies = self.policies();
        // The governor first: the energy preference can't be written
        // under the performance governor.
        let governors = read(policies[0].join("scaling_available_governors"));
        if governors.split_whitespace().any(|g| g == preset.governor()) {
            let failed: Vec<String> = policies.iter().filter_map(|p| write(&p.join("scaling_governor"), preset.governor()).err()).collect();
            out.push(if failed.is_empty() { Ok(format!("governor {}", preset.governor())) } else { Err(failed.join("; ")) });
        }
        if preset.governor() != "performance"
            && let Some(pref) = preset.energy().iter().find(|e| caps.preferences.iter().any(|p| p == *e))
        {
            let failed: Vec<String> = policies.iter().filter_map(|p| write(&p.join("energy_performance_preference"), pref).err()).collect();
            out.push(if failed.is_empty() { Ok(format!("energy preference {pref}")) } else { Err(failed.join("; ")) });
        }
        if let Some((file, inverted)) = self.turbo_file() {
            let on = preset.turbo();
            out.push(write(&file, if on != inverted { "1" } else { "0" }).map(|()| format!("turbo {}", if on { "on" } else { "off" })));
        }
        if let Some(p) = preset.platform().iter().find(|p| caps.platforms.iter().any(|c| c == *p)) {
            out.push(write(&self.firmware.join("platform_profile"), p).map(|()| format!("platform profile {p}")));
        }
        out
    }
}

/// What the processors allow (None: no cpufreq here).
pub fn caps() -> Option<CpuCaps> {
    Sys::default().caps()
}

/// `preset` set (root).
pub fn apply(preset: CpuPreset) -> Vec<Result<String, String>> {
    Sys::default().apply(preset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake() -> (PathBuf, Sys) {
        let root = Path::new("/var/tmp").join(format!("olo-cpu-tuning-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&root);
        let w = |p: &str, t: &str| {
            let p = root.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, t).unwrap();
        };
        for k in 0..2 {
            let d = format!("cpu/cpufreq/policy{k}");
            w(&format!("{d}/scaling_driver"), "intel_pstate\n");
            w(&format!("{d}/scaling_governor"), "performance\n");
            w(&format!("{d}/scaling_available_governors"), "performance powersave\n");
            w(&format!("{d}/energy_performance_preference"), "performance\n");
            w(&format!("{d}/energy_performance_available_preferences"), "default performance balance_performance balance_power power \n");
        }
        w("cpu/intel_pstate/no_turbo", "0\n");
        w("firmware/platform_profile", "performance\n");
        w("firmware/platform_profile_choices", "low-power balanced performance\n");
        let sys = Sys { cpu: root.join("cpu"), firmware: root.join("firmware") };
        (root, sys)
    }

    #[test]
    fn each_preset_sets_what_the_daemon_s_modes_do() {
        let (root, sys) = fake();
        let c = sys.caps().unwrap();
        assert_eq!((c.driver.as_str(), c.governor.as_str(), c.turbo, c.platform.as_str()), ("intel_pstate", "performance", true, "performance"));
        let all_ok = |r: Vec<Result<String, String>>| r.iter().all(Result::is_ok);
        assert!(all_ok(sys.apply(CpuPreset::Saving)));
        let c = sys.caps().unwrap();
        assert_eq!((c.governor.as_str(), c.preference.as_str(), c.turbo, c.platform.as_str()), ("powersave", "power", false, "low-power"));
        assert!(all_ok(sys.apply(CpuPreset::Balanced)));
        let c = sys.caps().unwrap();
        assert_eq!((c.governor.as_str(), c.preference.as_str(), c.turbo, c.platform.as_str()), ("powersave", "balance_performance", true, "balanced"));
        assert!(all_ok(sys.apply(CpuPreset::Fastest)));
        let c = sys.caps().unwrap();
        // (The performance governor holds the preference at performance
        // itself: it's left alone.)
        assert_eq!((c.governor.as_str(), c.turbo, c.platform.as_str()), ("performance", true, "performance"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
