//! Scrolling: something whose content is bigger than it shows, moved
//! under it.
//!
//! An element scrolls when its style says its overflow does
//! (`overflow_y_scroll`): what it holds is laid out at its full size and
//! drawn once, and the window's stage, on the display thread, shows it
//! from wherever it's scrolled to. The wheel and the touchpad scroll it
//! there, at once, with nothing drawn again and no work on the window's
//! looper, which is told where it got to. A [`ScrollHandle`] is how a
//! view knows where its scroller is and moves it (a scroll bar; a list
//! keeping what's chosen in view).

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{Bounds, Pixels, Point, Size, WindowCx, point, px};

static NEXT: AtomicU64 = AtomicU64::new(1);

/// What changed about a scroller: it moved across, it moved up or
/// down, its ends or its view did.
pub(crate) const ACROSS: u8 = 1;
pub(crate) const DOWN: u8 = 2;
pub(crate) const ENDS: u8 = 4;
pub(crate) const ANY: u8 = ACROSS | DOWN | ENDS;

/// The keyboard, by name: kept by a view and given to the element that
/// takes the keyboard ([`crate::InteractiveElement::track_focus`]), so
/// the view can give it the keyboard and know whether it has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusHandle(u64);

impl Default for FocusHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl FocusHandle {
    pub fn new() -> FocusHandle {
        FocusHandle(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    pub(crate) fn id(&self) -> u64 {
        self.0
    }

    /// Gives its element the keyboard (once it's shown, if it isn't
    /// yet).
    pub fn focus(&self, cx: &mut WindowCx) {
        cx.focus_handle(self.0);
    }

    /// Gives its element the keyboard if nothing in the window has it
    /// (nor has asked for it) by the time it's shown: where the keys go
    /// until something takes them.
    pub fn focus_if_none(&self, cx: &mut WindowCx) {
        cx.focus_handle_if_none(self.0);
    }

    /// Whether its element has the keyboard.
    pub fn is_focused(&self, cx: &WindowCx) -> bool {
        cx.handle_has_focus(self.0, false)
    }

    /// Whether its element, or something in it, has the keyboard.
    pub fn contains_focused(&self, cx: &WindowCx) -> bool {
        cx.handle_has_focus(self.0, true)
    }
}

/// A scroller, by name: where it is, how far it goes, and a way to move
/// it. Kept by a view and given to the element that scrolls
/// ([`crate::InteractiveElement::track_scroll`]); an element that
/// scrolls and has none keeps one of its own.
///
/// Offsets are as GPUI's: zero with the content's start showing, down to
/// minus `max_offset` at its end.
#[derive(Clone)]
pub struct ScrollHandle(pub(crate) Rc<Shared>);

pub(crate) struct Shared {
    pub(crate) id: u64,
    /// Where it's scrolled to, within its ends.
    pub(crate) offset: Cell<Point<Pixels>>,
    /// Where its content shows: past an end while it's stretched.
    pub(crate) shown: Cell<Point<Pixels>>,
    pub(crate) max: Cell<Size<Pixels>>,
    pub(crate) bounds: Cell<Bounds<Pixels>>,
    /// The looper moved it and the stage hasn't been told: with the
    /// next commit.
    pub(crate) set: Cell<bool>,
    /// How many times the looper has moved it (the stage's word on
    /// where it is, from before the latest, isn't taken).
    pub(crate) epoch: Cell<u32>,
    /// It's to go to its end, or to show one of what it holds, once
    /// that's laid out.
    pub(crate) to_bottom: Cell<bool>,
    pub(crate) to_item: Cell<Option<usize>>,
    /// A span of its content to show (down it: top, bottom).
    pub(crate) to_span: Cell<Option<(Pixels, Pixels)>>,
    pub(crate) to_offset: Cell<Option<Point<Pixels>>>,
    pub(crate) to_middle: Cell<Option<(Pixels, Pixels)>>,
    /// Where each thing it holds is, in its content.
    pub(crate) children: RefCell<Vec<Bounds<Pixels>>>,
}

impl Default for ScrollHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for ScrollHandle {
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl ScrollHandle {
    pub fn new() -> ScrollHandle {
        ScrollHandle(Rc::new(Shared {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            offset: Cell::default(),
            shown: Cell::default(),
            max: Cell::default(),
            bounds: Cell::default(),
            set: Cell::new(false),
            epoch: Cell::new(0),
            to_bottom: Cell::new(false),
            to_item: Cell::new(None),
            to_span: Cell::new(None),
            to_offset: Cell::new(None),
            to_middle: Cell::new(None),
            children: RefCell::default(),
        }))
    }

    /// Which scroller it is: part of the key of a drawing made from it
    /// (a scroll bar's knob), so one handed another scroller is drawn
    /// again from that one.
    pub fn id(&self) -> u64 {
        self.0.id
    }

    /// Where it's scrolled to: zero at its start, down to minus
    /// [`Self::max_offset`].
    pub fn offset(&self) -> Point<Pixels> {
        self.0.offset.get()
    }

    /// How far it scrolls, each way.
    pub fn max_offset(&self) -> Size<Pixels> {
        self.0.max.get()
    }

    /// Where the scroller is (as last laid out).
    pub fn bounds(&self) -> Bounds<Pixels> {
        self.0.bounds.get()
    }

    /// Where the `ix`th thing it holds is, in its content.
    pub fn bounds_for_item(&self, ix: usize) -> Option<Bounds<Pixels>> {
        self.0.children.borrow().get(ix).copied()
    }

    /// Scrolls to `offset` (kept within its ends).
    pub fn set_offset(&self, cx: &mut WindowCx, offset: Point<Pixels>) {
        let max = self.0.max.get();
        let to = point(offset.x.clamp(-max.width, px(0.)), offset.y.clamp(-max.height, px(0.)));
        let at = self.0.offset.get();
        if to != at || to != self.0.shown.get() {
            self.put(to);
            cx.core.scroll_moved(self.0.id, moved(at, to));
        }
    }

    /// For content that scrolls itself, laying out only what shows (a
    /// document editor): where it shows, how far it scrolls and where
    /// it is now, for the scroll bars on this handle, in place of what
    /// layout finds for a scroller that tracks it. The bars move it as
    /// ever ([`Self::set_offset`]); the content hears with
    /// `InteractiveElement::on_scroll_of`.
    pub fn set_extent(&self, cx: &mut WindowCx, bounds: Bounds<Pixels>, max: Size<Pixels>, offset: Point<Pixels>) {
        let at = self.0.offset.get();
        let ends = self.0.bounds.get() != bounds || self.0.max.get() != max;
        self.0.bounds.set(bounds);
        self.0.max.set(max);
        self.0.offset.set(offset);
        self.0.shown.set(offset);
        let ways = moved(at, offset) | if ends { ENDS } else { 0 };
        if ways != 0 {
            cx.core.note_scrolled(self.0.id, ways);
            cx.core.needs_commit = true;
        }
    }

    /// Scrolls by `delta` (positive towards the content's start, as an
    /// offset).
    pub fn scroll_by(&self, cx: &mut WindowCx, delta: Point<Pixels>) {
        let at = self.0.offset.get();
        self.set_offset(cx, point(at.x + delta.x, at.y + delta.y));
    }

    /// Scrolls to its end, once what it holds now is laid out (a log
    /// that just grew).
    pub fn scroll_to_bottom(&self, cx: &mut WindowCx) {
        self.0.to_bottom.set(true);
        cx.core.scroll_asked(self.clone());
    }

    /// Scrolls just far enough that the `ix`th thing it holds shows,
    /// once that's laid out.
    pub fn scroll_to_item(&self, cx: &mut WindowCx, ix: usize) {
        self.0.to_item.set(Some(ix));
        cx.core.scroll_asked(self.clone());
    }

    /// Scrolls just far enough that its content from `top` to `bottom`
    /// (points down it) shows, once it's next laid out: for what's
    /// built without the window to hand (a list bringing its chosen row
    /// into view as its view says what it shows).
    pub fn reveal_span(&self, top: Pixels, bottom: Pixels) {
        self.0.to_span.set(Some((top, bottom)));
    }

    /// Scrolls to `offset` once what it holds is next laid out, kept
    /// within its ends as they are then: for content whose size is
    /// changing in the same turn (a page zoomed about a point, where
    /// [`Self::set_offset`] would hold it to the ends it had before).
    pub fn set_offset_once_laid_out(&self, cx: &mut WindowCx, offset: Point<Pixels>) {
        self.0.to_offset.set(Some(offset));
        cx.core.scroll_asked(self.clone());
    }

    /// Scrolls so its content from `top` to `bottom` (points down it)
    /// shows in the middle of its view, if it doesn't show already, once
    /// what it holds is next laid out (a list's chosen row brought into
    /// view with what's round it, before the list has ever been laid
    /// out: its view's height isn't known until then).
    pub fn reveal_in_middle(&self, cx: &mut WindowCx, top: Pixels, bottom: Pixels) {
        self.0.to_middle.set(Some((top, bottom)));
        cx.core.scroll_asked(self.clone());
    }

    /// Whether something was asked of it that waits for layout.
    pub(crate) fn asked(&self) -> bool {
        self.0.to_bottom.get() || self.0.to_item.get().is_some() || self.0.to_span.get().is_some() || self.0.to_offset.get().is_some() || self.0.to_middle.get().is_some()
    }

    /// Puts it somewhere, from the looper's side: the stage is told with
    /// the next commit.
    pub(crate) fn put(&self, to: Point<Pixels>) {
        self.0.offset.set(to);
        self.0.shown.set(to);
        self.0.set.set(true);
        self.0.epoch.set(self.0.epoch.get().wrapping_add(1));
    }

    /// Puts it on whole device pixels at `scale`, as it's drawn there
    /// (what's scrolled is read from where it's scrolled to: a fraction
    /// of a pixel off, and every row is). Its offset is from another
    /// scale's pixels after the scale changes, and its view's while it
    /// wasn't shown (a list on a tab that isn't showing).
    pub(crate) fn snap(&self, scale: f32) {
        let at = self.0.offset.get();
        let whole = |v: Pixels| px((v.0 * scale).round() / scale);
        let to = point(whole(at.x), whole(at.y));
        if to != at {
            self.put(to);
        }
    }

    /// Does what was asked of it that waited for layout, on a display
    /// of `scale`. Which ways it moved (0: it didn't).
    pub(crate) fn settle(&self, scale: f32) -> u8 {
        let max = self.0.max.get();
        let at = self.0.offset.get();
        let mut to = self.0.to_offset.take().unwrap_or(at);
        if self.0.to_bottom.take() {
            to.y = -max.height;
        }
        if let Some(ix) = self.0.to_item.take()
            && let Some(item) = self.0.children.borrow().get(ix).copied()
        {
            // (Its place is in the content; the view shows from minus
            // the offset.)
            let view = self.0.bounds.get().size;
            let (top, bottom) = (item.top(), item.bottom());
            if top < -to.y {
                to.y = -top;
            } else if bottom > -to.y + view.height {
                to.y = -(bottom - view.height);
            }
            let (left, right) = (item.left(), item.right());
            if left < -to.x {
                to.x = -left;
            } else if right > -to.x + view.width {
                to.x = -(right - view.width);
            }
        }
        if let Some((top, bottom)) = self.0.to_span.take() {
            let view = self.0.bounds.get().size.height;
            if top < -to.y {
                to.y = -top;
            } else if bottom > -to.y + view {
                to.y = -(bottom - view);
            }
        }
        if let Some((top, bottom)) = self.0.to_middle.take() {
            let view = self.0.bounds.get().size.height;
            if top < -to.y || bottom > -to.y + view {
                to.y = -(top - (view - (bottom - top)) / 2.).max(px(0.));
            }
        }
        // On whole device pixels, as it's drawn there (a view's height
        // needn't be whole points, nor half what's left of it round a
        // row: a list's chosen row brought to its middle sat a third of
        // a point down, between pixels to everything that reads where
        // it is).
        let whole = |v: Pixels| px((v.0 * scale).round() / scale);
        let to = if to == at { to } else { point(whole(to.x), whole(to.y)) };
        let to = point(to.x.clamp(-max.width, px(0.)), to.y.clamp(-max.height, px(0.)));
        if to != at {
            self.put(to);
        }
        moved(at, to)
    }
}

/// Which ways a scroller moved, going from `at` to `to`.
pub(crate) fn moved(at: Point<Pixels>, to: Point<Pixels>) -> u8 {
    (if at.x != to.x { ACROSS } else { 0 }) | (if at.y != to.y { DOWN } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::size;

    fn handle(max: f32, view: f32) -> ScrollHandle {
        let h = ScrollHandle::new();
        h.0.max.set(size(px(0.), px(max)));
        h.0.bounds.set(Bounds { origin: point(px(10.), px(20.)), size: size(px(200.), px(view)) });
        *h.0.children.borrow_mut() = (0..20).map(|i| Bounds { origin: point(px(0.), px(i as f32 * 30.)), size: size(px(200.), px(30.)) }).collect();
        h
    }

    #[test]
    fn to_the_bottom_once_laid_out() {
        let h = handle(400., 200.);
        h.0.to_bottom.set(true);
        assert_eq!(h.settle(1.0), DOWN);
        assert_eq!(h.offset().y, px(-400.));
        // (The stage is to be told, as a move of the looper's own.)
        assert!(h.0.set.get());
        assert_eq!(h.0.epoch.get(), 1);
        // Nothing more asked: nothing moves.
        assert_eq!(h.settle(1.0), 0);
    }

    #[test]
    fn where_it_goes_is_on_whole_device_pixels() {
        // A row brought to the middle of a view that isn't whole points
        // tall (125⅓, 188 pixels at 1.5×): on a pixel, not half of what's
        // left round it.
        let h = handle(400., 125.0 + 1.0 / 3.0);
        h.0.to_middle.set(Some((px(300.), px(320.))));
        assert_ne!(h.settle(1.5), 0);
        let y = h.offset().y.0 * 1.5;
        assert!((y - y.round()).abs() < 1e-3, "{y}");
        assert!((h.offset().y.0 + 247.33).abs() < 0.01, "{:?}", h.offset());
        // On another scale's pixels (the scale changed; or it wasn't
        // shown while it did, and is shown again): put on this one's.
        h.snap(2.0);
        assert_eq!(h.offset().y, px(-247.5));
        // Already on them: it stays, and the stage isn't told again.
        let epoch = h.0.epoch.get();
        h.snap(2.0);
        assert_eq!(h.0.epoch.get(), epoch);
    }

    #[test]
    fn an_item_is_shown_by_scrolling_no_further_than_it_takes() {
        let h = handle(400., 200.);
        // Below the view: its bottom comes to the view's bottom.
        h.0.to_item.set(Some(10));
        assert_ne!(h.settle(1.0), 0);
        assert_eq!(h.offset().y, px(-(330. - 200.)));
        // Already showing: it stays.
        h.0.to_item.set(Some(8));
        assert_eq!(h.settle(1.0), 0);
        // Above the view: its top comes to the view's top.
        h.0.to_item.set(Some(1));
        assert_ne!(h.settle(1.0), 0);
        assert_eq!(h.offset().y, px(-30.));
        // Never past an end.
        h.0.to_item.set(Some(19));
        assert_ne!(h.settle(1.0), 0);
        assert_eq!(h.offset().y, px(-400.));
    }
}
