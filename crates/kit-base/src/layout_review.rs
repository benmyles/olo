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
    /// The controls (`Control`), from the agent tree.
    pub controls: Vec<Control>,
    /// Text a clip cut short.
    pub cut_texts: Vec<CutText>,
    /// Scroll views' viewports: what they cut is scrolled out of sight,
    /// not cut off.
    pub scrolls: Vec<Rect>,
    /// Boxes and panes what's in them must fit in (a BBox's frame, a tab
    /// view's pane): what starts in one and runs out of it overflows.
    pub containers: Vec<Rect>,
    /// Tab views' panes: a window's pages are in them.
    pub panes: Vec<Rect>,
    /// What a control is seen by where its agent rect is only part of it:
    /// a text field's frame (its well), round the text area its rect is.
    pub frames: Vec<Rect>,
}

/// A control agents read and use (the window's agent tree as painted):
/// where it was laid out, the clip it was painted in, and what it is.
#[derive(Debug, Clone)]
pub struct Control {
    pub rect: Rect,
    pub clip: Rect,
    pub role: String,
    pub name: String,
    pub clickable: bool,
    /// Its element id, whole (GPUI's: the ids of what it's in, then its
    /// own), as recorded while reviewing.
    pub element: Option<String>,
}

impl Control {
    /// What of it shows.
    pub fn shown(&self) -> Rect {
        self.rect.intersect(self.clip)
    }

    fn said(&self) -> String {
        format!("{} {:?}", self.role, self.name)
    }
}

/// Text a clip cut: where it was laid out, the clip, and its start.
#[derive(Debug, Clone)]
pub struct CutText {
    pub rect: Rect,
    pub clip: Rect,
    pub label: String,
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
    controls(parts, screen, &mut out);
    texts_over(parts, screen, &mut out);
    overflow(parts, ppp, &mut out);
    crowded(parts, screen, ppp, &mut out);
    window_edges(parts, screen, &mut out);
    crowded_lines(parts, ppp, &mut out);
    control_edges(parts, ppp, &mut out);
    uneven_gaps(parts, ppp, &mut out);
    after_commit(parts, &mut out);
    same_ids(parts, &mut out);
    out.sort_by_key(|f| f.level);
    out
}

/// Two controls with one element id share their state (a press, the
/// keyboard's focus), so a click on either is lost: a kit Button's id is
/// its label unless given one, and two Remove buttons in one window, in
/// nothing with an id of its own, collided (Preferences ▸ Security's,
/// 2026-11-08).
fn same_ids(p: &Parts, out: &mut Vec<Finding>) {
    let mut seen: Vec<(&str, &Control)> = Vec::new();
    for c in showing(p) {
        let Some(id) = c.element.as_deref() else { continue };
        match seen.iter().find(|(s, _)| *s == id) {
            Some((_, first)) => push(
                out,
                finding(Level::Error, "same-id", format!("{} and {} have one element id: clicks on them are lost (give each its own: `.id(…)`)", first.said(), c.said()), first.rect.union(c.rect)),
            ),
            None => seen.push((id, c)),
        }
    }
}

/// Nearer than this to the window's edge (and not on it, nor in a bar
/// along it), content looks crammed against it: the window's inset, less
/// a rounding's worth (a centred line of text sits a little in).
const EDGE_ROOM: f32 = crate::metrics::WINDOW_INSET - 4.0;
/// Closer than this (and not touching), two things look crammed
/// together: the closest the rhythm puts them (`metrics::TIGHT`).
const CROWDED: f32 = crate::metrics::TIGHT - SAME;
/// Two lines closer than this (and not touching): a box's edge almost on
/// its container's.
const LINE_ROOM: f32 = 4.0;

/// A scroll view's frame and bar round its viewport: a clip this much
/// bigger than a viewport is still the scroll view's.
const SCROLL_FRAME: f32 = 16.0;

/// Whether what `clip` cuts is scrolled out of sight (it's a scroll
/// view's viewport, or in one, or the scroll view with its frame), not
/// cut off. (Not any clip with a scroll view somewhere in it: a pane's
/// let a scroll view's content run past it, over the buttons below,
/// unseen: Touchpad ▸ Mouse.)
fn scrolled(p: &Parts, clip: Rect) -> bool {
    p.scrolls.iter().any(|s| s.expand(1.0).contains_rect(clip) || (clip.expand(1.0).contains_rect(*s) && s.expand(SCROLL_FRAME).contains_rect(clip)))
}

/// The controls that show (scrolled out of sight they're nowhere).
fn showing(p: &Parts) -> impl Iterator<Item = &Control> {
    p.controls.iter().filter(|c| c.clickable && c.role != "Window" && c.shown().is_positive())
}

/// Controls laid out to nothing, cut off by what they're in or by the
/// window's edge, or over each other.
fn controls(p: &Parts, screen: Rect, out: &mut Vec<Finding>) {
    for c in p.controls.iter().filter(|c| c.clickable && c.role != "Window") {
        if c.rect.width() < 1.0 || c.rect.height() < 1.0 {
            push(out, finding(Level::Error, "collapsed", format!("{} is laid out to {:.1}×{:.1}", c.said(), c.rect.width(), c.rect.height()), c.rect));
            continue;
        }
        if !c.shown().is_positive() || c.clip.expand(SAME).contains_rect(c.rect) || scrolled(p, c.clip) {
            continue;
        }
        let (kind, said) = if !screen.expand(SAME).contains_rect(c.rect) { ("outside", "runs past the window") } else { ("cut", "is cut off by what it's in") };
        push(out, finding(Level::Error, kind, format!("{} ({:.0}×{:.0}) {said}: give it room", c.said(), c.rect.width(), c.rect.height()), c.rect));
    }
    let shown: Vec<&Control> = showing(p).collect();
    for (i, a) in shown.iter().enumerate() {
        for b in &shown[i + 1..] {
            let (ra, rb) = (a.shown(), b.shown());
            let over = ra.intersect(rb);
            // (One inside the other is a control in a row, a cell in a
            // list: not over it.)
            if over.width() > 1.0 && over.height() > 1.0 && !ra.contains_rect(rb) && !rb.contains_rect(ra) {
                push(out, finding(Level::Error, "overlap", format!("{} and {} overlap by {:.1}×{:.1}", a.said(), b.said(), over.width(), over.height()), over));
            }
        }
    }
}

/// How far what's in the frame runs out of the box or pane it starts in,
/// at most (0: it all fits): what a window must grow by.
pub fn overflow_depth(p: &Parts, ppp: f32) -> f32 {
    overflows(p, ppp).into_iter().map(|(_, _, d)| d).fold(0.0, f32::max)
}

/// The empty space at the bottom of what holds a window's content: its
/// tab view's pane if it has one (the biggest), else the window
/// (`height` tall): what a window too tall for what it shows has to spare.
pub fn bottom_slack(p: &Parts, width: f32, height: f32) -> f32 {
    let mut items: Vec<Rect> = p.texts.iter().map(caps).collect();
    items.extend(p.controls.iter().filter(|c| c.role != "Window" && c.rect.width() >= 1.0 && !scrolled(p, c.clip)).map(|c| c.rect));
    // (A list filling the space is content too.)
    items.extend(p.scrolls.iter().copied());
    let _ = width;
    let pane = p.panes.iter().copied().max_by(|a, b| a.area().total_cmp(&b.area()));
    let (bottom, inside): (f32, Vec<&Rect>) = match pane {
        Some(b) => (b.max.y, items.iter().filter(|r| b.contains(r.min)).collect()),
        None => (height, items.iter().collect()),
    };
    bottom - inside.iter().map(|r| r.max.y).fold(0.0, f32::max)
}

/// Each thing that runs out of the bottom of the box or pane it starts
/// in: what, the box, and by how much.
fn overflows(p: &Parts, ppp: f32) -> Vec<(String, Rect, f32)> {
    let mut items: Vec<(Rect, String)> = p.texts.iter().map(|t| (caps(t), quote(t))).collect();
    items.extend(p.controls.iter().filter(|c| c.role != "Window" && c.rect.width() >= 1.0 && !scrolled(p, c.clip)).map(|c| (c.rect, c.said())));
    let mut out = Vec::new();
    for (r, name) in items {
        // The innermost box it starts in.
        let Some(b) = p.containers.iter().filter(|b| b.contains(r.min)).min_by(|a, b| a.area().total_cmp(&b.area())) else { continue };
        let d = r.max.y - b.max.y;
        // (A device pixel is layout's rounding at 1×.)
        if d > SAME.max(1.0 / ppp + 0.01) {
            out.push((name, *b, d));
        }
    }
    out
}

/// What runs past the right edge of the box or pane it starts in (a
/// check box's long label into the box's frame).
fn overflows_right(p: &Parts, ppp: f32) -> Vec<(String, Rect, f32)> {
    let mut items: Vec<(Rect, String)> = p.texts.iter().map(|t| (caps(t), quote(t))).collect();
    items.extend(p.controls.iter().filter(|c| c.role != "Window" && c.rect.width() >= 1.0 && !scrolled(p, c.clip)).map(|c| (c.rect, c.said())));
    let mut out = Vec::new();
    for (r, name) in items {
        let Some(b) = p.containers.iter().filter(|b| b.contains(r.min)).min_by(|a, b| a.area().total_cmp(&b.area())) else { continue };
        let d = r.max.x - b.max.x;
        if d > SAME.max(1.0 / ppp + 0.01) {
            out.push((name, *b, d));
        }
    }
    out
}

/// What runs out of the bottom of its box or pane (a tab's content under
/// the panel's Defaults row): the window or the box needs that much more.
fn overflow(p: &Parts, ppp: f32, out: &mut Vec<Finding>) {
    for (name, b, d) in overflows(p, ppp) {
        push(out, finding(Level::Error, "overflow", format!("{name} runs {d:.1} pt past the bottom of what it's in (y {:.0}): make that (or the window) taller", b.max.y), b));
    }
    for (name, b, d) in overflows_right(p, ppp) {
        push(
            out,
            finding(
                Level::Error,
                "overflow",
                format!("{name} runs {d:.1} pt past the right of what it's in (x {:.0}), into its inset: make that (or the window) wider, or say it in fewer words", b.max.x),
                b,
            ),
        );
    }
}

/// Text cut off (by what it's in, or the window's edge), or over text.
fn texts_over(p: &Parts, screen: Rect, out: &mut Vec<Finding>) {
    for t in &p.cut_texts {
        if scrolled(p, t.clip) {
            continue;
        }
        let (kind, said) = if !screen.expand(SAME).contains_rect(t.rect) { ("text-outside", "runs past the window") } else { ("text-cut", "is cut off by what it's in") };
        push(
            out,
            finding(
                Level::Error,
                kind,
                format!(
                    "{:?} {said} (laid out {:.0} wide, {:.0} shows; its line {:.1}–{:.1} down, what it's in {:.1}–{:.1}): give it room, or cut it short with an ellipsis (`style::fit`)",
                    t.label,
                    t.rect.width(),
                    t.rect.intersect(t.clip).width(),
                    t.rect.min.y,
                    t.rect.max.y,
                    t.clip.min.y,
                    t.clip.max.y
                ),
                t.rect,
            ),
        );
    }
    for (i, a) in p.texts.iter().enumerate() {
        for b in &p.texts[i + 1..] {
            let over = a.rect.intersect(b.rect);
            // (Lines' boxes touch by their leading, and keys' symbols set
            // closer than their advance by a point or so; their ink
            // doesn't.)
            if over.width() > 2.0 && over.height() > a.size.min(b.size) * 0.4 {
                push(out, finding(Level::Error, "text-over-text", format!("{} and {} overlap by {:.1}×{:.1}", quote(a), quote(b), over.width(), over.height()), over));
            }
        }
    }
}

/// The parts of a list or a strip the kit lays out together (rows under
/// their header, tabs side by side): spaced as the kit means them.
fn part_of_a_whole(c: &Control) -> bool {
    matches!(c.role.as_str(), "Row" | "ListItem" | "Cell" | "ColumnHeader" | "Tab" | "MenuItem" | "TreeItem")
}

/// What shows of text and controls, for spacing: (rect, name, is text,
/// baseline).
fn items(p: &Parts) -> Vec<(Rect, String, bool, f32)> {
    let mut out: Vec<(Rect, String, bool, f32)> = p.texts.iter().map(|t| (caps(t), quote(t), true, t.baseline)).collect();
    out.extend(showing(p).filter(|c| !part_of_a_whole(c)).map(|c| (c.shown(), c.said(), false, 0.0)));
    // Paragraphs (a note under a control) say themselves as labels only.
    out.extend(p.controls.iter().filter(|c| c.role == "Label" && c.shown().is_positive() && c.rect.height() > 20.0).map(|c| (c.shown(), c.said(), false, 0.0)));
    out
}

/// Two things (text, controls) side by side or one over the other with
/// next to no room between them (a point or two).
fn crowded(p: &Parts, screen: Rect, ppp: f32, out: &mut Vec<Finding>) {
    let all = items(p);
    let touching = SAME.max(1.0 / ppp);
    // (A view across the window, a document's or a list's, sets its own
    // insets: under a menu bar it's flush, as BeOS has it.)
    let across = |r: &Rect| r.width() >= 0.8 * screen.width();
    for (i, (ra, na, ta, ba)) in all.iter().enumerate() {
        for (rb, nb, tb, bb) in &all[i + 1..] {
            if (!ta && across(ra)) || (!tb && across(rb)) {
                continue;
            }
            // (Two on one grid of its own are spaced by it: a tray's icons.)
            let grid = grid_of(p, ra);
            if ra.expand(SAME).contains_rect(*rb) || rb.expand(SAME).contains_rect(*ra) || grid != grid_of(p, rb) || grid.is_some() {
                continue;
            }
            // (A line of text set in pieces, a word apart, is one line.)
            if *ta && *tb && (ba - bb).abs() <= SAME {
                continue;
            }
            let overlap_y = ra.max.y.min(rb.max.y) - ra.min.y.max(rb.min.y);
            let overlap_x = ra.max.x.min(rb.max.x) - ra.min.x.max(rb.min.x);
            let gap_x = (rb.min.x - ra.max.x).max(ra.min.x - rb.max.x);
            let gap_y = (rb.min.y - ra.max.y).max(ra.min.y - rb.max.y);
            let side = overlap_y >= 0.5 * ra.height().min(rb.height()) && gap_x > touching && gap_x < CROWDED;
            let stacked = overlap_x >= 0.5 * ra.width().min(rb.width()) && gap_y > touching && gap_y < CROWDED;
            if side || stacked {
                let gap = if side { gap_x } else { gap_y };
                push(out, finding(Level::Error, "crowded", format!("{na} and {nb} are {gap:.1} pt apart: give them room (`metrics::TIGHT` or `SPACING`)"), ra.union(*rb)));
            }
        }
    }
}

/// Text or a control almost against the window's edge (not in a bar
/// along it: a menu bar's, a toolbar's, a list's, which set their own).
fn window_edges(p: &Parts, screen: Rect, out: &mut Vec<Finding>) {
    // (Anything painted along an edge sets its own insets: a bar, a list,
    // a document's page. Not the window's own panel.)
    let bars: Vec<Rect> = p.covers.iter().copied().filter(|c| c.area() < 0.98 * screen.area()).collect();
    for (r, name, ..) in items(p) {
        for (side, d) in [("left", r.min.x - screen.min.x), ("right", screen.max.x - r.max.x), ("top", r.min.y - screen.min.y), ("bottom", screen.max.y - r.max.y)] {
            let on_edge = |b: &Rect| match side {
                "left" => b.min.x - screen.min.x <= SAME,
                "right" => screen.max.x - b.max.x <= SAME,
                "top" => b.min.y - screen.min.y <= SAME,
                _ => screen.max.y - b.max.y <= SAME,
            };
            // (A view across the window, a document's or a list's, sets
            // its own insets too.)
            // (So do a scroll view's bars, on the edge of a view along the
            // window's: a document's, as BeOS has it.)
            let in_scroll = p.scrolls.iter().any(|s| {
                s.expand(SAME).contains_rect(r)
                    && s.width() >= 0.8 * screen.width()
                    && (on_edge(s)
                        || match side {
                            "left" => s.min.x - screen.min.x < EDGE_ROOM,
                            "right" => screen.max.x - s.max.x < EDGE_ROOM,
                            "top" => s.min.y - screen.min.y < EDGE_ROOM,
                            _ => screen.max.y - s.max.y < EDGE_ROOM,
                        })
            });
            if bars.iter().any(|b| on_edge(b) && b.expand(SAME).contains_rect(r)) || grid_of(p, &r).is_some() || r.width() >= 0.8 * screen.width() || in_scroll {
                continue;
            }
            if d > SAME && d < EDGE_ROOM {
                push(out, finding(Level::Error, "window-edge", format!("{name} is {d:.1} pt from the window's {side} edge: inset it (`metrics::WINDOW_INSET`)"), r));
            }
        }
    }
}

/// Two long lines side by side, a point or three apart: a box's edge
/// almost on its container's (a box run down onto the tab view's edge).
fn crowded_lines(p: &Parts, ppp: f32, out: &mut Vec<Finding>) {
    // (Long ones: a box's or a panel's. A scroll bar's lines are a point
    // apart, and a selection's or a default button's ring a few points
    // round something small: drawn so.)
    let lines: Vec<Rect> = p.fills.iter().map(|f| f.rect).filter(|r| r.width().min(r.height()) <= 1.01 && r.width().max(r.height()) >= 120.0).collect();
    let touching = 1.5f32.max(1.0 / ppp + 0.01);
    for (i, a) in lines.iter().enumerate() {
        for b in &lines[i + 1..] {
            let flat = a.width() > a.height() && b.width() > b.height();
            let upright = a.height() > a.width() && b.height() > b.width();
            let (overlap, gap) = if flat {
                (a.max.x.min(b.max.x) - a.min.x.max(b.min.x), (b.min.y - a.max.y).max(a.min.y - b.max.y))
            } else if upright {
                (a.max.y.min(b.max.y) - a.min.y.max(b.min.y), (b.min.x - a.max.x).max(a.min.x - b.max.x))
            } else {
                continue;
            };
            let shorter = if flat { a.width().min(b.width()) } else { a.height().min(b.height()) };
            // (A scroll view's frame and its bars and rows inside it are
            // the kit's, drawn so.)
            let framing = |a: &Rect, b: &Rect| p.scrolls.iter().any(|s| s.expand(SAME).contains_rect(*a) && !s.contains_rect(*b) && s.expand(LINE_ROOM).intersects(*b));
            if framing(a, b) || framing(b, a) {
                continue;
            }
            // (A button's edge and its default ring round it, however wide
            // the button: the kit's, drawn so.)
            let ring = LINE_ROOM + 1.0;
            let ringed = p.controls.iter().any(|c| c.role == "Button" && c.rect.expand(ring).contains_rect(*a) && c.rect.expand(ring).contains_rect(*b));
            if ringed {
                continue;
            }
            if overlap >= 0.5 * shorter && gap > touching && gap < LINE_ROOM {
                let text = format!(
                    "two lines {gap:.1} pt apart (at {:.0},{:.0} and {:.0},{:.0}): an edge almost on another's; give it room (`metrics::BOX_INSET`, `WINDOW_INSET`)",
                    a.min.x, a.min.y, b.min.x, b.min.y
                );
                push(out, finding(Level::Error, "crowded-lines", text, a.union(*b)));
            }
        }
    }
}

/// Controls down a column almost lined up, on the left or the right.
fn control_edges(p: &Parts, ppp: f32, out: &mut Vec<Finding>) {
    // (A menu bar's titles are set by their lengths, as tabs are.)
    let shown: Vec<&Control> = showing(p).filter(|c| !part_of_a_whole(c) && c.role != "MenuBarItem").collect();
    let right_room = SAME.max(1.0 / ppp + 0.01);
    // (A check box or radio button ends where its label does: its right
    // edge lines up with nothing.)
    let sized = |c: &Control| !matches!(c.role.as_str(), "CheckBox" | "RadioButton" | "Label" | "Link" | "Clickable");
    for a in &shown {
        for b in &shown {
            let (ra, rb) = (a.shown(), b.shown());
            let below = rb.min.y - ra.max.y;
            if !(-2.0..=48.0).contains(&below) || grid_of(p, &ra) != grid_of(p, &rb) || section_of(p, &ra) != section_of(p, &rb) {
                continue;
            }
            let dl = (ra.min.x - rb.min.x).abs();
            let dr = (ra.max.x - rb.max.x).abs();
            if dl > SAME && dl <= NEAR_EDGE && dr > right_room {
                push(
                    out,
                    finding(
                        Level::Error,
                        "control-edges",
                        format!("{} (x {:.1}) and {} under it (x {:.1}) almost line up on the left: line them up", a.said(), ra.min.x, b.said(), rb.min.x),
                        ra.union(rb),
                    ),
                );
            } else if dr > right_room && dr <= NEAR_EDGE && dl > SAME && ra.width() >= 40.0 && rb.width() >= 40.0 && sized(a) && sized(b) {
                push(
                    out,
                    finding(
                        Level::Error,
                        "control-edges",
                        format!("{} (right {:.1}) and {} under it (right {:.1}) almost line up on the right: line them up", a.said(), ra.max.x, b.said(), rb.max.x),
                        ra.union(rb),
                    ),
                );
            }
        }
    }
}

/// The section `r` is in: the innermost band painted round it (a tray's
/// well, a workspace's row), or none. Things in different sections line
/// up only by chance.
fn section_of(p: &Parts, r: &Rect) -> Option<usize> {
    let c = r.center();
    p.covers.iter().enumerate().filter(|(_, b)| b.contains(c) && b.expand(SAME).contains_rect(*r)).min_by(|(_, a), (_, b)| a.area().total_cmp(&b.area())).map(|(i, _)| i)
}

/// A row of controls one height (buttons along the bottom) spaced
/// unevenly by a point or few.
fn uneven_gaps(p: &Parts, ppp: f32, out: &mut Vec<Finding>) {
    // (A text field by its frame, as it's seen: its rect is the text area
    // inside, whose insets round on their own and came out two device
    // pixels apart at 1.25×, the frames' gaps even.)
    let seen = |c: &Control| {
        let r = c.shown();
        if c.role != "TextInput" {
            return r;
        }
        p.frames.iter().filter(|f| f.expand(SAME).contains_rect(r)).min_by(|a, b| a.area().total_cmp(&b.area())).map_or(r, |f| f.intersect(c.clip))
    };
    let shown: Vec<Rect> = showing(p).filter(|c| matches!(c.role.as_str(), "Button" | "CheckBox" | "RadioButton" | "ComboBox" | "PopUpButton" | "TextInput")).map(seen).collect();
    let rounding = 1.0 / ppp + 0.01;
    let mut rows: Vec<Vec<Rect>> = Vec::new();
    for r in shown {
        match rows.iter_mut().find(|row| (row[0].min.y - r.min.y).abs() <= SAME && (row[0].height() - r.height()).abs() <= SAME) {
            Some(row) => row.push(r),
            None => rows.push(vec![r]),
        }
    }
    for mut row in rows {
        row.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
        let gaps: Vec<f32> = row.windows(2).map(|w| w[1].min.x - w[0].max.x).filter(|g| (0.0..=40.0).contains(g)).collect();
        for w in gaps.windows(2) {
            let d = (w[0] - w[1]).abs();
            if d > rounding && d <= NEAR_EDGE {
                push(
                    out,
                    finding(
                        Level::Error,
                        "uneven-gaps",
                        format!("controls in a row at y {:.0} are {:.1} and {:.1} pt apart: space them evenly", row[0].min.y, w[0], w[1]),
                        row[0].union(*row.last().unwrap()),
                    ),
                );
                break;
            }
        }
    }
}

/// What a commit row's buttons say: they act on what's above them.
const COMMITS: [&str; 5] = ["Apply", "Revert", "Save", "Save Changes", "Done"];

/// A commit row (Apply, Revert…) with something close under it. The row acts on what's above it and ends that group, so what
/// follows is a new section, `metrics::SECTION` under it; closer, the
/// row reads as the start of what follows (Preferences ▸ Power ▸ Graphics' Screen
/// Recordings box was 15 pt under its Apply, as close as the rows above
/// were to each other).
fn after_commit(p: &Parts, out: &mut Vec<Finding>) {
    let all = items(p);
    let mut rows: Vec<f32> = Vec::new();
    // (Enabled or not: a dimmed Apply ends its group all the same. Push
    // buttons, a commit row's: a toolbar's Save is an icon, not one.)
    let push_button = |c: &Control| c.shown().width() >= crate::metrics::BUTTON_MIN_WIDTH - SAME;
    for c in p.controls.iter().filter(|c| c.role == "Button" && COMMITS.contains(&c.name.as_str()) && c.shown().is_positive() && push_button(c)) {
        let r = c.shown();
        // (One row once. A box it's in starts above it: only what starts
        // under it counts.)
        if rows.iter().any(|y| (y - r.max.y).abs() <= SAME) {
            continue;
        }
        rows.push(r.max.y);
        let under = all.iter().filter(|(o, ..)| o.min.y > r.max.y).map(|(o, name, ..)| (o.min.y - r.max.y, name.clone(), *o));
        let boxes = p.containers.iter().filter(|b| b.min.y > r.max.y && !p.panes.contains(b)).map(|b| (b.min.y - r.max.y, "a box".to_owned(), *b));
        let Some((gap, name, at)) = under.chain(boxes).min_by(|a, b| a.0.total_cmp(&b.0)) else { continue };
        if gap < crate::metrics::SECTION - NEAR_EDGE {
            push(
                out,
                finding(Level::Error, "after-commit", format!("{} ends its group, but {name} is {gap:.0} pt under it: start what follows a section down (`metrics::SECTION`)", c.said()), r.union(at)),
            );
        }
    }
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
            let text =
                format!("an icon drawn {w:.0}×{h:.0} device px from a {tw}×{th} texture at {ppp}×: resampled a pixel off its size, it blurs (draw it at its texture's size, as icons::paint does)");
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
        let off: Vec<String> =
            edges.iter().zip(f.cut).filter(|((_, v, judged, window), cut)| *judged && !cut && (v - window).abs() > 0.01 && !whole(v * ppp)).map(|((side, v, ..), _)| format!("{side} {v}")).collect();
        if !off.is_empty() {
            let text = format!(
                "a fill {:.1}×{:.1} has edges between device pixels at {ppp}× ({}): it comes out a pixel off what it lines up with (keep sizes on the 2-point grid, style::snap what is worked out)",
                r.width(),
                r.height(),
                off.join(", ")
            );
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
                let (a, b) = if horizontal {
                    if k == 0 { (l.max.y, f.min.y) } else { (f.max.y, l.min.y) }
                } else if k == 0 {
                    (l.max.x, f.min.x)
                } else {
                    (f.max.x, l.min.x)
                };
                // The device pixel just past the line's edge, its centre.
                let first = if k == 0 { (px(a) + 0.5) / ppp } else { (px(b) - 0.5) / ppp };
                let probe = if horizontal { Pos2::new(mid_along, first) } else { Pos2::new(first, mid_along) };
                let covered = p.covers.iter().any(|c| *c != l && *c != f && c.contains(probe));
                if gap > 0.01 && gap < 1.0 && device >= 1.0 && !covered {
                    let at = |r: Rect| format!("{:.2},{:.2}–{:.2},{:.2}", r.min.x, r.min.y, r.max.x, r.max.y);
                    let text =
                        format!("a line ({}) and what it borders ({}) are {device:.0} device px apart at {ppp}× (meant to touch): inset from a rect on device pixels (style::inset)", at(l), at(f));
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
    let mut items: Vec<(Rect, f32, f32, String, bool, bool)> = p.texts.iter().map(|t| (t.rect, t.baseline, t.middle, quote(t), true, lone_symbol(t))).collect();
    items.extend(p.icons.iter().map(|i| i.rect).map(|r| (r, r.center().y, r.center().y, "an icon".to_owned(), false, false)));
    let grid = |r: &Rect| grid_of(p, r);
    for (ra, la, ma, na, ta, sa) in &items {
        // Its nearest neighbour to the right, in the same row (and on
        // the same grid, in the same box: two boxes side by side each
        // lay their rows out themselves, a slider's label in one beside
        // a menu field in the other, so their rows are level only by
        // chance, as a BBox's children were laid out in it in BeOS;
        // the boxes' labels, outside them, are held level).
        let beside = items
            .iter()
            .filter(|(rb, ..)| {
                let gap = rb.min.x - ra.max.x;
                let overlap = ra.max.y.min(rb.max.y) - ra.min.y.max(rb.min.y);
                (0.0..=160.0).contains(&gap) && overlap >= 0.4 * ra.height().min(rb.height()) && grid(ra) == grid(rb) && box_of(p, ra) == box_of(p, rb)
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
    let mut items: Vec<(Rect, f32, String)> = p.texts.iter().filter(|t| t.label.chars().count() > 3 && !centred(p, t)).map(|t| (t.rect, t.left, quote(t))).collect();
    items.extend(p.icons.iter().map(|i| i.rect).map(|r| (r, r.min.x, "an icon".to_owned())));
    // Text set from its right starts on a device pixel, so its right
    // edge is up to one off another's.
    let right = SAME.max(1.0 / ppp + 0.01);
    // One of a column set from the right (a list's sizes: "1 file, 199
    // bytes" over "sealed"): another over or under it ends where it does
    // and starts elsewhere, so where it starts is its length's doing, and
    // lines up with nothing (Backup's "sealed" under Select None).
    let from_right =
        |r: &Rect, x: f32| items.iter().any(|(o, ox, _)| (o.max.x - r.max.x).abs() <= right && (ox - x).abs() > SAME && (-2.0..=48.0).contains(&(o.min.y - r.max.y).max(r.min.y - o.max.y)));
    for (ra, xa, na) in &items {
        for (rb, xb, nb) in &items {
            let below = rb.min.y - ra.max.y;
            let d = xb - xa;
            // Lined up on the right (a column of labels before their
            // fields, of sizes), or centred over each other (icons of two
            // sizes).
            let other_way = (ra.max.x - rb.max.x).abs() <= right || (ra.center().x - rb.center().x).abs() <= SAME;
            // (On another grid, it lines up with the window's only by
            // chance. In another box, it lines up with that box's label
            // and inset, not with what's outside, as a BBox's children
            // were laid out in the box in BeOS: a check box under a box
            // lines up with its frame by its mark, its label where the
            // mark puts it, a few points from the box's text.)
            if (-2.0..=48.0).contains(&below)
                && d > SAME
                && d <= NEAR_EDGE
                && !other_way
                && grid_of(p, ra) == grid_of(p, rb)
                && box_of(p, ra) == box_of(p, rb)
                && !from_right(ra, *xa)
                && !from_right(rb, *xb)
            {
                let text = format!("{na} (x {xa:.0}) and {nb} under it (x {xb:.0}) almost line up on the left: line them up");
                push(out, finding(Level::Review, "near-edge", text, ra.union(*rb)));
            }
        }
    }
}

/// The innermost box or pane (`Parts::containers`) `r` starts in, if any.
fn box_of(p: &Parts, r: &Rect) -> Option<usize> {
    let boxes = p.containers.iter().enumerate();
    boxes.filter(|(_, b)| b.contains(r.min)).min_by(|(_, a), (_, b)| a.area().total_cmp(&b.area())).map(|(i, _)| i)
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
    p.whole_covers.iter().chain(&p.bands).copied().filter(|f| f.height() > 2.0 && f.height() <= 3.2 * r.height() && f.expand(SAME).contains_rect(r)).min_by(|a, b| a.area().total_cmp(&b.area()))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn button(name: &str, x: f32, element: &str) -> Control {
        let rect = Rect::from_min_max(pos2(x, 20.0), pos2(x + 80.0, 44.0));
        Control { rect, clip: Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 300.0)), role: "Button".into(), name: name.into(), clickable: true, element: Some(element.into()) }
    }

    /// Two buttons with one element id (a Remove in each of two boxes,
    /// neither with an id of its own) are found; with ids of their own,
    /// not.
    #[test]
    fn controls_with_one_id_are_found() {
        let mut p = Parts { controls: vec![button("Remove", 20.0, "[Remove]"), button("Remove", 200.0, "[Remove]")], ..Parts::default() };
        let found = |p: &Parts| review(p, 400.0, 300.0, 2.0).into_iter().filter(|f| f.kind == "same-id").count();
        assert_eq!(found(&p), 1);
        p.controls[1].element = Some("[face-remove]".into());
        assert_eq!(found(&p), 0);
    }
}
