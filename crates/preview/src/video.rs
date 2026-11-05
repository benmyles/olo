//! Playing video in a preview. A worker demuxes H.264, AV1 or VP9 (mp4 or
//! Matroska and WebM) and decodes it (OpenH264, rav1d, filmcraft-vp9) a few
//! frames ahead into a small queue; the
//! UI shows each frame when the clock reaches it. Queued frames stay as the
//! decoder made them (YUV, under half the size of RGBA) and only the one
//! at the front is turned into a picture, by the worker, so a page playing
//! twenty large videos at once holds megabytes rather than gigabytes. The clock runs on its
//! own and follows the sound (played by [`audio::Player`]) when there is
//! any, so a file without sound, or no sound server, still plays.

use olo_sync::{Condvar, Mutex};
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, VecDeque},
    io::BufReader,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use olo_images::{av1, hevc, vp9};

use crate::{
    Bitmap, audio,
    input::{Input, ReadSeek},
    load::Image,
    media,
};

/// Frames decoded ahead (about a fifth of a second at 30 fps)...
const AHEAD: usize = 6;
/// ...unless that would hold more than this, when large pictures get
/// fewer (never under two).
const QUEUE_BYTES: usize = 24 << 20;

/// How far the clock may drift from the sound before it is pulled back.
const DRIFT: f64 = 0.12;

struct Frame {
    pts: f64,
    pic: Pic,
}

/// A picture to show, in the form its viewer asked for.
#[derive(Clone)]
pub enum Picture {
    Rgba(Image),
    /// Straight BGRA bytes, `size` pixels (what pages take).
    Bgra {
        size: [usize; 2],
        data: Arc<Vec<u8>>,
    },
}

impl Picture {
    /// As RGBA (converting a BGRA one).
    pub fn into_image(self) -> Image {
        match self {
            Picture::Rgba(i) => i,
            Picture::Bgra { size, data } => Arc::new(Bitmap::from_bgra_unmultiplied(size, &data)),
        }
    }
}

/// A decoded picture: compact as decoded, or ready to show.
#[derive(Clone)]
enum Pic {
    H264(Arc<Planar>),
    /// AV1's or VP9's planes.
    Planes(Arc<av1::Frame>),
    Ready(Picture),
}

impl Pic {
    fn ready(&self) -> Option<&Picture> {
        match self {
            Pic::Ready(p) => Some(p),
            _ => None,
        }
    }

    fn bytes(&self) -> usize {
        match self {
            Pic::H264(p) => p.y.len() + p.u.len() + p.v.len(),
            // 16-bit samples, chroma subsampled (at most).
            Pic::Planes(f) => f.width as usize * f.height as usize * 6,
            Pic::Ready(Picture::Rgba(i)) => i.rgba.len(),
            Pic::Ready(Picture::Bgra { data, .. }) => data.len(),
        }
    }

    fn picture(&self, bgra: bool) -> Picture {
        match self {
            Pic::H264(p) if bgra => Picture::Bgra { size: [p.width, p.height], data: Arc::new(p.bgra()) },
            Pic::H264(p) => Picture::Rgba(p.image()),
            Pic::Planes(f) if bgra => {
                let mut data = f.to_rgba();
                for px in data.as_chunks_mut::<4>().0 {
                    px.swap(0, 2);
                }
                Picture::Bgra { size: [f.width as usize, f.height as usize], data: Arc::new(data) }
            }
            Pic::Planes(f) => Picture::Rgba(av1_image(f)),
            Pic::Ready(p) => p.clone(),
        }
    }
}

/// An H.264 picture: 8-bit 4:2:0 planes, packed.
struct Planar {
    width: usize,
    height: usize,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

impl Planar {
    fn from(yuv: &openh264::decoder::DecodedYUV<'_>) -> Planar {
        use openh264::formats::YUVSource;
        let (w, h) = yuv.dimensions();
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let (sy, su, sv) = yuv.strides();
        let pack = |plane: &[u8], stride: usize, w: usize, h: usize| {
            let mut out = Vec::with_capacity(w * h);
            for row in 0..h {
                out.extend_from_slice(&plane[row * stride..][..w]);
            }
            out
        };
        Planar { width: w, height: h, y: pack(yuv.y(), sy, w, h), u: pack(yuv.u(), su, cw, ch), v: pack(yuv.v(), sv, cw, ch) }
    }

    /// Each pixel as (r, g, b), converting as OpenH264 does
    /// (limited-range BT.601), in fixed point.
    fn convert(&self, mut put: impl FnMut(usize, [u8; 3])) {
        let (w, h) = (self.width, self.height);
        let cw = w.div_ceil(2);
        let c = |n: i32| (n >> 8).clamp(0, 255) as u8;
        for row in 0..h {
            let ys = &self.y[row * w..][..w];
            let (us, vs) = (&self.u[(row / 2) * cw..][..cw], &self.v[(row / 2) * cw..][..cw]);
            for (x, &y) in ys.iter().enumerate() {
                let (u, v) = (us[x / 2] as i32 - 128, vs[x / 2] as i32 - 128);
                let l = 298 * (y as i32 - 16) + 128;
                put(row * w + x, [c(l + 409 * v), c(l - 100 * u - 208 * v), c(l + 516 * u)]);
            }
        }
    }

    fn image(&self) -> Image {
        let mut out = vec![255u8; self.width * self.height * 4];
        let px = out.as_chunks_mut::<4>().0;
        self.convert(|i, [r, g, b]| {
            px[i][0] = r;
            px[i][1] = g;
            px[i][2] = b;
        });
        Arc::new(Bitmap::from_rgba_premultiplied([self.width, self.height], out))
    }

    fn bgra(&self) -> Vec<u8> {
        let mut out = vec![255u8; self.width * self.height * 4];
        let px = out.as_chunks_mut::<4>().0;
        self.convert(|i, [r, g, b]| {
            px[i][0] = b;
            px[i][1] = g;
            px[i][2] = r;
        });
        out
    }
}

#[derive(Default)]
struct Shared {
    queue: VecDeque<Frame>,
    /// Frames to keep ahead, from the pictures' size.
    ahead: usize,
    /// Make pictures BGRA (for pages) rather than RGBA.
    bgra: bool,
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
    /// There are pictures to show; without, only the clock and sound run.
    picture: bool,
    bgra: bool,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Video {
    /// Start playing `input` (of `duration` seconds) at once.
    pub fn start(input: &Input, duration: f64) -> Video {
        Video::start_with(input, duration, Options { playing: true, picture: true, sound: true, bgra: false })
    }

    /// Start `input` as `opts` say: playing or paused (showing its first
    /// picture), with pictures and sound or only one of them.
    pub fn start_with(input: &Input, duration: f64, opts: Options) -> Video {
        let shared = Arc::new((Mutex::new(Shared { eof: !opts.picture, bgra: opts.bgra, ..Default::default() }), Condvar::new()));
        let (i, s) = (input.clone(), shared.clone());
        let playing = opts.playing;
        let worker = opts
            .picture
            .then(|| {
                std::thread::Builder::new()
                    .name("video".into())
                    .spawn(move || {
                        if let Err(e) = decode(&i, &s) {
                            tracing::warn!("video {}: {e}", i.name);
                            let mut sh = s.0.lock().unwrap();
                            sh.error = Some(e);
                            sh.eof = true;
                        }
                    })
                    .ok()
            })
            .flatten();
        Video {
            shared,
            clock: Mutex::new(Clock { at: 0.0, since: playing.then(Instant::now), heard_ms: 0, finished: false, show_next: opts.picture }),
            audio: opts.sound.then(|| audio::Player::start_with(input, playing)),
            duration,
            picture: opts.picture,
            bgra: opts.bgra,
            worker,
        }
    }

    /// Stop, without waiting: the decoding thread, which lets its memory
    /// go as it finishes (after the picture it's on, or once the data it
    /// waits for comes).
    pub fn close(mut self) -> Option<std::thread::JoinHandle<()>> {
        self.worker.take()
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

    pub fn has_picture(&self) -> bool {
        self.picture
    }

    /// It played to the end.
    pub fn finished(&self) -> bool {
        self.clock.lock().unwrap().finished
    }

    pub fn set_volume(&self, volume: f64) {
        if let Some(a) = &self.audio {
            a.set_volume(volume);
        }
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

    pub fn set_playing(&self, on: bool) {
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
            c.show_next = self.picture;
        }
        if let Some(a) = self.sound() {
            a.seek(secs);
        }
        if !self.picture {
            return;
        }
        let (m, cv) = &*self.shared;
        let mut sh = m.lock().unwrap();
        sh.seek = Some(secs);
        sh.queue.clear();
        cv.notify_all();
    }

    /// The frame due now, if it changed, and when the next one is due.
    pub fn tick(&self) -> (Option<Image>, Option<Duration>) {
        let (p, next) = self.tick_picture();
        (p.map(Picture::into_image), next)
    }

    /// [`Video::tick`] in the form asked for at the start
    /// ([`Options::bgra`]).
    pub fn tick_picture(&self) -> (Option<Picture>, Option<Duration>) {
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
        let ended = if self.picture {
            // The pictures ran out (the sound ends by itself).
            sh.eof && sh.queue.is_empty() && sh.seek.is_none()
        } else {
            now >= self.duration || self.sound().is_some_and(|a| a.state.finished.load(Ordering::Relaxed))
        };
        if ended && c.since.is_some() {
            c.at = self.duration;
            c.since = None;
            c.finished = true;
        }
        // Playing, or waiting for the picture a seek asked for.
        let idle = if self.picture { Duration::from_millis(15) } else { Duration::from_millis(100) };
        let next = if c.since.is_some() || (c.show_next && !sh.eof) { Some(next.unwrap_or(idle)) } else { None };
        drop(sh);
        drop(c);
        // Ready unless the worker fell behind.
        (due.map(|f| f.pic.ready().cloned().unwrap_or_else(|| f.pic.picture(self.bgra))), next)
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        let (m, cv) = &*self.shared;
        m.lock().unwrap().stop = true;
        cv.notify_all();
    }
}

/// How [`Video::start_with`] starts.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub playing: bool,
    pub picture: bool,
    pub sound: bool,
    /// Pictures as BGRA bytes ([`Video::tick_picture`]).
    pub bgra: bool,
}

/// Make the picture at the front of the queue ready to show (outside the
/// lock: the UI may be waiting on it).
fn ripen(m: &Mutex<Shared>) {
    let (pts, pic, bgra) = {
        let sh = m.lock().unwrap();
        match sh.queue.front() {
            Some(f) if f.pic.ready().is_none() => (f.pts, f.pic.clone(), sh.bgra),
            _ => return,
        }
    };
    let image = pic.picture(bgra);
    let mut sh = m.lock().unwrap();
    if let Some(f) = sh.queue.front_mut()
        && f.pts == pts
        && f.pic.ready().is_none()
    {
        f.pic = Pic::Ready(image);
    }
}

fn av1_image(f: &av1::Frame) -> Image {
    Arc::new(Bitmap::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.to_rgba()))
}

/// One coded picture (H.264 in start-code form, AV1 as stored), with
/// its presentation time.
struct Packet {
    pts: f64,
    data: Vec<u8>,
    /// How long it shows, where the container says (an MP4's samples).
    dur: Option<f64>,
}

trait Demux: Send {
    fn codec(&self) -> Codec;
    /// HEVC's track: its decoder configuration and colour.
    fn hevc(&self) -> Option<(hevc::Config, hevc::Colour)> {
        None
    }
    /// What the container says of the track's colour (HDR's transfer and
    /// primaries among it), for pictures whose stream doesn't.
    fn colour(&self) -> hevc::Colour {
        Default::default()
    }
    fn next(&mut self) -> Option<Packet>;
    /// Go to the key frame at or before `secs`.
    fn seek(&mut self, secs: f64);
}

fn open(input: &Input) -> Result<Box<dyn Demux>, String> {
    if input.matroska() { Mkv::open(input).map(|d| Box::new(d) as _) } else { Mp4::open(input).map(|d| Box::new(d) as _) }
}

/// The picture showing `at` seconds into `input` (the first after a seek
/// there, as Viewer seeks), as RGBA: a video looked at without playing it
/// (an agent checking what it recorded: `olo-screen video-frame`). On the
/// caller's thread and a decoder's, done when it's out.
pub fn picture_at(input: &Input, at: f64) -> Result<Image, String> {
    let shared = Arc::new((Mutex::new(Shared { seek: Some(at.max(0.0)), ..Default::default() }), Condvar::new()));
    let (s, i) = (shared.clone(), input.clone());
    let worker = std::thread::Builder::new().name("video frame".into()).spawn(move || decode(&i, &s)).map_err(|e| e.to_string())?;
    let (m, cv) = &*shared;
    let mut sh = m.lock().unwrap();
    // (The decoder says nothing as it queues: looked at now and then.)
    while sh.queue.is_empty() && !sh.eof && sh.error.is_none() && !worker.is_finished() {
        sh = cv.wait_timeout(sh, Duration::from_millis(10)).unwrap().0;
    }
    let first = sh.queue.front().map(|f| f.pic.picture(false));
    sh.stop = true;
    drop(sh);
    cv.notify_all();
    let ended = worker.join().map_err(|_| "the decoder failed".to_string())?;
    match first {
        Some(p) => Ok(p.into_image()),
        None => Err(ended.err().unwrap_or_else(|| format!("nothing showing at {at} s"))),
    }
}

/// What has come before a seek's target, as the decoder goes to it.
#[derive(Default)]
struct Lead {
    /// The last picture before it, if it may still be showing there.
    held: Option<Pic>,
    /// Else where the last before it began (it ended before the target).
    ended: Option<f64>,
    /// A picture at or after the target was queued.
    reached: bool,
    /// Gone back for the last picture already (`ended`, past the end).
    again: bool,
}

/// What a file holds, as far as playing it goes.
#[derive(Debug, Clone, Default)]
pub struct Info {
    pub duration: Option<f64>,
    pub width: u32,
    pub height: u32,
    /// A picture track in a codec we play.
    pub picture: bool,
    pub sound: bool,
}

/// Read `input`'s tracks (mp4 or Matroska).
pub fn probe(input: &Input) -> Result<Info, String> {
    let (reader, size) = input.open()?;
    let mut info = Info::default();
    if input.matroska() {
        let mk = matroska_demuxer::MatroskaFile::open(BufReader::new(reader)).map_err(|e| e.to_string())?;
        let scale = mk.info().timestamp_scale().get() as f64;
        info.duration = mk.info().duration().map(|d| d * scale / 1e9);
        for t in mk.tracks() {
            if let Some(v) = t.video() {
                if !info.picture && matches!(t.codec_id(), "V_MPEG4/ISO/AVC" | "V_AV1" | "V_VP9" | "V_MPEGH/ISO/HEVC") {
                    info.picture = true;
                    (info.width, info.height) = (v.pixel_width().get() as u32, v.pixel_height().get() as u32);
                }
            } else if t.codec_id().starts_with("A_") {
                info.sound = true;
            }
        }
    } else {
        let r = mp4::Mp4Reader::read_header(BufReader::new(reader), size).map_err(|e| e.to_string())?;
        let d = r.duration().as_secs_f64();
        info.duration = (d > 0.0).then_some(d);
        for t in r.tracks().values() {
            match t.media_type() {
                Ok(mp4::MediaType::H264 | mp4::MediaType::H265 | mp4::MediaType::VP9) if !info.picture => {
                    info.picture = true;
                    (info.width, info.height) = (t.width() as u32, t.height() as u32);
                }
                Ok(mp4::MediaType::AAC) => info.sound = true,
                _ => {}
            }
        }
    }
    Ok(info)
}

/// The codecs played.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Codec {
    H264,
    Av1,
    Vp9,
    Hevc,
}

enum Decoder {
    H264(Box<openh264::decoder::Decoder>),
    Av1(av1::Decoder),
    Vp9(vp9::Decoder),
    Hevc(hevc::Decoder),
}

impl Decoder {
    fn new(codec: Codec, demux: &dyn Demux) -> Result<Decoder, String> {
        Ok(match codec {
            // Never flush after a packet: a picture forced out early breaks
            // the reordering of B-frames, and every B-frame after it fails.
            Codec::H264 => {
                let config = openh264::decoder::DecoderConfig::new().flush_after_decode(openh264::decoder::Flush::NoFlush);
                let d = openh264::decoder::Decoder::with_api_config(openh264::OpenH264API::from_source(), config);
                Decoder::H264(Box::new(d.map_err(|e| e.to_string())?))
            }
            // Threads to keep up with large pictures; pictures may come
            // out a few units late.
            Codec::Av1 => Decoder::Av1(av1::Decoder::new(0, 0)?),
            Codec::Vp9 => Decoder::Vp9(vp9::Decoder::new(0)),
            Codec::Hevc => {
                let (config, colour) = demux.hevc().ok_or("no HEVC configuration")?;
                Decoder::Hevc(hevc::Decoder::new(config, colour, 0))
            }
        })
    }
}

fn decode(input: &Input, shared: &(Mutex<Shared>, Condvar)) -> Result<(), String> {
    let (m, cv) = shared;
    let mut demux = open(input)?;
    let codec = demux.codec();
    let mut dec = Decoder::new(codec, demux.as_ref())?;
    let colour = demux.colour();
    // Presentation times of packets sent and not yet out: decoders hand
    // pictures back in display order, so each takes the earliest.
    // (Each with its duration, -1 where not known.)
    let mut times: BinaryHeap<Reverse<(i64, i64)>> = BinaryHeap::new();
    let mut from = 0.0;
    let mut av1_out = Vec::new();
    let mut vp9_out = Vec::new();
    let mut hevc_out = Vec::new();
    // What came before a seek's target (a still screen in a screen
    // recording is one picture, seconds long: the one showing there may
    // have started long before it).
    let mut lead = Lead::default();
    // The next seek is ours (`Lead::ended`), not asked for.
    let mut going_back = false;
    // Queue a picture (made only if it will be shown); false once a seek
    // or stop makes the rest pointless.
    let push = |times: &mut BinaryHeap<Reverse<(i64, i64)>>, from: f64, lead: &mut Lead, pic: &dyn Fn() -> Pic| -> bool {
        let Some(Reverse((t, d))) = times.pop() else { return true };
        let pts = t as f64 / 1e6;
        // Pictures before a seek's target only lead up to it; the last
        // is held (made) only if it may still be showing there (one known
        // to end before it isn't: making each cost a seek at 4K a sixth
        // more, `seek_cost`).
        if pts + 0.001 < from {
            if d < 0 || (t + d) as f64 / 1e6 > from + 0.001 {
                lead.held = Some(pic());
                lead.ended = None;
            } else {
                lead.held = None;
                lead.ended = Some(pts);
            }
            return true;
        }
        let pic = pic();
        let mut sh = m.lock().unwrap();
        if sh.seek.is_some() || sh.stop {
            return false;
        }
        if sh.ahead == 0 {
            sh.ahead = (QUEUE_BYTES / pic.bytes().max(1)).clamp(2, AHEAD);
        }
        // This one starts after the target: the one before is showing
        // there (at the target, so it's shown at once).
        if let Some(before) = lead.held.take()
            && pts > from + 0.001
        {
            sh.queue.push_back(Frame { pts: from, pic: before });
        }
        lead.reached = true;
        sh.queue.push_back(Frame { pts, pic });
        true
    };
    loop {
        ripen(m);
        {
            let mut sh = m.lock().unwrap();
            let mut unripe = false;
            while !sh.stop && sh.seek.is_none() && (sh.queue.len() >= sh.ahead.max(1) || sh.eof) {
                // The next picture due is still compact: make it first.
                if sh.queue.front().is_some_and(|f| f.pic.ready().is_none()) {
                    unripe = true;
                    break;
                }
                sh = cv.wait(sh).unwrap();
            }
            if sh.stop {
                return Ok(());
            }
            if unripe && sh.seek.is_none() {
                continue;
            }
            if let Some(t) = sh.seek.take() {
                sh.queue.clear();
                sh.eof = false;
                drop(sh);
                demux.seek(t);
                match &mut dec {
                    // (Its workers kept.)
                    Decoder::Vp9(d) => d.reset(),
                    Decoder::Hevc(d) => d.reset(),
                    _ => dec = Decoder::new(codec, demux.as_ref())?,
                }
                times.clear();
                lead = Lead { again: std::mem::take(&mut going_back), ..Lead::default() };
                from = t;
            }
        }
        let packet = demux.next();
        let ended = packet.is_none();
        if let Some(p) = &packet {
            times.push(Reverse(((p.pts * 1e6) as i64, p.dur.map_or(-1, |d| (d * 1e6) as i64))));
        }
        match (&mut dec, packet) {
            (Decoder::H264(d), Some(p)) => {
                if let Ok(Some(yuv)) = d.decode(&p.data) {
                    push(&mut times, from, &mut lead, &|| Pic::H264(Arc::new(Planar::from(&yuv))));
                }
            }
            (Decoder::H264(d), None) => {
                if let Ok(rest) = d.flush_remaining() {
                    for yuv in &rest {
                        if !push(&mut times, from, &mut lead, &|| Pic::H264(Arc::new(Planar::from(yuv)))) {
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
                for f in av1_out.drain(..) {
                    let f = Arc::new(f.with_colour(&colour));
                    if !push(&mut times, from, &mut lead, &|| Pic::Planes(f.clone())) {
                        break;
                    }
                }
            }
            // HEVC's come in display order, as H.264's and AV1's.
            (Decoder::Hevc(d), p) => {
                hevc_out.clear();
                match &p {
                    Some(p) => {
                        if let Err(e) = d.decode(&p.data, &mut hevc_out) {
                            tracing::debug!("hevc: {e}");
                        }
                    }
                    None => d.flush(&mut hevc_out),
                }
                for f in hevc_out.drain(..) {
                    let f = Arc::new(f);
                    if !push(&mut times, from, &mut lead, &|| Pic::Planes(f.clone())) {
                        break;
                    }
                }
            }
            // VP9 shows pictures in the order sent, each with its sample's
            // time: one that showed nothing (damaged) leaves its time
            // behind rather than shifting the rest.
            (Decoder::Vp9(d), p) => {
                vp9_out.clear();
                match &p {
                    Some(p) => {
                        if let Err(e) = d.decode(&p.data, (p.pts * 1e6) as i64, &mut vp9_out) {
                            tracing::debug!("vp9: {e}");
                        }
                    }
                    None => d.flush(&mut vp9_out),
                }
                for (t, f) in vp9_out.drain(..) {
                    while times.peek().is_some_and(|Reverse((e, _))| *e < t) {
                        times.pop();
                    }
                    let f = Arc::new(f.with_colour(&colour));
                    if !push(&mut times, from, &mut lead, &|| Pic::Planes(f.clone())) {
                        break;
                    }
                }
            }
        }
        if ended {
            times.clear();
            let mut sh = m.lock().unwrap();
            // Nothing at or after the target: the last picture is showing
            // there (held), or (past its end) is to be shown, decoded again.
            if sh.seek.is_none() && !sh.stop && !lead.reached {
                if let Some(before) = lead.held.take() {
                    sh.queue.push_back(Frame { pts: from, pic: before });
                } else if let Some(at) = lead.ended.take().filter(|_| !lead.again) {
                    sh.seek = Some(at);
                    going_back = true;
                    continue;
                }
            }
            sh.eof = true;
        }
    }
}

struct Mp4 {
    reader: mp4::Mp4Reader<BufReader<Box<dyn ReadSeek>>>,
    track: u32,
    codec: Codec,
    timescale: f64,
    /// Sent before each key frame: H.264's parameter sets, in start-code
    /// form, or AV1's sequence header (VP9 needs none; HEVC's decoder
    /// starts from its own, `hevc`).
    sets: Vec<u8>,
    hevc: Option<(hevc::Config, hevc::Colour)>,
    /// The track's colour as its sample entry says (AV1's, HEVC's).
    colour: hevc::Colour,
    nal_len: usize,
    /// Each sample's decode time, and the key frames (1-based).
    times: Vec<u64>,
    syncs: Option<Vec<u32>>,
    next: u32,
    /// Seconds added to the track's times to place them in the movie, from
    /// its edit list (encoders with B-frames start the pictures a frame or
    /// two in, so the first shows at 0).
    shift: f64,
}

impl Mp4 {
    fn open(input: &Input) -> Result<Mp4, String> {
        let (f, size) = input.open()?;
        let reader = mp4::Mp4Reader::read_header(BufReader::new(f), size).map_err(|e| e.to_string())?;
        let known = reader.tracks().iter().find_map(|(id, t)| match t.media_type() {
            Ok(mp4::MediaType::H264) => Some((*id, t, Codec::H264)),
            Ok(mp4::MediaType::VP9) => Some((*id, t, Codec::Vp9)),
            _ => None,
        });
        // AV1, which the mp4 crate doesn't know, and HEVC, whose `hvc1` it
        // doesn't know and `hev1`'s `hvcC` it skips: their tracks found by
        // walking the boxes.
        let av1 = if known.is_none() { input.open().ok().and_then(|(mut f, len)| media::mp4_av1(&mut f, len)) } else { None };
        let hv = if known.is_none() && av1.is_none() { input.open().ok().and_then(|(mut f, len)| media::mp4_hevc(&mut f, len)) } else { None };
        let (track, t, codec) = match (known, &av1, &hv) {
            (Some(k), ..) => k,
            (None, Some(a), _) => (a.id, reader.tracks().get(&a.id).ok_or("no AV1 track")?, Codec::Av1),
            (None, None, Some(h)) => (h.id, reader.tracks().get(&h.id).ok_or("no HEVC track")?, Codec::Hevc),
            _ => return Err("This video's format can't be played.".into()),
        };
        let hevc = match &hv {
            Some(h) => Some((hevc::config(&h.record).ok_or("no hvcC")?, h.colour)),
            None => None,
        };
        let stbl = &t.trak.mdia.minf.stbl;
        let mut sets = Vec::new();
        let mut nal_len = 0;
        match (&av1, codec) {
            // Its sequence header goes before each key frame.
            (Some(a), _) => sets.extend_from_slice(&a.obus),
            (None, Codec::Vp9 | Codec::Hevc) => {}
            (None, _) => {
                let avcc = &stbl.stsd.avc1.as_ref().ok_or("no avcC")?.avcc;
                for ps in avcc.sequence_parameter_sets.iter().chain(&avcc.picture_parameter_sets) {
                    sets.extend_from_slice(&[0, 0, 0, 1]);
                    sets.extend_from_slice(&ps.bytes);
                }
                nal_len = avcc.length_size_minus_one as usize % 4 + 1;
            }
        }
        let mut times = Vec::with_capacity(t.sample_count() as usize);
        let mut at = 0u64;
        for e in &stbl.stts.entries {
            for _ in 0..e.sample_count {
                times.push(at);
                at += e.sample_delta as u64;
            }
        }
        // An empty edit delays the track; the first real one says where in
        // the track the movie starts.
        let mut shift = 0.0;
        let movie_scale = reader.moov.mvhd.timescale.max(1) as f64;
        let timescale = t.timescale().max(1) as f64;
        for e in t.trak.edts.iter().flat_map(|e| e.elst.iter()).flat_map(|l| &l.entries) {
            if e.media_time == u32::MAX as u64 || e.media_time == u64::MAX {
                shift += e.segment_duration as f64 / movie_scale;
            } else {
                shift -= e.media_time as f64 / timescale;
                break;
            }
        }
        let colour = av1.as_ref().map(|a| a.colour).or(hv.as_ref().map(|h| h.colour)).unwrap_or_default();
        Ok(Mp4 { track, codec, timescale, shift, sets, hevc, colour, nal_len, times, syncs: stbl.stss.as_ref().map(|s| s.entries.clone()), next: 1, reader })
    }
}

impl Demux for Mp4 {
    fn colour(&self) -> hevc::Colour {
        self.colour
    }

    fn codec(&self) -> Codec {
        self.codec
    }

    fn hevc(&self) -> Option<(hevc::Config, hevc::Colour)> {
        self.hevc.clone()
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
            // (AV1 samples are OBUs as the decoder takes them, VP9's
            // frames or superframes, HEVC's length-prefixed NAL units.)
            match self.codec {
                Codec::Av1 | Codec::Vp9 | Codec::Hevc => data.extend_from_slice(&s.bytes),
                Codec::H264 => media::avcc_to_annexb(&s.bytes, self.nal_len, &mut data),
            }
            let pts = (s.start_time as i64 + s.rendering_offset as i64) as f64 / self.timescale + self.shift;
            return Some(Packet { pts, data, dur: Some(f64::from(s.duration) / self.timescale) });
        }
    }

    fn seek(&mut self, secs: f64) {
        let target = ((secs - self.shift).max(0.0) * self.timescale) as u64;
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
    file: matroska_demuxer::MatroskaFile<BufReader<Box<dyn ReadSeek>>>,
    track: u64,
    codec: Codec,
    scale: f64,
    sets: Vec<u8>,
    nal_len: usize,
    hevc: Option<(hevc::Config, hevc::Colour)>,
    /// The track's Colour element, whatever its codec.
    colour: hevc::Colour,
    frame: matroska_demuxer::Frame,
    /// Send the parameter sets before the next picture (start, seeks).
    fresh: bool,
}

impl Mkv {
    /// Whether the frame read is a key frame: as its block says, else
    /// (block groups say nothing) as VP9's own header, or HEVC's NAL
    /// units, do.
    fn key(&self) -> bool {
        self.frame.is_keyframe.unwrap_or_else(|| match (self.codec, &self.hevc) {
            (Codec::Vp9, _) => vp9::is_keyframe(&self.frame.data),
            (Codec::Hevc, Some((c, _))) => hevc::is_keyframe(&self.frame.data, c.nal_len),
            _ => false,
        })
    }

    fn open(input: &Input) -> Result<Mkv, String> {
        let (f, _) = input.open()?;
        let file = matroska_demuxer::MatroskaFile::open(BufReader::new(f)).map_err(|e| e.to_string())?;
        let (t, codec) = file
            .tracks()
            .iter()
            .filter(|t| t.video().is_some())
            .find_map(|t| match t.codec_id() {
                "V_MPEG4/ISO/AVC" => Some((t, Codec::H264)),
                "V_AV1" => Some((t, Codec::Av1)),
                "V_VP9" => Some((t, Codec::Vp9)),
                "V_MPEGH/ISO/HEVC" => Some((t, Codec::Hevc)),
                _ => None,
            })
            .ok_or("This video's format can't be played.")?;
        // HEVC's CodecPrivate is its decoder configuration record.
        let colour = t.video().map(media::mkv_colour).unwrap_or_default();
        let hevc = match codec {
            Codec::Hevc => Some((t.codec_private().and_then(hevc::config).ok_or("no HEVC configuration")?, colour)),
            _ => None,
        };
        // AV1 blocks are OBUs as the decoder takes them, with sequence
        // headers in key frames; VP9's are frames or superframes.
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
            hevc,
            colour,
            frame: matroska_demuxer::Frame::default(),
            fresh: true,
            file,
        })
    }
}

impl Demux for Mkv {
    fn colour(&self) -> hevc::Colour {
        self.colour
    }

    fn codec(&self) -> Codec {
        self.codec
    }

    fn hevc(&self) -> Option<(hevc::Config, hevc::Colour)> {
        self.hevc.clone()
    }

    fn next(&mut self) -> Option<Packet> {
        while self.file.next_frame(&mut self.frame).ok()? {
            if self.frame.track != self.track {
                continue;
            }
            let pts = self.frame.timestamp as f64 * self.scale;
            // (A block's own duration only: the track's default is the
            // frame rate's, and a still stretch of a recording is longer.)
            let dur = self.frame.duration.map(|d| d as f64 * self.scale);
            if self.codec != Codec::H264 {
                return Some(Packet { pts, data: std::mem::take(&mut self.frame.data), dur });
            }
            let mut data = Vec::with_capacity(self.frame.data.len() + 64);
            if self.fresh || self.key() {
                data.extend_from_slice(&self.sets);
                self.fresh = false;
            }
            media::avcc_to_annexb(&self.frame.data, self.nal_len, &mut data);
            return Some(Packet { pts, data, dur });
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
                if self.key() {
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

    /// A test's scratch folder, removed however the test ends (a failing
    /// one too).
    struct Gone(std::path::PathBuf);

    impl Drop for Gone {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// What a seek costs at 4K (H.264, OpenH264's key frames two seconds
    /// apart): to just before the second key frame, so every picture
    /// before it is decoded (and, held in case it's the one showing,
    /// kept). `cargo test --release -p olo-preview seek_cost -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn seek_cost() {
        use olo_video::{H264, Mp4, Order};
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-preview-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _gone = Gone(dir.clone());
        let path = dir.join("4k.mp4");
        let (w, h) = (3840u32, 2160u32);
        let mut enc = H264::new(w, h, 30).unwrap();
        let mut mp4 = Mp4::create(&path, w, h, 1000).unwrap();
        // A bar sweeping across, so every picture differs.
        for i in 0..62u32 {
            let px: Vec<u8> = (0..w * h).flat_map(|p| if (p % w) / 64 == i { [255, 255, 255, 255] } else { [(p % 251) as u8, (p / w % 241) as u8, 90, 255] }).collect();
            let (bits, key) = enc.encode(&px, w as usize * 4, Order::Bgra).unwrap();
            mp4.add(&bits, key, 33).unwrap();
        }
        mp4.finish().unwrap();
        let mut ms: Vec<f64> = (0..5)
            .map(|_| {
                let t = Instant::now();
                picture_at(&Input::file(&path), 1.95).unwrap();
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        ms.sort_by(f64::total_cmp);
        println!("seek to 1.95 s of 4K H.264 (59 pictures decoded): {:.0} ms (median of 5; {:.0}–{:.0})", ms[2], ms[0], ms[4]);
    }

    /// A still stretch of a screen recording is one picture, seconds
    /// long: a seek into it shows it (not the next one), and one past the
    /// last picture's start shows the last.
    #[test]
    fn seeks_into_a_long_picture() {
        use olo_video::{H264, Mp4, Order};
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-preview-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _gone = Gone(dir.clone());
        let path = dir.join("still.mp4");
        let (w, h) = (64u32, 48u32);
        let mut enc = H264::new(w, h, 30).unwrap();
        let mut mp4 = Mp4::create(&path, w, h, 1000).unwrap();
        // Grey 40 for 100 ms, 140 for 3 s, 240 for half a second.
        for (level, ms) in [(40u8, 100u32), (140, 3000), (240, 500)] {
            let px = vec![level; (w * h * 4) as usize];
            let (bits, key) = enc.encode(&px, w as usize * 4, Order::Bgra).unwrap();
            mp4.add(&bits, key, ms).unwrap();
        }
        mp4.finish().unwrap();
        let level = |at: f64| {
            let image = picture_at(&Input::file(&path), at).unwrap();
            image.rgba[(image.rgba.len() / 2) & !3]
        };
        let shown: Vec<(f64, u8)> = [0.0, 0.05, 0.1, 1.6, 3.09, 3.1, 3.4, 5.0].into_iter().map(|at| (at, level(at))).collect();
        let want = [40, 40, 140, 140, 140, 240, 240, 240];
        for ((at, got), want) in shown.iter().zip(want) {
            assert!((i32::from(*got) - want).abs() < 12, "at {at} s: {got}, not {want} ({shown:?})");
        }
    }

    /// `OLO_SAMPLES=dir cargo test -p olo-preview video -- --ignored`
    /// with 8-second 30 fps H.264 clips in `dir`: test.mp4 / test.mkv (one
    /// key frame), gop.mp4 / gop.mkv (one a second) and av1.webm (AV1 with
    /// reordering, a key frame a second), bframes.mp4 (see b_frames) and
    /// vp9.webm (libvpx's, hidden alt-ref frames in superframes, a key
    /// frame a second).
    #[test]
    #[ignore]
    fn seeks_to_the_frame() {
        let dir = std::path::PathBuf::from(std::env::var("OLO_SAMPLES").expect("OLO_SAMPLES"));
        for name in names(&["test.mp4", "test.mkv", "gop.mp4", "gop.mkv", "av1.webm", "bframes.mp4", "vp9.webm"]) {
            for target in [0.0, 5.0, 2.5] {
                seeks(&dir.join(&name), target);
            }
        }
    }

    /// The samples a test goes through: those named in `OLO_ONLY`
    /// (separated by commas), else all of `all`.
    fn names(all: &[&str]) -> Vec<String> {
        std::env::var("OLO_ONLY").map_or_else(|_| all.iter().map(|n| n.to_string()).collect(), |n| n.split(',').map(str::to_owned).collect())
    }

    /// The first pictures after seeking `path` to `target` are the one
    /// showing then and those after it, in order.
    fn seeks(path: &std::path::Path, target: f64) {
        let name = path.display();
        let shared = Arc::new((Mutex::new(Shared { seek: Some(target), ..Default::default() }), Condvar::new()));
        let s = shared.clone();
        let p = path.to_owned();
        let worker = std::thread::spawn(move || decode(&Input::file(&p), &s));
        let (m, cv) = &*shared;
        let mut sh = m.lock().unwrap();
        // (Three, or as many as it queues: two of 1080p's, by their bytes.)
        let want = |sh: &Shared| 3.min(if sh.ahead == 0 { 3 } else { sh.ahead });
        while sh.queue.len() < want(&sh) && !sh.eof {
            sh = cv.wait_timeout(sh, Duration::from_millis(50)).unwrap().0;
        }
        let pts: Vec<f64> = sh.queue.iter().map(|f| f.pts).collect();
        let wanted = want(&sh);
        sh.stop = true;
        drop(sh);
        cv.notify_all();
        worker.join().unwrap().unwrap();
        assert!(pts.len() >= wanted, "{name} @ {target}: {pts:?}");
        // The first picture is the one showing at the target.
        assert!((pts[0] - target).abs() < 0.05, "{name} @ {target}: {pts:?}");
        assert!(pts.windows(2).all(|w| w[1] > w[0]), "{name}: in order {pts:?}");
    }

    /// AV1 in MPEG-4 (which the mp4 crate doesn't know): `OLO_SAMPLES=dir`
    /// with av1.mp4, 4 seconds at 30 fps, a key frame a second (ffmpeg's
    /// testsrc2 through libsvtav1); its poster frame and facts too.
    #[test]
    #[ignore]
    fn av1_in_mp4() {
        let dir = std::path::PathBuf::from(std::env::var("OLO_SAMPLES").expect("OLO_SAMPLES"));
        let path = dir.join("av1.mp4");
        for target in [0.0, 2.5] {
            seeks(&path, target);
        }
        let Ok(crate::load::Update::Body(crate::load::Body::Media(m))) = media::video(&path, &Arc::new(std::sync::atomic::AtomicBool::new(false))) else { panic!("no facts") };
        assert!(m.playable && m.picture.is_some(), "{:?}", m.rows);
        assert!(m.note.starts_with("AV1 video, 640 × 360"), "{}", m.note);
    }

    /// VP9 in MPEG-4 and 10-bit VP9 in WebM: `OLO_SAMPLES=dir` with
    /// vp9.mp4 (4 s at 30 fps, hidden alt-ref frames, a key frame a
    /// second) and vp9-10bit.webm (profile 2, 4 s); every picture comes
    /// out, in order, and each file's poster and facts.
    #[test]
    #[ignore]
    fn vp9_plays() {
        let dir = std::path::PathBuf::from(std::env::var("OLO_SAMPLES").expect("OLO_SAMPLES"));
        for target in [0.0, 2.5] {
            seeks(&dir.join("vp9.mp4"), target);
        }
        for name in ["vp9.mp4", "vp9-10bit.webm"] {
            let path = dir.join(name);
            assert_eq!(all_pictures(&path).len(), 120, "{name}: every picture");
            let Ok(crate::load::Update::Body(crate::load::Body::Media(m))) = media::video(&path, &Arc::new(std::sync::atomic::AtomicBool::new(false))) else { panic!("no facts") };
            assert!(m.playable && m.picture.is_some(), "{name}: {:?}", m.rows);
            assert!(m.note.starts_with("VP9 video, 640 × 360"), "{name}: {}", m.note);
        }
    }

    /// HEVC in MPEG-4 (`hev1` and `hvc1`) and Matroska: `OLO_SAMPLES=dir`
    /// with hevc.mp4, hvc1.mp4 and hevc.mkv (any 1080p HEVC with B-frames,
    /// 10 s at 30 fps: the Jellyfish and Big Buck Bunny test clips, the
    /// last two remuxed with `ffmpeg -c copy`, `-tag:v hvc1` for the
    /// second) and hevc-10bit.mp4 (JCT-VC's WPP_D_ericsson_MAIN10_2 put in
    /// an mp4 likewise: Main 10, 48 pictures); every picture comes out, in
    /// order, seeks land, and each file's poster and facts.
    #[test]
    #[ignore]
    fn hevc_plays() {
        let dir = std::path::PathBuf::from(std::env::var("OLO_SAMPLES").expect("OLO_SAMPLES"));
        for name in names(&["hevc.mp4", "hvc1.mp4", "hevc.mkv", "hevc-10bit.mp4"]) {
            let path = dir.join(&name);
            let want = if name == "hevc-10bit.mp4" { 48 } else { 300 };
            assert_eq!(all_pictures(&path).len(), want, "{name}: every picture");
            if want == 300 {
                for target in [0.0, 5.0, 2.5] {
                    seeks(&path, target);
                }
            }
            let Ok(crate::load::Update::Body(crate::load::Body::Media(m))) = media::video(&path, &Arc::new(std::sync::atomic::AtomicBool::new(false))) else { panic!("no facts") };
            assert!(m.playable && m.picture.is_some(), "{name}: {:?}", m.rows);
            assert!(m.note.starts_with("HEVC video"), "{name}: {}", m.note);
        }
    }

    /// The times of every picture `path` plays, in the order they come.
    fn all_pictures(path: &std::path::Path) -> Vec<f64> {
        let shared = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
        let s = shared.clone();
        let p = path.to_owned();
        let worker = std::thread::spawn(move || decode(&Input::file(&p), &s));
        let (m, cv) = &*shared;
        let mut pts = Vec::new();
        loop {
            let mut sh = m.lock().unwrap();
            pts.extend(sh.queue.drain(..).map(|f| f.pts));
            cv.notify_all();
            if sh.eof && sh.queue.is_empty() {
                sh.stop = true;
                break;
            }
            sh = cv.wait_timeout(sh, Duration::from_millis(5)).unwrap().0;
            drop(sh);
        }
        cv.notify_all();
        worker.join().unwrap().unwrap();
        assert!(pts.windows(2).all(|w| w[1] > w[0]), "{}: in order {pts:?}", path.display());
        pts
    }

    /// Every picture of bframes.mp4 (H.264 High with B-frames and implicit
    /// weighted bi-prediction, as x264 makes by default) comes out, in
    /// display order.
    #[test]
    #[ignore]
    fn b_frames() {
        let path = std::path::PathBuf::from(std::env::var("OLO_SAMPLES").expect("OLO_SAMPLES")).join("bframes.mp4");
        let mut demux = Mp4::open(&Input::file(&path)).unwrap();
        let mut packets = 0;
        while demux.next().is_some() {
            packets += 1;
        }
        assert_eq!(all_pictures(&path).len(), packets, "every picture decodes");
    }

    /// Queued pictures stay compact, and convert as OpenH264 would.
    #[test]
    fn compact_pictures_match_openh264() {
        use openh264::formats::{RgbSliceU8, YUVBuffer};
        let (w, h) = (96usize, 64usize);
        let rgb: Vec<u8> = (0..w * h).flat_map(|i| [(i % w * 255 / w) as u8, (i / w * 255 / h) as u8, 200]).collect();
        let mut enc = openh264::encoder::Encoder::new().unwrap();
        let stream = enc.encode(&YUVBuffer::from_rgb_source(RgbSliceU8::new(&rgb, (w, h)))).unwrap().to_vec();
        let mut dec = openh264::decoder::Decoder::new().unwrap();
        let yuv = dec.decode(&stream).unwrap().expect("a picture");
        let mut theirs = vec![0u8; w * h * 4];
        yuv.write_rgba8(&mut theirs);
        let planar = Planar::from(&yuv);
        assert_eq!(Pic::H264(Arc::new(Planar::from(&yuv))).bytes(), w * h * 3 / 2);
        let ours = planar.image();
        assert_eq!(ours.size, [w, h]);
        let worst = ours.rgba.as_chunks::<4>().0.iter().zip(theirs.as_chunks::<4>().0.iter()).map(|(a, b)| (0..3).map(|i| (a[i] as i32 - b[i] as i32).abs()).max().unwrap()).max().unwrap();
        assert!(worst <= 2, "differs from OpenH264's conversion by {worst}");
        // Big pictures get fewer frames ahead, never under two.
        assert_eq!((QUEUE_BYTES / (2920 * 2000 * 3 / 2)).clamp(2, AHEAD), 2);
        assert_eq!((QUEUE_BYTES / (1280 * 720 * 3 / 2)).clamp(2, AHEAD), AHEAD);
    }

    /// Pictures decoded (and made RGBA) per second from av1-1080.mkv,
    /// vp9-1080.webm and hevc.mp4 (1080p AV1, VP9 and HEVC): needs to stay
    /// well above 30 in release builds.
    #[test]
    #[ignore]
    fn keeps_up() {
        let dir = std::path::PathBuf::from(std::env::var("OLO_SAMPLES").expect("OLO_SAMPLES"));
        for name in names(&["av1-1080.mkv", "vp9-1080.webm", "hevc.mp4", "gop.mp4", "av1.mp4"]) {
            let path = dir.join(&name);
            let shared = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
            let s = shared.clone();
            let start = Instant::now();
            let worker = std::thread::spawn(move || decode(&Input::file(&path), &s));
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
