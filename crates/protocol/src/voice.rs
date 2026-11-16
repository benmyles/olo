//! Voice processing: what the voice service (`olo-voice --service`) does to
//! the microphone, its settings (Preferences ▸ Sound, the Deskbar's speaker
//! menu) and what it is doing now.
//!
//! The service puts a source of its own, [`NODE`], in front of whichever
//! microphone the person chose, and makes it the default input: anything
//! that records gets the processed voice. Device lists show the microphone
//! it listens to in its place (`olo_system::audio`).

use std::{collections::BTreeMap, path::PathBuf};

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

/// Which model takes the noise out: both DeepFilterNet3, the low-delay one
/// trained without looking ahead (Ben, 2026-11-07: "keep DeepFilterNet3 as
/// an option but then add another option for the lower latency model").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Model {
    /// Looks 20 ms ahead: the voice 30 ms behind (40 ms in all on a
    /// call). The most noise out of a voice in a noisy room.
    #[default]
    #[serde(rename = "deepfilternet3")]
    DeepFilterNet3,
    /// DeepFilterNet3-LL: no look-ahead, the voice 10 ms behind (20 ms in
    /// all); as clean between words, a voice in a noisy room a little
    /// less clean (2 dB in the trials); three times the work a frame.
    #[serde(rename = "deepfilternet3-ll")]
    LowDelay,
}

impl Model {
    pub const ALL: [Model; 2] = [Model::DeepFilterNet3, Model::LowDelay];

    pub fn label(self) -> &'static str {
        match self {
            Model::DeepFilterNet3 => "DeepFilterNet3",
            Model::LowDelay => "DeepFilterNet3 Low Delay",
        }
    }

    /// Its word for `olo-voice --set model=…`.
    pub fn word(self) -> &'static str {
        match self {
            Model::DeepFilterNet3 => "deepfilternet3",
            Model::LowDelay => "low-delay",
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
    /// What takes the noise out.
    pub model: Model,
    /// Bring every voice to the same level.
    pub normalize: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { enabled: true, mode: Mode::Calls, echo_cancellation: true, noise: Noise::Strong, model: Model::DeepFilterNet3, normalize: true }
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

/// The level a voice is brought up to before it's processed, when its
/// microphone is boosted by the suggested amount ([`suggested_boost`]):
/// an active speech level of -26 dB of full scale (the mean power of the
/// frames someone speaks in, `olo_voice::SpeechLevel`), the nominal level
/// speech is stated at (ITU-T P.56), which leaves room for a raised voice
/// before anything clips and lets "Even out the voice's level" work from
/// a voice it can hear (Ben, 2026-11-16: "some inputs like airpods are
/// not loud enough on microphone input").
pub const AIM_DB: f32 = -26.0;

/// A boost goes by steps of this many dB, up to [`BOOST_MAX_DB`].
pub const BOOST_STEP_DB: f32 = 3.0;
pub const BOOST_MAX_DB: f32 = 30.0;

/// The boost that brings a voice measured at `speech_db` (the service's
/// [`State::speech_db`]) to [`AIM_DB`]: to the nearest step, none for one
/// loud enough already.
pub fn suggested_boost(speech_db: f32) -> f32 {
    ((AIM_DB - speech_db) / BOOST_STEP_DB).round().clamp(0.0, BOOST_MAX_DB / BOOST_STEP_DB) * BOOST_STEP_DB
}

/// A boost of `db` as a gain (1: none).
pub fn gain(db: f32) -> f32 {
    10f32.powf(db.max(0.0) / 20.0)
}

/// Each microphone's boost (dB, by its node name): one that's quiet
/// (AirPods) brought up before it's processed, the others as they are.
/// Kept apart from [`Settings`], which are the same for every microphone.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Boosts {
    pub by_microphone: BTreeMap<String, f32>,
}

impl Boosts {
    pub fn path() -> PathBuf {
        match std::env::var_os("OLO_VOICE_DIR") {
            Some(d) => PathBuf::from(d).join("voice-boost.json"),
            None => olo_xdg::config_dir().join("voice-boost.json"),
        }
    }

    pub fn load() -> Boosts {
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

    /// `microphone`'s boost (dB; 0: none).
    pub fn of(&self, microphone: &str) -> f32 {
        self.by_microphone.get(microphone).copied().unwrap_or(0.0).clamp(0.0, BOOST_MAX_DB)
    }

    /// `microphone` boosted by `db` (0: not at all, and forgotten).
    pub fn set(&mut self, microphone: &str, db: f32) {
        if db <= 0.0 {
            self.by_microphone.remove(microphone);
        } else {
            self.by_microphone.insert(microphone.to_owned(), db.min(BOOST_MAX_DB));
        }
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
    /// How loud a voice comes in from the microphone, before any boost
    /// (its active speech level, dB of full scale, over the last seconds
    /// of speech: `olo_voice::SpeechLevel`), once enough was heard; kept
    /// until another microphone is chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech_db: Option<f32>,
    /// The microphone's boost now (dB; [`Boosts`]).
    #[serde(default)]
    pub boost_db: f32,
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

#[cfg(test)]
mod boost_tests {
    use super::*;

    /// A quiet voice is suggested the boost that brings it to the aim, by
    /// whole steps; a loud one none; none beyond the most.
    #[test]
    fn the_boost_suggested() {
        assert_eq!(suggested_boost(-41.0), 15.0);
        assert_eq!(suggested_boost(-26.0), 0.0);
        assert_eq!(suggested_boost(-12.0), 0.0);
        assert_eq!(suggested_boost(-80.0), BOOST_MAX_DB);
        assert_eq!(suggested_boost(-30.5), 6.0);
    }

    /// Each microphone's own, none unless set, 0 forgotten.
    #[test]
    fn boosts_are_each_microphones() {
        let mut b = Boosts::default();
        b.set("airpods_mic.AA", 15.0);
        assert_eq!((b.of("airpods_mic.AA"), b.of("alsa_input.mic")), (15.0, 0.0));
        b.set("airpods_mic.AA", 99.0);
        assert_eq!(b.of("airpods_mic.AA"), BOOST_MAX_DB);
        b.set("airpods_mic.AA", 0.0);
        assert!(b.by_microphone.is_empty());
    }
}
