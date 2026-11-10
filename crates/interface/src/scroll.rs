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
            children: RefCell::default(),
        }))
    }

    pub(crate) fn id(&self) -> u64 {
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
        if to != self.0.offset.get() || to != self.0.shown.get() {
            self.put(to);
            cx.core.scroll_moved(self.0.id);
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

    /// Puts it somewhere, from the looper's side: the stage is told with
    /// the next commit.
    pub(crate) fn put(&self, to: Point<Pixels>) {
        self.0.offset.set(to);
        self.0.shown.set(to);
        self.0.set.set(true);
        self.0.epoch.set(self.0.epoch.get().wrapping_add(1));
    }

    /// Does what was asked of it that waited for layout. Whether it
    /// moved.
    pub(crate) fn settle(&self) -> bool {
        let max = self.0.max.get();
        let at = self.0.offset.get();
        let mut to = at;
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
        let to = point(to.x.clamp(-max.width, px(0.)), to.y.clamp(-max.height, px(0.)));
        if to == at {
            return false;
        }
        self.put(to);
        true
    }
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
        assert!(h.settle());
        assert_eq!(h.offset().y, px(-400.));
        // (The stage is to be told, as a move of the looper's own.)
        assert!(h.0.set.get());
        assert_eq!(h.0.epoch.get(), 1);
        // Nothing more asked: nothing moves.
        assert!(!h.settle());
    }

    #[test]
    fn an_item_is_shown_by_scrolling_no_further_than_it_takes() {
        let h = handle(400., 200.);
        // Below the view: its bottom comes to the view's bottom.
        h.0.to_item.set(Some(10));
        assert!(h.settle());
        assert_eq!(h.offset().y, px(-(330. - 200.)));
        // Already showing: it stays.
        h.0.to_item.set(Some(8));
        assert!(!h.settle());
        // Above the view: its top comes to the view's top.
        h.0.to_item.set(Some(1));
        assert!(h.settle());
        assert_eq!(h.offset().y, px(-30.));
        // Never past an end.
        h.0.to_item.set(Some(19));
        assert!(h.settle());
        assert_eq!(h.offset().y, px(-400.));
    }
}
