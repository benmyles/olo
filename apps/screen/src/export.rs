//! A screen's history as an MP4 video, made in process: the frames
//! (`record`) at 10 a second, H.264 from OpenH264, in an MP4 of our own
//! making (`Mp4`). Stretches with nothing happening (no action and no
//! frame changing for over `QUIET_MS`) are cut, each replaced by a card
//! saying how long it was ("4 min 12 s with nothing happening"). Run on a
//! worker: `progress` is told how far it has got and says whether to go
//! on.

use std::{io::Write, path::Path};

use openh264::{
    OpenH264API,
    encoder::{Encoder, EncoderConfig, FrameRate, FrameType},
    formats::{RgbaSliceU8, YUVBuffer},
};

use crate::{
    history,
    record::{self, Frame},
};

const FPS: u64 = 10;
const STEP_MS: u64 = 1000 / FPS;
/// Longer than this with nothing happening is cut.
const QUIET_MS: u64 = 10_000;
/// Kept either side of what happens, around a cut.
const LEAD_MS: u64 = 1500;
const CARD_MS: u64 = 2000;

/// One piece of the video: the screen from `from` to `to`, or a card for
/// a cut of `ms`.
#[derive(Debug, Clone, PartialEq)]
enum Piece {
    Screen { from: u64, to: u64 },
    Card { ms: u64 },
}

/// What happens when: the actions' starts and the frames' times, cut
/// into pieces where it goes quiet.
fn pieces(events: &mut [u64]) -> Vec<Piece> {
    events.sort_unstable();
    let (Some(&first), Some(&last)) = (events.first(), events.last()) else { return Vec::new() };
    let mut out = Vec::new();
    let mut from = first.saturating_sub(LEAD_MS / 3);
    for w in events.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b - a > QUIET_MS {
            out.push(Piece::Screen { from, to: a + LEAD_MS });
            out.push(Piece::Card { ms: (b - LEAD_MS) - (a + LEAD_MS) });
            from = b - LEAD_MS;
        }
    }
    out.push(Piece::Screen { from, to: last + LEAD_MS });
    out
}

/// "4 min 12 s".
fn span(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min {} s", s / 60, s % 60),
        _ => format!("{} h {} min", s / 3600, s / 60 % 60),
    }
}

/// Make screen `name`'s history into `out`. `progress(0..1)` false
/// stops it (and removes what was written).
pub fn export(name: &str, out: &Path, progress: &mut dyn FnMut(f32) -> bool) -> Result<(), String> {
    let actions = history::read(name);
    let segments = record::index(name);
    let mut events: Vec<u64> = actions.iter().map(|a| a.at).chain(segments.iter().flat_map(|s| s.frames.iter().map(|f| f.at))).collect();
    let plan = pieces(&mut events);
    if plan.is_empty() {
        return Err("nothing has been done on this screen".into());
    }
    let first = segments.iter().flat_map(|s| s.frames.first()).map(|f| f.at).next().ok_or("no frames were kept")?;
    let (_, probe) = record::frame_at(&segments, first).ok_or("no frames were kept")?;
    // H.264 wants even sizes.
    let (w, h) = (probe.width & !1, probe.height & !1);
    let total: u64 = plan.iter().map(|p| match p { Piece::Screen { from, to } => (to - from) / STEP_MS, Piece::Card { .. } => CARD_MS / STEP_MS }).sum::<u64>().max(1);
    let config = EncoderConfig::new().max_frame_rate(FrameRate::from_hz(FPS as f32));
    let mut encoder = Encoder::with_api_config(OpenH264API::from_source(), config).map_err(|e| e.to_string())?;
    let mut mp4 = Mp4::new(w as u32, h as u32);
    let mut done = 0u64;
    let mut last = probe;
    for p in &plan {
        let frames: Box<dyn Iterator<Item = Frame>> = match p {
            Piece::Screen { from, to } => {
                let mut player = record::Player::new(&segments);
                let mut keep = last.clone();
                Box::new((*from..*to).step_by(STEP_MS as usize).map(move |t| {
                    if let Some(f) = player.at(t) {
                        keep = f.clone();
                    }
                    keep.clone()
                }))
            }
            Piece::Card { ms } => {
                let card = card(w, h, &format!("{} with nothing happening", span(*ms)));
                Box::new(std::iter::repeat_n(card, (CARD_MS / STEP_MS) as usize))
            }
        };
        for f in frames {
            let even = crop(&f, w, h);
            let yuv = YUVBuffer::from_rgb_source(RgbaSliceU8::new(&even, (w, h)));
            let bits = encoder.encode(&yuv).map_err(|e| e.to_string())?;
            let key = matches!(bits.frame_type(), FrameType::IDR | FrameType::I);
            mp4.add(&bits.to_vec(), key);
            if matches!(p, Piece::Screen { .. }) {
                last = f;
            }
            done += 1;
            if !progress(done as f32 / total as f32) {
                return Err("cancelled".into());
            }
        }
    }
    let tmp = out.with_extension("mp4.part");
    mp4.write(&tmp, FPS as u32).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, out).map_err(|e| e.to_string())
}

/// `f` as exactly `w` × `h` RGBA (cut at the right and bottom, or
/// padded with black).
fn crop(f: &Frame, w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h.min(f.height) {
        let n = w.min(f.width) * 4;
        out[y * w * 4..y * w * 4 + n].copy_from_slice(&f.rgba[y * f.width * 4..y * f.width * 4 + n]);
    }
    out
}

/// A dark card with `text` in the middle, in the kit's own type.
fn card(w: usize, h: usize, text: &str) -> Frame {
    use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
    let mut rgba = [28u8, 30, 36, 255].repeat(w * h);
    let defs = olo_kit::style::all_font_definitions();
    let family = defs.families.get(&olo_kit::egui::FontFamily::Proportional).and_then(|f| f.first()).cloned();
    if let Some(font) = family.and_then(|k| defs.font_data.get(&k).cloned())
        && let Ok(font) = FontRef::try_from_slice((*font.font).as_ref())
    {
        let scale = PxScale::from((h as f32 / 18.0).max(16.0));
        let sf = font.as_scaled(scale);
        let width: f32 = text.chars().map(|c| sf.h_advance(font.glyph_id(c))).sum();
        let mut x = (w as f32 - width) / 2.0;
        let baseline = h as f32 / 2.0 + sf.ascent() / 2.5;
        for c in text.chars() {
            let g = font.glyph_id(c).with_scale_and_position(scale, ab_glyph::point(x, baseline));
            x += sf.h_advance(font.glyph_id(c));
            if let Some(o) = font.outline_glyph(g) {
                let b = o.px_bounds();
                o.draw(|gx, gy, cov| {
                    let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                    if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                        let i = (py as usize * w + px as usize) * 4;
                        for k in 0..3 {
                            let bg = rgba[i + k] as f32;
                            rgba[i + k] = (bg + (235.0 - bg) * cov.clamp(0.0, 1.0)) as u8;
                        }
                    }
                });
            }
        }
    }
    Frame { width: w, height: h, rgba }
}

/// The NAL units of an Annex B stream (start codes taken off).
fn nal_units(stream: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= stream.len() {
        if stream[i] == 0 && stream[i + 1] == 0 && stream[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(n, &s)| {
            let mut end = starts.get(n + 1).map_or(stream.len(), |&e| e - 3);
            // (A four-byte start code's leading zero.)
            while end > s && stream[end - 1] == 0 && starts.get(n + 1).is_some() {
                end -= 1;
            }
            &stream[s..end]
        })
        .filter(|u| !u.is_empty())
        .collect()
}

/// An MP4 of one H.264 track, written at the end: `ftyp`, `mdat` with
/// every sample (length-prefixed NAL units), and `moov` saying where
/// each is.
struct Mp4 {
    width: u32,
    height: u32,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    samples: Vec<Vec<u8>>,
    keys: Vec<u32>,
}

impl Mp4 {
    fn new(width: u32, height: u32) -> Mp4 {
        Mp4 { width, height, sps: None, pps: None, samples: Vec::new(), keys: Vec::new() }
    }

    /// One encoded picture (Annex B); its parameter sets are kept for
    /// the track's description.
    fn add(&mut self, annex_b: &[u8], key: bool) {
        let mut sample = Vec::new();
        for u in nal_units(annex_b) {
            match u[0] & 0x1f {
                7 => self.sps = Some(u.to_vec()),
                8 => self.pps = Some(u.to_vec()),
                _ => {
                    sample.extend_from_slice(&(u.len() as u32).to_be_bytes());
                    sample.extend_from_slice(u);
                }
            }
        }
        if sample.is_empty() {
            return;
        }
        self.samples.push(sample);
        if key {
            self.keys.push(self.samples.len() as u32);
        }
    }

    fn write(&self, path: &Path, fps: u32) -> std::io::Result<()> {
        let (Some(sps), Some(pps)) = (&self.sps, &self.pps) else { return Err(std::io::Error::other("the encoder gave no parameter sets")) };
        let ftyp = bx(b"ftyp", &[b"isom".as_slice(), &0x200u32.to_be_bytes(), b"isom", b"iso2", b"avc1", b"mp41"].concat());
        let data: usize = self.samples.iter().map(Vec::len).sum();
        let mdat_start = ftyp.len() as u64 + 8;
        let n = self.samples.len() as u32;
        let timescale = fps * 1000;
        let duration = n * 1000;
        let u32b = |v: u32| v.to_be_bytes().to_vec();
        let u16b = |v: u16| v.to_be_bytes().to_vec();
        let matrix: Vec<u8> = [0x10000u32, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000].iter().flat_map(|v| v.to_be_bytes()).collect();
        let mvhd = full(b"mvhd", 0, &[u32b(0), u32b(0), u32b(timescale), u32b(duration), u32b(0x10000), u16b(0x100), vec![0; 10], matrix.clone(), vec![0; 24], u32b(2)].concat());
        let tkhd = full(b"tkhd", 3, &[u32b(0), u32b(0), u32b(1), u32b(0), u32b(duration), vec![0; 8], u16b(0), u16b(0), u16b(0), u16b(0), matrix, u32b(self.width << 16), u32b(self.height << 16)].concat());
        let mdhd = full(b"mdhd", 0, &[u32b(0), u32b(0), u32b(timescale), u32b(duration), u16b(0x55c4), u16b(0)].concat());
        let hdlr = full(b"hdlr", 0, &[u32b(0), b"vide".to_vec(), vec![0; 12], b"Olo screen\0".to_vec()].concat());
        let avcc = bx(b"avcC", &[vec![1, sps[1], sps[2], sps[3], 0xff, 0xe1], u16b(sps.len() as u16), sps.clone(), vec![1], u16b(pps.len() as u16), pps.clone()].concat());
        // (No compressor name: 32 zero bytes.)
        let avc1 = bx(b"avc1", &[vec![0; 6], u16b(1), vec![0; 16], u16b(self.width as u16), u16b(self.height as u16), u32b(0x0048_0000), u32b(0x0048_0000), u32b(0), u16b(1), vec![0; 32], u16b(0x18), u16b(0xffff), avcc].concat());
        let stsd = full(b"stsd", 0, &[u32b(1), avc1].concat());
        let stts = full(b"stts", 0, &[u32b(1), u32b(n), u32b(1000)].concat());
        let stss = full(b"stss", 0, &[u32b(self.keys.len() as u32), self.keys.iter().flat_map(|k| k.to_be_bytes()).collect()].concat());
        let stsc = full(b"stsc", 0, &[u32b(1), u32b(1), u32b(n), u32b(1)].concat());
        let stsz = full(b"stsz", 0, &[u32b(0), u32b(n), self.samples.iter().flat_map(|s| (s.len() as u32).to_be_bytes()).collect()].concat());
        let co64 = full(b"co64", 0, &[u32b(1), mdat_start.to_be_bytes().to_vec()].concat());
        let stbl = bx(b"stbl", &[stsd, stts, stss, stsc, stsz, co64].concat());
        let vmhd = full(b"vmhd", 1, &[0u8; 8]);
        let dinf = bx(b"dinf", &full(b"dref", 0, &[u32b(1), full(b"url ", 1, &[])].concat()));
        let minf = bx(b"minf", &[vmhd, dinf, stbl].concat());
        let mdia = bx(b"mdia", &[mdhd, hdlr, minf].concat());
        let trak = bx(b"trak", &[tkhd, mdia].concat());
        let moov = bx(b"moov", &[mvhd, trak].concat());
        let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
        f.write_all(&ftyp)?;
        f.write_all(&((data + 8) as u32).to_be_bytes())?;
        f.write_all(b"mdat")?;
        for s in &self.samples {
            f.write_all(s)?;
        }
        f.write_all(&moov)?;
        f.flush()
    }
}

fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    [((body.len() + 8) as u32).to_be_bytes().as_slice(), kind, body].concat()
}

fn full(kind: &[u8; 4], flags: u32, body: &[u8]) -> Vec<u8> {
    bx(kind, &[&flags.to_be_bytes()[..], body].concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_stretches_become_cards() {
        let mut t = vec![1_000, 2_000, 3_000, 300_000, 301_000];
        let p = pieces(&mut t);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0], Piece::Screen { from: 500, to: 4_500 });
        assert_eq!(p[1], Piece::Card { ms: 298_500 - 4_500 });
        assert_eq!(p[2], Piece::Screen { from: 298_500, to: 302_500 });
        assert_eq!(span(252_000), "4 min 12 s");
        assert!(pieces(&mut []).is_empty());
    }

    #[test]
    fn annex_b_into_units() {
        let s = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9, 9];
        assert_eq!(nal_units(&s), vec![&[0x67, 1, 2][..], &[0x68, 3], &[0x65, 9, 9]]);
    }
}
