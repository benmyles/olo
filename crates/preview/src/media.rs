//! Audio and video: tags, cover art and facts (lofty), and a poster frame
//! for H.264 video decoded in process (mp4 / Matroska demuxing, OpenH264).
//! Playback lives in `audio.rs`.

use std::{
    io::BufReader,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use lofty::prelude::*;
use openh264::formats::YUVSource;
use tabula_kit::egui::ColorImage;

use super::load::{Body, Image, Update, color_image};

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
}

pub fn format_duration(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

pub fn audio(path: &Path) -> Result<Update, String> {
    let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let mut m = Media {
        picture: None,
        title: name,
        subtitle: String::new(),
        rows: Vec::new(),
        duration: None,
        playable: true,
        video: false,
        note: "Audio".into(),
    };
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
    let mut m = Media {
        picture: None,
        title: name,
        subtitle: String::new(),
        rows: Vec::new(),
        duration: None,
        playable: false,
        video: true,
        note: "Video".into(),
    };
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
    let Some((id, codec)) = video else {
        m.note = "MPEG-4 file".into();
        return Ok(());
    };
    let t = &r.tracks()[&id];
    let (w, h, fps) = (t.width(), t.height(), t.frame_rate());
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
    m.note = format!("{codec} video, {w} × {h}");
    if codec != "H.264" {
        return Ok(());
    }
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
            video = Some((t.track_number().get(), id.to_owned(), v.pixel_width().get(), v.pixel_height().get(), t.codec_private().map(<[u8]>::to_vec)));
        } else if audio.is_none() && id.starts_with("A_") {
            audio = Some(codec_name(id));
        }
    }
    let Some((track, id, w, h, private)) = video else {
        m.note = "Matroska file".into();
        return Ok(());
    };
    let codec = codec_name(&id);
    m.rows.push(("Size".into(), format!("{w} × {h}")));
    m.rows.push(("Format".into(), match audio {
        Some(a) => format!("{codec}; {a} audio"),
        None => codec.to_string(),
    }));
    m.note = format!("{codec} video, {w} × {h}");
    if id != "V_MPEG4/ISO/AVC" {
        return Ok(());
    }
    let Some((len, sets)) = private.as_deref().and_then(parse_avcc) else { return Ok(()) };
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
fn parse_avcc(b: &[u8]) -> Option<(usize, Vec<Vec<u8>>)> {
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
fn avcc_to_annexb(data: &[u8], len: usize, out: &mut Vec<u8>) {
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

fn yuv_image(yuv: &openh264::decoder::DecodedYUV<'_>) -> Image {
    let (w, h) = yuv.dimensions();
    let mut rgba = vec![0u8; w * h * 4];
    yuv.write_rgba8(&mut rgba);
    Arc::new(ColorImage::from_rgba_unmultiplied([w, h], &rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

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
