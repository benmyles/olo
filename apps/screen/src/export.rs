//! A screen's history as an MP4 video, made in process: the agent's
//! actions (each from a moment before it to a moment after it ended, its
//! result showing) played as they went, at 10 frames a second, and the
//! time between them skipped (Ben, 2026-11-15: "only include the actual
//! parts where actions/events occurred, skipping over all the
//! inactivity"). A skip shows the screen across what it skips, quickly:
//! under `CARD_FROM_MS` in half a second under a badge saying how long; a
//! longer one in `FF_MS`, dimmed under a card, "Fast-forwarding 4 h 12
//! min", the time of day racing on ("like it is fast forwarding through
//! time to the next point where activity starts"). The screen changing
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

use std::path::Path;

use crate::{
    history,
    record::{self, Frame, Player},
};

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
/// A skip this long or longer is the fast-forward card, in `FF_MS`;
/// shorter, a quick skip in `QUICK_MS`, under a badge.
const CARD_FROM_MS: u64 = 30_000;
const FF_MS: u64 = 1500;
const QUICK_MS: u64 = 500;

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
            let end = at + ms;
            if ms > LONG_MS { vec![(at.saturating_sub(BEFORE_MS), at + AFTER_MS), (end.saturating_sub(BEFORE_MS), end + AFTER_MS)] } else { vec![(at.saturating_sub(BEFORE_MS), end + AFTER_MS)] }
        })
        .collect();
    spans.sort_unstable();
    let mut out = Vec::new();
    let Some(&(mut from, mut to)) = spans.first() else { return out };
    for &(a, b) in &spans[1..] {
        if a <= to + PLAY_THROUGH_MS {
            to = to.max(b);
            continue;
        }
        out.push(Piece::Screen { from, to });
        out.push(Piece::Skip { from: to, to: a });
        (from, to) = (a, b);
    }
    out.push(Piece::Screen { from, to });
    out
}

/// How many pictures a skip of `ms` takes.
fn skip_frames(ms: u64) -> u64 {
    (if ms >= CARD_FROM_MS { FF_MS } else { QUICK_MS } / STEP_MS).max(2)
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

/// "2:31 PM" (local), at `ms` since the epoch.
fn clock(ms: u64) -> String {
    let t = olo_kit::clock::local((ms / 1000) as i64);
    let h = match t.hour % 12 {
        0 => 12,
        h => h,
    };
    format!("{h}:{:02} {}", t.minute, if t.hour < 12 { "AM" } else { "PM" })
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
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Made {
    pub frames: u64,
    pub encoded: u64,
    pub encoder: &'static str,
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
    let tmp = out.with_extension("mp4.part");
    // (Each frame a tenth of a second: FPS × 1000 ticks a second.)
    let mp4 = olo_video::Mp4::create_for(&tmp, vw as u32, vh as u32, FPS as u32 * 1000, encoder.codec()).map_err(|e| e.to_string())?;
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
                    // A few of the screen's pictures across it, each kept
                    // for frames on end (the card on it drawn anew for
                    // each: its time and its bar move).
                    let n = skip_frames(to - from);
                    let card = to - from >= CARD_FROM_MS;
                    let samples = if card { 5 } else { 2 };
                    let mut sampled = None;
                    let mut drawn: Option<Rect> = None;
                    for i in 0..n {
                        let s = i * samples / n;
                        let t = from + (to - from) * s / (samples - 1).max(1);
                        if sampled != Some(s) {
                            sampled = Some(s);
                            if player.at(t.max(first)).is_some() {
                                put(&player.frame, w, h, z, &mut under);
                            }
                            if card {
                                dim(&mut under);
                            }
                            pic.copy_from_slice(&under);
                        } else if !card {
                            // (A quick skip's badge doesn't change: the same
                            // picture, longer.)
                            if video.again() {
                                step(&mut done)?;
                                continue;
                            }
                        } else if let Some(r) = drawn {
                            restore(&mut pic, &under, vw, r);
                        }
                        let frame_t = from + (to - from) * i / (n - 1);
                        drawn = Some(if card { fast_forward(&mut pic, vw, vh, &span(to - from), &clock(frame_t), i as f32 / (n - 1) as f32) } else { badge(&mut pic, vw, vh, &span(to - from)) });
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
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let made = Made { frames: done, encoded: video.encoded, encoder: video.encoder.name(), size: (vw, vh) };
    video.mp4.finish().map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, out).map_err(|e| e.to_string())?;
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

/// The screen dimmed, under a fast-forward's card.
fn dim(bgra: &mut [u8]) {
    for b in bgra.iter_mut() {
        *b = ((*b as u16 * 102) >> 8) as u8;
    }
}

/// What `r` held in `under`, back in `pic` (both `w` wide).
fn restore(pic: &mut [u8], under: &[u8], w: usize, (x0, y0, x1, y1): Rect) {
    for y in y0..y1 {
        let (a, b) = ((y * w + x0) * 4, (y * w + x1) * 4);
        pic[a..b].copy_from_slice(&under[a..b]);
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

/// A quick skip's look over the screen: a badge at the top, ⏩ and how
/// long is skipped. Where it drew.
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

/// A fast-forward's card over the (dimmed) screen in `bgra`: ⏩,
/// "Fast-forwarding" and how long, the time of day there big below, and a
/// bar for how far through (`done`, 0–1). Where it drew.
fn fast_forward(bgra: &mut [u8], w: usize, h: usize, how_long: &str, time: &str, done: f32) -> Rect {
    let unit = (h as f32 / 40.0).max(8.0);
    let (cw, ch) = ((w as f32 * 0.42).max(unit * 24.0).min(w as f32 - 2.0 * unit), unit * 9.0);
    let (x0, y0) = ((w as f32 - cw) / 2.0, (h as f32 - ch) / 2.0);
    let card = (x0, y0, x0 + cw, y0 + ch);
    round_rect(bgra, w, h, card, unit * 0.8, [24, 26, 32], 0.92);
    if let Some(font) = ui_font().and_then(|d| ab_glyph::FontRef::try_from_slice_and_index(&d.0, d.1).ok()) {
        // ⏩ and the words, as one line.
        let small = unit * 1.3;
        let words = format!("Fast-forwarding {how_long}");
        let line = small * 1.3 + small * 0.6 + text_width(&font, small, &words);
        let (x, base) = ((w as f32 - line) / 2.0, y0 + unit * 2.7);
        let icon = ff_mark(bgra, w, h, x, base, small, [235, 235, 240]);
        draw_text(bgra, w, h, &font, small, x + icon + small * 0.6, base, &words, [235, 235, 240]);
        // The time there, racing on.
        let big = unit * 3.2;
        draw_text(bgra, w, h, &font, big, (w as f32 - text_width(&font, big, time)) / 2.0, y0 + unit * 6.6, time, [255, 255, 255]);
    }
    // How far through.
    let (bx0, bx1, by) = (x0 + unit * 1.5, x0 + cw - unit * 1.5, y0 + ch - unit * 1.4);
    round_rect(bgra, w, h, (bx0, by, bx1, by + unit * 0.35), unit * 0.17, [70, 74, 84], 1.0);
    round_rect(bgra, w, h, (bx0, by, bx0 + (bx1 - bx0) * done.clamp(0.0, 1.0), by + unit * 0.35), unit * 0.17, [120, 170, 255], 1.0);
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
        let p = pieces(&[(10_000, 300), (11_000, 200), (100_000, 500), (200_000, 60_000)]);
        assert_eq!(
            p,
            [
                Piece::Screen { from: 9_200, to: 12_700 },
                Piece::Skip { from: 12_700, to: 99_200 },
                Piece::Screen { from: 99_200, to: 102_000 },
                Piece::Skip { from: 102_000, to: 199_200 },
                // The wait: its start and its end.
                Piece::Screen { from: 199_200, to: 201_500 },
                Piece::Skip { from: 201_500, to: 259_200 },
                Piece::Screen { from: 259_200, to: 261_500 },
            ]
        );
        assert_eq!((skip_frames(5_000), skip_frames(86_500)), (5, 15));
        assert_eq!(span(252_000), "4 min 12 s");
        assert!(pieces(&[]).is_empty());
    }

    /// A screen's colours come out as they were (red stays red: the
    /// pictures go to the encoder in BGRA), and the cards' too.
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
        // In the fast-forward (from 2.4 s), the card's bar: blue.
        let bar = px(3.8, w / 2 - 40, h / 2 + 30);
        assert!(bar[2] > bar[0], "the bar came out {bar:?}");
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
        // 0.8 s before the first action to 1.5 s after the second (0.2 to
        // 7.3 s), the fast-forward, and the last action's 0.8 + 0.4 + 1.5 s.
        assert_eq!(shown, 71 + 15 + 27);
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
}
