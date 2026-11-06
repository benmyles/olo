//! Audio and video: tags, cover art and facts (lofty), and a poster frame
//! for H.264 video decoded in process (mp4 / Matroska demuxing, OpenH264);
//! `video` plays it.
//! Playback lives in `audio.rs`.

use std::{
    io::BufReader,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::Bitmap;
use lofty::prelude::*;
use openh264::formats::YUVSource;

use super::load::{Body, Image, Update, color_image};

#[derive(Clone)]
pub struct Media {
    /// Cover art or the video's poster frame.
    pub picture: Option<Image>,
    pub title: String,
    pub subtitle: String,
    pub rows: Vec<(String, String)>,
    pub duration: Option<f64>,
    /// Audio that can be played in the preview.
    pub playable: bool,
    pub video: bool,
    pub note: String,
    /// Its chapters (an MP4's `chpl`: Olo's recordings' marks): where
    /// each starts (seconds) and its title.
    pub chapters: Vec<(f64, String)>,
}

pub fn format_duration(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

pub fn audio(path: &Path) -> Result<Update, String> {
    let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let mut m = Media { picture: None, title: name, subtitle: String::new(), rows: Vec::new(), duration: None, playable: true, video: false, note: "Audio".into(), chapters: Vec::new() };
    if let Ok(f) = lofty::read_from_path(path) {
        let p = f.properties();
        let d = p.duration().as_secs_f64();
        if d > 0.0 {
            m.duration = Some(d);
            m.rows.push(("Length".into(), format_duration(d)));
        }
        if let Some(tag) = f.primary_tag().or_else(|| f.first_tag()) {
            if let Some(t) = tag.title().filter(|t| !t.trim().is_empty()) {
                m.title = t.into_owned();
            }
            let artist = tag.artist().map(|a| a.into_owned()).unwrap_or_default();
            let album = tag.album().map(|a| a.into_owned()).unwrap_or_default();
            m.subtitle = match (artist.is_empty(), album.is_empty()) {
                (false, false) => format!("{artist} — {album}"),
                (false, true) => artist,
                (true, false) => album,
                _ => String::new(),
            };
            if let Some(y) = tag.date() {
                m.rows.push(("Year".into(), y.year.to_string()));
            }
            if let Some(g) = tag.genre() {
                m.rows.push(("Genre".into(), g.into_owned()));
            }
            m.picture = tag.pictures().iter().find_map(|pic| image::load_from_memory(pic.data()).ok()).map(|img| {
                let img = if img.width().max(img.height()) > 1200 { img.thumbnail(1200, 1200) } else { img };
                color_image(img.to_rgba8())
            });
        }
        let codec = match f.file_type() {
            lofty::file::FileType::Mpeg => "MP3",
            lofty::file::FileType::Flac => "FLAC",
            lofty::file::FileType::Opus => "Opus",
            lofty::file::FileType::Vorbis => "Ogg Vorbis",
            lofty::file::FileType::Wav => "WAV",
            lofty::file::FileType::Mp4 => "MPEG-4 audio",
            lofty::file::FileType::Aiff => "AIFF",
            lofty::file::FileType::Aac => "AAC",
            _ => "Audio",
        };
        let mut fmt = codec.to_string();
        if let Some(k) = p.audio_bitrate().filter(|&k| k > 0) {
            fmt += &format!(", {k} kbps");
        }
        if let Some(hz) = p.sample_rate() {
            fmt += &format!(", {:.1} kHz", hz as f64 / 1000.0);
        }
        m.rows.push(("Format".into(), fmt));
        m.note = format!("{codec} audio");
    }
    Ok(Update::Body(Body::Media(m)))
}

pub fn video(path: &Path, cancel: &Arc<AtomicBool>) -> Result<Update, String> {
    let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let mut m = Media { picture: None, title: name, subtitle: String::new(), rows: Vec::new(), duration: None, playable: false, video: true, note: "Video".into(), chapters: Vec::new() };
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let res = if matches!(ext.as_str(), "mkv" | "webm") { mkv(path, &mut m, cancel) } else { mp4(path, &mut m, cancel) };
    if let Err(e) = res {
        tracing::debug!("video {}: {e}", path.display());
    }
    if let Some(d) = m.duration {
        m.rows.insert(0, ("Length".into(), format_duration(d)));
    }
    Ok(Update::Body(Body::Media(m)))
}

fn mp4(path: &Path, m: &mut Media, cancel: &Arc<AtomicBool>) -> Result<(), String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let size = f.metadata().map_err(|e| e.to_string())?.len();
    if let Ok(mut r) = std::fs::File::open(path) {
        m.chapters = mp4_chapters(&mut r, size);
    }
    let mut r = mp4::Mp4Reader::read_header(BufReader::new(f), size).map_err(|e| e.to_string())?;
    let mut video = None;
    let mut audio = None;
    for (id, t) in r.tracks() {
        match t.media_type() {
            Ok(mp4::MediaType::H264) => video = Some((*id, "H.264")),
            Ok(mp4::MediaType::H265) if video.is_none() => video = Some((*id, "HEVC")),
            Ok(mp4::MediaType::VP9) if video.is_none() => video = Some((*id, "VP9")),
            Ok(mp4::MediaType::AAC) => audio = Some("AAC"),
            _ => {}
        }
    }
    let d = r.duration().as_secs_f64();
    if d > 0.0 {
        m.duration = Some(d);
    }
    // AV1, which the mp4 crate doesn't know.
    let av1 = if video.is_none() { std::fs::File::open(path).ok().and_then(|mut f| mp4_av1(&mut f, size)) } else { None };
    if let Some(a) = &av1 {
        video = Some((a.id, "AV1"));
    }
    // HEVC: `hvc1` the crate doesn't know, `hev1` whose `hvcC` it skips.
    let hevc = if video.is_none_or(|v| v.1 == "HEVC") { std::fs::File::open(path).ok().and_then(|mut f| mp4_hevc(&mut f, size)) } else { None };
    if let Some(hv) = &hevc {
        video = Some((hv.id, "HEVC"));
    }
    let Some((id, codec)) = video else {
        m.note = "MPEG-4 file".into();
        return Ok(());
    };
    let t = &r.tracks()[&id];
    let (w, h) = match (&av1, &hevc) {
        (Some(a), _) => (a.width, a.height),
        (_, Some(hv)) => (hv.width, hv.height),
        _ => (t.width(), t.height()),
    };
    let fps = t.frame_rate();
    m.rows.push(("Size".into(), format!("{w} × {h}")));
    let mut fmt = codec.to_string();
    if fps > 0.0 {
        fmt += format!(", {fps:.3}").trim_end_matches('0').trim_end_matches('.');
        fmt += " fps";
    }
    if let Some(a) = audio {
        fmt += &format!("; {a} audio");
    }
    m.rows.push(("Format".into(), fmt));
    // (HEVC's as its stream says too, where the container doesn't.)
    let colour =
        av1.as_ref().map(|a| a.colour).or_else(|| hevc.as_ref().map(|hv| olo_images::hevc::config(&hv.record).map_or(hv.colour, |c| olo_images::hevc::colour_of(&c, hv.colour)))).unwrap_or_default();
    m.rows.extend(colour_row(&colour));
    m.note = format!("{codec} video, {w} × {h}");
    if let Some(a) = av1 {
        m.playable = true;
        // The first picture that decodes, a tenth of the way in.
        let mut dec = olo_images::av1::Decoder::new(1, 1)?;
        let count = t.sample_count();
        let syncs = t.trak.mdia.minf.stbl.stss.as_ref().map(|s| s.entries.clone()).unwrap_or_else(|| vec![1]);
        let start = syncs.iter().copied().filter(|&s| s <= (count / 10).max(1)).max().unwrap_or(1);
        let mut out = Vec::new();
        for sid in start..=(start + 120).min(count) {
            if cancel.load(Ordering::Relaxed) || !out.is_empty() {
                break;
            }
            let Ok(Some(sample)) = r.read_sample(id, sid) else { continue };
            let data = if sid == start { [a.obus.as_slice(), &sample.bytes].concat() } else { sample.bytes.to_vec() };
            dec.decode(&data, &mut out)?;
        }
        if let Some(f) = out.into_iter().next().map(|f| f.with_colour(&a.colour)) {
            m.picture = Some(Arc::new(Bitmap::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.to_rgba())));
        }
        return Ok(());
    }
    if let Some(config) = hevc.as_ref().and_then(|hv| olo_images::hevc::config(&hv.record)) {
        m.playable = true;
        // A key frame a tenth of the way in.
        let count = t.sample_count();
        let syncs = t.trak.mdia.minf.stbl.stss.as_ref().map(|s| s.entries.clone()).unwrap_or_else(|| vec![1]);
        let start = syncs.iter().copied().filter(|&s| s <= (count / 10).max(1)).max().unwrap_or(1);
        let colour = hevc.as_ref().map(|hv| hv.colour).unwrap_or_default();
        let mut dec = olo_images::hevc::Decoder::new(config, colour, 1);
        let mut out = Vec::new();
        for sid in start..=(start + 120).min(count) {
            if cancel.load(Ordering::Relaxed) || !out.is_empty() {
                break;
            }
            let Ok(Some(sample)) = r.read_sample(id, sid) else { continue };
            let _ = dec.decode(&sample.bytes, &mut out);
        }
        if out.is_empty() {
            dec.flush(&mut out);
        }
        if let Some(f) = out.first() {
            m.picture = Some(Arc::new(Bitmap::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.to_rgba())));
        }
        return Ok(());
    }
    if codec == "VP9" {
        m.playable = true;
        // A key frame a tenth of the way in, which decodes on its own.
        let count = t.sample_count();
        let syncs = t.trak.mdia.minf.stbl.stss.as_ref().map(|s| s.entries.clone()).unwrap_or_else(|| vec![1]);
        let start = syncs.iter().copied().filter(|&s| s <= (count / 10).max(1)).max().unwrap_or(1);
        let mut dec = olo_images::vp9::Decoder::new(1);
        let mut out = Vec::new();
        for sid in start..=(start + 120).min(count) {
            if cancel.load(Ordering::Relaxed) || !out.is_empty() {
                break;
            }
            let Ok(Some(sample)) = r.read_sample(id, sid) else { continue };
            dec.decode(&sample.bytes, 0, &mut out)?;
        }
        if let Some((_, f)) = out.first() {
            m.picture = Some(Arc::new(Bitmap::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.to_rgba())));
        }
        return Ok(());
    }
    if codec != "H.264" {
        return Ok(());
    }
    m.playable = true;
    let sps = t.sequence_parameter_set().map_err(|e| e.to_string())?.to_vec();
    let pps = t.picture_parameter_set().map_err(|e| e.to_string())?.to_vec();
    let count = t.sample_count();
    // A key frame a tenth of the way in (the first is often black).
    let syncs = t.trak.mdia.minf.stbl.stss.as_ref().map(|s| s.entries.clone()).unwrap_or_else(|| vec![1]);
    let target = (count / 10).max(1);
    let start = syncs.iter().copied().filter(|&s| s <= target).max().unwrap_or(1);
    let mut dec = openh264::decoder::Decoder::new().map_err(|e| e.to_string())?;
    let mut annexb = Vec::new();
    for sid in start..=(start + 60).min(count) {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let Ok(Some(sample)) = r.read_sample(id, sid) else { continue };
        annexb.clear();
        if sid == start {
            for ps in [&sps, &pps] {
                annexb.extend_from_slice(&[0, 0, 0, 1]);
                annexb.extend_from_slice(ps);
            }
        }
        avcc_to_annexb(&sample.bytes, 4, &mut annexb);
        if let Ok(Some(yuv)) = dec.decode(&annexb) {
            m.picture = Some(yuv_image(&yuv));
            break;
        }
    }
    Ok(())
}

fn mkv(path: &Path, m: &mut Media, cancel: &Arc<AtomicBool>) -> Result<(), String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut mk = matroska_demuxer::MatroskaFile::open(BufReader::new(f)).map_err(|e| e.to_string())?;
    let scale = mk.info().timestamp_scale().get() as f64;
    if let Some(d) = mk.info().duration() {
        m.duration = Some(d * scale / 1e9);
    }
    let mut video = None;
    let mut audio = None;
    for t in mk.tracks() {
        let id = t.codec_id();
        if let Some(v) = t.video()
            && video.is_none()
        {
            video = Some((t.track_number().get(), id.to_owned(), v.pixel_width().get(), v.pixel_height().get(), t.codec_private().map(<[u8]>::to_vec), mkv_colour(v)));
        } else if audio.is_none() && id.starts_with("A_") {
            audio = Some(codec_name(id));
        }
    }
    let Some((track, id, w, h, private, colour)) = video else {
        m.note = "Matroska file".into();
        return Ok(());
    };
    // (HEVC's as its stream says too, where the track doesn't.)
    let colour = match private.as_deref().filter(|_| id == "V_MPEGH/ISO/HEVC").and_then(olo_images::hevc::config) {
        Some(c) => olo_images::hevc::colour_of(&c, colour),
        None => colour,
    };
    let codec = codec_name(&id);
    m.rows.push(("Size".into(), format!("{w} × {h}")));
    m.rows.push((
        "Format".into(),
        match audio {
            Some(a) => format!("{codec}; {a} audio"),
            None => codec.to_string(),
        },
    ));
    m.rows.extend(colour_row(&colour));
    m.note = format!("{codec} video, {w} × {h}");
    if id == "V_AV1" {
        m.playable = true;
        return av1_poster(&mut mk, track, colour, m, cancel);
    }
    if id == "V_VP9" {
        m.playable = true;
        return vp9_poster(&mut mk, track, colour, m, cancel);
    }
    if id == "V_MPEGH/ISO/HEVC" {
        let Some(config) = private.as_deref().and_then(olo_images::hevc::config) else { return Ok(()) };
        m.playable = true;
        return hevc_poster(&mut mk, track, config, colour, m, cancel);
    }
    if id != "V_MPEG4/ISO/AVC" {
        return Ok(());
    }
    let Some((len, sets)) = private.as_deref().and_then(parse_avcc) else { return Ok(()) };
    m.playable = true;
    let mut dec = openh264::decoder::Decoder::new().map_err(|e| e.to_string())?;
    let mut frame = matroska_demuxer::Frame::default();
    let mut annexb = Vec::new();
    let mut first = true;
    let mut tries = 0;
    while mk.next_frame(&mut frame).unwrap_or(false) {
        if cancel.load(Ordering::Relaxed) || tries > 120 {
            break;
        }
        if frame.track != track {
            continue;
        }
        tries += 1;
        annexb.clear();
        if first {
            for ps in &sets {
                annexb.extend_from_slice(&[0, 0, 0, 1]);
                annexb.extend_from_slice(ps);
            }
            first = false;
        }
        avcc_to_annexb(&frame.data, len, &mut annexb);
        if let Ok(Some(yuv)) = dec.decode(&annexb) {
            m.picture = Some(yuv_image(&yuv));
            break;
        }
    }
    Ok(())
}

/// The first AV1 picture.
fn av1_poster<R: std::io::Read + std::io::Seek>(
    mk: &mut matroska_demuxer::MatroskaFile<R>,
    track: u64,
    colour: olo_images::hevc::Colour,
    m: &mut Media,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    let mut dec = olo_images::av1::Decoder::new(1, 1)?;
    let mut frame = matroska_demuxer::Frame::default();
    let mut out = Vec::new();
    let mut tries = 0;
    while out.is_empty() && tries < 120 && mk.next_frame(&mut frame).unwrap_or(false) {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        if frame.track == track {
            tries += 1;
            dec.decode(&frame.data, &mut out)?;
        }
    }
    if let Some(f) = out.into_iter().next().map(|f| f.with_colour(&colour)) {
        m.picture = Some(Arc::new(Bitmap::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.to_rgba())));
    }
    Ok(())
}

/// The first VP9 picture.
fn vp9_poster<R: std::io::Read + std::io::Seek>(
    mk: &mut matroska_demuxer::MatroskaFile<R>,
    track: u64,
    colour: olo_images::hevc::Colour,
    m: &mut Media,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    let mut dec = olo_images::vp9::Decoder::new(1);
    let mut frame = matroska_demuxer::Frame::default();
    let mut out = Vec::new();
    let mut tries = 0;
    while out.is_empty() && tries < 120 && mk.next_frame(&mut frame).unwrap_or(false) {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        if frame.track == track {
            tries += 1;
            dec.decode(&frame.data, 0, &mut out)?;
        }
    }
    if let Some(f) = out.into_iter().next().map(|(_, f)| f.with_colour(&colour)) {
        m.picture = Some(Arc::new(Bitmap::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.to_rgba())));
    }
    Ok(())
}

/// The first HEVC picture.
fn hevc_poster<R: std::io::Read + std::io::Seek>(
    mk: &mut matroska_demuxer::MatroskaFile<R>,
    track: u64,
    config: olo_images::hevc::Config,
    colour: olo_images::hevc::Colour,
    m: &mut Media,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    let mut dec = olo_images::hevc::Decoder::new(config, colour, 1);
    let mut frame = matroska_demuxer::Frame::default();
    let mut out = Vec::new();
    let mut tries = 0;
    while out.is_empty() && tries < 120 && mk.next_frame(&mut frame).unwrap_or(false) {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        if frame.track == track {
            tries += 1;
            let _ = dec.decode(&frame.data, &mut out);
        }
    }
    if out.is_empty() {
        dec.flush(&mut out);
    }
    if let Some(f) = out.first() {
        m.picture = Some(Arc::new(Bitmap::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.to_rgba())));
    }
    Ok(())
}

/// "Colour" among a video's facts, when it's more than SDR's sRGB-ish:
/// "HDR (PQ), BT.2020, mastered to 1,000 cd/m²", "HDR (HLG), BT.2020",
/// "Display P3". (From the container; a stream that says it only in
/// itself, AV1's sequence header, is shown right all the same.)
pub(crate) fn colour_row(c: &olo_images::hevc::Colour) -> Option<(String, String)> {
    use olo_images::hdr::{PRI_BT2020, PRI_P3_D65, TRC_HLG, TRC_PQ};
    let mut parts = Vec::new();
    match c.transfer {
        Some(TRC_PQ) => parts.push("HDR (PQ)".to_owned()),
        Some(TRC_HLG) => parts.push("HDR (HLG)".to_owned()),
        _ => {}
    }
    match c.primaries {
        Some(PRI_BT2020) => parts.push("BT.2020".into()),
        Some(PRI_P3_D65) => parts.push("Display P3".into()),
        _ => {}
    }
    if !parts.is_empty()
        && let Some(peak) = c.peak.filter(|_| matches!(c.transfer, Some(TRC_PQ | TRC_HLG)))
    {
        let n = peak.round() as u64;
        let n = if n >= 1000 { format!("{},{:03}", n / 1000, n % 1000) } else { n.to_string() };
        parts.push(format!("mastered to {n} cd/m²"));
    }
    (!parts.is_empty()).then(|| ("Colour".into(), parts.join(", ")))
}

/// A Matroska video track's colour (H.273's codes: matrix, primaries,
/// transfer; full range; how bright it was mastered: MaxCLL, else the
/// mastering display's peak).
pub(crate) fn mkv_colour(v: &matroska_demuxer::Video) -> olo_images::hevc::Colour {
    use matroska_demuxer::{MatrixCoefficients as M, Range};
    let Some(c) = v.colour() else { return Default::default() };
    // (Back to H.273's codes, those that change how it's shown.)
    use matroska_demuxer::{Primaries as P, TransferCharacteristics as T};
    let primaries = c.primaries().and_then(|p| match p {
        P::Bt709 => Some(1),
        P::Bt2020 => Some(9),
        P::SmpteRp432_2 => Some(11),
        P::SmpteEg432_2 => Some(12),
        _ => None,
    });
    let transfer = c.transfer_characteristics().and_then(|t| match t {
        T::Bt709 => Some(1),
        T::Smpte170 => Some(6),
        T::Iec61966_2_1 => Some(13),
        T::Bt220_10 => Some(14),
        T::Bt220_12 => Some(15),
        T::Bt2100 => Some(16),
        T::Hlg => Some(18),
        _ => None,
    });
    let peak = c.max_cll().map(|v| v as f32).filter(|&v| v > 0.0).or_else(|| c.mastering_metadata().and_then(|m| m.luminance_max()).map(|v| v as f32).filter(|&v| v > 0.0));
    let matrix = c.matrix_coefficients().and_then(|m| match m {
        M::Identity => Some(0),
        M::Bt709 => Some(1),
        M::Fcc73682 => Some(4),
        M::Bt470bg => Some(5),
        M::Smpte170 => Some(6),
        M::Smpte240 => Some(7),
        M::YCoCg => Some(8),
        M::Bt2020Ncl => Some(9),
        M::Bt2020Cl => Some(10),
        M::SmpteSt2085 => Some(11),
        M::ChromaDerivedNcl => Some(12),
        M::ChromaDerivedCl => Some(13),
        M::Bt2100 => Some(14),
        M::Unknown => None,
    });
    olo_images::hevc::Colour { matrix, full_range: c.range() == Some(Range::Full), primaries, transfer, peak }
}

/// An MPEG-4 file's AV1 track (an `av01` sample entry, which the mp4
/// crate passes over, though it keeps the track's sample tables): its id,
/// picture size and the sequence header OBUs of its `av1C`.
pub(crate) struct Av1Track {
    pub id: u32,
    pub width: u16,
    pub height: u16,
    pub obus: Vec<u8>,
    /// What its sample entry says of its colour (the stream's sequence
    /// header usually says it too, and is believed first).
    pub colour: olo_images::hevc::Colour,
}

/// A visual sample entry's colour, from its boxes (`boxes`, after its 78
/// bytes of fields): `colr` (nclx, ISO's, or nclc, QuickTime's, limited
/// range: H.273's primaries, transfer and matrix, then nclx's range flag)
/// and how bright it was mastered, `clli`'s MaxCLL, else `mdcv`'s
/// display peak (in 1/10,000 cd/m²).
fn entry_colour(boxes: &[u8]) -> olo_images::hevc::Colour {
    let mut colour = olo_images::hevc::Colour::default();
    let code = |c: &[u8], at: usize| c.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as u32).filter(|&v| v != 2);
    if let Some(c) = children(boxes, b"colr").next()
        && matches!(c.get(0..4), Some(b"nclx" | b"nclc"))
    {
        colour.primaries = code(c, 4);
        colour.transfer = code(c, 6);
        colour.matrix = code(c, 8);
        colour.full_range = &c[0..4] == b"nclx" && c.get(10).is_some_and(|b| b & 0x80 != 0);
    }
    let cll = children(boxes, b"clli").next().and_then(|c| c.get(0..2)).map(|b| u16::from_be_bytes([b[0], b[1]]) as f32).filter(|&v| v > 0.0);
    let mastered = children(boxes, b"mdcv").next().and_then(|c| c.get(16..20)).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f32 / 10_000.0).filter(|&v| v > 0.0);
    colour.peak = cll.or(mastered);
    colour
}

/// The AV1 track of the MPEG-4 file `r` (`len` bytes long), found by
/// walking its boxes to each track's sample description.
pub(crate) fn mp4_av1<R: std::io::Read + std::io::Seek>(r: &mut R, len: u64) -> Option<Av1Track> {
    let moov = moov(r, len)?;
    for trak in children(&moov, b"trak") {
        // tkhd: version and flags, two times (four or eight bytes each),
        // then the track's id.
        let tkhd = children(trak, b"tkhd").next()?;
        let off = if tkhd.first() == Some(&1) { 20 } else { 12 };
        let id = u32::from_be_bytes(tkhd.get(off..off + 4)?.try_into().ok()?);
        let Some(stsd) = within(trak, &[b"mdia", b"minf", b"stbl", b"stsd"]) else { continue };
        // Version, flags and the entry count, then the entries.
        for (kind, entry) in boxes(stsd.get(8..)?) {
            if &kind != b"av01" {
                continue;
            }
            // A visual sample entry's 78 bytes of fields (its width and
            // height 24 bytes in), then its boxes; av1C's four bytes of
            // fields, then the OBUs.
            let width = u16::from_be_bytes(entry.get(24..26)?.try_into().ok()?);
            let height = u16::from_be_bytes(entry.get(26..28)?.try_into().ok()?);
            let av1c = children(entry.get(78..)?, b"av1C").next()?;
            return Some(Av1Track { id, width, height, obus: av1c.get(4..)?.to_vec(), colour: entry_colour(entry.get(78..)?) });
        }
    }
    None
}

/// An MPEG-4 file's chapters, from Nero's `chpl` in `moov`'s `udta`
/// (version 1: four reserved bytes, a count, then each start in 100 ns
/// units and a title of up to 255 bytes).
pub(crate) fn mp4_chapters<R: std::io::Read + std::io::Seek>(r: &mut R, len: u64) -> Vec<(f64, String)> {
    let Some(moov) = moov(r, len) else { return Vec::new() };
    let Some(chpl) = within(&moov, &[b"udta", b"chpl"]) else { return Vec::new() };
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

/// An MPEG-4 file's `moov` box (`r`, `len` bytes long), read whole: it
/// holds only tables, and is small next to the pictures.
fn moov<R: std::io::Read + std::io::Seek>(r: &mut R, len: u64) -> Option<Vec<u8>> {
    use std::io::SeekFrom;
    // The top-level boxes, as far as `moov` (read whole: it holds only
    // tables, and is small next to the pictures).
    let mut at = 0u64;
    loop {
        if at + 8 > len {
            return None;
        }
        r.seek(SeekFrom::Start(at)).ok()?;
        let mut h = [0u8; 16];
        r.read_exact(&mut h[..8]).ok()?;
        let (mut size, mut head) = (u32::from_be_bytes(h[0..4].try_into().ok()?) as u64, 8);
        if size == 1 {
            r.read_exact(&mut h[8..16]).ok()?;
            (size, head) = (u64::from_be_bytes(h[8..16].try_into().ok()?), 16);
        } else if size == 0 {
            size = len - at;
        }
        if size < head {
            return None;
        }
        if &h[4..8] == b"moov" {
            if size > 64 << 20 {
                return None;
            }
            let mut b = vec![0; (size - head) as usize];
            r.read_exact(&mut b).ok()?;
            return Some(b);
        }
        at += size;
    }
}

/// An MPEG-4 file's HEVC track (an `hvc1` or `hev1` sample entry, whose
/// `hvcC` the mp4 crate reads only the version of): its id, picture size,
/// decoder configuration record and colour (its `colr`, when it has one).
pub(crate) struct HevcTrack {
    pub id: u32,
    pub width: u16,
    pub height: u16,
    pub record: Vec<u8>,
    pub colour: olo_images::hevc::Colour,
}

/// The HEVC track of the MPEG-4 file `r` (`len` bytes long).
pub(crate) fn mp4_hevc<R: std::io::Read + std::io::Seek>(r: &mut R, len: u64) -> Option<HevcTrack> {
    let moov = moov(r, len)?;
    for trak in children(&moov, b"trak") {
        let tkhd = children(trak, b"tkhd").next()?;
        let off = if tkhd.first() == Some(&1) { 20 } else { 12 };
        let id = u32::from_be_bytes(tkhd.get(off..off + 4)?.try_into().ok()?);
        let Some(stsd) = within(trak, &[b"mdia", b"minf", b"stbl", b"stsd"]) else { continue };
        for (kind, entry) in boxes(stsd.get(8..)?) {
            if &kind != b"hvc1" && &kind != b"hev1" {
                continue;
            }
            let width = u16::from_be_bytes(entry.get(24..26)?.try_into().ok()?);
            let height = u16::from_be_bytes(entry.get(26..28)?.try_into().ok()?);
            let inner = entry.get(78..)?;
            let record = children(inner, b"hvcC").next()?.to_vec();
            return Some(HevcTrack { id, width, height, record, colour: entry_colour(inner) });
        }
    }
    None
}

/// The boxes laid end to end in `b`: each one's type and contents.
fn boxes(b: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    let mut at = 0;
    std::iter::from_fn(move || {
        let h = b.get(at..at + 8)?;
        let size = u32::from_be_bytes(h[0..4].try_into().ok()?) as usize;
        let kind: [u8; 4] = h[4..8].try_into().ok()?;
        let (head, size) = match size {
            1 => (16, u64::from_be_bytes(b.get(at + 8..at + 16)?.try_into().ok()?) as usize),
            0 => (8, b.len() - at),
            n => (8, n),
        };
        if size < head || at + size > b.len() {
            return None;
        }
        let body = &b[at + head..at + size];
        at += size;
        Some((kind, body))
    })
}

fn children<'a>(b: &'a [u8], kind: &'a [u8; 4]) -> impl Iterator<Item = &'a [u8]> {
    boxes(b).filter(move |(k, _)| k == kind).map(|(_, body)| body)
}

/// The box at `path` (each the first of its type) inside `b`.
fn within<'a>(mut b: &'a [u8], path: &[&'a [u8; 4]]) -> Option<&'a [u8]> {
    for k in path {
        b = children(b, k).next()?;
    }
    Some(b)
}

fn codec_name(id: &str) -> &'static str {
    match id {
        "V_MPEG4/ISO/AVC" => "H.264",
        "V_MPEGH/ISO/HEVC" => "HEVC",
        "V_VP8" => "VP8",
        "V_VP9" => "VP9",
        "V_AV1" => "AV1",
        "A_AAC" => "AAC",
        "A_OPUS" => "Opus",
        "A_VORBIS" => "Vorbis",
        "A_FLAC" => "FLAC",
        "A_AC3" | "A_EAC3" => "Dolby Digital",
        "A_MPEG/L3" => "MP3",
        _ => "Video",
    }
}

/// The NAL length size and parameter sets in an avcC record.
pub(crate) fn parse_avcc(b: &[u8]) -> Option<(usize, Vec<Vec<u8>>)> {
    if b.len() < 7 || b[0] != 1 {
        return None;
    }
    let len = (b[4] & 3) as usize + 1;
    let mut sets = Vec::new();
    let mut at = 5;
    for mask in [0x1f, 0xff] {
        let n = (*b.get(at)? & mask) as usize;
        at += 1;
        for _ in 0..n {
            let l = u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]) as usize;
            sets.push(b.get(at + 2..at + 2 + l)?.to_vec());
            at += 2 + l;
        }
    }
    Some((len, sets))
}

/// Length-prefixed NAL units (as stored in mp4/mkv) to start-code form.
pub(crate) fn avcc_to_annexb(data: &[u8], len: usize, out: &mut Vec<u8>) {
    let mut at = 0;
    while at + len <= data.len() {
        let n = data[at..at + len].iter().fold(0usize, |a, &b| a << 8 | b as usize);
        at += len;
        let end = (at + n).min(data.len());
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&data[at..end]);
        at = end;
    }
}

pub(crate) fn yuv_image(yuv: &openh264::decoder::DecodedYUV<'_>) -> Image {
    let (w, h) = yuv.dimensions();
    let mut rgba = vec![0u8; w * h * 4];
    yuv.write_rgba8(&mut rgba);
    Arc::new(Bitmap::from_rgba_unmultiplied([w, h], &rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An HDR10 sample entry's boxes (as phones and encoders write them:
    /// `colr` nclx BT.2020, PQ, BT.2020's matrix, limited range; `clli`;
    /// `mdcv`) read as the picture's colour; MaxCLL over the mastering
    /// display's peak, which counts only when there's no MaxCLL.
    #[test]
    fn hdr10_sample_entry_colour() {
        let mk = |kind: &[u8; 4], body: &[u8]| [&((body.len() + 8) as u32).to_be_bytes()[..], kind, body].concat();
        let colr = mk(b"colr", &[b"nclx".as_slice(), &[0, 9, 0, 16, 0, 9, 0x00]].concat());
        let clli = mk(b"clli", &[0x03, 0xE8, 0x01, 0x90]);
        let mut mdcv = vec![0u8; 24];
        mdcv[16..20].copy_from_slice(&(4000u32 * 10_000).to_be_bytes());
        let mdcv = mk(b"mdcv", &mdcv);
        let c = entry_colour(&[colr.clone(), clli, mdcv.clone()].concat());
        assert_eq!((c.primaries, c.transfer, c.matrix, c.full_range, c.peak), (Some(9), Some(16), Some(9), false, Some(1000.0)));
        assert_eq!(entry_colour(&[colr, mdcv].concat()).peak, Some(4000.0));
        // HLG, full range, QuickTime's nclc (no range byte: limited).
        let hlg = entry_colour(&mk(b"colr", &[b"nclx".as_slice(), &[0, 9, 0, 18, 0, 9, 0x80]].concat()));
        assert_eq!((hlg.transfer, hlg.full_range), (Some(18), true));
        // Unspecified (2) says nothing.
        let none = entry_colour(&mk(b"colr", &[b"nclc".as_slice(), &[0, 2, 0, 2, 0, 2]].concat()));
        assert_eq!((none.primaries, none.transfer, none.matrix), (None, None, None));
        // Among the facts.
        assert_eq!(colour_row(&c).map(|r| r.1).as_deref(), Some("HDR (PQ), BT.2020, mastered to 1,000 cd/m²"));
        assert_eq!(colour_row(&hlg).map(|r| r.1).as_deref(), Some("HDR (HLG), BT.2020"));
        assert_eq!(colour_row(&none), None);
    }

    #[test]
    fn avcc_conversion() {
        let mut out = Vec::new();
        avcc_to_annexb(&[0, 0, 0, 2, 0x65, 0xaa, 0, 0, 0, 1, 0x41], 4, &mut out);
        assert_eq!(out, vec![0, 0, 0, 1, 0x65, 0xaa, 0, 0, 0, 1, 0x41]);
        let rec = [1, 0x64, 0, 0x1f, 0xff, 0xe1, 0, 2, 0x67, 1, 1, 0, 1, 0x68];
        let (len, sets) = parse_avcc(&rec).unwrap();
        assert_eq!(len, 4);
        assert_eq!(sets, vec![vec![0x67, 1], vec![0x68]]);
        assert_eq!(format_duration(3725.0), "1:02:05");
        assert_eq!(format_duration(65.4), "1:05");
    }
}
