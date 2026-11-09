//! Media Source Extensions' byte streams (W3C MSE): what a page's
//! `SourceBuffer`s are given, as it gives it (fragmented MPEG-4, ISO BMFF
//! Byte Stream Format; WebM), parsed as it comes into the track it holds,
//! its coded frames and their buffered time ranges.
//!
//! One `Stream` is a `MediaSource`: its buffers (one a `SourceBuffer`),
//! appended to by the page's scripts (on the page thread), read by the
//! player's decoder (on a thread of its own), which waits for what isn't
//! there yet. A buffer the player doesn't play (a separate audio track,
//! while Olo plays videos silent) keeps only its ranges, not its frames.
//!
//! No dependencies: the scripts' side (`olo-js`) and the player's
//! (`olo-preview`) both use it.

use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

/// A track's codec, as the player decodes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
    Vp9,
    /// Sound, or a picture the player doesn't decode: ranges only.
    Other,
}

/// The track a buffer holds, from its initialization segment.
#[derive(Clone, Debug)]
pub struct Track {
    pub video: bool,
    pub codec: Codec,
    /// What a decoder is given before a key frame: H.264's SPS and PPS
    /// with start codes, AV1's sequence header OBUs (none for the rest).
    pub sets: Vec<u8>,
    /// H.264's NAL unit length's size (its samples are length-prefixed).
    pub nal_len: usize,
    /// HEVC's decoder configuration record (`hvcC`).
    pub hevc: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// A coded frame: its place in what was appended (`seq`, decode order),
/// when it shows and for how long (seconds), whether a decoder can start
/// at it, and its bytes as the container holds them.
#[derive(Clone, Debug)]
pub struct Sample {
    pub seq: u64,
    /// Which initialization segment it came after (`Stream::track`): a
    /// player switching quality appends another, another size's.
    pub init: u32,
    pub pts: f64,
    pub dur: f64,
    pub key: bool,
    pub data: Arc<[u8]>,
}

/// What the player's next read found.
pub enum Next {
    Sample(Sample),
    /// Nothing more yet (the page appends as it plays).
    Waiting,
    /// The stream ended (`endOfStream()`) or went (its element let it go).
    End,
}

/// Whether a `SourceBuffer` of this type can be made (`MediaSource.
/// isTypeSupported`): MPEG-4 or WebM, with codecs the player decodes, or
/// sound (taken, its ranges kept: Olo plays videos silent).
pub fn supported(mime: &str) -> bool {
    let (container, codecs) = parse_mime(mime);
    let mp4 = matches!(container.as_str(), "video/mp4" | "audio/mp4");
    let webm = matches!(container.as_str(), "video/webm" | "audio/webm");
    if !mp4 && !webm {
        return false;
    }
    codecs.iter().all(|c| {
        let c = c.to_ascii_lowercase();
        let family = c.split('.').next().unwrap_or("");
        match family {
            "avc1" | "avc3" | "hev1" | "hvc1" => mp4,
            "av01" => true,
            "vp09" | "vp9" => true,
            "mp4a" | "ac-3" | "ec-3" => mp4,
            "opus" => true,
            "vorbis" => webm,
            "flac" => true,
            _ => false,
        }
    })
}

/// A MIME type's essence (lowercase) and its `codecs` parameter's list.
fn parse_mime(mime: &str) -> (String, Vec<String>) {
    let mut parts = mime.split(';');
    let container = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let mut codecs = Vec::new();
    for p in parts {
        let Some((k, v)) = p.split_once('=') else { continue };
        if k.trim().eq_ignore_ascii_case("codecs") {
            codecs = v.trim().trim_matches('"').split(',').map(|c| c.trim().to_owned()).filter(|c| !c.is_empty()).collect();
        }
    }
    (container, codecs)
}

/// A `MediaSource`: its buffers, whether its stream has ended, whether its
/// element still has it.
pub struct Stream {
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Default)]
struct State {
    buffers: Vec<Option<Buffer>>,
    ended: bool,
    detached: bool,
}

/// Kept frames' bytes past which those the player has read go (Chrome
/// evicts too; a page removes what it played as it goes).
const KEPT_BYTES: usize = 192 << 20;
/// Ranges closer than this are one (a frame's rounding apart).
const GAP: f64 = 0.1;

impl Stream {
    pub fn new() -> Arc<Stream> {
        Arc::new(Stream { state: Mutex::new(State::default()), wake: Condvar::new() })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A new buffer for `mime` (`addSourceBuffer`): its index, or None if
    /// the type isn't one taken.
    pub fn add_buffer(&self, mime: &str) -> Option<usize> {
        if !supported(mime) {
            return None;
        }
        let (container, _) = parse_mime(mime);
        let mut s = self.lock();
        s.buffers.push(Some(Buffer::new(container.ends_with("webm"))));
        Some(s.buffers.len() - 1)
    }

    pub fn remove_buffer(&self, index: usize) {
        if let Some(b) = self.lock().buffers.get_mut(index) {
            *b = None;
        }
        self.wake.notify_all();
    }

    /// `appendBuffer`: the bytes parsed as far as they go (a box or element
    /// split between appends waits for the rest).
    pub fn append(&self, index: usize, bytes: &[u8]) -> Result<(), String> {
        let mut s = self.lock();
        s.ended = false;
        let b = s.buffers.get_mut(index).and_then(Option::as_mut).ok_or("no such buffer")?;
        b.pending.extend_from_slice(bytes);
        if let Some(dir) = std::env::var_os("OLO_MSE_DUMP") {
            static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{index}-{n:04}.bin")), bytes);
        }
        let (frames, seq) = (b.samples.len(), b.next_seq);
        let result = if b.webm { b.parse_webm() } else { b.parse_mp4() };
        // OLO_MSE_DEBUG: what each append held.
        if std::env::var_os("OLO_MSE_DEBUG").is_some() {
            let new = &b.samples[frames.min(b.samples.len())..];
            let (first, last) = (new.iter().map(|f| f.pts).fold(f64::INFINITY, f64::min), new.iter().map(|f| f.pts).fold(f64::NEG_INFINITY, f64::max));
            let before = b.samples[..frames.min(b.samples.len())].iter().map(|f| f.pts).fold(f64::NEG_INFINITY, f64::max);
            eprintln!(
                "mse append: buffer {index} ({}), {} bytes, {} frames ({} parsed, {} key) {first:.3}..{last:.3}{}; ranges {:?}",
                b.track.as_ref().map_or("no track", |t| if t.video { "video" } else { "sound" }),
                bytes.len(),
                new.len(),
                b.next_seq - seq,
                new.iter().filter(|f| f.key).count(),
                if first < before { format!(" BEFORE the last ({before:.3})") } else { String::new() },
                b.ranges
            );
        }
        b.evict();
        drop(s);
        self.wake.notify_all();
        result
    }

    /// `SourceBuffer.buffered`: its ranges, in seconds.
    pub fn buffered(&self, index: usize) -> Vec<[f64; 2]> {
        self.lock().buffers.get(index).and_then(Option::as_ref).map(|b| b.ranges.clone()).unwrap_or_default()
    }

    /// `SourceBuffer.remove(start, end)`: its frames that show then, and
    /// the time from its ranges.
    pub fn remove(&self, index: usize, start: f64, end: f64) {
        if std::env::var_os("OLO_MSE_DEBUG").is_some() {
            eprintln!("mse remove: buffer {index} {start:.3}..{end:.3}");
        }
        let mut s = self.lock();
        let Some(b) = s.buffers.get_mut(index).and_then(Option::as_mut) else { return };
        b.samples.retain(|f| f.pts < start || f.pts >= end);
        b.bytes = b.samples.iter().map(|f| f.data.len()).sum();
        let mut ranges = Vec::new();
        for r in b.ranges.drain(..) {
            if r[1] <= start || r[0] >= end {
                ranges.push(r);
                continue;
            }
            if r[0] < start {
                ranges.push([r[0], start]);
            }
            if r[1] > end {
                ranges.push([end, r[1]]);
            }
        }
        b.ranges = ranges;
    }

    /// `timestampOffset`: added to the times of what is appended next.
    pub fn set_offset(&self, index: usize, offset: f64) {
        if let Some(b) = self.lock().buffers.get_mut(index).and_then(Option::as_mut)
            && offset.is_finite()
        {
            b.offset = offset;
        }
    }

    /// `abort()`: a segment half appended is dropped (what comes next starts
    /// one).
    pub fn abort(&self, index: usize) {
        if std::env::var_os("OLO_MSE_DEBUG").is_some() {
            eprintln!("mse abort: buffer {index}");
        }
        if let Some(b) = self.lock().buffers.get_mut(index).and_then(Option::as_mut) {
            b.pending.clear();
            b.moof = None;
        }
    }

    /// `endOfStream()`: nothing more comes (the player plays to the end).
    pub fn end(&self) {
        self.lock().ended = true;
        self.wake.notify_all();
    }

    /// Its element let it go: a decoder waiting for it stops.
    pub fn detach(&self) {
        self.lock().detached = true;
        self.wake.notify_all();
    }

    pub fn detached(&self) -> bool {
        self.lock().detached
    }

    /// Buffer `index`'s track as its initialization segment `init` had it.
    pub fn track(&self, index: usize, init: u32) -> Option<Track> {
        self.lock().buffers.get(index).and_then(Option::as_ref).and_then(|b| b.inits.get(init as usize).cloned())
    }

    /// The buffer holding a picture track and its track, waiting up to
    /// `wait` for one's initialization segment.
    pub fn video_track(&self, wait: Duration) -> Option<(usize, Track)> {
        let mut s = self.lock();
        let deadline = std::time::Instant::now() + wait;
        loop {
            let found = s.buffers.iter_mut().enumerate().find_map(|(i, b)| {
                let b = b.as_mut()?;
                let t = b.track.as_ref()?;
                t.video.then(|| (i, t.clone()))
            });
            if let Some(found) = found {
                return Some(found);
            }
            if s.detached {
                return None;
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return None;
            }
            s = self.wake.wait_timeout(s, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// The frame after `after` (a `seq`) in buffer `index`, waiting up to
    /// `wait` for one to be appended.
    pub fn next_sample(&self, index: usize, after: Option<u64>, wait: Duration) -> Next {
        let mut s = self.lock();
        let deadline = std::time::Instant::now() + wait;
        loop {
            if s.detached {
                return Next::End;
            }
            let Some(b) = s.buffers.get_mut(index).and_then(Option::as_mut) else { return Next::End };
            let at = match after {
                Some(a) => b.samples.partition_point(|f| f.seq <= a),
                None => 0,
            };
            if let Some(f) = b.samples.get(at) {
                let f = f.clone();
                b.read = Some(f.seq);
                return Next::Sample(f);
            }
            if s.ended {
                return Next::End;
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Next::Waiting;
            }
            s = self.wake.wait_timeout(s, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// Where to read from to show `secs`: before the key frame at or
    /// before it (the `seq` to read after), or, with none buffered there,
    /// after everything appended so far (the page appends it next).
    pub fn seek(&self, index: usize, secs: f64) -> Option<u64> {
        let s = self.lock();
        let b = s.buffers.get(index).and_then(Option::as_ref)?;
        let key = b.samples.iter().filter(|f| f.key && f.pts <= secs + 1e-3).max_by(|a, c| a.pts.total_cmp(&c.pts));
        // (A key frame long before it, a stretch removed between: not that.)
        let covered = b.ranges.iter().any(|r| r[0] - GAP <= secs && secs < r[1] + GAP);
        match key {
            Some(k) if covered => k.seq.checked_sub(1),
            _ => b.samples.last().map(|f| f.seq),
        }
    }
}

/// One `SourceBuffer`: what is waiting to be parsed, its track, its frames
/// (kept for the player only) and its ranges.
struct Buffer {
    webm: bool,
    pending: Vec<u8>,
    track: Option<Track>,
    /// Each initialization segment's track, in order (a frame's `init`).
    inits: Vec<Track>,
    /// MPEG-4's track id or WebM's track number of the track it holds.
    track_id: u64,
    /// Seconds a tick: 1 / an MPEG-4 track's timescale, WebM's
    /// TimecodeScale (ns) / 1e9.
    tick: f64,
    /// MPEG-4's `trex` defaults, by track: duration, size, flags.
    trex: HashMap<u32, [u32; 3]>,
    /// A `moof` waiting for its `mdat`.
    moof: Option<Vec<u8>>,
    /// WebM: the cluster's time (ticks), a track's default frame duration
    /// (s), the last frame's time (for the next one's gap).
    cluster: Option<i64>,
    default_duration: f64,
    last_pts: Option<f64>,
    samples: Vec<Sample>,
    keep: bool,
    bytes: usize,
    /// The last frame the player read.
    read: Option<u64>,
    next_seq: u64,
    ranges: Vec<[f64; 2]>,
    offset: f64,
}

impl Buffer {
    fn new(webm: bool) -> Buffer {
        Buffer {
            webm,
            pending: Vec::new(),
            track: None,
            inits: Vec::new(),
            track_id: 0,
            tick: if webm { 1e-3 } else { 1. / 90_000. },
            trex: HashMap::new(),
            moof: None,
            cluster: None,
            default_duration: 0.,
            last_pts: None,
            samples: Vec::new(),
            keep: false,
            bytes: 0,
            read: None,
            next_seq: 1,
            ranges: Vec::new(),
            offset: 0.,
        }
    }

    /// A frame parsed: its range, and (kept for the player) the frame.
    fn frame(&mut self, pts: f64, dur: f64, key: bool, data: &[u8]) {
        if !pts.is_finite() {
            return;
        }
        let dur = if dur.is_finite() && dur > 0. { dur } else { 0. };
        add_range(&mut self.ranges, pts, pts + dur.max(1e-3));
        let seq = self.next_seq;
        self.next_seq += 1;
        if self.keep {
            self.bytes += data.len();
            let init = self.inits.len().saturating_sub(1) as u32;
            self.samples.push(Sample { seq, init, pts, dur, key, data: Arc::from(data) });
        }
    }

    /// Frames the player has read go once too many bytes are kept.
    /// (Unread ones too where nothing reads them: no player plays it.)
    fn evict(&mut self) {
        if self.bytes <= KEPT_BYTES {
            return;
        }
        let read = self.read.unwrap_or(u64::MAX);
        let mut cut = 0;
        for f in &self.samples {
            if f.seq >= read || self.bytes <= KEPT_BYTES / 2 {
                break;
            }
            self.bytes -= f.data.len();
            cut += 1;
        }
        self.samples.drain(..cut);
    }

    // --- fragmented MPEG-4 (ISO BMFF Byte Stream Format) --- //

    fn parse_mp4(&mut self) -> Result<(), String> {
        let mut at = 0;
        let pending = std::mem::take(&mut self.pending);
        let mut result = Ok(());
        while let Some((kind, head, size)) = box_header(&pending[at..]) {
            if pending.len() - at < size {
                break;
            }
            let whole = &pending[at..at + size];
            match &kind {
                b"moov" => {
                    if let Err(e) = self.init_mp4(&whole[head..]) {
                        result = Err(e);
                    }
                }
                b"moof" => self.moof = Some(whole.to_vec()),
                b"mdat" => {
                    if let Some(moof) = self.moof.take() {
                        self.fragment(&moof, whole);
                    }
                }
                _ => {}
            }
            at += size;
        }
        self.pending = pending[at..].to_vec();
        result
    }

    /// The initialization segment: the picture track if there is one, else
    /// the first, and every track's defaults.
    fn init_mp4(&mut self, moov: &[u8]) -> Result<(), String> {
        if let Some(mvex) = children(moov, b"mvex").next() {
            for trex in children(mvex, b"trex") {
                if let (Some(id), Some(d), Some(s), Some(f)) = (be32(trex, 4), be32(trex, 12), be32(trex, 16), be32(trex, 20)) {
                    self.trex.insert(id, [d, s, f]);
                }
            }
        }
        let mut chosen: Option<(u32, f64, Track)> = None;
        for trak in children(moov, b"trak") {
            let Some(tkhd) = children(trak, b"tkhd").next() else { continue };
            let Some(id) = be32(tkhd, if tkhd.first() == Some(&1) { 20 } else { 12 }) else { continue };
            let Some(mdia) = children(trak, b"mdia").next() else { continue };
            let handler = children(mdia, b"hdlr").next().and_then(|h| h.get(8..12)).unwrap_or(b"????");
            let Some(mdhd) = children(mdia, b"mdhd").next() else { continue };
            let timescale = if mdhd.first() == Some(&1) { be32(mdhd, 20) } else { be32(mdhd, 12) }.unwrap_or(0);
            if timescale == 0 {
                continue;
            }
            let video = handler == b"vide";
            let mut track = Track { video, codec: Codec::Other, sets: Vec::new(), nal_len: 4, hevc: Vec::new(), width: 0, height: 0 };
            if video
                && let Some(stsd) = within(mdia, &[b"minf", b"stbl", b"stsd"])
                && let Some((kind, entry)) = boxes(stsd.get(8..).unwrap_or(&[])).next()
            {
                // A visual sample entry: 78 bytes of fields (width and
                // height 24 in), then its boxes.
                track.width = u32::from(be16(entry, 24).unwrap_or(0));
                track.height = u32::from(be16(entry, 26).unwrap_or(0));
                let inner = entry.get(78..).unwrap_or(&[]);
                match &kind {
                    b"avc1" | b"avc3" => {
                        if let Some((sets, len)) = children(inner, b"avcC").next().and_then(avcc) {
                            track.codec = Codec::H264;
                            track.sets = sets;
                            track.nal_len = len;
                        }
                    }
                    b"hvc1" | b"hev1" => {
                        if let Some(record) = children(inner, b"hvcC").next() {
                            track.codec = Codec::Hevc;
                            track.hevc = record.to_vec();
                        }
                    }
                    b"av01" => {
                        track.codec = Codec::Av1;
                        track.sets = children(inner, b"av1C").next().and_then(|c| c.get(4..)).unwrap_or(&[]).to_vec();
                    }
                    b"vp09" => track.codec = Codec::Vp9,
                    // (Encrypted, `encv`: not decoded.)
                    _ => {}
                }
            }
            let better = match &chosen {
                None => true,
                Some((_, _, t)) => video && !t.video,
            };
            if better {
                chosen = Some((id, timescale as f64, track));
            }
        }
        let (id, timescale, track) = chosen.ok_or("no track in the initialization segment")?;
        self.track_id = u64::from(id);
        self.tick = 1. / timescale;
        // A picture track's frames are kept, for the player.
        self.keep = track.video;
        self.inits.push(track.clone());
        self.track = Some(track);
        Ok(())
    }

    /// A media segment's fragment (`moof`) and its data (`mdat`, the box
    /// whole): its track's frames.
    fn fragment(&mut self, moof: &[u8], mdat: &[u8]) {
        let Some(track) = &self.track else { return };
        let video = track.video;
        let head = box_header(moof).map_or(8, |h| h.1);
        let body = &moof[head..];
        // Offsets are from the moof's first byte; the mdat follows it.
        let data_at = |offset: i64, len: usize| -> Option<&[u8]> {
            let start = usize::try_from(offset - moof.len() as i64).ok()?;
            mdat.get(start..start + len)
        };
        let mut frames = Vec::new();
        for traf in children(body, b"traf") {
            let Some(tfhd) = children(traf, b"tfhd").next() else { continue };
            let flags = be32(tfhd, 0).unwrap_or(0) & 0xFF_FFFF;
            let Some(id) = be32(tfhd, 4) else { continue };
            if u64::from(id) != self.track_id {
                continue;
            }
            let [mut d_dur, mut d_size, mut d_flags] = self.trex.get(&id).copied().unwrap_or([0, 0, 0]);
            let mut at = 8;
            // (A base data offset is from the file's start, which a byte
            // stream doesn't have: taken as the moof's, as Chrome does.)
            if flags & 0x1 != 0 {
                at += 8;
            }
            if flags & 0x2 != 0 {
                at += 4;
            }
            if flags & 0x8 != 0 {
                d_dur = be32(tfhd, at).unwrap_or(d_dur);
                at += 4;
            }
            if flags & 0x10 != 0 {
                d_size = be32(tfhd, at).unwrap_or(d_size);
                at += 4;
            }
            if flags & 0x20 != 0 {
                d_flags = be32(tfhd, at).unwrap_or(d_flags);
            }
            let mut dts: u64 = match children(traf, b"tfdt").next() {
                Some(t) if t.first() == Some(&1) => be64(t, 4).unwrap_or(0),
                Some(t) => u64::from(be32(t, 4).unwrap_or(0)),
                None => 0,
            };
            let mut next_data: i64 = 0;
            for trun in children(traf, b"trun") {
                let version = trun.first().copied().unwrap_or(0);
                let tflags = be32(trun, 0).unwrap_or(0) & 0xFF_FFFF;
                let count = be32(trun, 4).unwrap_or(0) as usize;
                let mut at = 8;
                if tflags & 0x1 != 0 {
                    next_data = i64::from(be32(trun, at).unwrap_or(0) as i32);
                    at += 4;
                }
                let first_flags = if tflags & 0x4 != 0 {
                    at += 4;
                    be32(trun, at - 4)
                } else {
                    None
                };
                for i in 0..count.min(1 << 20) {
                    let mut field = |on: bool| -> Option<u32> {
                        if !on {
                            return None;
                        }
                        let v = be32(trun, at);
                        at += 4;
                        v
                    };
                    let dur = field(tflags & 0x100 != 0).unwrap_or(d_dur);
                    let size = field(tflags & 0x200 != 0).unwrap_or(d_size) as usize;
                    let sflags = field(tflags & 0x400 != 0).or(if i == 0 { first_flags } else { None }).unwrap_or(d_flags);
                    let cto = field(tflags & 0x800 != 0).map_or(0, |v| if version == 0 { i64::from(v) } else { i64::from(v as i32) });
                    // (`sample_is_non_sync_sample`, bit 16; sound is all key.)
                    let key = !video || sflags & 0x1_0000 == 0;
                    let pts = (dts as i64 + cto) as f64 * self.tick + self.offset;
                    frames.push((pts, f64::from(dur) * self.tick, key, next_data, size));
                    next_data += size as i64;
                    dts += u64::from(dur);
                }
            }
        }
        for (pts, dur, key, offset, size) in frames {
            let data = if self.keep { data_at(offset, size).map(<[u8]>::to_vec) } else { None };
            self.frame(pts, dur, key, data.as_deref().unwrap_or(&[]));
        }
    }

    // --- WebM --- //

    fn parse_webm(&mut self) -> Result<(), String> {
        let pending = std::mem::take(&mut self.pending);
        let mut at = 0;
        loop {
            let b = &pending[at..];
            let Some((id, id_len)) = ebml_id(b) else { break };
            let Some((size, size_len, unknown)) = ebml_size(&b[id_len..]) else { break };
            let head = id_len + size_len;
            // Segment and Cluster are entered (their ends come with later
            // appends, or never: a live stream's sizes are unknown); so is
            // any element of unknown size.
            if id == 0x1853_8067 || id == 0x1F43_B675 || unknown {
                if id == 0x1F43_B675 {
                    self.cluster = None;
                }
                at += head;
                continue;
            }
            let size = usize::try_from(size).map_err(|_| "a WebM element too large")?;
            if b.len() < head + size {
                break;
            }
            let body = &b[head..head + size];
            match id {
                // Info: TimecodeScale (ns a tick).
                0x1549_A966 => {
                    for (cid, c) in ebml_children(body) {
                        if cid == 0x2A_D7B1 {
                            self.tick = uint(c) as f64 / 1e9;
                        }
                    }
                }
                0x1654_AE6B => self.webm_tracks(body),
                // A cluster's Timecode.
                0xE7 => self.cluster = Some(uint(body) as i64),
                0xA3 => self.block(body, None),
                // BlockGroup: its Block, BlockDuration, and a ReferenceBlock
                // (which a key frame has none of).
                0xA0 => {
                    let (mut block, mut dur, mut refers) = (None, None, false);
                    for (cid, c) in ebml_children(body) {
                        match cid {
                            0xA1 => block = Some(c),
                            0x9B => dur = Some(uint(c)),
                            0xFB => refers = true,
                            _ => {}
                        }
                    }
                    if let Some(block) = block {
                        self.block(block, Some((dur, !refers)));
                    }
                }
                _ => {}
            }
            at += head + size;
        }
        self.pending = pending[at..].to_vec();
        Ok(())
    }

    /// Tracks: the picture track if there is one, else the first.
    fn webm_tracks(&mut self, body: &[u8]) {
        let mut chosen: Option<(u64, f64, Track)> = None;
        for (id, entry) in ebml_children(body) {
            if id != 0xAE {
                continue;
            }
            let (mut number, mut kind, mut codec_id, mut private, mut default) = (0, 0, String::new(), Vec::new(), 0u64);
            let (mut width, mut height) = (0, 0);
            for (cid, c) in ebml_children(entry) {
                match cid {
                    0xD7 => number = uint(c),
                    0x83 => kind = uint(c),
                    0x86 => codec_id = String::from_utf8_lossy(c).trim_end_matches('\0').to_owned(),
                    0x63A2 => private = c.to_vec(),
                    0x23_E383 => default = uint(c),
                    0xE0 => {
                        for (vid, v) in ebml_children(c) {
                            match vid {
                                0xB0 => width = uint(v) as u32,
                                0xBA => height = uint(v) as u32,
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            let video = kind == 1;
            let mut track = Track { video, codec: Codec::Other, sets: Vec::new(), nal_len: 4, hevc: Vec::new(), width, height };
            if video {
                match codec_id.as_str() {
                    "V_VP9" => track.codec = Codec::Vp9,
                    "V_AV1" => {
                        track.codec = Codec::Av1;
                        track.sets = private.get(4..).unwrap_or(&[]).to_vec();
                    }
                    "V_MPEG4/ISO/AVC" => {
                        if let Some((sets, len)) = avcc(&private) {
                            track.codec = Codec::H264;
                            track.sets = sets;
                            track.nal_len = len;
                        }
                    }
                    "V_MPEGH/ISO/HEVC" => {
                        track.codec = Codec::Hevc;
                        track.hevc = private;
                    }
                    _ => {}
                }
            }
            let better = match &chosen {
                None => true,
                Some((_, _, t)) => video && !t.video,
            };
            if better {
                chosen = Some((number, default as f64 / 1e9, track));
            }
        }
        if let Some((number, default, track)) = chosen {
            self.track_id = number;
            self.default_duration = default;
            self.keep = track.video;
            self.inits.push(track.clone());
            self.track = Some(track);
        }
    }

    /// A SimpleBlock or a BlockGroup's Block (with its group's duration
    /// and whether it is a key frame).
    fn block(&mut self, b: &[u8], group: Option<(Option<u64>, bool)>) {
        let Some((track, len, _)) = ebml_size(b) else { return };
        if track != self.track_id || self.track.is_none() {
            return;
        }
        let (Some(tc), Some(&flags)) = (b.get(len..len + 2), b.get(len + 2)) else { return };
        let tc = i64::from(i16::from_be_bytes([tc[0], tc[1]]));
        let cluster = self.cluster.unwrap_or(0);
        let pts = (cluster + tc) as f64 * self.tick + self.offset;
        let key = match group {
            Some((_, key)) => key,
            None => flags & 0x80 != 0,
        };
        // Laced frames (sound, several to a block): one range, no frames.
        let laced = (flags >> 1) & 3 != 0;
        let explicit = group.and_then(|g| g.0).map(|d| d as f64 * self.tick);
        // A block's length: its group's, else the track's default, else
        // the last gap (and the gap to it fills the range between).
        let gap = self.last_pts.map(|l| pts - l).filter(|d| *d > 0. && *d < 1.);
        if let Some(last) = self.last_pts
            && gap.is_some()
        {
            add_range(&mut self.ranges, last, pts);
        }
        let dur = explicit.or((self.default_duration > 0.).then_some(self.default_duration)).or(gap).unwrap_or(1. / 30.);
        self.last_pts = Some(pts);
        let data = if laced || !self.keep { &[][..] } else { &b[len + 3..] };
        self.frame(pts, dur, key, data);
    }
}

/// `[start, end)` into sorted, merged ranges.
fn add_range(ranges: &mut Vec<[f64; 2]>, start: f64, end: f64) {
    if !(end > start) {
        return;
    }
    let mut merged = [start, end];
    ranges.retain(|r| {
        if r[0] <= merged[1] + GAP && merged[0] <= r[1] + GAP {
            merged = [merged[0].min(r[0]), merged[1].max(r[1])];
            false
        } else {
            true
        }
    });
    let at = ranges.partition_point(|r| r[0] < merged[0]);
    ranges.insert(at, merged);
}

// --- boxes and elements --- //

fn be16(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|x| u16::from_be_bytes([x[0], x[1]]))
}
fn be32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]))
}
fn be64(b: &[u8], at: usize) -> Option<u64> {
    b.get(at..at + 8).and_then(|x| x.try_into().ok()).map(u64::from_be_bytes)
}

/// A box's type, header length and whole size (from its header alone).
fn box_header(b: &[u8]) -> Option<([u8; 4], usize, usize)> {
    let size = be32(b, 0)? as usize;
    let kind: [u8; 4] = b.get(4..8)?.try_into().ok()?;
    match size {
        1 => Some((kind, 16, usize::try_from(be64(b, 8)?).ok()?)).filter(|h| h.2 >= 16),
        // (To the end of the file: a byte stream has none.)
        0 => None,
        n if n >= 8 => Some((kind, 8, n)),
        _ => None,
    }
}

/// The boxes in `b`, each its type and body.
fn boxes(b: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    let mut at = 0;
    std::iter::from_fn(move || {
        let (kind, head, size) = box_header(b.get(at..)?)?;
        let body = b.get(at + head..at + size)?;
        at += size;
        Some((kind, body))
    })
}
fn children<'a>(b: &'a [u8], kind: &'a [u8; 4]) -> impl Iterator<Item = &'a [u8]> {
    boxes(b).filter(move |(k, _)| k == kind).map(|(_, body)| body)
}
fn within<'a>(mut b: &'a [u8], path: &[&'a [u8; 4]]) -> Option<&'a [u8]> {
    for k in path {
        b = children(b, k).next()?;
    }
    Some(b)
}

/// An `avcC` record: its SPS and PPS with start codes, and its NAL
/// length's size.
fn avcc(b: &[u8]) -> Option<(Vec<u8>, usize)> {
    let len = usize::from(*b.get(4)? & 3) + 1;
    let mut sets = Vec::new();
    let mut at = 5;
    for mask in [0x1F, 0xFF] {
        let n = *b.get(at)? & mask;
        at += 1;
        for _ in 0..n {
            let l = usize::from(be16(b, at)?);
            sets.extend_from_slice(&[0, 0, 0, 1]);
            sets.extend_from_slice(b.get(at + 2..at + 2 + l)?);
            at += 2 + l;
        }
    }
    Some((sets, len))
}

/// An EBML element id (its marker bits kept, as ids are written) and its
/// length.
fn ebml_id(b: &[u8]) -> Option<(u32, usize)> {
    let first = *b.first()?;
    let len = first.leading_zeros() as usize + 1;
    if len > 4 {
        return None;
    }
    let bytes = b.get(..len)?;
    Some((bytes.iter().fold(0u32, |a, &x| a << 8 | u32::from(x)), len))
}
/// An EBML size (or a block's track number): its value, length, and
/// whether it is "unknown" (all its value bits set).
fn ebml_size(b: &[u8]) -> Option<(u64, usize, bool)> {
    let first = *b.first()?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 {
        return None;
    }
    let bytes = b.get(..len)?;
    let mask = if len == 8 { 0 } else { 0xFFu8 >> len };
    let value = bytes[1..].iter().fold(u64::from(first & mask), |a, &x| a << 8 | u64::from(x));
    let unknown = value == (1u64 << (7 * len)) - 1;
    Some((value, len, unknown))
}
/// The complete elements in `b`.
fn ebml_children(b: &[u8]) -> impl Iterator<Item = (u32, &[u8])> {
    let mut at = 0;
    std::iter::from_fn(move || {
        let rest = b.get(at..)?;
        let (id, il) = ebml_id(rest)?;
        let (size, sl, _) = ebml_size(rest.get(il..)?)?;
        let size = usize::try_from(size).ok()?;
        let body = rest.get(il + sl..il + sl + size)?;
        at += il + sl + size;
        Some((id, body))
    })
}
fn uint(b: &[u8]) -> u64 {
    b.iter().take(8).fold(0u64, |a, &x| a << 8 | u64::from(x))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        b.extend_from_slice(kind);
        b.extend_from_slice(body);
        b
    }
    fn full(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
        let mut b = vec![version];
        b.extend_from_slice(&flags.to_be_bytes()[1..]);
        b.extend_from_slice(body);
        mp4_box(kind, &b)
    }
    /// An initialization segment: one H.264 track (id 1, 1000 ticks a
    /// second), its avcC with one SPS and one PPS.
    fn init() -> Vec<u8> {
        let mut avcc = vec![1, 0x64, 0, 0x1F, 0xFF, 0xE1, 0, 3, 0x67, 1, 2, 1, 0, 2, 0x68, 3];
        avcc = mp4_box(b"avcC", &avcc);
        let mut entry = vec![0u8; 78];
        entry[24..26].copy_from_slice(&640u16.to_be_bytes());
        entry[26..28].copy_from_slice(&360u16.to_be_bytes());
        entry.extend_from_slice(&avcc);
        let stsd = full(b"stsd", 0, 0, &[&1u32.to_be_bytes()[..], &mp4_box(b"avc1", &entry)].concat());
        let stbl = mp4_box(b"stbl", &stsd);
        let minf = mp4_box(b"minf", &stbl);
        let mut hdlr = vec![0u8; 4];
        hdlr.extend_from_slice(b"vide");
        hdlr.extend_from_slice(&[0; 12]);
        let hdlr = full(b"hdlr", 0, 0, &hdlr);
        let mut mdhd = vec![0u8; 8];
        mdhd.extend_from_slice(&1000u32.to_be_bytes());
        mdhd.extend_from_slice(&[0; 8]);
        let mdhd = full(b"mdhd", 0, 0, &mdhd);
        let mdia = mp4_box(b"mdia", &[mdhd, hdlr, minf].concat());
        let mut tkhd = vec![0u8; 8];
        tkhd.extend_from_slice(&1u32.to_be_bytes());
        tkhd.extend_from_slice(&[0; 60]);
        let tkhd = full(b"tkhd", 0, 0, &tkhd);
        let trak = mp4_box(b"trak", &[tkhd, mdia].concat());
        let trex = full(b"trex", 0, 0, &[1u32, 1, 40, 0, 0x1_0000].iter().flat_map(|v| v.to_be_bytes()).collect::<Vec<_>>());
        let mvex = mp4_box(b"mvex", &trex);
        [mp4_box(b"ftyp", b"isom\0\0\0\0"), mp4_box(b"moov", &[trak, mvex].concat())].concat()
    }
    /// A media segment: `n` frames of 40 ticks from `start`, the first a
    /// key frame, each 3 bytes (its index).
    fn segment(start: u64, n: u8) -> Vec<u8> {
        let tfhd = full(b"tfhd", 0, 0x2_0000, &1u32.to_be_bytes());
        let tfdt = full(b"tfdt", 1, 0, &start.to_be_bytes());
        let mut trun = (n as u32).to_be_bytes().to_vec();
        let offset_at = trun.len();
        trun.extend_from_slice(&0i32.to_be_bytes());
        trun.extend_from_slice(&0u32.to_be_bytes()); // first sample flags: key
        for _ in 0..n {
            trun.extend_from_slice(&3u32.to_be_bytes());
        }
        let trun_box = |t: &[u8]| full(b"trun", 0, 0x1 | 0x4 | 0x200, t);
        let traf = |t: &[u8]| mp4_box(b"traf", &[tfhd.clone(), tfdt.clone(), trun_box(t)].concat());
        let moof = |t: &[u8]| mp4_box(b"moof", &[full(b"mfhd", 0, 0, &1u32.to_be_bytes()), traf(t)].concat());
        let len = moof(&trun).len();
        trun[offset_at..offset_at + 4].copy_from_slice(&((len + 8) as i32).to_be_bytes());
        let data: Vec<u8> = (0..n).flat_map(|i| [i, i, i]).collect();
        [moof(&trun), mp4_box(b"mdat", &data)].concat()
    }

    #[test]
    fn mp4_segments_give_ranges_and_frames() {
        let s = Stream::new();
        let i = s.add_buffer("video/mp4; codecs=\"avc1.64001f\"").unwrap();
        s.append(i, &init()).unwrap();
        let (vi, track) = s.video_track(Duration::ZERO).unwrap();
        assert_eq!((vi, track.codec, track.width, track.height, track.nal_len), (i, Codec::H264, 640, 360, 4));
        assert_eq!(track.sets, [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3]);
        // A segment split across appends parses once it is whole.
        let seg = segment(0, 25);
        s.append(i, &seg[..10]).unwrap();
        assert!(s.buffered(i).is_empty());
        s.append(i, &seg[10..]).unwrap();
        assert_eq!(s.buffered(i), [[0., 1.]]);
        let Next::Sample(first) = s.next_sample(i, None, Duration::ZERO) else { panic!("no frame") };
        assert!(first.key && first.pts == 0. && (first.dur - 0.04).abs() < 1e-9 && &*first.data == [0, 0, 0]);
        let Next::Sample(second) = s.next_sample(i, Some(first.seq), Duration::ZERO) else { panic!("no second frame") };
        assert!(!second.key && (second.pts - 0.04).abs() < 1e-9 && &*second.data == [1, 1, 1]);
        // A gap, then the time after it: two ranges; removing joins nothing.
        s.append(i, &segment(2000, 25)).unwrap();
        assert_eq!(s.buffered(i), [[0., 1.], [2., 3.]]);
        s.remove(i, 0., 0.5);
        assert_eq!(s.buffered(i), [[0.5, 1.], [2., 3.]]);
        // Seeking into the second: from its key frame.
        let after = s.seek(i, 2.5).unwrap();
        let Next::Sample(k) = s.next_sample(i, Some(after), Duration::ZERO) else { panic!("no key frame") };
        assert!(k.key && (k.pts - 2.).abs() < 1e-9);
        s.end();
        assert!(matches!(s.next_sample(i, Some(u64::MAX - 1), Duration::ZERO), Next::End));
    }

    #[test]
    fn webm_clusters_give_ranges() {
        fn el(id: &[u8], body: &[u8]) -> Vec<u8> {
            let mut b = id.to_vec();
            b.extend_from_slice(&[0x01, 0, 0, 0, 0, 0, 0, 0]);
            b[id.len() + 1..].copy_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
            b.extend_from_slice(body);
            b
        }
        let track = el(&[0xAE], &[el(&[0xD7], &[1]), el(&[0x83], &[1]), el(&[0x86], b"V_VP9")].concat());
        let head = [
            el(&[0x1A, 0x45, 0xDF, 0xA3], &[]),
            vec![0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
            el(&[0x15, 0x49, 0xA9, 0x66], &el(&[0x2A, 0xD7, 0xB1], &[0x0F, 0x42, 0x40])),
            el(&[0x16, 0x54, 0xAE, 0x6B], &track),
        ]
        .concat();
        let block = |tc: i16, key: bool| el(&[0xA3], &[&[0x81][..], &tc.to_be_bytes(), &[if key { 0x80 } else { 0 }], &[9, 9]].concat());
        let cluster = |time: u8| [vec![0x1F, 0x43, 0xB6, 0x75, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF], el(&[0xE7], &[time]), block(0, true), block(33, false), block(66, false)].concat();
        let s = Stream::new();
        let i = s.add_buffer("video/webm; codecs=\"vp9\"").unwrap();
        s.append(i, &head).unwrap();
        let (_, t) = s.video_track(Duration::ZERO).unwrap();
        assert_eq!(t.codec, Codec::Vp9);
        s.append(i, &cluster(0)).unwrap();
        s.append(i, &cluster(100)).unwrap();
        let r = s.buffered(i);
        assert_eq!(r.len(), 1, "{r:?}");
        assert!(r[0][0] == 0. && (r[0][1] - 0.199).abs() < 1e-6, "{r:?}");
        let Next::Sample(f) = s.next_sample(i, None, Duration::ZERO) else { panic!("no frame") };
        assert!(f.key && &*f.data == [9, 9]);
    }

    #[test]
    fn types() {
        assert!(supported("video/mp4; codecs=\"avc1.4d401f\""));
        assert!(supported("video/webm; codecs=\"vp09.00.51.08.01.01.01.01.00\""));
        assert!(supported("audio/mp4; codecs=\"mp4a.40.2\""));
        assert!(supported("audio/webm; codecs=\"opus\""));
        assert!(!supported("video/mp2t; codecs=\"avc1.42E01E\""));
        assert!(!supported("video/webm; codecs=\"vp8\""));
    }
}
