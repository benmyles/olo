//! Stacking windows by hand, as in Haiku's Stack & Tile, with ⌘ for its
//! Option key: ⌘-drag a window's tab.
//!
//! - Along its group's tab bar, the tab follows the pointer and the others
//!   make room where it would go; let go, and it is there (`Phase::Along`).
//! - Pulled away from the tab bar (`TEAR` above or below it, `TEAR_SIDE`
//!   past its ends), the window comes out of its group and follows the
//!   pointer on its own, its tab its own again, the pointer still on it
//!   (`Phase::Loose`). Let go there, it stays out: on its own until put in
//!   a group (`Grouping::Alone`).
//! - Held over another window's or group's tab bar (within `NEAR` of it,
//!   and not hidden behind another window there), whatever its app, the
//!   window folds into just its tab, in the place it would take among
//!   theirs, and they make room for it; that group's tabs and frame light
//!   up in the focus blue (`Decoration::target`). Let go, and it is in that
//!   group, in front, at its frame (`put_in_group`); moved off again, it is
//!   whole under the pointer. Back over its own group, it goes back in.
//!
//! The same ⌘-drag of a window in no group moves it, and puts it in
//! whatever group (or window) it is let go over.

use std::time::Instant;

use olo_theme::deco::TAB_HEIGHT;
use smithay::{
    desktop::space::SpaceElement,
    input::pointer::{
        AxisFrame, ButtonEvent, CursorIcon, CursorImageStatus, Focus, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
        GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
    },
    utils::{IsAlive, Logical, Point, Rectangle, Serial},
};

use super::{
    WindowElement,
    groups::{in_tab_order, tab_bar, tab_place},
};
use crate::{
    focus::PointerFocusTarget,
    state::{AnvilState, Backend},
};

/// How far above or below the tab bar the pointer goes before the tab
/// comes out of its group.
pub const TEAR: f64 = 24.0;
/// How far past the group's sides.
pub const TEAR_SIDE: f64 = 48.0;
/// How near another tab bar (above or below it) the pointer must be for
/// the window to go in there.
pub const NEAR: f64 = 12.0;
/// The least of a tab kept on either side of the pointer once it is the
/// window's own again.
const HOLD_INSET: f64 = 8.0;

/// A tab being ⌘-dragged.
#[derive(Debug)]
pub struct TabDrag {
    pub window: WindowElement,
    /// Where on its tab it is held: from the tab's left, and the top.
    hold: Point<f64, Logical>,
    pub phase: Phase,
}

#[derive(Debug)]
pub enum Phase {
    /// Along its group's tabs: its tab this far along the top, and the
    /// place among the others it would take.
    Along { x: i32, index: usize },
    /// Out on its own, following the pointer; over a window it would go
    /// in with, if any.
    Loose { over: Option<Over> },
}

#[derive(Debug)]
pub struct Over {
    /// The window under the pointer, whose group it would go in (or a
    /// new one with it, if it is in none).
    pub target: WindowElement,
    /// Its tab's place among theirs.
    pub index: usize,
    /// The target's own tab (offset, width), when it is in no group: laid
    /// out beside the held one meanwhile, and put back if it isn't
    /// joined.
    alone: Option<(i32, Option<i32>)>,
}

impl TabDrag {
    /// Out of any group, following the pointer (or held over one).
    pub fn loose(&self) -> bool {
        matches!(self.phase, Phase::Loose { .. })
    }

    fn over(&self) -> Option<&Over> {
        match &self.phase {
            Phase::Loose { over } => over.as_ref(),
            Phase::Along { .. } => None,
        }
    }
}

impl<BackendData: Backend> AnvilState<BackendData> {
    /// ⌘ is held (as the command key, or as Super when it isn't one).
    pub fn command_held(&self) -> bool {
        self.keymap.command_held() || self.seat.get_keyboard().is_some_and(|k| k.modifier_state().logo)
    }

    /// The tabs of a group `tabs` (in order) at `frame` as the drag lays
    /// them: the order with the dragged tab in its place, and the held one
    /// (with where it is held), if it is held along them; with the
    /// dragged window in its place, if it is held over them to go in.
    pub(super) fn drag_slots(&mut self, tabs: &[WindowElement], frame: Rectangle<i32, Logical>) -> (Vec<WindowElement>, Option<(WindowElement, i32)>) {
        let Some(drag) = &self.tab_drag else { return (tabs.to_vec(), None) };
        let w = drag.window.clone();
        match &drag.phase {
            Phase::Along { x, index } if tabs.contains(&w) => {
                let mut slots: Vec<WindowElement> = tabs.iter().filter(|o| **o != w).cloned().collect();
                slots.insert((*index).min(slots.len()), w.clone());
                (slots, Some((w, *x)))
            }
            Phase::Loose { over: Some(o) } if tabs.contains(&o.target) && !tabs.contains(&w) => {
                let mut slots = tabs.to_vec();
                slots.insert(o.index.min(slots.len()), w.clone());
                // Folded into its tab, at the group's top-left so its tab's
                // offset is among theirs.
                w.decoration().docked = true;
                if self.space.element_location(&w) != Some(frame.loc) {
                    self.space.relocate_element(&w, frame.loc);
                }
                (slots, None)
            }
            _ => (tabs.to_vec(), None),
        }
    }

    /// A ⌘-press on `window`'s tab: drag it (deferred like moves: this is
    /// inside the pointer's button handler).
    pub(super) fn start_tab_drag(&mut self, window: &WindowElement, serial: Serial) {
        self.keymap.command_used();
        let window = window.clone();
        self.handle.insert_idle(move |data| {
            let Some(start_data) = data.pointer.grab_start_data() else { return };
            if !window.alive() || data.space.element_location(&window).is_none() {
                return;
            }
            let (title, content) = (window.title(), window.content_size());
            let (at, tab) = {
                let mut d = window.decoration();
                (d.pointer, d.tab_layout(&title, content))
            };
            let hold = Point::from((at.x - tab.x as f64, at.y));
            let phase = match data.groups.place(&window) {
                Some((_, index)) => Phase::Along { x: tab.x, index: index as usize },
                None => Phase::Loose { over: None },
            };
            tracing::info!(id = window.id(), ?phase, "tab drag");
            data.tab_drag = Some(TabDrag { window, hold, phase });
            data.cursor_status = CursorImageStatus::Named(CursorIcon::Grabbing);
            let pointer = data.pointer.clone();
            pointer.set_grab(data, TabDragGrab { start_data }, serial, Focus::Clear);
        });
    }

    /// The pointer at `p` while a tab is dragged.
    pub fn tab_drag_motion(&mut self, p: Point<f64, Logical>) {
        let Some(mut drag) = self.tab_drag.take() else { return };
        let w = drag.window.clone();
        if !w.alive() || self.space.element_location(&w).is_none() {
            self.drop_target(&mut drag, None);
            return;
        }
        self.cursor_status = CursorImageStatus::Named(CursorIcon::Grabbing);
        if let Phase::Along { index, .. } = drag.phase {
            let mates = self.groups.mates(&w);
            let frame = self.space.element_geometry(&w);
            match frame {
                Some(frame) if mates.len() > 1 && !torn(frame, p) => {
                    let others: Vec<WindowElement> = in_tab_order(&mates).into_iter().filter(|o| *o != w).collect();
                    let wanted: Vec<i32> = others.iter().map(wanted_width).collect();
                    let width = wanted_width(&w);
                    let left = p.x - frame.loc.x as f64 - drag.hold.x;
                    // Its slot's width where it is now, for its middle.
                    let mut with = wanted.clone();
                    with.insert(index.min(wanted.len()), width);
                    let slot_w = tab_bar(&with, frame.size.w)[index.min(wanted.len())].1;
                    let (index, (_, slot_w)) = tab_place(&wanted, width, frame.size.w, left + slot_w as f64 / 2.0, Some(index));
                    let x = (left.round() as i32).clamp(0, (frame.size.w - slot_w).max(0));
                    drag.phase = Phase::Along { x, index };
                    self.tab_drag = Some(drag);
                    self.lay_tabs(&mates, frame);
                    return;
                }
                _ => self.tear_off(&mut drag),
            }
        }
        // On its own: over another group's tabs, or following the pointer.
        let target = self.target_at(p, &w);
        let index = target.as_ref().map(|t| {
            let tabs = self.tabs_of(t);
            let frame = self.space.element_geometry(t).unwrap_or_default();
            let wanted: Vec<i32> = tabs.iter().map(wanted_width).collect();
            let current = drag.over().filter(|o| o.target == *t).map(|o| o.index);
            tab_place(&wanted, wanted_width(&w), frame.size.w, p.x - frame.loc.x as f64, current).0
        });
        self.drop_target(&mut drag, target.zip(index));
        match drag.over().map(|o| o.target.clone()) {
            Some(t) => {
                self.tab_drag = Some(drag);
                let tabs = self.tabs_of(&t);
                if let Some(frame) = self.space.element_geometry(&t) {
                    self.lay_tabs(&tabs, frame);
                }
            }
            None => {
                {
                    let mut d = w.decoration();
                    d.docked = false;
                    d.tab_width = None;
                    d.tab_offset = 0;
                }
                let loc = (p - drag.hold).to_i32_round();
                if self.space.element_location(&w) != Some(loc) {
                    self.moved_by_hand(&w);
                    self.space.relocate_element(&w, loc);
                }
                self.tab_drag = Some(drag);
            }
        }
    }

    /// The tab let go of.
    pub fn end_tab_drag(&mut self) {
        let Some(mut drag) = self.tab_drag.take() else { return };
        self.cursor_status = CursorImageStatus::default_named();
        let w = drag.window.clone();
        let over = match &mut drag.phase {
            Phase::Loose { over } => over.take(),
            Phase::Along { .. } => None,
        };
        match (&drag.phase, over) {
            (Phase::Along { index, .. }, _) => {
                tracing::info!(id = w.id(), index, "tab moved");
                self.move_tab(&w, *index);
            }
            (Phase::Loose { .. }, Some(over)) => {
                self.light(&over.target, false);
                let joined = w.alive() && self.put_in_group(&w, &over.target, Some(over.index));
                if joined {
                    // Its tab only until it has drawn at the group's size.
                    w.decoration().docked_since = Some(Instant::now());
                } else {
                    self.put_back_alone(&over);
                    let mut d = w.decoration();
                    d.docked = false;
                    d.tab_width = None;
                    d.tab_offset = 0;
                }
            }
            (Phase::Loose { .. }, None) => {
                w.decoration().docked = false;
            }
        }
    }

    /// Out of its group, under the pointer, its own tab back.
    fn tear_off(&mut self, drag: &mut TabDrag) {
        let w = drag.window.clone();
        self.take_out_of_group(&w);
        let (title, content) = (w.title(), w.content_size());
        let own = {
            let mut d = w.decoration();
            d.tab_width = None;
            d.tab_offset = 0;
            d.docked = false;
            d.tab_layout(&title, content).width as f64
        };
        self.moved_by_hand(&w);
        drag.hold.x = drag.hold.x.clamp(HOLD_INSET.min(own / 2.0), (own - HOLD_INSET).max(own / 2.0));
        drag.phase = Phase::Loose { over: None };
        tracing::info!(id = w.id(), "tab torn off");
    }

    /// Moved by hand now: no longer tiled or zoomed (at the size it is).
    fn moved_by_hand(&mut self, w: &WindowElement) {
        self.forget_tile(w);
        if self.is_zoomed(w)
            && let Some(frame) = self.space.element_geometry(w)
        {
            self.set_frame(w, frame, false);
        }
    }

    /// The window whose tab bar is under `p` (or near it), the one being
    /// dragged left out: a groupable one not hidden there by another.
    fn target_at(&self, p: Point<f64, Logical>, dragged: &WindowElement) -> Option<WindowElement> {
        for e in self.space.elements().rev() {
            if e == dragged {
                continue;
            }
            let Some(frame) = self.space.element_geometry(e) else { continue };
            let (x, y) = (frame.loc.x as f64, frame.loc.y as f64);
            let row = p.x >= x && p.x < x + frame.size.w as f64 && p.y >= y - NEAR && p.y < y + TAB_HEIGHT as f64 + NEAR;
            if row && self.groupable(e) {
                return Some(e.clone());
            }
            // In front, it hides whatever is behind it there.
            if e.is_in_input_region(&(p - frame.loc.to_f64())) {
                return None;
            }
        }
        None
    }

    /// `t`'s group's tabs in order, or `t` alone.
    fn tabs_of(&self, t: &WindowElement) -> Vec<WindowElement> {
        let mates = self.groups.mates(t);
        if mates.is_empty() { vec![t.clone()] } else { in_tab_order(&mates) }
    }

    /// Hold the dragged tab over `target` (at a place), or over nothing:
    /// the old target unlit and its tabs back, the new one lit.
    fn drop_target(&mut self, drag: &mut TabDrag, target: Option<(WindowElement, usize)>) {
        let Phase::Loose { over } = &mut drag.phase else { return };
        let same = matches!((&*over, &target), (Some(o), Some((t, _))) if o.target == *t);
        if same {
            if let (Some(o), Some((_, index))) = (over.as_mut(), target) {
                o.index = index;
            }
            return;
        }
        if let Some(old) = over.take() {
            self.light(&old.target, false);
            self.put_back_alone(&old);
        }
        if let Some((t, index)) = target {
            self.light(&t, true);
            let alone = (!self.groups.has(&t)).then(|| {
                let d = t.decoration();
                (d.tab_offset, d.tab_width)
            });
            *over = Some(Over { target: t, index, alone });
        }
    }

    /// `t`'s group (or `t`) lit up as where a tab would go, or not.
    fn light(&self, t: &WindowElement, on: bool) {
        for w in self.tabs_of(t) {
            w.decoration().target = on;
        }
    }

    /// A target in no group, its own tab back where it was.
    fn put_back_alone(&self, over: &Over) {
        if let Some((offset, width)) = over.alone
            && !self.groups.has(&over.target)
        {
            let mut d = over.target.decoration();
            d.tab_offset = offset;
            d.tab_width = width;
        }
    }
}

/// Whether `p` is far enough from the tab bar of a group at `frame` for
/// the tab to come out.
pub fn torn(frame: Rectangle<i32, Logical>, p: Point<f64, Logical>) -> bool {
    let (x, y, w) = (frame.loc.x as f64, frame.loc.y as f64, frame.size.w as f64);
    p.y < y - TEAR || p.y > y + TAB_HEIGHT as f64 + TEAR || p.x < x - TEAR_SIDE || p.x > x + w + TEAR_SIDE
}

fn wanted_width(w: &WindowElement) -> i32 {
    let title = w.title();
    w.decoration().wanted_tab_width(&title)
}

/// The pointer while a tab is ⌘-dragged (see the module's notes).
pub struct TabDragGrab<BackendData: Backend + 'static> {
    start_data: PointerGrabStartData<AnvilState<BackendData>>,
}

impl<BackendData: Backend> PointerGrab<AnvilState<BackendData>> for TabDragGrab<BackendData> {
    fn motion(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        _focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        data.tab_drag_motion(event.location);
    }

    fn relative_motion(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(&mut self, data: &mut AnvilState<BackendData>, handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>, event: &ButtonEvent) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(&mut self, data: &mut AnvilState<BackendData>, handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>, details: AxisFrame) {
        handle.axis(data, details)
    }

    fn frame(&mut self, data: &mut AnvilState<BackendData>, handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(&mut self, data: &mut AnvilState<BackendData>, handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>, event: &GestureSwipeBeginEvent) {
        handle.gesture_swipe_begin(data, event)
    }

    fn gesture_swipe_update(&mut self, data: &mut AnvilState<BackendData>, handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>, event: &GestureSwipeUpdateEvent) {
        handle.gesture_swipe_update(data, event)
    }

    fn gesture_swipe_end(&mut self, data: &mut AnvilState<BackendData>, handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>, event: &GestureSwipeEndEvent) {
        handle.gesture_swipe_end(data, event)
    }

    fn gesture_pinch_begin(&mut self, data: &mut AnvilState<BackendData>, handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>, event: &GesturePinchBeginEvent) {
        handle.gesture_pinch_begin(data, event)
    }

    fn gesture_pinch_update(&mut self, data: &mut AnvilState<BackendData>, handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>, event: &GesturePinchUpdateEvent) {
        handle.gesture_pinch_update(data, event)
    }

    fn gesture_pinch_end(&mut self, data: &mut AnvilState<BackendData>, handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>, event: &GesturePinchEndEvent) {
        handle.gesture_pinch_end(data, event)
    }

    fn gesture_hold_begin(&mut self, data: &mut AnvilState<BackendData>, handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>, event: &GestureHoldBeginEvent) {
        handle.gesture_hold_begin(data, event)
    }

    fn gesture_hold_end(&mut self, data: &mut AnvilState<BackendData>, handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>, event: &GestureHoldEndEvent) {
        handle.gesture_hold_end(data, event)
    }

    fn start_data(&self) -> &PointerGrabStartData<AnvilState<BackendData>> {
        &self.start_data
    }

    /// Let go, or another grab took over: either way the drag ends here.
    fn unset(&mut self, data: &mut AnvilState<BackendData>) {
        data.end_tab_drag();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tab_tears_off_once_pulled_away_from_the_bar() {
        let frame = Rectangle::new((100, 100).into(), (400, 300).into());
        let at = |x: f64, y: f64| torn(frame, Point::from((x, y)));
        // On the bar, and a little above or below it: still along.
        assert!(!at(150.0, 110.0));
        assert!(!at(150.0, 100.0 - TEAR + 1.0));
        assert!(!at(150.0, 100.0 + TAB_HEIGHT as f64 + TEAR - 1.0));
        // Past its ends, a little: along still (it stops at the end).
        assert!(!at(100.0 - TEAR_SIDE + 1.0, 110.0));
        assert!(!at(500.0 + TEAR_SIDE - 1.0, 110.0));
        // Further: out.
        assert!(at(150.0, 100.0 - TEAR - 1.0));
        assert!(at(150.0, 100.0 + TAB_HEIGHT as f64 + TEAR + 1.0));
        assert!(at(100.0 - TEAR_SIDE - 1.0, 110.0));
        assert!(at(500.0 + TEAR_SIDE + 1.0, 110.0));
    }
}
