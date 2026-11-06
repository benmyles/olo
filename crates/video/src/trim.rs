//! A recording trimmed, without encoding it again (`olo-screenshot
//! --trim`, the Viewer's Trim…): its pictures copied from the key frame at
//! or before the start to the one showing at the end, an edit list
//! starting playback exactly at the start; its sound (AAC) copied from a
//! frame before the start (the decoder's overlap) with its own edit list.
//! As fast as copying the file; nothing lost. For Olo's recordings and
//! any H.264, HEVC or AV1 MP4 without reordered frames (B-frames:
//! refused): the samples and the video's sample entries (more than one
//! where a recording's encoder failed and another went on) are copied as
//! they are.

use std::{fs::File, io::BufReader, path::Path};

use crate::{Mp4, aac};

/// What was made.
#[derive(Debug, Clone, PartialEq)]
pub struct Trimmed {
    pub seconds: f64,
    pub frames: usize,
    pub sound: bool,
}

/// `src` from `start` to `end` seconds, into `dst`.
pub fn trim(src: &Path, dst: &Path, start: f64, end: f64) -> Result<Trimmed, String> {
    if !(start >= 0.0 && end > start) {
        return Err("the end must come after the start".into());
    }
    let f = File::open(src).map_err(|e| format!("{}: {e}", src.display()))?;
    let size = f.metadata().map_err(|e| e.to_string())?.len();
    let mut r = mp4::Mp4Reader::read_header(BufReader::new(f), size).map_err(|e| format!("not a video Olo can read: {e}"))?;
    let starts = edit_starts(src).unwrap_or_default();
    let (&vt, video) = r.tracks().iter().find(|(_, t)| t.track_type().ok() == Some(mp4::TrackType::Video)).ok_or("no video in it")?;
    let entries = video_entries(src).ok_or("no video Olo can trim in it (H.264, HEVC or AV1)")?;
    // (The sample entry sample `i` is in.)
    let entry_of = |i: u32| entries.iter().rev().find(|(from, _)| *from <= i).map_or(&entries[0].1, |(_, e)| e);
    let entry = entries[0].1.clone();
    let (vscale, vcount) = (video.timescale(), video.sample_count());
    // (The entry's width and height: the mp4 crate knows them only for
    // the codecs it reads.)
    let (w, h) = (u32::from(u16::from_be_bytes([entry[32], entry[33]])), u32::from(u16::from_be_bytes([entry[34], entry[35]])));
    let sound = r.tracks().iter().find(|(_, t)| t.track_type().ok() == Some(mp4::TrackType::Audio) && t.media_type().ok() == Some(mp4::MediaType::AAC)).map(|(&id, t)| {
        let aot = t.audio_profile().map(|p| p as u8).unwrap_or(2);
        let freq = t.sample_freq_index().map(|f| f as u8).unwrap_or(3);
        let chans = t.channel_config().map(|c| c as u8).unwrap_or(2);
        let config = vec![(aot << 3) | (freq >> 1), ((freq & 1) << 7) | (chans << 3)];
        (id, t.timescale(), t.sample_count(), config, u32::from(chans), t.bitrate())
    });
    // The pictures: from the key frame at or before the start.
    let from_ticks = (start * f64::from(vscale)).round() as u64;
    let to_ticks = (end * f64::from(vscale)).round() as u64;
    let mut samples = Vec::new();
    for i in 1..=vcount {
        let Some(s) = r.read_sample(vt, i).map_err(|e| e.to_string())? else { break };
        if s.rendering_offset != 0 {
            return Err("this video's frames are reordered (B-frames): it can't be trimmed without encoding it again".into());
        }
        let ends = s.start_time + u64::from(s.duration);
        if ends <= from_ticks {
            if s.is_sync {
                samples.clear();
                samples.push((i, s));
            } else if !samples.is_empty() {
                samples.push((i, s));
            }
            continue;
        }
        if s.start_time >= to_ticks {
            break;
        }
        samples.push((i, s));
    }
    // (Before the start: only from its last key frame.)
    let first_key = samples.iter().position(|(_, s)| s.is_sync).ok_or("no key frame before the start")?;
    samples.drain(..first_key);
    let Some((first_i, first)) = samples.first() else { return Err("nothing between the start and the end".into()) };
    let lead = from_ticks.saturating_sub(first.start_time).min(u64::from(u32::MAX)) as u32;
    let mut entry_now = entry_of(*first_i);
    let mut mp4 = Mp4::copying(dst, w, h, vscale, entry_now.clone()).map_err(|e| e.to_string())?;
    let has_sound = sound.is_some();
    let audio_samples = match &sound {
        Some((at, ascale, acount, config, chans, bitrate)) => {
            let rate = *ascale;
            let delay = starts.get(&2).copied().unwrap_or(0) as u64;
            // The sound frame holding the start, and one before it (the
            // decoder's overlap); to the one holding the end.
            let frame = aac::FRAME as u64;
            let m0 = (start * f64::from(rate)).round() as u64 + delay;
            let m1 = (end * f64::from(rate)).round() as u64 + delay;
            let a0 = (m0 / frame).saturating_sub(1);
            let a1 = m1.div_ceil(frame).min(u64::from(*acount));
            let track = aac::Track { config: config.clone(), channels: *chans, delay: (m0 - a0 * frame) as u32 };
            mp4.with_sound(&track, rate, *bitrate);
            let mut aus = Vec::new();
            for i in a0..a1 {
                if let Some(s) = r.read_sample(*at, i as u32 + 1).map_err(|e| e.to_string())? {
                    aus.push(s.bytes.to_vec());
                }
            }
            aus
        }
        None => Vec::new(),
    };
    // Written: the pictures as they were, each shown its time, the last
    // cut at the end; the sound between them.
    let n = samples.len();
    let mut sound = audio_samples.into_iter();
    let per = sound.len().div_ceil(n.max(1)).max(1);
    for (k, (i, s)) in samples.iter().enumerate() {
        let shown_till = (s.start_time + u64::from(s.duration)).min(to_ticks);
        let duration = shown_till.saturating_sub(s.start_time).max(1) as u32;
        // (Another encoder's stream from here: its entry too.)
        if entry_of(*i) != entry_now {
            entry_now = entry_of(*i);
            mp4.next_entry(entry_now.clone()).map_err(|e| e.to_string())?;
        }
        mp4.add_sample(&s.bytes, k == 0 || s.is_sync, duration).map_err(|e| e.to_string())?;
        for au in sound.by_ref().take(per) {
            mp4.add_sound(&au).map_err(|e| e.to_string())?;
        }
    }
    for au in sound {
        mp4.add_sound(&au).map_err(|e| e.to_string())?;
    }
    mp4.start_at(lead);
    mp4.finish().map_err(|e| e.to_string())?;
    Ok(Trimmed { seconds: end - start, frames: n, sound: has_sound })
}

/// The video track's first sample entry (the whole box: `avc1`, `hvc1`,
/// `av01`, …), read from the file's boxes.
#[cfg(test)]
pub(crate) fn video_entry(path: &Path) -> Option<Vec<u8>> {
    video_entries(path)?.into_iter().next().map(|(_, e)| e)
}

/// The video track's sample entries (whole boxes), each from its first
/// sample (counted from 1): one, or more where a recording's encoder
/// failed and another's stream went on (`Mp4::new_entry`). Read from
/// `moov` alone.
pub(crate) fn video_entries(path: &Path) -> Option<Vec<(u32, Vec<u8>)>> {
    let moov = moov(path)?;
    let u32_at = |b: &[u8], at: usize| b.get(at..at + 4).and_then(|x| x.try_into().ok()).map(u32::from_be_bytes);
    let mut at = 0;
    while let Some((kind, body, next)) = child(&moov, at) {
        at = next;
        if kind != *b"trak" {
            continue;
        }
        let mdia = find(body, b"mdia")?;
        if find(mdia, b"hdlr").and_then(|h| h.get(8..12)) != Some(b"vide") {
            continue;
        }
        let stbl = find(mdia, b"minf").and_then(|m| find(m, b"stbl"))?;
        let stsd = find(stbl, b"stsd")?;
        // Version, flags and the count, then the entries, whole.
        let mut entries = Vec::new();
        let mut e = 8;
        for _ in 0..u32_at(stsd, 4)? {
            let size = u32_at(stsd, e)? as usize;
            entries.push(stsd.get(e..e + size)?.to_vec());
            e += size;
        }
        let first = entries.first()?;
        if crate::Codec::of_entry(first.get(4..8)?).is_none() || first.len() < 36 + 8 {
            return None;
        }
        // stsc's runs (first chunk, samples a chunk, entry) to each
        // entry's first sample.
        let chunks = find(stbl, b"stco").or_else(|| find(stbl, b"co64")).and_then(|c| u32_at(c, 4)).unwrap_or(0);
        let stsc = find(stbl, b"stsc")?;
        let runs: Vec<(u32, u32, u32)> = (0..u32_at(stsc, 4)? as usize).filter_map(|i| Some((u32_at(stsc, 8 + i * 12)?, u32_at(stsc, 12 + i * 12)?, u32_at(stsc, 16 + i * 12)?))).collect();
        let mut out: Vec<(u32, Vec<u8>)> = Vec::new();
        let mut sample = 1u32;
        for (i, &(chunk, per, entry)) in runs.iter().enumerate() {
            let entry = entries.get(entry.saturating_sub(1) as usize)?;
            if out.last().is_none_or(|(_, last)| last != entry) {
                out.push((sample, entry.clone()));
            }
            let end = runs.get(i + 1).map_or(chunks + 1, |r| r.0);
            sample += end.saturating_sub(chunk) * per;
        }
        if out.is_empty() {
            out.push((1, first.clone()));
        }
        return Some(out);
    }
    None
}

/// An MP4's `moov` box, read on its own (seeking past the pictures).
pub(crate) fn moov(path: &Path) -> Option<Vec<u8>> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut at = 0u64;
    while at + 8 <= len {
        f.seek(SeekFrom::Start(at)).ok()?;
        let mut h = [0u8; 16];
        f.read_exact(&mut h[..8]).ok()?;
        let (mut size, mut head) = (u64::from(u32::from_be_bytes(h[0..4].try_into().ok()?)), 8u64);
        if size == 1 {
            f.read_exact(&mut h[8..16]).ok()?;
            size = u64::from_be_bytes(h[8..16].try_into().ok()?);
            head = 16;
        } else if size == 0 {
            size = len - at;
        }
        if size < head {
            return None;
        }
        if &h[4..8] == b"moov" {
            let mut body = vec![0u8; (size - head) as usize];
            f.read_exact(&mut body).ok()?;
            return Some(body);
        }
        at += size;
    }
    None
}

/// Each track's edit list's media start (`elst`'s first media time), by
/// handler: 1 for video, 2 for sound. Read from the file's boxes.
pub(crate) fn edit_starts(path: &Path) -> Option<std::collections::HashMap<u8, u32>> {
    let moov = moov(path)?;
    let moov = moov.as_slice();
    let mut out = std::collections::HashMap::new();
    let mut at = 0;
    while let Some((kind, body, next)) = child(moov, at) {
        at = next;
        if kind != *b"trak" {
            continue;
        }
        let handler = find(body, b"mdia").and_then(|m| find(m, b"hdlr")).and_then(|h| h.get(8..12)).map(|t| match t {
            b"vide" => 1,
            b"soun" => 2,
            _ => 0,
        })?;
        if let Some(elst) = find(body, b"edts").and_then(|e| find(e, b"elst")) {
            // version 0: flags, count, then duration and media time.
            if elst.len() >= 16 && elst[0] == 0 {
                out.insert(handler, u32::from_be_bytes([elst[12], elst[13], elst[14], elst[15]]));
            }
        }
    }
    Some(out)
}

/// The first box `kind` among `data`'s children: its body.
pub(crate) fn find<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    let mut at = 0;
    while let Some((k, body, next)) = child(data, at) {
        if k == *kind {
            return Some(body);
        }
        at = next;
    }
    None
}

/// The box at `at` in `data`: its kind, body and where the next starts.
fn child(data: &[u8], at: usize) -> Option<([u8; 4], &[u8], usize)> {
    let head = data.get(at..at + 8)?;
    let mut size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
    let kind = [head[4], head[5], head[6], head[7]];
    let mut body_at = at + 8;
    if size == 1 {
        let large = data.get(at + 8..at + 16)?;
        size = u64::from_be_bytes(large.try_into().ok()?) as usize;
        body_at = at + 16;
    } else if size == 0 {
        size = data.len() - at;
    }
    let end = at.checked_add(size)?.min(data.len());
    Some((kind, data.get(body_at..end)?, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{H264, Order};

    /// Four seconds, a picture a second (its shade the second), a key frame
    /// every two: trimmed from 1.5 to 3.2 s, read back.
    #[test]
    fn a_recording_trimmed() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-trim-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let (src, dst) = (dir.join("r.mp4"), dir.join("t.mp4"));
        let (w, h) = (64u32, 48u32);
        let mut enc = H264::new(w, h, 1).unwrap();
        let mut mp4 = Mp4::create(&src, w, h, 1000).unwrap();
        for i in 0..4u8 {
            let px: Vec<u8> = (0..w * h).flat_map(|_| [i * 60, i * 60, i * 60, 255]).collect();
            let (bits, key) = enc.encode(&px, w as usize * 4, Order::Bgra).unwrap();
            mp4.add(&bits, key, 1000).unwrap();
        }
        mp4.finish().unwrap();
        let t = trim(&src, &dst, 1.5, 3.2).unwrap();
        assert!(t.frames >= 3, "{t:?}");
        let f = std::fs::File::open(&dst).unwrap();
        let size = f.metadata().unwrap().len();
        let r = mp4::Mp4Reader::read_header(std::io::BufReader::new(f), size).unwrap();
        let v = r.tracks().values().next().unwrap();
        // The copy starts at a key frame at or before 1.5 s, and plays
        // from 1.5 s by its edit list: 1.7 s long.
        let starts = edit_starts(&dst).unwrap();
        assert!(starts.get(&1).is_some_and(|&s| s == 500 || s == 1500), "{starts:?}");
        assert!((1650..=1750).contains(&r.duration().as_millis()), "{:?}", r.duration());
        assert!(v.sample_count() >= 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_backwards() {
        assert!(trim(Path::new("/nonexistent"), Path::new("/tmp/x"), 3.0, 1.0).is_err());
    }
}
