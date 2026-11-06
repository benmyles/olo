//! A recording made a GIF (`olo-screenshot --gif`, Screenshot's Save as
//! GIF): the MP4's pictures decoded (OpenH264; HEVC and AV1 by
//! `olo_images`), made smaller by area
//! averages to at most `Options::width`, shown at `Options::fps`, in one
//! palette for the whole GIF (NeuQuant trained on pictures from across
//! it, so colours don't shift from frame to frame), each frame only the
//! rectangle that changed, what stayed the same in it transparent (the
//! LZW codes such runs in a few bytes). Two passes over the video: the
//! first for the palette, the second for the frames; cancellable between
//! any two pictures.

use std::{
    fs::File,
    io::BufWriter,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

/// How a GIF is made.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Frames a second (a picture held longer is one frame, longer).
    pub fps: u32,
    /// The widest it's made (pixels; 0: the video's own).
    pub width: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options { fps: 10, width: 960 }
    }
}

/// What was made.
#[derive(Debug, Clone, PartialEq)]
pub struct Made {
    pub frames: usize,
    pub width: u32,
    pub height: u32,
    pub seconds: f64,
    pub bytes: u64,
}

/// Colours in the palette; the last index is transparency.
const COLOURS: usize = 255;
const CLEAR: u8 = 255;
/// Pictures the palette is trained on.
const TRAIN: usize = 16;

/// The MP4 at `src` as a GIF at `dst`; `progress` told how far (0 to 1).
pub fn from_mp4(src: &Path, dst: &Path, opts: &Options, cancel: &AtomicBool, progress: &dyn Fn(f32)) -> Result<Made, String> {
    let video = Video::open(src)?;
    let (vw, vh) = (video.width, video.height);
    let (ow, oh) = if opts.width > 0 && vw > opts.width { (opts.width, ((u64::from(vh) * u64::from(opts.width) / u64::from(vw)) as u32).max(1)) } else { (vw, vh) };
    let fps = opts.fps.clamp(1, 50);
    let seconds = video.seconds();
    let shown = ((seconds * f64::from(fps)).ceil() as usize).max(1);
    // Pass one: pictures from across it, for the palette.
    let every = shown.div_ceil(TRAIN).max(1);
    let mut sample: Vec<u8> = Vec::new();
    video.each_shown(fps, ow, oh, cancel, &mut |k, rgba| {
        if k % every == 0 {
            sample.extend_from_slice(rgba);
        }
        progress(0.3 * k as f32 / shown as f32);
        Ok(())
    })?;
    if sample.is_empty() {
        return Err("the video shows nothing".into());
    }
    let quant = color_quant::NeuQuant::new(10, COLOURS, &sample);
    drop(sample);
    let mut palette = quant.color_map_rgb();
    palette.resize(256 * 3, 0);
    // A colour's index, by its 15 bits (what a GIF can tell apart anyway).
    let mut cache = vec![u16::MAX; 1 << 15];
    let mut index = |p: &[u8]| -> u8 {
        let key = (usize::from(p[0] >> 3) << 10) | (usize::from(p[1] >> 3) << 5) | usize::from(p[2] >> 3);
        if cache[key] == u16::MAX {
            cache[key] = quant.index_of(&[p[0], p[1], p[2], 255]) as u16;
        }
        cache[key] as u8
    };
    // Pass two: each change a frame, written once the next comes (its
    // delay is till then).
    let out = File::create(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    let mut gif = gif::Encoder::new(BufWriter::new(out), ow as u16, oh as u16, &palette).map_err(|e| e.to_string())?;
    gif.set_repeat(gif::Repeat::Infinite).map_err(|e| e.to_string())?;
    let mut shown_now: Option<Vec<u8>> = None;
    let mut pending: Option<(gif::Frame<'static>, usize)> = None;
    let mut frames = 0usize;
    let mut last_k = 0;
    video.each_shown(fps, ow, oh, cancel, &mut |k, rgba| {
        last_k = k;
        let indexed: Vec<u8> = rgba.chunks_exact(4).map(&mut index).collect();
        let frame = match &shown_now {
            None => Some(gif::Frame { width: ow as u16, height: oh as u16, buffer: indexed.clone().into(), ..Default::default() }),
            Some(prev) => changed(prev, &indexed, ow as usize, oh as usize).map(|[x, y, w, h]| {
                let mut buf = Vec::with_capacity(w * h);
                for row in y..y + h {
                    let at = row * ow as usize;
                    for col in x..x + w {
                        let (now, was) = (indexed[at + col], prev[at + col]);
                        buf.push(if now == was { CLEAR } else { now });
                    }
                }
                gif::Frame { left: x as u16, top: y as u16, width: w as u16, height: h as u16, transparent: Some(CLEAR), buffer: buf.into(), ..Default::default() }
            }),
        };
        if let Some(mut f) = frame {
            f.dispose = gif::DisposalMethod::Keep;
            if let Some(p) = pending.take() {
                write(&mut gif, p, k, fps)?;
                frames += 1;
            }
            pending = Some((f, k));
            shown_now = Some(indexed);
        }
        progress(0.3 + 0.7 * k as f32 / shown as f32);
        Ok(())
    })?;
    if let Some(p) = pending.take() {
        write(&mut gif, p, (last_k + 1).max(shown), fps)?;
        frames += 1;
    }
    drop(gif);
    let bytes = std::fs::metadata(dst).map(|m| m.len()).unwrap_or(0);
    progress(1.0);
    Ok(Made { frames, width: ow, height: oh, seconds, bytes })
}

/// Frame `f`, shown from moment `from` till `to` (at `fps`), written.
fn write(gif: &mut gif::Encoder<BufWriter<File>>, (mut f, from): (gif::Frame<'static>, usize), to: usize, fps: u32) -> Result<(), String> {
    // (In hundredths, from the start: no drift from rounding each.)
    let centis = |k: usize| (k as f64 * 100.0 / f64::from(fps)).round() as u64;
    f.delay = (centis(to) - centis(from)).clamp(2, u64::from(u16::MAX)) as u16;
    gif.write_frame(&f).map_err(|e| e.to_string())
}

/// The rectangle (x, y, w, h) in which `now` differs from `was`, if it does.
fn changed(was: &[u8], now: &[u8], w: usize, h: usize) -> Option<[usize; 4]> {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        let (a, b) = (&was[y * w..(y + 1) * w], &now[y * w..(y + 1) * w]);
        if a == b {
            continue;
        }
        let first = a.iter().zip(b).position(|(p, q)| p != q).unwrap_or(0);
        let last = w - 1 - a.iter().rev().zip(b.iter().rev()).position(|(p, q)| p != q).unwrap_or(0);
        x0 = x0.min(first);
        x1 = x1.max(last);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    (x0 <= x1 && y0 <= y1).then(|| [x0, y0, x1 - x0 + 1, y1 - y0 + 1])
}

/// An MP4's video track (H.264, HEVC or AV1), read sample by sample.
struct Video {
    path: std::path::PathBuf,
    track: u32,
    width: u32,
    height: u32,
    timescale: u32,
    samples: u32,
    duration: u64,
    /// What its decoder starts from.
    start: Start,
    /// Later sample entries' parameter sets (another encoder's stream,
    /// where a recording's failed), each from its first sample.
    later: Vec<(u32, Vec<u8>)>,
}

/// A later sample entry's parameter sets, as its decoder is given them
/// before its first sample: H.264's SPS and PPS (Annex B), AV1's
/// configOBUs. (HEVC's: none; no recording goes on in another's.)
fn entry_sets(entry: &[u8]) -> Option<Vec<u8>> {
    let boxes = entry.get(86..)?;
    match entry.get(4..8)? {
        b"avc1" | b"avc3" => {
            let c = crate::trim::find(boxes, b"avcC")?;
            let mut out = Vec::new();
            let mut at = 5;
            for mask in [0x1f, 0xff] {
                let n = (*c.get(at)? & mask) as usize;
                at += 1;
                for _ in 0..n {
                    let l = u16::from_be_bytes([*c.get(at)?, *c.get(at + 1)?]) as usize;
                    out.extend_from_slice(&[0, 0, 0, 1]);
                    out.extend_from_slice(c.get(at + 2..at + 2 + l)?);
                    at += 2 + l;
                }
            }
            Some(out)
        }
        b"av01" => crate::trim::find(boxes, b"av1C").and_then(|c| c.get(4..)).map(<[u8]>::to_vec),
        _ => None,
    }
}

/// A decoder's start: H.264's parameter sets (Annex B, sent before the
/// first sample), HEVC's `hvcC`, AV1's configOBUs (its sequence header,
/// sent before the first sample).
#[derive(Clone)]
enum Start {
    H264(Vec<u8>),
    Hevc(Vec<u8>),
    Av1(Vec<u8>),
}

impl Video {
    fn open(path: &Path) -> Result<Video, String> {
        let f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let size = f.metadata().map_err(|e| e.to_string())?.len();
        let reader = mp4::Mp4Reader::read_header(std::io::BufReader::new(f), size).map_err(|e| format!("not a video Olo can read: {e}"))?;
        let (&track, t) = reader.tracks().iter().find(|(_, t)| t.track_type().ok() == Some(mp4::TrackType::Video)).ok_or("no video in it")?;
        let entries = crate::trim::video_entries(path).ok_or("not a video Olo makes GIFs of (H.264, HEVC or AV1)")?;
        let later = entries.iter().skip(1).filter_map(|(from, e)| Some((*from, entry_sets(e)?))).collect();
        let entry = entries.into_iter().next().map(|(_, e)| e).ok_or("no video entry")?;
        // (A visual sample entry: 8 bytes of header and 78 of fields, then
        // its boxes.)
        let config = |kind: &[u8; 4]| entry.get(86..).and_then(|b| crate::trim::find(b, kind)).map(<[u8]>::to_vec);
        let start = match &entry[4..8] {
            b"hvc1" | b"hev1" => Start::Hevc(config(b"hvcC").ok_or("an HEVC video without its configuration")?),
            b"av01" => Start::Av1(config(b"av1C").and_then(|c| c.get(4..).map(<[u8]>::to_vec)).ok_or("an AV1 video without its configuration")?),
            _ => {
                let mut params = Vec::new();
                for set in [t.sequence_parameter_set().ok(), t.picture_parameter_set().ok()].into_iter().flatten() {
                    params.extend_from_slice(&[0, 0, 0, 1]);
                    params.extend_from_slice(set);
                }
                Start::H264(params)
            }
        };
        let (width, height) = (u32::from(u16::from_be_bytes([entry[32], entry[33]])), u32::from(u16::from_be_bytes([entry[34], entry[35]])));
        let (timescale, samples, duration) = (t.timescale(), t.sample_count(), t.duration().as_millis() as u64);
        Ok(Video { path: path.to_path_buf(), track, width, height, timescale, samples, duration, start, later })
    }

    /// Whether sample `i` starts a stream (the first, or a later sample
    /// entry's first, whose sets `dec` is given).
    fn fresh(&self, dec: &mut Decoder, i: u32) -> bool {
        match self.later.iter().find(|(from, _)| *from == i) {
            Some((_, sets)) => {
                dec.restart(sets);
                true
            }
            None => i == 1,
        }
    }

    fn seconds(&self) -> f64 {
        self.duration as f64 / 1000.0
    }

    /// Each moment shown at `fps` (`k`, from 0), its picture `ow` × `oh`
    /// (RGBA), in order: a sample is decoded once and given for every
    /// moment it's on screen.
    fn each_shown(&self, fps: u32, ow: u32, oh: u32, cancel: &AtomicBool, f: &mut dyn FnMut(usize, &[u8]) -> Result<(), String>) -> Result<(), String> {
        // (A reader of its own: the file read again from the start.)
        let mut r = self.reopen()?;
        let mut dec = Decoder::new(&self.start)?;
        let shown = ((self.seconds() * f64::from(fps)).ceil() as usize).max(1);
        let mut rgb = vec![0u8; self.width as usize * self.height as usize * 3];
        let mut small = vec![0u8; ow as usize * oh as usize * 4];
        let mut have = false;
        let mut k = 0usize;
        for i in 1..=self.samples {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let Some(s) = r.read_sample(self.track, i).map_err(|e| e.to_string())? else { break };
            let fresh = self.fresh(&mut dec, i);
            if dec.decode(&s.bytes, fresh, (self.width, self.height), &mut rgb)? {
                shrink(&rgb, self.width as usize, self.height as usize, &mut small, ow as usize, oh as usize);
                have = true;
            }
            // The moments this sample is on screen till the next starts.
            let end = (s.start_time + u64::from(s.duration)) as f64 / f64::from(self.timescale.max(1));
            while have && k < shown && (k as f64 / f64::from(fps)) < end {
                f(k, &small)?;
                k += 1;
            }
        }
        // (A picture the decoder still held, then the last held to the end.)
        if dec.finish((self.width, self.height), &mut rgb) {
            shrink(&rgb, self.width as usize, self.height as usize, &mut small, ow as usize, oh as usize);
            have = true;
        }
        while have && k < shown {
            f(k, &small)?;
            k += 1;
        }
        Ok(())
    }

    /// A reader of its own, from the start.
    fn reopen(&self) -> Result<mp4::Mp4Reader<std::io::BufReader<File>>, String> {
        let f = File::open(&self.path).map_err(|e| e.to_string())?;
        let size = f.metadata().map_err(|e| e.to_string())?.len();
        mp4::Mp4Reader::read_header(std::io::BufReader::new(f), size).map_err(|e| e.to_string())
    }
}

/// A video's decoder, of its codec.
enum Decoder {
    H264(Box<openh264::decoder::Decoder>, Vec<u8>),
    Hevc(Box<olo_images::hevc::Decoder>),
    Av1(olo_images::av1::Decoder, Vec<u8>),
}

impl Decoder {
    fn new(start: &Start) -> Result<Decoder, String> {
        Ok(match start {
            Start::H264(params) => Decoder::H264(Box::new(openh264::decoder::Decoder::new().map_err(|e| e.to_string())?), params.clone()),
            Start::Hevc(record) => {
                let config = olo_images::hevc::config(record).ok_or("an HEVC configuration Olo can't read")?;
                // (One thread: each sample's picture out at once, none held.)
                Decoder::Hevc(Box::new(olo_images::hevc::Decoder::new(config, olo_images::hevc::Colour::default(), 1)))
            }
            // (No frames held back: each sample's picture out at once.)
            Start::Av1(obus) => Decoder::Av1(olo_images::av1::Decoder::new(0, 1)?, obus.clone()),
        })
    }

    /// Sample `sample` (as the file holds it; `first`: the stream's
    /// first) decoded into `rgb` (`size`, RGB) if it gave a picture of
    /// that size.
    fn decode(&mut self, sample: &[u8], first: bool, size: (u32, u32), rgb: &mut [u8]) -> Result<bool, String> {
        let pictures = match self {
            Decoder::H264(dec, params) => {
                let mut annex_b = if first { params.clone() } else { Vec::new() };
                let mut b = sample;
                while b.len() >= 4 {
                    let n = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
                    if b.len() < 4 + n {
                        break;
                    }
                    annex_b.extend_from_slice(&[0, 0, 0, 1]);
                    annex_b.extend_from_slice(&b[4..4 + n]);
                    b = &b[4 + n..];
                }
                return Ok(match dec.decode(&annex_b).map_err(|e| e.to_string())? {
                    Some(yuv) => {
                        use openh264::formats::YUVSource as _;
                        let ok = yuv.dimensions() == (size.0 as usize, size.1 as usize);
                        if ok {
                            yuv.write_rgb8(rgb);
                        }
                        ok
                    }
                    None => false,
                });
            }
            Decoder::Hevc(dec) => {
                let mut out = Vec::new();
                dec.decode(sample, &mut out)?;
                out
            }
            Decoder::Av1(dec, config) => {
                let mut out = Vec::new();
                let unit = if first { [config.as_slice(), sample].concat() } else { sample.to_vec() };
                dec.decode(&unit, &mut out)?;
                out
            }
        };
        Ok(into_rgb(pictures, size, rgb))
    }

    /// Another stream's parameter sets from the next sample (`fresh`).
    fn restart(&mut self, sets: &[u8]) {
        match self {
            Decoder::H264(_, params) => *params = sets.to_vec(),
            Decoder::Av1(_, config) => *config = sets.to_vec(),
            Decoder::Hevc(_) => {}
        }
    }

    /// At the end: a picture the decoder still held, into `rgb`.
    fn finish(&mut self, size: (u32, u32), rgb: &mut [u8]) -> bool {
        let mut out = Vec::new();
        match self {
            Decoder::H264(..) => return false,
            Decoder::Hevc(dec) => dec.flush(&mut out),
            Decoder::Av1(dec, _) => dec.flush(&mut out),
        }
        into_rgb(out, size, rgb)
    }
}

/// The last of `pictures`, if it's `size`, into `rgb` (RGB).
fn into_rgb(pictures: Vec<olo_images::av1::Frame>, size: (u32, u32), rgb: &mut [u8]) -> bool {
    let Some(f) = pictures.into_iter().last().filter(|f| (f.width, f.height) == size) else { return false };
    for (o, i) in rgb.as_chunks_mut::<3>().0.iter_mut().zip(f.to_rgba().as_chunks::<4>().0) {
        o.copy_from_slice(&i[..3]);
    }
    true
}

/// `src` (RGB, `sw` × `sh`) into `dst` (RGBA, `dw` × `dh`): each pixel the
/// average of the source pixels it covers.
fn shrink(src: &[u8], sw: usize, sh: usize, dst: &mut [u8], dw: usize, dh: usize) {
    if (sw, sh) == (dw, dh) {
        for (o, i) in dst.chunks_exact_mut(4).zip(src.chunks_exact(3)) {
            o[..3].copy_from_slice(i);
            o[3] = 255;
        }
        return;
    }
    for y in 0..dh {
        let (y0, y1) = (y * sh / dh, ((y + 1) * sh / dh).max(y * sh / dh + 1));
        for x in 0..dw {
            let (x0, x1) = (x * sw / dw, ((x + 1) * sw / dw).max(x * sw / dw + 1));
            let mut sum = [0u32; 3];
            for sy in y0..y1 {
                let row = &src[(sy * sw + x0) * 3..(sy * sw + x1) * 3];
                for p in row.chunks_exact(3) {
                    sum[0] += u32::from(p[0]);
                    sum[1] += u32::from(p[1]);
                    sum[2] += u32::from(p[2]);
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let o = &mut dst[(y * dw + x) * 4..(y * dw + x) * 4 + 4];
            o[0] = (sum[0] / n) as u8;
            o[1] = (sum[1] / n) as u8;
            o[2] = (sum[2] / n) as u8;
            o[3] = 255;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A recording of a red square moving on grey, two seconds, made a
    /// GIF and read back: its size, a frame a move, the square's colour.
    #[test]
    fn a_recording_made_a_gif() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-gif-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let (mp4_path, gif_path) = (dir.join("r.mp4"), dir.join("r.gif"));
        let (w, h) = (320u32, 240u32);
        let mut enc = crate::H264::new(w, h, 30).unwrap();
        let mut mp4 = crate::Mp4::create(&mp4_path, w, h, 1000).unwrap();
        for i in 0..4u32 {
            // BGRA: grey, the square red, 40 px further each half second.
            let px: Vec<u8> = (0..w * h)
                .flat_map(|p| {
                    let (x, y) = (p % w, p / w);
                    if (40 + i * 40..80 + i * 40).contains(&x) && (100..140).contains(&y) { [0, 0, 220, 255] } else { [128, 128, 128, 255] }
                })
                .collect();
            let (bits, key) = enc.encode(&px, w as usize * 4, crate::Order::Bgra).unwrap();
            mp4.add(&bits, key, 500).unwrap();
        }
        mp4.finish().unwrap();
        let never = AtomicBool::new(false);
        let made = from_mp4(&mp4_path, &gif_path, &Options { fps: 10, width: 160 }, &never, &|_| {}).unwrap();
        assert_eq!((made.width, made.height), (160, 120));
        // A frame for each move: the still stretches held, not repeated.
        assert_eq!(made.frames, 4, "{made:?}");
        let mut d = gif::DecodeOptions::new();
        d.set_color_output(gif::ColorOutput::RGBA);
        let mut r = d.read_info(std::fs::File::open(&gif_path).unwrap()).unwrap();
        assert_eq!((r.width(), r.height()), (160, 120));
        let first = r.read_next_frame().unwrap().unwrap().clone();
        assert_eq!(first.delay, 50);
        // The square at (20..40, 50..70) in the half-size picture: red.
        let at = |x: usize, y: usize| &first.buffer[(y * 160 + x) * 4..(y * 160 + x) * 4 + 3];
        let red = at(30, 60);
        assert!(red[0] > 180 && red[1] < 50 && red[2] < 50, "{red:?}");
        let grey = at(100, 20);
        assert!((110..150).contains(&grey[0]) && (110..150).contains(&grey[2]), "{grey:?}");
        // The next: only the part that changed.
        let second = r.read_next_frame().unwrap().unwrap();
        assert!(second.width < 160 && second.height < 120, "{}x{}", second.width, second.height);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_what_changed() {
        let was = vec![0u8; 8 * 4];
        let mut now = was.clone();
        assert_eq!(changed(&was, &now, 8, 4), None);
        now[8 + 3] = 1;
        now[2 * 8 + 5] = 1;
        assert_eq!(changed(&was, &now, 8, 4), Some([3, 1, 3, 2]));
    }

    #[test]
    fn averages_when_smaller() {
        // 4 × 2 of red and blue halves to 2 × 1.
        let src: Vec<u8> = (0..8).flat_map(|i| if i % 4 < 2 { [200, 0, 0] } else { [0, 0, 100] }).collect();
        let mut dst = vec![0u8; 2 * 4];
        shrink(&src, 4, 2, &mut dst, 2, 1);
        assert_eq!(dst, [200, 0, 0, 255, 0, 0, 100, 255]);
    }
}

/// For tests: a video opened, and each of its pictures decoded (RGB, its
/// size), in order.
#[cfg(test)]
pub(crate) struct Opened(Video);

#[cfg(test)]
pub(crate) fn tests_open(path: &Path) -> Result<Opened, String> {
    Video::open(path).map(Opened)
}

#[cfg(test)]
impl Opened {
    pub(crate) fn each_decoded(&self, f: &mut dyn FnMut(&[u8]) -> Result<(), String>) -> Result<(), String> {
        let v = &self.0;
        let mut r = v.reopen()?;
        let mut dec = Decoder::new(&v.start)?;
        let mut rgb = vec![0u8; v.width as usize * v.height as usize * 3];
        for i in 1..=v.samples {
            let Some(s) = r.read_sample(v.track, i).map_err(|e| e.to_string())? else { break };
            let fresh = v.fresh(&mut dec, i);
            if dec.decode(&s.bytes, fresh, (v.width, v.height), &mut rgb)? {
                f(&rgb)?;
            }
        }
        Ok(())
    }
}
