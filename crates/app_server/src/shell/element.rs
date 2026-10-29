use std::{borrow::Cow, time::Duration};

use smithay::{
    backend::{
        input::InputTime,
        renderer::{
            ImportAll, ImportMem, Renderer, Texture,
            element::{
                AsRenderElements, memory::MemoryRenderBufferRenderElement, solid::SolidColorRenderElement,
                surface::WaylandSurfaceRenderElement,
            },
        },
    },
    desktop::{
        Window, WindowSurface, WindowSurfaceType, space::SpaceElement, utils::OutputPresentationFeedback,
    },
    backend::input::ButtonState,
    input::{
        Seat,
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
            GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
            GestureSwipeUpdateEvent, MotionEvent, PointerTarget, RelativeMotionEvent,
        },
        tablet::tool::TabletToolTarget,
        touch::{FrameMarker, TouchTarget},
    },
    output::Output,
    reexports::{
        wayland_protocols::{
            wp::presentation_time::server::wp_presentation_feedback, xdg::shell::server::xdg_toplevel,
        },
        wayland_server::protocol::wl_surface::WlSurface,
    },
    render_elements,
    utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale, Serial, Size, user_data::UserDataMap},
    wayland::{
        compositor::{SurfaceData as WlSurfaceData, with_states},
        dmabuf::DmabufFeedback,
        seat::WaylandFocus,
        shell::xdg::XdgToplevelSurfaceData,
    },
};

use super::deco;
use crate::{AnvilState, focus::PointerFocusTarget, state::Backend};

#[derive(Debug, Clone, PartialEq)]
pub struct WindowElement(pub Window);

impl WindowElement {
    /// Server-side decorated right now (negotiated, and not fullscreen).
    pub fn decorated(&self) -> bool {
        if !self.decoration().enabled {
            return false;
        }
        match self.0.underlying_surface() {
            WindowSurface::Wayland(t) => !t.with_committed_state(|s| {
                s.is_some_and(|s| s.states.contains(xdg_toplevel::State::Fullscreen))
            }),
        }
    }

    pub fn title(&self) -> String {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(t) => with_states(t.wl_surface(), |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .and_then(|d| d.lock().unwrap().title.clone())
            })
            .unwrap_or_default(),
        }
    }

    pub fn app_id(&self) -> String {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(t) => with_states(t.wl_surface(), |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .and_then(|d| d.lock().unwrap().app_id.clone())
            })
            .unwrap_or_default(),
        }
    }

    /// The client's own geometry size (without our decoration).
    pub fn content_size(&self) -> Size<i32, Logical> {
        SpaceElement::geometry(&self.0).size
    }

    /// Offset from this element's render origin to the client's render origin.
    fn client_render_offset(&self) -> Point<i32, Logical> {
        let geo = SpaceElement::geometry(&self.0);
        if self.decorated() {
            deco::content_offset() - geo.loc
        } else {
            Point::default()
        }
    }

    /// Where the client's geometry starts relative to the element location.
    pub fn content_offset(&self) -> Point<i32, Logical> {
        if self.decorated() { deco::content_offset() } else { Point::default() }
    }

    /// Element size for a given client size.
    pub fn outer_size(&self, content: Size<i32, Logical>) -> Size<i32, Logical> {
        if self.decorated() { content + deco::extra_size() } else { content }
    }

    /// Client size for a given element size.
    pub fn inner_size(&self, outer: Size<i32, Logical>) -> Size<i32, Logical> {
        if self.decorated() {
            let extra = deco::extra_size();
            ((outer.w - extra.w).max(1), (outer.h - extra.h).max(1)).into()
        } else {
            outer
        }
    }

    /// Rolled up to just its tab.
    pub fn rolled(&self) -> bool {
        self.decorated() && self.decoration().rolled
    }

    /// Only its tab shows: rolled up, behind others in its group, or
    /// being put in a group by hand.
    fn tab_only(&self) -> bool {
        self.decorated() && {
            let d = self.decoration();
            d.rolled || d.behind || d.docked
        }
    }

    fn on_decoration(&self, location: Point<f64, Logical>) -> bool {
        if !self.decorated() {
            return false;
        }
        let title = self.title();
        let content = self.content_size();
        self.decoration().is_on_decoration(&title, content, location)
    }

    pub fn surface_under(
        &self,
        location: Point<f64, Logical>,
        window_type: WindowSurfaceType,
    ) -> Option<(PointerFocusTarget, Point<i32, Logical>)> {
        if self.on_decoration(location) {
            return Some((PointerFocusTarget::SSD(SSD(self.clone())), Point::default()));
        }
        if self.tab_only() {
            return None;
        }
        let offset = self.client_render_offset();

        let surface_under = self.0.surface_under(location - offset.to_f64(), window_type);
        let (under, loc) = match self.0.underlying_surface() {
            WindowSurface::Wayland(_) => {
                surface_under.map(|(surface, loc)| (PointerFocusTarget::WlSurface(surface), loc))
            }
        }?;
        Some((under, loc + offset))
    }

    pub fn with_surfaces<F>(&self, processor: F)
    where
        F: FnMut(&WlSurface, &WlSurfaceData),
    {
        self.0.with_surfaces(processor);
    }

    pub fn send_frame<T, F>(
        &self,
        output: &Output,
        time: T,
        throttle: Option<Duration>,
        primary_scan_out_output: F,
    ) where
        T: Into<Duration>,
        F: FnMut(&WlSurface, &WlSurfaceData) -> Option<Output> + Copy,
    {
        self.0.send_frame(output, time, throttle, primary_scan_out_output)
    }

    pub fn send_dmabuf_feedback<'a, P, F>(
        &self,
        output: &Output,
        primary_scan_out_output: P,
        select_dmabuf_feedback: F,
    ) where
        P: FnMut(&WlSurface, &WlSurfaceData) -> Option<Output> + Copy,
        F: Fn(&WlSurface, &WlSurfaceData) -> &'a DmabufFeedback + Copy,
    {
        self.0
            .send_dmabuf_feedback(output, primary_scan_out_output, select_dmabuf_feedback)
    }

    pub fn take_presentation_feedback<F1, F2>(
        &self,
        output_feedback: &mut OutputPresentationFeedback,
        primary_scan_out_output: F1,
        presentation_feedback_flags: F2,
    ) where
        F1: FnMut(&WlSurface, &WlSurfaceData) -> Option<Output> + Copy,
        F2: FnMut(&WlSurface, &WlSurfaceData) -> wp_presentation_feedback::Kind + Copy,
    {
        self.0.take_presentation_feedback(
            output_feedback,
            primary_scan_out_output,
            presentation_feedback_flags,
        )
    }


    #[inline]
    pub fn is_wayland(&self) -> bool {
        self.0.is_wayland()
    }

    #[inline]
    pub fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        self.0.wl_surface()
    }

    #[inline]
    pub fn user_data(&self) -> &UserDataMap {
        self.0.user_data()
    }
}

impl IsAlive for WindowElement {
    #[inline]
    fn alive(&self) -> bool {
        self.0.alive()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SSD(pub WindowElement);

impl IsAlive for SSD {
    #[inline]
    fn alive(&self) -> bool {
        self.0.alive()
    }
}

impl WaylandFocus for SSD {
    #[inline]
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        self.0.wl_surface()
    }
}

const BTN_LEFT: u32 = 0x110;

impl<BackendData: Backend> PointerTarget<AnvilState<BackendData>> for SSD {
    fn enter(&self, _seat: &Seat<AnvilState<BackendData>>, data: &mut AnvilState<BackendData>, event: &MotionEvent) {
        data.deco_motion(&self.0, event.location);
    }
    fn motion(&self, _seat: &Seat<AnvilState<BackendData>>, data: &mut AnvilState<BackendData>, event: &MotionEvent) {
        data.deco_motion(&self.0, event.location);
    }
    fn relative_motion(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        _data: &mut AnvilState<BackendData>,
        _event: &RelativeMotionEvent,
    ) {
    }
    fn button(&self, seat: &Seat<AnvilState<BackendData>>, data: &mut AnvilState<BackendData>, event: &ButtonEvent) {
        let pressed = event.state == ButtonState::Pressed;
        data.deco_button(seat, &self.0, event.button, pressed, event.serial);
    }
    fn axis(&self, _seat: &Seat<AnvilState<BackendData>>, _data: &mut AnvilState<BackendData>, _frame: AxisFrame) {}
    fn frame(&self, _seat: &Seat<AnvilState<BackendData>>, _data: &mut AnvilState<BackendData>) {}
    fn leave(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        _serial: Serial,
        _time: InputTime,
    ) {
        data.deco_leave(&self.0);
    }
    fn gesture_swipe_begin(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        _data: &mut AnvilState<BackendData>,
        _event: &GestureSwipeBeginEvent,
    ) {
    }
    fn gesture_swipe_update(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        _data: &mut AnvilState<BackendData>,
        _event: &GestureSwipeUpdateEvent,
    ) {
    }
    fn gesture_swipe_end(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        _data: &mut AnvilState<BackendData>,
        _event: &GestureSwipeEndEvent,
    ) {
    }
    fn gesture_pinch_begin(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        _data: &mut AnvilState<BackendData>,
        _event: &GesturePinchBeginEvent,
    ) {
    }
    fn gesture_pinch_update(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        _data: &mut AnvilState<BackendData>,
        _event: &GesturePinchUpdateEvent,
    ) {
    }
    fn gesture_pinch_end(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        _data: &mut AnvilState<BackendData>,
        _event: &GesturePinchEndEvent,
    ) {
    }
    fn gesture_hold_begin(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        _data: &mut AnvilState<BackendData>,
        _event: &GestureHoldBeginEvent,
    ) {
    }
    fn gesture_hold_end(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        _data: &mut AnvilState<BackendData>,
        _event: &GestureHoldEndEvent,
    ) {
    }
}

impl<BackendData: Backend> TouchTarget<AnvilState<BackendData>> for SSD {
    fn down(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &smithay::input::touch::DownEvent,
    ) {
        data.deco_motion(&self.0, event.location);
        data.deco_button(seat, &self.0, BTN_LEFT, true, event.serial);
    }

    fn up(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &smithay::input::touch::UpEvent,
    ) {
        data.deco_button(seat, &self.0, BTN_LEFT, false, event.serial);
    }

    fn motion(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        event: &smithay::input::touch::MotionEvent,
    ) {
        data.deco_motion(&self.0, event.location);
    }

    fn frame(&self, _seat: &Seat<AnvilState<BackendData>>, _data: &mut AnvilState<BackendData>, _marker: FrameMarker) {
    }

    fn cancel(&self, _seat: &Seat<AnvilState<BackendData>>, data: &mut AnvilState<BackendData>, _marker: FrameMarker) {
        data.deco_leave(&self.0);
    }

    fn shape(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        _data: &mut AnvilState<BackendData>,
        _event: &smithay::input::touch::ShapeEvent,
    ) {
    }

    fn orientation(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        _data: &mut AnvilState<BackendData>,
        _event: &smithay::input::touch::OrientationEvent,
    ) {
    }

    fn last_frame(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        _data: &mut AnvilState<BackendData>,
    ) -> Option<FrameMarker> {
        None
    }
}

impl<BackendData: Backend> TabletToolTarget<AnvilState<BackendData>> for SSD {
    fn proximity_in(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        _data: &mut AnvilState<BackendData>,
        _tool_descriptor: &smithay::backend::input::TabletToolDescriptor,
        _tablet: &smithay::input::tablet::Tablet,
        _serial: Serial,
    ) {
    }

    fn proximity_out(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        _tool_descriptor: &smithay::backend::input::TabletToolDescriptor,
    ) {
        data.deco_leave(&self.0);
    }

    fn down(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        _tool_descriptor: &smithay::backend::input::TabletToolDescriptor,
        event: &smithay::input::tablet::tool::DownEvent,
    ) {
        data.deco_button(seat, &self.0, BTN_LEFT, true, event.serial);
    }

    fn up(
        &self,
        seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        _tool_descriptor: &smithay::backend::input::TabletToolDescriptor,
        event: &smithay::input::tablet::tool::UpEvent,
    ) {
        data.deco_button(seat, &self.0, BTN_LEFT, false, event.serial);
    }

    fn motion(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        data: &mut AnvilState<BackendData>,
        _tool_descriptor: &smithay::backend::input::TabletToolDescriptor,
        event: &smithay::input::tablet::tool::MotionEvent,
    ) {
        data.deco_motion(&self.0, event.location);
    }

    fn button(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        _data: &mut AnvilState<BackendData>,
        _tool_descriptor: &smithay::backend::input::TabletToolDescriptor,
        _event: &smithay::input::tablet::tool::ButtonEvent,
    ) {
    }

    fn axis(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        _data: &mut AnvilState<BackendData>,
        _tool_descriptor: &smithay::backend::input::TabletToolDescriptor,
        _frame: smithay::input::tablet::tool::AxisFrame,
    ) {
    }

    fn frame(
        &self,
        _seat: &Seat<AnvilState<BackendData>>,
        _data: &mut AnvilState<BackendData>,
        _tool_descriptor: &smithay::backend::input::TabletToolDescriptor,
        _time: InputTime,
    ) {
    }
}

impl SpaceElement for WindowElement {
    fn geometry(&self) -> Rectangle<i32, Logical> {
        let geo = SpaceElement::geometry(&self.0);
        if self.decorated() {
            Rectangle::from_size(geo.size + deco::extra_size())
        } else {
            geo
        }
    }
    fn bbox(&self) -> Rectangle<i32, Logical> {
        let bbox = SpaceElement::bbox(&self.0);
        if self.rolled() || self.decorated() && self.decoration().docked {
            let tab = self.decoration().tab_layout(&self.title(), self.content_size());
            return tab.rect();
        }
        if self.decorated() {
            let geo = SpaceElement::geometry(&self.0);
            let frame = Rectangle::from_size(geo.size + deco::extra_size());
            let mut client = bbox;
            client.loc += self.client_render_offset();
            frame.merge(client)
        } else {
            bbox
        }
    }
    fn is_in_input_region(&self, point: &Point<f64, Logical>) -> bool {
        self.on_decoration(*point)
            || !self.tab_only() && SpaceElement::is_in_input_region(&self.0, &(*point - self.client_render_offset().to_f64()))
    }
    fn z_index(&self) -> u8 {
        SpaceElement::z_index(&self.0)
    }

    fn set_activate(&self, activated: bool) {
        self.decoration().focused = activated;
        SpaceElement::set_activate(&self.0, activated);
    }
    fn output_enter(&self, output: &Output, overlap: Rectangle<i32, Logical>) {
        SpaceElement::output_enter(&self.0, output, overlap);
    }
    fn output_leave(&self, output: &Output) {
        SpaceElement::output_leave(&self.0, output);
    }
    #[profiling::function]
    fn refresh(&self) {
        SpaceElement::refresh(&self.0);
    }
}

render_elements!(
    pub WindowRenderElement<R> where R: ImportAll + ImportMem;
    Window=WaylandSurfaceRenderElement<R>,
    Tab=crate::render::Exact<MemoryRenderBufferRenderElement<R>>,
    Decoration=SolidColorRenderElement,
);

impl<R: Renderer> std::fmt::Debug for WindowRenderElement<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Window(arg0) => f.debug_tuple("Window").field(arg0).finish(),
            Self::Tab(arg0) => f.debug_tuple("Tab").field(arg0).finish(),
            Self::Decoration(arg0) => f.debug_tuple("Decoration").field(arg0).finish(),
            Self::_GenericCatcher(arg0) => f.debug_tuple("_GenericCatcher").field(arg0).finish(),
        }
    }
}

impl<R> AsRenderElements<R> for WindowElement
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + Texture + Send + 'static,
{
    type RenderElement = WindowRenderElement<R>;

    fn render_elements<C: From<Self::RenderElement>>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<C> {
        let window_bbox = SpaceElement::bbox(&self.0);

        if self.decorated() && !window_bbox.is_empty() {
            let title = self.title();
            let content = self.content_size();
            let (tabs, frame) =
                self.decoration()
                    .render_elements(renderer, &title, content, location, scale, alpha);
            if self.tab_only() {
                return tabs.into_iter().map(WindowRenderElement::Tab).map(C::from).collect();
            }
            let client_location = location + self.client_render_offset().to_physical_precise_round(scale);
            let window_elements: Vec<WindowRenderElement<R>> =
                AsRenderElements::render_elements(&self.0, renderer, client_location, scale, alpha);

            // Popups and the window first, so menus overlap the tab.
            window_elements
                .into_iter()
                .chain(tabs.into_iter().map(WindowRenderElement::Tab))
                .chain(frame.into_iter().map(WindowRenderElement::Decoration))
                .map(C::from)
                .collect()
        } else {
            AsRenderElements::render_elements(&self.0, renderer, location, scale, alpha)
                .into_iter()
                .map(C::from)
                .collect()
        }
    }
}
