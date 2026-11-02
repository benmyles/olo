//! The layout review's checks: what a frame looks like it got wrong,
//! from what it painted, which the kit collects into [`Parts`] from what
//! its style drew (`olo_ui::review`) at the scale it was drawn at. See
//! "Layout review" in docs/HACKING.md for what each check finds and why.

pub use crate::review::{Finding, Level};

/// Cap height as a fraction of the font size (Noto Sans's: 714 of its
/// 1000 units): what of a line of text is seen to be centred.
pub const CAP_HEIGHT: f32 = 0.714;

/// How many of each kind a frame reports (the rest are like them).
const MOST: usize = 8;
/// Closer than this, two things count as lined up (a rounding).
const SAME: f32 = 0.5;
/// Further than this, a difference looks meant rather than missed.
const NEAR_EDGE: f32 = 4.0;
const NEAR_HEIGHT: f32 = 3.0;

/// A point, in the window's logical pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Pos2 {
    pub x: f32,
    pub y: f32,
}

impl Pos2 {
    pub const ZERO: Pos2 = Pos2 { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Pos2 {
        Pos2 { x, y }
    }
}

pub const fn pos2(x: f32, y: f32) -> Pos2 {
    Pos2 { x, y }
}

/// A difference between points.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub fn length(self) -> f32 {
        self.x.hypot(self.y)
    }
}

impl std::ops::Sub for Pos2 {
    type Output = Vec2;
    fn sub(self, o: Pos2) -> Vec2 {
        Vec2 { x: self.x - o.x, y: self.y - o.y }
    }
}

/// A span along one axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rangef {
    pub min: f32,
    pub max: f32,
}

impl Rangef {
    pub fn intersects(self, o: Rangef) -> bool {
        o.min <= self.max && self.min <= o.max
    }
}

impl From<std::ops::RangeInclusive<f32>> for Rangef {
    fn from(r: std::ops::RangeInclusive<f32>) -> Rangef {
        Rangef { min: *r.start(), max: *r.end() }
    }
}

/// A rectangle, `min` its top left and `max` its bottom right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub min: Pos2,
    pub max: Pos2,
}

impl Rect {
    /// Nothing: what extending from begins with.
    pub const NOTHING: Rect = Rect { min: pos2(f32::INFINITY, f32::INFINITY), max: pos2(-f32::INFINITY, -f32::INFINITY) };

    pub const fn from_min_max(min: Pos2, max: Pos2) -> Rect {
        Rect { min, max }
    }

    pub fn from_min_size(min: Pos2, width: f32, height: f32) -> Rect {
        Rect { min, max: pos2(min.x + width, min.y + height) }
    }

    pub fn from_x_y_ranges(x: impl Into<Rangef>, y: impl Into<Rangef>) -> Rect {
        let (x, y) = (x.into(), y.into());
        Rect { min: pos2(x.min, y.min), max: pos2(x.max, y.max) }
    }

    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    pub fn left(&self) -> f32 {
        self.min.x
    }

    pub fn right(&self) -> f32 {
        self.max.x
    }

    pub fn top(&self) -> f32 {
        self.min.y
    }

    pub fn bottom(&self) -> f32 {
        self.max.y
    }

    pub fn area(&self) -> f32 {
        self.width() * self.height()
    }

    pub fn center(&self) -> Pos2 {
        pos2((self.min.x + self.max.x) / 2.0, (self.min.y + self.max.y) / 2.0)
    }

    pub fn x_range(&self) -> Rangef {
        Rangef { min: self.min.x, max: self.max.x }
    }

    pub fn y_range(&self) -> Rangef {
        Rangef { min: self.min.y, max: self.max.y }
    }

    /// `p` in it, its edges included.
    pub fn contains(&self, p: Pos2) -> bool {
        self.min.x <= p.x && p.x <= self.max.x && self.min.y <= p.y && p.y <= self.max.y
    }

    pub fn contains_rect(&self, o: Rect) -> bool {
        self.contains(o.min) && self.contains(o.max)
    }

    pub fn intersects(&self, o: Rect) -> bool {
        self.min.x <= o.max.x && o.min.x <= self.max.x && self.min.y <= o.max.y && o.min.y <= self.max.y
    }

    pub fn intersect(&self, o: Rect) -> Rect {
        Rect { min: pos2(self.min.x.max(o.min.x), self.min.y.max(o.min.y)), max: pos2(self.max.x.min(o.max.x), self.max.y.min(o.max.y)) }
    }

    pub fn union(&self, o: Rect) -> Rect {
        Rect { min: pos2(self.min.x.min(o.min.x), self.min.y.min(o.min.y)), max: pos2(self.max.x.max(o.max.x), self.max.y.max(o.max.y)) }
    }

    /// Wider and taller by `by` on every side.
    pub fn expand(&self, by: f32) -> Rect {
        Rect { min: pos2(self.min.x - by, self.min.y - by), max: pos2(self.max.x + by, self.max.y + by) }
    }

    pub fn shrink(&self, by: f32) -> Rect {
        self.expand(-by)
    }

    /// Its width and height both more than none.
    pub fn is_positive(&self) -> bool {
        self.min.x < self.max.x && self.min.y < self.max.y
    }

    pub fn extend_with(&mut self, p: Pos2) {
        self.min = pos2(self.min.x.min(p.x), self.min.y.min(p.y));
        self.max = pos2(self.max.x.max(p.x), self.max.y.max(p.y));
    }
}

#[derive(Debug, Clone)]
pub struct Text {
    pub rect: Rect,
    /// The first line's baseline, and the middle of its capitals (what
    /// `style::label` centres).
    pub baseline: f32,
    pub middle: f32,
    /// Where the layout started it (not where the first glyph's ink is).
    pub left: f32,
    pub label: String,
    pub size: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Fill {
    pub rect: Rect,
    /// Its top, bottom, left and right edges where a clip cut it (the
    /// clip's, not its own).
    pub cut: [bool; 4],
}

/// What a frame told the review about its layout, beyond what it

/// A picture drawn from a bitmap: where, its bitmap's size in pixels
/// (if known), and whether all of the bitmap is drawn.
#[derive(Debug, Clone)]
pub struct Icon {
    pub rect: Rect,
    pub texture: Option<[usize; 2]>,
    pub whole: bool,
}

/// What a frame painted, as the checks read it, and what it marked.
#[derive(Debug, Clone, Default)]
pub struct Parts {
    pub texts: Vec<Text>,
    pub fills: Vec<Fill>,
    /// Everything opaque painted over an area (fills and gradients): what
    /// a crack between two of them might be covered by.
    pub covers: Vec<Rect>,
    /// Those of them no clip cut: what's centred in a band is centred
    /// in the whole of it, not in what of it a scroll view shows.
    pub whole_covers: Vec<Rect>,
    pub icons: Vec<Icon>,
    /// Outlines' widths and where (rect strokes, straight segments).
    pub strokes: Vec<(f32, Rect)>,
    /// Areas on a grid of their own (a terminal's screen): what's in
    /// them isn't held level with what's beside them.
    pub grids: Vec<Rect>,
    /// Areas what's in them is centred across, though nothing fills
    /// them (a tab hanging in a strip).
    pub bands: Vec<Rect>,
}

/// What looks wrong in a frame (its `parts`), drawn at `ppp` device
/// pixels a point in a window `width` × `height` (logical), errors
/// first.
pub fn review(parts: &Parts, width: f32, height: f32, ppp: f32) -> Vec<Finding> {
    let screen = Rect::from_min_size(Pos2::ZERO, width, height);
    let mut out = Vec::new();
    blurry_icons(parts, ppp, &mut out);
    line_weights(parts, ppp, &mut out);
    blurry_outlines(parts, ppp, &mut out);
    seams(parts, ppp, &mut out);
    off_pixels(parts, screen, ppp, &mut out);
    near_heights(parts, screen, &mut out);
    near_level(parts, ppp, &mut out);
    near_edges(parts, ppp, &mut out);
    off_centre(parts, &mut out);
    uneven_rows(parts, ppp, &mut out);
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
        let Some([tw, th]) = icon.texture else { continue };
        let off = |a: f32, b: usize| (a.round() - b as f32).abs();
        let (dw, dh) = (off(w, tw), off(h, th));
        if icon.whole && dw.max(dh) > 0.0 && dw.max(dh) <= 2.0 {
            let text = format!("an icon drawn {w:.0}×{h:.0} device px from a {tw}×{th} texture at {ppp}×: resampled a pixel off its size, it blurs (draw it at its texture's size, as icons::paint does)");
            push(out, finding(Level::Error, "blurry-icon", text, r));
        }
    }
}

/// The device pixels a fill of `a..b` covers, each edge rounded to the
/// nearest.
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

/// Fills laid out between device pixels: snapped (`style::snap`), one
/// comes out a pixel taller, shorter or further over than laid out, and
/// not snapped its edges blur: rows of one height end up of two, a bar a pixel off
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
fn near_level(p: &Parts, ppp: f32, out: &mut Vec<Finding>) {
    // (rect, baseline, middle, name, is text, is a lone symbol)
    let mut items: Vec<(Rect, f32, f32, String, bool, bool)> =
        p.texts.iter().map(|t| (t.rect, t.baseline, t.middle, quote(t), true, lone_symbol(t))).collect();
    items.extend(p.icons.iter().map(|i| i.rect).map(|r| (r, r.center().y, r.center().y, "an icon".to_owned(), false, false)));
    let grid = |r: &Rect| grid_of(p, r);
    for (ra, la, ma, na, ta, sa) in &items {
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
        if let Some((rb, lb, mb, nb, tb, sb)) = beside {
            // Text against text by baseline; an icon by its middle against
            // the text's capitals' (as `style::label` centres them).
            let (a, b) = if *ta && *tb { (*la, *lb) } else { (*ma, *mb) };
            let d = (a - b).abs();
            let most = if *ta && *tb { 0.5 * ra.height().min(rb.height()) } else { NEAR_EDGE };
            // A symbol set by its ink (`style::key_combo`) may be off its
            // face's baseline by up to two device pixels (↵ stands that
            // far above its own, and is set down onto the capitals'), and
            // a small one beside it (⌃) a fraction of one off the letters'.
            let same = if *sa || *sb { SAME.max(2.0 / ppp + 0.01) } else { SAME };
            if d > same && d <= most {
                let text = format!("{na} and {nb} beside it are almost level, {:.0} px apart: line them up", d.max(1.0));
                push(out, finding(Level::Review, "near-level", text, ra.union(*rb)));
            }
        }
    }
}

/// A single symbol (⇧ ⌘ ←), not a letter or a word.
fn lone_symbol(t: &Text) -> bool {
    let mut chars = t.label.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if !c.is_ascii() && !c.is_alphanumeric())
}

/// Left edges down a column: almost lined up.
fn near_edges(p: &Parts, ppp: f32, out: &mut Vec<Finding>) {
    // (A label of a character or three is centred in a cell: a date, a key.)
    let mut items: Vec<(Rect, f32, String)> =
        p.texts.iter().filter(|t| t.label.chars().count() > 3 && !centred(p, t)).map(|t| (t.rect, t.left, quote(t))).collect();
    items.extend(p.icons.iter().map(|i| i.rect).map(|r| (r, r.min.x, "an icon".to_owned())));
    for (ra, xa, na) in &items {
        for (rb, xb, nb) in &items {
            let below = rb.min.y - ra.max.y;
            let d = xb - xa;
            // Lined up on the right (a column of labels before their
            // fields, of sizes), or centred over each other (icons of two
            // sizes). Text set from its right starts on a device pixel,
            // so its right edge is up to one off another's.
            let right = SAME.max(1.0 / ppp + 0.01);
            let other_way = (ra.max.x - rb.max.x).abs() <= right || (ra.center().x - rb.center().x).abs() <= SAME;
            // (On another grid, it lines up with the window's only by chance.)
            if (-2.0..=48.0).contains(&below) && d > SAME && d <= NEAR_EDGE && !other_way && grid_of(p, ra) == grid_of(p, rb) {
                let text = format!("{na} (x {xa:.0}) and {nb} under it (x {xb:.0}) almost line up on the left: line them up");
                push(out, finding(Level::Review, "near-edge", text, ra.union(*rb)));
            }
        }
    }
}

/// The grid (`Parts::grids`) `r` is laid out on, if not the window's: the
/// innermost one marked around it (a card in a scrolled list).
fn grid_of(p: &Parts, r: &Rect) -> Option<usize> {
    let grids = p.grids.iter().enumerate();
    grids.filter(|(_, g)| g.contains(r.center())).min_by(|(_, a), (_, b)| a.area().total_cmp(&b.area())).map(|(i, _)| i)
}

/// The band (a fill or gradient no more than a few times as tall) `r`
/// sits in, if any.
fn band_of(p: &Parts, r: Rect) -> Option<Rect> {
    p.whole_covers
        .iter()
        .chain(&p.bands)
        .copied()
        .filter(|f| f.height() > 2.0 && f.height() <= 3.2 * r.height() && f.expand(SAME).contains_rect(r))
        .min_by(|a, b| a.area().total_cmp(&b.area()))
}

/// What of `t` is seen to be centred: its capitals, across its width.
fn caps(t: &Text) -> Rect {
    let half = t.size * CAP_HEIGHT / 2.0;
    Rect::from_x_y_ranges(t.rect.x_range(), (t.middle - half)..=(t.middle + half))
}

/// Text centred across the fill it is on (a button's): its left edge is
/// wherever its length puts it.
fn centred(p: &Parts, t: &Text) -> bool {
    band_of(p, caps(t)).is_some_and(|b| (b.center().x - t.rect.center().x).abs() <= 1.0 && b.width() <= 240.0)
}

fn off_centre(p: &Parts, out: &mut Vec<Finding>) {
    let mut items: Vec<(Rect, f32, String)> = p.texts.iter().map(|t| (caps(t), t.middle, quote(t))).collect();
    // An icon level with text beside it on its line (an emoji in a
    // message, a mark after a name) goes with that line: the line is
    // judged, and the icon against it (near-level), not the band.
    let in_line = |r: Rect| {
        p.texts.iter().any(|t| {
            let gap = (t.rect.left() - r.right()).max(r.left() - t.rect.right());
            (t.middle - r.center().y).abs() <= 1.0 && gap <= t.size * 1.5 && t.rect.y_range().intersects(r.y_range())
        })
    };
    items.extend(p.icons.iter().map(|i| i.rect).filter(|r| !in_line(*r)).map(|r| (r, r.center().y, "an icon".to_owned())));
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
    // A column on one grid: a list's heading is a row apart from its
    // first row only when the list is scrolled to a whole row.
    let mut columns: Vec<Vec<&Text>> = Vec::new();
    for t in texts {
        match columns.iter_mut().find(|c| (c[0].left - t.left).abs() <= SAME && c[0].size == t.size && grid_of(p, &c[0].rect) == grid_of(p, &t.rect)) {
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
        // The step: the gap most others are within rounding of (a median
        // would lean towards the long gaps between rows further apart).
        let near = |s: f32| gaps.iter().filter(|g| (*g - s).abs() <= rounding).count();
        let usual = gaps.iter().copied().max_by(|a, b| near(*a).cmp(&near(*b)).then(b.total_cmp(a))).unwrap_or(0.0);
        let even = near(usual);
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

