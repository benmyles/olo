//! The public power mode IDs also work with Linux power-profiles-daemon.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    PowerSaver,
    Balanced,
    Performance,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::PowerSaver, Mode::Balanced, Mode::Performance];

    /// The daemon's name for it.
    pub fn id(self) -> &'static str {
        match self {
            Mode::PowerSaver => "power-saver",
            Mode::Balanced => "balanced",
            Mode::Performance => "performance",
        }
    }

    pub fn from_id(id: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::PowerSaver => "Battery Saver",
            Mode::Balanced => "Adaptive Performance",
            Mode::Performance => "Low Latency",
        }
    }

    /// What it does, in a sentence.
    pub fn about(self) -> &'static str {
        match self {
            Mode::PowerSaver => "Stretch the remaining charge while keeping everyday work responsive.",
            Mode::Balanced => "Boost when work arrives and ease back between tasks.",
            Mode::Performance => "Keep foreground work ready between bursts. Fans start sooner.",
        }
    }
}

/// The modes the daemon offers, the one in use, and what it acts on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Modes {
    pub active: Mode,
    pub available: Vec<Mode>,
    /// Why Low Latency is limited now, if it is: the computer on a lap,
    /// running hot (the daemon's words: "lap-detected",
    /// "high-operating-temperature").
    pub degraded: Option<String>,
    /// What the daemon drives: the processors' ("intel_pstate") and the
    /// platform's ("platform_profile") drivers, each once.
    pub drivers: Vec<String>,
}

impl Modes {
    /// Why Low Latency is limited, said plainly.
    pub fn degraded_text(&self) -> Option<String> {
        let why = self.degraded.as_deref()?;
        Some(match why {
            "lap-detected" => "Low Latency is limited while the computer is on a lap.".into(),
            "high-operating-temperature" => "Low Latency is limited while the computer runs hot.".into(),
            other => format!("Low Latency is limited ({other})."),
        })
    }
}

/// What the session has observed, after any automatic low-battery choice.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub modes: Option<Modes>,
    pub on_battery: bool,
    pub percent: Option<u32>,
    pub resume_on_ac: Option<Mode>,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use crate::PowerSettings;
    #[test]
    fn migrate_shared_and_battery_only_timers_without_changing_the_other_supply() {
        let mut s: PowerSettings = serde_json::from_value(serde_json::json!({"display_off_minutes":7, "sleep_minutes":45, "sleep_on_battery_only":true})).unwrap();
        assert_eq!(s.idle_minutes(false), (7, 0));
        assert_eq!(s.idle_minutes(true), (7, 45));
        s.set_idle_minutes(false, true, 60);
        s.set_idle_minutes(true, false, 5);
        assert_eq!(s.idle_minutes(false), (7, 60));
        assert_eq!(s.idle_minutes(true), (5, 45));
        let restored: PowerSettings = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        assert_eq!(restored, s);
    }
}
