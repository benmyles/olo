//! BeOS window-management behaviour: what clicking and dragging the tab and
//! frame does, zoom, minimize and send-to-back.

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
};
use crate::{
    focus::{KeyboardFocusTarget, PointerFocusTarget},
    state::{AnvilState, Backend},
};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

/// The element rectangle a window had before it was zoomed.
#[derive(Default)]
struct PreZoom(RefCell<Option<Rectangle<i32, Logical>>>);

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
    fn deco_part(&self, window: &WindowElement, location: Point<f64, Logical>) -> Part {
        let title = window.title();
        let content = window.content_size();
        window.decoration().hit(&title, content, location)
    }

    pub fn deco_motion(&mut self, window: &WindowElement, location: Point<f64, Logical>) {
        let part = self.deco_part(window, location);
        let mut deco = window.decoration();
        deco.hover = part;
        deco.pointer = location;
        drop(deco);
        self.cursor_status = CursorImageStatus::Named(cursor_for(part));
    }

    pub fn deco_leave(&mut self, window: &WindowElement) {
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
        if !pressed {
            let was = std::mem::take(&mut window.decoration().pressed);
            if was == hover {
                match was {
                    Part::Close => self.close_window(window),
                    Part::Zoom => self.toggle_zoom(window),
                    _ => {}
                }
            }
            return;
        }

        match (button, hover) {
            (BTN_LEFT, Part::Close | Part::Zoom) => window.decoration().pressed = hover,
            (BTN_RIGHT, Part::Tab) => self.send_to_back(window),
            (BTN_LEFT, Part::Tab) => {
                let location = window.decoration().pointer;
                let now = Instant::now();
                let double = {
                    let mut deco = window.decoration();
                    let prev = deco.last_tab_press.replace((now, location));
                    prev.is_some_and(|(t, p)| {
                        now.duration_since(t) < DOUBLE_CLICK && (p - location).x.abs() + (p - location).y.abs() < 6.0
                    })
                };
                if double {
                    window.decoration().last_tab_press = None;
                    self.minimize_window(window);
                    return;
                }
                let shift = seat
                    .get_keyboard()
                    .map(|k| k.modifier_state().shift)
                    .unwrap_or(false);
                if shift {
                    self.start_tab_slide(window, serial);
                } else {
                    self.start_move(seat, window, serial);
                }
            }
            (BTN_LEFT, Part::Frame(edges)) => self.start_resize(seat, window, serial, edges),
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
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(w) => {
                let w = w.clone();
                self.handle.insert_idle(move |data| data.move_request_x11(&w));
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
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(w) => {
                let w = w.clone();
                self.handle
                    .insert_idle(move |data| data.resize_request_x11(&w, edges));
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
        match window.0.underlying_surface() {
            WindowSurface::Wayland(w) => w.send_close(),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(w) => {
                let _ = w.close();
            }
        }
    }

    /// BeOS right-click on the tab: push the window behind all others.
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
        self.space.raise_element(window, true);
        #[cfg(feature = "xwayland")]
        if let Some(surface) = window.0.x11_surface()
            && let Some(xwm) = self.xwm.as_mut()
        {
            let _ = xwm.raise_window(surface);
        }
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

    pub fn minimize_window(&mut self, window: &WindowElement) {
        self.minimize(window);
    }

    pub fn restore_window(&mut self, window: &WindowElement) {
        self.restore(window);
    }

    /// Usable area for windows on the output the window is (mostly) on.
    pub fn zoom_area(&self, window: &WindowElement) -> Option<Rectangle<i32, Logical>> {
        let output = self
            .space
            .outputs_for_element(window)
            .into_iter()
            .next()
            .or_else(|| self.space.outputs().next().cloned())?;
        let geo = self.space.output_geometry(&output)?;
        let zone = layer_map_for_output(&output).non_exclusive_zone();
        Some(Rectangle::new(geo.loc + zone.loc, zone.size))
    }

    /// Zoom (maximize) a window into the usable area, frame included.
    pub fn zoom_window(&mut self, window: &WindowElement) {
        let Some(area) = self.zoom_area(window) else {
            return;
        };
        if let Some(location) = self.space.element_location(window) {
            window.user_data().insert_if_missing(PreZoom::default);
            let pre = window.user_data().get::<PreZoom>().unwrap();
            pre.0
                .borrow_mut()
                .get_or_insert(Rectangle::new(location, window.geometry().size));
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
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => {
                let _ = x.set_maximized(true);
                let _ = x.configure(Rectangle::new(area.loc + window.content_offset(), content));
            }
        }
        self.space.map_element(window.clone(), area.loc, true);
    }

    pub fn unzoom_window(&mut self, window: &WindowElement) {
        let pre = window
            .user_data()
            .get::<PreZoom>()
            .and_then(|p| p.0.borrow_mut().take());
        match window.0.underlying_surface() {
            WindowSurface::Wayland(t) => {
                t.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Maximized);
                    state.size = pre.map(|r| window.inner_size(r.size));
                });
                if t.is_initial_configure_sent() {
                    t.send_pending_configure();
                }
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => {
                let _ = x.set_maximized(false);
                if let Some(r) = pre {
                    let _ = x.configure(Rectangle::new(r.loc + window.content_offset(), window.inner_size(r.size)));
                }
            }
        }
        if let Some(r) = pre {
            self.space.map_element(window.clone(), r.loc, true);
        }
    }

    pub fn is_zoomed(&self, window: &WindowElement) -> bool {
        match window.0.underlying_surface() {
            WindowSurface::Wayland(t) => t.with_pending_state(|s| s.states.contains(xdg_toplevel::State::Maximized)),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => x.is_maximized(),
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
        let frame_w = content.w + 2 * tabula_theme::deco::BORDER;
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
