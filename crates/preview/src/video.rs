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

/// Frames decoded ahead (a tenth of a second at 60 fps)...
const AHEAD: usize = 6;
/// ...unless that would hold more than this, when larger pictures get
/// fewer (never under two). A 4K picture as planes is 12 MB: six of
/// them (at 24 MB it had two, 33 ms at 60 fps, and any hiccup a
/// decoder had dropped pictures: NVDEC's H.264 150 a minute).
const QUEUE_BYTES: usize = 80 << 20;

/// How long the clock waits for a late decoder (with no sound to follow)
/// before it goes on without it.
const HOLD: Duration = Duration::from_millis(200);

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
    /// Straight BGRA bytes, `size` pixels (what pages take); `p3`: in
    /// Display P3 (a wide video's), else sRGB.
    Bgra {
        size: [usize; 2],
        data: Arc<Vec<u8>>,
        p3: bool,
    },
    /// An HDR picture as HDR10 (PQ in BT.2020), RGBA half floats, for a
    /// window drawing in it ([`Video::draw_hdr`]).
    Deep {
        size: [usize; 2],
        data: Arc<Vec<u16>>,
    },
    /// Its planes as decoded (4:2:0 Y'CbCr), for a window's GPU to make
    /// RGB of (`Window::paint_yuv`): an SDR picture whose viewer asked
    /// (`Options::yuv`).
    #[cfg(feature = "gpu")]
    Yuv(Arc<olo_yuv::YuvFrame>),
}

impl Picture {
    /// As RGBA (converting a BGRA one).
    pub fn into_image(self) -> Image {
        match self {
            Picture::Rgba(i) => i,
            Picture::Bgra { size, data, p3 } => Arc::new(Bitmap { p3, ..Bitmap::from_bgra_unmultiplied(size, &data) }),
            Picture::Deep { size, data } => Arc::new(Bitmap::from_rgba_unmultiplied(size, &deep_to_srgb(&data))),
            #[cfg(feature = "gpu")]
            Picture::Yuv(f) => Arc::new(Bitmap::from_rgba_unmultiplied([f.width as usize, f.height as usize], &yuv_to_rgba(&f))),
        }
    }

    /// Its numbers are Display P3's (a wide video's: BT.2020, P3).
    pub fn p3(&self) -> bool {
        match self {
            Picture::Rgba(i) => i.p3,
            Picture::Bgra { p3, .. } => *p3,
            Picture::Deep { .. } => false,
            #[cfg(feature = "gpu")]
            Picture::Yuv(_) => false,
        }
    }
}

/// A YUV picture as RGBA (a still of it, a copy: rare; the window's GPU
/// draws it otherwise).
#[cfg(feature = "gpu")]
fn yuv_to_rgba(f: &olo_yuv::YuvFrame) -> Vec<u8> {
    let m = f.rgb_matrix();
    let (w, h) = (f.width as usize, f.height as usize);
    // (Wherever its planes are: rows at their pitch.)
    let ((luma, pitch_y), (chroma, pitch_uv)) = (f.luma(), f.chroma());
    let bytes = if f.deep { 2 } else { 1 };
    let sample = |plane: &[u8], at: usize| if f.deep { f32::from(u16::from_le_bytes([plane[at], plane[at + 1]])) / 65535.0 } else { f32::from(plane[at]) / 255.0 };
    let mut out = vec![255u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let c = (y / 2) * pitch_uv + (x / 2) * 2 * bytes;
            let v = [sample(luma, y * pitch_y + x * bytes), sample(chroma, c), sample(chroma, c + bytes), 1.0];
            for (k, o) in out[(y * w + x) * 4..][..3].iter_mut().enumerate() {
                let r: f32 = (0..4).map(|col| m[col][k] * v[col]).sum();
                *o = (r.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
    out
}

/// An HDR10 picture's half floats (PQ, BT.2020) as sRGB RGBA, its light
/// to SDR's white (what's brighter clipped): for whoever wants an
/// ordinary picture of one (a still, a copy).
fn deep_to_srgb(rgba: &[u16]) -> Vec<u8> {
    let unhalf = |h: u16| {
        let (exp, mant) = (u32::from(h >> 10) & 0x1F, u32::from(h & 0x3FF));
        if exp == 0 { mant as f32 / 16_777_216.0 } else { f32::from_bits(((exp + 112) << 23) | (mant << 13)) }
    };
    let m = olo_color::Primaries::BT2020.to(&olo_color::Primaries::SRGB).0;
    rgba.chunks_exact(4)
        .flat_map(|p| {
            let light = [0, 1, 2].map(|i| (olo_color::pq_decode(f64::from(unhalf(p[i]))) * 10_000.0 / olo_color::SDR_WHITE_NITS).min(1.0));
            let srgb = [0, 1, 2].map(|r| {
                let v = m[r][0] * light[0] + m[r][1] * light[1] + m[r][2] * light[2];
                (olo_color::Transfer::Srgb.encode(v.clamp(0.0, 1.0)) * 255.0).round() as u8
            });
            [srgb[0], srgb[1], srgb[2], 255]
        })
        .collect()
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
            Pic::Planes(f) => f.bytes(),
            Pic::Ready(Picture::Rgba(i)) => i.rgba.len(),
            Pic::Ready(Picture::Bgra { data, .. }) => data.len(),
            Pic::Ready(Picture::Deep { data, .. }) => data.len() * 2,
            #[cfg(feature = "gpu")]
            Pic::Ready(Picture::Yuv(f)) => f.bytes(),
        }
    }

    /// Its planes for the GPU (`Picture::Yuv`, of stream `stream`), if it
    /// can be drawn so: 4:2:0, and not an HDR picture to be drawn as HDR10
    /// (`hdr`), nor one whose colours need converting.
    #[cfg(feature = "gpu")]
    fn yuv(&self, stream: u64, hdr: bool) -> Option<olo_yuv::YuvFrame> {
        use olo_yuv::{YuvFrame, YuvMatrix};
        match self {
            // (OpenH264's: limited-range BT.601, as `Planar::convert`.)
            Pic::H264(p) => {
                let mut uv = Vec::with_capacity(p.u.len() * 2);
                for (&u, &v) in p.u.iter().zip(&p.v) {
                    uv.push(u);
                    uv.push(v);
                }
                Some(YuvFrame { stream, width: p.width as u32, height: p.height as u32, deep: false, y: p.y.clone(), uv, matrix: YuvMatrix::Bt601, full_range: false, memory: None })
            }
            Pic::Planes(f) if !(hdr && f.is_hdr()) => {
                let y = f.yuv420()?;
                let matrix = match y.matrix {
                    709 => YuvMatrix::Bt709,
                    2020 => YuvMatrix::Bt2020,
                    _ => YuvMatrix::Bt601,
                };
                Some(YuvFrame { stream, width: y.width, height: y.height, deep: y.deep, y: y.y, uv: y.uv, matrix, full_range: y.full_range, memory: None })
            }
            _ => None,
        }
    }

    /// As a picture to show; `hdr`, an HDR picture as HDR10
    /// (`Picture::Deep`); `yuv`, as planes for the GPU where it can be
    /// (`Picture::Yuv`, of that stream).
    fn picture(&self, bgra: bool, hdr: bool, yuv: Option<u64>) -> Picture {
        #[cfg(feature = "gpu")]
        if let Some(f) = yuv.and_then(|stream| self.yuv(stream, hdr)) {
            return Picture::Yuv(Arc::new(f));
        }
        #[cfg(not(feature = "gpu"))]
        let _ = yuv;
        match self {
            Pic::Planes(f) if hdr && f.is_hdr() => match f.to_pq_half() {
                Some(data) => Picture::Deep { size: [f.width as usize, f.height as usize], data: Arc::new(data) },
                None => Pic::Planes(f.clone()).picture(bgra, false, None),
            },
            Pic::H264(p) if bgra => Picture::Bgra { size: [p.width, p.height], data: Arc::new(p.bgra()), p3: false },
            Pic::H264(p) => Picture::Rgba(p.image()),
            // (A wide video's in Display P3: drawn so in a window drawing
            // in it, converted in one that isn't.)
            Pic::Planes(f) if bgra => {
                let (mut data, p3) = f.to_rgba_wide();
                for px in data.as_chunks_mut::<4>().0 {
                    px.swap(0, 2);
                }
                Picture::Bgra { size: [f.width as usize, f.height as usize], data: Arc::new(data), p3 }
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
    /// Pictures shown, those passed over (already late when their turn
    /// came: dropped), and those shown late (a frame or more after
    /// their time): `Video::playback`.
    shown: u64,
    skipped: u64,
    late: u64,
    /// They were logged (played to the end).
    told: bool,
    queue: VecDeque<Frame>,
    /// Frames to keep ahead, from the pictures' size.
    ahead: usize,
    /// Make pictures BGRA (for pages) rather than RGBA.
    bgra: bool,
    /// Make them Y'CbCr planes where they can be (`Picture::Yuv`), of this
    /// stream.
    yuv: Option<u64>,
    /// What's asked to decode, and what does ("NVIDIA's GPU (NVDEC)"),
    /// and which GPUs could.
    device: Device,
    decoding: &'static str,
    decoders: Decoders,
    /// Make an HDR picture HDR10's (`Video::draw_hdr`: a window drawing
    /// in HDR10 shows it), rather than mapped for SDR.
    hdr: bool,
    /// A picture of the stream so far was HDR (`Video::is_hdr`).
    stream_hdr: bool,
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
    /// Waiting for the decoder since (`HOLD`).
    starving: Option<Instant>,
    /// Media seconds a second (1.5: one and a half times as fast).
    rate: f64,
}

impl Clock {
    fn now(&self) -> f64 {
        self.at + self.since.map_or(0.0, |s| s.elapsed().as_secs_f64() * self.rate)
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
        Video::start_with(input, duration, Options { playing: true, picture: true, sound: true, bgra: false, yuv: false, device: Device::Processor })
    }

    /// Start `input` as `opts` say: playing or paused (showing its first
    /// picture), with pictures and sound or only one of them.
    pub fn start_with(input: &Input, duration: f64, opts: Options) -> Video {
        #[cfg(feature = "gpu")]
        let yuv = opts.yuv.then(olo_yuv::YuvFrame::new_stream);
        #[cfg(not(feature = "gpu"))]
        let yuv = None;
        let shared = Arc::new((Mutex::new(Shared { eof: !opts.picture, bgra: opts.bgra, yuv, device: opts.device, ..Default::default() }), Condvar::new()));
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
            clock: Mutex::new(Clock { at: 0.0, since: playing.then(Instant::now), heard_ms: 0, finished: false, show_next: opts.picture, starving: None, rate: 1.0 }),
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

    /// Play at `rate` (`audio::MIN_RATE` to `MAX_RATE`: 0.25 to 2), the
    /// sound's pitch kept; from where it is now. A decoder that can't
    /// keep up at 2× passes pictures over, as at any speed.
    pub fn set_rate(&self, rate: f64) {
        let rate = rate.clamp(audio::MIN_RATE, audio::MAX_RATE);
        {
            let mut c = self.clock.lock().unwrap();
            c.at = c.now();
            if c.since.is_some() {
                c.since = Some(Instant::now());
            }
            c.rate = rate;
        }
        if let Some(a) = &self.audio {
            a.set_rate(rate);
        }
    }

    /// How fast it plays (1: as recorded).
    pub fn rate(&self) -> f64 {
        self.clock.lock().unwrap().rate
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

    /// What decodes its pictures now ("NVIDIA's GPU (NVDEC)", "the
    /// processor"), once it has started.
    pub fn decoding(&self) -> &'static str {
        self.shared.0.lock().unwrap().decoding
    }

    /// How smoothly it has played so far.
    pub fn playback(&self) -> Playback {
        let sh = self.shared.0.lock().unwrap();
        Playback { shown: sh.shown, dropped: sh.skipped, late: sh.late }
    }

    /// Which GPUs here decode its stream, once it has started.
    pub fn decoders(&self) -> Decoders {
        self.shared.0.lock().unwrap().decoders
    }

    /// Whether the stream's pictures are HDR (PQ or HLG), as far as it
    /// has been seen.
    pub fn is_hdr(&self) -> bool {
        self.shared.0.lock().unwrap().stream_hdr
    }

    /// HDR pictures drawn as HDR10 (`Picture::Deep`) from now, for a
    /// window drawing in it, or mapped for SDR as they otherwise are.
    pub fn draw_hdr(&self, on: bool) {
        self.shared.0.lock().unwrap().hdr = on;
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
        let (m, cv) = &*self.shared;
        let mut sh = m.lock().unwrap();
        // Waiting for the decoder (starting, or a stall), with no sound
        // to keep up with: the clock waits too, for a while, so the next
        // picture is shown late rather than those after it passed over;
        // a decoder that can't keep up at all is caught up with by
        // passing pictures over, not played slower.
        let silent = c.since.is_some() && self.sound().is_none();
        let starved = silent && sh.queue.is_empty() && !sh.eof && sh.seek.is_none();
        if starved && c.starving.get_or_insert_with(Instant::now).elapsed() < HOLD {
            c.at = c.now();
            c.since = Some(Instant::now());
        }
        // The first picture: the clock starts with it (the decoder's
        // starting took its time).
        if silent
            && sh.shown == 0
            && let Some(f) = sh.queue.front()
            && c.now() > f.pts
        {
            c.at = f.pts;
            c.since = Some(Instant::now());
        }
        let now = c.now();
        let mut due = None;
        let mut passed = 0;
        let stepping = c.show_next;
        while let Some(f) = sh.queue.front() {
            if f.pts > now + 0.004 && !c.show_next {
                break;
            }
            passed += u64::from(due.is_some());
            due = sh.queue.pop_front();
            if c.show_next {
                c.show_next = false;
                break;
            }
        }
        if let Some(f) = &due {
            cv.notify_all();
            c.starving = None;
            sh.shown += 1;
            if !stepping && c.since.is_some() {
                sh.skipped += passed;
                sh.late += u64::from(now - f.pts > LATE);
            }
        }
        // (Its time in the media's seconds: sooner played faster.)
        let next = sh.queue.front().map(|f| Duration::from_secs_f64(((f.pts - now) / c.rate).max(0.004)));
        let ended = if self.picture {
            // The pictures ran out (the sound ends by itself).
            sh.eof && sh.queue.is_empty() && sh.seek.is_none()
        } else {
            now >= self.duration || self.sound().is_some_and(|a| a.state.finished.load(Ordering::Relaxed))
        };
        if ended && c.since.is_some() {
            if !sh.told && sh.shown > 0 {
                sh.told = true;
                tracing::info!("video: played to its end: {} pictures shown, {} dropped, {} late; decoded on {}", sh.shown, sh.skipped, sh.late, sh.decoding);
            }
            c.at = self.duration;
            c.since = None;
            c.finished = true;
        }
        // Playing, or waiting for the picture a seek asked for.
        let idle = if self.picture { Duration::from_millis(15) } else { Duration::from_millis(100) };
        let next = if c.since.is_some() || (c.show_next && !sh.eof) { Some(next.unwrap_or(idle)) } else { None };
        let (hdr, yuv) = (sh.hdr, sh.yuv);
        // (A picture made here, the worker behind: its stream known HDR
        // all the same.)
        sh.stream_hdr |= due.as_ref().is_some_and(|f| matches!(&f.pic, Pic::Planes(p) if p.is_hdr()));
        drop(sh);
        drop(c);
        // Ready unless the worker fell behind.
        (due.map(|f| f.pic.ready().cloned().unwrap_or_else(|| f.pic.picture(self.bgra, hdr, yuv))), next)
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        let (m, cv) = &*self.shared;
        let mut sh = m.lock().unwrap();
        sh.stop = true;
        if sh.shown > 0 && !sh.told {
            tracing::info!("video: {} pictures shown, {} dropped, {} late; decoded on {}", sh.shown, sh.skipped, sh.late, sh.decoding);
        }
        drop(sh);
        cv.notify_all();
    }
}

/// How smoothly a video has played: pictures shown, dropped (passed over,
/// already late when their turn came) and shown late (a frame or more
/// after their time).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Playback {
    pub shown: u64,
    pub dropped: u64,
    pub late: u64,
}

/// A picture shown this long after its time was late (more than a
/// frame at 60 a second, with the display's own frame of slack).
const LATE: f64 = 0.025;

/// How [`Video::start_with`] starts.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub playing: bool,
    pub picture: bool,
    pub sound: bool,
    /// Pictures as BGRA bytes ([`Video::tick_picture`]).
    pub bgra: bool,
    /// Pictures as Y'CbCr planes for a window's GPU where they can be
    /// (`Picture::Yuv`; a build with the `gpu` feature): a 4K picture is
    /// 12 MB rather than 33, and nothing converts it on the CPU.
    pub yuv: bool,
    /// What decodes (a GPU only with `yuv`: its pictures are planes).
    pub device: Device,
}

/// What decodes a video's pictures (the Viewer's View ▸ Decode With;
/// Preferences ▸ Video's default).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Device {
    /// The person's choice (Preferences ▸ Power ▸ Graphics ▸ Video Playback), read
    /// by the player's worker.
    #[default]
    Default,
    /// The built-in GPU where its driver decodes the video (Intel's:
    /// AV1 and VP9 with Fedora's driver, H.264 and HEVC too with the
    /// full one), else NVIDIA's where there is one, on battery too (Ben,
    /// 2026-11-06: its decoder with the GPU in its power-saving preset
    /// costs less than the processor's cores), else the processor.
    Automatic,
    /// NVIDIA's GPU (NVDEC).
    Nvidia,
    /// The built-in GPU (VA-API).
    Intel,
    /// The processor (OpenH264, rust_h265, filmcraft-vp9, rav1d).
    Processor,
}

/// Which GPUs here decode a stream (as planes for a window's GPU).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Decoders {
    /// NVIDIA's (NVDEC: H.264, HEVC, VP9, AV1).
    pub nvidia: bool,
    /// Intel's or AMD's (VA-API: AV1 and VP9 with Fedora's drivers).
    pub intel: bool,
}

impl Decoders {
    #[cfg(feature = "gpu")]
    fn of(planes: bool, codec: Codec) -> Decoders {
        Decoders { nvidia: planes && olo_video::nvdec::available(), intel: planes && olo_vaapi::decode::available(va_codec(codec)) }
    }

    #[cfg(not(feature = "gpu"))]
    fn of(_: bool, _: Codec) -> Decoders {
        Decoders::default()
    }
}

impl Device {
    pub fn parse(s: &str) -> Option<Device> {
        match s {
            "auto" | "automatic" => Some(Device::Automatic),
            "nvidia" => Some(Device::Nvidia),
            "intel" | "vaapi" => Some(Device::Intel),
            "cpu" | "processor" => Some(Device::Processor),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Device::Default => "default",
            Device::Automatic => "auto",
            Device::Nvidia => "nvidia",
            Device::Intel => "intel",
            Device::Processor => "cpu",
        }
    }
}

/// Make the picture at the front of the queue ready to show (outside the
/// lock: the UI may be waiting on it).
fn ripen(m: &Mutex<Shared>) {
    let (pts, pic, bgra, hdr, yuv) = {
        let sh = m.lock().unwrap();
        match sh.queue.front() {
            Some(f) if f.pic.ready().is_none() => (f.pts, f.pic.clone(), sh.bgra, sh.hdr, sh.yuv),
            _ => return,
        }
    };
    let image = pic.picture(bgra, hdr, yuv);
    let mut sh = m.lock().unwrap();
    sh.stream_hdr |= matches!(&pic, Pic::Planes(f) if f.is_hdr());
    if let Some(f) = sh.queue.front_mut()
        && f.pts == pts
        && f.pic.ready().is_none()
    {
        f.pic = Pic::Ready(image);
    }
}

fn av1_image(f: &av1::Frame) -> Image {
    let (rgba, p3) = f.to_rgba_wide();
    Arc::new(Bitmap { p3, ..Bitmap::from_rgba_unmultiplied([f.width as usize, f.height as usize], &rgba) })
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
    let first = sh.queue.front().map(|f| f.pic.picture(false, false, None));
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
    /// NVIDIA's GPU (pictures as planes for the window's GPU).
    #[cfg(feature = "gpu")]
    Nv(Box<Nv>),
    /// Intel's or AMD's GPU through VA-API (as planes too).
    #[cfg(feature = "gpu")]
    Va(Box<Va>),
}

/// VA-API's decoder, and what it needs of the stream, as NVDEC does.
#[cfg(feature = "gpu")]
struct Va {
    dec: olo_vaapi::decode::Decoder,
    hevc: Option<hevc::Config>,
    fresh: bool,
}

/// The codec as VA-API's decoder names it.
#[cfg(feature = "gpu")]
fn va_codec(codec: Codec) -> olo_vaapi::decode::Codec {
    match codec {
        Codec::Av1 => olo_vaapi::decode::Codec::Av1,
        Codec::Vp9 => olo_vaapi::decode::Codec::Vp9,
        Codec::H264 => olo_vaapi::decode::Codec::H264,
        Codec::Hevc => olo_vaapi::decode::Codec::Hevc,
    }
}

/// A sample as a GPU's parser takes it: HEVC's NAL units Annex B (start
/// codes, not lengths), with its parameter sets first after a start or
/// a seek (`fresh`); others as they are.
#[cfg(feature = "gpu")]
fn annex_b<'a>(hevc: Option<&hevc::Config>, fresh: &mut bool, data: &'a [u8]) -> std::borrow::Cow<'a, [u8]> {
    let Some(cfg) = hevc else { return std::borrow::Cow::Borrowed(data) };
    let mut out = Vec::with_capacity(data.len() + 128);
    if std::mem::take(fresh) {
        for set in &cfg.sets {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(set);
        }
    }
    media::avcc_to_annexb(data, usize::from(cfg.nal_len), &mut out);
    std::borrow::Cow::Owned(out)
}

/// NVDEC, and what it needs of the stream: HEVC's parameter sets and its
/// NAL units' length size (its samples made Annex B), and whether the
/// next packet starts a stream (after a seek: the sets first).
#[cfg(feature = "gpu")]
struct Nv {
    dec: olo_video::nvdec::Nvdec,
    hevc: Option<hevc::Config>,
    fresh: bool,
}

#[cfg(feature = "gpu")]
impl Nv {
    /// `data` as NVDEC's parser takes it.
    fn packet<'a>(&mut self, data: &'a [u8]) -> std::borrow::Cow<'a, [u8]> {
        annex_b(self.hevc.as_ref(), &mut self.fresh, data)
    }
}

/// The names said of what decodes.
const ON_NVIDIA: &str = "NVIDIA's GPU (NVDEC)";
const ON_INTEL: &str = "Intel's GPU (VA-API)";
const ON_OTHER_GPU: &str = "the built-in GPU (VA-API)";
const ON_PROCESSOR: &str = "the processor";

/// A GPU decoder's pictures left in the memory they're copied into, for
/// the window's GPU to copy from as they are (`YuvMemory`); the processor
/// copies them out into planes with `OLO_VIDEO_HELD=0`, to compare.
#[cfg(feature = "gpu")]
fn held() -> bool {
    std::env::var_os("OLO_VIDEO_HELD").is_none_or(|v| v != "0")
}

/// A picture as a GPU decoded it, for the window to draw: its matrix
/// from H.273's number (else by its height, as players guess).
#[cfg(feature = "gpu")]
#[allow(clippy::too_many_arguments)]
fn gpu_picture(stream: u64, width: u32, height: u32, deep: bool, (y, uv, memory): (Vec<u8>, Vec<u8>, Option<olo_yuv::YuvMemory>), matrix: u8, full_range: bool) -> olo_yuv::YuvFrame {
    use olo_yuv::YuvMatrix;
    let matrix = match matrix {
        1 => YuvMatrix::Bt709,
        9 | 10 => YuvMatrix::Bt2020,
        5 | 6 => YuvMatrix::Bt601,
        _ if height >= 720 => YuvMatrix::Bt709,
        _ => YuvMatrix::Bt601,
    };
    olo_yuv::YuvFrame { stream, width, height, deep, y, uv, matrix, full_range, memory }
}

impl Decoder {
    /// A decoder on `device` (NVIDIA's GPU only for pictures as planes,
    /// `planes`), else on the processor; and what it is.
    fn on(device: Device, planes: bool, codec: Codec, demux: &dyn Demux) -> Result<(Decoder, &'static str), String> {
        #[cfg(feature = "gpu")]
        {
            // (The person's choice, read here: off the window's thread.)
            let video = olo_kit_base::settings::current().map(|s| s.video.clone()).unwrap_or_default();
            let device = match device {
                Device::Default => Device::parse(&video.decoder).filter(|d| *d != Device::Default).unwrap_or(Device::Automatic),
                d => d,
            };
            // Intel's (or AMD's) GPU, for what its driver decodes: AV1
            // and VP9 with Fedora's drivers, H.264 and HEVC too with the
            // full one (Olo Doctor installs it).
            let va = Some(va_codec(codec)).filter(|c| planes && olo_vaapi::decode::available(*c));
            // Automatic: the built-in GPU where it decodes this (already
            // awake, and the screen's), else NVIDIA's, on battery too.
            let (intel, nvidia) = match device {
                Device::Nvidia => (false, true),
                Device::Intel => (true, false),
                Device::Automatic => (va.is_some(), va.is_none() && olo_video::nvdec::available() && (video.nvidia_on_battery || !olo_video::on_battery())),
                Device::Processor | Device::Default => (false, false),
            };
            if intel && let Some(c) = va {
                match olo_vaapi::decode::Decoder::new(c) {
                    Ok(mut dec) => {
                        // (Its pictures left where the GPU copies them.)
                        dec.hold(held());
                        let on = if dec.driver().contains("Intel") { ON_INTEL } else { ON_OTHER_GPU };
                        let hevc = (codec == Codec::Hevc).then(|| demux.hevc().map(|(c, _)| c)).flatten();
                        return Ok((Decoder::Va(Box::new(Va { dec, hevc, fresh: true })), on));
                    }
                    Err(e) => tracing::info!("video: VA-API: {e}; the processor decodes"),
                }
            }
            if nvidia && planes {
                let (c, hevc) = match codec {
                    Codec::H264 => (olo_video::nvdec::Codec::H264, None),
                    Codec::Hevc => (olo_video::nvdec::Codec::Hevc, demux.hevc().map(|(c, _)| c)),
                    Codec::Vp9 => (olo_video::nvdec::Codec::Vp9, None),
                    Codec::Av1 => (olo_video::nvdec::Codec::Av1, None),
                };
                match olo_video::nvdec::Nvdec::new(c) {
                    Ok(mut dec) => {
                        // (As VA-API's: left where they're copied.)
                        dec.hold(held());
                        return Ok((Decoder::Nv(Box::new(Nv { dec, hevc, fresh: true })), ON_NVIDIA));
                    }
                    Err(e) => tracing::info!("video: NVDEC: {e}; the processor decodes"),
                }
            }
        }
        #[cfg(not(feature = "gpu"))]
        let _ = (device, planes);
        Ok((Decoder::new(codec, demux)?, ON_PROCESSOR))
    }

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
    let (device, planes) = {
        let sh = m.lock().unwrap();
        (sh.device, sh.yuv.is_some())
    };
    let (mut dec, decoding) = Decoder::on(device, planes, codec, demux.as_ref())?;
    {
        let mut sh = m.lock().unwrap();
        sh.decoding = decoding;
        sh.decoders = Decoders::of(planes, codec);
    }
    #[cfg(feature = "gpu")]
    let mut nv_out: Vec<olo_video::nvdec::Picture> = Vec::new();
    #[cfg(feature = "gpu")]
    let mut va_out: Vec<olo_vaapi::decode::Picture> = Vec::new();
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
                    #[cfg(feature = "gpu")]
                    Decoder::Nv(n) => {
                        n.dec.reset();
                        n.fresh = true;
                    }
                    #[cfg(feature = "gpu")]
                    Decoder::Va(v) => {
                        v.dec.reset();
                        v.fresh = true;
                    }
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
            // NVIDIA's GPU: pictures in display order, as planes. One it
            // can't decode (a format its NVDEC hasn't): the processor from
            // where this stream started.
            #[cfg(feature = "gpu")]
            (Decoder::Nv(n), p) => {
                nv_out.clear();
                let failed = match &p {
                    Some(p) => {
                        let data = n.packet(&p.data);
                        n.dec.decode(&data, (p.pts * 1e6) as i64, &mut nv_out).err()
                    }
                    None => {
                        n.dec.flush(&mut nv_out);
                        None
                    }
                };
                if let Some(e) = failed {
                    tracing::info!("video {}: NVDEC: {e}; the processor decodes", input.name);
                    dec = Decoder::new(codec, demux.as_ref())?;
                    m.lock().unwrap().decoding = ON_PROCESSOR;
                    demux.seek(from);
                    times.clear();
                    continue;
                }
                let stream = m.lock().unwrap().yuv.unwrap_or(0);
                for pic in nv_out.drain(..) {
                    let planes = match pic.held {
                        Some(held) => {
                            let (ptr, len, y, uv) = (held.ptr(), held.len(), held.y, held.uv);
                            // SAFETY: the decoder's memory, unchanged while
                            // `held` (kept with it) lives.
                            (Vec::new(), Vec::new(), Some(unsafe { olo_yuv::YuvMemory::new(ptr, len, y, uv, Box::new(held)) }))
                        }
                        None => (pic.y, pic.uv, None),
                    };
                    let frame = Arc::new(gpu_picture(stream, pic.width, pic.height, pic.deep, planes, pic.matrix, pic.full_range));
                    if !push(&mut times, from, &mut lead, &|| Pic::Ready(Picture::Yuv(frame.clone()))) {
                        break;
                    }
                }
            }
            // Intel's GPU: a picture for each sample shown, with its
            // time (as VP9's on the processor). One it can't decode: the
            // processor from where this stream started.
            #[cfg(feature = "gpu")]
            (Decoder::Va(v), p) => {
                va_out.clear();
                let failed = match &p {
                    Some(p) => {
                        let data = annex_b(v.hevc.as_ref(), &mut v.fresh, &p.data);
                        v.dec.decode(&data, (p.pts * 1e6) as i64, &mut va_out).err()
                    }
                    // (H.264's and HEVC's last pictures, held for their
                    // order.)
                    None => v.dec.flush(&mut va_out).err(),
                };
                if let Some(e) = failed {
                    tracing::info!("video {}: VA-API: {e}; the processor decodes", input.name);
                    dec = Decoder::new(codec, demux.as_ref())?;
                    m.lock().unwrap().decoding = ON_PROCESSOR;
                    demux.seek(from);
                    times.clear();
                    continue;
                }
                let stream = m.lock().unwrap().yuv.unwrap_or(0);
                for pic in va_out.drain(..) {
                    while times.peek().is_some_and(|Reverse((e, _))| *e < pic.pts) {
                        times.pop();
                    }
                    // (Left where the GPU copied it: the window's GPU copies
                    // it from there, the processor copying nothing.)
                    let planes = match pic.held {
                        Some(held) => {
                            let (ptr, len, y, uv) = (held.ptr(), held.len(), held.y, held.uv);
                            // SAFETY: the decoder's memory, unchanged while
                            // `held` (kept with it) lives.
                            (Vec::new(), Vec::new(), Some(unsafe { olo_yuv::YuvMemory::new(ptr, len, y, uv, Box::new(held)) }))
                        }
                        None => (pic.y, pic.uv, None),
                    };
                    let frame = Arc::new(gpu_picture(stream, pic.width, pic.height, pic.deep, planes, pic.matrix, pic.full_range));
                    if !push(&mut times, from, &mut lead, &|| Pic::Ready(Picture::Yuv(frame.clone()))) {
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
    /// Later sample entries' sets, each from its first sample (empty:
    /// the first entry's again): `media::mp4_later_entries`.
    later: Vec<(u32, Vec<u8>)>,
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
        // Another encoder's stream after the first's (a recording's whose
        // encoder failed), in sample entries of their own.
        let later = match codec {
            Codec::H264 | Codec::Av1 => input.open().ok().map(|(mut f, len)| media::mp4_later_entries(&mut f, len, track)).unwrap_or_default(),
            _ => Vec::new(),
        };
        Ok(Mp4 { track, codec, timescale, shift, sets, later, hevc, colour, nal_len, times, syncs: stbl.stss.as_ref().map(|s| s.entries.clone()), next: 1, reader })
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
                // (The sets of the sample entry this sample is in.)
                let later = self.later.iter().rev().find(|(from, _)| *from <= id).map(|(_, sets)| sets).filter(|sets| !sets.is_empty());
                data.extend_from_slice(later.unwrap_or(&self.sets));
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
        // (Until the worker ends too: one that fails says why, here, rather
        // than leaving this waiting for pictures that won't come.)
        while sh.queue.len() < want(&sh) && !sh.eof && !worker.is_finished() {
            sh = cv.wait_timeout(sh, Duration::from_millis(50)).unwrap().0;
        }
        let pts: Vec<f64> = sh.queue.iter().map(|f| f.pts).collect();
        let wanted = want(&sh);
        sh.stop = true;
        drop(sh);
        cv.notify_all();
        if let Err(e) = worker.join().unwrap() {
            panic!("{name} @ {target}: {e} (pictures before it: {pts:?})");
        }
        assert!(pts.len() >= wanted, "{name} @ {target}: {pts:?}");
        // The first picture is the one showing at the target.
        assert!((pts[0] - target).abs() < 0.05, "{name} @ {target}: {pts:?}");
        assert!(pts.windows(2).all(|w| w[1] > w[0]), "{name}: in order {pts:?}");
    }

    /// A recording whose encoder failed and another went on: its second
    /// sample entry's parameter sets given at its first key frame, so it
    /// plays across the switch and from a seek past it. `OLO_FALLBACK_MP4`
    /// is olo-video's `a_recording_goes_on_with_another_encoder`'s file
    /// (kept with `OLO_KEEP_TEST_VIDEO=1`): NVENC to 1 s, then OpenH264.
    #[test]
    #[ignore]
    fn plays_across_another_encoders_entry() {
        let path = std::path::PathBuf::from(std::env::var("OLO_FALLBACK_MP4").expect("OLO_FALLBACK_MP4"));
        // (`OLO_FALLBACK_SEEKS=3.3,3.6`: where a recording's own switch is.)
        let targets: Vec<f64> = std::env::var("OLO_FALLBACK_SEEKS").map_or_else(|_| vec![0.5, 0.97, 1.5], |s| s.split(',').filter_map(|t| t.parse().ok()).collect());
        for target in targets {
            seeks(&path, target);
        }
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

    /// HDR10 said only in the stream (its SPS's VUI; no `colr`):
    /// `OLO_SAMPLES=dir` with Chromium's bear-1280x720-hevc-10bit-hdr10.mp4
    /// (media/test/data: x265's HDR10 tags on its SDR bear, so its
    /// pictures blow out as PQ, as tagged). Its facts say HDR, and its
    /// pictures are wide (P3), and HDR10's when a window draws in it.
    #[test]
    #[ignore]
    fn hdr10_in_the_stream() {
        let dir = std::path::PathBuf::from(std::env::var("OLO_SAMPLES").expect("OLO_SAMPLES"));
        let path = dir.join("bear-1280x720-hevc-10bit-hdr10.mp4");
        let Ok(crate::load::Update::Body(crate::load::Body::Media(m))) = media::video(&path, &Arc::new(std::sync::atomic::AtomicBool::new(false))) else { panic!("no facts") };
        let colour = m.rows.iter().find(|r| r.0 == "Colour").map(|r| r.1.as_str());
        assert_eq!(colour, Some("HDR (PQ), BT.2020"), "{:?}", m.rows);
        let input = Input::file(&path);
        assert!(picture_at(&input, 1.0).unwrap().p3);
        let video = Video::start_with(&input, 3.0, Options { playing: false, picture: true, sound: false, bgra: true, yuv: false, device: Device::Processor });
        video.draw_hdr(true);
        let t0 = Instant::now();
        let deep = loop {
            if let (Some(p), _) = video.tick_picture() {
                break p;
            }
            assert!(t0.elapsed() < Duration::from_secs(10), "no picture");
            std::thread::sleep(Duration::from_millis(20));
        };
        // (The first may have been made before the switch; either way
        // the stream is known HDR now.)
        assert!(video.is_hdr());
        assert!(matches!(deep, Picture::Deep { .. } | Picture::Bgra { p3: true, .. }));
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
            // (A worker that ended without eof failed: said below.)
            if (sh.eof || worker.is_finished()) && sh.queue.is_empty() {
                sh.stop = true;
                break;
            }
            sh = cv.wait_timeout(sh, Duration::from_millis(5)).unwrap().0;
            drop(sh);
        }
        cv.notify_all();
        if let Err(e) = worker.join().unwrap() {
            panic!("{}: {e}", path.display());
        }
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
        // 4K's planes get all six ahead (a tenth of a second at 60);
        // bigger pictures fewer, never under two.
        assert_eq!((QUEUE_BYTES / (3840 * 2160 * 3 / 2)).clamp(2, AHEAD), AHEAD);
        assert_eq!((QUEUE_BYTES / (7680 * 4320 * 3 / 2)).clamp(2, AHEAD), 2);
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
                if (sh.eof || worker.is_finished()) && sh.queue.is_empty() {
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
