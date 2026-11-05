//! A sound server for tests: speaks enough of the PulseAudio protocol for
//! Olo's players (olo-preview's `audio`), plays nothing, and reports
//! what it hears on stderr, a line a second per stream: bytes consumed,
//! whether it's corked, and how loud the samples were (RMS of S16LE, 0 to
//! 32767). Samples are consumed in real time, so positions and latency
//! behave as with a real server.
//!
//! Record streams hear a steady 440 Hz tone (RMS about 5700), and the
//! server has one speaker and one microphone, both called "Fake".
//! `FAKESOUND_AIRPODS=AA:BB:CC:DD:EE:FF` adds a pair of AirPods as
//! PipeWire shows them (a Bluetooth output playing AAC at music quality,
//! the default, and their card with its profiles), for Preferences ▸
//! Sound and the AirPods service. Subscriptions are taken (no events
//! come: nothing changes).
//!
//! ```text
//! olo-fakesound /tmp/fake.sock &
//! PULSE_SERVER=unix:/tmp/fake.sock olo-viewer song.mp3
//! ```

use olo_sync::Mutex;
use std::{
    collections::HashMap,
    io::{Cursor, Read},
    os::unix::net::{UnixListener, UnixStream},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use pulseaudio::protocol::{self, Command};

const TICK: Duration = Duration::from_millis(10);

struct Stream {
    spec: protocol::SampleSpec,
    attr: protocol::stream::BufferAttr,
    corked: bool,
    buffer: Vec<u8>,
    /// Bytes asked for and not yet sent.
    requested: usize,
    written: i64,
    read: i64,
    drain: Option<u32>,
    /// This second's bytes and sum of squares.
    heard: (usize, f64, u64),
    reported: Instant,
    /// A record stream, and where its tone has got to (in samples).
    record: Option<u64>,
}

struct Client {
    writer: UnixStream,
    version: u16,
    streams: HashMap<u32, Stream>,
}

fn main() {
    olo_crash::install();
    let path = std::env::args().nth(1).expect("usage: olo-fakesound SOCKET");
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).expect("bind");
    eprintln!("fakesound: listening on {path}");
    let next_channel = Arc::new(Mutex::new(0u32));
    for (n, conn) in listener.incoming().enumerate() {
        let Ok(sock) = conn else { continue };
        let next_channel = next_channel.clone();
        std::thread::spawn(move || {
            if let Err(e) = serve(n as u32, sock, &next_channel) {
                eprintln!("fakesound: client {n}: {e}");
            }
            eprintln!("fakesound: client {n} gone");
        });
    }
}

fn serve(id: u32, sock: UnixStream, next_channel: &Mutex<u32>) -> Result<(), String> {
    let client = Arc::new(Mutex::new(Client { writer: sock.try_clone().map_err(|e| e.to_string())?, version: 13, streams: HashMap::new() }));
    {
        let client = Arc::downgrade(&client);
        std::thread::spawn(move || {
            while let Some(c) = client.upgrade() {
                tick(&mut c.lock().unwrap());
                drop(c);
                std::thread::sleep(TICK);
            }
        });
    }
    let mut reader = sock;
    let mut payload = Vec::new();
    loop {
        let desc = protocol::read_descriptor(&mut reader).map_err(|e| e.to_string())?;
        payload.resize(desc.length as usize, 0);
        reader.read_exact(&mut payload).map_err(|e| e.to_string())?;
        let mut c = client.lock().unwrap();
        if desc.channel != u32::MAX {
            if let Some(s) = c.streams.get_mut(&desc.channel) {
                s.buffer.extend_from_slice(&payload);
                s.written += payload.len() as i64;
                s.requested = s.requested.saturating_sub(payload.len());
            }
            continue;
        }
        let version = c.version;
        let (seq, cmd) = Command::read_tag_prefixed(&mut Cursor::new(&payload[..]), version).map_err(|e| e.to_string())?;
        command(id, &mut c, seq, cmd, next_channel).map_err(|e| e.to_string())?;
    }
}

fn command(id: u32, c: &mut Client, seq: u32, cmd: Command, next_channel: &Mutex<u32>) -> Result<(), protocol::ProtocolError> {
    let v = c.version;
    match cmd {
        Command::Auth(p) => {
            c.version = p.version.min(protocol::MAX_VERSION);
            protocol::write_reply_message(&mut c.writer, seq, &protocol::AuthReply { version: c.version, ..Default::default() }, c.version)
        }
        Command::SetClientName(_) => protocol::write_reply_message(&mut c.writer, seq, &protocol::SetClientNameReply { client_id: id }, v),
        Command::CreatePlaybackStream(mut p) => {
            let a = &mut p.buffer_attr;
            if a.max_length == u32::MAX {
                a.max_length = 4 << 20;
            }
            if a.target_length == u32::MAX {
                a.target_length = 128 << 10;
            }
            if a.pre_buffering == u32::MAX {
                a.pre_buffering = a.target_length;
            }
            if a.minimum_request_length == u32::MAX {
                a.minimum_request_length = a.target_length / 8;
            }
            let channel = {
                let mut n = next_channel.lock().unwrap();
                *n += 1;
                *n
            };
            let corked = p.flags.start_corked;
            eprintln!("fakesound: stream {channel}: {:?} {} Hz × {}, corked {corked}", p.sample_spec.format, p.sample_spec.sample_rate, p.sample_spec.channels);
            c.streams.insert(
                channel,
                Stream {
                    spec: p.sample_spec,
                    attr: p.buffer_attr,
                    corked,
                    buffer: Vec::new(),
                    requested: p.buffer_attr.target_length as usize,
                    written: 0,
                    read: 0,
                    drain: None,
                    heard: (0, 0.0, 0),
                    reported: Instant::now(),
                    record: None,
                },
            );
            let reply = protocol::CreatePlaybackStreamReply {
                channel,
                stream_index: channel,
                sample_spec: p.sample_spec,
                channel_map: p.channel_map,
                buffer_attr: p.buffer_attr,
                requested_bytes: p.buffer_attr.target_length,
                sink_name: Some(c"fake".into()),
                ..Default::default()
            };
            protocol::write_reply_message(&mut c.writer, seq, &reply, v)
        }
        Command::CorkPlaybackStream(p) => {
            if let Some(s) = c.streams.get_mut(&p.channel) {
                s.corked = p.cork;
                eprintln!("fakesound: stream {}: corked {}", p.channel, p.cork);
            }
            protocol::write_ack_message(&mut c.writer, seq)
        }
        Command::FlushPlaybackStream(ch) => {
            if let Some(s) = c.streams.get_mut(&ch) {
                s.buffer.clear();
                s.read = s.written;
                s.drain = None;
                eprintln!("fakesound: stream {ch}: flushed");
            }
            protocol::write_ack_message(&mut c.writer, seq)
        }
        Command::DrainPlaybackStream(ch) => {
            if let Some(s) = c.streams.get_mut(&ch) {
                s.drain = Some(seq);
            }
            Ok(())
        }
        Command::GetPlaybackLatency(p) => {
            let (playing, write_offset, read_offset) = c.streams.get(&p.channel).map_or((false, 0, 0), |s| (!s.corked, s.written, s.read));
            let now = SystemTime::now();
            let reply = protocol::PlaybackLatency { sink_usec: 0, source_usec: 0, playing, local_time: p.now, remote_time: now, write_offset, read_offset, underrun_for: 0, playing_for: 0 };
            protocol::write_reply_message(&mut c.writer, seq, &reply, v)
        }
        Command::CreateRecordStream(p) => {
            let channel = {
                let mut n = next_channel.lock().unwrap();
                *n += 1;
                *n
            };
            eprintln!("fakesound: record {channel}: {:?} {} Hz × {}", p.sample_spec.format, p.sample_spec.sample_rate, p.sample_spec.channels);
            c.streams.insert(
                channel,
                Stream {
                    spec: p.sample_spec,
                    attr: p.buffer_attr,
                    corked: p.flags.start_corked,
                    buffer: Vec::new(),
                    requested: 0,
                    written: 0,
                    read: 0,
                    drain: None,
                    heard: (0, 0.0, 0),
                    reported: Instant::now(),
                    record: Some(0),
                },
            );
            let reply = protocol::CreateRecordStreamReply {
                channel,
                stream_index: channel,
                sample_spec: p.sample_spec,
                channel_map: p.channel_map,
                buffer_attr: p.buffer_attr,
                sink_name: Some(c"fake-mic".into()),
                ..Default::default()
            };
            protocol::write_reply_message(&mut c.writer, seq, &reply, v)
        }
        Command::CorkRecordStream(p) => {
            if let Some(s) = c.streams.get_mut(&p.channel) {
                s.corked = p.cork;
            }
            protocol::write_ack_message(&mut c.writer, seq)
        }
        Command::Subscribe(_) => protocol::write_ack_message(&mut c.writer, seq),
        Command::GetCardInfoList => {
            let cards: Vec<protocol::CardInfo> = airpods()
                .map(|(addr, name, _)| {
                    let mut props = protocol::Props::new();
                    props.set_bytes(c"api.bluez5.address", format!("{addr}\0"));
                    let profile = |n: &str, d: &str| protocol::CardProfileInfo {
                        name: std::ffi::CString::new(n).unwrap(),
                        description: Some(std::ffi::CString::new(d).unwrap()),
                        priority: 0,
                        available: 1,
                        num_sinks: 1,
                        num_sources: u32::from(n.starts_with("headset")),
                    };
                    protocol::CardInfo {
                        index: 1,
                        name: std::ffi::CString::new(name.replace("output", "card").trim_end_matches(".1")).unwrap(),
                        props,
                        profiles: vec![profile("a2dp-sink", "High Fidelity Playback (A2DP Sink, codec AAC)"), profile("headset-head-unit", "Headset Head Unit (HSP/HFP, codec mSBC)")],
                        active_profile: Some(c"a2dp-sink".into()),
                        owner_module_index: None,
                        driver: None,
                        ports: Vec::new(),
                    }
                })
                .into_iter()
                .collect();
            protocol::write_reply_message(&mut c.writer, seq, &cards, v)
        }
        Command::GetServerInfo => {
            let default = airpods().map(|(_, n, _)| std::ffi::CString::new(n).unwrap()).unwrap_or(c"fake".into());
            let reply = protocol::ServerInfo { server_name: Some(c"fakesound".into()), default_sink_name: Some(default), default_source_name: Some(c"fake-mic".into()), ..Default::default() };
            protocol::write_reply_message(&mut c.writer, seq, &reply, v)
        }
        Command::GetSinkInfoList => {
            let sink = protocol::SinkInfo { index: 0, name: c"fake".into(), description: Some(c"Fake Speakers".into()), cvolume: protocol::ChannelVolume::norm(2), ..Default::default() };
            let mut sinks = vec![sink];
            if let Some((addr, name, desc)) = airpods() {
                let mut props = protocol::Props::new();
                props.set_bytes(c"api.bluez5.address", format!("{addr}\0"));
                props.set_bytes(c"api.bluez5.codec", "aac\0");
                props.set_bytes(c"api.bluez5.profile", "a2dp-sink\0");
                sinks.push(protocol::SinkInfo {
                    index: 1,
                    name: std::ffi::CString::new(name).unwrap(),
                    description: Some(std::ffi::CString::new(desc).unwrap()),
                    props,
                    cvolume: protocol::ChannelVolume::norm(2),
                    ..Default::default()
                });
            }
            protocol::write_reply_message(&mut c.writer, seq, &sinks, v)
        }
        Command::GetSourceInfoList => {
            let mic = protocol::SourceInfo { index: 0, name: c"fake-mic".into(), description: Some(c"Fake Microphone".into()), cvolume: protocol::ChannelVolume::norm(1), ..Default::default() };
            protocol::write_reply_message(&mut c.writer, seq, &vec![mic], v)
        }
        Command::DeleteRecordStream(ch) | Command::DeletePlaybackStream(ch) => {
            c.streams.remove(&ch);
            protocol::write_ack_message(&mut c.writer, seq)
        }
        other => {
            eprintln!("fakesound: not implemented: {:?}", other.tag());
            protocol::write_error(&mut c.writer, seq, &protocol::PulseError::NotImplemented)
        }
    }
}

/// `FAKESOUND_AIRPODS`'s pair: address, the output's name and description.
fn airpods() -> Option<(String, String, String)> {
    let addr = std::env::var("FAKESOUND_AIRPODS").ok().filter(|a| !a.is_empty())?;
    let name = format!("bluez_output.{}.1", addr.replace(':', "_"));
    Some((addr, name, "AirPods Pro".to_owned()))
}

/// Consume a tick's worth of each playing stream, ask for more, report.
fn tick(c: &mut Client) {
    let v = c.version;
    let mut out = Vec::new();
    for (&ch, s) in c.streams.iter_mut() {
        let frame = s.spec.format.bytes_per_sample() * s.spec.channels as usize;
        let per_tick = (s.spec.sample_rate as usize * frame) as u128 * TICK.as_millis() / 1000;
        if let Some(t) = &mut s.record {
            if !s.corked && s.spec.format == protocol::SampleFormat::S16Le {
                let frames = per_tick as usize / frame.max(1);
                let mut block = Vec::with_capacity(frames * frame);
                for _ in 0..frames {
                    let x = (*t as f64 * 440.0 * std::f64::consts::TAU / s.spec.sample_rate as f64).sin() * 8000.0;
                    for _ in 0..s.spec.channels {
                        block.extend_from_slice(&(x as i16).to_le_bytes());
                    }
                    *t += 1;
                }
                out.push(Out::Block(ch, block));
            }
            continue;
        }
        if !s.corked {
            let n = (per_tick as usize).min(s.buffer.len()) / frame.max(1) * frame;
            if s.spec.format == protocol::SampleFormat::S16Le {
                for p in s.buffer[..n].as_chunks::<2>().0 {
                    let x = i16::from_le_bytes(*p) as f64;
                    s.heard.1 += x * x;
                    s.heard.2 += 1;
                }
            }
            s.buffer.drain(..n);
            s.read += n as i64;
            s.heard.0 += n;
            if s.buffer.is_empty()
                && let Some(seq) = s.drain.take()
            {
                eprintln!("fakesound: stream {ch}: drained");
                out.push(Out::Ack(seq));
            }
            let want = (s.attr.target_length as usize).saturating_sub(s.buffer.len() + s.requested);
            if s.drain.is_none() && want >= (s.attr.minimum_request_length as usize).max(1) {
                s.requested += want;
                out.push(Out::Request(ch, want as u32));
            }
        }
        if s.reported.elapsed() >= Duration::from_secs(1) {
            let rms = if s.heard.2 > 0 { (s.heard.1 / s.heard.2 as f64).sqrt() } else { 0.0 };
            eprintln!("fakesound: stream {ch}: {} bytes, rms {rms:.0}{}", s.heard.0, if s.corked { ", corked" } else { "" });
            s.heard = (0, 0.0, 0);
            s.reported = Instant::now();
        }
    }
    for o in out {
        let _ = match o {
            Out::Ack(seq) => protocol::write_ack_message(&mut c.writer, seq),
            Out::Block(channel, block) => protocol::write_memblock(&mut c.writer, channel, &block, 0),
            Out::Request(channel, length) => protocol::write_command_message(&mut c.writer, u32::MAX, &Command::Request(protocol::Request { channel, length }), v),
        };
    }
}

enum Out {
    Ack(u32),
    Request(u32, u32),
    Block(u32, Vec<u8>),
}
