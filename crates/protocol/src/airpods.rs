//! AirPods: what `olo-airpods --service` knows of them (a JSON file in the
//! runtime directory, rewritten on every change, which Deskbar's tray and
//! Preferences ▸ Bluetooth and ▸ Sound follow), Olo's own settings for
//! them, and what may be asked of the service over its socket
//! ([`Request`], one JSON line each way; `olo-airpods` on the command
//! line asks the same).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Where the service's state and socket are: the runtime directory, or
/// `OLO_AIRPODS_DIR` (tests: a pretend service kept apart from the
/// person's).
fn runtime_dir() -> PathBuf {
    std::env::var_os("OLO_AIRPODS_DIR").or_else(|| std::env::var_os("XDG_RUNTIME_DIR")).map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// What the buds do with the sound around.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ListeningMode {
    Off,
    Transparency,
    Adaptive,
    NoiseCancellation,
}

impl ListeningMode {
    /// In the order they're offered (as on an iPhone).
    pub const ALL: [ListeningMode; 4] = [ListeningMode::Off, ListeningMode::Transparency, ListeningMode::Adaptive, ListeningMode::NoiseCancellation];

    pub fn label(self) -> &'static str {
        match self {
            ListeningMode::Off => "Off",
            ListeningMode::Transparency => "Transparency",
            ListeningMode::Adaptive => "Adaptive",
            ListeningMode::NoiseCancellation => "Noise Cancellation",
        }
    }

    /// The word on the command line ("anc").
    pub fn word(self) -> &'static str {
        match self {
            ListeningMode::Off => "off",
            ListeningMode::Transparency => "transparency",
            ListeningMode::Adaptive => "adaptive",
            ListeningMode::NoiseCancellation => "anc",
        }
    }

    pub fn from_word(w: &str) -> Option<ListeningMode> {
        ListeningMode::ALL.into_iter().find(|m| m.word() == w || m.label().eq_ignore_ascii_case(w))
    }
}

/// Which bud's microphone the AirPods use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BudMicrophone {
    #[default]
    Automatic,
    Left,
    Right,
}

/// One part's battery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Charge {
    /// 0–100.
    pub percent: u8,
    pub charging: bool,
}

/// What the buds can do (by model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Abilities {
    /// Noise Cancellation and Transparency.
    pub noise_control: bool,
    /// Adaptive, Conversation Awareness, Personalized Volume.
    pub adaptive: bool,
    /// Two buds and a case, rather than over-ear headphones.
    pub buds: bool,
}

/// The AirPods' own settings, as they last told them (None: not told).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BudSettings {
    pub listening_mode: Option<ListeningMode>,
    /// The modes press and hold goes through.
    pub press_and_hold_modes: Vec<ListeningMode>,
    pub conversation_awareness: Option<bool>,
    pub personalized_volume: Option<bool>,
    /// How much Adaptive lets in: 0 more, 100 less.
    pub adaptive_strength: Option<u8>,
    pub ear_detection: Option<bool>,
    pub one_bud_noise_cancellation: Option<bool>,
    pub volume_swipe: Option<bool>,
    pub case_sounds: Option<bool>,
    pub microphone: Option<BudMicrophone>,
    /// 0 default, 1 slower, 2 slowest.
    pub press_speed: Option<u8>,
    /// 0 default, 1 shorter, 2 shortest.
    pub hold_duration: Option<u8>,
}

/// How the AirPods' sound is carried, as the sound server has it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Sound {
    /// The sound server's output for them ("" while there's none).
    pub output: String,
    /// Its headset microphone (BlueZ's: call quality while used), when
    /// it has one.
    pub input: String,
    /// Their own microphone's source (Olo's: wideband, the sound staying
    /// at music quality), while their channel is open.
    pub microphone: String,
    /// "aac", "sbc", "msbc", "cvsd" …
    pub codec: String,
    /// Music quality (A2DP) rather than call quality (the headset
    /// profile, which the microphone needs).
    pub high_quality: bool,
    /// The sound card, and the profiles to switch between.
    pub card: String,
}

/// A pair of AirPods paired with this computer.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AirPods {
    /// The Bluetooth address ("AA:BB:CC:DD:EE:FF").
    pub address: String,
    /// BlueZ's object.
    pub path: String,
    /// Their name, as BlueZ has it ("AirPods Pro", or what they were renamed).
    pub name: String,
    /// The model, said ("AirPods Pro (2nd generation, USB-C)").
    pub model: String,
    /// Apple's Bluetooth product id.
    pub product: u16,
    /// Apple's model number ("A3048"), serial and firmware, once told.
    pub model_number: String,
    pub serial: String,
    pub firmware: String,
    pub abilities: Abilities,
    /// Connected over Bluetooth.
    pub connected: bool,
    /// Olo is talking to them (their levels and settings are current).
    pub linked: bool,
    pub left: Option<Charge>,
    pub right: Option<Charge>,
    pub case: Option<Charge>,
    /// Over-ear headphones' one level.
    pub whole: Option<Charge>,
    /// Buds in an ear (0–2).
    pub in_ear: u8,
    /// Buds in their case.
    pub in_case: u8,
    /// Out of their case (or its lid open) near this computer: heard in
    /// their broadcast lately. The levels above come from it while they
    /// aren't connected here.
    pub nearby: bool,
    /// Connected to another device (a phone), as their broadcast says.
    pub elsewhere: bool,
    /// The wearer is speaking (Conversation Awareness has turned the
    /// sound down).
    pub speaking: bool,
    pub settings: BudSettings,
    pub sound: Sound,
}

impl AirPods {
    /// "L 80% · R 75% · Case 50%" (with ⚡ for what charges), or "" when
    /// nothing is known.
    pub fn levels(&self) -> String {
        let one = |name: &str, c: &Option<Charge>| c.map(|c| format!("{name} {}%{}", c.percent, if c.charging { " ⚡" } else { "" }));
        [one("L", &self.left), one("R", &self.right), one("Case", &self.case), one("", &self.whole).map(|s| s.trim().to_owned())].into_iter().flatten().collect::<Vec<_>>().join(" · ")
    }

    /// The lowest level of what's in use (both buds, or the headphones).
    pub fn lowest(&self) -> Option<u8> {
        [self.left, self.right, self.whole].into_iter().flatten().map(|c| c.percent).min()
    }
}

/// Everything the service knows.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AirPodsState {
    /// The service's process: a file it left behind when it died is no one's.
    pub pid: u32,
    pub devices: Vec<AirPods>,
    /// Their broadcasts are heard (for connecting and switching
    /// automatically, and their levels while not connected); if not, why.
    pub listening: bool,
    pub not_listening: Option<String>,
}

impl AirPodsState {
    pub fn path() -> PathBuf {
        runtime_dir().join("olo-airpods.json")
    }

    pub fn socket() -> PathBuf {
        runtime_dir().join("olo-airpods.sock")
    }

    pub fn load() -> AirPodsState {
        let state: AirPodsState = std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        if state.pid != 0 && !std::path::Path::new(&format!("/proc/{}", state.pid)).exists() {
            return AirPodsState::default();
        }
        state
    }

    /// Write it (atomically).
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        let tmp = path.with_extension("json.part");
        std::fs::write(&tmp, serde_json::to_vec(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, &path)
    }

    /// The pair connected (the first, if several are).
    pub fn connected(&self) -> Option<&AirPods> {
        self.devices.iter().find(|d| d.connected)
    }

    pub fn by_address(&self, address: &str) -> Option<&AirPods> {
        self.devices.iter().find(|d| d.address.eq_ignore_ascii_case(address))
    }
}

/// Where sound is recorded from while AirPods are connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MicrophoneChoice {
    /// The computer's own microphones; the AirPods keep playing at music
    /// quality.
    #[default]
    Computer,
    /// The AirPods' microphone, as Apple's devices use it: their own
    /// wideband stream, the AirPods still playing at music quality.
    AirPods,
    /// The AirPods' microphone over Bluetooth's headset profile, as any
    /// headset's (playback drops to call quality while it's used).
    AirPodsHeadset,
}

/// Olo's own settings for AirPods (Preferences ▸ Bluetooth and ▸ Sound).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AirPodsSettings {
    /// Pause what's playing when a bud comes out, and play again when
    /// it's back.
    pub pause_when_removed: bool,
    /// Turn the sound down while the wearer speaks (Conversation
    /// Awareness tells when).
    pub lower_when_speaking: bool,
    /// Connect when they're taken out of the case near this computer and
    /// aren't in use by anything else.
    pub connect_automatically: bool,
    /// Take the AirPods when something plays here, and give them back
    /// when the phone needs them, as Apple's devices do.
    pub switch_automatically: bool,
    pub microphone: MicrophoneChoice,
}

impl Default for AirPodsSettings {
    fn default() -> AirPodsSettings {
        AirPodsSettings { pause_when_removed: true, lower_when_speaking: true, connect_automatically: true, switch_automatically: true, microphone: MicrophoneChoice::Computer }
    }
}

impl AirPodsSettings {
    pub fn path() -> PathBuf {
        olo_xdg::config_dir().join("airpods.json")
    }

    pub fn load() -> AirPodsSettings {
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

/// A change to the AirPods' own settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "setting", content = "value")]
pub enum Change {
    ListeningMode(ListeningMode),
    PressAndHoldModes(Vec<ListeningMode>),
    ConversationAwareness(bool),
    PersonalizedVolume(bool),
    AdaptiveStrength(u8),
    EarDetection(bool),
    OneBudNoiseCancellation(bool),
    VolumeSwipe(bool),
    CaseSounds(bool),
    Microphone(BudMicrophone),
    PressSpeed(u8),
    HoldDuration(u8),
    Name(String),
}

/// What may be asked of the service. `address` None: the pair connected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "do")]
pub enum Request {
    /// Everything it knows (answered with the state).
    Status,
    Set { address: Option<String>, change: Change },
    Connect { address: Option<String> },
    Disconnect { address: Option<String> },
    /// Music quality, or the headset profile (call quality, with the
    /// microphone).
    Quality { address: Option<String>, high: bool },
    /// Start or stop their own microphone stream (wideband, the sound
    /// staying at music quality).
    Microphone { address: Option<String>, on: bool },
}

/// The service's answer.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Reply {
    pub ok: bool,
    pub error: Option<String>,
    pub state: Option<AirPodsState>,
}

/// Ask the service (blocks up to `timeout`; call off the UI thread).
pub fn ask(request: &Request, timeout: std::time::Duration) -> std::io::Result<Reply> {
    use std::io::{BufRead, Write};
    let stream = std::os::unix::net::UnixStream::connect(AirPodsState::socket())?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let mut line = serde_json::to_vec(request).map_err(std::io::Error::other)?;
    line.push(b'\n');
    (&stream).write_all(&line)?;
    let mut answer = String::new();
    std::io::BufReader::new(&stream).read_line(&mut answer)?;
    serde_json::from_str(&answer).map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_read_as_json() {
        let r = Request::Set { address: None, change: Change::ListeningMode(ListeningMode::NoiseCancellation) };
        let text = serde_json::to_string(&r).unwrap();
        assert_eq!(text, r#"{"do":"set","address":null,"change":{"setting":"listening-mode","value":"noise-cancellation"}}"#);
        assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), r);
        assert_eq!(ListeningMode::from_word("anc"), Some(ListeningMode::NoiseCancellation));
    }

    #[test]
    fn levels_said() {
        let a = AirPods { left: Some(Charge { percent: 80, charging: false }), right: Some(Charge { percent: 75, charging: true }), case: None, ..Default::default() };
        assert_eq!(a.levels(), "L 80% · R 75% ⚡");
        assert_eq!(a.lowest(), Some(75));
    }
}
