//! The layout review for GPUI windows (`{"review":true}` on the app's
//! socket: `tsession review`, `olo-screen review`), with the egui kit's
//! checks (`olo_kit_base::layout_review`). Asked for, each window is
//! drawn once more while the kit's drawing ([`crate::style`], labels,
//! text fields, icons) says what it drew; the frame's last element
//! ([`crate::Frame`]) hands that to the checks and the answer goes back.
//! Otherwise nothing is recorded: drawing costs one check of whether
//! this window is being reviewed.
//!
//! What is in an area on a grid of its own ([`own_grid`]) or centred in
//! a band nothing fills ([`band`]) is marked as in the egui kit.

use std::{
    cell::RefCell,
    collections::HashMap,
    sync::mpsc,
    time::Duration,
};

use gpui::{App, Bounds, Pixels, Window, WindowId};
use olo_kit_base::{
    commands::WindowReview,
    layout_review::{self as checks, Fill, Icon, Parts, Text},
};

/// Closer than this, two things count as lined up (a rounding).
const SAME: f32 = 0.5;
/// How long a review waits for every window to draw (a window that
/// doesn't, a menu, is left out).
const WAIT: Duration = Duration::from_millis(1000);

struct Pending {
    /// Which review (a timer of an earlier one leaves this one be).
    id: u64,
    windows: HashMap<WindowId, Parts>,
    done: Vec<WindowReview>,
    reply: mpsc::Sender<Vec<WindowReview>>,
}

thread_local! {
    static PENDING: RefCell<Option<Pending>> = const { RefCell::new(None) };
}

/// Whether `window`'s drawing is being recorded.
fn recording(window: &Window) -> bool {
    PENDING.with(|p| p.borrow().as_ref().is_some_and(|p| p.windows.contains_key(&window.window_handle().window_id())))
}

fn with_parts(window: &Window, f: impl FnOnce(&mut Parts, checks::Rect)) {
    let id = window.window_handle().window_id();
    let clip = kb(window.content_mask().bounds);
    PENDING.with(|p| {
        if let Some(parts) = p.borrow_mut().as_mut().and_then(|p| p.windows.get_mut(&id)) {
            f(parts, clip);
        }
    });
}

fn kb(b: Bounds<Pixels>) -> checks::Rect {
    let (x, y) = (f32::from(b.origin.x), f32::from(b.origin.y));
    checks::Rect::from_min_max(checks::pos2(x, y), checks::pos2(x + f32::from(b.size.width), y + f32::from(b.size.height)))
}

/// A rectangle filled (`style`'s lines, frames and fills).
pub(crate) fn fill(window: &Window, b: Bounds<Pixels>) {
    if !recording(window) {
        return;
    }
    with_parts(window, |parts, clip| {
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
pub(crate) fn gradient(window: &Window, b: Bounds<Pixels>) {
    if !recording(window) {
        return;
    }
    with_parts(window, |parts, clip| {
        let r = kb(b);
        parts.covers.push(r.intersect(clip));
        if clip.contains_rect(r) {
            parts.whole_covers.push(r);
        }
    });
}

/// A line of text: where it was laid out (`rect`, its first glyph's
/// origin at `left`), its baseline, and its size in points.
pub(crate) fn text(window: &Window, rect: Bounds<Pixels>, baseline: Pixels, text: &str, size: f32) {
    if !recording(window) {
        return;
    }
    with_parts(window, |parts, clip| {
        let label: String = text.split('\n').next().unwrap_or("").trim().chars().take(28).collect();
        let r = kb(rect);
        // Text a clip cuts (in a scroll view) is left alone.
        if label.is_empty() || !clip.expand(SAME).contains_rect(r) {
            return;
        }
        let baseline = f32::from(baseline);
        parts.texts.push(Text { rect: r, baseline, middle: baseline - size * checks::CAP_HEIGHT / 2.0, left: r.min.x, label, size });
    });
}

/// A picture drawn from a bitmap `texture` pixels in size, all of it.
pub(crate) fn icon(window: &Window, b: Bounds<Pixels>, texture: [usize; 2]) {
    if !recording(window) {
        return;
    }
    with_parts(window, |parts, clip| {
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
pub fn own_grid(window: &Window, b: Bounds<Pixels>) {
    with_parts(window, |parts, _| parts.grids.push(kb(b)));
}

/// What is in `b` is centred across it, though nothing is painted to
/// show it (a tab hanging in a strip). Call it as it's drawn.
pub fn band(window: &Window, b: Bounds<Pixels>) {
    with_parts(window, |parts, _| parts.bands.push(kb(b)));
}

/// Reviews every window: each is drawn again, recorded; the answer goes
/// to `reply` once all have been (or after [`WAIT`]).
pub(crate) fn start(cx: &mut App, reply: mpsc::Sender<Vec<WindowReview>>) {
    // The app's windows (a menu showing isn't one).
    let windows: Vec<_> = cx.windows().into_iter().filter(|w| w.downcast::<crate::Frame>().is_some()).collect();
    let id = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let id = p.as_ref().map_or(0, |p| p.id + 1);
        *p = Some(Pending { id, windows: windows.iter().map(|w| (w.window_id(), Parts::default())).collect(), done: Vec::new(), reply });
        id
    });
    if windows.is_empty() {
        send(id);
        return;
    }
    for w in windows {
        let _ = w.update(cx, |_, window, _| window.draw_now());
    }
    cx.spawn(async move |cx| {
        cx.background_executor().timer(WAIT).await;
        send(id);
    })
    .detach();
}

/// The window drew its frame (the last thing [`crate::Frame`] paints):
/// its parts go to the checks.
pub(crate) fn finish(window: &Window, title: &str) {
    let id = window.window_handle().window_id();
    let all = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let Some(pending) = p.as_mut() else { return None };
        let Some(parts) = pending.windows.remove(&id) else { return None };
        let size = window.viewport_size();
        let (w, h) = (f32::from(size.width), f32::from(size.height));
        let scale = window.scale_factor();
        let findings = checks::review(&parts, w, h, scale);
        pending.done.push(WindowReview { title: title.to_owned(), width: w as u32, height: h as u32, scale, layer: None, findings });
        pending.windows.is_empty().then_some(pending.id)
    });
    if let Some(review) = all {
        send(review);
    }
}

/// Answers review `id`, if it's the one pending.
fn send(id: u64) {
    let p = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        if p.as_ref().is_some_and(|p| p.id == id) { p.take() } else { None }
    });
    if let Some(p) = p {
        let _ = p.reply.send(p.done);
    }
}
