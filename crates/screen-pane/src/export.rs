//! A screen's history as an MP4 video, made in process: the agent's
//! actions (each from a moment before it to a moment after it ended, its
//! result showing) played as they went, at 10 frames a second, and the
//! time between them skipped (Ben, 2026-11-15: "only include the actual
//! parts where actions/events occurred, skipping over all the
//! inactivity"). A skip shows the screen across what it skips, quickly
//! (in half a second; one of `LONG_SKIP_MS` or more in `FF_MS`, the
//! screen racing on: "like it is fast forwarding through time to the next
//! point where activity starts"), under a small badge at the top saying
//! how long, never a card over the screen ("too intrusive"), and the
//! screen plays on as it went for `FF_PAD_MS` more before each skip and
//! after it, so what's on it can be read ("a little too quick to ff":
//! Ben, 2026-11-16). The screen changing
//! by itself (a page's animation, a spinner, a clock) isn't something
//! happening: on real histories it was, nearly every second of them.
//!
//! The work follows what's kept, not the history's length ("make sure it
//! is all super efficient so we're not doing useless or wasteful encoding
//! work"): the pictures are played forward once (`record::Player`, each
//! record applied once, a jump ahead from the nearest keyframe); a
//! picture is encoded only when it changed (one that didn't is the one
//! before, shown longer), straight from the player's frame; and on the
//! GPU where there's one (`olo_video::Encoder`, as Screenshot's
//! recordings are: NVIDIA's, else the Intel or AMD GPU's, else the
//! processor's). Any of the actions may be given (a part of the history:
//! what the viewer lists, or the rows chosen). Run on a worker:
//! `progress` is told how far it has got and says whether to go on.

use std::{
    fs::{File, OpenOptions},
    io::Write as _,
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    history,
    record::{self, Frame, Player},
};

/// Monotonic per-process sequence so every temporary `.part` file created for
/// an export or screenshot is uniquely named and never collides with another
/// pane's job or an unrelated sibling file.
static PART_SEQ: AtomicU64 = AtomicU64::new(1);

/// Maximum screenshot output side and total pixels (256 MiB RGBA budget) so
/// zoomed screenshots on large frames cannot allocate gigabytes.
pub(crate) const MAX_SHOT_SIDE: usize = 16_384;
pub(crate) const MAX_SHOT_PIXELS: usize = 64 * 1024 * 1024;

/// Whether a 0-based action selection has gaps between its sorted unique
/// indices (e.g. `[2, 3, 4, 8]` → `true`, `[2, 3, 4]` → `false`).
pub(crate) fn has_gaps(actions: &[usize]) -> bool {
    if actions.len() <= 1 {
        return false;
    }
    let mut sorted = actions.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted.windows(2).any(|w| w[1] != w[0] + 1)
}

/// Parse a 0-based `--part` spec (`"2-4,8"` or `"0,3"`) into sorted unique
/// action indices in `0..count`.
pub fn parse_part_spec(spec: &str, count: usize) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for piece in spec.split(',') {
        let piece = piece.trim();
        if piece.is_empty() {
            return Err(format!("invalid --part spec {spec:?}"));
        }
        if let Some((a, b)) = piece.split_once('-') {
            let start: usize = a.trim().parse().map_err(|_| format!("invalid --part range {piece:?}"))?;
            let end: usize = b.trim().parse().map_err(|_| format!("invalid --part range {piece:?}"))?;
            if start > end {
                return Err(format!("invalid --part range {piece:?}: start > end"));
            }
            for i in start..=end {
                if i < count {
                    out.push(i);
                }
            }
        } else {
            let i: usize = piece.parse().map_err(|_| format!("invalid --part index {piece:?}"))?;
            if i < count {
                out.push(i);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    if out.is_empty() {
        return Err("none of the selected actions exist in this history".into());
    }
    Ok(out)
}

/// An exclusively created (`O_CREAT | O_EXCL | O_NOFOLLOW`, mode `0600`),
/// uniquely named sibling temporary file beside a destination path.
///
/// Cleanup belongs strictly to the creator that successfully created this file:
/// a failure before creation never unlinks anything, and dropping before
/// [`ExclusivePartFile::publish`] removes only this creator's unique temp file.
pub(crate) struct ExclusivePartFile {
    path: PathBuf,
    file: Option<File>,
    keep: bool,
}

impl ExclusivePartFile {
    pub(crate) fn create_beside(target: &Path) -> Result<ExclusivePartFile, String> {
        let parent = target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
        let base = target.file_name().and_then(|n| n.to_str()).unwrap_or("output");
        let pid = std::process::id();
        for _ in 0..32 {
            let seq = PART_SEQ.fetch_add(1, Ordering::Relaxed);
            let candidate = parent.join(format!(".{base}.{pid}-{seq}.part"));
            match OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(&candidate) {
                Ok(file) => return Ok(ExclusivePartFile { path: candidate, file: Some(file), keep: false }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("{}: {e}", candidate.display())),
            }
        }
        Err(format!("couldn't allocate a unique temporary file beside {}", target.display()))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn take_file(&mut self) -> Option<File> {
        self.file.take()
    }

    pub(crate) fn publish(mut self, target: &Path) -> Result<(), String> {
        if let Some(mut f) = self.file.take() {
            f.flush().map_err(|e| format!("{}: {e}", self.path.display()))?;
        }
        std::fs::rename(&self.path, target).map_err(|e| format!("{}: {e}", target.display()))?;
        self.keep = true;
        Ok(())
    }
}

impl Drop for ExclusivePartFile {
    fn drop(&mut self) {
        self.file.take();
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

const FPS: u64 = 10;
const STEP_MS: u64 = 1000 / FPS;
/// A picture's time in the MP4's ticks (FPS × 1000 a second).
const TICKS: u32 = 1000;
/// Kept before an action begins and after it ends.
const BEFORE_MS: u64 = 800;
const AFTER_MS: u64 = 1500;
/// An action longer than this (a wait) keeps its start and its end.
const LONG_MS: u64 = 10_000;
/// Time between what's kept shorter than this plays as it went (a skip
/// of a second or two, its badge come and gone, was choppier than the
/// seconds it saved).
const PLAY_THROUGH_MS: u64 = 3000;
/// A skip this long or longer takes `FF_MS`, the screen racing on under
/// its badge; shorter, `QUICK_MS`.
const LONG_SKIP_MS: u64 = 30_000;
const FF_MS: u64 = 1500;
const QUICK_MS: u64 = 500;
/// The screen as it went, kept this much longer before each skip and
/// after it: time to read what's on it (Ben, 2026-11-16: "add an extra
/// 5s delay before and after each fast forward").
const FF_PAD_MS: u64 = 5_000;

/// One piece of the video: the screen from `from` to `to` as it went, or
/// skipped through.
#[derive(Debug, Clone, PartialEq)]
enum Piece {
    Screen { from: u64, to: u64 },
    Skip { from: u64, to: u64 },
}

/// What's kept around these actions (when each began, how long it took),
/// and the skips between.
fn pieces(actions: &[(u64, u64)]) -> Vec<Piece> {
    let mut spans: Vec<(u64, u64)> = actions
        .iter()
        .flat_map(|&(at, ms)| {
            let end = at.saturating_add(ms);
            if ms > LONG_MS {
                vec![(at.saturating_sub(BEFORE_MS), at.saturating_add(AFTER_MS)), (end.saturating_sub(BEFORE_MS), end.saturating_add(AFTER_MS))]
            } else {
                vec![(at.saturating_sub(BEFORE_MS), end.saturating_add(AFTER_MS))]
            }
        })
        .collect();
    spans.sort_unstable();
    let mut out = Vec::new();
    let Some(&(mut from, mut to)) = spans.first() else { return out };
    for &(a, b) in &spans[1..] {
        // (What's between, less the time kept on either side of a skip,
        // as short as a play-through: it plays through.)
        if a <= to.saturating_add(PLAY_THROUGH_MS + 2 * FF_PAD_MS) {
            to = to.max(b);
            continue;
        }
        out.push(Piece::Screen { from, to: to.saturating_add(FF_PAD_MS) });
        out.push(Piece::Skip { from: to.saturating_add(FF_PAD_MS), to: a.saturating_sub(FF_PAD_MS) });
        (from, to) = (a.saturating_sub(FF_PAD_MS), b);
    }
    out.push(Piece::Screen { from, to });
    out
}

/// How many pictures a skip of `ms` takes.
fn skip_frames(ms: u64) -> u64 {
    (if ms >= LONG_SKIP_MS { FF_MS } else { QUICK_MS } / STEP_MS).max(2)
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

/// The video being written: the picture last encoded is held until the
/// next one comes, as long as it showed.
struct Out {
    encoder: olo_video::Encoder,
    mp4: olo_video::Mp4,
    held: Option<(Vec<u8>, bool, u32)>,
    encoded: u64,
}

impl Out {
    /// The picture before, shown a frame longer (false: there's none).
    fn again(&mut self) -> bool {
        self.held.as_mut().map(|h| h.2 += TICKS).is_some()
    }

    /// A new picture, in BGRA, `stride` bytes a row (as wide as the video
    /// or wider).
    fn picture(&mut self, bgra: &[u8], stride: usize) -> Result<(), String> {
        self.flush()?;
        let (bits, key) = self.encoder.encode(bgra, stride, olo_video::Order::Bgra)?;
        self.held = Some((bits, key, TICKS));
        self.encoded += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        if let Some((bits, key, ticks)) = self.held.take() {
            self.mp4.add(&bits, key, ticks).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

/// The screen's picture `frame`, each pixel `zoom` × `zoom`, written to `path`
/// as a PNG (streamed row by row into an exclusively created sibling temp file,
/// renamed once whole). Validates output dimensions and pixel budget against
/// `frame`'s actual size with checked arithmetic.
pub fn save_shot(frame: &Frame, zoom: usize, path: &Path) -> Result<(), String> {
    let z = zoom.max(1);
    if frame.width == 0 || frame.height == 0 {
        return Err("cannot save an empty screenshot".into());
    }
    let expected_src = frame.width.checked_mul(frame.height).and_then(|p| p.checked_mul(4)).ok_or("source frame dimensions overflow")?;
    if frame.rgba.len() != expected_src {
        return Err(format!("source frame pixel count {} != expected {expected_src}", frame.rgba.len()));
    }
    let w = frame.width.checked_mul(z).ok_or("screenshot width overflow")?;
    let h = frame.height.checked_mul(z).ok_or("screenshot height overflow")?;
    let pixels = w.checked_mul(h).ok_or("screenshot pixel count overflow")?;
    if w > MAX_SHOT_SIDE || h > MAX_SHOT_SIDE || pixels > MAX_SHOT_PIXELS {
        return Err(format!("zoomed screenshot {w}×{h} exceeds maximum allowed size ({MAX_SHOT_SIDE} side, {MAX_SHOT_PIXELS} pixels)"));
    }
    let row_bytes = w.checked_mul(4).ok_or("screenshot row overflow")?;
    let mut part = ExclusivePartFile::create_beside(path)?;
    let file = part.take_file().ok_or("temporary file unavailable")?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    let mut stream = writer.stream_writer_with_size(row_bytes.min(64 << 10)).map_err(|e| e.to_string())?;
    let mut wide = Vec::with_capacity(row_bytes);
    for y in 0..frame.height {
        let row = &frame.rgba[y * frame.width * 4..(y + 1) * frame.width * 4];
        wide.clear();
        if z == 1 {
            wide.extend_from_slice(row);
        } else {
            for px in row.as_chunks::<4>().0 {
                for _ in 0..z {
                    wide.extend_from_slice(px);
                }
            }
        }
        for _ in 0..z {
            stream.write_all(&wide).map_err(|e| e.to_string())?;
        }
    }
    stream.finish().map_err(|e| e.to_string())?;
    drop(writer);
    part.publish(path)
}

/// Make screen `name`'s history into `out`: all of it (`actions` None),
/// or the part around these of its actions (their numbers in the
/// history, from 0), each pixel `zoom` × `zoom` (small screens easier to
/// read). `progress(0..1)` false stops it (and removes what was written).
pub fn export(name: &str, actions: Option<&[usize]>, zoom: usize, encoder: olo_video::Choice, out: &Path, progress: &mut dyn FnMut(f32) -> bool) -> Result<Made, String> {
    let all = history::read(name);
    let chosen: Vec<(u64, u64)> = match actions {
        None => all.iter().map(|a| (a.at, a.ms)).collect(),
        Some(which) => which.iter().filter_map(|&i| all.get(i)).map(|a| (a.at, a.ms)).collect(),
    };
    export_from(&chosen, &record::index(name), encoder, zoom, out, progress)
}

/// What an export made: how many frames (a tenth of a second each), how
/// many pictures it encoded for them, and on what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Made {
    pub frames: u64,
    pub encoded: u64,
    pub encoder: String,
    /// Its size, zoomed.
    pub size: (usize, usize),
}

/// The largest a video is made: H.264's (level 5.1: 4096 a side, about
/// 9.4 million pixels) and the GPUs' encoders'.
const MOST_SIDE: usize = 4096;
const MOST_PIXELS: usize = 4096 * 2304;

/// The zooms a screen of `w` × `h` can be exported at (1 always).
pub fn zooms(w: usize, h: usize) -> Vec<usize> {
    (1..=3).filter(|&z| z == 1 || ((w & !1) * z <= MOST_SIDE && (h & !1) * z <= MOST_SIDE && (w & !1) * (h & !1) * z * z <= MOST_PIXELS)).collect()
}

/// [`export`] of these actions (when each began, how long it took) and
/// frames, by the `encoder` chosen.
fn export_from(actions: &[(u64, u64)], segments: &[record::Segment], encoder: olo_video::Choice, zoom: usize, out: &Path, progress: &mut dyn FnMut(f32) -> bool) -> Result<Made, String> {
    let plan = pieces(actions);
    if plan.is_empty() {
        return Err("nothing has been done on this screen".into());
    }
    let first = segments.iter().flat_map(|s| s.frames.first()).map(|f| f.at).next().ok_or("no frames were kept")?;
    let mut player = Player::new(segments);
    // The size the video is: the screen's where the first piece begins
    // (H.264 wants even sizes), zoomed as far as it may be.
    let at_start = match plan[0] {
        Piece::Screen { from, .. } | Piece::Skip { from, .. } => from.max(first),
    };
    let probe = player.at(at_start).ok_or("no frames were kept")?;
    let (w, h) = (probe.width & !1, probe.height & !1);
    let z = zooms(w, h).into_iter().filter(|&z| z <= zoom.max(1)).max().unwrap_or(1);
    let (vw, vh) = (w * z, h * z);
    let total: u64 = plan
        .iter()
        .map(|p| match *p {
            Piece::Screen { from, to } => (to - from) / STEP_MS,
            Piece::Skip { from, to } => skip_frames(to - from),
        })
        .sum::<u64>()
        .max(1);
    let encoder = open_encoder(encoder, &plan, segments, vw, vh)?;
    let mut part = ExclusivePartFile::create_beside(out)?;
    // Close our creation handle before `Mp4::create_for` opens the exclusive path.
    drop(part.take_file());
    // (Each frame a tenth of a second: FPS × 1000 ticks a second.)
    let mp4 = olo_video::Mp4::create_for(part.path(), vw as u32, vh as u32, FPS as u32 * 1000, encoder.codec()).map_err(|e| e.to_string())?;
    let mut video = Out { encoder, mp4, held: None, encoded: 0 };
    // The picture encoded, as the video is (zoomed, in BGRA: the order the
    // GPUs' encoders take, so nothing is copied again on the way); and a
    // skip's screen under its card, kept to draw the next frame's card
    // on without drawing the screen again.
    let mut pic = vec![0u8; vw * vh * 4];
    let mut under = vec![0u8; vw * vh * 4];
    // Which of the player's pictures was encoded last (None: another).
    let mut shown: Option<u64> = None;
    let mut done = 0u64;
    let mut step = |done: &mut u64| -> Result<(), String> {
        *done += 1;
        if progress(*done as f32 / total as f32) { Ok(()) } else { Err("cancelled".into()) }
    };
    let result = (|| -> Result<(), String> {
        for p in &plan {
            match *p {
                Piece::Screen { from, to } => {
                    for t in (from..to).step_by(STEP_MS as usize) {
                        if player.at(t.max(first)).is_none() {
                            continue;
                        }
                        let now = player.applied;
                        if shown != Some(now) || !video.again() {
                            put(&player.frame, w, h, z, &mut pic);
                            video.picture(&pic, vw * 4)?;
                            shown = Some(now);
                        }
                        step(&mut done)?;
                    }
                }
                Piece::Skip { from, to } => {
                    // A few of the screen's pictures across it (a long
                    // one's racing on), each kept for frames on end,
                    // under the badge saying how long.
                    let n = skip_frames(to - from);
                    let samples = if to - from >= LONG_SKIP_MS { 5 } else { 2 };
                    let mut sampled = None;
                    for i in 0..n {
                        let s = i * samples / n;
                        let t = from + (to - from) * s / (samples - 1).max(1);
                        if sampled != Some(s) {
                            sampled = Some(s);
                            if player.at(t.max(first)).is_some() {
                                put(&player.frame, w, h, z, &mut under);
                            }
                        } else if video.again() {
                            // (The badge doesn't change: the same picture,
                            // longer.)
                            step(&mut done)?;
                            continue;
                        }
                        pic.copy_from_slice(&under);
                        badge(&mut pic, vw, vh, &span(to - from));
                        video.picture(&pic, vw * 4)?;
                        step(&mut done)?;
                    }
                    shown = None;
                }
            }
        }
        video.flush()
    })();
    if let Err(e) = result {
        drop(video);
        drop(part);
        return Err(e);
    }
    let made = Made { frames: done, encoded: video.encoded, encoder: video.encoder.name().to_owned(), size: (vw, vh) };
    video.mp4.finish().map_err(|e| e.to_string())?;
    part.publish(out)?;
    Ok(made)
}

/// The encoder for this export. Chosen for it (Auto), by how much there
/// is to encode (the pictures that change in what's kept, and the skips'):
/// a little on the built-in GPU or the processor, at once, a lot on
/// NVIDIA's, whose start (its GPU woken, its encoder made) takes two
/// seconds and pays for itself past about 600 million pixels (measured
/// 2026-11-15: 195 pictures of 1440 × 900 in 1.9 s on Intel's, 3.5 s on
/// NVIDIA's; 554 of 2160 × 1350 in 9.3 s and 5.9 s).
fn open_encoder(choice: olo_video::Choice, plan: &[Piece], segments: &[record::Segment], w: usize, h: usize) -> Result<olo_video::Encoder, String> {
    use olo_video::{Choice, Codec, Encoder};
    let open = |c: Choice| Encoder::new(w as u32, h as u32, FPS as u32, c, Codec::H264);
    if choice != Choice::Auto {
        return open(choice);
    }
    let records = |from: u64, to: u64| -> u64 { segments.iter().map(|s| (s.frames.partition_point(|f| f.at < to) - s.frames.partition_point(|f| f.at < from)) as u64).sum() };
    let pictures: u64 = plan
        .iter()
        .map(|p| match *p {
            Piece::Screen { from, to } => records(from, to).min((to - from) / STEP_MS) + 1,
            Piece::Skip { from, to } => skip_frames(to - from),
        })
        .sum();
    if pictures * (w * h) as u64 <= 600_000_000 { open(Choice::Vaapi).or_else(|_| open(Choice::Cpu)).or_else(|_| open(Choice::Auto)) } else { open(Choice::Auto) }
}

/// A rectangle of pixels: from x0, y0 up to x1, y1.
type Rect = (usize, usize, usize, usize);

/// `f` (RGBA) into `out` as the video is: its first `w` × `h` (cut, or
/// padded with black), each pixel `z` × `z`, in BGRA. One pass over what
/// the video holds.
fn put(f: &Frame, w: usize, h: usize, z: usize, out: &mut [u8]) {
    let row = w * z * 4;
    let n = w.min(f.width);
    for y in 0..h {
        let line = &mut out[y * z * row..(y * z + 1) * row];
        if y < f.height {
            let src = &f.rgba[y * f.width * 4..(y * f.width + n) * 4];
            // (A pixel a word, red and blue swapped: vectorised.)
            let bgra = |s: &[u8; 4]| {
                let v = u32::from_le_bytes(*s);
                ((v >> 16) & 0xff | (v & 0xff) << 16 | v & 0x0000_ff00 | 0xff00_0000).to_le_bytes()
            };
            let pixels = src.as_chunks::<4>().0;
            if z == 1 {
                for (d, s) in line.as_chunks_mut::<4>().0.iter_mut().zip(pixels) {
                    *d = bgra(s);
                }
            } else {
                for (d, s) in line.chunks_exact_mut(4 * z).zip(pixels) {
                    let px = bgra(s);
                    for o in d.as_chunks_mut::<4>().0 {
                        *o = px;
                    }
                }
            }
            line[n * z * 4..].fill(0);
        } else {
            line.fill(0);
        }
        // The row's copies below it.
        for k in 1..z {
            out.copy_within(y * z * row..(y * z + 1) * row, (y * z + k) * row);
        }
    }
}

/// `(x0, y0, x1, y1)` as whole pixels inside `w` × `h`, a pixel round.
fn covering((x0, y0, x1, y1): (f32, f32, f32, f32), w: usize, h: usize) -> Rect {
    let c = |v: f32, most: usize| (v.max(0.0) as usize).min(most);
    (c(x0 - 1.0, w), c(y0 - 1.0, h), c(x1.ceil() + 1.0, w), c(y1.ceil() + 1.0, h))
}

/// ⏩ (two triangles, not a glyph the face may lack), `size` tall, its
/// left at `x`, on `baseline`.
fn ff_mark(bgra: &mut [u8], w: usize, h: usize, x: f32, baseline: f32, size: f32, color: [u8; 3]) -> f32 {
    let icon = size * 1.3;
    for k in 0..2 {
        let left = x + k as f32 * icon * 0.5;
        triangle(bgra, w, h, [(left, baseline - size * 0.85), (left, baseline + size * 0.05), (left + icon * 0.5, baseline - size * 0.4)], color);
    }
    icon
}

/// A skip's look over the screen: a badge at the top, ⏩ and how long is
/// skipped. Where it drew.
fn badge(bgra: &mut [u8], w: usize, h: usize, how_long: &str) -> Rect {
    let unit = (h as f32 / 40.0).max(8.0);
    let size = unit * 1.3;
    let gap = size * 0.5;
    let font = ui_font().and_then(|d| ab_glyph::FontRef::try_from_slice_and_index(&d.0, d.1).ok());
    let words = font.as_ref().map_or(0.0, |f| text_width(f, size, how_long));
    let width = size * 1.3 + gap + words + 2.0 * unit;
    let (x0, y0) = ((w as f32 - width) / 2.0, unit * 1.5);
    let card = (x0, y0, x0 + width, y0 + size * 2.0);
    round_rect(bgra, w, h, card, size, [24, 26, 32], 0.88);
    let base = y0 + size * 1.42;
    let icon = ff_mark(bgra, w, h, x0 + unit, base, size, [235, 235, 240]);
    if let Some(font) = &font {
        draw_text(bgra, w, h, font, size, x0 + unit + icon + gap, base, how_long, [235, 235, 240]);
    }
    covering(card, w, h)
}

/// `color` (RGB) over the pixel at `i` (BGRA) by `a` (0–1).
fn blend(bgra: &mut [u8], i: usize, color: [u8; 3], a: f32) {
    let a = a.clamp(0.0, 1.0);
    for (k, c) in [color[2], color[1], color[0]].into_iter().enumerate() {
        let bg = bgra[i + k] as f32;
        bgra[i + k] = (bg + (c as f32 - bg) * a).round() as u8;
    }
}

/// A rectangle with corners rounded by `r`, filled with `color` at `alpha`
/// (its edges anti-aliased).
fn round_rect(rgba: &mut [u8], w: usize, h: usize, (x0, y0, x1, y1): (f32, f32, f32, f32), r: f32, color: [u8; 3], alpha: f32) {
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let r = r.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
    for y in (y0.floor().max(0.0) as usize)..(y1.ceil() as usize).min(h) {
        for x in (x0.floor().max(0.0) as usize)..(x1.ceil() as usize).min(w) {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // Distance outside the rounded rectangle (negative inside).
            let (qx, qy) = ((x0 + r - px).max(px - (x1 - r)).max(0.0), (y0 + r - py).max(py - (y1 - r)).max(0.0));
            let d = (qx * qx + qy * qy).sqrt() - r;
            let edge = (x0 - px).max(px - x1).max(y0 - py).max(py - y1);
            let cover = (0.5 - d.max(edge)).clamp(0.0, 1.0);
            if cover > 0.0 {
                blend(rgba, (y * w + x) * 4, color, cover * alpha);
            }
        }
    }
}

/// A filled triangle, four samples a pixel.
fn triangle(rgba: &mut [u8], w: usize, h: usize, p: [(f32, f32); 3], color: [u8; 3]) {
    let side = |a: (f32, f32), b: (f32, f32), x: f32, y: f32| (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0);
    let (lo_x, hi_x) = (p.iter().map(|q| q.0).fold(f32::MAX, f32::min), p.iter().map(|q| q.0).fold(f32::MIN, f32::max));
    let (lo_y, hi_y) = (p.iter().map(|q| q.1).fold(f32::MAX, f32::min), p.iter().map(|q| q.1).fold(f32::MIN, f32::max));
    let inside = |x: f32, y: f32| {
        let (a, b, c) = (side(p[0], p[1], x, y), side(p[1], p[2], x, y), side(p[2], p[0], x, y));
        (a >= 0.0 && b >= 0.0 && c >= 0.0) || (a <= 0.0 && b <= 0.0 && c <= 0.0)
    };
    for y in (lo_y.floor().max(0.0) as usize)..(hi_y.ceil() as usize).min(h) {
        for x in (lo_x.floor().max(0.0) as usize)..(hi_x.ceil() as usize).min(w) {
            let n = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)].iter().filter(|(dx, dy)| inside(x as f32 + dx, y as f32 + dy)).count();
            if n > 0 {
                blend(rgba, (y * w + x) * 4, color, n as f32 / 4.0);
            }
        }
    }
}

fn text_width(font: &ab_glyph::FontRef, size: f32, text: &str) -> f32 {
    use ab_glyph::{Font, ScaleFont};
    let sf = font.as_scaled(ab_glyph::PxScale::from(size));
    text.chars().map(|c| sf.h_advance(font.glyph_id(c))).sum()
}

/// `text` at `size` pixels, its baseline at `baseline`, from `x`.
#[allow(clippy::too_many_arguments)]
fn draw_text(rgba: &mut [u8], w: usize, h: usize, font: &ab_glyph::FontRef, size: f32, mut x: f32, baseline: f32, text: &str, color: [u8; 3]) {
    use ab_glyph::{Font, ScaleFont};
    let scale = ab_glyph::PxScale::from(size);
    let sf = font.as_scaled(scale);
    for c in text.chars() {
        let id = font.glyph_id(c);
        if let Some(o) = font.outline_glyph(id.with_scale_and_position(scale, ab_glyph::point(x, baseline))) {
            let b = o.px_bounds();
            o.draw(|gx, gy, cov| {
                let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                    blend(rgba, (py as usize * w + px as usize) * 4, color, cov);
                }
            });
        }
        x += sf.h_advance(id);
    }
}

/// Olo's UI face (Olo's own fonts first, then the system's): its file's
/// bytes and the face's index in it, read once.
fn ui_font() -> Option<&'static (Vec<u8>, u32)> {
    static FONT: std::sync::OnceLock<Option<(Vec<u8>, u32)>> = std::sync::OnceLock::new();
    FONT.get_or_init(|| {
        let query = fontdb::Query { families: &[fontdb::Family::Name("Noto Sans"), fontdb::Family::SansSerif], weight: fontdb::Weight::NORMAL, ..Default::default() };
        let mut own = fontdb::Database::new();
        if let Some(dir) = olo_xdg::fonts_dir() {
            own.load_fonts_dir(dir);
        }
        let mut system = fontdb::Database::new();
        system.load_system_fonts();
        [own, system].into_iter().find_map(|db| {
            let id = db.query(&query)?;
            db.with_face_data(id, |data, index| (data.to_vec(), index))
        })
    })
    .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_happened_is_kept_the_rest_skipped() {
        // Two close together, one a while after, a long wait.
        // (Each skip with 5 s more of the screen on either side.)
        let p = pieces(&[(10_000, 300), (11_000, 200), (100_000, 500), (200_000, 60_000)]);
        assert_eq!(
            p,
            [
                Piece::Screen { from: 9_200, to: 17_700 },
                Piece::Skip { from: 17_700, to: 94_200 },
                Piece::Screen { from: 94_200, to: 107_000 },
                Piece::Skip { from: 107_000, to: 194_200 },
                // The wait: its start and its end.
                Piece::Screen { from: 194_200, to: 206_500 },
                Piece::Skip { from: 206_500, to: 254_200 },
                Piece::Screen { from: 254_200, to: 261_500 },
            ]
        );
        // Closer than a play-through and the time kept either side: they
        // play through.
        assert_eq!(pieces(&[(10_000, 0), (24_000, 0)]), [Piece::Screen { from: 9_200, to: 25_500 }]);
        assert_eq!(pieces(&[(10_000, 0), (25_400, 0)]).len(), 3);
        assert_eq!((skip_frames(5_000), skip_frames(86_500)), (5, 15));
        assert_eq!(span(252_000), "4 min 12 s");
        assert!(pieces(&[]).is_empty());
    }

    #[test]
    fn save_shot_zooms_png_accurately() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-pane-shot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let frame = Frame { width: 2, height: 2, rgba: vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255] };
        let path = dir.join("shot.png");
        save_shot(&frame, 2, &path).unwrap();
        let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()));
        let mut reader = dec.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (4, 4));
        assert_eq!(&buf[0..4], &[255, 0, 0, 255]);
        assert_eq!(&buf[4..8], &[255, 0, 0, 255]);
        assert_eq!(&buf[8..12], &[0, 255, 0, 255]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A screen's colours come out as they were (red stays red: the
    /// pictures go to the encoder in BGRA), and a skip's badge over it.
    #[test]
    fn colours_come_out_as_they_were() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-export-colours-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (w, h) = (320usize, 200usize);
        let base = 1_700_000_000_000u64;
        let red = Frame { width: w, height: h, rgba: [230u8, 60, 40, 255].repeat(w * h) };
        record::write_for_test(&dir, vec![(red.clone(), base, false), (red, base + 100_000, true)]);
        let out = dir.join("out.mp4");
        export_from(&[(base + 1_000, 100), (base + 99_000, 100)], &record::index_dir(&dir), olo_video::Choice::Cpu, 1, &out, &mut |_| true).unwrap();
        let px = |at: f64, x: usize, y: usize| {
            let image = olo_preview::video::picture_at(&olo_preview::input::Input::file(&out), at).unwrap();
            let i = (y * image.size[0] + x) * 4;
            [image.rgba[i], image.rgba[i + 1], image.rgba[i + 2]]
        };
        let near = |a: [u8; 3], b: [u8; 3]| a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 12);
        let screen = px(0.5, 20, 20);
        assert!(near(screen, [230, 60, 40]), "the screen came out {screen:?}");
        // In the skip (from 7.4 s, the first action's 2.4 s and 5 s more),
        // the badge at the top, dark over the red; the screen below it as
        // it was, not dimmed.
        let badge = px(8.0, w / 2, 14);
        assert!(badge.iter().all(|&c| c < 90), "the badge came out {badge:?}");
        let under = px(8.0, 20, h / 2);
        assert!(near(under, [230, 60, 40]), "the screen under the badge came out {under:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Twelve hours of a screen changing by itself (a keyframe every 30
    /// s, a clock, something animating), two bursts of actions: the video
    /// is those and a fast-forward, and only pictures that changed are
    /// encoded.
    #[test]
    fn twelve_hours_are_what_was_done_and_cost_little() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-export-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (w, h) = (320usize, 200usize);
        let base = 1_700_000_000_000u64;
        let screen = |block: usize, clock: u8| {
            let mut f = Frame { width: w, height: h, rgba: [40u8, 60, 90, 255].repeat(w * h) };
            for y in block..(block + 30).min(h) {
                for x in 0..256 {
                    f.rgba[(y * w + x) * 4] = 230;
                }
            }
            f.rgba[(195 * w + 300) * 4 + 1] = clock;
            f
        };
        let mut frames = Vec::new();
        for i in 0..50u64 {
            frames.push((screen(i as usize * 3, 0), base + i * 100, false));
        }
        let quiet = 12 * 3600 * 1000;
        let mut t = base + 5_000;
        while t < base + quiet {
            // A keyframe every 30 s, a minute's tick every 60 s, and
            // something animating between.
            frames.push((screen(147, ((t - base) / 60_000) as u8), t, true));
            frames.push((screen(140 + ((t / 30_000) % 7) as usize, ((t - base) / 60_000) as u8), t + 10_000, false));
            t += 30_000;
        }
        for i in 0..50u64 {
            frames.push((screen(150 - i as usize * 3, 200), base + quiet + i * 100, false));
        }
        record::write_for_test(&dir, frames);
        let segments = record::index_dir(&dir);
        // (The second's result: the screen still for its last 2.3 s.)
        let actions = [(base + 1_000, 300), (base + 5_500, 300), (base + quiet + 2_000, 400)];
        let out = dir.join("out.mp4");
        let started = std::time::Instant::now();
        let Made { encoded, frames: shown, .. } = export_from(&actions, &segments, olo_video::Choice::Cpu, 1, &out, &mut |_| true).unwrap();
        let took = started.elapsed();
        eprintln!("12 hours: {shown} frames, {encoded} pictures encoded, in {took:?}");
        assert!(out.is_file());
        // 0.8 s before the first action to 1.5 s after the second and 5 s
        // more (0.2 to 12.3 s), the fast-forward, and 5 s and the last
        // action's 0.8 + 0.4 + 1.5 s.
        assert_eq!(shown, 121 + 15 + 77);
        // The still 2.3 s after the second action not encoded again (a
        // keyframe in it, once).
        assert!(encoded <= shown - 20, "{encoded} of {shown} encoded");
        // A part of it, the last action alone, at twice the size.
        let part = export_from(&actions[2..], &segments, olo_video::Choice::Cpu, 2, &out, &mut |_| true).unwrap();
        assert_eq!((part.frames, part.size), (27, (640, 400)));
        assert_eq!(zooms(320, 200), [1, 2, 3]);
        assert_eq!(zooms(2160, 1350), [1], "4320 wide is past H.264's 4096");
        assert_eq!(zooms(1440, 900), [1, 2]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn review2_f2_exclusive_part_preserves_existing_part_and_refuses_symlink() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-pane-r2-f2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Pre-existing sibling `shot.png.part` belonging to someone else must
        // NEVER be truncated or deleted by `save_shot` (success or failure).
        let bystander = dir.join("shot.png.part");
        std::fs::write(&bystander, b"important-unrelated-data").unwrap();
        let target = dir.join("shot.png");

        let frame = Frame { width: 2, height: 2, rgba: vec![10u8; 16] };
        save_shot(&frame, 1, &target).unwrap();
        assert_eq!(std::fs::read(&bystander).unwrap(), b"important-unrelated-data", "bystander .part file must remain untouched");
        assert!(target.is_file());

        // Two concurrent ExclusivePartFile creators for the same destination
        // get distinct paths and dropping one never deletes the other.
        let p1 = ExclusivePartFile::create_beside(&target).unwrap();
        let p2 = ExclusivePartFile::create_beside(&target).unwrap();
        assert_ne!(p1.path(), p2.path());
        assert!(p1.path().starts_with(&dir) && p2.path().starts_with(&dir));
        let p2_path = p2.path().to_path_buf();
        drop(p1);
        assert!(p2_path.is_file(), "dropping p1 must not unlink p2's temp file");
        drop(p2);
        assert!(!p2_path.exists(), "dropping p2 must unlink its own temp file");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn review2_f22_save_shot_rejects_oversize_zoom_without_allocating() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-pane-r2-f22-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("huge.png");

        // 8192x8192 @ 3x = 24576x24576 (2.25 GiB RGBA) must be refused before allocating.
        let frame = Frame { width: 8192, height: 8192, rgba: vec![0u8; 8192 * 8192 * 4] };
        let err = save_shot(&frame, 3, &out).unwrap_err();
        assert!(err.contains("exceeds maximum"), "unexpected error: {err}");
        assert!(!out.exists());

        // Short/corrupt source buffer is also refused without panicking.
        let bad = Frame { width: 100, height: 100, rgba: vec![0u8; 16] };
        assert!(save_shot(&bad, 1, &out).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn review2_f44_parse_part_spec_and_has_gaps() {
        assert_eq!(parse_part_spec("2-4,8", 10).unwrap(), vec![2, 3, 4, 8]);
        assert!(has_gaps(&[2, 3, 4, 8]));
        assert!(!has_gaps(&[2, 3, 4]));
        assert!(!has_gaps(&[4, 2, 3]));
        assert!(!has_gaps(&[5]));
        assert!(!has_gaps(&[]));
        assert_eq!(parse_part_spec("0,0,1-2", 5).unwrap(), vec![0, 1, 2]);
        assert!(parse_part_spec("4-2", 10).is_err());
        assert!(parse_part_spec("99", 5).is_err());
        assert!(parse_part_spec("", 5).is_err());
    }
}
