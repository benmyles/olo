//! The layout review (`{"review":true}` on the app's socket:
//! `tsession review`, `olo-screen review`), with the checks of
//! `olo_kit_base::layout_review`. Asked for, each window draws
//! everything once more while the kit's drawing ([`crate::style`],
//! labels, text fields, icons) says what it drew; that goes to the
//! checks and the answer goes back. Otherwise nothing is recorded:
//! drawing costs one check of whether the window is being reviewed.
//!
//! What is in an area on a grid of its own ([`own_grid`]) or centred in
//! a band nothing fills ([`band`]) is marked so.

use olo_interface::{Bounds, Painter, Pixels, WindowCx};
use olo_kit_base::{
    commands::WindowReview,
    layout_review::{self as checks, Control, CutText, Fill, Icon, Parts, Text},
};

/// A window the review leaves out (a menu showing: not a window's
/// layout), said as the window is made.
#[derive(Default)]
pub(crate) struct NotReviewed(pub(crate) bool);

/// Closer than this, two things count as lined up (a rounding).
const SAME: f32 = 0.5;

/// Whether what `g` draws is being recorded for a review.
pub(crate) fn recording(g: &Painter) -> bool {
    g.has_ext()
}

thread_local! {
    /// How far what's being recorded shows from where it was drawn
    /// (`Painter::scrolled_by`): the review goes by where things show.
    static SHIFT: std::cell::Cell<(f32, f32)> = const { std::cell::Cell::new((0.0, 0.0)) };
}

fn with_parts(g: &mut Painter, f: impl FnOnce(&mut Parts, checks::Rect)) {
    let by = g.scrolled_by();
    SHIFT.set((f32::from(by.x), f32::from(by.y)));
    let clip = kb(g.clip());
    if let Some(parts) = g.ext::<Parts>() {
        f(parts, clip);
    }
    SHIFT.set((0.0, 0.0));
}

/// `b` as the checks take it, where it shows (within [`with_parts`]:
/// moved by what scrolls round it).
fn kb(b: Bounds<Pixels>) -> checks::Rect {
    let (dx, dy) = SHIFT.get();
    let (x, y) = (f32::from(b.origin.x) + dx, f32::from(b.origin.y) + dy);
    checks::Rect::from_min_max(checks::pos2(x, y), checks::pos2(x + f32::from(b.size.width), y + f32::from(b.size.height)))
}

/// A rectangle filled (`style`'s lines, frames and fills).
pub(crate) fn fill(g: &mut Painter, b: Bounds<Pixels>) {
    if !recording(g) {
        return;
    }
    with_parts(g, |parts, clip| {
        let r = kb(b);
        let seen = r.intersect(clip);
        if !seen.is_positive() {
            return;
        }
        let cut = [r.min.y < seen.min.y, r.max.y > seen.max.y, r.min.x < seen.min.x, r.max.x > seen.max.x];
        parts.fills.push(Fill { rect: seen, cut });
        parts.covers.push(seen);
        if seen == r {
            parts.whole_covers.push(seen);
        }
    });
}

/// An opaque gradient filling `b`.
pub(crate) fn gradient(g: &mut Painter, b: Bounds<Pixels>) {
    if !recording(g) {
        return;
    }
    with_parts(g, |parts, clip| {
        let r = kb(b);
        parts.covers.push(r.intersect(clip));
        if clip.contains_rect(r) {
            parts.whole_covers.push(r);
        }
    });
}

/// A line of text: where it was laid out (`rect`, its first glyph's
/// origin at `left`), its baseline, and its size in points.
pub(crate) fn text(g: &mut Painter, rect: Bounds<Pixels>, baseline: Pixels, text: &str, size: f32) {
    if !recording(g) {
        return;
    }
    with_parts(g, |parts, clip| {
        let label: String = text.split('\n').next().unwrap_or("").trim().chars().take(28).collect();
        let r = kb(rect);
        if label.is_empty() {
            return;
        }
        // Text a clip cuts: scrolled out of sight, or cut off (the
        // checks tell which by the scroll views).
        if !clip.expand(SAME).contains_rect(r) {
            if r.intersects(clip) {
                parts.cut_texts.push(CutText { rect: r, clip, label });
            }
            return;
        }
        let baseline = f32::from(baseline) + SHIFT.get().1;
        parts.texts.push(Text { rect: r, baseline, middle: baseline - size * checks::CAP_HEIGHT / 2.0, left: r.min.x, label, size });
    });
}

/// A picture drawn from a bitmap `texture` pixels in size, all of it.
pub(crate) fn icon(g: &mut Painter, b: Bounds<Pixels>, texture: [usize; 2]) {
    if !recording(g) {
        return;
    }
    with_parts(g, |parts, clip| {
        let r = kb(b);
        // Pictures (thumbnails, previews) are scaled anyway; icons are
        // drawn at their size.
        if r.width() <= 128.0 && r.height() <= 128.0 && r.intersects(clip) {
            parts.icons.push(Icon { rect: r, texture: Some(texture), whole: true });
        }
    });
}

/// The text in `b` is laid out on a grid of its own (a terminal's
/// screen), not the window's: it isn't held level with what's beside
/// it. Call it as it's drawn.
pub fn own_grid(g: &mut Painter, b: Bounds<Pixels>) {
    with_parts(g, |parts, _| parts.grids.push(kb(b)));
}

/// `b` is a box or a pane what's in it must fit in (a BBox's frame, a tab
/// view's pane): the review finds what runs out of it.
pub fn container(g: &mut Painter, b: Bounds<Pixels>) {
    with_parts(g, |parts, clip| {
        if let Some(r) = shown(b, clip) {
            parts.containers.push(r);
        }
    });
}

/// What shows of `b` in `clip` (a box half scrolled out of sight is, to
/// the review, the half that shows: what's below isn't in it), if any
/// does.
fn shown(b: Bounds<Pixels>, clip: checks::Rect) -> Option<checks::Rect> {
    let r = kb(b).intersect(clip);
    r.is_positive().then_some(r)
}

/// `b` is a tab view's pane: what's in it must fit, and a window's pages
/// are measured in it (the lint's fit check).
pub fn pane(g: &mut Painter, b: Bounds<Pixels>) {
    with_parts(g, |parts, clip| {
        if let Some(r) = shown(b, clip) {
            parts.containers.push(r);
            parts.panes.push(r);
        }
    });
}

/// `b` is a control's frame, round its agent rect (a text field's well):
/// what the eye goes by in a row of controls.
pub fn frame(g: &mut Painter, b: Bounds<Pixels>) {
    with_parts(g, |parts, clip| {
        if let Some(r) = shown(b, clip) {
            parts.frames.push(r);
        }
    });
}

/// `b` is a scroll view's viewport: what it cuts is scrolled out of
/// sight, not cut off. (ScrollView and ColumnListView say it.)
pub fn scroll_area(g: &mut Painter, b: Bounds<Pixels>) {
    with_parts(g, |parts, _| parts.scrolls.push(kb(b)));
}

/// What is in `b` is centred across it, though nothing is painted to
/// show it (a tab hanging in a strip). Call it as it's drawn.
pub fn band(g: &mut Painter, b: Bounds<Pixels>) {
    with_parts(g, |parts, _| parts.bands.push(kb(b)));
}

/// Reviews a window, on its looper: everything is drawn once more,
/// recorded, and checked. How far its content ran out of its boxes and
/// the empty space at its bottom come too (the lint's fit check).
pub(crate) fn review_window(cx: &mut WindowCx, dark: bool) -> (WindowReview, (f32, f32)) {
    let before = cx.set_paint_ext(Some(Box::new(Parts::default())));
    cx.refresh();
    cx.flush_now();
    let mut parts = cx.set_paint_ext(before).and_then(|p| p.downcast::<Parts>().ok()).map(|p| *p).unwrap_or_default();
    // (What was drawn for the review is as good as any: nothing more
    // to draw.)
    let size = cx.viewport_size();
    let whole = kb(Bounds { origin: Default::default(), size });
    parts.controls = cx
        .agent_nodes()
        .into_iter()
        .map(|n| Control {
            rect: kb(n.bounds),
            clip: n.clip.map_or(whole, kb),
            role: n.role.to_owned(),
            name: n.name.as_ref().map(|s| s.to_string()).unwrap_or_default(),
            clickable: n.clickable,
            element: None,
        })
        .collect();
    let (w, h) = (f32::from(size.width), f32::from(size.height));
    let scale = cx.scale_factor();
    let findings = checks::review(&parts, w, h, scale);
    let fit = (checks::overflow_depth(&parts, scale), checks::bottom_slack(&parts, w, h));
    (WindowReview { title: cx.window_title().to_string(), width: w as u32, height: h as u32, scale, dark, layer: None, resizing: cx.commit_is_held(), findings }, fit)
}
