//! Playing video in a preview. A worker demuxes H.264 (mp4 or Matroska)
//! and decodes it with OpenH264 a few frames ahead into a small queue; the
//! UI shows each frame when the clock reaches it. The clock runs on its
//! own and follows the sound (played by [`audio::Player`]) when there is
//! any, so a file without sound, or no sound server, still plays.

use std::{
    cmp::Reverse,
    collections::{BinaryHeap, VecDeque},
    io::BufReader,
    path::Path,
    sync::{Arc, Condvar, Mutex, atomic::Ordering},
    time::{Duration, Instant},
};

use tabula_kit::images::av1;

use crate::{audio, load::Image, media};

/// Frames decoded ahead (about a fifth of a second at 30 fps).
const AHEAD: usize = 6;

/// How far the clock may drift from the sound before it is pulled back.
const DRIFT: f64 = 0.12;

struct Frame {
    pts: f64,
    image: Image,
}

#[derive(Default)]
struct Shared {
    queue: VecDeque<Frame>,
    seek: Option<f64>,
    stop: bool,
    /// The stream ended and the queue holds its last frames.
    eof: bool,
    error: Option<String>,
}

struct Clock {
    /// Media time at `since`, or the stopped time when paused.
    at: f64,
    since: Option<Instant>,
    /// The sound's last reported position, to notice new reports.
    heard_ms: u64,
    finished: bool,
    /// Show the next frame whatever its time (after a seek, when paused).
    show_next: bool,
}

impl Clock {
    fn now(&self) -> f64 {
        self.at + self.since.map_or(0.0, |s| s.elapsed().as_secs_f64())
    }
}

pub struct Video {
    shared: Arc<(Mutex<Shared>, Condvar)>,
    clock: Mutex<Clock>,
    audio: Option<audio::Player>,
    duration: f64,
}

impl Video {
    /// Start playing `path` (of `duration` seconds) at once.
    pub fn start(path: &Path, duration: f64) -> Video {
        let shared = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
        let (p, s) = (path.to_owned(), shared.clone());
        std::thread::Builder::new()
            .name("video".into())
            .spawn(move || {
                if let Err(e) = decode(&p, &s) {
                    tracing::warn!("video {}: {e}", p.display());
                    let mut sh = s.0.lock().unwrap();
                    sh.error = Some(e);
                    sh.eof = true;
                }
            })
            .ok();
        Video {
            shared,
            clock: Mutex::new(Clock { at: 0.0, since: Some(Instant::now()), heard_ms: 0, finished: false, show_next: true }),
            audio: Some(audio::Player::start(path)),
            duration,
        }
    }

    pub fn position(&self) -> f64 {
        self.clock.lock().unwrap().now().clamp(0.0, self.duration)
    }

    pub fn duration(&self) -> f64 {
        self.duration
    }

    pub fn playing(&self) -> bool {
        self.clock.lock().unwrap().since.is_some()
    }

    pub fn error(&self) -> Option<String> {
        self.shared.0.lock().unwrap().error.clone()
    }

    /// The sound, while it plays: a missing server or a file without
    /// sound leaves the clock to itself.
    fn sound(&self) -> Option<&audio::Player> {
        self.audio.as_ref().filter(|a| a.state.error.lock().unwrap().is_none())
    }

    pub fn toggle(&self) {
        let (playing, finished) = {
            let c = self.clock.lock().unwrap();
            (c.since.is_some(), c.finished)
        };
        if finished {
            self.seek(0.0);
        }
        self.set_playing(!playing || finished);
    }

    fn set_playing(&self, on: bool) {
        let mut c = self.clock.lock().unwrap();
        c.at = c.now();
        c.since = on.then(Instant::now);
        c.finished = false;
        if let Some(a) = self.sound() {
            a.set_playing(on);
        }
    }

    pub fn seek(&self, secs: f64) {
        let secs = secs.clamp(0.0, self.duration);
        {
            let mut c = self.clock.lock().unwrap();
            c.at = secs;
            if c.since.is_some() {
                c.since = Some(Instant::now());
            }
            c.finished = false;
            c.show_next = true;
        }
        if let Some(a) = self.sound() {
            a.seek(secs);
        }
        let (m, cv) = &*self.shared;
        let mut sh = m.lock().unwrap();
        sh.seek = Some(secs);
        sh.queue.clear();
        cv.notify_all();
    }

    /// The frame due now, if it changed, and when the next one is due.
    pub fn tick(&self) -> (Option<Image>, Option<Duration>) {
        let mut c = self.clock.lock().unwrap();
        // Follow the sound: each new position it reports pulls the clock
        // back if they have drifted apart.
        if let Some(a) = self.sound()
            && c.since.is_some()
            && a.state.playing.load(Ordering::Relaxed)
        {
            let heard = a.state.position_ms.load(Ordering::Relaxed);
            if heard != c.heard_ms {
                c.heard_ms = heard;
                let t = heard as f64 / 1000.0;
                if (c.now() - t).abs() > DRIFT {
                    c.at = t;
                    c.since = Some(Instant::now());
                }
            }
        }
        let now = c.now();
        let (m, cv) = &*self.shared;
        let mut sh = m.lock().unwrap();
        let mut due = None;
        while let Some(f) = sh.queue.front() {
            if f.pts > now + 0.004 && !c.show_next {
                break;
            }
            due = sh.queue.pop_front();
            if c.show_next {
                c.show_next = false;
                break;
            }
        }
        if due.is_some() {
            cv.notify_all();
        }
        let next = sh.queue.front().map(|f| Duration::from_secs_f64((f.pts - now).max(0.004)));
        if sh.eof && sh.queue.is_empty() && sh.seek.is_none() && c.since.is_some() {
            // The pictures ran out: stop (the sound ends by itself).
            c.at = self.duration;
            c.since = None;
            c.finished = true;
        }
        // Playing, or waiting for the picture a seek asked for.
        let next = if c.since.is_some() || (c.show_next && !sh.eof) { Some(next.unwrap_or(Duration::from_millis(15))) } else { None };
        (due.map(|f| f.image), next)
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        let (m, cv) = &*self.shared;
        m.lock().unwrap().stop = true;
        cv.notify_all();
    }
}

fn av1_image(f: &av1::Frame) -> Image {
    Arc::new(tabula_kit::egui::ColorImage::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.to_rgba()))
}

/// One coded picture (H.264 in start-code form, AV1 as stored), with
/// its presentation time.
struct Packet {
    pts: f64,
    data: Vec<u8>,
}

trait Demux: Send {
    fn codec(&self) -> Codec;
    fn next(&mut self) -> Option<Packet>;
    /// Go to the key frame at or before `secs`.
    fn seek(&mut self, secs: f64);
}

fn open(path: &Path) -> Result<Box<dyn Demux>, String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if matches!(ext.as_str(), "mkv" | "webm") { Mkv::open(path).map(|d| Box::new(d) as _) } else { Mp4::open(path).map(|d| Box::new(d) as _) }
}

/// The codecs played.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Codec {
    H264,
    Av1,
}

enum Decoder {
    H264(Box<openh264::decoder::Decoder>),
    Av1(av1::Decoder),
}

impl Decoder {
    fn new(codec: Codec) -> Result<Decoder, String> {
        Ok(match codec {
            Codec::H264 => Decoder::H264(Box::new(openh264::decoder::Decoder::new().map_err(|e| e.to_string())?)),
            // Threads to keep up with large pictures; pictures may come
            // out a few units late.
            Codec::Av1 => Decoder::Av1(av1::Decoder::new(0, 0)?),
        })
    }
}

fn decode(path: &Path, shared: &(Mutex<Shared>, Condvar)) -> Result<(), String> {
    let (m, cv) = shared;
    let mut demux = open(path)?;
    let codec = demux.codec();
    let mut dec = Decoder::new(codec)?;
    // Presentation times of packets sent and not yet out: decoders hand
    // pictures back in display order, so each takes the earliest.
    let mut times: BinaryHeap<Reverse<i64>> = BinaryHeap::new();
    let mut from = 0.0;
    let mut av1_out = Vec::new();
    // Queue a picture (made only if it will be shown); false once a seek
    // or stop makes the rest pointless.
    let push = |times: &mut BinaryHeap<Reverse<i64>>, from: f64, image: &dyn Fn() -> Image| -> bool {
        let Some(Reverse(t)) = times.pop() else { return true };
        let pts = t as f64 / 1e6;
        // Pictures before a seek's target only lead up to it.
        if pts + 0.001 < from {
            return true;
        }
        let image = image();
        let mut sh = m.lock().unwrap();
        if sh.seek.is_some() || sh.stop {
            return false;
        }
        sh.queue.push_back(Frame { pts, image });
        true
    };
    loop {
        {
            let mut sh = m.lock().unwrap();
            while !sh.stop && sh.seek.is_none() && (sh.queue.len() >= AHEAD || sh.eof) {
                sh = cv.wait(sh).unwrap();
            }
            if sh.stop {
                return Ok(());
            }
            if let Some(t) = sh.seek.take() {
                sh.queue.clear();
                sh.eof = false;
                drop(sh);
                demux.seek(t);
                dec = Decoder::new(codec)?;
                times.clear();
                from = t;
            }
        }
        let packet = demux.next();
        let ended = packet.is_none();
        if let Some(p) = &packet {
            times.push(Reverse((p.pts * 1e6) as i64));
        }
        match (&mut dec, packet) {
            (Decoder::H264(d), Some(p)) => {
                if let Ok(Some(yuv)) = d.decode(&p.data) {
                    push(&mut times, from, &|| media::yuv_image(&yuv));
                }
            }
            (Decoder::H264(d), None) => {
                if let Ok(rest) = d.flush_remaining() {
                    for yuv in &rest {
                        if !push(&mut times, from, &|| media::yuv_image(yuv)) {
                            break;
                        }
                    }
                }
            }
            (Decoder::Av1(d), p) => {
                av1_out.clear();
                match &p {
                    Some(p) => {
                        if let Err(e) = d.decode(&p.data, &mut av1_out) {
                            tracing::debug!("av1: {e}");
                        }
                    }
                    None => d.flush(&mut av1_out),
                }
                for f in &av1_out {
                    if !push(&mut times, from, &|| av1_image(f)) {
                        break;
                    }
                }
            }
        }
        if ended {
            times.clear();
            m.lock().unwrap().eof = true;
        }
    }
}

struct Mp4 {
    reader: mp4::Mp4Reader<BufReader<std::fs::File>>,
    track: u32,
    timescale: f64,
    /// Parameter sets, in start-code form, sent before each key frame.
    sets: Vec<u8>,
    nal_len: usize,
    /// Each sample's decode time, and the key frames (1-based).
    times: Vec<u64>,
    syncs: Option<Vec<u32>>,
    next: u32,
}

impl Mp4 {
    fn open(path: &Path) -> Result<Mp4, String> {
        let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let size = f.metadata().map_err(|e| e.to_string())?.len();
        let reader = mp4::Mp4Reader::read_header(BufReader::new(f), size).map_err(|e| e.to_string())?;
        let (track, t) = reader
            .tracks()
            .iter()
            .find(|(_, t)| matches!(t.media_type(), Ok(mp4::MediaType::H264)))
            .ok_or("This video's format can't be played.")?;
        let stbl = &t.trak.mdia.minf.stbl;
        let avcc = &stbl.stsd.avc1.as_ref().ok_or("no avcC")?.avcc;
        let mut sets = Vec::new();
        for ps in avcc.sequence_parameter_sets.iter().chain(&avcc.picture_parameter_sets) {
            sets.extend_from_slice(&[0, 0, 0, 1]);
            sets.extend_from_slice(&ps.bytes);
        }
        let mut times = Vec::with_capacity(t.sample_count() as usize);
        let mut at = 0u64;
        for e in &stbl.stts.entries {
            for _ in 0..e.sample_count {
                times.push(at);
                at += e.sample_delta as u64;
            }
        }
        Ok(Mp4 {
            track: *track,
            timescale: t.timescale().max(1) as f64,
            sets,
            nal_len: avcc.length_size_minus_one as usize % 4 + 1,
            times,
            syncs: stbl.stss.as_ref().map(|s| s.entries.clone()),
            next: 1,
            reader,
        })
    }
}

impl Demux for Mp4 {
    fn codec(&self) -> Codec {
        Codec::H264
    }

    fn next(&mut self) -> Option<Packet> {
        loop {
            let id = self.next;
            if id as usize > self.times.len() {
                return None;
            }
            self.next += 1;
            let Ok(Some(s)) = self.reader.read_sample(self.track, id) else { continue };
            let mut data = Vec::with_capacity(s.bytes.len() + 64);
            if s.is_sync {
                data.extend_from_slice(&self.sets);
            }
            media::avcc_to_annexb(&s.bytes, self.nal_len, &mut data);
            let pts = (s.start_time as i64 + s.rendering_offset as i64) as f64 / self.timescale;
            return Some(Packet { pts, data });
        }
    }

    fn seek(&mut self, secs: f64) {
        let target = (secs * self.timescale) as u64;
        // The last sample starting at or before the target, then the key
        // frame at or before that.
        let n = self.times.partition_point(|&t| t <= target).max(1) as u32;
        self.next = match &self.syncs {
            Some(s) => s.iter().copied().filter(|&k| k <= n).max().unwrap_or(1),
            None => n,
        };
    }
}

struct Mkv {
    file: matroska_demuxer::MatroskaFile<BufReader<std::fs::File>>,
    track: u64,
    codec: Codec,
    scale: f64,
    sets: Vec<u8>,
    nal_len: usize,
    frame: matroska_demuxer::Frame,
    /// Send the parameter sets before the next picture (start, seeks).
    fresh: bool,
}

impl Mkv {
    fn open(path: &Path) -> Result<Mkv, String> {
        let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let file = matroska_demuxer::MatroskaFile::open(BufReader::new(f)).map_err(|e| e.to_string())?;
        let (t, codec) = file
            .tracks()
            .iter()
            .filter(|t| t.video().is_some())
            .find_map(|t| match t.codec_id() {
                "V_MPEG4/ISO/AVC" => Some((t, Codec::H264)),
                "V_AV1" => Some((t, Codec::Av1)),
                _ => None,
            })
            .ok_or("This video's format can't be played.")?;
        // AV1 blocks are OBUs as the decoder takes them, with sequence
        // headers in key frames.
        let (mut nal_len, mut sets) = (0, Vec::new());
        if codec == Codec::H264 {
            let (len, ps) = t.codec_private().and_then(media::parse_avcc).ok_or("no avcC")?;
            nal_len = len;
            for p in ps {
                sets.extend_from_slice(&[0, 0, 0, 1]);
                sets.extend_from_slice(&p);
            }
        }
        Ok(Mkv {
            track: t.track_number().get(),
            codec,
            scale: file.info().timestamp_scale().get() as f64 / 1e9,
            sets,
            nal_len,
            frame: matroska_demuxer::Frame::default(),
            fresh: true,
            file,
        })
    }
}

impl Demux for Mkv {
    fn codec(&self) -> Codec {
        self.codec
    }

    fn next(&mut self) -> Option<Packet> {
        while self.file.next_frame(&mut self.frame).ok()? {
            if self.frame.track != self.track {
                continue;
            }
            let pts = self.frame.timestamp as f64 * self.scale;
            if self.codec == Codec::Av1 {
                return Some(Packet { pts, data: std::mem::take(&mut self.frame.data) });
            }
            let mut data = Vec::with_capacity(self.frame.data.len() + 64);
            if self.fresh || self.frame.is_keyframe == Some(true) {
                data.extend_from_slice(&self.sets);
                self.fresh = false;
            }
            media::avcc_to_annexb(&self.frame.data, self.nal_len, &mut data);
            return Some(Packet { pts, data });
        }
        None
    }

    fn seek(&mut self, secs: f64) {
        // The demuxer lands on the exact time, usually between key frames:
        // look a little way back for the last key frame before it (further
        // each time none turns up), then go to that.
        let target = (secs.max(0.0) / self.scale) as u64;
        let mut back = (2.0 / self.scale) as u64;
        self.fresh = true;
        loop {
            let from = target.saturating_sub(back);
            if self.file.seek(from).is_err() {
                break;
            }
            let mut key = None;
            while self.file.next_frame(&mut self.frame).unwrap_or(false) {
                if self.frame.track != self.track {
                    continue;
                }
                if self.frame.timestamp > target {
                    break;
                }
                if self.frame.is_keyframe == Some(true) {
                    key = Some(self.frame.timestamp);
                }
            }
            if let Some(k) = key
                && self.file.seek(k).is_ok()
            {
                return;
            }
            if from == 0 {
                break;
            }
            back = back.saturating_mul(4);
        }
        let _ = self.file.seek(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `TABULA_SAMPLES=dir cargo test -p tabula-preview video -- --ignored`
    /// with 8-second 30 fps H.264 clips in `dir`: test.mp4 / test.mkv (one
    /// key frame), gop.mp4 / gop.mkv (one a second) and av1.webm (AV1 with
    /// reordering, a key frame a second).
    #[test]
    #[ignore]
    fn seeks_to_the_frame() {
        let dir = std::path::PathBuf::from(std::env::var("TABULA_SAMPLES").expect("TABULA_SAMPLES"));
        for name in ["test.mp4", "test.mkv", "gop.mp4", "gop.mkv", "av1.webm"] {
            for target in [0.0, 5.0, 2.5] {
                let path = dir.join(name);
                let shared = Arc::new((Mutex::new(Shared { seek: Some(target), ..Default::default() }), Condvar::new()));
                let s = shared.clone();
                let worker = std::thread::spawn(move || decode(&path, &s));
                let (m, cv) = &*shared;
                let mut sh = m.lock().unwrap();
                while sh.queue.len() < 3 && !sh.eof {
                    sh = cv.wait_timeout(sh, Duration::from_millis(50)).unwrap().0;
                }
                let pts: Vec<f64> = sh.queue.iter().map(|f| f.pts).collect();
                sh.stop = true;
                drop(sh);
                cv.notify_all();
                worker.join().unwrap().unwrap();
                assert!(pts.len() >= 3, "{name} @ {target}: {pts:?}");
                // The first picture is the one showing at the target.
                assert!((pts[0] - target).abs() < 0.05, "{name} @ {target}: {pts:?}");
                assert!(pts.windows(2).all(|w| w[1] > w[0]), "{name}: in order {pts:?}");
            }
        }
    }

    /// Pictures decoded (and made RGBA) per second from av1-1080.mkv (4 s of
    /// 1080p AV1): needs to stay well above 30 in release builds.
    #[test]
    #[ignore]
    fn keeps_up() {
        let dir = std::path::PathBuf::from(std::env::var("TABULA_SAMPLES").expect("TABULA_SAMPLES"));
        for name in ["av1-1080.mkv", "gop.mp4"] {
            let path = dir.join(name);
            let shared = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
            let s = shared.clone();
            let start = Instant::now();
            let worker = std::thread::spawn(move || decode(&path, &s));
            let (m, cv) = &*shared;
            let mut n = 0;
            loop {
                let mut sh = m.lock().unwrap();
                n += sh.queue.drain(..).count();
                cv.notify_all();
                if sh.eof && sh.queue.is_empty() {
                    sh.stop = true;
                    break;
                }
                drop(sh);
                std::thread::yield_now();
            }
            cv.notify_all();
            worker.join().unwrap().unwrap();
            let fps = n as f64 / start.elapsed().as_secs_f64();
            eprintln!("{name}: {n} pictures, {fps:.0} a second");
            assert!(n > 100 && fps > 45.0, "{name}: {n} at {fps:.0}/s");
        }
    }
}
