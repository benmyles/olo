//! Layout review: what a frame looks like it got wrong, read from what it
//! painted. Where the layout lint ([`crate::layout_lint`]) proves content
//! fits, the review reads one frame's shapes (text with its baseline,
//! filled bands, one-point lines, icons) at the scale it was drawn at and
//! finds what tends to look off.
//!
//! Errors, which tests fail on:
//! - an icon between device pixels, or not a whole number of them: it
//!   blurs;
//! - one-point lines of different weights (each edge rounded to the
//!   nearest device pixel on its own: at 1.5× some come out 1 px, some
//!   2 px);
//! - an outline whose width isn't a whole number of device pixels (a
//!   1-point stroke at 1.5×): it blurs;
//! - a seam: a line and what it borders, meant to touch (less than a
//!   point apart), a device pixel apart (an inner bevel placed a point in
//!   from a line at 1.5×, where a point is a pixel and a half: inset by
//!   [`crate::style::hairline`] from a rect on device pixels instead).
//!
//! For review, each a place to look at closely (most are mistakes, a few
//! are meant):
//! - bands stacked on each other, or side by side, almost but not quite
//!   matching (a 21-point menu bar over a 22-point header);
//! - text or icons almost but not quite level with their neighbour in
//!   the next column;
//! - left edges almost but not quite lined up down a column;
//! - text or an icon off-centre in the band it sits in;
//! - rows down a column spaced unevenly.
//!
//! [`review`] runs on any frame's shapes: `layout_lint::review` over a
//! view in a test, and every kit app's windows on request
//! (`{"review":true}` on its socket: `olo-screen review`, `tsession
//! review`). Text on a grid of its own ([`own_grid`]: a terminal's
//! screen) isn't held level with what is beside it; what is centred in
//! an area nothing fills ([`band`]: a tab hanging in a strip) is
//! measured against that. See "Layout review" in docs/HACKING.md.

use egui::{Context, Id, Pos2, Rect, Shape, TextureId, Ui, epaint::ClippedShape};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Wrong at this scale: fix it.
    Error,
    /// Looks off: look closely, and fix it unless it is meant.
    Review,
}

/// One thing the review found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub level: Level,
    /// What kind: "blurry-icon", "line-weights", "blurry-outline",
    /// "seam", "near-height", "near-level", "near-edge", "off-centre",
    /// "uneven-rows".
    pub kind: String,
    pub text: String,
    /// Where (the first thing it is about), in the window's logical
    /// pixels.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = match self.level {
            Level::Error => "error",
            Level::Review => "review",
        };
        write!(f, "{level} {}: {} (at {:.0},{:.0} {:.0}×{:.0})", self.kind, self.text, self.x, self.y, self.width, self.height)
    }
}

/// How many of each kind a frame reports (the rest are like them).
const MOST: usize = 8;
/// Closer than this, two things count as lined up (a rounding).
const SAME: f32 = 0.5;
/// Further than this, a difference looks meant rather than missed.
const NEAR_EDGE: f32 = 4.0;
const NEAR_HEIGHT: f32 = 3.0;

#[derive(Debug, Clone)]
struct Text {
    rect: Rect,
    /// The first line's baseline, and the middle of its capitals (what
    /// `style::label` centres).
    baseline: f32,
    middle: f32,
    /// Where the layout started it (not where the first glyph's ink is).
    left: f32,
    label: String,
    size: f32,
}

#[derive(Debug, Clone, Copy)]
struct Fill {
    rect: Rect,
    /// Its top, bottom, left and right edges where a clip cut it (the
    /// clip's, not its own).
    cut: [bool; 4],
}

/// What a frame told the review about its layout, beyond what it
/// painted: [`marks`] after the pass.
#[derive(Debug, Clone, Default)]
pub struct Marks {
    /// Areas on a grid of their own ([`own_grid`]).
    pub grids: Vec<Rect>,
    /// Areas what is in them is centred across, though nothing fills
    /// them ([`band`]).
    pub bands: Vec<Rect>,
    /// Each texture's size in pixels: an icon drawn a pixel or two off
    /// its texture's size is resampled.
    pub textures: std::collections::HashMap<TextureId, [usize; 2]>,
}

fn marks_id() -> Id {
    Id::unique("olo-layout-review-marks")
}

fn mark(ui: &Ui, add: impl FnOnce(&mut Marks)) {
    let pass = ui.ctx().cumulative_pass_nr();
    ui.ctx().data_mut(|d| {
        let (at, marks) = d.get_temp_mut_or_default::<(u64, Marks)>(marks_id());
        if *at != pass {
            *at = pass;
            *marks = Marks::default();
        }
        add(marks);
    });
}

/// The text in `rect` is laid out on a grid of its own, not the
/// window's (a terminal's screen: its lines are a cell apart at any text
/// size): the review doesn't hold it level with the chrome beside it,
/// which it can only be at some sizes. Call it each frame.
pub fn own_grid(ui: &Ui, rect: Rect) {
    mark(ui, |m| m.grids.push(rect));
}

/// What is in `rect` is centred across it, though nothing is painted to
/// show it (a tab hanging in a strip, its top below the strip's): the
/// review measures that centring against `rect`, not the fill around it.
/// Call it each frame.
pub fn band(ui: &Ui, rect: Rect) {
    mark(ui, |m| m.bands.push(rect));
}

/// What the pass just run marked with [`own_grid`] and [`band`].
pub fn marks(ctx: &Context) -> Marks {
    let pass = ctx.cumulative_pass_nr();
    let mut marks = ctx.data(|d| d.get_temp::<(u64, Marks)>(marks_id())).filter(|(at, _)| at + 1 == pass).map(|(_, m)| m).unwrap_or_default();
    marks.textures = ctx.tex_manager().read().allocated().map(|(id, meta)| (*id, meta.size)).collect();
    marks
}

#[derive(Debug, Default)]
struct Parts {
    /// What the frame marked.
    marks: Marks,
    texts: Vec<Text>,
    fills: Vec<Fill>,
    /// Everything opaque painted over an area (fills and gradients): what
    /// a crack between two of them might be covered by.
    covers: Vec<Rect>,
    icons: Vec<Icon>,
    /// Outlines' widths and where (rect strokes, straight segments).
    strokes: Vec<(f32, Rect)>,
}

/// A picture drawn from a texture: where, and from which texture (all of
/// it when `whole`).
#[derive(Debug)]
struct Icon {
    rect: Rect,
    texture: TextureId,
    whole: bool,
}

fn collect(shape: &Shape, clip: Rect, out: &mut Parts) {
    match shape {
        Shape::Vec(shapes) => {
            for s in shapes {
                collect(s, clip, out);
            }
        }
        Shape::Rect(r) => {
            let seen = r.rect.intersect(clip);
            if !seen.is_positive() {
                return;
            }
            if r.fill.a() > 0 {
                let cut = [r.rect.min.y < seen.min.y, r.rect.max.y > seen.max.y, r.rect.min.x < seen.min.x, r.rect.max.x > seen.max.x];
                out.fills.push(Fill { rect: seen, cut });
                out.covers.push(seen);
            }
            if r.stroke.width > 0.0 && r.stroke.color.a() > 0 {
                out.strokes.push((r.stroke.width, seen));
            }
        }
        Shape::LineSegment { points: [a, b], stroke } => {
            let straight = (a.x - b.x).abs() < 0.01 || (a.y - b.y).abs() < 0.01;
            let seen = Rect::from_two_pos(*a, *b).intersect(clip.expand(stroke.width));
            if straight && stroke.width > 0.0 && stroke.color.a() > 0 && (seen.width() > 0.0 || seen.height() > 0.0) {
                out.strokes.push((stroke.width, Rect::from_two_pos(*a, *b)));
            }
        }
        Shape::Text(t) => {
            let label: String = t.galley.job.text.split('\n').next().unwrap_or("").trim().chars().take(28).collect();
            let Some(row) = t.galley.rows.first() else { return };
            if row.row.glyphs.is_empty() {
                return;
            }
            let rect = t.galley.rect.translate(t.pos.to_vec2());
            // Text a clip cuts (in a scroll view) is left alone.
            if label.is_empty() || t.angle != 0.0 || !clip.expand(SAME).contains_rect(rect) {
                return;
            }
            let size = t.galley.job.sections.first().map_or(0.0, |s| s.format.font_id.size);
            let baseline = t.pos.y + crate::style::baseline_offset(&t.galley);
            let middle = baseline - size * crate::style::CAP_HEIGHT / 2.0;
            out.texts.push(Text { rect, baseline, middle, left: t.pos.x + row.pos.x, label, size });
        }
        Shape::Mesh(m) if m.texture_id != TextureId::default() && !m.vertices.is_empty() => {
            let (mut r, mut uv) = (Rect::NOTHING, Rect::NOTHING);
            for v in &m.vertices {
                r.extend_with(v.pos);
                uv.extend_with(v.uv);
            }
            // Pictures (thumbnails, previews) are scaled anyway; icons
            // are drawn at their size.
            if r.width() <= 128.0 && r.height() <= 128.0 && r.intersects(clip) {
                let whole = uv.min == Pos2::ZERO && uv.max == egui::pos2(1.0, 1.0);
                out.icons.push(Icon { rect: r, texture: m.texture_id, whole });
            }
        }
        Shape::Mesh(m) if m.texture_id == TextureId::default() && !m.vertices.is_empty() => {
            // A gradient (`style::vgradient`): it covers its bounds.
            let mut r = Rect::NOTHING;
            for v in &m.vertices {
                r.extend_with(v.pos);
            }
            if m.vertices.iter().all(|v| v.color.a() == 255) {
                out.covers.push(r.intersect(clip));
            }
        }
        _ => {}
    }
}

/// What looks wrong in a frame of `shapes` drawn at `ppp` device pixels
/// a point in a window of `size` (logical), errors first; `marks`, what
/// the frame marked ([`marks`]).
pub fn review(shapes: &[ClippedShape], size: egui::Vec2, ppp: f32, marks: &Marks) -> Vec<Finding> {
    let mut parts = Parts { marks: marks.clone(), ..Default::default() };
    for s in shapes {
        collect(&s.shape, s.clip_rect, &mut parts);
    }
    let screen = Rect::from_min_size(Pos2::ZERO, size);
    let mut out = Vec::new();
    blurry_icons(&parts, ppp, &mut out);
    line_weights(&parts, ppp, &mut out);
    blurry_outlines(&parts, ppp, &mut out);
    seams(&parts, ppp, &mut out);
    off_pixels(&parts, screen, ppp, &mut out);
    near_heights(&parts, screen, &mut out);
    near_level(&parts, &mut out);
    near_edges(&parts, &mut out);
    off_centre(&parts, &mut out);
    uneven_rows(&parts, ppp, &mut out);
    out.sort_by_key(|f| f.level);
    out
}

fn finding(level: Level, kind: &str, text: String, at: Rect) -> Finding {
    Finding { level, kind: kind.into(), text, x: at.min.x, y: at.min.y, width: at.width(), height: at.height() }
}

/// Keep the first [`MOST`] of each kind.
fn push(out: &mut Vec<Finding>, f: Finding) {
    if out.iter().filter(|o| o.kind == f.kind).count() < MOST && !out.iter().any(|o| o.kind == f.kind && o.text == f.text) {
        out.push(f);
    }
}

fn whole(v: f32) -> bool {
    (v - v.round()).abs() < 0.02
}

fn quote(t: &Text) -> String {
    format!("“{}”", t.label)
}

fn blurry_icons(p: &Parts, ppp: f32, out: &mut Vec<Finding>) {
    for icon in &p.icons {
        let r = icon.rect;
        let (x, y, w, h) = (r.min.x * ppp, r.min.y * ppp, r.width() * ppp, r.height() * ppp);
        if !(whole(x) && whole(y) && whole(w) && whole(h)) {
            let text = format!("an icon at {x:.2},{y:.2} ({w:.2}×{h:.2} device px) isn't on whole device pixels at {ppp}×: it blurs (style::snap it)");
            push(out, finding(Level::Error, "blurry-icon", text, r));
            continue;
        }
        // On whole pixels, but a pixel or two off the texture's own size
        // (its corners snapped one by one): resampled all the same. Further
        // off, it is a picture scaled on purpose.
        let Some(&[tw, th]) = p.marks.textures.get(&icon.texture) else { continue };
        let off = |a: f32, b: usize| (a.round() - b as f32).abs();
        let (dw, dh) = (off(w, tw), off(h, th));
        if icon.whole && dw.max(dh) > 0.0 && dw.max(dh) <= 2.0 {
            let text = format!("an icon drawn {w:.0}×{h:.0} device px from a {tw}×{th} texture at {ppp}×: resampled a pixel off its size, it blurs (draw it at its texture's size, as icons::paint does)");
            push(out, finding(Level::Error, "blurry-icon", text, r));
        }
    }
}

/// The device pixels egui fills for `a..b` (it rounds each edge to the
/// nearest).
fn device_span(a: f32, b: f32, ppp: f32) -> i32 {
    ((b * ppp).round() - (a * ppp).round()) as i32
}

fn line_weights(p: &Parts, ppp: f32, out: &mut Vec<Finding>) {
    // One-point lines (`style::hline`, `vline`, `frame_rect`), by how
    // many device pixels thick they come out.
    let mut by: std::collections::BTreeMap<i32, Vec<Rect>> = Default::default();
    for f in &p.fills {
        let r = f.rect;
        let (thin, long) = if (r.height() - 1.0).abs() < 0.01 && r.width() >= 3.0 {
            (device_span(r.min.y, r.max.y, ppp), true)
        } else if (r.width() - 1.0).abs() < 0.01 && r.height() >= 3.0 {
            (device_span(r.min.x, r.max.x, ppp), true)
        } else {
            (0, false)
        };
        if long {
            by.entry(thin).or_default().push(r);
        }
    }
    if by.len() > 1 {
        let total: usize = by.values().map(Vec::len).sum();
        let (_, fewest) = by.iter().min_by_key(|(_, v)| v.len()).expect("two weights");
        let weights: Vec<String> = by.iter().map(|(w, v)| format!("{} at {w} px", v.len())).collect();
        let text = format!("one-point lines come out at different weights at {ppp}× ({} of {total}): draw lines whole device pixels thick", weights.join(", "));
        push(out, finding(Level::Error, "line-weights", text, fewest[0]));
    }
}

fn blurry_outlines(p: &Parts, ppp: f32, out: &mut Vec<Finding>) {
    let blurry: Vec<&(f32, Rect)> = p.strokes.iter().filter(|(w, _)| !whole(w * ppp)).collect();
    if let Some((w, r)) = blurry.first() {
        let text = format!("{} outline(s) {w} pt wide are {:.2} device px at {ppp}×: they blur (make strokes whole device pixels)", blurry.len(), w * ppp);
        push(out, finding(Level::Error, "blurry-outline", text, *r));
    }
}

/// Fills laid out between device pixels. egui rounds each edge to the
/// nearest, so one comes out a pixel taller, shorter or further over
/// than laid out: rows of one height end up of two, a bar a pixel off
/// the band beside it, and the text centred in it a pixel off its middle
/// (`style::text_origin` rounds on its own). Sizes on the 2-point grid
/// (`metrics`) are whole device pixels at 1×, 1.5× and 2×; what is
/// worked out (centred, divided) wants `style::snap`. At a scale the grid
/// can't land on (1.25×, 1.75×: two points are 2.5 and 3.5 pixels) the
/// edges are still crisp, each rounded, and spacing may differ by a
/// pixel; nothing to fix there.
fn off_pixels(p: &Parts, screen: Rect, ppp: f32, out: &mut Vec<Finding>) {
    if !whole(2.0 * ppp) {
        return;
    }
    for f in &p.fills {
        let r = f.rect;
        // A line's ends round with what they meet: only its sides count.
        let (wide, tall) = (r.width() > 1.01, r.height() > 1.01);
        let (rows, cols) = (wide || !tall, tall || !wide);
        let edges = [("top", r.min.y, rows, screen.min.y), ("bottom", r.max.y, rows, screen.max.y), ("left", r.min.x, cols, screen.min.x), ("right", r.max.x, cols, screen.max.x)];
        // At the window's own edge there is nothing past it to be off from.
        let off: Vec<String> = edges
            .iter()
            .zip(f.cut)
            .filter(|((_, v, judged, window), cut)| *judged && !cut && (v - window).abs() > 0.01 && !whole(v * ppp))
            .map(|((side, v, ..), _)| format!("{side} {v}"))
            .collect();
        if !off.is_empty() {
            let text = format!("a fill {:.1}×{:.1} has edges between device pixels at {ppp}× ({}): it comes out a pixel off what it lines up with (keep sizes on the 2-point grid, style::snap what is worked out)", r.width(), r.height(), off.join(", "));
            push(out, finding(Level::Error, "off-pixel", text, r));
        }
    }
}

/// A line (a fill a point thick or less) and a fill beside it along its
/// length, less than a point apart but a whole device pixel or more
/// apart once drawn: what shows through looks like a crack.
fn seams(p: &Parts, ppp: f32, out: &mut Vec<Finding>) {
    let px = |v: f32| (v * ppp).round();
    for (i, l) in p.fills.iter().enumerate() {
        let l = l.rect;
        let horizontal = l.height() <= 1.01 && l.width() >= 3.0;
        let vertical = l.width() <= 1.01 && l.height() >= 3.0;
        if !horizontal && !vertical {
            continue;
        }
        for (j, f) in p.fills.iter().enumerate() {
            let f = f.rect;
            if i == j {
                continue;
            }
            // Along the line: shared for two device pixels or more.
            let (along, gaps) = if horizontal {
                (px(l.max.x.min(f.max.x)) - px(l.min.x.max(f.min.x)), [(f.min.y - l.max.y, px(f.min.y) - px(l.max.y)), (l.min.y - f.max.y, px(l.min.y) - px(f.max.y))])
            } else {
                (px(l.max.y.min(f.max.y)) - px(l.min.y.max(f.min.y)), [(f.min.x - l.max.x, px(f.min.x) - px(l.max.x)), (l.min.x - f.max.x, px(l.min.x) - px(f.max.x))])
            };
            if along < 2.0 {
                continue;
            }
            for (k, (gap, device)) in gaps.into_iter().enumerate() {
                // The crack's middle, in points: covered by something else?
                let mid_along = if horizontal { (l.min.x.max(f.min.x) + l.max.x.min(f.max.x)) / 2.0 } else { (l.min.y.max(f.min.y) + l.max.y.min(f.max.y)) / 2.0 };
                let (a, b) = if horizontal { if k == 0 { (l.max.y, f.min.y) } else { (f.max.y, l.min.y) } } else if k == 0 { (l.max.x, f.min.x) } else { (f.max.x, l.min.x) };
                // The device pixel just past the line's edge, its centre.
                let first = if k == 0 { (px(a) + 0.5) / ppp } else { (px(b) - 0.5) / ppp };
                let probe = if horizontal { Pos2::new(mid_along, first) } else { Pos2::new(first, mid_along) };
                let covered = p.covers.iter().any(|c| *c != l && *c != f && c.contains(probe));
                if gap > 0.01 && gap < 1.0 && device >= 1.0 && !covered {
                    let at = |r: Rect| format!("{:.2},{:.2}–{:.2},{:.2}", r.min.x, r.min.y, r.max.x, r.max.y);
                    let text = format!(
                        "a line ({}) and what it borders ({}) are {device:.0} device px apart at {ppp}× (meant to touch): inset from a rect on device pixels (style::inset)",
                        at(l),
                        at(f)
                    );
                    push(out, finding(Level::Error, "seam", text, l.union(f)));
                }
            }
        }
    }
}

/// Filled bands: wide, and a bar's height.
fn bands(p: &Parts, screen: Rect) -> Vec<Rect> {
    let wide = (screen.width() * 0.2).max(80.0);
    let mut out: Vec<Rect> = p.fills.iter().map(|f| f.rect).filter(|r| r.width() >= wide && (12.0..=48.0).contains(&r.height())).collect();
    out.dedup_by(|a, b| (a.min - b.min).length() < 0.01 && (a.max - b.max).length() < 0.01);
    out
}

fn near_heights(p: &Parts, screen: Rect, out: &mut Vec<Finding>) {
    let bands = bands(p, screen);
    for a in &bands {
        for b in &bands {
            let overlap_x = a.max.x.min(b.max.x) - a.min.x.max(b.min.x);
            let d = (a.height() - b.height()).abs();
            // One over the other (a line between them at most).
            if overlap_x >= 0.5 * a.width().min(b.width()) && (0.0..=1.01).contains(&(b.min.y - a.max.y)) && d > SAME && d <= NEAR_HEIGHT {
                let text = format!("a band {:.0} tall over one {:.0} tall (at y {:.0}): make them one height, or clearly different", a.height(), b.height(), b.min.y);
                push(out, finding(Level::Review, "near-height", text, a.union(*b)));
            }
            // Side by side: their tops or bottoms almost level.
            let overlap_y = a.max.y.min(b.max.y) - a.min.y.max(b.min.y);
            if overlap_y > 0.0 && (0.0..=1.01).contains(&(b.min.x - a.max.x)) {
                for (what, ea, eb) in [("tops", a.min.y, b.min.y), ("bottoms", a.max.y, b.max.y)] {
                    let d = (ea - eb).abs();
                    if d > SAME && d <= NEAR_HEIGHT {
                        let text = format!("bands side by side with {what} {d:.1} px apart (y {ea:.0} and {eb:.0}): line them up");
                        push(out, finding(Level::Review, "near-height", text, a.union(*b)));
                    }
                }
            }
        }
    }
}

/// Text and icons in neighbouring columns: almost level.
fn near_level(p: &Parts, out: &mut Vec<Finding>) {
    // (rect, baseline, middle, name, is text)
    let mut items: Vec<(Rect, f32, f32, String, bool)> = p.texts.iter().map(|t| (t.rect, t.baseline, t.middle, quote(t), true)).collect();
    items.extend(p.icons.iter().map(|i| i.rect).map(|r| (r, r.center().y, r.center().y, "an icon".to_owned(), false)));
    let grid = |r: &Rect| p.marks.grids.iter().position(|g| g.contains(r.center()));
    for (ra, la, ma, na, ta) in &items {
        // Its nearest neighbour to the right, in the same row (and on
        // the same grid).
        let beside = items
            .iter()
            .filter(|(rb, ..)| {
                let gap = rb.min.x - ra.max.x;
                let overlap = ra.max.y.min(rb.max.y) - ra.min.y.max(rb.min.y);
                (0.0..=160.0).contains(&gap) && overlap >= 0.4 * ra.height().min(rb.height()) && grid(ra) == grid(rb)
            })
            .min_by(|a, b| a.0.min.x.total_cmp(&b.0.min.x));
        if let Some((rb, lb, mb, nb, tb)) = beside {
            // Text against text by baseline; an icon by its middle against
            // the text's capitals' (as `style::label` centres them).
            let (a, b) = if *ta && *tb { (*la, *lb) } else { (*ma, *mb) };
            let d = (a - b).abs();
            let most = if *ta && *tb { 0.5 * ra.height().min(rb.height()) } else { NEAR_EDGE };
            if d > SAME && d <= most {
                let text = format!("{na} and {nb} beside it are almost level, {:.0} px apart: line them up", d.max(1.0));
                push(out, finding(Level::Review, "near-level", text, ra.union(*rb)));
            }
        }
    }
}

/// Left edges down a column: almost lined up.
fn near_edges(p: &Parts, out: &mut Vec<Finding>) {
    // (A label of a character or three is centred in a cell: a date, a key.)
    let mut items: Vec<(Rect, f32, String)> =
        p.texts.iter().filter(|t| t.label.chars().count() > 3 && !centred(p, t)).map(|t| (t.rect, t.left, quote(t))).collect();
    items.extend(p.icons.iter().map(|i| i.rect).map(|r| (r, r.min.x, "an icon".to_owned())));
    for (ra, xa, na) in &items {
        for (rb, xb, nb) in &items {
            let below = rb.min.y - ra.max.y;
            let d = xb - xa;
            // Lined up on the right (a column of labels before their
            // fields), or centred over each other (icons of two sizes).
            let other_way = (ra.max.x - rb.max.x).abs() <= SAME || (ra.center().x - rb.center().x).abs() <= SAME;
            if (-2.0..=48.0).contains(&below) && d > SAME && d <= NEAR_EDGE && !other_way {
                let text = format!("{na} (x {xa:.0}) and {nb} under it (x {xb:.0}) almost line up on the left: line them up");
                push(out, finding(Level::Review, "near-edge", text, ra.union(*rb)));
            }
        }
    }
}

/// The band (a fill or gradient no more than a few times as tall) `r`
/// sits in, if any.
fn band_of(p: &Parts, r: Rect) -> Option<Rect> {
    p.covers
        .iter()
        .chain(&p.marks.bands)
        .copied()
        .filter(|f| f.height() > 2.0 && f.height() <= 3.2 * r.height() && f.expand(SAME).contains_rect(r))
        .min_by(|a, b| a.area().total_cmp(&b.area()))
}

/// What of `t` is seen to be centred: its capitals, across its width.
fn caps(t: &Text) -> Rect {
    let half = t.size * crate::style::CAP_HEIGHT / 2.0;
    Rect::from_x_y_ranges(t.rect.x_range(), (t.middle - half)..=(t.middle + half))
}

/// Text centred across the fill it is on (a button's): its left edge is
/// wherever its length puts it.
fn centred(p: &Parts, t: &Text) -> bool {
    band_of(p, caps(t)).is_some_and(|b| (b.center().x - t.rect.center().x).abs() <= 1.0 && b.width() <= 240.0)
}

fn off_centre(p: &Parts, out: &mut Vec<Finding>) {
    let mut items: Vec<(Rect, f32, String)> = p.texts.iter().map(|t| (caps(t), t.middle, quote(t))).collect();
    items.extend(p.icons.iter().map(|i| i.rect).map(|r| (r, r.center().y, "an icon".to_owned())));
    for (r, middle, name) in items {
        let Some(band) = band_of(p, r) else { continue };
        let d = middle - band.center().y;
        if d.abs() > 1.0 {
            let way = if d > 0.0 { "low" } else { "high" };
            let text = format!("{name} sits {:.0} px {way} in its {:.0}-px band: centre it", d.abs(), band.height());
            push(out, finding(Level::Review, "off-centre", text, band));
        }
    }
}

/// Rows down a column (text starting at one x, in one size) spaced
/// unevenly, beyond what rounding to device pixels makes. Only where the
/// column has a rhythm (most of its gaps one step): a column of different
/// things (a box's title, a slider, a field, a paragraph) is spaced by
/// what each is, not by one step.
fn uneven_rows(p: &Parts, ppp: f32, out: &mut Vec<Finding>) {
    let mut texts: Vec<&Text> = p.texts.iter().collect();
    texts.sort_by(|a, b| a.left.total_cmp(&b.left).then(a.baseline.total_cmp(&b.baseline)));
    let mut columns: Vec<Vec<&Text>> = Vec::new();
    for t in texts {
        match columns.iter_mut().find(|c| (c[0].left - t.left).abs() <= SAME && c[0].size == t.size) {
            Some(c) => c.push(t),
            None => columns.push(vec![t]),
        }
    }
    let rounding = 1.0 / ppp + 0.01;
    for mut c in columns {
        c.sort_by(|a, b| a.baseline.total_cmp(&b.baseline));
        let gaps: Vec<f32> = c.windows(2).map(|w| w[1].baseline - w[0].baseline).collect();
        if gaps.len() < 3 {
            continue;
        }
        let mut sorted = gaps.clone();
        sorted.sort_by(f32::total_cmp);
        let usual = sorted[sorted.len() / 2];
        let even = gaps.iter().filter(|g| (*g - usual).abs() <= rounding).count();
        if even < 2 || even * 2 < gaps.len() {
            continue;
        }
        for (i, g) in gaps.iter().enumerate() {
            let d = (g - usual).abs();
            if d > rounding && d <= NEAR_HEIGHT {
                let text = format!("rows at x {:.0} are {usual:.0} px apart, but {} to {} is {g:.0}: space them evenly", c[0].left, quote(c[i]), quote(c[i + 1]));
                push(out, finding(Level::Review, "uneven-rows", text, c[i].rect.union(c[i + 1].rect)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style;
    use egui::{Align, Color32, Context, Painter, ViewportId, pos2, vec2};

    /// What the review finds in a frame painted by `paint` at `scale`.
    fn frame(scale: f32, paint: impl Fn(&Painter)) -> Vec<Finding> {
        let ctx = Context::default();
        ctx.set_fonts(style::all_font_definitions());
        let size = vec2(400.0, 300.0);
        let mut raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), ..Default::default() };
        raw.viewports.entry(ViewportId::ROOT).or_default().native_pixels_per_point = Some(scale);
        let mut shapes = Vec::new();
        for _ in 0..2 {
            let out = ctx.run_ui(raw.clone(), |ui| paint(ui.painter()));
            shapes = out.shapes.clone();
            out.drop_without_applying_deltas();
        }
        review(&shapes, size, scale, &Marks { textures: [(TextureId::User(7), [23, 23])].into(), ..Default::default() })
    }

    fn kinds(found: &[Finding]) -> Vec<&str> {
        found.iter().map(|f| f.kind.as_str()).collect()
    }

    fn text(p: &Painter, x: f32, cy: f32, s: &str) {
        style::label(p, x, cy, Align::Min, s, style::plain_font(), Color32::BLACK);
    }

    #[test]
    fn bands_of_nearly_one_height() {
        let bars = |second: f32| {
            move |p: &Painter| {
                p.rect_filled(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 21.0)), 0.0, Color32::GRAY);
                p.rect_filled(Rect::from_min_size(pos2(0.0, 21.0), vec2(400.0, second)), 0.0, Color32::DARK_GRAY);
            }
        };
        assert!(kinds(&frame(2.0, bars(22.0))).contains(&"near-height"));
        assert!(!kinds(&frame(2.0, bars(21.0))).contains(&"near-height"));
        assert!(!kinds(&frame(2.0, bars(30.0))).contains(&"near-height"));
    }

    #[test]
    fn text_almost_level_and_almost_lined_up() {
        let found = frame(2.0, |p| {
            text(p, 10.0, 20.0, "Left");
            text(p, 120.0, 23.0, "Right");
        });
        assert!(kinds(&found).contains(&"near-level"), "{found:?}");
        let found = frame(2.0, |p| {
            text(p, 10.0, 20.0, "Left");
            text(p, 120.0, 20.0, "Right");
        });
        assert!(!kinds(&found).contains(&"near-level"), "{found:?}");
        let found = frame(2.0, |p| {
            text(p, 11.0, 20.0, "Terminal");
            text(p, 14.0, 40.0, "Tabs");
        });
        assert!(kinds(&found).contains(&"near-edge"), "{found:?}");
        let found = frame(2.0, |p| {
            text(p, 11.0, 20.0, "Terminal");
            text(p, 23.0, 40.0, "Indented");
        });
        assert!(!kinds(&found).contains(&"near-edge"), "{found:?}");
    }

    #[test]
    fn text_off_centre_in_its_band() {
        let band = |cy: f32| {
            move |p: &Painter| {
                p.rect_filled(Rect::from_min_size(pos2(0.0, 10.0), vec2(400.0, 22.0)), 0.0, Color32::GRAY);
                text(p, 8.0, cy, "Header");
            }
        };
        assert!(!kinds(&frame(2.0, band(21.0))).contains(&"off-centre"));
        assert!(kinds(&frame(2.0, band(24.0))).contains(&"off-centre"));
    }

    #[test]
    fn rows_spaced_unevenly() {
        let rows = |ys: &'static [f32]| {
            move |p: &Painter| {
                for (i, y) in ys.iter().enumerate() {
                    text(p, 10.0, *y, &format!("Row {i}"));
                }
            }
        };
        assert!(!kinds(&frame(2.0, rows(&[10.0, 30.0, 50.0, 70.0]))).contains(&"uneven-rows"));
        assert!(kinds(&frame(2.0, rows(&[10.0, 30.0, 52.0, 72.0]))).contains(&"uneven-rows"));
        // No two gaps alike: no rhythm to break.
        assert!(!kinds(&frame(2.0, rows(&[10.0, 29.0, 88.0, 151.0, 217.0]))).contains(&"uneven-rows"));
    }

    #[test]
    fn seams_between_nested_lines() {
        // A frame, then a bevel a point inside it: at 1.5× a point is a
        // pixel and a half, so a one-pixel frame leaves a crack.
        let nested = |p: &Painter| {
            let r = Rect::from_min_size(pos2(10.0, 10.0), vec2(60.0, 20.0));
            style::frame_rect(p, r, Color32::BLACK);
            style::bevel(p, r.shrink(1.0), Color32::WHITE, Color32::GRAY);
        };
        assert!(!kinds(&frame(2.0, nested)).contains(&"seam"));
        assert!(kinds(&frame(1.5, nested)).contains(&"seam"), "{:?}", frame(1.5, nested));
        // The sunken well's two bevels are placed on device pixels.
        let well = |p: &Painter| style::sunken(p, Rect::from_min_size(pos2(10.0, 10.0), vec2(60.0, 20.0)), &olo_theme::Palette::light(), false);
        assert!(!kinds(&frame(1.5, well)).contains(&"seam"), "{:?}", frame(1.5, well));
    }

    /// The kit's sizes and gaps are on the 2-point grid: whole device
    /// pixels at 1×, 1.5× and 2×, so what is laid out with them is too.
    /// (Lines are whole device pixels by `style::line_pixels`; radii and
    /// type sizes aren't places.)
    #[test]
    fn kit_sizes_on_the_two_point_grid() {
        use crate::{menu, metrics as m, widgets as w};
        let sizes = [
            ("metrics::SPACING", m::SPACING),
            ("metrics::DESKBAR_WIDTH", m::DESKBAR_WIDTH),
            ("metrics::TIGHT", m::TIGHT),
            ("metrics::CARD_INSET", m::CARD_INSET),
            ("metrics::SELECTION_PAD", m::SELECTION_PAD),
            ("metrics::WINDOW_INSET", m::WINDOW_INSET),
            ("metrics::BOX_INSET", m::BOX_INSET),
            ("metrics::LABEL_GAP", m::LABEL_GAP),
            ("metrics::MARK_GAP", m::MARK_GAP),
            ("metrics::MARK_SIZE", m::MARK_SIZE),
            ("metrics::CONTROL_HEIGHT", m::CONTROL_HEIGHT),
            ("metrics::MARK_ROW", m::MARK_ROW),
            ("metrics::LABEL_ROW", m::LABEL_ROW),
            ("metrics::CONTROL_PAD", m::CONTROL_PAD),
            ("metrics::BUTTON_MIN_WIDTH", m::BUTTON_MIN_WIDTH),
            ("metrics::TEXT_INSET", m::TEXT_INSET),
            ("metrics::LIST_ROW", m::LIST_ROW),
            ("metrics::CELL_INSET", m::CELL_INSET),
            ("metrics::TOOLBAR_HEIGHT", m::TOOLBAR_HEIGHT),
            ("menu::BAR_HEIGHT", menu::BAR_HEIGHT),
            ("menu::ITEM_HEIGHT", menu::ITEM_HEIGHT),
            ("widgets::SCROLL_BAR", w::SCROLL_BAR),
            ("widgets::SMALL_BUTTON_HEIGHT", w::SMALL_BUTTON_HEIGHT),
            ("widgets::HEADER_HEIGHT", w::HEADER_HEIGHT),
            ("widgets::TAB_HEIGHT", w::TAB_HEIGHT),
            ("widgets::STRIP_TAB_MAX", w::STRIP_TAB_MAX),
            ("widgets::STRIP_HEIGHT", w::STRIP_HEIGHT),
            ("widgets::STRIP_ROW", w::STRIP_ROW),
            ("widgets::STRIP_SIDE_WIDTH", w::STRIP_SIDE_WIDTH),
            ("widgets::STRIP_SIDE_MIN", w::STRIP_SIDE_MIN),
            ("widgets::STRIP_SIDE_MAX", w::STRIP_SIDE_MAX),
            ("widgets::ALERT_TEXT_X", w::ALERT_TEXT_X),
        ];
        let off: Vec<String> = sizes.iter().filter(|(_, v)| v % 2.0 != 0.0).map(|(n, v)| format!("{n} = {v}")).collect();
        assert!(off.is_empty(), "off the 2-point grid: {}", off.join(", "));
    }

    #[test]
    fn fills_between_device_pixels() {
        // A 21-point bar and what follows it: at 1.5× its bottom is 31.5
        // device pixels.
        let bars = |h: f32| {
            move |p: &Painter| {
                p.rect_filled(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, h)), 0.0, Color32::GRAY);
                p.rect_filled(Rect::from_min_size(pos2(0.0, h), vec2(400.0, 26.0)), 0.0, Color32::DARK_GRAY);
            }
        };
        assert!(kinds(&frame(1.5, bars(21.0))).contains(&"off-pixel"));
        assert!(!kinds(&frame(1.0, bars(21.0))).contains(&"off-pixel"));
        assert!(!kinds(&frame(2.0, bars(21.0))).contains(&"off-pixel"));
        assert!(!kinds(&frame(1.5, bars(26.0))).contains(&"off-pixel"));
        // At 1.25× the grid itself is between pixels: not flagged.
        assert!(!kinds(&frame(1.25, bars(26.0))).contains(&"off-pixel"));
        // Where a clip cuts it, the edge is the clip's.
        let clipped = |p: &Painter| {
            p.with_clip_rect(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 30.0))).rect_filled(Rect::from_min_size(pos2(0.0, 10.0), vec2(400.0, 20.5)), 0.0, Color32::GRAY);
        };
        assert!(!kinds(&frame(1.5, clipped)).contains(&"off-pixel"), "{:?}", frame(1.5, clipped));
    }

    /// Every kit widget, alone in a window, light and dark at 1×, 1.5×
    /// and 2×: the review finds nothing at all in any of them, not even
    /// something to look at.
    #[test]
    fn kit_widgets_pass_the_review() {
        use crate::{
            layout_lint,
            menu::{self, Menu},
            widgets::*,
        };
        let size = vec2(320.0, 220.0);
        let icon = |p: &Painter, r: Rect| {
            p.rect_filled(r, 2.0, Color32::from_rgb(90, 120, 200));
        };
        let tabs = [StripTab { label: "One", icon: Some(&icon), status: None }, StripTab { label: "Two", icon: Some(&icon), status: None }];
        let menus = [Menu::new("File").item("New", "new", None), Menu::new("Edit").item("Copy", "copy", None)];
        // Open, the row after a separator highlighted.
        let file = Menu::new("File")
            .item("New", "new", Some(egui::Key::N))
            .item("Open…", "open", Some(egui::Key::O))
            .separator()
            .item("Word Wrap", "wrap", None)
            .checked(true)
            .submenu(Menu::new("Recent").item("notes.txt", "recent", None))
            .separator()
            .item("Quit", "quit", Some(egui::Key::Q));
        let places = Menu::new("Go").item("Home", "home", None).icon("folder").separator().item("Trash", "trash", None).icon("trash");
        let popup = |ui: &mut egui::Ui, m: &Menu| {
            let r = Rect::from_min_size(ui.max_rect().min, menu::popup_size(ui.ctx(), m));
            let mut state = menu::PopupState { selected: Some(3.min(m.items.len() - 1)), ..Default::default() };
            menu::popup_ui(&mut ui.new_child(egui::UiBuilder::new().max_rect(r)), m, &mut state);
        };
        type Demo<'a> = (&'a str, Box<dyn Fn(&mut egui::Ui) + 'a>);
        let widgets: Vec<Demo> = vec![
            ("Button", Box::new(|ui| {
                ui.add(Button::new("Default").default(true));
                ui.add(Button::new("Cancel"));
            })),
            ("CheckBox", Box::new(|ui| {
                ui.add(CheckBox::new(&mut true, "On"));
                ui.add(CheckBox::new(&mut false, "Off"));
            })),
            ("RadioButton", Box::new(|ui| {
                ui.add(RadioButton::new(true, "On"));
                ui.add(RadioButton::new(false, "Off"));
            })),
            ("TextControl", Box::new(|ui| {
                ui.add(TextControl::new(&mut "Text".to_owned()).label("Name:"));
                ui.add(TextControl::new(&mut String::new()).label("Find:").hint("City or region"));
            })),
            ("TextView", Box::new(|ui| {
                TextView::new("t", &mut "Some text\nover lines".to_owned()).show(ui);
            })),
            ("Slider", Box::new(|ui| {
                ui.add(Slider::new(&mut 0.5, 0.0..=1.0).label("Speed").limits("Slow", "Fast"));
            })),
            ("MenuField", Box::new(|ui| {
                ui.add(MenuField::new("m", &mut 0, &["One", "Two"]).label("Pick:"));
            })),
            ("BBox", Box::new(|ui| {
                BBox::new("Box").show(ui, |ui| {
                    ui.add(StringView::new("Inside"));
                });
            })),
            ("StatusBar", Box::new(|ui| {
                ui.add(StatusBar::new(0.4));
            })),
            ("TabView", Box::new(|ui| {
                TabView::new(&mut 0, &["One", "Two"]).show(ui, |ui, _| {
                    ui.add(StringView::new("Inside"));
                });
            })),
            ("ScrollView", Box::new(|ui| {
                ScrollView::new("s").show(ui, |ui| {
                    for i in 0..30 {
                        ui.add(StringView::new(&format!("Line {i}")));
                    }
                });
            })),
            ("separator", Box::new(|ui| {
                separator(ui);
            })),
            ("ColorControl", Box::new(|ui| {
                ui.add(ColorControl::new("c", &mut [200, 120, 40]));
            })),
            ("menu bar", Box::new(|ui| {
                menu::menu_bar(ui, &menus, &mut Default::default());
            })),
            ("popup menu", Box::new(|ui| popup(ui, &file))),
            ("popup menu with icons", Box::new(|ui| popup(ui, &places))),
            ("TabStrip", Box::new(|ui| {
                let r = Rect::from_min_size(ui.max_rect().min, vec2(320.0, STRIP_HEIGHT));
                TabStrip::new("ts", &tabs, 0).show(ui, r);
            })),
            ("TabStrip, side", Box::new(|ui| {
                let r = Rect::from_min_size(ui.max_rect().min, vec2(STRIP_SIDE_WIDTH, 220.0));
                TabStrip::new("ts", &tabs, 0).side().show(ui, r);
            })),
            ("TabStrip, side, closed", Box::new(|ui| {
                let r = Rect::from_min_size(ui.max_rect().min, vec2(STRIP_SIDE_WIDTH, 220.0));
                TabStrip::new("ts", &tabs, 1).side().closed().show(ui, r);
            })),
        ];
        let mut found = Vec::new();
        for (name, w) in &widgets {
            for line in layout_lint::review(size, |ui| w(ui)) {
                found.push(format!("{name}: {line}"));
            }
        }
        assert!(found.is_empty(), "kit widgets the review finds something in:\n  - {}", found.join("\n  - "));
    }

    #[test]
    fn lines_and_icons_on_device_pixels() {
        let lines = |p: &Painter| {
            style::hline(p, 0.0, 200.0, 10.0, Color32::BLACK);
            style::hline(p, 0.0, 200.0, 11.0 + 2.0, Color32::BLACK);
        };
        assert!(!kinds(&frame(2.0, lines)).contains(&"line-weights"));
        assert!(!kinds(&frame(1.5, lines)).contains(&"line-weights"), "{:?}", frame(1.5, lines));
        let icon = |x: f32| {
            move |p: &Painter| {
                p.image(TextureId::User(1), Rect::from_min_size(pos2(x, 10.0), vec2(16.0, 16.0)), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
        };
        assert!(!kinds(&frame(2.0, icon(10.5))).contains(&"blurry-icon"));
        assert!(kinds(&frame(1.5, icon(10.5))).contains(&"blurry-icon"));
        assert!(!kinds(&frame(1.5, icon(10.0 + 2.0 / 3.0))).contains(&"blurry-icon"));
        // On whole pixels, but 24 device px from a 23 px texture (a 15.5
        // point icon whose corners were snapped one by one): resampled.
        let from = |w: f32| {
            move |p: &Painter| {
                p.image(TextureId::User(7), Rect::from_min_size(pos2(10.0, 10.0), vec2(w, w)), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
        };
        assert!(kinds(&frame(1.5, from(16.0))).contains(&"blurry-icon"));
        assert!(!kinds(&frame(1.5, from(23.0 / 1.5))).contains(&"blurry-icon"), "{:?}", frame(1.5, from(23.0 / 1.5)));
        assert!(!kinds(&frame(1.0, from(64.0))).contains(&"blurry-icon"));
        let outline = |p: &Painter| {
            p.rect_stroke(Rect::from_min_size(pos2(10.0, 10.0), vec2(40.0, 20.0)), 0.0, egui::Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Inside);
        };
        assert!(!kinds(&frame(2.0, outline)).contains(&"blurry-outline"));
        assert!(kinds(&frame(1.5, outline)).contains(&"blurry-outline"));
    }
}
