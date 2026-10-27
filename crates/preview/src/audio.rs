//! Playing audio in a preview: symphonia decodes, and samples go to the
//! sound server over the PulseAudio native protocol (spoken by PipeWire
//! too), implemented in Rust, so nothing outside the process is involved.

use std::{
    ffi::CString,
    io::BufReader,
    os::unix::net::UnixStream,
    sync::{
        Arc, Mutex,
        OnceLock,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc,
    },
};

use pulseaudio::protocol;
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{CODEC_TYPE_NULL, CODEC_TYPE_OPUS, Decoder, DecoderOptions},
    formats::{FormatOptions, FormatReader, SeekMode, SeekTo},
    io::MediaSourceStream,
    meta::MetadataOptions,
    probe::Hint,
    units::Time,
};

use crate::input::{self, Input};

#[derive(Default)]
pub struct State {
    /// Playback position in milliseconds.
    pub position_ms: AtomicU64,
    pub duration_ms: AtomicU64,
    pub playing: AtomicBool,
    pub finished: AtomicBool,
    pub error: Mutex<Option<String>>,
    /// Loudness in thousandths (1000 as recorded), applied to the samples.
    pub volume: AtomicU32,
}

enum Ev {
    Play,
    Pause,
    Seek(f64),
    Stop,
    Server(u32, Box<protocol::Command>),
    Timing(protocol::PlaybackLatency),
    Gone,
}

pub struct Player {
    tx: mpsc::Sender<Ev>,
    pub state: Arc<State>,
}

impl Player {
    /// Start playing `input` at once.
    pub fn start(input: &Input) -> Player {
        Player::start_with(input, true)
    }

    /// Start `input`, playing or paused at the beginning.
    pub fn start_with(input: &Input, playing: bool) -> Player {
        let (tx, rx) = mpsc::channel();
        let state = Arc::new(State { volume: AtomicU32::new(1000), ..Default::default() });
        let (input, st, tx2) = (input.clone(), state.clone(), tx.clone());
        std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || {
                if let Err(e) = run(&input, playing, &st, tx2, rx) {
                    tracing::warn!("audio {}: {e}", input.name);
                    *st.error.lock().unwrap() = Some(e);
                }
                st.playing.store(false, Ordering::Relaxed);
            })
            .ok();
        Player { tx, state }
    }

    pub fn toggle(&self) {
        let playing = self.state.playing.load(Ordering::Relaxed);
        if self.state.finished.load(Ordering::Relaxed) {
            self.state.finished.store(false, Ordering::Relaxed);
            let _ = self.tx.send(Ev::Seek(0.0));
            let _ = self.tx.send(Ev::Play);
        } else {
            let _ = self.tx.send(if playing { Ev::Pause } else { Ev::Play });
        }
    }

    pub fn set_playing(&self, on: bool) {
        let _ = self.tx.send(if on { Ev::Play } else { Ev::Pause });
    }

    pub fn seek(&self, secs: f64) {
        self.state.finished.store(false, Ordering::Relaxed);
        let _ = self.tx.send(Ev::Seek(secs));
    }

    /// From 0 (silent) to 1 (as recorded); takes effect within the
    /// server's short buffer.
    pub fn set_volume(&self, volume: f64) {
        self.state.volume.store((volume.clamp(0.0, 1.0) * 1000.0).round() as u32, Ordering::Relaxed);
    }
}

/// The length of `input`'s sound in seconds (0 when unknown).
pub fn probe(input: &Input) -> Result<f64, String> {
    Source::open(input).map(|(_, _, d)| d)
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.tx.send(Ev::Stop);
    }
}

struct Source {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    channels: usize,
    /// Interleaved S16LE bytes decoded but not yet sent.
    pending: Vec<u8>,
    eof: bool,
}

impl Source {
    fn open(input: &Input) -> Result<(Source, u32, f64), String> {
        let (reader, size) = input.open()?;
        let mss = MediaSourceStream::new(Box::new(input::Source { reader, size }), Default::default());
        let mut hint = Hint::new();
        if !input.ext.is_empty() {
            hint.with_extension(&input.ext);
        }
        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &FormatOptions { enable_gapless: true, ..Default::default() }, &MetadataOptions::default())
            .map_err(|_| "This audio format can't be played.".to_string())?;
        let format = probed.format;
        // The first track we can decode (a video's picture track may come
        // first).
        let tracks: Vec<_> = format.tracks().iter().filter(|t| t.codec_params.codec != CODEC_TYPE_NULL).collect();
        if tracks.is_empty() {
            return Err("There is no sound in this file.".into());
        }
        let (track, decoder) = tracks
            .iter()
            .find_map(|t| crate::opus::codecs().make(&t.codec_params, &DecoderOptions::default()).ok().map(|d| (*t, d)))
            .ok_or("This audio codec can't be played.")?;
        // The decoder's parameters: it may fill in what the container left out.
        let params = decoder.codec_params().clone();
        // Opus always decodes at 48 kHz; its header's rate is the original's.
        let rate = if params.codec == CODEC_TYPE_OPUS { 48_000 } else { params.sample_rate.ok_or("Unknown sample rate.")? };
        let channels = params.channels.map(|c| c.count()).unwrap_or(2).clamp(1, 2);
        let duration = match (params.n_frames, params.time_base) {
            (Some(n), Some(tb)) => {
                let t = tb.calc_time(n);
                t.seconds as f64 + t.frac
            }
            (Some(n), None) => n as f64 / rate as f64,
            _ => 0.0,
        };
        let id = track.id;
        Ok((Source { format, decoder, track: id, channels, pending: Vec::new(), eof: false }, rate, duration))
    }

    /// Fill `out` with up to `want` bytes; fewer only at the end.
    fn read(&mut self, want: usize, out: &mut Vec<u8>) {
        out.clear();
        while out.len() < want {
            if self.pending.is_empty() && !self.decode_next() {
                break;
            }
            let n = (want - out.len()).min(self.pending.len());
            out.extend(self.pending.drain(..n));
        }
    }

    fn decode_next(&mut self) -> bool {
        if self.eof {
            return false;
        }
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(_) => {
                    self.eof = true;
                    return false;
                }
            };
            if packet.track_id() != self.track {
                continue;
            }
            let Ok(decoded) = self.decoder.decode(&packet) else { continue };
            let spec = *decoded.spec();
            let src_channels = spec.channels.count();
            let mut sb = SampleBuffer::<i16>::new(decoded.capacity() as u64, spec);
            sb.copy_interleaved_ref(decoded);
            // Down-mix anything wider than stereo by keeping the front pair;
            // a mono stream the container called stereo plays in both.
            for frame in sb.samples().chunks(src_channels.max(1)) {
                for c in 0..self.channels {
                    let s = frame[c.min(frame.len() - 1)];
                    self.pending.extend_from_slice(&s.to_le_bytes());
                }
            }
            return true;
        }
    }

    fn seek(&mut self, secs: f64) {
        let t = Time::new(secs.max(0.0) as u64, secs.max(0.0).fract());
        let _ = self.format.seek(SeekMode::Coarse, SeekTo::Time { time: t, track_id: Some(self.track) });
        self.decoder.reset();
        self.pending.clear();
        self.eof = false;
    }
}

const SEQ_CREATE: u32 = 10;
const SEQ_TIMING: u32 = 20;
const SEQ_DRAIN: u32 = 30;
const SEQ_OTHER: u32 = 40;

/// How this process appears in the sound server's list of what's
/// playing: a name and an icon name, set once as the app starts.
static IDENTITY: OnceLock<(String, String)> = OnceLock::new();

/// Name this process to the sound server ("Viewer", "tabula-viewer").
/// Until it's called, sound plays as "Tabula".
pub fn set_identity(name: &str, icon: &str) {
    let _ = IDENTITY.set((name.into(), icon.into()));
}

fn connect() -> Result<(UnixStream, u16), String> {
    let path = pulseaudio::socket_path_from_env().ok_or("No sound server is running.")?;
    let sock = UnixStream::connect(path).map_err(|_| "The sound server can't be reached.".to_string())?;
    let mut r = BufReader::new(sock.try_clone().map_err(|e| e.to_string())?);
    let mut w = sock;
    let cookie = pulseaudio::cookie_path_from_env().and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
    let auth = protocol::AuthParams { version: protocol::MAX_VERSION, supports_shm: false, supports_memfd: false, cookie };
    let err = |e: protocol::ProtocolError| e.to_string();
    protocol::write_command_message(&mut w, 0, &protocol::Command::Auth(auth), protocol::MAX_VERSION).map_err(err)?;
    let (_, reply) = protocol::read_reply_message::<protocol::AuthReply>(&mut r, protocol::MAX_VERSION).map_err(err)?;
    let version = protocol::MAX_VERSION.min(reply.version);
    let mut props = protocol::Props::new();
    let (name, icon) = IDENTITY.get().map_or(("Tabula", "tabula"), |(n, i)| (n.as_str(), i.as_str()));
    props.set(protocol::Prop::ApplicationName, CString::new(name).unwrap_or_default());
    props.set(protocol::Prop::ApplicationIconName, CString::new(icon).unwrap_or_default());
    protocol::write_command_message(&mut w, 1, &protocol::Command::SetClientName(props), version).map_err(err)?;
    protocol::read_reply_message::<protocol::SetClientNameReply>(&mut r, version).map_err(err)?;
    Ok((w, version))
}

/// Shuts a socket down when dropped.
struct Closer(UnixStream);

impl Drop for Closer {
    fn drop(&mut self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
}

fn run(input: &Input, playing: bool, st: &State, tx: mpsc::Sender<Ev>, rx: mpsc::Receiver<Ev>) -> Result<(), String> {
    let (mut src, rate, duration) = Source::open(input)?;
    st.duration_ms.store((duration * 1000.0) as u64, Ordering::Relaxed);
    let (mut sock, version) = connect()?;
    // However this ends, the connection closes: its reader thread holds a
    // clone of the socket and would otherwise keep it (and itself) alive.
    let _closer = Closer(sock.try_clone().map_err(|e| e.to_string())?);
    let err = |e: protocol::ProtocolError| e.to_string();
    let bps = rate as u64 * src.channels as u64 * 2;
    let params = protocol::PlaybackStreamParams {
        sample_spec: protocol::SampleSpec { format: protocol::SampleFormat::S16Le, channels: src.channels as u8, sample_rate: rate },
        channel_map: if src.channels == 1 { protocol::ChannelMap::mono() } else { protocol::ChannelMap::stereo() },
        cvolume: Some(protocol::ChannelVolume::norm(src.channels as u8)),
        sink_name: Some(protocol::DEFAULT_SINK.to_owned()),
        // A short buffer, so pause and seek answer quickly.
        buffer_attr: protocol::stream::BufferAttr {
            max_length: u32::MAX,
            target_length: (bps / 5) as u32,
            pre_buffering: u32::MAX,
            minimum_request_length: u32::MAX,
            fragment_size: u32::MAX,
        },
        flags: protocol::stream::StreamFlags { start_corked: !playing, ..Default::default() },
        ..Default::default()
    };
    protocol::write_command_message(&mut sock, SEQ_CREATE, &protocol::Command::CreatePlaybackStream(params), version).map_err(err)?;
    let mut reader = BufReader::new(sock.try_clone().map_err(|e| e.to_string())?);
    let (_, info) = protocol::read_reply_message::<protocol::CreatePlaybackStreamReply>(&mut reader, version).map_err(err)?;
    let channel = info.channel;

    // Server messages arrive on their own thread and join the UI's
    // controls in one queue.
    {
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("audio-server".into())
            .spawn(move || {
                loop {
                    match protocol::read_command_message(&mut reader, version) {
                        Ok((seq, protocol::Command::Reply)) if seq == SEQ_TIMING => {
                            let mut ts = protocol::TagStructReader::new(&mut reader, version);
                            match ts.read::<protocol::PlaybackLatency>() {
                                Ok(t) => {
                                    if tx.send(Ev::Timing(t)).is_err() {
                                        return;
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        Ok((seq, cmd)) => {
                            if tx.send(Ev::Server(seq, Box::new(cmd))).is_err() {
                                return;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let _ = tx.send(Ev::Gone);
            })
            .map_err(|e| e.to_string())?;
    }

    let mut buf = Vec::new();
    // Bytes sent since the stream began, and where the read index was
    // reset by the last seek (the flush moves it to the write index).
    let mut written: i64 = 0;
    let mut base_bytes: i64 = 0;
    let mut base_secs = 0.0;
    let mut draining = false;
    let mut send_chunk = |src: &mut Source, sock: &mut UnixStream, len: usize, written: &mut i64, draining: &mut bool| -> Result<(), String> {
        if *draining {
            return Ok(());
        }
        src.read(len, &mut buf);
        let volume = st.volume.load(Ordering::Relaxed);
        if volume < 1000 {
            for s in buf.as_chunks_mut::<2>().0 {
                *s = ((i16::from_le_bytes(*s) as i32 * volume as i32 / 1000) as i16).to_le_bytes();
            }
        }
        if buf.is_empty() {
            protocol::write_command_message(sock, SEQ_DRAIN, &protocol::Command::DrainPlaybackStream(channel), version).map_err(err)?;
            *draining = true;
        } else {
            protocol::write_memblock(sock, channel, &buf, 0).map_err(err)?;
            *written += buf.len() as i64;
        }
        Ok(())
    };
    send_chunk(&mut src, &mut sock, info.requested_bytes as usize, &mut written, &mut draining)?;
    st.playing.store(playing, Ordering::Relaxed);

    let cork = |sock: &mut UnixStream, cork: bool| {
        protocol::write_command_message(sock, SEQ_OTHER, &protocol::Command::CorkPlaybackStream(protocol::CorkStreamParams { channel, cork }), version)
            .map_err(err)
    };
    for ev in rx {
        match ev {
            Ev::Stop | Ev::Gone => break,
            Ev::Play => {
                cork(&mut sock, false)?;
                st.playing.store(true, Ordering::Relaxed);
            }
            Ev::Pause => {
                cork(&mut sock, true)?;
                st.playing.store(false, Ordering::Relaxed);
            }
            Ev::Seek(secs) => {
                let secs = secs.clamp(0.0, duration.max(0.0));
                src.seek(secs);
                protocol::write_command_message(&mut sock, SEQ_OTHER, &protocol::Command::FlushPlaybackStream(channel), version).map_err(err)?;
                base_bytes = written;
                base_secs = secs;
                draining = false;
                st.position_ms.store((secs * 1000.0) as u64, Ordering::Relaxed);
            }
            Ev::Server(_, cmd) if matches!(*cmd, protocol::Command::Request(protocol::Request { channel: c, .. }) if c == channel) => {
                let protocol::Command::Request(protocol::Request { length, .. }) = *cmd else { continue };
                send_chunk(&mut src, &mut sock, length as usize, &mut written, &mut draining)?;
                protocol::write_command_message(
                    &mut sock,
                    SEQ_TIMING,
                    &protocol::Command::GetPlaybackLatency(protocol::LatencyParams { channel, now: std::time::SystemTime::now() }),
                    version,
                )
                .map_err(err)?;
            }
            Ev::Server(seq, cmd) if matches!(*cmd, protocol::Command::Reply) && seq == SEQ_DRAIN && draining => {
                st.finished.store(true, Ordering::Relaxed);
                st.playing.store(false, Ordering::Relaxed);
                st.position_ms.store((duration * 1000.0) as u64, Ordering::Relaxed);
                cork(&mut sock, true)?;
            }
            Ev::Server(_, cmd) if matches!(*cmd, protocol::Command::Error(_)) => return Err(format!("Sound server error: {cmd:?}")),
            Ev::Timing(t) => {
                let played = (t.read_offset - base_bytes).max(0) as f64 / bps as f64;
                if !st.finished.load(Ordering::Relaxed) {
                    st.position_ms.store(((base_secs + played) * 1000.0) as u64, Ordering::Relaxed);
                }
            }
            Ev::Server(..) => {}
        }
    }
    Ok(())
}


/// Sound that arrives as it happens (a call's other end): the owner
/// pushes 16-bit samples as they come, and the server takes them with a
/// short buffer. When too few have come it plays silence, and when too
/// many have piled up it drops the oldest, so the delay stays short.
pub struct LiveSink {
    shared: Arc<LiveShared>,
    sock: Option<UnixStream>,
}

struct LiveShared {
    queue: Mutex<std::collections::VecDeque<i16>>,
    volume: AtomicU32,
    stop: AtomicBool,
    rate: u32,
    channels: u8,
}

/// What the server keeps buffered, and the most we hold before dropping.
const LIVE_TARGET_MS: u64 = 60;
const LIVE_MAX_MS: u64 = 200;

impl LiveSink {
    /// Start a stream of `channels` interleaved channels at `rate`.
    pub fn start(rate: u32, channels: u8) -> Result<LiveSink, String> {
        let channels = channels.clamp(1, 2);
        let (mut sock, version) = connect()?;
        let err = |e: protocol::ProtocolError| e.to_string();
        let bytes = |ms: u64| (rate as u64 * channels as u64 * 2 * ms / 1000) as u32;
        let params = protocol::PlaybackStreamParams {
            sample_spec: protocol::SampleSpec { format: protocol::SampleFormat::S16Le, channels, sample_rate: rate },
            channel_map: if channels == 1 { protocol::ChannelMap::mono() } else { protocol::ChannelMap::stereo() },
            cvolume: Some(protocol::ChannelVolume::norm(channels)),
            sink_name: Some(protocol::DEFAULT_SINK.to_owned()),
            buffer_attr: protocol::stream::BufferAttr {
                max_length: u32::MAX,
                target_length: bytes(LIVE_TARGET_MS),
                pre_buffering: bytes(10),
                minimum_request_length: bytes(10),
                fragment_size: u32::MAX,
            },
            flags: protocol::stream::StreamFlags { adjust_latency: true, ..Default::default() },
            ..Default::default()
        };
        protocol::write_command_message(&mut sock, SEQ_CREATE, &protocol::Command::CreatePlaybackStream(params), version).map_err(err)?;
        let mut reader = BufReader::new(sock.try_clone().map_err(|e| e.to_string())?);
        let (_, info) = protocol::read_reply_message::<protocol::CreatePlaybackStreamReply>(&mut reader, version).map_err(err)?;
        let shared = Arc::new(LiveShared {
            queue: Mutex::new(std::collections::VecDeque::new()),
            volume: AtomicU32::new(1000),
            stop: AtomicBool::new(false),
            rate,
            channels,
        });
        let (sh, mut w) = (shared.clone(), sock.try_clone().map_err(|e| e.to_string())?);
        let channel = info.channel;
        std::thread::Builder::new()
            .name("live-sound".into())
            .spawn(move || {
                let mut out = Vec::new();
                let mut send = |w: &mut UnixStream, length: usize| {
                    sh.take(length, &mut out);
                    protocol::write_memblock(w, channel, &out, 0)
                };
                if send(&mut w, info.requested_bytes as usize).is_err() {
                    return;
                }
                while !sh.stop.load(Ordering::Relaxed) {
                    match protocol::read_command_message(&mut reader, version) {
                        Ok((_, protocol::Command::Request(protocol::Request { channel: c, length }))) if c == channel => {
                            if send(&mut w, length as usize).is_err() {
                                break;
                            }
                        }
                        Ok((_, protocol::Command::Error(e))) => {
                            tracing::warn!("live sound: {e:?}");
                            break;
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(LiveSink { shared, sock: Some(sock) })
    }

    /// Queue interleaved samples to play.
    pub fn push(&self, samples: &[i16]) {
        let mut q = self.shared.queue.lock().unwrap();
        q.extend(samples);
        let max = (self.shared.rate as u64 * self.shared.channels as u64 * LIVE_MAX_MS / 1000) as usize;
        if q.len() > max {
            let keep = (self.shared.rate as u64 * self.shared.channels as u64 * LIVE_TARGET_MS / 1000) as usize;
            let drop = (q.len() - keep) / self.shared.channels as usize * self.shared.channels as usize;
            q.drain(..drop);
        }
    }

    /// From 0 (silent) to 1 (as sent).
    pub fn set_volume(&self, volume: f64) {
        self.shared.volume.store((volume.clamp(0.0, 1.0) * 1000.0).round() as u32, Ordering::Relaxed);
    }
}

impl LiveShared {
    /// `length` bytes of what's queued, padded with silence.
    fn take(&self, length: usize, out: &mut Vec<u8>) {
        out.clear();
        let frame = self.channels as usize * 2;
        let want = length / frame * frame / 2;
        let volume = self.volume.load(Ordering::Relaxed) as i32;
        let mut q = self.queue.lock().unwrap();
        let n = want.min(q.len());
        for s in q.drain(..n) {
            let s = if volume < 1000 { (s as i32 * volume / 1000) as i16 } else { s };
            out.extend_from_slice(&s.to_le_bytes());
        }
        out.resize(want * 2, 0);
    }
}

impl Drop for LiveSink {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(s) = self.sock.take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// A microphone being listened to: 16-bit samples go to a callback as
/// the server sends them, about every 10 ms, until it's dropped.
pub struct Recorder {
    sock: UnixStream,
}

impl Recorder {
    /// Record from `source` (the server's name for it; the default one
    /// when None) at `rate` with `channels` interleaved channels.
    pub fn start(source: Option<&str>, rate: u32, channels: u8, mut heard: impl FnMut(&[i16]) + Send + 'static) -> Result<Recorder, String> {
        let channels = channels.clamp(1, 2);
        let (mut sock, version) = connect()?;
        let err = |e: protocol::ProtocolError| e.to_string();
        let source_name = match source {
            Some(s) => CString::new(s).map_err(|e| e.to_string())?,
            None => protocol::DEFAULT_SOURCE.to_owned(),
        };
        let params = protocol::RecordStreamParams {
            sample_spec: protocol::SampleSpec { format: protocol::SampleFormat::S16Le, channels, sample_rate: rate },
            channel_map: if channels == 1 { protocol::ChannelMap::mono() } else { protocol::ChannelMap::stereo() },
            source_name: Some(source_name),
            buffer_attr: protocol::stream::BufferAttr {
                max_length: u32::MAX,
                target_length: u32::MAX,
                pre_buffering: u32::MAX,
                minimum_request_length: u32::MAX,
                fragment_size: rate * channels as u32 * 2 / 100,
            },
            flags: protocol::stream::StreamFlags { adjust_latency: true, ..Default::default() },
            ..Default::default()
        };
        protocol::write_command_message(&mut sock, SEQ_CREATE, &protocol::Command::CreateRecordStream(params), version).map_err(err)?;
        let mut reader = BufReader::new(sock.try_clone().map_err(|e| e.to_string())?);
        let (_, info) = protocol::read_reply_message::<protocol::CreateRecordStreamReply>(&mut reader, version).map_err(err)?;
        let channel = info.channel;
        std::thread::Builder::new()
            .name("recorder".into())
            .spawn(move || {
                use std::io::Read;
                let mut payload = Vec::new();
                let mut samples = Vec::new();
                // A byte left over when a block ends mid-sample.
                let mut odd: Option<u8> = None;
                while let Ok(desc) = protocol::read_descriptor(&mut reader) {
                    payload.resize(desc.length as usize, 0);
                    if reader.read_exact(&mut payload).is_err() {
                        break;
                    }
                    if desc.channel == channel {
                        samples.clear();
                        let mut bytes = &payload[..];
                        if let Some(b) = odd.take()
                            && let Some((&first, rest)) = bytes.split_first()
                        {
                            samples.push(i16::from_le_bytes([b, first]));
                            bytes = rest;
                        }
                        let (pairs, rest) = bytes.as_chunks::<2>();
                        samples.extend(pairs.iter().map(|p| i16::from_le_bytes(*p)));
                        odd = rest.first().copied();
                        heard(&samples);
                    } else if desc.channel == u32::MAX
                        && let Ok((_, protocol::Command::Error(e))) =
                            protocol::Command::read_tag_prefixed(&mut std::io::Cursor::new(&payload[..]), version)
                    {
                        tracing::warn!("recorder: {e:?}");
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Recorder { sock })
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.sock.shutdown(std::net::Shutdown::Both);
    }
}
