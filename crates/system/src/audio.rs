//! Sound devices and their volumes, over the PulseAudio native protocol (which
//! PipeWire serves too), with a change feed instead of polling.

use std::{
    ffi::CString,
    io::BufReader,
    os::unix::net::UnixStream,
    path::PathBuf,
    time::Duration,
};

use pulseaudio::protocol::{
    self, ChannelVolume, Command, CommandReply, ProtocolError, SetDeviceMuteParams, SetDeviceVolumeParams,
    SubscriptionMask, Volume as PaVolume,
};

pub type Result<T> = std::result::Result<T, ProtocolError>;

const DEFAULT_SINK: &str = "@DEFAULT_SINK@";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Volume {
    /// 0.0–1.0 (and above when amplified), on the server's cubic scale, so
    /// it matches what other mixers show.
    pub level: f32,
    pub muted: bool,
}

fn socket_path() -> Option<PathBuf> {
    if let Ok(s) = std::env::var("PULSE_SERVER")
        && let Some(p) = s.strip_prefix("unix:")
    {
        return Some(p.into());
    }
    let runtime = std::env::var_os("PULSE_RUNTIME_PATH")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join("pulse")))?;
    let p = runtime.join("native");
    p.exists().then_some(p)
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
        // No cookie file (a new account, a sandbox): a cookie of zeros, the
        // length the protocol wants. PipeWire doesn't check what's in it, but
        // turns away an empty one, and with it every sound control.
        let cookie = pulseaudio::cookie_path_from_env().and_then(|p| std::fs::read(p).ok()).filter(|c| c.len() == 256).unwrap_or_else(|| vec![0; 256]);
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
                private: bluetooth(props).is_some()
                    || prop(props, "device.form_factor").is_some_and(|f| matches!(f.as_str(), "headphone" | "headset" | "handset" | "hands-free" | "earpiece")),
                bluetooth: bluetooth(props),
                description: description
                    .as_ref()
                    .map(|d| tidy_description(&d.to_string_lossy()))
                    .filter(|d| !d.is_empty())
                    .unwrap_or_else(|| name.clone()),
                is_default: name == default,
                volume: Volume { level: level(cvolume), muted },
                channels: cvolume.channels().len().max(1),
                name,
            }
        };
        let outputs = sinks
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
        let inputs = sources
            .iter()
            .filter(|s| s.monitor_of_sink_index.is_none())
            .map(|s| device(&s.name, &s.description, &s.cvolume, s.muted, &default_in, &s.props))
            .collect();
        Ok(Devices { outputs, inputs })
    }
}

/// A property's text.
fn prop(props: &protocol::Props, key: &str) -> Option<String> {
    let key = CString::new(key).ok()?;
    let v = props.get_bytes(&key)?;
    Some(String::from_utf8_lossy(v.strip_suffix(&[0]).unwrap_or(v)).into_owned())
}

/// How a Bluetooth device's sound is carried (PipeWire's properties).
fn bluetooth(props: &protocol::Props) -> Option<Bluetooth> {
    Some(Bluetooth {
        address: prop(props, "api.bluez5.address")?,
        codec: prop(props, "api.bluez5.codec").unwrap_or_default(),
        profile: prop(props, "api.bluez5.profile").unwrap_or_default(),
    })
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
    if ch.is_empty() {
        0.0
    } else {
        ch.iter().map(|v| v.as_u32() as f32).sum::<f32>() / ch.len() as f32 / PaVolume::NORM.as_u32() as f32
    }
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
}

impl Devices {
    /// See [`devices`].
    pub fn presented(mut self) -> Devices {
        use olo_protocol::voice;
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
    let name = CString::new(name).map_err(|e| ProtocolError::Invalid(e.to_string()))?;
    let mut c = Conn::connect()?;
    c.ack(&match kind {
        Kind::Output => Command::SetDefaultSink(name),
        Kind::Input => Command::SetDefaultSource(name),
    })
}

/// Set every channel of device `name` to `level` (0.0–1.0).
pub fn set_device_volume(kind: Kind, name: &str, level: f32, channels: usize) -> Result<()> {
    let params = SetDeviceVolumeParams {
        device_index: None,
        device_name: Some(CString::new(name).map_err(|e| ProtocolError::Invalid(e.to_string()))?),
        volume: channel_volume(level, channels),
    };
    Conn::connect()?.ack(&match kind {
        Kind::Output => Command::SetSinkVolume(params),
        Kind::Input => Command::SetSourceVolume(params),
    })
}

pub fn set_device_muted(kind: Kind, name: &str, mute: bool) -> Result<()> {
    let params = SetDeviceMuteParams {
        device_index: None,
        device_name: Some(CString::new(name).map_err(|e| ProtocolError::Invalid(e.to_string()))?),
        mute,
    };
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
            events.ack(&Command::Subscribe(SubscriptionMask::SINK | SubscriptionMask::SOURCE | SubscriptionMask::SERVER))?;
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
    c.ack(&Command::SetSinkVolume(SetDeviceVolumeParams {
        device_index: None,
        device_name: Some(CString::new(DEFAULT_SINK).unwrap()),
        volume,
    }))
}

pub fn set_muted(mute: bool) -> Result<()> {
    Conn::connect()?.ack(&Command::SetSinkMute(SetDeviceMuteParams {
        device_index: None,
        device_name: Some(CString::new(DEFAULT_SINK).unwrap()),
        mute,
    }))
}

pub fn toggle_mute() -> Result<()> {
    let muted = volume()?.muted;
    set_muted(!muted)
}

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
