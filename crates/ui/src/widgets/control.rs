//! A control the pointer can use: what BeOS's controls share. It keeps
//! whether the pointer is over it and whether it's held down on it
//! (across frames, by its id), draws itself through a closure told
//! those, and takes a click (the press and release both on it). The
//! window is drawn again only when one of those changes. It says what it
//! is to agents as it draws. A focusable one ([`Control::focusable`]) is
//! in the window's tab order: Tab and ⇧Tab move the keyboard to it, it
//! draws itself focused (BeOS underlines the label in the navigation
//! colour), and Space clicks it.

use std::rc::Rc;

use gpui::{
    App, Bounds, CursorStyle, DispatchPhase, Element, ElementId, FocusHandle, GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, SharedString, Style, Window, px,
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
    /// It has the keyboard (reached by Tab).
    pub focused: bool,
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
    /// What agents read as its value (a menu's choice).
    value: Option<SharedString>,
    /// The pointing hand over it (a link), not the arrow.
    pointing: bool,
    focusable: bool,
    /// Return clicks it, from anywhere in the window (a default button).
    default: bool,
}

/// A control `width` by `height`, drawn by `paint`.
pub fn control(id: impl Into<ElementId>, width: f32, height: f32, paint: impl FnOnce(Bounds<Pixels>, ControlState, &mut Window, &mut App) + 'static) -> Control {
    Control { id: id.into(), width: px(width), height: px(height), enabled: true, paint: Some(Box::new(paint)), on_click: None, node: None, value: None, pointing: false, focusable: false, default: false }
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

    /// In the window's tab order, Space clicking it while it has the
    /// keyboard (not while it can't be used).
    pub fn focusable(mut self) -> Self {
        self.focusable = true;
        self
    }

    /// Return clicks it from anywhere in its window, when what has the
    /// keyboard doesn't take Return itself (a text field's Return is its
    /// own): BeOS's default button.
    pub fn default_key(mut self) -> Self {
        self.default = true;
        self
    }

    /// What agents read it as: its role ("Button"), its name, and whether
    /// it's checked (check boxes, radio buttons).
    pub fn agent(mut self, role: &'static str, name: impl Into<SharedString>, checked: Option<bool>) -> Self {
        self.node = Some((role, name.into(), checked));
        self
    }

    /// [`Control::agent`] with a value (a filter menu: "Program", "Any").
    pub fn agent_value(mut self, role: &'static str, name: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        self.node = Some((role, name.into(), None));
        self.value = Some(value.into());
        self
    }

    /// The pointing hand over it (a link), not the arrow.
    pub fn pointing(mut self) -> Self {
        self.pointing = true;
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

/// A focusable control's keyboard, kept across frames by its id.
#[derive(Clone)]
struct Keyboard(FocusHandle);

impl Element for Control {
    type RequestLayoutState = ();
    type PrepaintState = (Hitbox, Option<FocusHandle>);

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

    fn prepaint(&mut self, id: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> (Hitbox, Option<FocusHandle>) {
        let focus = (self.focusable && self.enabled).then(|| {
            window.with_optional_element_state::<Keyboard, _>(id, |k, _| {
                let k = k.flatten().unwrap_or_else(|| Keyboard(cx.focus_handle().tab_stop(true)));
                (k.0.clone(), Some(k))
            })
        });
        if let Some(f) = &focus {
            window.set_focus_handle(f, cx);
        }
        (window.insert_hitbox(bounds, HitboxBehavior::Normal), focus)
    }

    fn paint(&mut self, id: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), (hitbox, focus): &mut (Hitbox, Option<FocusHandle>), window: &mut Window, cx: &mut App) {
        let enabled = self.enabled;
        let hovered = hitbox.is_hovered(window);
        let focused = focus.as_ref().is_some_and(|f| f.is_focused(window));
        if let Some(f) = focus.as_ref() {
            window.insert_tab_stop(f);
        }
        let held = window.with_optional_element_state::<Held, _>(id, |held, _| {
            let held = held.flatten().unwrap_or_default();
            (held.clone(), Some(held))
        });
        let down = held.0.get();
        let state = ControlState { hovered: hovered && enabled, pressed: down && hovered && enabled, enabled, focused };
        if enabled {
            window.set_cursor_style(if self.pointing { CursorStyle::PointingHand } else { CursorStyle::Arrow }, hitbox);
        }
        if let Some(paint) = self.paint.take() {
            // A control that can't be used now is faded, as egui fades a
            // disabled one (its `disabled_alpha`).
            window.with_element_opacity((!enabled).then_some(DISABLED_OPACITY), |window| paint(bounds, state, window, cx));
        }
        if let Some((role, name, checked)) = self.node.clone() {
            agent::record(window, cx, agent::Node { role, name: Some(name), value: self.value.clone(), bounds, enabled, clickable: enabled, checked, focused, ..Default::default() });
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
        // The window's Return, as its default button.
        if self.default
            && let Some(f) = self.on_click.clone()
        {
            let b = hitbox.bounds;
            crate::app::set_default_button(window, cx, std::rc::Rc::new(move |window: &mut Window, cx: &mut App| f(b, window, cx)));
        }
        // Space, while it has the keyboard: clicked.
        if focused && let Some(f) = self.on_click.clone() {
            let b = hitbox.bounds;
            window.on_key_event(move |e: &KeyDownEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble && e.keystroke.key == "space" && !e.keystroke.modifiers.modified() {
                    f(b, window, cx);
                    cx.stop_propagation();
                }
            });
        }
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
