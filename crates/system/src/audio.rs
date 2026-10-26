//! The default output's volume, over the PulseAudio native protocol (which
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
        let cookie = pulseaudio::cookie_path_from_env().and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
        let auth = protocol::AuthParams { version: protocol::MAX_VERSION, supports_shm: false, supports_memfd: false, cookie };
        let reply: protocol::AuthReply = c.request(&Command::Auth(auth))?;
        c.version = protocol::MAX_VERSION.min(reply.version);
        let mut props = protocol::Props::new();
        props.set(protocol::Prop::ApplicationName, CString::new("Tabula").unwrap());
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
        let ch = s.cvolume.channels();
        let level = if ch.is_empty() {
            0.0
        } else {
            ch.iter().map(|v| v.as_u32() as f32).sum::<f32>() / ch.len() as f32 / PaVolume::NORM.as_u32() as f32
        };
        Ok(Volume { level, muted: s.muted })
    }
}

pub fn volume() -> Result<Volume> {
    Conn::connect()?.volume()
}

/// Set every channel of the default output to `level` (0.0–1.0).
pub fn set_volume(level: f32) -> Result<()> {
    let mut c = Conn::connect()?;
    let channels = c.sink()?.cvolume.channels().len().max(1);
    let raw = (level.clamp(0.0, 1.5) * PaVolume::NORM.as_u32() as f32).round() as u32;
    let mut volume = ChannelVolume::empty();
    for _ in 0..channels {
        volume.push(PaVolume::from_u32_clamped(raw));
    }
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
