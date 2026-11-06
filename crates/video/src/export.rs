//! A recording edited and made again (Screenshot's editor;
//! `olo-screenshot --export`): what's kept of it (a trim is one stretch,
//! each cut splits one), each picture cropped and made smaller, at most
//! so many pictures a second, coded as finely as asked, its sound kept
//! or left out, its marks kept where their moments are; as an MP4 or a
//! GIF. On the caller's thread (a worker), told how far it is, and
//! stopped between any two pictures when `cancel` is set.
//!
//! The pictures are decoded in order (`read::Video`) and each is shown in
//! the new video for as long as it was in what's kept: a picture across
//! a cut joins up with itself (what's cut collapses), one wholly cut is
//! left out, and the times are put on the frame rate's grid when there is
//! one (a picture shorter than a step that doesn't reach the next is
//! left out, as a screen recording's pictures come only when something
//! changed). The sound (AAC) is decoded (symphonia), the stretches kept
//! taken to the sample, and coded again (`aac::Aac`). A GIF is made from
//! the edited video (`gif::from_mp4`).

use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{Choice, Codec, Encoder, Mp4, Order, Quality, read::Video};

/// What's done to a recording.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Edit {
    /// What's kept (seconds, in order, apart); empty: all of it.
    pub keep: Vec<(f64, f64)>,
    /// What's kept of each picture (its pixels: x, y, width, height).
    pub crop: Option<[u32; 4]>,
    /// The sound left out.
    pub mute: bool,
    /// Marks (seconds, title), each kept if its moment is; None: the
    /// recording's own.
    pub marks: Option<Vec<(f64, String)>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Mp4,
    Gif,
}

impl Format {
    pub fn parse(s: &str) -> Option<Format> {
        match s {
            "mp4" => Some(Format::Mp4),
            "gif" => Some(Format::Gif),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Mp4 => "mp4",
            Format::Gif => "gif",
        }
    }
}

/// How it's made.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    pub format: Format,
    /// The pictures' width (their height follows); 0: as they are. Never
    /// larger than they are.
    pub width: u32,
    /// At most this many pictures a second; 0: as recorded (a GIF: 15).
    pub fps: u32,
    pub quality: Quality,
    pub encoder: Choice,
    pub codec: Codec,
}

impl Default for Options {
    fn default() -> Options {
        Options { format: Format::Mp4, width: 0, fps: 0, quality: Quality::High, encoder: Choice::Auto, codec: Codec::H264 }
    }
}

/// What was made.
#[derive(Debug, Clone, PartialEq)]
pub struct Exported {
    pub seconds: f64,
    pub frames: usize,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub sound: bool,
    /// What coded it ("NVIDIA's GPU (NVENC)"); a GIF's: its own.
    pub encoder: &'static str,
}

/// Where source moment `t` is in what's kept (what's cut collapsed; a
/// moment in a cut is where the next kept stretch starts).
pub fn kept_time(keep: &[(f64, f64)], t: f64) -> f64 {
    keep.iter().map(|&(a, b)| (t - a).clamp(0.0, (b - a).max(0.0))).sum()
}

/// Whether source moment `t` is kept.
pub fn is_kept(keep: &[(f64, f64)], t: f64) -> bool {
    keep.iter().any(|&(a, b)| (a..b).contains(&t))
}

/// `keep` as given, or all of a `seconds`-long video; in order, apart,
/// within it, none empty.
fn tidy(keep: &[(f64, f64)], seconds: f64) -> Vec<(f64, f64)> {
    let mut v: Vec<(f64, f64)> = if keep.is_empty() { vec![(0.0, seconds)] } else { keep.to_vec() };
    v.retain(|(a, b)| a.is_finite() && b.is_finite());
    for r in &mut v {
        *r = (r.0.clamp(0.0, seconds), r.1.clamp(0.0, seconds));
    }
    v.retain(|(a, b)| b > a);
    v.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (a, b) in v {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// The crop within a `w` × `h` picture (whole of it if none), even, at
/// least 2 × 2.
fn crop_within(crop: Option<[u32; 4]>, w: u32, h: u32) -> [u32; 4] {
    let [x, y, cw, ch] = crop.unwrap_or([0, 0, w, h]);
    let (x, y) = (x.min(w.saturating_sub(2)), y.min(h.saturating_sub(2)));
    let (cw, ch) = (cw.min(w - x).max(2), ch.min(h - y).max(2));
    [x, y, cw & !1, ch & !1]
}

/// The pictures' size for `crop` (w, h) at `width` (0: as it is; never
/// larger): even, the crop's shape kept.
pub fn out_size(crop: (u32, u32), width: u32) -> (u32, u32) {
    let (cw, ch) = crop;
    let w = if width == 0 || width >= cw { cw } else { width.max(2) };
    let h = ((u64::from(ch) * u64::from(w) + u64::from(cw) / 2) / u64::from(cw.max(1))) as u32;
    crate::even(w.max(2), h.max(2))
}

/// `src` (RGB, `sw` wide) cut to `crop` and made `dw` × `dh` (RGBA, area
/// averages) into `dst`; `cut` a buffer kept between pictures.
fn fit(src: &[u8], sw: u32, crop: [u32; 4], cut: &mut Vec<u8>, dst: &mut [u8], dw: u32, dh: u32) {
    let [x, y, cw, ch] = crop;
    cut.clear();
    for row in y..y + ch {
        let at = (row as usize * sw as usize + x as usize) * 3;
        cut.extend_from_slice(&src[at..at + cw as usize * 3]);
    }
    crate::read::shrink(cut, cw as usize, ch as usize, dst, dw as usize, dh as usize);
}

/// The recording's own marks (Nero's `chpl`: seconds, title).
pub fn marks_of(path: &Path) -> Vec<(f64, String)> {
    let Some(moov) = crate::trim::moov(path) else { return Vec::new() };
    let Some(chpl) = crate::trim::find(&moov, b"udta").and_then(|u| crate::trim::find(u, b"chpl")) else { return Vec::new() };
    let mut out = Vec::new();
    let (Some(&count), mut at) = (chpl.get(8), 9usize) else { return out };
    for _ in 0..count {
        let Some(start) = chpl.get(at..at + 8).and_then(|b| b.try_into().ok()).map(u64::from_be_bytes) else { break };
        let Some(&n) = chpl.get(at + 8) else { break };
        let Some(title) = chpl.get(at + 9..at + 9 + n as usize) else { break };
        out.push((start as f64 / 1e7, String::from_utf8_lossy(title).into_owned()));
        at += 9 + n as usize;
    }
    out
}

/// `src` edited (`edit`) and made as `opts` say into `dst` (written as
/// `dst.part` and put in place when whole); `progress` told how far (0
/// to 1).
pub fn export(src: &Path, dst: &Path, edit: &Edit, opts: &Options, cancel: &AtomicBool, progress: &dyn Fn(f32)) -> Result<Exported, String> {
    let part = PathBuf::from(format!("{}.part", dst.display()));
    let made = match opts.format {
        Format::Mp4 => to_mp4(src, &part, edit, opts, cancel, progress),
        Format::Gif => {
            // The edited video first (finely: the GIF's colours are what
            // lose), then the GIF of it.
            let video = PathBuf::from(format!("{}.video.part", dst.display()));
            let fps = if opts.fps == 0 { 15 } else { opts.fps.min(50) };
            let first = Options { format: Format::Mp4, fps, quality: Quality::High, encoder: Choice::Cpu, codec: Codec::H264, ..*opts };
            let made = to_mp4(src, &video, &Edit { mute: true, ..edit.clone() }, &first, cancel, &|p| progress(0.6 * p)).and_then(|m| {
                crate::gif::from_mp4(&video, &part, &crate::gif::Options { fps, width: 0 }, cancel, &|p| progress(0.6 + 0.4 * p))
                    .map(|g| Exported { seconds: g.seconds, frames: g.frames, width: g.width, height: g.height, bytes: g.bytes, sound: false, encoder: "GIF" })
                    .map(|e| Exported { seconds: m.seconds, ..e })
            });
            let _ = std::fs::remove_file(&video);
            made
        }
    };
    match made {
        Ok(mut m) => {
            std::fs::rename(&part, dst).map_err(|e| format!("{}: {e}", dst.display()))?;
            m.bytes = std::fs::metadata(dst).map(|m| m.len()).unwrap_or(m.bytes);
            progress(1.0);
            Ok(m)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

fn to_mp4(src: &Path, dst: &Path, edit: &Edit, opts: &Options, cancel: &AtomicBool, progress: &dyn Fn(f32)) -> Result<Exported, String> {
    let video = Video::open(src)?;
    let starts = crate::trim::edit_starts(src).unwrap_or_default();
    // (A trimmed recording plays from its edit list's start: what's before
    // is there only to decode from.)
    let lead = starts.get(&1).map_or(0.0, |&t| f64::from(t) / f64::from(video.timescale().max(1)));
    let seconds = (video.seconds() - lead).max(0.0);
    let keep = tidy(&edit.keep, seconds);
    if keep.is_empty() {
        return Err("nothing is kept".into());
    }
    let total = kept_time(&keep, seconds);
    let crop = crop_within(edit.crop, video.width, video.height);
    let (ow, oh) = out_size((crop[2], crop[3]), opts.width);
    let fps = opts.fps.min(120);
    let mut enc = Encoder::with_quality(ow, oh, if fps == 0 { 30 } else { fps }, opts.encoder, opts.codec, opts.quality)?;
    let mut mp4 = Mp4::create_for(dst, ow, oh, 1000, enc.codec()).map_err(|e| e.to_string())?;
    // An output moment (seconds) to the MP4's ticks: on the frame rate's
    // grid when there is one.
    let ticks = |t: f64| -> u64 { if fps == 0 { (t * 1000.0).round() as u64 } else { ((t * f64::from(fps)).round() * 1000.0 / f64::from(fps)).round() as u64 } };
    let mut rgba = vec![0u8; ow as usize * oh as usize * 4];
    let mut cut = Vec::new();
    let mut frames = 0usize;
    let mut at = 0u64;
    video.each_picture(cancel, &mut |a, b, rgb| {
        let (a, b) = (a - lead, b - lead);
        let (from, to) = (ticks(kept_time(&keep, a)), ticks(kept_time(&keep, b)));
        // (Wholly cut, or between two steps of the frame rate: left out.)
        if to <= from || to <= at {
            return Ok(());
        }
        let from = from.max(at);
        // A gap before it (only at the very start): the first picture
        // shown from 0.
        let from = if frames == 0 { 0 } else { from };
        fit(rgb, video.width, crop, &mut cut, &mut rgba, ow, oh);
        let (bits, key) = enc.encode(&rgba, ow as usize * 4, Order::Rgba)?;
        mp4.add(&bits, key, (to - from) as u32).map_err(|e| e.to_string())?;
        frames += 1;
        at = to;
        progress((kept_time(&keep, b) / total.max(1e-9)) as f32 * if edit.mute { 1.0 } else { 0.9 });
        Ok(())
    })?;
    if frames == 0 {
        return Err("no picture is in what's kept".into());
    }
    // (The last picture held to the end.)
    let end = ticks(total);
    if end > at {
        mp4.extend_last((end - at) as u32);
    }
    let sound = if edit.mute { false } else { sound(src, &mut mp4, &keep, starts.get(&2).copied().unwrap_or(0), cancel)? };
    let marks = edit.marks.clone().unwrap_or_else(|| marks_of(src));
    mp4.chapters(marks.into_iter().filter(|(t, _)| is_kept(&keep, *t)).map(|(t, title)| ((kept_time(&keep, t) * 1000.0).round() as u64, title)).collect());
    mp4.finish().map_err(|e| e.to_string())?;
    Ok(Exported { seconds: total, frames, width: ow, height: oh, bytes: 0, sound, encoder: enc.name() })
}

/// The recording's sound (AAC) in what's kept, coded again into `mp4`;
/// whether it had any. `delay`: the samples before its edit list's start
/// (the encoder's priming).
fn sound(src: &Path, mp4: &mut Mp4, keep: &[(f64, f64)], delay: u32, cancel: &AtomicBool) -> Result<bool, String> {
    use symphonia::core::{
        audio::SampleBuffer,
        codecs::{CODEC_TYPE_AAC, CodecParameters, DecoderOptions},
        formats::Packet,
    };
    let f = File::open(src).map_err(|e| e.to_string())?;
    let size = f.metadata().map_err(|e| e.to_string())?.len();
    let mut r = mp4::Mp4Reader::read_header(BufReader::new(f), size).map_err(|e| e.to_string())?;
    let Some((id, rate, count, config, channels)) =
        r.tracks().iter().find(|(_, t)| t.track_type().ok() == Some(mp4::TrackType::Audio) && t.media_type().ok() == Some(mp4::MediaType::AAC)).map(|(&id, t)| {
            let aot = t.audio_profile().map(|p| p as u8).unwrap_or(2);
            let freq = t.sample_freq_index().map(|f| f as u8).unwrap_or(3);
            let chans = t.channel_config().map(|c| c as u8).unwrap_or(2);
            let config = vec![(aot << 3) | (freq >> 1), ((freq & 1) << 7) | (chans << 3)];
            (id, t.timescale(), t.sample_count(), config, u32::from(chans).clamp(1, 2))
        })
    else {
        return Ok(false);
    };
    let params = CodecParameters::new().for_codec(CODEC_TYPE_AAC).with_sample_rate(rate).with_extra_data(config.into_boxed_slice()).clone();
    let mut dec = symphonia::default::get_codecs().make(&params, &DecoderOptions::default()).map_err(|e| format!("the sound: {e}"))?;
    let mut aac = crate::aac::Aac::new(rate, channels, crate::aac::SOUND_BITRATE)?;
    mp4.with_sound(&aac.track(), rate, crate::aac::SOUND_BITRATE);
    let ch = channels as usize;
    // Kept stretches in samples (a channel's), from the edit list's start.
    let kept: Vec<(u64, u64)> = keep.iter().map(|&(a, b)| ((a * f64::from(rate)).round() as u64, (b * f64::from(rate)).round() as u64)).collect();
    let mut frame: Vec<i16> = Vec::with_capacity(crate::aac::FRAME * ch);
    let mut n = 0u64;
    let mut skip = u64::from(delay);
    let mut buf: Option<SampleBuffer<i16>> = None;
    for i in 1..=count {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let Some(s) = r.read_sample(id, i).map_err(|e| e.to_string())? else { break };
        let decoded = match dec.decode(&Packet::new_from_slice(0, s.start_time, u64::from(s.duration), &s.bytes)) {
            Ok(d) => d,
            // (A frame it can't decode: its moment silent, the rest kept.)
            Err(_) => continue,
        };
        let spec = *decoded.spec();
        let b = buf.get_or_insert_with(|| SampleBuffer::new(decoded.capacity() as u64, spec));
        if b.capacity() < decoded.capacity() {
            *b = SampleBuffer::new(decoded.capacity() as u64, spec);
        }
        b.copy_interleaved_ref(decoded);
        let got = spec.channels.count().max(1);
        for at in b.samples().chunks_exact(got) {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            if kept.iter().any(|&(a, b)| (a..b).contains(&n)) {
                // (Mono from stereo or the other way, as the encoder takes it.)
                match (got, ch) {
                    (1, 2) => frame.extend_from_slice(&[at[0], at[0]]),
                    (g, 1) if g >= 2 => frame.push(((i32::from(at[0]) + i32::from(at[1])) / 2) as i16),
                    _ => frame.extend_from_slice(&at[..ch]),
                }
                if frame.len() == crate::aac::FRAME * ch {
                    if let Some(au) = aac.encode(&frame)? {
                        mp4.add_sound(&au).map_err(|e| e.to_string())?;
                    }
                    frame.clear();
                }
            }
            n += 1;
        }
    }
    if !frame.is_empty() {
        frame.resize(crate::aac::FRAME * ch, 0);
        if let Some(au) = aac.encode(&frame)? {
            mp4.add_sound(&au).map_err(|e| e.to_string())?;
        }
    }
    for au in aac.flush()? {
        mp4.add_sound(&au).map_err(|e| e.to_string())?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_cut_collapses() {
        let keep = [(1.0, 3.0), (5.0, 6.0)];
        assert_eq!(kept_time(&keep, 0.5), 0.0);
        assert_eq!(kept_time(&keep, 2.0), 1.0);
        assert_eq!(kept_time(&keep, 4.0), 2.0);
        assert_eq!(kept_time(&keep, 5.5), 2.5);
        assert_eq!(kept_time(&keep, 9.0), 3.0);
        assert!(is_kept(&keep, 5.2) && !is_kept(&keep, 4.0));
        // Overlapping and out of order, made tidy; outside the video, cut.
        assert_eq!(tidy(&[(5.0, 7.0), (1.0, 3.0), (2.0, 4.0)], 6.0), vec![(1.0, 4.0), (5.0, 6.0)]);
        assert_eq!(tidy(&[], 4.0), vec![(0.0, 4.0)]);
    }

    #[test]
    fn sizes_keep_the_crop_s_shape() {
        assert_eq!(out_size((1280, 800), 0), (1280, 800));
        assert_eq!(out_size((1280, 800), 640), (640, 400));
        // Never larger; odd sizes made even.
        assert_eq!(out_size((1280, 800), 4000), (1280, 800));
        assert_eq!(out_size((601, 301), 0), (600, 300));
        assert_eq!(crop_within(Some([10, 10, 5000, 5000]), 320, 240), [10, 10, 310, 230]);
        assert_eq!(crop_within(None, 321, 241), [0, 0, 320, 240]);
    }

    /// A recording (a red square moving, a second each place, four
    /// places) cut in the middle, cropped and halved: the length, size,
    /// pictures and marks as asked, and the square where it should be.
    #[test]
    fn a_recording_cut_cropped_and_halved() {
        let dir = PathBuf::from(format!("/var/tmp/olo-export-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let (src, dst) = (dir.join("r.mp4"), dir.join("e.mp4"));
        let (w, h) = (320u32, 240u32);
        let mut enc = crate::H264::new(w, h, 30).unwrap();
        let mut mp4 = Mp4::create(&src, w, h, 1000).unwrap();
        for i in 0..4u32 {
            let px: Vec<u8> = (0..w * h)
                .flat_map(|p| {
                    let (x, y) = (p % w, p / w);
                    if (40 + i * 60..80 + i * 60).contains(&x) && (100..140).contains(&y) { [0, 0, 220, 255] } else { [128, 128, 128, 255] }
                })
                .collect();
            let (bits, key) = enc.encode(&px, w as usize * 4, Order::Bgra).unwrap();
            mp4.add(&bits, key, 1000).unwrap();
        }
        mp4.chapters(vec![(500, "first".into()), (1500, "cut".into()), (2500, "third".into())]);
        mp4.finish().unwrap();
        assert_eq!(marks_of(&src).iter().map(|m| m.1.as_str()).collect::<Vec<_>>(), ["first", "cut", "third"]);
        // The second second cut out; the right half kept, halved.
        let edit = Edit { keep: vec![(0.0, 1.0), (2.0, 4.0)], crop: Some([160, 0, 160, 240]), mute: true, marks: None };
        let never = AtomicBool::new(false);
        let made = export(&src, &dst, &edit, &Options { width: 80, encoder: Choice::Cpu, ..Options::default() }, &never, &|_| {}).unwrap();
        assert_eq!((made.width, made.height), (80, 120));
        assert!((made.seconds - 3.0).abs() < 1e-6, "{made:?}");
        // Three pictures: the first, then the third and fourth (the cut's
        // gone).
        assert_eq!(made.frames, 3, "{made:?}");
        assert_eq!(marks_of(&dst), vec![(0.5, "first".to_owned()), (1.5, "third".to_owned())]);
        // Its last picture: the square (x 220..260 of the source) at x 30..50
        // of the half-size crop.
        let v = Video::open(&dst).unwrap();
        assert_eq!((v.width, v.height), (80, 120));
        assert!((v.seconds() - 3.0).abs() < 0.01, "{}", v.seconds());
        let mut last = Vec::new();
        v.each_picture(&never, &mut |_, _, rgb| {
            last = rgb.to_vec();
            Ok(())
        })
        .unwrap();
        let at = |x: usize, y: usize| &last[(y * 80 + x) * 3..(y * 80 + x) * 3 + 3];
        let red = at(40, 60);
        assert!(red[0] > 160 && red[1] < 80, "{red:?}");
        let grey = at(5, 10);
        assert!((100..160).contains(&grey[0]), "{grey:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The sound cut to the sample: a second of 440 Hz then a second of
    /// 880 Hz, half of each kept, comes out a second long, 440 then 880
    /// (where libfdk-aac is installed).
    #[test]
    fn the_sound_is_cut_to_the_sample() {
        use symphonia::core::{
            audio::SampleBuffer,
            codecs::{CODEC_TYPE_AAC, CodecParameters, DecoderOptions},
            formats::Packet,
        };
        let Ok(mut aac) = crate::aac::Aac::new(48_000, 2, 160_000) else { return eprintln!("no libfdk-aac here") };
        let dir = PathBuf::from(format!("/var/tmp/olo-export-sound-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let (src, dst) = (dir.join("r.mp4"), dir.join("e.mp4"));
        let (w, h) = (64u32, 48u32);
        let mut enc = crate::H264::new(w, h, 30).unwrap();
        let mut mp4 = Mp4::create(&src, w, h, 1000).unwrap();
        mp4.with_sound(&aac.track(), 48_000, 160_000);
        for _ in 0..2 {
            let (bits, key) = enc.encode(&vec![128u8; (w * h * 4) as usize], w as usize * 4, Order::Bgra).unwrap();
            mp4.add(&bits, key, 1000).unwrap();
        }
        let tone = |n: usize| -> i16 {
            let f = if n < 48_000 { 440.0 } else { 880.0 };
            ((n as f32 / 48_000.0 * f * std::f32::consts::TAU).sin() * 8000.0) as i16
        };
        for f in 0..(2 * 48_000 / crate::aac::FRAME + 1) {
            let frame: Vec<i16> = (0..crate::aac::FRAME).flat_map(|i| [tone(f * crate::aac::FRAME + i); 2]).collect();
            if let Some(au) = aac.encode(&frame).unwrap() {
                mp4.add_sound(&au).unwrap();
            }
        }
        for au in aac.flush().unwrap() {
            mp4.add_sound(&au).unwrap();
        }
        mp4.finish().unwrap();
        let edit = Edit { keep: vec![(0.5, 1.0), (1.5, 2.0)], ..Edit::default() };
        let never = AtomicBool::new(false);
        let made = export(&src, &dst, &edit, &Options { encoder: Choice::Cpu, ..Options::default() }, &never, &|_| {}).unwrap();
        assert!(made.sound);
        // Decoded back, its priming cut as its edit list says.
        let delay = crate::trim::edit_starts(&dst).unwrap_or_default().get(&2).copied().unwrap_or(0) as usize;
        let f = File::open(&dst).unwrap();
        let size = f.metadata().unwrap().len();
        let mut r = mp4::Mp4Reader::read_header(BufReader::new(f), size).unwrap();
        let (&id, t) = r.tracks().iter().find(|(_, t)| t.track_type().ok() == Some(mp4::TrackType::Audio)).unwrap();
        let count = t.sample_count();
        let params = CodecParameters::new().for_codec(CODEC_TYPE_AAC).with_sample_rate(48_000).with_extra_data(vec![0x11, 0x90].into_boxed_slice()).clone();
        let mut dec = symphonia::default::get_codecs().make(&params, &DecoderOptions::default()).unwrap();
        let mut left: Vec<i16> = Vec::new();
        for i in 1..=count {
            let s = r.read_sample(id, i).unwrap().unwrap();
            let d = dec.decode(&Packet::new_from_slice(0, s.start_time, u64::from(s.duration), &s.bytes)).unwrap();
            let mut b = SampleBuffer::<i16>::new(d.capacity() as u64, *d.spec());
            b.copy_interleaved_ref(d);
            left.extend(b.samples().chunks_exact(2).map(|c| c[0]));
        }
        let left = &left[delay.min(left.len())..];
        assert!((47_000..50_100).contains(&left.len()), "{} samples", left.len());
        // Zero crossings a second, in the middle of each half: twice the
        // frequency.
        let crossings = |s: &[i16]| s.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count() as f32 * 48_000.0 / s.len() as f32 / 2.0;
        let (first, second) = (crossings(&left[4_000..20_000]), crossings(&left[28_000..44_000]));
        assert!((first - 440.0).abs() < 15.0, "{first} Hz, then {second} Hz");
        assert!((second - 880.0).abs() < 25.0, "{first} Hz, then {second} Hz");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
