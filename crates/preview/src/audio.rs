//! Playing audio in a preview: symphonia decodes, and samples go to the
//! sound server over the PulseAudio native protocol (spoken by PipeWire
//! too), implemented in Rust, so nothing outside the process is involved.

use std::{
    ffi::CString,
    io::BufReader,
    os::unix::net::UnixStream,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
};

use pulseaudio::protocol;
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{CODEC_TYPE_NULL, Decoder, DecoderOptions},
    formats::{FormatOptions, FormatReader, SeekMode, SeekTo},
    io::MediaSourceStream,
    meta::MetadataOptions,
    probe::Hint,
    units::Time,
};

#[derive(Default)]
pub struct State {
    /// Playback position in milliseconds.
    pub position_ms: AtomicU64,
    pub duration_ms: AtomicU64,
    pub playing: AtomicBool,
    pub finished: AtomicBool,
    pub error: Mutex<Option<String>>,
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
    /// Start playing `path` at once.
    pub fn start(path: &Path) -> Player {
        let (tx, rx) = mpsc::channel();
        let state = Arc::new(State::default());
        let (path, st, tx2) = (path.to_owned(), state.clone(), tx.clone());
        std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || {
                if let Err(e) = run(&path, &st, tx2, rx) {
                    tracing::warn!("audio {}: {e}", path.display());
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

    pub fn seek(&self, secs: f64) {
        self.state.finished.store(false, Ordering::Relaxed);
        let _ = self.tx.send(Ev::Seek(secs));
    }
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
    fn open(path: &Path) -> Result<(Source, u32, f64), String> {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension() {
            hint.with_extension(&ext.to_string_lossy());
        }
        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &FormatOptions { enable_gapless: true, ..Default::default() }, &MetadataOptions::default())
            .map_err(|_| "This audio format can't be played.".to_string())?;
        let format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or("There is no sound in this file.")?;
        let params = track.codec_params.clone();
        let decoder = symphonia::default::get_codecs()
            .make(&params, &DecoderOptions::default())
            .map_err(|_| "This audio codec can't be played.".to_string())?;
        let rate = params.sample_rate.ok_or("Unknown sample rate.")?;
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
            // Down-mix anything wider than stereo by keeping the front pair.
            for frame in sb.samples().chunks(src_channels.max(1)) {
                for &s in frame.iter().take(self.channels) {
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
    props.set(protocol::Prop::ApplicationName, CString::new("Tracker").unwrap());
    props.set(protocol::Prop::ApplicationIconName, CString::new("tabula-tracker").unwrap());
    protocol::write_command_message(&mut w, 1, &protocol::Command::SetClientName(props), version).map_err(err)?;
    protocol::read_reply_message::<protocol::SetClientNameReply>(&mut r, version).map_err(err)?;
    Ok((w, version))
}

fn run(path: &Path, st: &State, tx: mpsc::Sender<Ev>, rx: mpsc::Receiver<Ev>) -> Result<(), String> {
    let (mut src, rate, duration) = Source::open(path)?;
    st.duration_ms.store((duration * 1000.0) as u64, Ordering::Relaxed);
    let (mut sock, version) = connect()?;
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
    st.playing.store(true, Ordering::Relaxed);

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
