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
//! Mac; give the item `.key("s")` and the profile's keys take its place). Its width and whether it's away are the app's to keep across
//! restarts and upgrades ([`SidePane::width`], [`SidePane::collapsed`]).
//!
//! ```ignore
//! let pane = cx.new(|_| SidePane::new(300.0, 220.0));
//! cx.observe(&pane, |_, _, cx| cx.notify()).detach();
//! // render: the pane's own width (0 when away), the divider at its edge
//! let w = pane.read(cx).shown(total_width, 320.0);
//! layer.child(side_divider(&pane, w, top, height, "Chat List"))
//! ```

use gpui::{
    App, Bounds, CursorStyle, Entity, HitboxBehavior, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Styled as _, Window, canvas, div, px,
};

use crate::agent;

/// The divider's grip, points across: from the pane's edge over what's
/// beside it (the pane's own scroll bar runs up to its edge).
pub const GRIP: f32 = 6.0;

/// A side pane's size and whether it's put away.
#[derive(Debug, Clone, PartialEq)]
pub struct SidePane {
    /// Its width when shown (at least `min`).
    width: f32,
    collapsed: bool,
    min: f32,
    /// The divider being dragged: where it was pressed, and the width
    /// shown then (0: away).
    drag: Option<(f32, f32)>,
}

impl SidePane {
    /// `width` wide, never narrower than `min` while shown.
    pub fn new(width: f32, min: f32) -> SidePane {
        SidePane { width: width.max(min), collapsed: false, min, drag: None }
    }

    /// As kept: `width`, and put away or not.
    pub fn restored(mut self, width: Option<f32>, collapsed: bool) -> SidePane {
        if let Some(w) = width {
            self.width = w.max(self.min);
        }
        self.collapsed = collapsed;
        self
    }

    /// Its width when shown.
    pub fn width(&self) -> f32 {
        self.width
    }

    /// Put away.
    pub fn collapsed(&self) -> bool {
        self.collapsed
    }

    pub fn set_collapsed(&mut self, collapsed: bool) {
        self.collapsed = collapsed;
    }

    /// Put away, or brought back.
    pub fn toggle(&mut self) {
        self.collapsed = !self.collapsed;
    }

    /// Its divider is being dragged.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// How wide it shows in a window `total` wide that keeps at least
    /// `rest` for what's beside it: 0 when away; else its width, no
    /// narrower than `min`, no wider than leaves `rest`. Whole points.
    pub fn shown(&self, total: f32, rest: f32) -> f32 {
        if self.collapsed {
            return 0.0;
        }
        self.width.min(total - rest).max(self.min).round()
    }

    fn press(&mut self, x: f32, shown: f32) {
        self.drag = Some((x, shown));
    }

    /// The divider dragged to `x`: true if what shows changed. Past half
    /// its narrowest, it goes; out from away by half that, it comes back.
    fn drag_to(&mut self, x: f32) -> bool {
        let Some((from, shown)) = self.drag else { return false };
        let want = shown + (x - from);
        let was = (self.width, self.collapsed);
        if want < self.min * 0.5 {
            self.collapsed = true;
        } else {
            self.collapsed = false;
            self.width = want.max(self.min);
        }
        (self.width, self.collapsed) != was
    }

    fn release(&mut self) {
        self.drag = None;
    }
}

/// `pane`'s divider: a grip at `x` (the pane's edge, its shown width: 0
/// when away, a grip at the window's edge to drag it out), from `top`,
/// `height` tall. Dragged, it sizes the pane (and puts it away, or brings
/// it back); a double click puts it away or brings it back. Agents read
/// it as a Splitter named `name`.
pub fn side_divider(pane: &Entity<SidePane>, x: f32, top: f32, height: f32, name: &'static str) -> impl IntoElement {
    let pane = pane.clone();
    let left = x.max(0.0);
    div().absolute().left(px(left)).top(px(top)).w(px(GRIP)).h(px(height.max(0.0))).child(
        canvas(
            |b, window, _| window.insert_hitbox(b, HitboxBehavior::Normal),
            move |b: Bounds<Pixels>, hitbox, window: &mut Window, cx: &mut App| {
                window.set_cursor_style(CursorStyle::ResizeLeftRight, &hitbox);
                let (collapsed, width) = {
                    let p = pane.read(cx);
                    (p.collapsed, p.width)
                };
                agent::record(
                    window,
                    cx,
                    agent::Node {
                        role: "Splitter",
                        name: Some(name.into()),
                        value: Some(if collapsed { "hidden".into() } else { format!("{width:.0}").into() }),
                        bounds: b,
                        enabled: true,
                        clickable: true,
                        ..Default::default()
                    },
                );
                let (down, moved, up) = (pane.clone(), pane.clone(), pane.clone());
                let shown = x;
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble() || e.button != MouseButton::Left || !hitbox.is_hovered(window) {
                        return;
                    }
                    down.update(cx, |p, cx| {
                        if e.click_count >= 2 {
                            p.toggle();
                            p.release();
                        } else {
                            p.press(f32::from(e.position.x), shown);
                        }
                        cx.notify();
                    });
                    cx.stop_propagation();
                });
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if !phase.capture() || !moved.read(cx).dragging() {
                        return;
                    }
                    if e.pressed_button != Some(MouseButton::Left) {
                        moved.update(cx, |p, _| p.release());
                        return;
                    }
                    let x = f32::from(e.position.x);
                    moved.update(cx, |p, cx| {
                        if p.drag_to(x) {
                            cx.notify();
                        }
                    });
                    window.refresh();
                });
                window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                    if phase.capture() && e.button == MouseButton::Left && up.read(cx).dragging() {
                        up.update(cx, |p, _| p.release());
                    }
                });
                // (Nothing drawn: the panes' edges are the divider.)
            },
        )
        .size_full(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dragged narrower it stops at its narrowest, past half that it goes;
    /// from away, dragged out, it comes back; a toggle does the same.
    #[test]
    fn a_side_pane_sizes_goes_and_comes_back() {
        let mut p = SidePane::new(300.0, 220.0);
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
}
