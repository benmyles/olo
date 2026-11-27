//! On-disk `.olof` frame segment codec (`OLOFRM1\n`).
//!
//! A segment is `MAGIC`, then records: a little-endian u32 length and
//! that many bytes: when (u64 ms), the frame's width and height (u32),
//! whether it is a keyframe (u8), its tile count (u32), and each tile's
//! x, y, width, height (u16) and compressed length (u32) before its bytes.

use std::{
    collections::HashMap,
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::{MAX_RECORD_BYTES, canonical_grid_count, checked_local_frame_bytes, decode_zstd_exact, encode_zstd_bounded, validate_canonical_tile};

#[cfg(target_os = "linux")]
const O_NONBLOCK_FLAG: i32 = 0x800;
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const O_NOFOLLOW_FLAG: i32 = 0x10000;
#[cfg(all(target_os = "linux", not(target_arch = "aarch64")))]
const O_NOFOLLOW_FLAG: i32 = 0x20000;
#[cfg(target_os = "linux")]
const O_CLOEXEC_FLAG: i32 = 0x80000;

/// Open `path` read-only without blocking on FIFOs or following symlinks,
/// verifying via `fstat` that the opened descriptor is a regular file.
fn open_regular_ro(path: &Path) -> Option<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(target_os = "linux")]
    opts.custom_flags(O_NONBLOCK_FLAG | O_NOFOLLOW_FLAG | O_CLOEXEC_FLAG);
    let file = opts.open(path).ok()?;
    if !file.metadata().ok()?.file_type().is_file() {
        return None;
    }
    Some(file)
}

/// True only when `dir` exists as a real directory (not a symlink).
fn is_real_dir(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_dir())
}

const MAGIC: &[u8; 8] = b"OLOFRM1\n";
pub(crate) const TILE: usize = 64;
const KEY_EVERY: Duration = Duration::from_secs(30);
const SEGMENT_MS: u64 = 5 * 60 * 1000;
const RECORD_HEADER_BYTES: usize = 21;

/// A whole frame: its size and RGBA rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl Frame {
    /// One of the screen's pictures (`ImageFormat::Rgba`): width, height,
    /// then the rows.
    pub fn decode_rgba(bytes: &[u8]) -> Option<Frame> {
        let width = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?) as usize;
        let height = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
        let pixel_bytes = checked_local_frame_bytes(width, height).ok()?;
        let end = 8usize.checked_add(pixel_bytes)?;
        let rgba = bytes.get(8..end)?.to_vec();
        Some(Frame { width, height, rgba })
    }

    /// The tiles where `self` differs from `before` (all of them when it
    /// is None or another size), as (x, y, w, h).
    pub fn changed(&self, before: Option<&Frame>) -> Vec<(usize, usize, usize, usize)> {
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

    /// Copy the RGBA pixels of tile `(x, y, w, h)` into a contiguous buffer.
    pub fn tile(&self, (x, y, w, h): (usize, usize, usize, usize)) -> Vec<u8> {
        let mut out = Vec::with_capacity(w * h * 4);
        for row in y..y + h {
            let at = (row * self.width + x) * 4;
            out.extend_from_slice(&self.rgba[at..at + w * 4]);
        }
        out
    }

    /// Copy contiguous RGBA `pixels` into tile `(x, y, w, h)` of `self`.
    pub fn put_tile(&mut self, (x, y, w, h): (usize, usize, usize, usize), pixels: &[u8]) {
        for (i, row) in (y..y + h).enumerate() {
            let at = (row * self.width + x) * 4;
            if let (Some(dst), Some(src)) = (self.rgba.get_mut(at..at + w * 4), pixels.get(i * w * 4..(i + 1) * w * 4)) {
                dst.copy_from_slice(src);
            }
        }
    }
}

/// One frame record in a segment, as the index has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub at: u64,
    pub key: bool,
    offset: u64,
}

/// A segment's file and its frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub path: PathBuf,
    pub frames: Vec<Entry>,
}

/// A lightweight timestamp + keyframe mark for wire transport and scrubber UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameMark {
    pub at: u64,
    pub key: bool,
}

/// Source-neutral index of recorded frame timestamps and the screen's last size.
///
/// Local sources build this from [`Segment`]s; remote sources receive it over
/// the wire without exposing local filesystem paths or byte offsets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameIndex {
    /// Sorted oldest-first.
    pub frames: Vec<FrameMark>,
    /// The size of the screen's last picture.
    pub last_size: Option<(usize, usize)>,
}

impl FrameIndex {
    pub fn from_segments(segments: &[Segment]) -> FrameIndex {
        let total: usize = segments.iter().map(|s| s.frames.len()).sum();
        let mut frames = Vec::with_capacity(total);
        for seg in segments {
            for e in &seg.frames {
                frames.push(FrameMark { at: e.at, key: e.key });
            }
        }
        FrameIndex { frames, last_size: last_size(segments) }
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn first(&self) -> Option<u64> {
        self.frames.first().map(|f| f.at)
    }

    pub fn last(&self) -> Option<u64> {
        self.frames.last().map(|f| f.at)
    }

    pub fn bounds(&self) -> Option<(u64, u64)> {
        Some((self.first()?, self.last()?))
    }

    /// The time of the frame showing the screen at `t` (frames are in time order).
    pub fn frame_time(&self, t: u64) -> Option<u64> {
        let i = self.frames.partition_point(|f| f.at <= t);
        self.frames.get(i.checked_sub(1)?).map(|f| f.at)
    }

    /// Timestamp of the first recorded frame strictly after `t`.
    pub fn next_after(&self, t: u64) -> Option<u64> {
        let i = self.frames.partition_point(|f| f.at <= t);
        self.frames.get(i).map(|f| f.at)
    }
}

fn segments_in(dir: &Path) -> Vec<PathBuf> {
    if !is_real_dir(dir) {
        return Vec::new();
    }
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|d| d.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_file())).map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "olof")).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// The frames in history folder `dir`, segment by segment, oldest first (read
/// from the records' heads: their pixels are left on disk). Empty (header-only)
/// segments, symlinks, and non-regular files are omitted so segment search
/// stays monotonic and never hangs on a FIFO.
pub fn index_dir(dir: &Path) -> Vec<Segment> {
    segments_in(dir)
        .into_iter()
        .filter_map(|path| {
            let frames = index_segment_entries(&path)?;
            (!frames.is_empty()).then_some(Segment { frames, path })
        })
        .collect()
}

/// The frames in `dir` as [`index_dir`] has them, reading again only the
/// segments that changed since `known` had them (a screen running adds to
/// its newest only).
pub fn index_known_in(dir: &Path, known: &mut HashMap<PathBuf, (u64, Vec<Entry>)>) -> Vec<Segment> {
    let paths = segments_in(dir);
    known.retain(|p, _| paths.contains(p));
    paths
        .into_iter()
        .filter_map(|path| {
            let meta = std::fs::symlink_metadata(&path).ok()?;
            if !meta.file_type().is_file() {
                return None;
            }
            let len = meta.len();
            let frames = match known.get(&path) {
                Some((l, frames)) if *l == len => frames.clone(),
                _ => {
                    let frames = index_segment_entries(&path)?;
                    known.insert(path.clone(), (len, frames.clone()));
                    frames
                }
            };
            (!frames.is_empty()).then_some(Segment { path, frames })
        })
        .collect()
}

/// Cached per-segment state for incremental indexing (avoiding full-file
/// rescans on append while catching equal-size inode replacements and
/// truncations).
#[derive(Debug, Clone)]
struct CachedSeg {
    dev: u64,
    ino: u64,
    parsed_offset: u64,
    file_len: u64,
    frames: Arc<Vec<Entry>>,
    last_size: Option<(usize, usize)>,
}

/// Result of an incremental scan via [`SegmentCache::scan`].
#[derive(Debug, Clone)]
pub struct IncrementalScan {
    pub segments: Vec<Segment>,
    pub last_size: Option<(usize, usize)>,
    /// True if any segment was added, removed, replaced, truncated, or grew.
    pub changed: bool,
    /// True if an existing segment was removed, replaced (inode changed), or truncated.
    pub replaced: bool,
    /// Number of newly scanned records during this call (useful for verification).
    pub scanned_records: usize,
}

/// Incremental `.olof` directory indexer keyed by `(dev, ino)` and parsed byte
/// offset so appending a frame only reads the newly written record(s) and
/// tracks `last_size` directly from record headers without reopening files.
#[derive(Debug, Default)]
pub struct SegmentCache {
    entries: HashMap<PathBuf, CachedSeg>,
}

impl SegmentCache {
    pub fn new() -> SegmentCache {
        SegmentCache::default()
    }

    pub fn scan(&mut self, dir: &Path) -> IncrementalScan {
        let paths = segments_in(dir);
        let prev_len = self.entries.len();
        self.entries.retain(|p, _| paths.contains(p));
        let mut replaced = self.entries.len() < prev_len;
        let mut changed = replaced;
        let mut scanned_records = 0usize;

        let mut segments = Vec::with_capacity(paths.len());
        let mut last_size: Option<(usize, usize)> = None;

        for path in paths {
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            if !meta.file_type().is_file() {
                continue;
            }
            let (dev, ino, len) = (meta.dev(), meta.ino(), meta.len());
            let mut updated: Option<CachedSeg> = None;

            if let Some(cur) = self.entries.get(&path) {
                if cur.dev == dev && cur.ino == ino && len == cur.file_len {
                    if !cur.frames.is_empty() {
                        if cur.last_size.is_some() {
                            last_size = cur.last_size;
                        }
                        segments.push(Segment { path, frames: (*cur.frames).clone() });
                    }
                    continue;
                } else if cur.dev == dev && cur.ino == ino && len > cur.file_len {
                    // Pure append on the same inode: seek straight to `cur.parsed_offset`.
                    if let Some(mut scanned) = scan_segment_from(&path, cur.parsed_offset)
                        && scanned.dev == dev
                        && scanned.ino == ino
                    {
                        scanned_records += scanned.entries.len();
                        let mut frames = (*cur.frames).clone();
                        let grew = !scanned.entries.is_empty();
                        frames.append(&mut scanned.entries);
                        let seg_last = scanned.last_size.or(cur.last_size);
                        if grew {
                            changed = true;
                        }
                        updated = Some(CachedSeg { dev: scanned.dev, ino: scanned.ino, parsed_offset: scanned.next_offset, file_len: scanned.file_len, frames: Arc::new(frames), last_size: seg_last });
                    }
                } else {
                    // Different inode or truncated: full rescan and mark replaced.
                    replaced = true;
                    changed = true;
                }
            } else {
                changed = true;
            }

            let seg_entry = match updated {
                Some(u) => u,
                None => {
                    let Some(scanned) = scan_segment_from(&path, 0) else {
                        self.entries.remove(&path);
                        continue;
                    };
                    scanned_records += scanned.entries.len();
                    CachedSeg { dev: scanned.dev, ino: scanned.ino, parsed_offset: scanned.next_offset, file_len: scanned.file_len, frames: Arc::new(scanned.entries), last_size: scanned.last_size }
                }
            };
            if !seg_entry.frames.is_empty() {
                if seg_entry.last_size.is_some() {
                    last_size = seg_entry.last_size;
                }
                segments.push(Segment { path: path.clone(), frames: (*seg_entry.frames).clone() });
            }
            self.entries.insert(path, seg_entry);
        }

        IncrementalScan { segments, last_size, changed, replaced, scanned_records }
    }
}

struct ScannedSegment {
    entries: Vec<Entry>,
    next_offset: u64,
    last_size: Option<(usize, usize)>,
    dev: u64,
    ino: u64,
    file_len: u64,
}

/// Scan records in regular `.olof` file `path` starting at `start_offset` (0
/// verifies `MAGIC` first).
fn scan_segment_from(path: &Path, start_offset: u64) -> Option<ScannedSegment> {
    let file = open_regular_ro(path)?;
    let meta = file.metadata().ok()?;
    let (dev, ino, size) = (meta.dev(), meta.ino(), meta.len());
    let mut f = std::io::BufReader::new(file);
    let mut offset = if start_offset == 0 {
        let mut magic = [0u8; 8];
        f.read_exact(&mut magic).ok()?;
        if &magic != MAGIC {
            return None;
        }
        8u64
    } else {
        if start_offset < 8 || start_offset > size {
            return None;
        }
        f.seek(SeekFrom::Start(start_offset)).ok()?;
        start_offset
    };
    let mut out = Vec::new();
    let mut last_dims: Option<(usize, usize)> = None;
    loop {
        let mut head = [0u8; 4 + 8 + 4 + 4 + 1];
        if f.read_exact(&mut head).is_err() {
            break;
        }
        let len = u32::from_le_bytes(head[0..4].try_into().ok()?) as u64;
        let at = u64::from_le_bytes(head[4..12].try_into().ok()?);
        let w = u32::from_le_bytes(head[12..16].try_into().ok()?) as usize;
        let h = u32::from_le_bytes(head[16..20].try_into().ok()?) as usize;
        // One still being written (the screen is running) waits; refuse
        // truncated, oversized, or invalid-dimension records too.
        let Some(next) = offset.checked_add(4).and_then(|o| o.checked_add(len)) else {
            break;
        };
        if !(RECORD_HEADER_BYTES as u64..=MAX_RECORD_BYTES as u64).contains(&len) || next > size || checked_local_frame_bytes(w, h).is_err() {
            break;
        }
        out.push(Entry { at, key: head[20] == 1, offset });
        last_dims = Some((w, h));
        offset = next;
        if f.seek(SeekFrom::Start(offset)).is_err() {
            break;
        }
    }
    Some(ScannedSegment { entries: out, next_offset: offset, last_size: last_dims, dev, ino, file_len: size })
}

fn index_segment_entries(path: &Path) -> Option<Vec<Entry>> {
    scan_segment_from(path, 0).map(|s| s.entries)
}

#[cfg(test)]
fn index_segment(path: &Path) -> Option<Vec<Entry>> {
    index_segment_entries(path)
}

/// The size of the screen's last picture (validated against local frame limits).
pub fn last_size(segments: &[Segment]) -> Option<(usize, usize)> {
    use std::os::unix::fs::FileExt;
    let seg = segments.iter().rev().find(|s| !s.frames.is_empty())?;
    let e = seg.frames.last()?;
    let mut head = [0u8; 4 + 8 + 4 + 4];
    open_regular_ro(&seg.path)?.read_exact_at(&mut head, e.offset).ok()?;
    let u = |at: usize| u32::from_le_bytes([head[at], head[at + 1], head[at + 2], head[at + 3]]) as usize;
    let (w, h) = (u(12), u(16));
    checked_local_frame_bytes(w, h).ok()?;
    Some((w, h))
}

/// The record at `offset` in `path`: when, and its tiles, applied to
/// `frame` (made the record's size if it isn't).
fn apply(path: &Path, offset: u64, frame: &mut Frame) -> Option<u64> {
    apply_from(&mut open_regular_ro(path)?, offset, frame)
}

/// [`apply`], from a segment's file already open.
fn apply_from(f: &mut std::fs::File, offset: u64, frame: &mut Frame) -> Option<u64> {
    let file_len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(offset)).ok()?;
    let mut len = [0u8; 4];
    f.read_exact(&mut len).ok()?;
    let rec_len = u32::from_le_bytes(len) as usize;
    if !(RECORD_HEADER_BYTES..=MAX_RECORD_BYTES).contains(&rec_len) {
        return None;
    }
    let rec_end = offset.checked_add(4)?.checked_add(rec_len as u64)?;
    if rec_end > file_len {
        return None;
    }
    let mut rec = vec![0u8; rec_len];
    f.read_exact(&mut rec).ok()?;
    let at = u64::from_le_bytes(rec.get(0..8)?.try_into().ok()?);
    let (w, h) = (u32::from_le_bytes(rec.get(8..12)?.try_into().ok()?) as usize, u32::from_le_bytes(rec.get(12..16)?.try_into().ok()?) as usize);
    let frame_bytes = checked_local_frame_bytes(w, h).ok()?;
    let keyframe = rec[16] == 1;
    let tiles = u32::from_le_bytes(rec.get(17..21)?.try_into().ok()?) as usize;
    let grid_count = canonical_grid_count(w, h);
    if tiles > grid_count || tiles > (rec.len() - RECORD_HEADER_BYTES) / 12 {
        return None;
    }
    let size_mismatch = frame.width != w || frame.height != h || frame.rgba.len() != frame_bytes;
    if keyframe && tiles != grid_count {
        return None;
    }
    // Legacy recordings wrote full-grid resize transitions with `keyframe == false`.
    // Accept a mismatched-size non-keyframe ONLY when it covers the entire
    // canonical grid (every unique canonical tile validated below); partial
    // mismatched-size deltas are still rejected.
    if !keyframe && size_mismatch && tiles != grid_count {
        return None;
    }
    let u16_at = |p: usize| rec.get(p..p.checked_add(2)?).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    // Validate every tile header on the canonical 64x64 grid (no duplicates,
    // exact grid bounds) and compressed slice before allocating or decompressing.
    let mut seen = vec![0u64; grid_count.div_ceil(64)];
    let mut p = RECORD_HEADER_BYTES;
    let mut expect_total = 0usize;
    for _ in 0..tiles {
        let (x, y, tw, th) = (u16_at(p)?, u16_at(p.checked_add(2)?)?, u16_at(p.checked_add(4)?)?, u16_at(p.checked_add(6)?)?);
        let tile_bytes = validate_canonical_tile(w, h, x, y, tw, th, &mut seen).ok()?;
        expect_total = expect_total.checked_add(tile_bytes)?;
        if expect_total > frame_bytes {
            return None;
        }
        let p8 = p.checked_add(8)?;
        let p12 = p.checked_add(12)?;
        let clen = u32::from_le_bytes(rec.get(p8..p12)?.try_into().ok()?) as usize;
        let end = p12.checked_add(clen)?;
        if end > rec.len() {
            return None;
        }
        p = end;
    }
    if p != rec.len() {
        return None;
    }
    // Decode all tiles before mutating `frame`, capping each decompression at its tile's pixels.
    let mut decoded = Vec::with_capacity(tiles);
    let mut p = RECORD_HEADER_BYTES;
    for _ in 0..tiles {
        let (x, y, tw, th) = (u16_at(p)?, u16_at(p + 2)?, u16_at(p + 4)?, u16_at(p + 6)?);
        let clen = u32::from_le_bytes(rec[p + 8..p + 12].try_into().ok()?) as usize;
        let expect = tw * th * 4;
        let pixels = decode_zstd_exact(&rec[p + 12..p + 12 + clen], expect).ok()?;
        decoded.push(((x, y, tw, th), pixels));
        p += 12 + clen;
    }
    if frame.width != w || frame.height != h || frame.rgba.len() != frame_bytes {
        *frame = Frame { width: w, height: h, rgba: vec![0; frame_bytes] };
    }
    for (rect, pixels) in decoded {
        frame.put_tile(rect, &pixels);
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

/// Binary search for the latest non-empty segment whose first frame is `<= t`,
/// tolerating empty (header-only) segments anywhere in `segments`.
fn find_segment(segments: &[Segment], t: u64) -> Option<usize> {
    let (mut lo, mut hi) = (0usize, segments.len());
    let mut best: Option<usize> = None;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        // Find a non-empty probe inside `lo..hi`, starting at `mid`.
        let probe = (mid..hi).find(|&i| !segments[i].frames.is_empty()).or_else(|| (lo..mid).rev().find(|&i| !segments[i].frames.is_empty()));
        let Some(p) = probe else { break };
        if segments[p].frames[0].at <= t {
            best = Some(p);
            lo = p + 1;
        } else {
            hi = p;
        }
    }
    best
}

fn file_identity(meta: &std::fs::Metadata) -> (u64, u64) {
    (meta.dev(), meta.ino())
}

fn step_player(segments: &[Segment], at: &mut Option<(usize, usize)>, file: &mut Option<(usize, std::fs::File, (u64, u64))>, frame: &mut Frame, applied: &mut u64, t: u64) -> Option<u64> {
    let s = find_segment(segments, t)?;
    let seg = &segments[s];
    let last = seg.frames.partition_point(|f| f.at <= t).checked_sub(1)?;
    // (From the nearest keyframe when there's one past where it is: a
    // fast-forward across hours decodes a picture a sample, not every
    // keyframe between.)
    let key = seg.frames[..=last].iter().rposition(|f| f.key);
    let from = match *at {
        Some((ps, pi)) if ps == s && pi == last => return Some(seg.frames[last].at),
        Some((ps, pi)) if ps == s && pi < last => key.map_or(pi + 1, |k| k.max(pi + 1)),
        _ => key?,
    };
    if file.as_ref().is_none_or(|(i, _, _)| *i != s) {
        let f = open_regular_ro(&seg.path)?;
        let id = file_identity(&f.metadata().ok()?);
        *file = Some((s, f, id));
    }
    let (_, open, _) = file.as_mut()?;
    for (idx, e) in seg.frames[from..=last].iter().enumerate() {
        apply_from(open, e.offset, frame)?;
        *applied += 1;
        // Advance position after each successfully applied record so a failure
        // on a later record never leaves `*at` pointing at overwritten pixels.
        *at = Some((s, from + idx));
    }
    Some(seg.frames[last].at)
}

/// Plays frames forward: each record applied once, in order, going back
/// to a keyframe only on a jump back or into another segment, and on
/// from the nearest keyframe on a jump ahead past one. Where `t` is is
/// found by halving (a day's history is thousands of records), and the
/// segment's file kept open.
pub struct Player<'a> {
    segments: &'a [Segment],
    /// The segment and record the frame is up to.
    at: Option<(usize, usize)>,
    /// The segment whose file is open, with its `(dev, ino)` identity.
    file: Option<(usize, std::fs::File, (u64, u64))>,
    pub frame: Frame,
    /// How many records it has applied: the same as before a step, the
    /// picture hasn't changed.
    pub applied: u64,
}

impl<'a> Player<'a> {
    pub fn new(segments: &'a [Segment]) -> Player<'a> {
        Player { segments, at: None, file: None, frame: Frame { width: 0, height: 0, rgba: Vec::new() }, applied: 0 }
    }

    /// The screen as it was at `t`; None before the first frame.
    pub fn at(&mut self, t: u64) -> Option<&Frame> {
        step_player(self.segments, &mut self.at, &mut self.file, &mut self.frame, &mut self.applied, t)?;
        Some(&self.frame)
    }
}

/// Plays frames forward over an owned `Vec<Segment>` that can be updated as
/// new frames are appended without discarding the currently decoded frame.
pub struct OwnedPlayer {
    segments: Vec<Segment>,
    at: Option<(usize, usize)>,
    file: Option<(usize, std::fs::File, (u64, u64))>,
    pub frame: Frame,
    pub applied: u64,
}

impl OwnedPlayer {
    pub fn new(segments: Vec<Segment>) -> OwnedPlayer {
        OwnedPlayer { segments, at: None, file: None, frame: Frame { width: 0, height: 0, rgba: Vec::new() }, applied: 0 }
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Replace the segment list, preserving the decoded frame and open file if
    /// the previously decoded record still sits on the same underlying inode at
    /// the same path and offset (invalidating both if the file was replaced or
    /// truncated).
    pub fn update(&mut self, segments: Vec<Segment>) {
        let keep = self.at.and_then(|(s, i)| {
            let old_seg = self.segments.get(s)?;
            let old_e = old_seg.frames.get(i)?;
            let new_s = segments.iter().position(|ns| ns.path == old_seg.path)?;
            let new_e = segments[new_s].frames.get(i)?;
            if *new_e != *old_e {
                return None;
            }
            // Verify the path on disk still refers to the same `(dev, ino)` regular
            // file as our open descriptor and wasn't replaced, symlinked, or truncated.
            if let Some((open_s, _, open_id)) = &self.file
                && *open_s == s
            {
                let cur_meta = std::fs::symlink_metadata(&segments[new_s].path).ok()?;
                if !cur_meta.file_type().is_file() || file_identity(&cur_meta) != *open_id || cur_meta.len() <= old_e.offset {
                    return None;
                }
            }
            Some((new_s, i))
        });
        if let Some((new_s, i)) = keep {
            if let Some((open_s, _, _)) = &mut self.file {
                if Some(*open_s) == self.at.map(|(s, _)| s) {
                    *open_s = new_s;
                } else {
                    self.file = None;
                }
            }
            self.at = Some((new_s, i));
        } else {
            self.at = None;
            self.file = None;
            self.frame = Frame { width: 0, height: 0, rgba: Vec::new() };
        }
        self.segments = segments;
    }

    /// Clear all cached segments, open descriptors, and decoded pixels.
    pub fn reset(&mut self) {
        self.segments.clear();
        self.at = None;
        self.file = None;
        self.frame = Frame { width: 0, height: 0, rgba: Vec::new() };
    }

    /// The screen as it was at `t` (`(frame_timestamp_ms, &Frame)`); None
    /// before the first frame.
    pub fn at(&mut self, t: u64) -> Option<(u64, &Frame)> {
        let when = step_player(&self.segments, &mut self.at, &mut self.file, &mut self.frame, &mut self.applied, t)?;
        Some((when, &self.frame))
    }
}

/// Keeps a screen's frames: each one looked at, its changes written.
pub struct Writer {
    dir: PathBuf,
    max_bytes: u64,
    file: Option<(std::fs::File, u64)>,
    last: Option<Frame>,
    last_key: Option<Instant>,
    force_key: bool,
}

impl Writer {
    pub fn new(dir: PathBuf, max_bytes: u64) -> Writer {
        Writer { dir, max_bytes, file: None, last: None, last_key: None, force_key: false }
    }

    /// Ensure the next frame added is written as a full keyframe.
    pub fn force_keyframe(&mut self) {
        self.force_key = true;
    }

    pub fn add(&mut self, frame: Frame, at: u64) -> std::io::Result<()> {
        let expect = checked_local_frame_bytes(frame.width, frame.height).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        if frame.rgba.len() != expect {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("frame pixel length {} != expected {expect}", frame.rgba.len())));
        }
        // A new segment every few minutes, starting with a keyframe.
        if self.file.as_ref().is_none_or(|(_, start)| at.saturating_sub(*start) >= SEGMENT_MS) {
            if let Ok(m) = std::fs::symlink_metadata(&self.dir)
                && !m.file_type().is_dir()
            {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "recording directory is not a real directory"));
            }
            std::fs::create_dir_all(&self.dir)?;
            let path = self.dir.join(format!("frames-{at:016}.olof"));
            let mut opts = std::fs::OpenOptions::new();
            opts.create(true).append(true);
            #[cfg(target_os = "linux")]
            opts.custom_flags(O_NOFOLLOW_FLAG | O_CLOEXEC_FLAG);
            let mut f = opts.open(path)?;
            f.write_all(MAGIC)?;
            self.file = Some((f, at));
            self.last = None;
            self.trim();
        }
        let size_changed = self.last.as_ref().is_some_and(|b| b.width != frame.width || b.height != frame.height);
        let key = std::mem::take(&mut self.force_key) || self.last.is_none() || size_changed || self.last_key.is_none_or(|k| k.elapsed() >= KEY_EVERY);
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
            let packed = encode_zstd_bounded(&frame.tile(*t)[..], 3)?;
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

    /// Write a sequence of `(frame, at_ms, force_keyframe)` frames into `dir`.
    pub fn write_frames(dir: &Path, frames: Vec<(Frame, u64, bool)>) -> std::io::Result<()> {
        let mut w = Writer::new(dir.to_owned(), 1 << 40);
        for (frame, at, key) in frames {
            if key {
                w.force_keyframe();
            }
            w.add(frame, at)?;
        }
        Ok(())
    }

    /// Drop the oldest segments while the frames take more than allowed.
    fn trim(&self) {
        let segs = segments_in(&self.dir);
        let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let mut total: u64 = segs.iter().map(|p| size(p)).sum();
        for p in &segs[..segs.len().saturating_sub(1)] {
            if total <= self.max_bytes {
                break;
            }
            total -= size(p);
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Drop the segments in `dir` that ended before `cutoff` (ms): each
/// ends where the next begins.
pub fn prune(dir: &Path, cutoff: u64) {
    let segs = segments_in(dir);
    let start = |p: &Path| p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.strip_prefix("frames-")).and_then(|s| s.parse::<u64>().ok());
    for pair in segs.windows(2) {
        if start(&pair[1]).is_some_and(|next| next < cutoff) {
            let _ = std::fs::remove_file(&pair[0]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: usize, h: usize, fill: u8) -> Frame {
        Frame { width: w, height: h, rgba: vec![fill; w * h * 4] }
    }

    fn tempdir(tag: &str) -> PathBuf {
        let d = PathBuf::from("/var/tmp").join(format!("olo-codec-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn keeps_what_changed_and_plays_it_back() {
        let dir = tempdir("roundtrip");
        let mut w = Writer::new(dir.clone(), 512 << 20);
        let a = frame(200, 130, 10);
        w.add(a.clone(), 1_000).unwrap();
        // One pixel in the second tile row: only that tile goes.
        let mut b = a.clone();
        b.rgba[(70 * 200 + 5) * 4] = 99;
        assert_eq!(b.changed(Some(&a)), vec![(0, 64, 64, 64)]);
        w.add(b.clone(), 2_000).unwrap();
        // Nothing new: nothing kept.
        w.add(b.clone(), 3_000).unwrap();

        let segs = index_dir(&dir);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].frames.iter().map(|f| (f.at, f.key)).collect::<Vec<_>>(), vec![(1_000, true), (2_000, false)]);
        assert_eq!(last_size(&segs), Some((200, 130)));
        assert_eq!(frame_at(&segs, 1_500).unwrap(), (1_000, a.clone()));
        assert_eq!(frame_at(&segs, 9_000).unwrap(), (2_000, b.clone()));
        assert!(frame_at(&segs, 500).is_none());

        let idx = FrameIndex::from_segments(&segs);
        assert_eq!(idx.len(), 2);
        assert_eq!(idx.bounds(), Some((1_000, 2_000)));
        assert_eq!(idx.frame_time(999), None);
        assert_eq!(idx.frame_time(1_000), Some(1_000));
        assert_eq!(idx.frame_time(1_999), Some(1_000));
        assert_eq!(idx.frame_time(2_500), Some(2_000));
        assert_eq!(idx.next_after(0), Some(1_000));
        assert_eq!(idx.next_after(1_000), Some(2_000));
        assert_eq!(idx.next_after(2_000), None);
        assert_eq!(idx.last_size, Some((200, 130)));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn owned_player_keeps_state_across_appends_and_jumps() {
        let dir = tempdir("owned-player");
        let mut w = Writer::new(dir.clone(), 512 << 20);
        let f1 = frame(128, 64, 1);
        let mut f2 = f1.clone();
        f2.rgba[0] = 2;
        let mut f3 = f2.clone();
        f3.rgba[4] = 3;
        w.add(f1.clone(), 100).unwrap();
        w.add(f2.clone(), 200).unwrap();

        let mut known = HashMap::new();
        let segs1 = index_known_in(&dir, &mut known);
        let mut p = OwnedPlayer::new(segs1);
        assert_eq!(p.at(150), Some((100, &f1)));
        assert_eq!(p.applied, 1);
        assert_eq!(p.at(200), Some((200, &f2)));
        assert_eq!(p.applied, 2);

        // Append a third frame and update the player: stepping to 300 only applies 1 record.
        w.add(f3.clone(), 300).unwrap();
        let segs2 = index_known_in(&dir, &mut known);
        p.update(segs2);
        assert_eq!(p.at(300), Some((300, &f3)));
        assert_eq!(p.applied, 3);
        // Same timestamp again: no work.
        assert_eq!(p.at(350), Some((300, &f3)));
        assert_eq!(p.applied, 3);
        // Jump back rewinds to keyframe.
        assert_eq!(p.at(100), Some((100, &f1)));
        assert_eq!(p.applied, 4);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn prune_removes_old_segments_and_keeps_latest() {
        let dir = tempdir("prune");
        let a = frame(64, 64, 1);
        let b = frame(64, 64, 2);
        let c = frame(64, 64, 3);
        Writer::write_frames(&dir, vec![(a, 1_000, true), (b, 1_000 + SEGMENT_MS, true), (c, 1_000 + 2 * SEGMENT_MS, true)]).unwrap();
        assert_eq!(segments_in(&dir).len(), 3);
        prune(&dir, 1_000 + SEGMENT_MS + 1);
        assert_eq!(segments_in(&dir).len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn edges_are_partial_tiles_and_decode_rgba_guards_bounds() {
        let f = frame(130, 70, 0);
        let all = f.changed(None);
        assert_eq!(all.len(), 3 * 2);
        assert!(all.contains(&(128, 64, 2, 6)));

        let mut raw = Vec::new();
        raw.extend_from_slice(&2u32.to_le_bytes());
        raw.extend_from_slice(&2u32.to_le_bytes());
        raw.extend_from_slice(&[7u8; 16]);
        assert_eq!(Frame::decode_rgba(&raw), Some(Frame { width: 2, height: 2, rgba: vec![7; 16] }));
        assert_eq!(Frame::decode_rgba(&raw[..10]), None);
        let mut overflow = Vec::new();
        overflow.extend_from_slice(&u32::MAX.to_le_bytes());
        overflow.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(Frame::decode_rgba(&overflow), None);
    }

    fn write_raw_segment(dir: &Path, name: &str, record: &[u8], declared_len: Option<u32>) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        let mut raw = Vec::new();
        raw.extend_from_slice(MAGIC);
        raw.extend_from_slice(&declared_len.unwrap_or(record.len() as u32).to_le_bytes());
        raw.extend_from_slice(record);
        std::fs::write(&path, raw).unwrap();
        path
    }

    #[test]
    fn rejects_olof_zstd_bomb_and_crafted_headers() {
        let dir = tempdir("crafted-olof");
        let keep = frame(64, 64, 9);

        // 1. Record header declares 2 GiB on a 40-byte file: refused before allocating.
        let mut rec = Vec::new();
        rec.extend_from_slice(&1_000u64.to_le_bytes());
        rec.extend_from_slice(&64u32.to_le_bytes());
        rec.extend_from_slice(&64u32.to_le_bytes());
        rec.push(1);
        rec.extend_from_slice(&0u32.to_le_bytes());
        let p_trunc = write_raw_segment(&dir, "frames-0001.olof", &rec, Some(0x7FFF_FFFF));
        assert!(index_segment(&p_trunc).unwrap().is_empty());
        let mut dst = keep.clone();
        assert!(apply(&p_trunc, 8, &mut dst).is_none());
        assert_eq!(dst, keep);

        // 2. Record names absurd frame dimensions (100_000 x 100_000): refused before allocating.
        let mut rec = Vec::new();
        rec.extend_from_slice(&2_000u64.to_le_bytes());
        rec.extend_from_slice(&100_000u32.to_le_bytes());
        rec.extend_from_slice(&100_000u32.to_le_bytes());
        rec.push(1);
        rec.extend_from_slice(&0u32.to_le_bytes());
        let p_dim = write_raw_segment(&dir, "frames-0002.olof", &rec, None);
        let segs = vec![Segment { path: p_dim, frames: vec![Entry { at: 2_000, key: true, offset: 8 }] }];
        assert!(frame_at(&segs, 2_000).is_none());

        // 3. Record names tile_count or tile clen bigger than the record holds.
        let mut rec = Vec::new();
        rec.extend_from_slice(&3_000u64.to_le_bytes());
        rec.extend_from_slice(&64u32.to_le_bytes());
        rec.extend_from_slice(&64u32.to_le_bytes());
        rec.push(1);
        rec.extend_from_slice(&1_000_000u32.to_le_bytes());
        let p_tcnt = write_raw_segment(&dir, "frames-0003a.olof", &rec, None);
        let segs = vec![Segment { path: p_tcnt, frames: vec![Entry { at: 3_000, key: true, offset: 8 }] }];
        assert!(frame_at(&segs, 3_000).is_none());

        let mut rec = Vec::new();
        rec.extend_from_slice(&3_000u64.to_le_bytes());
        rec.extend_from_slice(&64u32.to_le_bytes());
        rec.extend_from_slice(&64u32.to_le_bytes());
        rec.push(1);
        rec.extend_from_slice(&1u32.to_le_bytes());
        for v in [0u16, 0, 64, 64] {
            rec.extend_from_slice(&v.to_le_bytes());
        }
        rec.extend_from_slice(&u32::MAX.to_le_bytes());
        let p_clen = write_raw_segment(&dir, "frames-0003.olof", &rec, None);
        let segs = vec![Segment { path: p_clen, frames: vec![Entry { at: 3_000, key: true, offset: 8 }] }];
        assert!(frame_at(&segs, 3_000).is_none());

        // 3b. Out-of-bounds tile and duplicate overlapping tiles exceeding frame bytes.
        let tile_z = zstd::encode_all(&vec![0u8; 64 * 64 * 4][..], 3).unwrap();
        let mut rec = Vec::new();
        rec.extend_from_slice(&3_500u64.to_le_bytes());
        rec.extend_from_slice(&64u32.to_le_bytes());
        rec.extend_from_slice(&64u32.to_le_bytes());
        rec.push(1);
        rec.extend_from_slice(&1u32.to_le_bytes());
        for v in [60u16, 0, 10, 10] {
            rec.extend_from_slice(&v.to_le_bytes());
        }
        rec.extend_from_slice(&(tile_z.len() as u32).to_le_bytes());
        rec.extend_from_slice(&tile_z);
        let p_oob = write_raw_segment(&dir, "frames-0003b.olof", &rec, None);
        let segs = vec![Segment { path: p_oob, frames: vec![Entry { at: 3_500, key: true, offset: 8 }] }];
        assert!(frame_at(&segs, 3_500).is_none());

        let mut rec = Vec::new();
        rec.extend_from_slice(&3_600u64.to_le_bytes());
        rec.extend_from_slice(&64u32.to_le_bytes());
        rec.extend_from_slice(&64u32.to_le_bytes());
        rec.push(1);
        rec.extend_from_slice(&2u32.to_le_bytes());
        for _ in 0..2 {
            for v in [0u16, 0, 64, 64] {
                rec.extend_from_slice(&v.to_le_bytes());
            }
            rec.extend_from_slice(&(tile_z.len() as u32).to_le_bytes());
            rec.extend_from_slice(&tile_z);
        }
        let p_dup = write_raw_segment(&dir, "frames-0003c.olof", &rec, None);
        let segs = vec![Segment { path: p_dup, frames: vec![Entry { at: 3_600, key: true, offset: 8 }] }];
        assert!(frame_at(&segs, 3_600).is_none());

        // 4. Small compressed zstd bomb (~1 KiB) expanding to 8 MiB for a 64x64 (16 KiB) tile.
        let huge = vec![0u8; 8 * 1024 * 1024];
        for (idx, bomb) in [zstd::encode_all(&huge[..], 3).unwrap(), zstd::bulk::compress(&huge, 3).unwrap()].into_iter().enumerate() {
            let mut rec = Vec::new();
            rec.extend_from_slice(&4_000u64.to_le_bytes());
            rec.extend_from_slice(&64u32.to_le_bytes());
            rec.extend_from_slice(&64u32.to_le_bytes());
            rec.push(1);
            rec.extend_from_slice(&1u32.to_le_bytes());
            for v in [0u16, 0, 64, 64] {
                rec.extend_from_slice(&v.to_le_bytes());
            }
            rec.extend_from_slice(&(bomb.len() as u32).to_le_bytes());
            rec.extend_from_slice(&bomb);
            let p_bomb = write_raw_segment(&dir, &format!("frames-bomb-{idx}.olof"), &rec, None);
            let segs = vec![Segment { path: p_bomb, frames: vec![Entry { at: 4_000, key: true, offset: 8 }] }];
            assert!(frame_at(&segs, 4_000).is_none());
        }

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn astra_f5_supports_4k_at_3x_and_validates_last_size() {
        // 3840x2160 at 3x = 11520x6480 = 298,598,400 RGBA bytes (> 256 MiB).
        let (w, h) = (11_520usize, 6_480usize);
        let mut raw = vec![0u8; 8 + w * h * 4];
        raw[0..4].copy_from_slice(&(w as u32).to_le_bytes());
        raw[4..8].copy_from_slice(&(h as u32).to_le_bytes());
        let decoded = Frame::decode_rgba(&raw);
        assert!(decoded.is_some(), "4K@3x (11520x6480) capture must decode on local path");

        // last_size must reject absurd/corrupt dimensions the decoder would refuse.
        let dir = tempdir("f5-last-size");
        let mut rec = Vec::new();
        rec.extend_from_slice(&1_000u64.to_le_bytes());
        rec.extend_from_slice(&100_000u32.to_le_bytes());
        rec.extend_from_slice(&100_000u32.to_le_bytes());
        rec.push(1);
        rec.extend_from_slice(&0u32.to_le_bytes());
        let p = write_raw_segment(&dir, "frames-bad-dim.olof", &rec, None);
        let segs = vec![Segment { path: p, frames: vec![Entry { at: 1_000, key: true, offset: 8 }] }];
        assert_eq!(last_size(&segs), None, "last_size must not advertise invalid dimensions");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn astra_f8_rejects_excess_noncanonical_small_tiles_in_olof() {
        let dir = tempdir("f8-olof-small-tiles");
        // A 64x64 frame has 1 canonical 64x64 grid tile; 64 tiny 1x1 tiles fit
        // within frame_bytes but exceed the canonical grid count and positions.
        let tiny = zstd::encode_all(&[1u8, 2, 3, 4][..], 3).unwrap();
        let mut rec = Vec::new();
        rec.extend_from_slice(&1_000u64.to_le_bytes());
        rec.extend_from_slice(&64u32.to_le_bytes());
        rec.extend_from_slice(&64u32.to_le_bytes());
        rec.push(1);
        rec.extend_from_slice(&64u32.to_le_bytes());
        for i in 0..64u16 {
            for v in [i, 0, 1, 1] {
                rec.extend_from_slice(&v.to_le_bytes());
            }
            rec.extend_from_slice(&(tiny.len() as u32).to_le_bytes());
            rec.extend_from_slice(&tiny);
        }
        let p = write_raw_segment(&dir, "frames-small.olof", &rec, None);
        let segs = vec![Segment { path: p, frames: vec![Entry { at: 1_000, key: true, offset: 8 }] }];
        assert!(frame_at(&segs, 1_000).is_none(), "non-canonical excess tile table must be rejected");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn astra_f19_failed_multirecord_seek_does_not_poison_cache() {
        let dir = tempdir("f19-poison");
        let a = frame(64, 64, 10);
        let b = frame(64, 64, 20);
        Writer::write_frames(&dir, vec![(a.clone(), 100, true), (b.clone(), 200, false)]).unwrap();
        let seg_path = segments_in(&dir)[0].clone();
        // Append a third record C at t=300 with valid header + invalid zstd payload.
        let mut bad_rec = Vec::new();
        bad_rec.extend_from_slice(&300u64.to_le_bytes());
        bad_rec.extend_from_slice(&64u32.to_le_bytes());
        bad_rec.extend_from_slice(&64u32.to_le_bytes());
        bad_rec.push(0); // delta
        bad_rec.extend_from_slice(&1u32.to_le_bytes());
        for v in [0u16, 0, 64, 64] {
            bad_rec.extend_from_slice(&v.to_le_bytes());
        }
        let garbage = b"not-valid-zstd";
        bad_rec.extend_from_slice(&(garbage.len() as u32).to_le_bytes());
        bad_rec.extend_from_slice(garbage);
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&seg_path).unwrap();
            f.write_all(&(bad_rec.len() as u32).to_le_bytes()).unwrap();
            f.write_all(&bad_rec).unwrap();
        }
        let segs = index_dir(&dir);
        assert_eq!(segs[0].frames.len(), 3);
        let mut p = OwnedPlayer::new(segs);
        // Cache frame A.
        assert_eq!(p.at(100), Some((100, &a)));
        // Seek past valid delta B to damaged record C -> fails.
        assert_eq!(p.at(300), None);
        // Seeking A again must return A's pixels, NOT B's poisoned pixels!
        assert_eq!(p.at(100), Some((100, &a)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn astra_f20_replaced_segment_invalidates_unlinked_fd() {
        let dir = tempdir("f20-replace");
        let f1 = frame(64, 64, 11);
        let f2 = frame(64, 64, 22);
        Writer::write_frames(&dir, vec![(f1.clone(), 100, true)]).unwrap();
        let seg_path = segments_in(&dir)[0].clone();
        let mut p = OwnedPlayer::new(index_dir(&dir));
        assert_eq!(p.at(100), Some((100, &f1)));

        // Build a replacement file with identical first entry (same at/key/offset)
        // plus a second frame at t=200, then atomically rename it over `seg_path`.
        let tmp_dir = dir.join("tmp");
        Writer::write_frames(&tmp_dir, vec![(f1.clone(), 100, true), (f2.clone(), 200, false)]).unwrap();
        let new_file = segments_in(&tmp_dir)[0].clone();
        std::fs::rename(&new_file, &seg_path).unwrap();
        let _ = std::fs::remove_dir_all(&tmp_dir);

        p.update(index_dir(&dir));
        assert_eq!(p.at(200), Some((200, &f2)), "OwnedPlayer must reopen replaced segment inode");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn astra_f21_header_only_empty_segment_does_not_hide_later_frames() {
        let dir = tempdir("f21-empty-seg");
        let f1 = frame(64, 64, 33);
        let f2 = frame(64, 64, 44);
        Writer::write_frames(&dir, vec![(f1.clone(), 100, true), (f2.clone(), 100 + 2 * SEGMENT_MS, true)]).unwrap();
        // Insert a header-only empty segment sorted strictly between the two valid ones.
        let empty_path = dir.join(format!("frames-{:016}.olof", 100 + SEGMENT_MS));
        std::fs::write(&empty_path, MAGIC).unwrap();

        let segs_raw = vec![
            Segment { path: segments_in(&dir)[0].clone(), frames: index_segment(&segments_in(&dir)[0]).unwrap() },
            Segment { path: empty_path.clone(), frames: vec![] },
            Segment { path: segments_in(&dir)[2].clone(), frames: index_segment(&segments_in(&dir)[2]).unwrap() },
        ];
        // Direct Player / OwnedPlayer with an empty segment in the middle.
        let mut p = Player::new(&segs_raw);
        assert_eq!(p.at(100 + 2 * SEGMENT_MS + 50), Some(&f2));
        let mut op = OwnedPlayer::new(segs_raw);
        assert_eq!(op.at(100 + 2 * SEGMENT_MS + 50), Some((100 + 2 * SEGMENT_MS, &f2)));

        // Also index_dir must find f2 cleanly.
        let mut op2 = OwnedPlayer::new(index_dir(&dir));
        assert_eq!(op2.at(100 + 2 * SEGMENT_MS + 50), Some((100 + 2 * SEGMENT_MS, &f2)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn review2_f19_fifo_and_symlink_olof_never_hang_or_follow() {
        let dir = tempdir("r2-f19-fifo");
        std::fs::create_dir_all(&dir).unwrap();

        // Create a real FIFO named `frames-0001.olof` with no writer; opening it
        // without O_NONBLOCK hangs indefinitely!
        let fifo = dir.join("frames-0001.olof");
        let status = std::process::Command::new("mkfifo").arg(&fifo).status().expect("mkfifo");
        assert!(status.success());

        // Also create a symlink pointing to a real segment outside `dir`.
        let outside = tempdir("r2-f19-outside");
        let f_out = frame(64, 64, 55);
        Writer::write_frames(&outside, vec![(f_out.clone(), 500, true)]).unwrap();
        let outside_seg = segments_in(&outside)[0].clone();
        let link = dir.join("frames-0002.olof");
        std::os::unix::fs::symlink(&outside_seg, &link).unwrap();

        // Run inside a worker thread with a strict timeout so a failing unfixed
        // implementation fails promptly instead of wedging the test runner.
        let dir_c = dir.clone();
        let link_c = link.clone();
        let fifo_c = fifo.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let segs = index_dir(&dir_c);
            let mut known = HashMap::new();
            let segs2 = index_known_in(&dir_c, &mut known);
            let direct_fifo = vec![Segment { path: fifo_c, frames: vec![Entry { at: 100, key: true, offset: 8 }] }];
            let ls_fifo = last_size(&direct_fifo);
            let fa_fifo = frame_at(&direct_fifo, 100);
            let direct_link = vec![Segment { path: link_c, frames: vec![Entry { at: 500, key: true, offset: 8 }] }];
            let ls_link = last_size(&direct_link);
            let fa_link = frame_at(&direct_link, 500);
            let _ = tx.send((segs.len(), segs2.len(), ls_fifo, fa_fifo.is_some(), ls_link, fa_link.is_some()));
        });
        let (n1, n2, ls_fifo, fa_fifo, ls_link, fa_link) = rx.recv_timeout(Duration::from_secs(2)).expect("olof hung on FIFO!");
        assert_eq!(n1, 0, "index_dir must skip FIFOs and symlinks");
        assert_eq!(n2, 0, "index_known_in must skip FIFOs and symlinks");
        assert_eq!(ls_fifo, None, "last_size must reject FIFO");
        assert!(!fa_fifo, "frame_at must reject FIFO");
        assert_eq!(ls_link, None, "last_size must reject symlink");
        assert!(!fa_link, "frame_at must reject symlink");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn review2_f39_resize_is_marked_keyframe_and_legacy_resize_plays_back() {
        let dir = tempdir("r2-f39-resize");
        let f1 = frame(64, 64, 10);
        let f2 = frame(128, 64, 20);
        let mut f3 = f2.clone();
        f3.rgba[0] = 99;

        // 1. New Writer::add on resize within 30s must mark the resize record as keyframe.
        let mut w = Writer::new(dir.clone(), 512 << 20);
        w.add(f1.clone(), 1_000).unwrap();
        w.add(f2.clone(), 2_000).unwrap();
        w.add(f3.clone(), 3_000).unwrap();
        let segs = index_dir(&dir);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].frames.len(), 3);
        assert!(segs[0].frames[1].key, "Writer::add must mark dimension change as keyframe");
        assert_eq!(frame_at(&segs, 2_000), Some((2_000, f2.clone())));
        assert_eq!(frame_at(&segs, 3_000), Some((3_000, f3.clone())));

        // 2. Legacy recording: full-grid resize with key flag = 0 must still play back,
        //    while a partial (non-full-grid) mismatched-size delta must still be rejected.
        let legacy_dir = tempdir("r2-f39-legacy");
        let seg_path = segments_in(&dir)[0].clone();
        let mut raw = std::fs::read(&seg_path).unwrap();
        // Flip the key byte of record #1 (offset + 4 + 16) from 1 to 0.
        let rec1_key_pos = segs[0].frames[1].offset as usize + 4 + 16;
        assert_eq!(raw[rec1_key_pos], 1);
        raw[rec1_key_pos] = 0;
        std::fs::create_dir_all(&legacy_dir).unwrap();
        std::fs::write(legacy_dir.join("frames-0000000000001000.olof"), raw).unwrap();

        let lsegs = index_dir(&legacy_dir);
        assert!(!lsegs[0].frames[1].key);
        let mut p = OwnedPlayer::new(lsegs.clone());
        assert_eq!(p.at(1_000), Some((1_000, &f1)));
        assert_eq!(p.at(2_000), Some((2_000, &f2)), "legacy full-grid resize with key=0 must play back");
        assert_eq!(p.at(3_000), Some((3_000, &f3)), "delta after legacy resize must play back");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&legacy_dir);
    }

    #[test]
    fn review2_f34_and_f41_incremental_cache_and_empty_player_reset() {
        let dir = tempdir("r2-f34-cache");
        let f1 = frame(64, 64, 1);
        let f2 = frame(64, 64, 2);
        let f3 = frame(128, 64, 3);
        let mut w = Writer::new(dir.clone(), 512 << 20);
        w.add(f1.clone(), 100).unwrap();
        w.add(f2.clone(), 200).unwrap();

        let mut cache = SegmentCache::new();
        let s1 = cache.scan(&dir);
        assert!(s1.changed);
        assert_eq!(s1.scanned_records, 2);
        assert_eq!(s1.last_size, Some((64, 64)));

        // Unchanged scan does zero record reads.
        let s1_again = cache.scan(&dir);
        assert!(!s1_again.changed);
        assert_eq!(s1_again.scanned_records, 0);

        // Appending 1 frame scans ONLY that 1 newly added record (not all 3).
        w.add(f3.clone(), 300).unwrap();
        let s2 = cache.scan(&dir);
        assert!(s2.changed && !s2.replaced);
        assert_eq!(s2.scanned_records, 1, "append must scan only the new record, not rescan the whole segment");
        assert_eq!(s2.last_size, Some((128, 64)));
        assert_eq!(s2.segments[0].frames.len(), 3);

        // Equal-size inode replacement is detected cleanly.
        let seg_path = s2.segments[0].path.clone();
        let tmp_dir = dir.join("tmp");
        let g1 = frame(64, 64, 9);
        Writer::write_frames(&tmp_dir, vec![(g1.clone(), 100, true)]).unwrap();
        let repl = segments_in(&tmp_dir)[0].clone();
        std::fs::rename(&repl, &seg_path).unwrap();
        let _ = std::fs::remove_dir_all(&tmp_dir);
        let s3 = cache.scan(&dir);
        assert!(s3.changed && s3.replaced, "replaced segment inode must be flagged");
        assert_eq!(s3.last_size, Some((64, 64)));

        // Empty update clears OwnedPlayer completely (never serves stale pixels).
        let mut player = OwnedPlayer::new(s3.segments);
        assert_eq!(player.at(100), Some((100, &g1)));
        player.update(Vec::new());
        assert_eq!(player.at(100), None);
        assert!(player.frame.rgba.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
