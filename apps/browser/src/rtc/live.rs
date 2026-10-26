//! Live streams to media elements. Pages run in processes of their own,
//! but cameras and calls live in NetPositive's; a media element playing
//! a stream (`srcObject`) gets a `data:` URL naming the stream's secret,
//! and our player in the page's process (media.rs) connects to this
//! socket with it and reads the stream's pictures and sound as they
//! come.
//!
//! Messages are a tag byte, a little-endian u32 length and the payload:
//! 1 a picture (u32 width, u32 height, I420), 2 sound (u32 rate, u8
//! channels, 16-bit samples), 3 which kinds the stream has now (u8
//! audio, u8 video).

use std::{
    io::{self, BufRead, BufReader, BufWriter, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    sync::mpsc,
    time::Duration,
};

use super::{
    frames::{Frame, Picture, Sound},
    hub::{HUB, SubId, TrackKey},
};

/// Where the socket is, for pages' processes.
const ENV: &str = "TABULA_BROWSER_LIVE";

/// What a media element's URL holds (after the data: prefix).
pub const MAGIC: &str = "TABULA-LIVE:";

/// Name the socket in the environment, for the processes to come. Runs
/// at the start of `main`, before there are threads.
pub fn init() {
    if std::env::var_os(ENV).is_some() {
        return;
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let path = dir.join(format!("tabula-browser-{}.live", std::process::id()));
    // SAFETY: called first thing in main, before any thread exists.
    unsafe { std::env::set_var(ENV, path) };
}

/// Listen (NetPositive's process, once).
pub fn serve() {
    let Some(path) = std::env::var_os(ENV) else { return };
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => return tracing::warn!("live streams: {e}"),
    };
    std::thread::Builder::new()
        .name("live".into())
        .spawn(move || {
            for conn in listener.incoming().flatten() {
                let _ = std::thread::Builder::new().name("live-conn".into()).spawn(move || {
                    if let Err(e) = connection(conn) {
                        tracing::debug!("live stream: {e}");
                    }
                });
            }
        })
        .ok();
}

/// Remove the socket as NetPositive quits.
pub fn close() {
    if let Some(path) = std::env::var_os(ENV) {
        let _ = std::fs::remove_file(path);
    }
}

fn connection(conn: UnixStream) -> io::Result<()> {
    conn.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut secret = String::new();
    BufReader::new(conn.try_clone()?).take(256).read_line(&mut secret)?;
    let secret = secret.trim().to_string();
    let mut w = BufWriter::with_capacity(1 << 16, conn);
    let (tx, rx) = mpsc::sync_channel(64);
    let mut subs: Vec<(TrackKey, SubId)> = Vec::new();
    let mut version = None;
    let result = (|| loop {
        let Some((v, audio, video)) = HUB.stream_tracks(&secret) else { return Ok(()) };
        if version != Some(v) {
            version = Some(v);
            let want: Vec<TrackKey> = audio.iter().chain(video.iter()).cloned().collect();
            if want.iter().ne(subs.iter().map(|(k, _)| k)) {
                subs.drain(..).for_each(|(_, id)| HUB.unsubscribe(id));
                subs = want.into_iter().map(|k| (k.clone(), HUB.subscribe(k, tx.clone()))).collect();
            }
            message(&mut w, 3, &[audio.is_some() as u8, video.is_some() as u8])?;
            w.flush()?;
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok((_, Frame::Picture(p))) => {
                let mut head = Vec::with_capacity(8);
                head.extend_from_slice(&p.width.to_le_bytes());
                head.extend_from_slice(&p.height.to_le_bytes());
                w.write_all(&[1])?;
                w.write_all(&((head.len() + p.data.len()) as u32).to_le_bytes())?;
                w.write_all(&head)?;
                w.write_all(&p.data)?;
                w.flush()?;
            }
            Ok((_, Frame::Sound(s))) => {
                let mut payload = Vec::with_capacity(5 + s.samples.len() * 2);
                payload.extend_from_slice(&s.rate.to_le_bytes());
                payload.push(s.channels);
                payload.extend(s.samples.iter().flat_map(|x| x.to_le_bytes()));
                message(&mut w, 2, &payload)?;
                w.flush()?;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    })();
    subs.into_iter().for_each(|(_, id)| HUB.unsubscribe(id));
    result
}

fn message(w: &mut impl Write, tag: u8, payload: &[u8]) -> io::Result<()> {
    w.write_all(&[tag])?;
    w.write_all(&(payload.len() as u32).to_le_bytes())?;
    w.write_all(payload)
}

/// What a player reads.
pub enum Message {
    Picture(Picture),
    Sound(Sound),
    Kinds { audio: bool, video: bool },
}

/// A player's end (in a page's process).
pub struct Reader(BufReader<UnixStream>);

impl Reader {
    pub fn connect(secret: &str) -> io::Result<Reader> {
        let path = std::env::var_os(ENV).ok_or_else(|| io::Error::other("no live stream socket"))?;
        let mut s = UnixStream::connect(path)?;
        s.write_all(format!("{secret}\n").as_bytes())?;
        Ok(Reader(BufReader::with_capacity(1 << 16, s)))
    }

    /// A handle that ends the connection (and so a blocked `next`).
    pub fn closer(&self) -> io::Result<UnixStream> {
        self.0.get_ref().try_clone()
    }

    pub fn next(&mut self) -> io::Result<Message> {
        let mut head = [0u8; 5];
        self.0.read_exact(&mut head)?;
        let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
        if len > 64 << 20 {
            return Err(io::Error::other("message too big"));
        }
        let mut payload = vec![0u8; len];
        self.0.read_exact(&mut payload)?;
        let u32_at = |i: usize| payload.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        match head[0] {
            1 => {
                let (Some(width), Some(height)) = (u32_at(0), u32_at(4)) else { return Err(io::Error::other("short picture")) };
                payload.drain(..8);
                if payload.len() != width as usize * height as usize * 3 / 2 {
                    return Err(io::Error::other("bad picture"));
                }
                Ok(Message::Picture(Picture { width, height, data: payload }))
            }
            2 => {
                let (Some(rate), Some(&channels)) = (u32_at(0), payload.get(4)) else { return Err(io::Error::other("short sound")) };
                let samples = payload[5..].as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect();
                Ok(Message::Sound(Sound { rate, channels, samples }))
            }
            3 => Ok(Message::Kinds { audio: payload.first() == Some(&1), video: payload.get(1) == Some(&1) }),
            _ => Err(io::Error::other("unknown message")),
        }
    }
}

