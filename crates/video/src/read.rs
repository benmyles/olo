//! A video's pictures read in order (an MP4's video track: H.264 by
//! OpenH264, HEVC and AV1 by `olo_images`), for what's made from a
//! recording: a GIF (`gif.rs`) and an export with edits (`export.rs`).
//! Each sample is decoded once, as the file holds them (Olo's recordings
//! and these encoders' have no reordered frames), across later sample
//! entries (another encoder's stream, where a recording's failed).

use std::{
    fs::File,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

/// An MP4's video track (H.264, HEVC or AV1), read sample by sample.
pub struct Video {
    path: std::path::PathBuf,
    track: u32,
    pub width: u32,
    pub height: u32,
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
    pub fn open(path: &Path) -> Result<Video, String> {
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

    /// Its clock's ticks a second (its samples' times are in them).
    pub fn timescale(&self) -> u32 {
        self.timescale
    }

    pub fn seconds(&self) -> f64 {
        self.duration as f64 / 1000.0
    }

    /// Each picture in order, full size (RGB), with when it shows from
    /// and till (seconds; the last held to the video's end).
    pub fn each_picture(&self, cancel: &AtomicBool, f: &mut dyn FnMut(f64, f64, &[u8]) -> Result<(), String>) -> Result<(), String> {
        let mut r = self.reopen()?;
        let mut dec = Decoder::new(&self.start)?;
        let mut rgb = vec![0u8; self.width as usize * self.height as usize * 3];
        let scale = f64::from(self.timescale.max(1));
        // A picture waiting for the next to say when it ends (a sample's
        // own duration ends it otherwise).
        let mut pending: Option<(f64, f64, Vec<u8>)> = None;
        for i in 1..=self.samples {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let Some(s) = r.read_sample(self.track, i).map_err(|e| e.to_string())? else { break };
            let fresh = self.fresh(&mut dec, i);
            let (from, to) = (s.start_time as f64 / scale, (s.start_time + u64::from(s.duration)) as f64 / scale);
            if dec.decode(&s.bytes, fresh, (self.width, self.height), &mut rgb)? {
                if let Some((a, _, p)) = pending.take() {
                    f(a, from, &p)?;
                }
                pending = Some((from, to, rgb.clone()));
            } else if let Some(p) = pending.as_mut() {
                // (A sample that showed nothing new: the last picture on.)
                p.1 = to;
            }
        }
        // A picture the decoder still held: on from where the last ends.
        if dec.finish((self.width, self.height), &mut rgb) {
            let at = pending.as_ref().map_or(0.0, |p| p.1);
            if let Some((a, b, p)) = pending.take() {
                f(a, b, &p)?;
            }
            pending = Some((at, self.seconds().max(at), rgb.clone()));
        }
        if let Some((a, b, p)) = pending {
            f(a, b.max(self.seconds()), &p)?;
        }
        Ok(())
    }

    /// The picture showing at each of `times` (seconds, in order), made
    /// `w` × `h` (RGBA, area averages) and given with its index as it
    /// comes: an editor's pictures along its timeline, in one pass.
    pub fn pictures_near(&self, times: &[f64], w: u32, h: u32, cancel: &AtomicBool, f: &mut dyn FnMut(usize, &[u8]) -> Result<(), String>) -> Result<(), String> {
        let mut small = vec![0u8; w as usize * h as usize * 4];
        let mut k = 0usize;
        self.each_picture(cancel, &mut |a, b, rgb| {
            let mut made = false;
            while k < times.len() && times[k] < b {
                if times[k] + 1e-9 >= a || k == 0 || !made {
                    if !made {
                        shrink(rgb, self.width as usize, self.height as usize, &mut small, w as usize, h as usize);
                        made = true;
                    }
                    f(k, &small)?;
                }
                k += 1;
            }
            Ok(())
        })
    }

    /// Each moment shown at `fps` (`k`, from 0), its picture `ow` × `oh`
    /// (RGBA), in order: a sample is decoded once and given for every
    /// moment it's on screen.
    pub fn each_shown(&self, fps: u32, ow: u32, oh: u32, cancel: &AtomicBool, f: &mut dyn FnMut(usize, &[u8]) -> Result<(), String>) -> Result<(), String> {
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
pub fn shrink(src: &[u8], sw: usize, sh: usize, dst: &mut [u8], dw: usize, dh: usize) {
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
