//! A sound server for tests: speaks enough of the PulseAudio protocol for
//! Tabula's players (tabula-preview's `audio`), plays nothing, and reports
//! what it hears on stderr, a line a second per stream: bytes consumed,
//! whether it's corked, and how loud the samples were (RMS of S16LE, 0 to
//! 32767). Samples are consumed in real time, so positions and latency
//! behave as with a real server.
//!
//! ```text
//! tabula-fakesound /tmp/fake.sock &
//! PULSE_SERVER=unix:/tmp/fake.sock tabula-viewer song.mp3
//! ```

use std::{
    collections::HashMap,
    io::{Cursor, Read},
    os::unix::net::{UnixListener, UnixStream},
    sync::{Arc, Mutex},
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
}

struct Client {
    writer: UnixStream,
    version: u16,
    streams: HashMap<u32, Stream>,
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: tabula-fakesound SOCKET");
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
            let reply = protocol::PlaybackLatency {
                sink_usec: 0,
                source_usec: 0,
                playing,
                local_time: p.now,
                remote_time: now,
                write_offset,
                read_offset,
                underrun_for: 0,
                playing_for: 0,
            };
            protocol::write_reply_message(&mut c.writer, seq, &reply, v)
        }
        Command::DeletePlaybackStream(ch) => {
            c.streams.remove(&ch);
            protocol::write_ack_message(&mut c.writer, seq)
        }
        other => {
            eprintln!("fakesound: not implemented: {:?}", other.tag());
            protocol::write_error(&mut c.writer, seq, &protocol::PulseError::NotImplemented)
        }
    }
}

/// Consume a tick's worth of each playing stream, ask for more, report.
fn tick(c: &mut Client) {
    let v = c.version;
    let mut out = Vec::new();
    for (&ch, s) in c.streams.iter_mut() {
        let frame = s.spec.format.bytes_per_sample() * s.spec.channels as usize;
        let per_tick = (s.spec.sample_rate as usize * frame) as u128 * TICK.as_millis() / 1000;
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
            Out::Request(channel, length) => {
                protocol::write_command_message(&mut c.writer, u32::MAX, &Command::Request(protocol::Request { channel, length }), v)
            }
        };
    }
}

enum Out {
    Ack(u32),
    Request(u32, u32),
}
