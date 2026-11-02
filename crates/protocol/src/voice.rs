//! Voice processing: what the voice service (`olo-voice --service`) does to
//! the microphone, its settings (Preferences ▸ Sound, the Deskbar's speaker
//! menu) and what it is doing now.
//!
//! The service puts a source of its own, [`NODE`], in front of whichever
//! microphone the person chose, and makes it the default input: anything
//! that records gets the processed voice. Device lists show the microphone
//! it listens to in its place (`olo_system::audio`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The processed source's node name.
pub const NODE: &str = "olo_voice";

/// The runtime directory, or `OLO_VOICE_DIR` (tests: a service kept apart
/// from the person's).
fn runtime_dir() -> PathBuf {
    std::env::var_os("OLO_VOICE_DIR").or_else(|| std::env::var_os("XDG_RUNTIME_DIR")).map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// What the microphone is for. The processing is the same; what differs
/// is the buffering around it: calls keep it to the least that never runs
/// dry, recordings take plenty, since only an unbroken take matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Talking with people: the least delay that still sounds clean.
    #[default]
    Calls,
    /// Recording: the cleanest, unbroken sound, whatever the delay.
    Recording,
}

impl Mode {
    pub const ALL: [Mode; 2] = [Mode::Calls, Mode::Recording];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Calls => "Calls",
            Mode::Recording => "Recording",
        }
    }
}

/// How hard noise is taken out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Noise {
    Off,
    /// Turned down by up to 18 dB: natural, a room still sounds a room.
    Moderate,
    /// Up to 30 dB.
    #[default]
    Strong,
    /// As far as it goes: only the voice.
    Full,
}

impl Noise {
    pub const ALL: [Noise; 4] = [Noise::Off, Noise::Moderate, Noise::Strong, Noise::Full];

    pub fn label(self) -> &'static str {
        match self {
            Noise::Off => "Off",
            Noise::Moderate => "Moderate",
            Noise::Strong => "Strong",
            Noise::Full => "Voice Only",
        }
    }

    /// How far noise may be turned down, dB.
    pub fn limit_db(self) -> f32 {
        match self {
            Noise::Off => 0.0,
            Noise::Moderate => 18.0,
            Noise::Strong => 30.0,
            Noise::Full => 100.0,
        }
    }
}

/// What the voice processing does (Preferences ▸ Sound).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Process the microphone at all.
    pub enabled: bool,
    pub mode: Mode,
    /// Take out what the speakers play (only done while the sound comes
    /// out of something the microphone can hear: not headphones).
    pub echo_cancellation: bool,
    pub noise: Noise,
    /// Bring every voice to the same level.
    pub normalize: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { enabled: true, mode: Mode::Calls, echo_cancellation: true, noise: Noise::Strong, normalize: true }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        match std::env::var_os("OLO_VOICE_DIR") {
            Some(d) => PathBuf::from(d).join("voice-settings.json"),
            None => olo_xdg::config_dir().join("voice.json"),
        }
    }

    pub fn load() -> Settings {
        std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.new");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, path)
    }
}

/// What the service is doing (its state file, followed by inotify).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// The service's process (a state file whose process is gone is old).
    pub pid: u32,
    /// [`NODE`] exists.
    pub running: bool,
    /// The microphone it listens to: its node name and what to call it.
    pub microphone: Option<String>,
    pub microphone_label: Option<String>,
    /// Something is recording from it now.
    pub active: bool,
    /// Echo cancellation is on now (it is set, and the sound comes out of
    /// something the microphone can hear).
    pub echo_cancelling: bool,
    /// What processing adds, ms.
    pub latency_ms: u32,
    /// A 10 ms frame's processing, µs (an average over the last second).
    pub frame_us: u32,
    /// Frames the source had nothing for (should stay 0).
    pub dropouts: u64,
    /// Why it isn't running, if it isn't.
    pub error: Option<String>,
}

impl State {
    pub fn path() -> PathBuf {
        runtime_dir().join("olo-voice.json")
    }

    pub fn load() -> State {
        let state: State = std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        if state.pid != 0 && !std::path::Path::new(&format!("/proc/{}", state.pid)).exists() {
            return State::default();
        }
        state
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        let tmp = path.with_extension("json.part");
        std::fs::write(&tmp, serde_json::to_vec(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, &path)
    }
}
