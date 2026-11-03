//! Preferences ▸ Performance (`performance.json` in Olo's config folder)
//! and what the supervisor (`olo-supervisor`) makes of it, published for
//! Deskbar, Activity Monitor and agents (`olo-supervisor.json` in the
//! runtime folder). The supervisor keeps the desktop responsive: builds
//! and other background work lowered, the active window's app favoured
//! (Olo itself is never lowered), and a watch for stuck processes, leaks,
//! memory that threatens the system, and zombies, each found from several
//! signals with its evidence. Off by default.

use serde::{Deserialize, Serialize};

/// Every number the supervisor's judgements go by, tunable in
/// Preferences ▸ Performance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Thresholds {
    /// A spin: busier than this (% of one core) …
    pub spin_cpu: f32,
    /// … for this long (minutes) …
    pub spin_minutes: f32,
    /// … with fewer voluntary context switches a second than this (it
    /// never waits for anything) …
    pub spin_switches: f32,
    /// … and less I/O than this (KB a second: no reading or writing).
    pub spin_io_kb: f32,
    /// A leak: growing over this long (minutes) …
    pub leak_minutes: f32,
    /// … at least this fast (MB a minute) …
    pub leak_rate_mb: f32,
    /// … by at least this much in all (MB) …
    pub leak_growth_mb: f32,
    /// … steadily: a straight line fits it at least this well (R², 0–1).
    pub leak_fit: f32,
    /// Memory that threatens the system: a process with this share of
    /// RAM (%) …
    pub memory_share: f32,
    /// … while less than this is available (% of RAM) …
    pub memory_available: f32,
    /// … or tasks stall on memory this much of the time (%, the last
    /// 10 s: PSI).
    pub memory_stall: f32,
    /// Zombies: left unreaped this long (minutes) …
    pub zombie_minutes: f32,
    /// … by a parent leaving at least this many (fewer are a parent that
    /// reaps lazily, as Chrome's does: one process slot each) …
    pub zombie_least: u32,
    /// … and urgent when a parent leaves this many.
    pub zombie_count: u32,
}

impl Default for Thresholds {
    fn default() -> Thresholds {
        Thresholds {
            spin_cpu: 90.0,
            spin_minutes: 3.0,
            spin_switches: 2.0,
            spin_io_kb: 4.0,
            leak_minutes: 30.0,
            leak_rate_mb: 2.0,
            leak_growth_mb: 300.0,
            leak_fit: 0.85,
            memory_share: 25.0,
            memory_available: 10.0,
            memory_stall: 10.0,
            zombie_minutes: 5.0,
            zombie_least: 3,
            zombie_count: 20,
        }
    }
}

/// Work that's background by its name: builds and compilers, indexers,
/// package managers.
pub const BACKGROUND: &[&str] = &[
    "rustc",
    "cargo",
    "cc1",
    "cc1plus",
    "cc",
    "c++",
    "gcc",
    "g++",
    "clang",
    "clang++",
    "ld",
    "ld.lld",
    "lld",
    "mold",
    "ld.bfd",
    "ld.gold",
    "as",
    "make",
    "ninja",
    "cmake",
    "go",
    "javac",
    "gradle",
    "kotlinc",
    "tsc",
    "esbuild",
    "swc",
    "bazel",
    "buck2",
    "sccache",
    "kache",
    "rust-analyzer",
    "clippy-driver",
    "rustdoc",
    "zig",
    "ghc",
    "tracker-miner-fs-3",
    "baloo_file",
    "updatedb",
    "rpm-ostree",
    "dnf",
    "flatpak",
    "pip",
    "npm",
    "pnpm",
    "yarn",
    "bun",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerformanceSettings {
    /// The supervisor at work (Deskbar's item turns it on and off).
    pub enabled: bool,
    /// Lower builds and background work (nice, batch scheduling, idle
    /// I/O) and favour the active window's app.
    pub priorities: bool,
    /// How far background work is lowered (nice, 1–19).
    pub background_nice: i32,
    /// Names that are background work (`BACKGROUND`, as the person
    /// changed it).
    pub background: Vec<String>,
    /// Watch for stuck processes, leaks, memory and zombies.
    pub watchdog: bool,
    /// Ask about an urgent finding (an alert, with Stop Process).
    pub alerts: bool,
    pub thresholds: Thresholds,
    /// Processes never warned about (by name, or part of their command).
    pub ignored: Vec<String>,
}

impl Default for PerformanceSettings {
    fn default() -> PerformanceSettings {
        PerformanceSettings {
            enabled: false,
            priorities: true,
            background_nice: 10,
            background: BACKGROUND.iter().map(|s| (*s).to_owned()).collect(),
            watchdog: true,
            alerts: true,
            thresholds: Thresholds::default(),
            ignored: Vec::new(),
        }
    }
}

impl PerformanceSettings {
    pub fn path() -> std::path::PathBuf {
        olo_xdg::config_dir().join("performance.json")
    }

    pub fn load() -> PerformanceSettings {
        std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    /// Write them (atomically: readers never see half a file).
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.new");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, path)
    }

    /// Whether `name` (or `command`) is on the ignore list.
    pub fn ignores(&self, name: &str, command: &str) -> bool {
        self.ignored.iter().any(|i| !i.is_empty() && (i == name || command.contains(i.as_str())))
    }
}

/// What a finding is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// Busy for long without waiting on anything or doing any I/O.
    Spin,
    /// Its memory grows steadily.
    Leak,
    /// It holds so much memory the system is short of it.
    Memory,
    /// It leaves its exited children unreaped.
    Zombies,
}

impl FindingKind {
    pub fn title(self) -> &'static str {
        match self {
            FindingKind::Spin => "Seems stuck",
            FindingKind::Leak => "Memory keeps growing",
            FindingKind::Memory => "Using too much memory",
            FindingKind::Zombies => "Leaves zombies",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub pid: i32,
    pub name: String,
    pub command: String,
    pub kind: FindingKind,
    /// How sure (0–1), from how many signals agree and how strongly.
    pub confidence: f32,
    /// Worth asking the person about now.
    pub urgent: bool,
    /// One line for people.
    pub summary: String,
    /// What it rests on, a line each (the samples, the trend, the
    /// pressure).
    pub evidence: Vec<String>,
    /// When first found (seconds since the epoch).
    pub since: u64,
}

/// Olo's priority tiers: what keeps the desktop responsive runs above
/// everything else, always (each program asks rtkit for itself as it
/// starts, `olo_system::priority::raise_self`; the supervisor re-asserts
/// them), and only then does the focused app stand above background
/// work (the supervisor lowering that). The nice a program's name gets,
/// if any: app_server (every input event and frame) highest; the session,
/// Deskbar and the Wayland proxy next.
pub fn tier(name: &str) -> Option<i32> {
    match name {
        "olo-app-server" => Some(CORE_NICE),
        "olo-voice" | "olo-airpods" => Some(AUDIO_NICE),
        "olo-session" | "olo-deskbar" | "olo-wlproxy" => Some(SHELL_NICE),
        _ => None,
    }
}

/// The compositor's nice.
pub const CORE_NICE: i32 = -10;
/// The voice services' (olo-voice, olo-airpods): a microphone's sound
/// every 10 ms, where a late frame is a click in someone's call. Their
/// PipeWire threads are realtime besides (rtkit, by libpipewire; the
/// supervisor sees that they are: [`AUDIO_THREAD`]).
pub const AUDIO_NICE: i32 = -10;
/// The name PipeWire gives the thread a stream's sound runs on, in a
/// program of the audio tier: realtime.
pub const AUDIO_THREAD: &str = "data-loop";
/// Its realtime priority, at most (rtkit gives up to its
/// `MaxRealtimePriority`, 20 by default; PipeWire's clients ask 83).
pub const AUDIO_RT_PRIORITY: u32 = 20;
/// The session's, Deskbar's and the proxy's.
pub const SHELL_NICE: i32 = -5;

/// How busy the scheduler takes a frame's thread to be, at least, while
/// it runs (its `uclamp.min`, of 1024): app_server's main thread (every
/// input event and frame). On a computer of fast and slow cores (Apple's,
/// under Asahi) the scheduler otherwise puts a thread that's mostly idle
/// on a slow core at a low clock, and a frame's work there takes three
/// or four times as long: Activity's first frame of its processes, 13–17
/// ms; 3.7–4.6 with this. Nothing while the thread sleeps: idle costs
/// nothing. (Taken by `olo_system::priority::util_min_self`, and GPUI's
/// for its windows' threads.)
pub const CORE_UTIL_MIN: u32 = 1024;
/// A window's thread's (GPUI's main thread, in every app): as fast as
/// 1024 there (a fast core near its top clock), 512 no faster than a
/// slow core's top.
pub const UI_UTIL_MIN: u32 = 768;
/// The voice service's processing thread's: a 10 ms frame's work on a
/// fast core (0.57 ms) rather than a slow one at a low clock (1.1 ms,
/// measured in pw-sandbox, where nothing is realtime).
pub const AUDIO_UTIL_MIN: u32 = 1024;

/// A program of Olo's own the supervisor found running above its tier
/// and raised ([`SupervisorState::raised`]).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Raised {
    pub pid: i32,
    pub name: String,
    pub nice_before: i32,
    pub nice: i32,
}

/// A process the supervisor lowered, and how.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lowered {
    pub pid: i32,
    pub name: String,
    pub why: String,
    pub nice_before: i32,
    pub nice: i32,
    /// "batch" or "normal".
    pub policy: String,
    /// "idle" or "normal".
    pub io: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    /// Seconds since the epoch.
    pub at: u64,
    pub text: String,
}

/// What the supervisor is doing, for everyone to read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SupervisorState {
    pub running: bool,
    /// The supervisor's process: one that has gone (it crashed) left a
    /// file that is no longer true, read as off.
    pub pid: u32,
    /// The process favoured (the active window's app), if any.
    pub favoured: Option<(i32, String)>,
    pub findings: Vec<Finding>,
    pub lowered: Vec<Lowered>,
    /// Olo's own programs it found above their tier and raised
    /// ([`tier`]); each asks for itself as it starts, so usually none.
    pub raised: Vec<Raised>,
    /// What it did lately, newest last.
    pub log: Vec<LogEntry>,
    /// The system's memory: available (% of RAM) and stalls (% some,
    /// last 10 s).
    pub memory_available: f32,
    pub memory_stall: f32,
}

impl SupervisorState {
    pub fn path() -> std::path::PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
        dir.join("olo-supervisor.json")
    }

    /// What it published, if its process is still there (else off).
    pub fn load() -> SupervisorState {
        let s = Self::read();
        if s.pid == 0 || std::path::Path::new("/proc").join(s.pid.to_string()).exists() { s } else { SupervisorState { log: s.log, ..Default::default() } }
    }

    /// The file as it is, whoever wrote it (the supervisor itself,
    /// taking over from the one before it).
    pub fn read() -> SupervisorState {
        std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        let tmp = path.with_extension("json.new");
        std::fs::write(&tmp, serde_json::to_string(self).unwrap_or_default())?;
        std::fs::rename(tmp, path)
    }

    pub fn urgent(&self) -> usize {
        self.findings.iter().filter(|f| f.urgent).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_read_with_what_they_lack() {
        let s: PerformanceSettings = serde_json::from_str(r#"{"enabled": true, "thresholds": {"spin_cpu": 80}}"#).unwrap();
        assert!(s.enabled && s.priorities && s.watchdog);
        assert_eq!(s.thresholds.spin_cpu, 80.0);
        assert_eq!(s.thresholds.leak_minutes, 30.0);
        assert!(s.background.iter().any(|b| b == "rustc"));
        assert!(!PerformanceSettings::default().enabled, "off by default");
        let mut s = PerformanceSettings::default();
        s.ignored.push("my-sim".into());
        assert!(s.ignores("my-sim", "") && s.ignores("python3", "python3 my-sim --fast") && !s.ignores("other", "other"));
    }
}
