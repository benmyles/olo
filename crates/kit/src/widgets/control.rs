//! A control the pointer can use: what BeOS's controls share. Its node
//! in the window's tree keeps whether the pointer is over it and
//! whether it's held down on it; it draws itself through a closure told
//! those, and is drawn again, alone, when one of them changes. It takes
//! a click (the press and release both on it) and says what it is to
//! agents. A focusable one ([`Control::focusable`]) is in the window's
//! tab order: Tab and ⇧Tab move the keyboard to it, it draws itself
//! focused (BeOS underlines the label in the navigation colour), and
//! Space clicks it.

use std::{
    hash::{Hash, Hasher},
    rc::Rc,
};

use olo_interface::{Bounds, ClickEvent, CursorStyle, Element, ElementId, InteractiveElement as _, IntoElement, Painter, Pixels, SharedString, Styled as _, WindowCx, control_canvas, px, relative};

pub use olo_interface::{ControlState, DISABLED_OPACITY};

type Paint = Rc<dyn Fn(&mut Painter, Bounds<Pixels>, ControlState)>;
type OnClick = Rc<dyn Fn(&ClickEvent, &mut WindowCx)>;

pub struct Control {
    id: ElementId,
    width: Pixels,
    height: Pixels,
    enabled: bool,
    paint: Paint,
    paint_key: Option<u64>,
    on_click: Option<OnClick>,
    node: Option<(&'static str, SharedString, Option<bool>)>,
    /// What agents read as its value (a menu's choice).
    value: Option<SharedString>,
    /// The pointing hand over it (a link), not the arrow.
    pointing: bool,
    focusable: bool,
    /// Return clicks it, from anywhere in the window (a default button).
    default: bool,
    /// No wider than what it's in (`shrinks`).
    shrinks: bool,
    /// What its tip says, once the pointer has rested on it.
    tip: Option<SharedString>,
}

/// A control `width` by `height`, drawn by `paint`.
pub fn control(id: impl Into<ElementId>, width: f32, height: f32, paint: impl Fn(&mut Painter, Bounds<Pixels>, ControlState) + 'static) -> Control {
    Control {
        id: id.into(),
        width: px(width),
        height: px(height),
        enabled: true,
        paint: Rc::new(paint),
        paint_key: None,
        on_click: None,
        node: None,
        value: None,
        pointing: false,
        focusable: false,
        default: false,
        shrinks: false,
        tip: None,
    }
}

impl Control {
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Takes a click (where the control is: `WindowCx::listener_bounds`).
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut WindowCx) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    /// Everything its look depends on, besides how the pointer and the
    /// keyboard stand to it (its label, whether it's checked): while
    /// that's the same it isn't drawn again when what holds it says
    /// what it shows again.
    pub fn look(mut self, look: impl Hash) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        look.hash(&mut hasher);
        self.paint_key = Some(hasher.finish());
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

    /// A tip saying `text`, once the pointer has rested on it (a tool's
    /// name and keys).
    pub fn tip(mut self, text: impl Into<SharedString>) -> Self {
        self.tip = Some(text.into());
        self
    }

    /// The pointing hand over it (a link), not the arrow.
    pub fn pointing(mut self) -> Self {
        self.pointing = true;
        self
    }

    /// Its width at most what it's in: narrower, its paint draws what
    /// fits (a menu field's long choice cut short; the layout lint found
    /// Sound's and Display's run out of their windows).
    pub fn shrinks(mut self) -> Self {
        self.shrinks = true;
        self
    }
}

impl IntoElement for Control {
    fn into_element(self) -> Element {
        let paint = self.paint;
        let tip_key = self.id.clone();
        let mut element = control_canvas(move |g, bounds, state| paint(g, bounds, state)).id(self.id).w(self.width).h(self.height).flex_shrink_0().disabled(!self.enabled);
        if let Some(key) = self.paint_key {
            element = element.paint_key(key);
        }
        if self.shrinks {
            // (Narrower in a row, never shorter in a squeezed column: too
            // little room overflows, where the layout lint sees it.)
            element = element.flex_shrink().max_w(relative(1.0)).min_h(self.height);
        }
        if self.enabled {
            element = element.cursor(if self.pointing { CursorStyle::PointingHand } else { CursorStyle::Arrow });
            if self.focusable {
                element = element.tab_stop();
            }
            if self.default && self.on_click.is_some() {
                element = element.default_key();
            }
        }
        if let Some((role, name, checked)) = self.node {
            element = element.agent(role, name).agent_checked(checked).agent_clickable(true);
            if let Some(value) = self.value {
                element = element.agent_value(value);
            }
        }
        if let Some(f) = self.on_click {
            element = element.on_click(move |event, cx| f(event, cx));
        }
        if let Some(text) = self.tip {
            element = crate::tip::on(element, format!("{tip_key:?}"), text);
        }
        element
    }
}
