//! A pane beside the rest of a window that the person sizes and puts away
//! (Ben, 2026-11-09: "make it so that i can shrink/collapse the contact
//! sidebar when i want for more space … if that shrink/collapse thing
//! isn't already part of the olo ui kit it probably should be so it's
//! reusable"): a chat list, a source list, an inspector.
//!
//! Its divider is dragged to size it, down to its narrowest (`min`);
//! dragged on past half that, it goes, and the rest of the window has
//! its room; dragged out again from the window's edge (where a grip
//! stays), it comes back. A double click on the divider puts it away or
//! brings it back, as does the app's View ▸ Hide Sidebar, which calls
//! [`SidePane::toggle`] (every app's key for it: `app.sidebar`, ⌥⌘S on a
//! Mac; give the item `.key("s")` and the profile's keys take its place).
//! Its width and whether it's away are the app's to keep across restarts
//! and upgrades ([`SidePane::width`], [`SidePane::collapsed`]).
//!
//! In a window too narrow for both, the pane's narrowest beside what the
//! rest needs (a tile a third of a screen wide: CLAUDE.md, "Every tiling
//! layout"), they show one at a time ([`SidePane::panes`]), each the
//! whole width, never both squeezed. A list to choose from (Telegram's
//! chats, Phone's conversations) shows first, and what it opens once
//! something is ([`SidePane::show_rest`]), with a back arrow to the list
//! (`ArrowButton::back`, calling [`SidePane::show_pane`]). A pane that
//! gives way (a sidebar of calendars, an assistant) has what's beside it
//! first ([`SidePane::first`]); its View ▸ Show brings it in front
//! ([`SidePane::toggle`]), its back arrow takes it away again.
//!
//! The view keeps the [`SidePane`] (a handle: clones are the same pane)
//! and is told when its divider changed what shows (`on_change`):
//!
//! ```ignore
//! match self.pane.panes(total_width, 320.0) {
//!     Panes::Both(w) => { /* the pane w wide, the rest beside it, and: */
//!         layer.child(side_divider(&self.pane, w, top, height, "Chat List", cx.msg(Msg::PaneMoved))) }
//!     Panes::Pane => { /* the pane alone */ }
//!     Panes::Rest => { /* the rest alone, a back arrow if `self.pane.alone()` */ }
//! }
//! ```

use std::{cell::RefCell, rc::Rc};

use olo_interface::{CursorStyle, Element, InteractiveElement as _, MouseButton, ParentElement as _, Styled as _, WindowCx, agent, canvas, div, px};

/// The divider's grip, points across: from the pane's edge over what's
/// beside it (the pane's own scroll bar runs up to its edge).
pub const GRIP: f32 = 6.0;

/// Which shows where there's room for only one: the pane, or what's
/// beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Front {
    Pane,
    Rest,
}

/// What a window with a side pane shows ([`SidePane::panes`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Panes {
    /// Side by side, the pane this wide (whole points).
    Both(f32),
    /// The pane alone, the whole width: too narrow for both, the pane in
    /// front.
    Pane,
    /// What's beside the pane alone: the pane put away, or too narrow for
    /// both and what's beside it in front.
    Rest,
}

impl Panes {
    /// The pane's width beside the rest: 0 unless they're side by side.
    pub fn width(self) -> f32 {
        if let Panes::Both(w) = self { w } else { 0.0 }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Pane {
    /// Its width when shown (at least `min`).
    width: f32,
    collapsed: bool,
    min: f32,
    /// The divider being dragged: where it was pressed, and the width
    /// shown then (0: away).
    drag: Option<(f32, f32)>,
    /// Which shows in a window too narrow for both.
    front: Front,
    /// Laid out last too narrow for both.
    alone: bool,
}

/// A side pane's size and whether it's put away.
#[derive(Debug, Clone)]
pub struct SidePane(Rc<RefCell<Pane>>);

impl SidePane {
    /// `width` wide, never narrower than `min` while shown.
    pub fn new(width: f32, min: f32) -> SidePane {
        SidePane(Rc::new(RefCell::new(Pane { width: width.max(min), collapsed: false, min, drag: None, front: Front::Pane, alone: false })))
    }

    /// Which shows first in a window too narrow for both: [`Front::Rest`]
    /// for a pane that gives way (a sidebar, an assistant); the pane, a
    /// list to choose from, unless told.
    pub fn first(self, front: Front) -> SidePane {
        self.0.borrow_mut().front = front;
        self
    }

    /// As kept: `width`, and put away or not.
    pub fn restored(self, width: Option<f32>, collapsed: bool) -> SidePane {
        {
            let mut p = self.0.borrow_mut();
            if let Some(w) = width {
                p.width = w.max(p.min);
            }
            p.collapsed = collapsed;
        }
        self
    }

    /// Its width when shown.
    pub fn width(&self) -> f32 {
        self.0.borrow().width
    }

    /// Put away.
    pub fn collapsed(&self) -> bool {
        self.0.borrow().collapsed
    }

    pub fn set_collapsed(&self, collapsed: bool) {
        self.0.borrow_mut().collapsed = collapsed;
    }

    /// Put away, or brought back (View ▸ Hide Sidebar). One at a time
    /// (laid out last too narrow for both), the pane or what's beside it
    /// brought in front instead: shown so, it's not put away.
    pub fn toggle(&self) {
        let mut p = self.0.borrow_mut();
        if p.alone {
            p.front = match p.front {
                Front::Pane => Front::Rest,
                Front::Rest => Front::Pane,
            };
            if p.front == Front::Pane {
                p.collapsed = false;
            }
        } else {
            p.collapsed = !p.collapsed;
        }
    }

    /// Something in the pane opened (a chat chosen): one at a time, what's
    /// beside it shows.
    pub fn show_rest(&self) {
        self.0.borrow_mut().front = Front::Rest;
    }

    /// Back to the pane (the back arrow): one at a time, the pane shows,
    /// and beside the rest again once there's room.
    pub fn show_pane(&self) {
        let mut p = self.0.borrow_mut();
        p.front = Front::Pane;
        p.collapsed = false;
    }

    /// Laid out last one at a time, too narrow for both: what's beside
    /// the pane has a way back to it.
    pub fn alone(&self) -> bool {
        self.0.borrow().alone
    }

    /// The pane shows, as laid out last (a menu's Show or Hide).
    pub fn showing(&self) -> bool {
        let p = self.0.borrow();
        if p.alone { p.front == Front::Pane } else { !p.collapsed }
    }

    /// Its divider is being dragged.
    pub fn dragging(&self) -> bool {
        self.0.borrow().drag.is_some()
    }

    /// What shows in a window `total` wide whose rest needs at least
    /// `rest`: side by side while the pane's narrowest fits beside it
    /// (the pane its width, no wider than leaves `rest`; whole points),
    /// the rest alone while it's put away; too narrow for both, one at a
    /// time, whichever is in front.
    pub fn panes(&self, total: f32, rest: f32) -> Panes {
        let mut p = self.0.borrow_mut();
        p.alone = total - rest < p.min;
        if !p.alone {
            return if p.collapsed { Panes::Rest } else { Panes::Both(p.width.min(total - rest).max(p.min).round()) };
        }
        match p.front {
            Front::Pane => Panes::Pane,
            Front::Rest => Panes::Rest,
        }
    }

    /// How wide it shows in a window `total` wide that keeps at least
    /// `rest` for what's beside it: 0 when away; else its width, no
    /// narrower than `min`, no wider than leaves `rest` (or squeezing
    /// it, too narrow for both: [`Self::panes`] shows them one at a time
    /// instead). Whole points.
    pub fn shown(&self, total: f32, rest: f32) -> f32 {
        let p = self.0.borrow();
        if p.collapsed {
            return 0.0;
        }
        p.width.min(total - rest).max(p.min).round()
    }

    fn press(&self, x: f32, shown: f32) {
        self.0.borrow_mut().drag = Some((x, shown));
    }

    /// The divider dragged to `x`: true if what shows changed. Past half
    /// its narrowest, it goes; out from away by half that, it comes back.
    fn drag_to(&self, x: f32) -> bool {
        let mut p = self.0.borrow_mut();
        let Some((from, shown)) = p.drag else { return false };
        let want = shown + (x - from);
        let was = (p.width, p.collapsed);
        if want < p.min * 0.5 {
            p.collapsed = true;
        } else {
            p.collapsed = false;
            p.width = want.max(p.min);
        }
        (p.width, p.collapsed) != was
    }

    fn release(&self) {
        self.0.borrow_mut().drag = None;
    }
}

/// `pane`'s divider: a grip at `x` (the pane's edge, its shown width: 0
/// when away, a grip at the window's edge to drag it out), from `top`,
/// `height` tall. Dragged, it sizes the pane (and puts it away, or brings
/// it back); a double click puts it away or brings it back; `on_change`
/// is told each time what shows changed (the view lays out again:
/// `cx.msg(..)`). Agents read it as a Splitter named `name`.
pub fn side_divider(pane: &SidePane, x: f32, top: f32, height: f32, name: &'static str, on_change: impl Fn(&(), &mut WindowCx) + 'static) -> Element {
    let left = x.max(0.0);
    let on_change = Rc::new(on_change);
    let (said, down, moved, up) = (pane.clone(), pane.clone(), pane.clone(), pane.clone());
    let (pressed, dragged) = (on_change.clone(), on_change);
    let shown = x;
    div().absolute().left(px(left)).top(px(top)).w(px(GRIP)).h(px(height.max(0.0))).child(
        // (Nothing drawn: the panes' edges are the divider.)
        canvas(move |g, b| {
            let (collapsed, width) = (said.collapsed(), said.width());
            g.agent(agent::Node {
                role: "Splitter",
                name: Some(name.into()),
                value: Some(if collapsed { "hidden".into() } else { format!("{width:.0}").into() }),
                bounds: b,
                enabled: true,
                clickable: true,
                ..Default::default()
            });
        })
        .size_full()
        .cursor(CursorStyle::ResizeLeftRight)
        .on_mouse_down(MouseButton::Left, move |e, cx| {
            if e.click_count >= 2 {
                down.toggle();
                down.release();
                pressed(&(), cx);
            } else {
                down.press(f32::from(e.position.x), shown);
            }
            cx.stop_propagation();
        })
        .on_drag_move(move |e, cx| {
            if !moved.dragging() {
                return;
            }
            if e.pressed_button != Some(MouseButton::Left) {
                moved.release();
                return;
            }
            if moved.drag_to(f32::from(e.position.x)) {
                dragged(&(), cx);
            }
        })
        .on_window_mouse_up(move |e, _| {
            if e.button == MouseButton::Left && up.dragging() {
                up.release();
            }
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dragged narrower it stops at its narrowest, past half that it goes;
    /// from away, dragged out, it comes back; a toggle does the same.
    #[test]
    fn a_side_pane_sizes_goes_and_comes_back() {
        let p = SidePane::new(300.0, 220.0);
        assert_eq!(p.shown(1000.0, 320.0), 300.0);
        // Narrower, to its narrowest.
        p.press(300.0, 300.0);
        assert!(p.drag_to(250.0));
        assert_eq!(p.shown(1000.0, 320.0), 250.0);
        p.drag_to(150.0);
        assert_eq!((p.shown(1000.0, 320.0), p.collapsed()), (220.0, false));
        // Past half its narrowest: away, and its width kept.
        p.drag_to(100.0);
        assert_eq!((p.shown(1000.0, 320.0), p.collapsed(), p.width()), (0.0, true, 220.0));
        p.release();
        // Out from the edge.
        p.press(0.0, 0.0);
        assert!(!p.drag_to(60.0));
        assert!(p.drag_to(140.0));
        assert_eq!((p.collapsed(), p.shown(1000.0, 320.0)), (false, 220.0));
        p.drag_to(360.0);
        assert_eq!(p.shown(1000.0, 320.0), 360.0);
        p.release();
        // Never more than leaves the rest its room.
        assert_eq!(p.shown(600.0, 320.0), 280.0);
        // Hide Sidebar, and back as it was.
        p.toggle();
        assert_eq!(p.shown(1000.0, 320.0), 0.0);
        p.toggle();
        assert_eq!(p.shown(1000.0, 320.0), 360.0);
        // Kept: as it was left.
        let q = SidePane::new(300.0, 220.0).restored(Some(180.0), true);
        assert_eq!((q.width(), q.collapsed()), (220.0, true));
    }

    /// Too narrow for both, one at a time, never squeezed: a list first,
    /// what it opened once something is, back to the list; a pane that
    /// gives way, what's beside it first, brought in front and taken
    /// away by View ▸ Show and Hide.
    #[test]
    fn a_side_pane_too_narrow_shows_one_at_a_time() {
        let p = SidePane::new(300.0, 220.0);
        assert_eq!(p.panes(1000.0, 360.0), Panes::Both(300.0));
        assert!(!p.alone() && p.showing());
        // A third of the smallest screen: 407 points across.
        assert_eq!(p.panes(407.0, 360.0), Panes::Pane);
        assert!(p.alone() && p.showing());
        p.show_rest();
        assert_eq!(p.panes(407.0, 360.0), Panes::Rest);
        assert!(!p.showing());
        // Wider again: side by side, whichever was in front.
        assert_eq!(p.panes(1000.0, 360.0), Panes::Both(300.0));
        // The back arrow: the list.
        p.panes(407.0, 360.0);
        p.show_pane();
        assert_eq!(p.panes(407.0, 360.0), Panes::Pane);
        // View ▸ Hide, one at a time: what's beside it, the list not put away.
        p.toggle();
        assert_eq!((p.panes(407.0, 360.0), p.collapsed()), (Panes::Rest, false));
        // Put away side by side, then too narrow: whichever is in front;
        // brought in front, it's beside the rest again once there's room.
        let q = SidePane::new(300.0, 220.0).restored(None, true);
        assert_eq!(q.panes(1000.0, 360.0), Panes::Rest);
        q.panes(407.0, 360.0);
        q.show_pane();
        assert_eq!((q.panes(407.0, 360.0), q.panes(1000.0, 360.0)), (Panes::Pane, Panes::Both(300.0)));
        // A sidebar that gives way: what's beside it first.
        let s = SidePane::new(180.0, 180.0).first(Front::Rest);
        assert_eq!(s.panes(600.0, 440.0), Panes::Rest);
        assert!(!s.showing());
        assert_eq!(s.panes(800.0, 440.0), Panes::Both(180.0));
        s.panes(600.0, 440.0);
        s.toggle();
        assert_eq!(s.panes(600.0, 440.0), Panes::Pane);
        s.toggle();
        assert_eq!(s.panes(600.0, 440.0), Panes::Rest);
    }
}
