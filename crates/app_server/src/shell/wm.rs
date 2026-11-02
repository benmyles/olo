//! BeOS window-management behaviour: what clicking and dragging the tab and
//! frame does, zoom, minimize and send-to-back.
//!
//! Double-clicking the tab rolls the window up; double-clicking its top
//! or bottom edge zooms it vertically, its left or right edge
//! horizontally (`toggle_zoom_along`); the corners only resize.

use std::{cell::RefCell, time::Instant};

use smithay::{
    desktop::{WindowSurface, layer_map_for_output, space::SpaceElement},
    input::{
        Seat,
        pointer::{
            AxisFrame, ButtonEvent, CursorIcon, CursorImageStatus, Focus, GestureHoldBeginEvent,
            GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
            GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
            GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab, PointerInnerHandle,
            RelativeMotionEvent,
        },
    },
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{Logical, Point, Rectangle, Serial},
};

use super::{
    ResizeEdge, WindowElement,
    deco::{DOUBLE_CLICK, Part},
    tiling::Axis,
};
use crate::{
    focus::{KeyboardFocusTarget, PointerFocusTarget},
    state::{AnvilState, Backend},
};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

/// Space left between a zoomed window and Deskbar.
const DESKBAR_GAP: i32 = 4;

/// The element rectangle a window had before it was zoomed or tiled.
#[derive(Default)]
pub(super) struct PreZoom(pub(super) RefCell<Option<Rectangle<i32, Logical>>>);

/// The element rectangle a tiled (or arranged) window was zoomed from, to
/// go back to when it is unzoomed (its own frame stays in [`PreZoom`]).
#[derive(Default)]
pub(super) struct ZoomedFrom(pub(super) RefCell<Option<Rectangle<i32, Logical>>>);

/// A press on `part` of a window's tab or frame: true if it is the
/// second of a double-click there.
fn double_click(window: &WindowElement, part: Part) -> bool {
    let mut deco = window.decoration();
    let (now, at) = (Instant::now(), deco.pointer);
    let prev = deco.last_press.replace((now, at, part));
    let double = prev.is_some_and(|(t, p, was)| was == part && now.duration_since(t) < DOUBLE_CLICK && (p - at).x.abs() + (p - at).y.abs() < 6.0);
    if double {
        deco.last_press = None;
    }
    double
}

fn cursor_for(part: Part) -> CursorIcon {
    match part {
        Part::Frame(e) if e == ResizeEdge::TOP_LEFT => CursorIcon::NwResize,
        Part::Frame(e) if e == ResizeEdge::TOP_RIGHT => CursorIcon::NeResize,
        Part::Frame(e) if e == ResizeEdge::BOTTOM_LEFT => CursorIcon::SwResize,
        Part::Frame(e) if e == ResizeEdge::BOTTOM_RIGHT => CursorIcon::SeResize,
        Part::Frame(e) if e == ResizeEdge::TOP => CursorIcon::NResize,
        Part::Frame(e) if e == ResizeEdge::BOTTOM => CursorIcon::SResize,
        Part::Frame(e) if e == ResizeEdge::LEFT => CursorIcon::WResize,
        Part::Frame(e) if e == ResizeEdge::RIGHT => CursorIcon::EResize,
        _ => CursorIcon::Default,
    }
}

impl<BackendData: Backend> AnvilState<BackendData> {
    pub fn deco_motion(&mut self, window: &WindowElement, location: Point<f64, Logical>) {
        let (title, content) = (window.title(), window.content_size());
        let mut deco = window.decoration();
        let part = deco.hit(&title, content, location);
        deco.hover = part;
        deco.pointer = location;
        let cut = if part == Part::Tab { deco.cut_title(&title, content) } else { None };
        drop(deco);
        self.cursor_status = CursorImageStatus::Named(cursor_for(part));
        // On a title the tab cuts short: its tip. (Where it is on screen
        // is worked out when it shows: this is inside the pointer's
        // motion, which holds its lock.)
        self.tab_tip_hover(window, title, cut.filter(|r| r.to_f64().contains(location)));
    }

    pub fn deco_leave(&mut self, window: &WindowElement) {
        self.tab_tip_hover(window, String::new(), None);
        let mut deco = window.decoration();
        deco.hover = Part::None;
        deco.pressed = Part::None;
        drop(deco);
        self.cursor_status = CursorImageStatus::default_named();
    }

    pub fn deco_button(
        &mut self,
        seat: &Seat<Self>,
        window: &WindowElement,
        button: u32,
        pressed: bool,
        serial: Serial,
    ) {
        let hover = window.decoration().hover;
        if pressed {
            self.tab_tip_pressed();
        }
        if !pressed {
            let was = std::mem::take(&mut window.decoration().pressed);
            if was == hover {
                match was {
                    Part::Close => {
                        tracing::info!(id = window.id(), "close box clicked");
                        self.close_window(window);
                    }
                    Part::Zoom => self.toggle_zoom(window),
                    _ => {}
                }
            }
            return;
        }

        match (button, hover) {
            (BTN_LEFT, Part::Close | Part::Zoom) => window.decoration().pressed = hover,
            (BTN_RIGHT, Part::Tab) => self.window_menu(window),
            (BTN_LEFT, Part::Tab) => {
                if double_click(window, hover) {
                    self.toggle_rolled(window);
                    return;
                }
                let shift = seat
                    .get_keyboard()
                    .map(|k| k.modifier_state().shift)
                    .unwrap_or(false);
                // ⌘-drag: along its group's tabs, out of the group, or into
                // another (shell/tabdrag.rs).
                if self.command_held() && self.groupable(window) {
                    self.start_tab_drag(window, serial);
                } else if shift {
                    self.start_tab_slide(window, serial);
                } else {
                    self.start_move(seat, window, serial);
                }
            }
            (BTN_LEFT, Part::Frame(edges)) => {
                let axis = match edges {
                    e if e == ResizeEdge::TOP || e == ResizeEdge::BOTTOM => Some(Axis::Vertical),
                    e if e == ResizeEdge::LEFT || e == ResizeEdge::RIGHT => Some(Axis::Horizontal),
                    _ => None,
                };
                if let Some(axis) = axis
                    && double_click(window, hover)
                {
                    self.toggle_zoom_along(window, axis);
                    return;
                }
                self.start_resize(seat, window, serial, edges);
            }
            _ => {}
        }
    }

    fn start_move(&mut self, seat: &Seat<Self>, window: &WindowElement, serial: Serial) {
        match window.0.underlying_surface() {
            WindowSurface::Wayland(w) => {
                let seat = seat.clone();
                let toplevel = w.clone();
                self.handle
                    .insert_idle(move |data| data.move_request_xdg(&toplevel, &seat, serial));
            }
        }
    }

    fn start_resize(&mut self, seat: &Seat<Self>, window: &WindowElement, serial: Serial, edges: ResizeEdge) {
        match window.0.underlying_surface() {
            WindowSurface::Wayland(w) => {
                let seat = seat.clone();
                let toplevel = w.clone();
                self.handle.insert_idle(move |data| {
                    data.resize_request_xdg(&toplevel, &seat, serial, edges)
                });
            }
        }
    }

    fn start_tab_slide(&mut self, window: &WindowElement, serial: Serial) {
        // Deferred like moves: we are inside the pointer's button handler.
        let window = window.clone();
        self.handle.insert_idle(move |data| {
            let Some(start_data) = data.pointer.grab_start_data() else {
                return;
            };
            let initial_offset = window.decoration().tab_offset;
            let grab = TabSlideGrab { start_data, window, initial_offset };
            let pointer = data.pointer.clone();
            pointer.set_grab(data, grab, serial, Focus::Keep);
        });
    }

    pub fn close_window(&mut self, window: &WindowElement) {
        window.meta().closed_by_hand.set(true);
        match window.0.underlying_surface() {
            WindowSurface::Wayland(w) => w.send_close(),
        }
    }

    /// A right-click on the tab: the window's menu (Hide, Zoom, tiling,
    /// workspaces, Send to Back, Close), shown by the command palette,
    /// which stays running, at the pointer on its display.
    pub fn window_menu(&mut self, window: &WindowElement) {
        // Not the pointer's own location: this runs inside its grab.
        let Some(loc) = self.space.element_location(window) else { return };
        let at = loc.to_f64() + window.decoration().pointer;
        let Some(output) = self.space.output_under(at).next().cloned() else { return };
        let Some(geo) = self.space.output_geometry(&output) else { return };
        let (x, y) = ((at.x - geo.loc.x as f64).round(), (at.y - geo.loc.y as f64).round());
        let (id, x, y) = (window.id().to_string(), x.to_string(), y.to_string());
        self.spawn_program("olo-palette", &["--window-menu", &id, &x, &y, &output.name()]);
    }

    /// Push the window behind all others (from its menu, as BeOS's
    /// right-click on the tab did).
    pub fn send_to_back(&mut self, window: &WindowElement) {
        self.space.lower_element(window);
        // Focus follows the new front-most window.
        let front = self.space.elements().next_back().cloned();
        if let Some(front) = front
            && &front != window
        {
            self.focus_window(&front);
        }
    }

    pub fn focus_window(&mut self, window: &WindowElement) {
        // A window of a client going away takes nothing (`live`).
        if !live(window) {
            return;
        }
        self.space.raise_element(window, true);
        self.keep_children_above();
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        let keyboard = self.seat.get_keyboard().unwrap();
        // A menu's popup grab may still be live (the menu that opened this
        // window is being torn down); left alone it would hand focus back
        // to the menu's window on the next key press.
        if keyboard.is_grabbed() {
            keyboard.unset_grab(self);
        }
        keyboard.set_focus(self, Some(KeyboardFocusTarget::from(window.clone())), serial);
    }

    /// Each window that names a parent (a dialog, Tracker's Quick Look)
    /// in front of it, as `xdg_toplevel.set_parent` asks, like BeOS's
    /// floating and modal feels: whatever raised the parent, its children
    /// come back over it, in their order, and theirs over them. An alert
    /// that names none is over all its app's windows (BAlert's
    /// app-modal feel). Done wherever a window is raised, and once a turn
    /// for the rest.
    pub fn keep_children_above(&mut self) {
        use smithay::reexports::wayland_server::Resource;
        let client = |w: &WindowElement| w.wl_surface().and_then(|s| s.client()).map(|c| c.id());
        // Space only raises to the top: a child under its parent goes
        // there, which is just over the parent (only just raised itself).
        for _ in 0..=self.space.elements().count() {
            let order: Vec<WindowElement> = self.space.elements().cloned().collect();
            let under = order.iter().enumerate().find(|(i, w)| {
                let Some(parent) = w.0.toplevel().and_then(|t| t.parent()) else {
                    return crate::shell::groups::is_parentless_alert(w)
                        && client(w).is_some_and(|c| order[i + 1..].iter().any(|o| client(o) == Some(c.clone()) && !crate::shell::groups::is_parentless_alert(o)));
                };
                order[i + 1..].iter().any(|p| p.wl_surface().as_deref() == Some(&parent))
            });
            let Some((_, child)) = under else { break };
            self.space.raise_element(&child.clone(), false);
        }
        self.keep_pips_above();
    }

    /// Roll a window up to just its tab, or back down.
    pub fn toggle_rolled(&mut self, window: &WindowElement) {
        if !window.decorated() {
            return;
        }
        let rolled = !window.decoration().rolled;
        // A group rolls up together (its tabs stay side by side).
        let mut together = self.groups.mates(window);
        if together.is_empty() {
            together.push(window.clone());
        }
        for w in &together {
            w.decoration().rolled = rolled;
        }
        tracing::info!(id = window.id(), rolled, "rolled");
    }

    pub fn minimize_window(&mut self, window: &WindowElement) {
        self.minimize(window);
    }

    pub fn restore_window(&mut self, window: &WindowElement) {
        self.restore(window);
    }

    /// Where a window zooms to on the display it is (mostly) on: the
    /// tiling area, inside the margins (`Settings::tiling`).
    pub fn zoom_area(&self, window: &WindowElement) -> Option<Rectangle<i32, Logical>> {
        self.tile_area(&self.window_output(window)?)
    }

    /// Where `window` is, or is on its way to: its frame, but at the size
    /// it was asked while its app hasn't drawn that yet. (Moved and sized
    /// at once, it is at its new place straight away and at its new size
    /// only when its app draws it, a frame or more later: in between, its
    /// old size at the new place can be mostly on another display.)
    pub fn heading_frame(&self, window: &WindowElement) -> Option<Rectangle<i32, Logical>> {
        let frame = self.space.element_geometry(window)?;
        let WindowSurface::Wayland(t) = window.0.underlying_surface();
        Some(match super::heading_size(t) {
            Some(size) => Rectangle::new(frame.loc, window.outer_size(size)),
            None => frame,
        })
    }

    /// The display a window is on (or on its way to, `heading_frame`):
    /// the one under the middle of its frame, else the one it overlaps
    /// most, else the main one.
    pub fn window_output(&self, window: &WindowElement) -> Option<smithay::output::Output> {
        let frame = self.heading_frame(window);
        if let Some(f) = frame {
            let middle = (f.loc + f.size.downscale(2).to_point()).to_f64();
            if let Some(o) = self.space.output_under(middle).next() {
                return Some(o.clone());
            }
        }
        let overlap = |o: &smithay::output::Output| {
            let (Some(f), Some(g)) = (frame, self.space.output_geometry(o)) else { return 0 };
            f.intersection(g).map_or(0, |r| r.size.w as i64 * r.size.h as i64)
        };
        self.space.outputs().filter(|o| overlap(o) > 0).max_by_key(|o| overlap(o)).cloned().or_else(|| self.main_output())
    }

    /// Where windows go on a display: all of it but Deskbar and panels.
    pub fn usable_area(&self, output: &smithay::output::Output) -> Option<Rectangle<i32, Logical>> {
        let geo = self.space.output_geometry(output)?;
        let map = layer_map_for_output(output);
        let zone = map.non_exclusive_zone();
        let mut area = Rectangle::new(geo.loc + zone.loc, zone.size);
        // Stop beside Deskbar, as BeOS does (it sits in a corner and
        // reserves no edge, so the zone above doesn't leave it out).
        // (One that is there: drawn, so with a size.)
        let bar = map.layers().filter(|l| l.namespace() == "deskbar").filter_map(|l| map.layer_geometry(l)).find(|g| g.size.w > 0 && g.size.h > 0);
        if let Some(bar) = bar.map(|b| Rectangle::new(geo.loc + b.loc, b.size)) {
            if bar.loc.x + bar.size.w / 2 >= area.loc.x + area.size.w / 2 {
                let right = bar.loc.x - DESKBAR_GAP;
                area.size.w = area.size.w.min(right - area.loc.x);
            } else {
                let left = bar.loc.x + bar.size.w + DESKBAR_GAP;
                area.size.w -= (left - area.loc.x).max(0);
                area.loc.x = area.loc.x.max(left);
            }
        }
        Some(area)
    }

    /// Zoom (maximize) a window into the tiling area, frame included.
    pub fn zoom_window(&mut self, window: &WindowElement) {
        let Some(area) = self.zoom_area(window) else {
            return;
        };
        if let Some(location) = self.space.element_location(window)
            && !self.is_zoomed(window)
        {
            let here = Rectangle::new(location, window.geometry().size);
            window.user_data().insert_if_missing(PreZoom::default);
            window.user_data().insert_if_missing(ZoomedFrom::default);
            let pre = window.user_data().get::<PreZoom>().unwrap();
            // Tiled or arranged (its own frame already kept): come back
            // here, not to its own frame.
            let arranged = pre.0.borrow().is_some();
            *window.user_data().get::<ZoomedFrom>().unwrap().0.borrow_mut() = arranged.then_some(here);
            pre.0.borrow_mut().get_or_insert(here);
        }
        let content = window.inner_size(area.size);
        match window.0.underlying_surface() {
            WindowSurface::Wayland(t) => {
                t.with_pending_state(|state| {
                    state.states.set(xdg_toplevel::State::Maximized);
                    state.size = Some(content);
                });
                if t.is_initial_configure_sent() {
                    t.send_pending_configure();
                }
            }
        }
        // Moved, not mapped again: mapping raises and activates, and a
        // zoom keeps its place in the stack (Zoom All, a refit when the
        // margins change, a client's own maximize).
        if self.space.element_location(window).is_some() {
            self.space.relocate_element(window, area.loc);
        } else {
            self.space.map_element(window.clone(), area.loc, true);
        }
    }

    pub fn unzoom_window(&mut self, window: &WindowElement) {
        // Zoomed from a tile, or a place Tile All gave it: back there (its
        // own frame stays remembered, for Restore).
        let from = window.user_data().get::<ZoomedFrom>().and_then(|z| z.0.borrow_mut().take());
        if let Some(tile) = Self::tiled(window) {
            self.retile(window, tile);
            return;
        }
        // (What it remembers moves with its display; in reach, whatever
        // happened to the displays meanwhile.)
        if let Some(frame) = from {
            let frame = self.put_back_at(frame);
            self.set_frame(window, frame, false);
            return;
        }
        let pre = window
            .user_data()
            .get::<PreZoom>()
            .and_then(|p| p.0.borrow_mut().take())
            .map(|r| self.put_back_at(r));
        // (Worked out before taking the surface's lock: inner_size takes it
        // too, and taking it twice hangs the whole desktop.)
        let size = pre.map(|r| window.inner_size(r.size));
        match window.0.underlying_surface() {
            WindowSurface::Wayland(t) => {
                t.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Maximized);
                    state.size = size;
                });
                if t.is_initial_configure_sent() {
                    t.send_pending_configure();
                }
            }
        }
        if let Some(r) = pre {
            self.space.relocate_element(window, r.loc);
        }
    }

    pub fn is_zoomed(&self, window: &WindowElement) -> bool {
        match window.0.underlying_surface() {
            WindowSurface::Wayland(t) => t.with_pending_state(|s| s.states.contains(xdg_toplevel::State::Maximized)),
        }
    }

    pub fn toggle_zoom(&mut self, window: &WindowElement) {
        if self.is_zoomed(window) {
            self.unzoom_window(window);
        } else {
            self.zoom_window(window);
        }
    }
}

/// Shift-drag on the tab slides it along the window's top edge.
pub struct TabSlideGrab<BackendData: Backend + 'static> {
    start_data: PointerGrabStartData<AnvilState<BackendData>>,
    window: WindowElement,
    initial_offset: i32,
}

impl<BackendData: Backend> PointerGrab<AnvilState<BackendData>> for TabSlideGrab<BackendData> {
    fn motion(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        _focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        let dx = (event.location.x - self.start_data.location.x).round() as i32;
        let title = self.window.title();
        let content = self.window.content_size();
        let mut deco = self.window.decoration();
        let frame_w = content.w + 2 * olo_theme::deco::BORDER;
        let tab_w = deco.tab_layout(&title, content).width;
        deco.tab_offset = (self.initial_offset + dx).clamp(0, (frame_w - tab_w).max(0));
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

    fn button(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        details: AxisFrame,
    ) {
        handle.axis(data, details)
    }

    fn frame(&mut self, data: &mut AnvilState<BackendData>, handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event)
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event)
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event)
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event)
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event)
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event)
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event)
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut AnvilState<BackendData>,
        handle: &mut PointerInnerHandle<'_, AnvilState<BackendData>>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event)
    }

    fn start_data(&self) -> &PointerGrabStartData<AnvilState<BackendData>> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut AnvilState<BackendData>) {}
}

/// Whether `window`'s client is still there. A client that disconnects
/// has its windows destroyed one by one; while that goes on its other
/// windows are still in the space, and an event naming one (the keyboard
/// entering it, as the next window down takes focus) names an object of
/// a client that's gone, which wayland-backend aborts the process for
/// (it happens in a destructor, which can't unwind): killing an app with
/// two windows took app_server down.
pub fn live(window: &WindowElement) -> bool {
    use smithay::reexports::wayland_server::Resource;
    window.wl_surface().is_some_and(|s| s.is_alive() && s.client().is_some())
}
