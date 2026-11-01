//! A control the pointer can use: what BeOS's controls share. It keeps
//! whether the pointer is over it and whether it's held down on it
//! (across frames, by its id), draws itself through a closure told
//! those, and takes a click (the press and release both on it). The
//! window is drawn again only when one of those changes. It says what it
//! is to agents as it draws.

use std::rc::Rc;

use gpui::{
    App, Bounds, CursorStyle, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, SharedString, Style, Window, px,
};

use crate::agent;

/// What a control draws itself as.
#[derive(Clone, Copy, Debug, Default)]
pub struct ControlState {
    /// The pointer is over it.
    pub hovered: bool,
    /// Held down on it, the pointer still over it.
    pub pressed: bool,
    pub enabled: bool,
}

/// How opaque a control that can't be used now is drawn.
pub const DISABLED_OPACITY: f32 = 0.5;

type Paint = Box<dyn FnOnce(Bounds<Pixels>, ControlState, &mut Window, &mut App)>;
type OnClick = Rc<dyn Fn(Bounds<Pixels>, &mut Window, &mut App)>;

pub struct Control {
    id: ElementId,
    width: Pixels,
    height: Pixels,
    enabled: bool,
    paint: Option<Paint>,
    on_click: Option<OnClick>,
    node: Option<(&'static str, SharedString, Option<bool>)>,
}

/// A control `width` by `height`, drawn by `paint`.
pub fn control(id: impl Into<ElementId>, width: f32, height: f32, paint: impl FnOnce(Bounds<Pixels>, ControlState, &mut Window, &mut App) + 'static) -> Control {
    Control { id: id.into(), width: px(width), height: px(height), enabled: true, paint: Some(Box::new(paint)), on_click: None, node: None }
}

impl Control {
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Takes a click; told where the control is.
    pub fn on_click(mut self, f: impl Fn(Bounds<Pixels>, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    /// What agents read it as: its role ("Button"), its name, and whether
    /// it's checked (check boxes, radio buttons).
    pub fn agent(mut self, role: &'static str, name: impl Into<SharedString>, checked: Option<bool>) -> Self {
        self.node = Some((role, name.into(), checked));
        self
    }
}

impl IntoElement for Control {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// Whether it's held down: kept across frames by the control's id, and
/// shared with its event handlers (element state is only reachable while
/// drawing).
#[derive(Clone, Default)]
struct Held(Rc<std::cell::Cell<bool>>);

impl Element for Control {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = self.width.into();
        style.size.height = self.height.into();
        style.flex_shrink = 0.0;
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, _: &mut App) -> Hitbox {
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(&mut self, id: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), hitbox: &mut Hitbox, window: &mut Window, cx: &mut App) {
        let enabled = self.enabled;
        let hovered = hitbox.is_hovered(window);
        let held = window.with_optional_element_state::<Held, _>(id, |held, _| {
            let held = held.flatten().unwrap_or_default();
            (held.clone(), Some(held))
        });
        let down = held.0.get();
        let state = ControlState { hovered: hovered && enabled, pressed: down && hovered && enabled, enabled };
        if enabled {
            window.set_cursor_style(CursorStyle::Arrow, hitbox);
        }
        if let Some(paint) = self.paint.take() {
            // A control that can't be used now is faded, as egui fades a
            // disabled one (its `disabled_alpha`).
            window.with_element_opacity((!enabled).then_some(DISABLED_OPACITY), |window| paint(bounds, state, window, cx));
        }
        if let Some((role, name, checked)) = self.node.clone() {
            agent::record(window, cx, agent::Node { role, name: Some(name), bounds, enabled, clickable: enabled, checked, ..Default::default() });
        }
        if !enabled {
            return;
        }
        // The pointer coming or going: drawn again.
        let hb = hitbox.clone();
        let was = hovered;
        window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, _| {
            if phase.bubble() && hb.is_hovered(window) != was {
                window.refresh();
            }
        });
        let hb = hitbox.clone();
        let h = held.clone();
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, _| {
            if phase.bubble() && e.button == MouseButton::Left && hb.is_hovered(window) {
                h.0.set(true);
                window.refresh();
            }
        });
        let hb = hitbox.clone();
        let on_click = self.on_click.clone();
        window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
            if phase.bubble() && e.button == MouseButton::Left && held.0.get() {
                held.0.set(false);
                window.refresh();
                if hb.is_hovered(window)
                    && let Some(f) = &on_click
                {
                    f(hb.bounds, window, cx);
                }
            }
        });
    }
}
