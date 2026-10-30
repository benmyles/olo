//! The frames a screen drew, kept beside its history of actions
//! (`history`): `olo-screen record NAME` runs with each screen (in its
//! process group, so it ends with it), looks at the screen eight times a
//! second for a few seconds after each action and once a second
//! otherwise, and keeps only what changed: the 64-pixel tiles that differ
//! from the frame before, zstd-compressed, with a whole frame every 30 s
//! (a keyframe, to start playing from). Frames go in segments of five
//! minutes, `frames-START.olof` in the history's folder, the oldest
//! dropped past `MAX_BYTES`.
//!
//! A segment is `MAGIC`, then records: a little-endian u32 length and
//! that many bytes: when (u64 ms), the frame's width and height (u32),
//! whether it is a keyframe (u8), its tile count (u32), and each tile's
//! x, y, width, height (u16) and compressed length (u32) before its bytes.

use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crate::{history, screens};

const MAGIC: &[u8; 8] = b"OLOFRM1\n";
const TILE: usize = 64;
/// How often the screen is looked at, after an action and otherwise.
const BUSY: Duration = Duration::from_millis(125);
const IDLE: Duration = Duration::from_secs(1);
/// How long after an action it is looked at often.
const BUSY_FOR: Duration = Duration::from_secs(5);
const KEY_EVERY: Duration = Duration::from_secs(30);
const SEGMENT_MS: u64 = 5 * 60 * 1000;
/// The most a screen's frames take on disk.
const MAX_BYTES: u64 = 512 << 20;

/// A whole frame: its size and RGBA rows.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl Frame {
    /// One of the screen's pictures (`ImageFormat::Rgba`): width, height,
    /// then the rows.
    fn decode(bytes: &[u8]) -> Option<Frame> {
        let width = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?) as usize;
        let height = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
        let rgba = bytes.get(8..8 + width * height * 4)?.to_vec();
        Some(Frame { width, height, rgba })
    }

    /// The tiles where `self` differs from `before` (all of them when it
    /// is None or another size), as (x, y, w, h).
    fn changed(&self, before: Option<&Frame>) -> Vec<(usize, usize, usize, usize)> {
        let same_size = before.is_some_and(|b| b.width == self.width && b.height == self.height);
        let mut out = Vec::new();
        for ty in (0..self.height).step_by(TILE) {
            for tx in (0..self.width).step_by(TILE) {
                let (w, h) = (TILE.min(self.width - tx), TILE.min(self.height - ty));
                let differs = !same_size
                    || (ty..ty + h).any(|y| {
                        let row = (y * self.width + tx) * 4;
                        self.rgba[row..row + w * 4] != before.unwrap().rgba[row..row + w * 4]
                    });
                if differs {
                    out.push((tx, ty, w, h));
                }
            }
        }
        out
    }

    fn tile(&self, (x, y, w, h): (usize, usize, usize, usize)) -> Vec<u8> {
        let mut out = Vec::with_capacity(w * h * 4);
        for row in y..y + h {
            let at = (row * self.width + x) * 4;
            out.extend_from_slice(&self.rgba[at..at + w * 4]);
        }
        out
    }

    fn put_tile(&mut self, (x, y, w, h): (usize, usize, usize, usize), pixels: &[u8]) {
        for (i, row) in (y..y + h).enumerate() {
            let at = (row * self.width + x) * 4;
            if let (Some(dst), Some(src)) = (self.rgba.get_mut(at..at + w * 4), pixels.get(i * w * 4..(i + 1) * w * 4)) {
                dst.copy_from_slice(src);
            }
        }
    }
}

/// One frame record in a segment, as the index has it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Entry {
    pub at: u64,
    pub key: bool,
    offset: u64,
}

/// A segment's file and its frames.
#[derive(Debug, Clone)]
pub struct Segment {
    pub path: PathBuf,
    pub frames: Vec<Entry>,
}

fn segments_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "olof")).collect()).unwrap_or_default();
    v.sort();
    v
}

/// Screen `name`'s frames, segment by segment, oldest first (read from
/// the records' heads: their pixels are left on disk).
pub fn index(name: &str) -> Vec<Segment> {
    segments_in(&history::dir(name)).into_iter().filter_map(|path| Some(Segment { frames: index_segment(&path)?, path })).collect()
}

fn index_segment(path: &Path) -> Option<Vec<Entry>> {
    let mut f = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let mut magic = [0u8; 8];
    f.read_exact(&mut magic).ok()?;
    if &magic != MAGIC {
        return None;
    }
    let mut out = Vec::new();
    let mut offset = 8u64;
    loop {
        let mut head = [0u8; 4 + 8 + 4 + 4 + 1];
        if f.read_exact(&mut head).is_err() {
            break;
        }
        let len = u32::from_le_bytes(head[0..4].try_into().ok()?) as u64;
        let at = u64::from_le_bytes(head[4..12].try_into().ok()?);
        out.push(Entry { at, key: head[20] == 1, offset });
        offset += 4 + len;
        if f.seek(SeekFrom::Start(offset)).is_err() {
            break;
        }
    }
    Some(out)
}

/// The record at `offset` in `path`: when, and its tiles, applied to
/// `frame` (made the record's size if it isn't).
fn apply(path: &Path, offset: u64, frame: &mut Frame) -> Option<u64> {
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(SeekFrom::Start(offset)).ok()?;
    let mut len = [0u8; 4];
    f.read_exact(&mut len).ok()?;
    let mut rec = vec![0u8; u32::from_le_bytes(len) as usize];
    f.read_exact(&mut rec).ok()?;
    let at = u64::from_le_bytes(rec.get(0..8)?.try_into().ok()?);
    let (w, h) = (u32::from_le_bytes(rec.get(8..12)?.try_into().ok()?) as usize, u32::from_le_bytes(rec.get(12..16)?.try_into().ok()?) as usize);
    if frame.width != w || frame.height != h {
        *frame = Frame { width: w, height: h, rgba: vec![0; w * h * 4] };
    }
    let tiles = u32::from_le_bytes(rec.get(17..21)?.try_into().ok()?) as usize;
    let mut p = 21;
    let u16_at = |p: usize| rec.get(p..p + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    for _ in 0..tiles {
        let (x, y, tw, th) = (u16_at(p)?, u16_at(p + 2)?, u16_at(p + 4)?, u16_at(p + 6)?);
        let clen = u32::from_le_bytes(rec.get(p + 8..p + 12)?.try_into().ok()?) as usize;
        let pixels = zstd::decode_all(rec.get(p + 12..p + 12 + clen)?).ok()?;
        frame.put_tile((x, y, tw, th), &pixels);
        p += 12 + clen;
    }
    Some(at)
}

/// The screen as it was at `at` (ms since the epoch): the last frame
/// recorded at or before then, built up from its keyframe. None before
/// the first.
pub fn frame_at(segments: &[Segment], at: u64) -> Option<(u64, Frame)> {
    let seg = segments.iter().rev().find(|s| s.frames.first().is_some_and(|f| f.at <= at))?;
    let last = seg.frames.iter().rposition(|f| f.at <= at)?;
    let key = seg.frames[..=last].iter().rposition(|f| f.key)?;
    let mut frame = Frame { width: 0, height: 0, rgba: Vec::new() };
    let mut when = 0;
    for e in &seg.frames[key..=last] {
        when = apply(&seg.path, e.offset, &mut frame)?;
    }
    Some((when, frame))
}

/// Plays frames forward: each record applied once, in order, going back
/// to a keyframe only on a jump back or into another segment.
pub struct Player<'a> {
    segments: &'a [Segment],
    /// The segment and record the frame is up to.
    at: Option<(usize, usize)>,
    pub frame: Frame,
}

impl<'a> Player<'a> {
    pub fn new(segments: &'a [Segment]) -> Player<'a> {
        Player { segments, at: None, frame: Frame { width: 0, height: 0, rgba: Vec::new() } }
    }

    /// The screen as it was at `t`; None before the first frame.
    pub fn at(&mut self, t: u64) -> Option<&Frame> {
        let s = self.segments.iter().rposition(|s| s.frames.first().is_some_and(|f| f.at <= t))?;
        let seg = &self.segments[s];
        let last = seg.frames.iter().rposition(|f| f.at <= t)?;
        let from = match self.at {
            Some((ps, pi)) if ps == s && pi <= last => pi + 1,
            _ => seg.frames[..=last].iter().rposition(|f| f.key)?,
        };
        for e in &seg.frames[from..=last] {
            apply(&seg.path, e.offset, &mut self.frame)?;
        }
        self.at = Some((s, last));
        Some(&self.frame)
    }
}

/// Keeps a screen's frames: each one looked at, its changes written.
struct Writer {
    dir: PathBuf,
    file: Option<(std::fs::File, u64)>,
    last: Option<Frame>,
    last_key: Option<Instant>,
}

impl Writer {
    fn add(&mut self, frame: Frame, at: u64) -> std::io::Result<()> {
        // A new segment every few minutes, starting with a keyframe.
        if self.file.as_ref().is_none_or(|(_, start)| at.saturating_sub(*start) >= SEGMENT_MS) {
            std::fs::create_dir_all(&self.dir)?;
            let path = self.dir.join(format!("frames-{at:016}.olof"));
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
            f.write_all(MAGIC)?;
            self.file = Some((f, at));
            self.last = None;
            self.trim();
        }
        let key = self.last.is_none() || self.last_key.is_none_or(|k| k.elapsed() >= KEY_EVERY);
        let tiles = frame.changed(if key { None } else { self.last.as_ref() });
        if tiles.is_empty() {
            return Ok(());
        }
        let mut rec = Vec::with_capacity(64 << 10);
        rec.extend_from_slice(&at.to_le_bytes());
        rec.extend_from_slice(&(frame.width as u32).to_le_bytes());
        rec.extend_from_slice(&(frame.height as u32).to_le_bytes());
        rec.push(u8::from(key));
        rec.extend_from_slice(&(tiles.len() as u32).to_le_bytes());
        for t in &tiles {
            let packed = zstd::encode_all(&frame.tile(*t)[..], 3)?;
            for v in [t.0, t.1, t.2, t.3] {
                rec.extend_from_slice(&(v as u16).to_le_bytes());
            }
            rec.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            rec.extend_from_slice(&packed);
        }
        let (f, _) = self.file.as_mut().unwrap();
        f.write_all(&(rec.len() as u32).to_le_bytes())?;
        f.write_all(&rec)?;
        if key {
            self.last_key = Some(Instant::now());
        }
        self.last = Some(frame);
        Ok(())
    }

    /// Drop the oldest segments while the frames take more than allowed.
    fn trim(&self) {
        let segs = segments_in(&self.dir);
        let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let mut total: u64 = segs.iter().map(|p| size(p)).sum();
        for p in &segs[..segs.len().saturating_sub(1)] {
            if total <= MAX_BYTES {
                break;
            }
            total -= size(p);
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Record screen `name` until it is gone.
pub fn run(name: &str) -> Result<(), String> {
    let screen = screens::get(name)?;
    let picture = screens::dir().join("views").join(format!("record-{name}.rgba"));
    let _ = std::fs::create_dir_all(picture.parent().unwrap());
    let actions = history::dir(name).join("actions.jsonl");
    let mut w = Writer { dir: history::dir(name), file: None, last: None, last_key: None };
    let mut failures = 0;
    loop {
        let began = Instant::now();
        if !screen.alive() {
            break;
        }
        match screen.picture(&picture, None, true, olo_protocol::ImageFormat::Rgba).and_then(|()| std::fs::read(&picture).map_err(|e| e.to_string())) {
            Ok(bytes) => {
                failures = 0;
                if let Some(frame) = Frame::decode(&bytes)
                    && let Err(e) = w.add(frame, history::now_ms())
                {
                    tracing::warn!("recording {name}: {e}");
                }
            }
            Err(e) => {
                failures += 1;
                if failures > 20 {
                    return Err(e);
                }
            }
        }
        // Often for a while after an action, seldom otherwise.
        let acted = std::fs::metadata(&actions).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
        let period = if acted.is_some_and(|a| a < BUSY_FOR) { BUSY } else { IDLE };
        std::thread::sleep(period.saturating_sub(began.elapsed()));
    }
    let _ = std::fs::remove_file(&picture);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: usize, h: usize, fill: u8) -> Frame {
        Frame { width: w, height: h, rgba: vec![fill; w * h * 4] }
    }

    #[test]
    fn keeps_what_changed_and_plays_it_back() {
        let dir = tempdir();
        let mut w = Writer { dir: dir.clone(), file: None, last: None, last_key: None };
        let a = frame(200, 130, 10);
        w.add(a.clone(), 1_000).unwrap();
        // One pixel in the second tile row: only that tile goes.
        let mut b = a.clone();
        b.rgba[(70 * 200 + 5) * 4] = 99;
        assert_eq!(b.changed(Some(&a)), vec![(0, 64, 64, 64)]);
        w.add(b.clone(), 2_000).unwrap();
        // Nothing new: nothing kept.
        w.add(b.clone(), 3_000).unwrap();
        let segs = vec![Segment { frames: index_segment(&segments_in(&dir)[0]).unwrap(), path: segments_in(&dir)[0].clone() }];
        assert_eq!(segs[0].frames.iter().map(|f| (f.at, f.key)).collect::<Vec<_>>(), vec![(1_000, true), (2_000, false)]);
        assert_eq!(frame_at(&segs, 1_500).unwrap(), (1_000, a));
        assert_eq!(frame_at(&segs, 9_000).unwrap(), (2_000, b));
        assert!(frame_at(&segs, 500).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn edges_are_partial_tiles() {
        let f = frame(130, 70, 0);
        let all = f.changed(None);
        assert_eq!(all.len(), 3 * 2);
        assert!(all.contains(&(128, 64, 2, 6)));
    }

    fn tempdir() -> PathBuf {
        let d = std::path::PathBuf::from("/var/tmp").join(format!("olo-record-test-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }
}
