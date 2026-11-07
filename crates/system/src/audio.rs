//! Sound devices and their volumes, over the PulseAudio native protocol (which
//! PipeWire serves too), with a change feed instead of polling.

use std::{ffi::CString, io::BufReader, os::unix::net::UnixStream, path::PathBuf, time::Duration};

use pulseaudio::protocol::{self, ChannelVolume, Command, CommandReply, ProtocolError, SetDeviceMuteParams, SetDeviceVolumeParams, SubscriptionMask, Volume as PaVolume};

pub type Result<T> = std::result::Result<T, ProtocolError>;

const DEFAULT_SINK: &str = "@DEFAULT_SINK@";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Volume {
    /// 0.0–1.0 (and above when amplified), on the server's cubic scale, so
    /// it matches what other mixers show.
    pub level: f32,
    pub muted: bool,
}

/// The sound server's socket, as libpulse finds it (`PULSE_SERVER`,
/// `PULSE_RUNTIME_PATH`, `XDG_RUNTIME_DIR`).
pub(crate) fn socket_path() -> Option<PathBuf> {
    if let Ok(s) = std::env::var("PULSE_SERVER")
        && let Some(p) = s.strip_prefix("unix:")
    {
        return Some(p.into());
    }
    let runtime = std::env::var_os("PULSE_RUNTIME_PATH").map(PathBuf::from).or_else(|| std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join("pulse")))?;
    let p = runtime.join("native");
    p.exists().then_some(p)
}

/// The cookie to show the sound server. No cookie file (a new account, a
/// sandbox): one of zeros, the length the protocol wants. PipeWire doesn't
/// check what's in it, but turns away an empty one, and with it every
/// sound control.
pub(crate) fn cookie() -> Vec<u8> {
    pulseaudio::cookie_path_from_env().and_then(|p| std::fs::read(p).ok()).filter(|c| c.len() == 256).unwrap_or_else(|| vec![0; 256])
}

struct Conn {
    sock: BufReader<UnixStream>,
    version: u16,
    seq: u32,
}

impl Conn {
    fn connect() -> Result<Conn> {
        let path = socket_path().ok_or_else(|| ProtocolError::Invalid("no sound server".into()))?;
        let stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut c = Conn { sock: BufReader::new(stream), version: protocol::MAX_VERSION, seq: 0 };
        let cookie = cookie();
        let auth = protocol::AuthParams { version: protocol::MAX_VERSION, supports_shm: false, supports_memfd: false, cookie };
        let reply: protocol::AuthReply = c.request(&Command::Auth(auth))?;
        c.version = protocol::MAX_VERSION.min(reply.version);
        let mut props = protocol::Props::new();
        props.set(protocol::Prop::ApplicationName, CString::new("Olo").unwrap());
        let _: protocol::SetClientNameReply = c.request(&Command::SetClientName(props))?;
        Ok(c)
    }

    fn send(&mut self, cmd: &Command) -> Result<()> {
        self.seq += 1;
        protocol::write_command_message(self.sock.get_mut(), self.seq, cmd, self.version)
    }

    fn request<T: CommandReply>(&mut self, cmd: &Command) -> Result<T> {
        self.send(cmd)?;
        Ok(protocol::read_reply_message::<T>(&mut self.sock, self.version)?.1)
    }

    fn ack(&mut self, cmd: &Command) -> Result<()> {
        self.send(cmd)?;
        protocol::read_ack_message(&mut self.sock).map(|_| ())
    }

    fn sink(&mut self) -> Result<protocol::SinkInfo> {
        let q = protocol::GetSinkInfo { index: None, name: Some(CString::new(DEFAULT_SINK).unwrap()) };
        self.request(&Command::GetSinkInfo(q))
    }

    fn volume(&mut self) -> Result<Volume> {
        let s = self.sink()?;
        Ok(Volume { level: level(&s.cvolume), muted: s.muted })
    }

    fn devices(&mut self) -> Result<Devices> {
        let server: protocol::ServerInfo = self.request(&Command::GetServerInfo)?;
        let default_out = server.default_sink_name.map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let default_in = server.default_source_name.map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let sinks: protocol::SinkInfoList = self.request(&Command::GetSinkInfoList)?;
        let sources: protocol::SourceInfoList = self.request(&Command::GetSourceInfoList)?;
        let device = |name: &CString, description: &Option<CString>, cvolume: &ChannelVolume, muted: bool, default: &str, props: &protocol::Props| {
            let name = name.to_string_lossy().into_owned();
            Device {
                private: bluetooth(props).is_some() || prop(props, "device.form_factor").is_some_and(|f| matches!(f.as_str(), "headphone" | "headset" | "handset" | "hands-free" | "earpiece")),
                bluetooth: bluetooth(props),
                description: description.as_ref().map(|d| tidy_description(&d.to_string_lossy())).filter(|d| !d.is_empty()).unwrap_or_else(|| name.clone()),
                is_default: name == default,
                volume: Volume { level: level(cvolume), muted },
                channels: cvolume.channels().len().max(1),
                name,
            }
        };
        let outputs: Vec<Device> = sinks
            .iter()
            .map(|s| {
                let mut d = device(&s.name, &s.description, &s.cvolume, s.muted, &default_out, &s.props);
                // What the active port is: headphones in the jack are
                // private however the card is described.
                use protocol::port_info::PortType;
                if let Some(port) = s.ports.get(s.active_port) {
                    d.private |= matches!(port.port_type, PortType::Headphones | PortType::Headset | PortType::Handset | PortType::Earpiece | PortType::Handsfree);
                }
                d
            })
            .collect();
        // A sink's monitor isn't a microphone.
        let inputs = sources.iter().filter(|s| s.monitor_of_sink_index.is_none()).map(|s| device(&s.name, &s.description, &s.cvolume, s.muted, &default_in, &s.props)).collect();
        // Bluetooth devices connected with nothing playing through them.
        let cards: protocol::CardInfoList = self.request(&Command::GetCardInfoList)?;
        let silent = unplayable(
            &outputs,
            cards.iter().filter_map(|k| {
                Some(SilentCard {
                    card: k.name.to_string_lossy().into_owned(),
                    description: prop(&k.props, "device.description").unwrap_or_default(),
                    address: prop(&k.props, "api.bluez5.address")?,
                    active: k.active_profile.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
                    profiles: k.profiles.iter().filter(|p| p.available != 0).map(|p| p.name.to_string_lossy().into_owned()).collect(),
                    plays: k.profiles.iter().any(|p| plays(&p.name.to_string_lossy())),
                })
            }),
        );
        Ok(Devices { outputs, inputs, no_output: false, unplayable: silent })
    }
}

/// Sound goes nowhere: Olo's silent output ([`NO_OUTPUT`]) made (once a
/// login: the sound server forgets it) and made the default. The sound
/// server's own default (WirePlumber keeps it) comes back to it whenever
/// it is made again.
pub fn set_no_output() -> Result<()> {
    ensure_no_output()?;
    set_default(Kind::Output, NO_OUTPUT)
}

/// Remember whether None is the output, for the next login.
fn remember_no_output(on: bool) {
    let choice = olo_protocol::sound::SoundChoice { no_output: on };
    if olo_protocol::sound::SoundChoice::load() != choice {
        let _ = choice.save();
    }
}

/// Olo's silent output, made if it isn't there (at login, when None was
/// the choice: `olo_protocol::SoundChoice`).
pub fn ensure_no_output() -> Result<()> {
    if Conn::connect()?.devices()?.outputs.iter().any(|d| d.name == NO_OUTPUT) {
        return Ok(());
    }
    let mut c = Conn::connect()?;
    let args = format!("sink_name={NO_OUTPUT} sink_properties=device.description=None");
    c.ack(&Command::LoadModule(protocol::LoadModuleParams {
        name: CString::new("module-null-sink").expect("no NUL"),
        arguments: Some(CString::new(args).map_err(|e| ProtocolError::Invalid(e.to_string()))?),
    }))
}

/// A property's text.
fn prop(props: &protocol::Props, key: &str) -> Option<String> {
    let key = CString::new(key).ok()?;
    let v = props.get_bytes(&key)?;
    Some(String::from_utf8_lossy(v.strip_suffix(&[0]).unwrap_or(v)).into_owned())
}

/// How a Bluetooth device's sound is carried (PipeWire's properties).
fn bluetooth(props: &protocol::Props) -> Option<Bluetooth> {
    Some(Bluetooth { address: prop(props, "api.bluez5.address")?, codec: prop(props, "api.bluez5.codec").unwrap_or_default(), profile: prop(props, "api.bluez5.profile").unwrap_or_default() })
}

/// Drop Apple board codes ("MacBook Pro J416 Speakers" → "MacBook Pro
/// Speakers"), which mean nothing to people.
fn tidy_description(s: &str) -> String {
    let board = |w: &str| {
        let mut c = w.chars();
        c.next() == Some('J') && {
            let rest: String = c.collect();
            let digits = rest.trim_end_matches(|ch: char| ch.is_ascii_lowercase());
            (3..=4).contains(&digits.len()) && digits.chars().all(|ch| ch.is_ascii_digit())
        }
    };
    s.split_whitespace().filter(|w| !board(w)).collect::<Vec<_>>().join(" ")
}

fn level(cvolume: &ChannelVolume) -> f32 {
    let ch = cvolume.channels();
    if ch.is_empty() { 0.0 } else { ch.iter().map(|v| v.as_u32() as f32).sum::<f32>() / ch.len() as f32 / PaVolume::NORM.as_u32() as f32 }
}

fn channel_volume(level: f32, channels: usize) -> ChannelVolume {
    let raw = (level.clamp(0.0, 1.5) * PaVolume::NORM.as_u32() as f32).round() as u32;
    let mut volume = ChannelVolume::empty();
    for _ in 0..channels.max(1) {
        volume.push(PaVolume::from_u32_clamped(raw));
    }
    volume
}

/// Speakers and headphones, or microphones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Output,
    Input,
}

/// A sound device as the server knows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    /// The server's name for it (what the setters take).
    pub name: String,
    /// What to show ("MacBook Pro Speakers").
    pub description: String,
    pub volume: Volume,
    pub channels: usize,
    pub is_default: bool,
    /// A Bluetooth device's address and how its sound is carried.
    pub bluetooth: Option<Bluetooth>,
    /// Heard (or hearing) only by the person: headphones, a headset, an
    /// earpiece, anything Bluetooth. An output that isn't is heard by the
    /// microphone too (echo cancellation's business).
    pub private: bool,
}

/// How a Bluetooth device's sound is carried.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Bluetooth {
    /// "AA:BB:CC:DD:EE:FF".
    pub address: String,
    /// "aac", "sbc", "msbc", "cvsd", "lc3" …
    pub codec: String,
    /// "a2dp-sink" (music), "headset-head-unit" (calls) …
    pub profile: String,
}

impl Bluetooth {
    /// Music quality (A2DP), not a headset's call quality.
    pub fn high_quality(&self) -> bool {
        self.profile.starts_with("a2dp")
    }

    /// The codec as people know it ("AAC").
    pub fn codec_name(&self) -> String {
        match self.codec.as_str() {
            "sbc_xq" => "SBC XQ".to_owned(),
            "msbc" => "mSBC".to_owned(),
            "aptx_hd" => "aptX HD".to_owned(),
            "aptx" => "aptX".to_owned(),
            "lc3_swb" => "LC3-SWB".to_owned(),
            c => c.to_uppercase(),
        }
    }
}

/// A sound card (a Bluetooth device's, here) and the ways it can work.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Card {
    pub name: String,
    /// The Bluetooth address, for a Bluetooth device's.
    pub address: Option<String>,
    pub profile: String,
    /// Name, description, available.
    pub profiles: Vec<(String, String, bool)>,
}

/// The sound server's cards.
pub fn cards() -> Result<Vec<Card>> {
    let mut c = Conn::connect()?;
    let list: protocol::CardInfoList = c.request(&Command::GetCardInfoList)?;
    Ok(list
        .iter()
        .map(|k| Card {
            name: k.name.to_string_lossy().into_owned(),
            address: prop(&k.props, "api.bluez5.address"),
            profile: k.active_profile.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            profiles: k
                .profiles
                .iter()
                .map(|p| (p.name.to_string_lossy().into_owned(), p.description.as_ref().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default(), p.available != 0))
                .collect(),
        })
        .collect())
}

/// Switch card `card` to `profile` ("a2dp-sink", "headset-head-unit").
pub fn set_card_profile(card: &str, profile: &str) -> Result<()> {
    let bad = |e: std::ffi::NulError| ProtocolError::Invalid(e.to_string());
    Conn::connect()?.ack(&Command::SetCardProfile(protocol::SetCardProfileParams {
        card_index: None,
        card_name: Some(CString::new(card).map_err(bad)?),
        profile_name: CString::new(profile).map_err(bad)?,
    }))
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Devices {
    pub outputs: Vec<Device>,
    pub inputs: Vec<Device>,
    /// Sound goes nowhere: "None" chosen as the output ([`set_no_output`]).
    pub no_output: bool,
    /// Bluetooth devices connected with no output: nothing plays through
    /// them (Ben's AirPods, 2026-11-04: an input, but no output).
    pub unplayable: Vec<Unplayable>,
}

/// A Bluetooth device the sound server has (its card) with no output, and
/// why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Unplayable {
    /// The card's name (what [`set_card_profile`] takes).
    pub card: String,
    /// "AirPods Pro".
    pub description: String,
    pub address: String,
    pub why: Silence,
    /// The profile that plays through it (music's, A2DP, else a call's),
    /// if it offers one now.
    pub profile: Option<String>,
}

/// Why a connected Bluetooth device plays nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Silence {
    /// Its sound is turned off (its card's profile is "off").
    #[default]
    Off,
    /// It offers no way to play sound now (no music or call profile
    /// available: BlueZ's audio connection didn't come up).
    NoWay,
    /// A way is chosen, yet no output came of it.
    NotStarted,
}

impl Silence {
    /// Said after "… is connected but plays no sound:".
    pub fn said(self) -> &'static str {
        match self {
            Silence::Off => "its sound is turned off",
            Silence::NoWay => "it offers no way to play sound (its Bluetooth audio didn't connect)",
            Silence::NotStarted => "its sound didn't start",
        }
    }
}

/// A Bluetooth card as `unplayable` judges it.
struct SilentCard {
    card: String,
    description: String,
    address: String,
    active: String,
    /// Its profiles available now.
    profiles: Vec<String>,
    /// Whether it has a profile that plays at all, available now or not
    /// (a phone sending its sound here has none: it isn't missing one).
    plays: bool,
}

/// A profile that plays sound through a device: music's (A2DP) or a
/// call's (HFP/HSP).
fn plays(profile: &str) -> bool {
    profile.starts_with("a2dp-sink") || profile.starts_with("headset-head-unit")
}

/// The Bluetooth cards among `cards` with no output among `outputs` (by
/// address), each with why and the profile that would play through it.
fn unplayable(outputs: &[Device], cards: impl Iterator<Item = SilentCard>) -> Vec<Unplayable> {
    cards
        .filter(|c| c.plays && !outputs.iter().any(|o| o.bluetooth.as_ref().is_some_and(|b| b.address.eq_ignore_ascii_case(&c.address))))
        .map(|c| {
            let playing = |prefix: &str| c.profiles.iter().find(|p| p.starts_with(prefix)).cloned();
            let profile = playing("a2dp-sink").or_else(|| playing("headset-head-unit"));
            let why = match (&profile, c.active.as_str()) {
                (None, _) => Silence::NoWay,
                (Some(_), "off" | "") => Silence::Off,
                (Some(_), _) => Silence::NotStarted,
            };
            let description = if c.description.is_empty() { c.address.clone() } else { c.description };
            Unplayable { card: c.card, description, address: c.address, why, profile }
        })
        .collect()
}

/// The output that is none: a silent sink of Olo's (PipeWire's null sink),
/// made the default when the person chooses None, so sound goes nowhere
/// rather than to the only device there is (a speaker in another room).
/// Never listed as a device ([`Devices::presented`]).
pub const NO_OUTPUT: &str = "olo.no-output";

impl Devices {
    /// See [`devices`].
    pub fn presented(mut self) -> Devices {
        use olo_protocol::voice;
        // None, chosen: not a device, but the choice.
        if let Some(i) = self.outputs.iter().position(|d| d.name == NO_OUTPUT) {
            self.no_output = self.outputs.remove(i).is_default;
        }
        if let Some(i) = self.inputs.iter().position(|d| d.name == voice::NODE) {
            let processed = self.inputs.remove(i);
            if processed.is_default {
                let listening = voice::State::load().microphone;
                for d in &mut self.inputs {
                    d.is_default = listening.as_deref() == Some(d.name.as_str());
                }
            }
        }
        self
    }

    pub fn default_device(&self, kind: Kind) -> Option<&Device> {
        match kind {
            Kind::Output => &self.outputs,
            Kind::Input => &self.inputs,
        }
        .iter()
        .find(|d| d.is_default)
    }
}

/// The device lists as people see them: the voice service's processed
/// source ([`olo_protocol::voice::NODE`]) isn't one of them; the microphone
/// it listens to stands in its place (and is the default when it is).
pub fn devices() -> Result<Devices> {
    Conn::connect()?.devices().map(Devices::presented)
}

/// The device lists as the sound server has them (the voice service's own
/// source included).
pub fn devices_raw() -> Result<Devices> {
    Conn::connect()?.devices()
}

/// Make `name` where new sound goes (or comes from).
pub fn set_default(kind: Kind, name: &str) -> Result<()> {
    if kind == Kind::Output {
        remember_no_output(name == NO_OUTPUT);
    }
    let name = CString::new(name).map_err(|e| ProtocolError::Invalid(e.to_string()))?;
    let mut c = Conn::connect()?;
    c.ack(&match kind {
        Kind::Output => Command::SetDefaultSink(name),
        Kind::Input => Command::SetDefaultSource(name),
    })
}

/// Set every channel of device `name` to `level` (0.0–1.0).
pub fn set_device_volume(kind: Kind, name: &str, level: f32, channels: usize) -> Result<()> {
    let params = SetDeviceVolumeParams { device_index: None, device_name: Some(CString::new(name).map_err(|e| ProtocolError::Invalid(e.to_string()))?), volume: channel_volume(level, channels) };
    Conn::connect()?.ack(&match kind {
        Kind::Output => Command::SetSinkVolume(params),
        Kind::Input => Command::SetSourceVolume(params),
    })
}

pub fn set_device_muted(kind: Kind, name: &str, mute: bool) -> Result<()> {
    let params = SetDeviceMuteParams { device_index: None, device_name: Some(CString::new(name).map_err(|e| ProtocolError::Invalid(e.to_string()))?), mute };
    Conn::connect()?.ack(&match kind {
        Kind::Output => Command::SetSinkMute(params),
        Kind::Input => Command::SetSourceMute(params),
    })
}

/// Like [`watch`], for the whole device list as people see it
/// ([`devices`]; Preferences ▸ Sound, the Deskbar).
pub fn watch_devices(mut changed: impl FnMut(Option<Devices>)) -> ! {
    watch_devices_raw(move |d| changed(d.map(Devices::presented)))
}

/// [`watch_devices`], as the sound server has them ([`devices_raw`]).
pub fn watch_devices_raw(mut changed: impl FnMut(Option<Devices>)) -> ! {
    let mut last: Option<Option<Devices>> = None;
    let mut report = |d: Option<Devices>| {
        if last.as_ref() != Some(&d) {
            last = Some(d.clone());
            changed(d);
        }
    };
    loop {
        let r: Result<()> = (|| {
            let mut query = Conn::connect()?;
            let mut events = Conn::connect()?;
            events.sock.get_ref().set_read_timeout(None)?;
            // (Cards too: a Bluetooth device's profile turned on or off.)
            events.ack(&Command::Subscribe(SubscriptionMask::SINK | SubscriptionMask::SOURCE | SubscriptionMask::SERVER | SubscriptionMask::CARD))?;
            report(Some(query.devices()?));
            loop {
                let (_, cmd) = protocol::read_command_message(&mut events.sock, events.version)?;
                if let Command::SubscribeEvent(_) = cmd {
                    std::thread::sleep(Duration::from_millis(30));
                    report(Some(query.devices()?));
                }
            }
        })();
        if let Err(e) = r {
            tracing::debug!("sound server: {e}");
        }
        report(None);
        std::thread::sleep(Duration::from_secs(3));
    }
}

pub fn volume() -> Result<Volume> {
    Conn::connect()?.volume()
}

/// Set every channel of the default output to `level` (0.0–1.0).
pub fn set_volume(level: f32) -> Result<()> {
    let mut c = Conn::connect()?;
    let volume = channel_volume(level, c.sink()?.cvolume.channels().len());
    c.ack(&Command::SetSinkVolume(SetDeviceVolumeParams { device_index: None, device_name: Some(CString::new(DEFAULT_SINK).unwrap()), volume }))
}

pub fn set_muted(mute: bool) -> Result<()> {
    Conn::connect()?.ack(&Command::SetSinkMute(SetDeviceMuteParams { device_index: None, device_name: Some(CString::new(DEFAULT_SINK).unwrap()), mute }))
}

pub fn toggle_mute() -> Result<()> {
    let muted = volume()?.muted;
    set_muted(!muted)
}

/// Mute the microphone in use, or unmute it (a laptop's microphone key):
/// whether it is muted now. When that is Olo's voice processing
/// (`olo_voice`), the microphone it records from is muted with it: the
/// sound stops at the hardware, and a laptop's microphone light (the
/// kernel's `audio-micmute` trigger, following the card's capture
/// switch) says so.
pub fn toggle_input_mute() -> Result<bool> {
    let mut c = Conn::connect()?;
    let q = protocol::GetSourceInfo { index: None, name: Some(CString::new("@DEFAULT_SOURCE@").unwrap()) };
    let source: protocol::SourceInfo = c.request(&Command::GetSourceInfo(q))?;
    let mute = !source.muted;
    let mut sources = vec![source.index];
    if source.name.to_bytes().starts_with(b"olo_voice") {
        let outputs: protocol::SourceOutputInfoList = c.request(&Command::GetSourceOutputInfoList)?;
        sources.extend(outputs.iter().filter(|o| prop(&o.props, "node.name").as_deref() == Some(VOICE_MICROPHONE)).map(|o| o.source_index));
    }
    sources.dedup();
    for index in sources {
        let params = SetDeviceMuteParams { device_index: Some(index), device_name: None, mute };
        Conn::connect()?.ack(&Command::SetSourceMute(params))?;
    }
    Ok(mute)
}

/// The stream Olo's voice processing records the microphone with.
const VOICE_MICROPHONE: &str = "olo_voice.microphone";

/// Call `changed` with the volume now and whenever it (or the default
/// output) changes; None while there is no sound server. Never returns:
/// run it on its own thread.
pub fn watch(mut changed: impl FnMut(Option<Volume>)) -> ! {
    let mut last: Option<Option<Volume>> = None;
    let mut report = |v: Option<Volume>| {
        if last != Some(v) {
            changed(v);
            last = Some(v);
        }
    };
    loop {
        if let Err(e) = watch_once(&mut report) {
            tracing::debug!("sound server: {e}");
        }
        report(None);
        std::thread::sleep(Duration::from_secs(3));
    }
}

fn watch_once(report: &mut impl FnMut(Option<Volume>)) -> Result<()> {
    let mut query = Conn::connect()?;
    let mut events = Conn::connect()?;
    events.sock.get_ref().set_read_timeout(None)?;
    events.ack(&Command::Subscribe(SubscriptionMask::SINK | SubscriptionMask::SERVER))?;
    report(Some(query.volume()?));
    loop {
        let (_, cmd) = protocol::read_command_message(&mut events.sock, events.version)?;
        if let Command::SubscribeEvent(_) = cmd {
            // Events come in bursts (one per channel change); let them settle.
            std::thread::sleep(Duration::from_millis(30));
            report(Some(query.volume()?));
        }
    }
}

#[cfg(test)]
mod tests {

    /// A Bluetooth card with no output: said why (off, no way, not
    /// started), with the profile that plays (music's before a call's);
    /// one with an output isn't.
    #[test]
    fn bluetooth_cards_with_no_output() {
        let card = |address: &str, active: &str, profiles: &[&str]| SilentCard {
            card: format!("bluez_card.{address}"),
            description: "AirPods Pro".into(),
            address: address.into(),
            active: active.into(),
            profiles: profiles.iter().map(|p| p.to_string()).collect(),
            plays: true,
        };
        // A phone sending its sound here: nothing to play through.
        let phone = SilentCard { plays: false, ..card("EE:EE", "a2dp-source", &["a2dp-source", "audio-gateway"]) };
        let playing = Device {
            name: "bluez_output.AA_AA".into(),
            description: "AirPods".into(),
            volume: Volume { level: 0.5, muted: false },
            channels: 2,
            is_default: true,
            bluetooth: Some(Bluetooth { address: "AA:AA".into(), ..Bluetooth::default() }),
            private: true,
        };
        let got = unplayable(
            std::slice::from_ref(&playing),
            vec![
                card("aa:aa", "a2dp-sink", &["a2dp-sink"]),
                card("BB:BB", "off", &["headset-head-unit", "a2dp-sink-aac", "off"]),
                card("CC:CC", "off", &["off"]),
                card("DD:DD", "headset-head-unit", &["headset-head-unit"]),
                phone,
            ]
            .into_iter(),
        );
        let said: Vec<(&str, Silence, Option<&str>)> = got.iter().map(|u| (u.address.as_str(), u.why, u.profile.as_deref())).collect();
        assert_eq!(said, [("BB:BB", Silence::Off, Some("a2dp-sink-aac")), ("CC:CC", Silence::NoWay, None), ("DD:DD", Silence::NotStarted, Some("headset-head-unit"))]);
    }

    use super::*;

    #[test]
    fn channel_volumes() {
        let v = channel_volume(0.5, 2);
        assert_eq!(v.channels().len(), 2);
        assert!((level(&v) - 0.5).abs() < 0.001);
        assert_eq!(channel_volume(9.0, 0).channels().len(), 1);
        assert_eq!(tidy_description("MacBook Pro J416 Speakers"), "MacBook Pro Speakers");
        assert_eq!(tidy_description("MacBook Air J413s Microphone"), "MacBook Air Microphone");
        assert_eq!(tidy_description("JBL Flip 5"), "JBL Flip 5");
    }

    /// Reads (never changes) the real server: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_devices() {
        let d = devices().expect("sound server");
        for dev in d.outputs.iter().chain(&d.inputs) {
            eprintln!("{} {:?} {:?}", if dev.is_default { "*" } else { " " }, dev.description, dev.volume);
        }
        assert!(d.default_device(Kind::Output).is_some());
    }
}

#[cfg(test)]
mod live_write {
    use super::*;

    /// Writes the default input's own level and mute state back (a no-op
    /// that exercises the setters): `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn rewrite_input_unchanged() {
        let before = devices().unwrap();
        let mic = before.default_device(Kind::Input).expect("an input").clone();
        set_device_volume(Kind::Input, &mic.name, mic.volume.level, mic.channels).unwrap();
        set_device_muted(Kind::Input, &mic.name, mic.volume.muted).unwrap();
        set_default(Kind::Input, &mic.name).unwrap();
        let after = devices().unwrap();
        let again = after.default_device(Kind::Input).unwrap();
        assert_eq!(again.name, mic.name);
        assert_eq!(again.volume.muted, mic.volume.muted);
        assert!((again.volume.level - mic.volume.level).abs() < 0.001);
    }
}

/// Whether anything is playing (a playback stream that isn't paused),
/// told `changed` when that changes; None while there is no sound server.
/// For "switch automatically": the moment something starts to play.
pub fn watch_playing(mut changed: impl FnMut(Option<bool>)) -> ! {
    let mut last: Option<Option<bool>> = None;
    let mut report = |p: Option<bool>| {
        if last != Some(p) {
            last = Some(p);
            changed(p);
        }
    };
    loop {
        let r: Result<()> = (|| {
            let mut query = Conn::connect()?;
            let mut events = Conn::connect()?;
            events.sock.get_ref().set_read_timeout(None)?;
            events.ack(&Command::Subscribe(SubscriptionMask::SINK_INPUT))?;
            let playing = |q: &mut Conn| -> Result<bool> {
                let list: protocol::SinkInputInfoList = q.request(&Command::GetSinkInputInfoList)?;
                Ok(list.iter().any(|s| !s.corked))
            };
            report(Some(playing(&mut query)?));
            loop {
                let (_, cmd) = protocol::read_command_message(&mut events.sock, events.version)?;
                if let Command::SubscribeEvent(_) = cmd {
                    report(Some(playing(&mut query)?));
                }
            }
        })();
        if let Err(e) = r {
            tracing::debug!("sound server: {e}");
        }
        report(None);
        std::thread::sleep(Duration::from_secs(3));
    }
}

/// The apps recording from a microphone now (a call, a meeting): each
/// recording stream that isn't paused, on a source that isn't another's
/// monitor, and not Olo's own (its voice processing's stream, Olo's
/// programs: dictation, a screen recording), by its application's
/// name, told `changed` when that changes; None while there is no sound
/// server. Never returns: run it on its own thread.
pub fn watch_listening(mut changed: impl FnMut(Option<Vec<String>>)) -> ! {
    let mut last: Option<Option<Vec<String>>> = None;
    let mut report = |p: Option<Vec<String>>| {
        if last.as_ref() != Some(&p) {
            last = Some(p.clone());
            changed(p);
        }
    };
    loop {
        let r: Result<()> = (|| {
            let mut query = Conn::connect()?;
            let mut events = Conn::connect()?;
            events.sock.get_ref().set_read_timeout(None)?;
            events.ack(&Command::Subscribe(SubscriptionMask::SOURCE_OUTPUT))?;
            report(Some(listening(&mut query)?));
            loop {
                let (_, cmd) = protocol::read_command_message(&mut events.sock, events.version)?;
                if let Command::SubscribeEvent(_) = cmd {
                    // (Streams come and go in bursts as a call starts.)
                    std::thread::sleep(Duration::from_millis(30));
                    report(Some(listening(&mut query)?));
                }
            }
        })();
        if let Err(e) = r {
            tracing::debug!("sound server: {e}");
        }
        report(None);
        std::thread::sleep(Duration::from_secs(3));
    }
}

/// [`watch_listening`]'s apps, now.
fn listening(q: &mut Conn) -> Result<Vec<String>> {
    let sources: protocol::SourceInfoList = q.request(&Command::GetSourceInfoList)?;
    let monitors: std::collections::HashSet<u32> = sources.iter().filter(|s| s.monitor_of_sink_index.is_some()).map(|s| s.index).collect();
    let outputs: protocol::SourceOutputInfoList = q.request(&Command::GetSourceOutputInfoList)?;
    let mut apps: Vec<String> = outputs
        .iter()
        // (A monitor's, by its source, or as PipeWire marks one not yet
        // linked to it.)
        .filter(|o| !o.corked && !monitors.contains(&o.source_index) && prop(&o.props, "stream.capture.sink").as_deref() != Some("true"))
        .filter(|o| prop(&o.props, "node.name").as_deref() != Some(VOICE_MICROPHONE))
        .filter(|o| !prop(&o.props, "application.process.binary").is_some_and(|b| b.starts_with("olo-") || b.starts_with("olo_")))
        .map(|o| prop(&o.props, "application.name").or_else(|| prop(&o.props, "application.process.binary")).unwrap_or_else(|| "An app".to_owned()))
        .collect();
    apps.sort();
    apps.dedup();
    Ok(apps)
}

#[cfg(test)]
mod mic_mute_tests {
    /// The microphone key on this machine (`cargo test -p olo-system
    /// mic_key -- --ignored --nocapture`): muted, its light, unmuted.
    #[test]
    #[ignore]
    fn mic_key() {
        let led = || std::fs::read_to_string("/sys/class/leds/platform::micmute/brightness").unwrap_or_default().trim().to_owned();
        eprintln!("before: light {}", led());
        let muted = super::toggle_input_mute().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        eprintln!("muted {muted}: light {}", led());
        let muted = super::toggle_input_mute().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        eprintln!("muted {muted}: light {}", led());
    }
}

#[cfg(test)]
mod no_output_tests {
    /// None on this machine's sound server (`cargo test -- --ignored`):
    /// sound goes nowhere, and None isn't listed as a device.
    #[test]
    #[ignore]
    fn live_no_output() {
        super::set_no_output().unwrap();
        // (WirePlumber makes it the default a moment later.)
        let mut d = super::devices().unwrap();
        for _ in 0..40 {
            if d.no_output {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            d = super::devices().unwrap();
        }
        eprintln!("no_output={} outputs={:?}", d.no_output, d.outputs.iter().map(|o| (&o.description, o.is_default)).collect::<Vec<_>>());
        assert!(d.no_output && !d.outputs.iter().any(|o| o.name == super::NO_OUTPUT));
    }
}

#[cfg(test)]
mod listening_tests {
    /// The apps recording, as a sound server says them
    /// (`OLO_LISTEN_SECONDS=N cargo test -p olo-system listening_live --
    /// --ignored --nocapture`, with `PULSE_SERVER` a private server's).
    #[test]
    #[ignore]
    fn listening_live() {
        let seconds: u64 = std::env::var("OLO_LISTEN_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(5);
        let start = std::time::Instant::now();
        std::thread::spawn(move || {
            super::watch_listening(move |apps| eprintln!("{:.1}: {apps:?}", start.elapsed().as_secs_f32()));
        });
        std::thread::sleep(std::time::Duration::from_secs(seconds));
    }
}
