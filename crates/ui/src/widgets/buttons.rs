//! BeOS's push button, check box and radio button: bevels and gradients
//! on device pixels, the label's capitals centred, pushed in by a line
//! while held.

use std::rc::Rc;

use gpui::{App, Bounds, FontWeight, IntoElement, Pixels, RenderOnce, SharedString, Window, fill, point, px, size};
use olo_kit_base::metrics as m;

use super::control::{ControlState, control};
use crate::{style, theme};

/// A small button's height (`Button::small`).
pub const SMALL_BUTTON_HEIGHT: f32 = 20.0;

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;
type AtHandler = Rc<dyn Fn(Bounds<Pixels>, &mut Window, &mut App)>;

/// BButton: a bevelled push button. The default one sits in a recessed
/// ring (drawn outside its bounds, so a row of buttons stays aligned).
#[derive(IntoElement)]
pub struct Button {
    id: SharedString,
    label: SharedString,
    default: bool,
    pushed: bool,
    small: bool,
    enabled: bool,
    min_width: f32,
    on_click: Option<AtHandler>,
}

impl Button {
    pub fn new(label: impl Into<SharedString>) -> Button {
        let label = label.into();
        Button { id: label.clone(), label, default: false, pushed: false, small: false, enabled: true, min_width: m::BUTTON_MIN_WIDTH, on_click: None }
    }

    /// Its id, when two buttons share a label.
    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = id.into();
        self
    }

    pub fn default(mut self, on: bool) -> Self {
        self.default = on;
        self
    }

    /// Drawn held down: the chosen one of a set of tool buttons.
    pub fn pushed(mut self, on: bool) -> Self {
        self.pushed = on;
        self
    }

    /// Smaller, in the small font, for actions inside something dense.
    pub fn small(mut self) -> Self {
        self.small = true;
        self.min_width = self.min_width.min(48.0);
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(move |_, window, cx| f(window, cx)));
        self
    }

    /// Takes a click, told where the button is (a menu to open under it).
    pub fn on_click_at(mut self, f: impl Fn(Bounds<Pixels>, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

/// A button's width for `label` (on the two-point grid).
pub fn button_width(window: &mut Window, label: &str, small: bool) -> f32 {
    let size = if small { style::DETAIL_SIZE } else { style::PLAIN_SIZE };
    let pad = if small { m::CONTROL_PAD * 0.6 } else { m::CONTROL_PAD };
    let min = if small { 48.0 } else { m::BUTTON_MIN_WIDTH };
    let tw = f32::from(style::text_width(window, label.to_string(), size, style::ui_font(FontWeight::NORMAL)));
    (((tw + 2.0 * pad).max(min)) / 2.0).ceil() * 2.0
}

impl RenderOnce for Button {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let font_size = if self.small { style::DETAIL_SIZE } else { style::PLAIN_SIZE };
        let pad = if self.small { m::CONTROL_PAD * 0.6 } else { m::CONTROL_PAD };
        let tw = f32::from(style::text_width(window, self.label.clone(), font_size, style::ui_font(FontWeight::NORMAL)));
        let w = (((tw + 2.0 * pad).max(self.min_width)) / 2.0).ceil() * 2.0;
        let h = if self.small { SMALL_BUTTON_HEIGHT } else { m::CONTROL_HEIGHT };
        let (label, default, pushed) = (self.label.clone(), self.default, self.pushed);
        let mut c = control(self.id.clone(), w, h, move |r, s, window, cx| paint_button(r, s, &label, default, pushed, font_size, window, cx)).enabled(self.enabled).focusable().agent(
            "Button",
            self.label.clone(),
            None,
        );
        if default {
            c = c.default_key();
        }
        if let Some(f) = self.on_click {
            c = c.on_click(move |b, window, cx| f(b, window, cx));
        }
        c
    }
}

/// A button that is only its words, in the small font and the link
/// colour, underlined under the pointer: a small action beside what it
/// acts on, in a line that has no room for a push button (a box's Reset,
/// in its top line: `BBox::action`). `LABEL_ROW` tall.
#[derive(IntoElement)]
pub struct LinkButton {
    id: SharedString,
    label: SharedString,
    enabled: bool,
    on_click: Option<Handler>,
}

impl LinkButton {
    pub fn new(label: impl Into<SharedString>) -> LinkButton {
        let label = label.into();
        LinkButton { id: label.clone(), label, enabled: true, on_click: None }
    }

    /// Its id, when two share a label (each box's Reset).
    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = id.into();
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

/// A link button's width for `label` (on the two-point grid).
pub fn link_button_width(window: &mut Window, label: &str) -> f32 {
    let tw = f32::from(style::text_width(window, label.to_string(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL)));
    (tw / 2.0).ceil() * 2.0
}

impl RenderOnce for LinkButton {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let w = link_button_width(window, &self.label);
        let label = self.label.clone();
        let mut c = control(self.id.clone(), w, m::LABEL_ROW, move |r, s, window, cx| {
            let p = theme::palette(cx);
            // (The blue of links, readable on the panel.)
            let link = olo_theme::readable(p.hue(olo_theme::Hue::Blue), p.panel_background, 4.5);
            let color = if !s.enabled {
                style::disabled_text(&p)
            } else if s.pressed {
                style::shade(&p, link, 1.25)
            } else {
                style::c(link)
            };
            let text = style::label(window, cx, r.left(), r.center().y, style::Align::Left, label.to_string(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), color);
            if s.focused {
                focus_underline(window, text, &p);
            } else if s.hovered && s.enabled {
                style::hline_above(window, text.left(), text.right(), text.bottom(), color);
            }
        })
        .enabled(self.enabled)
        .focusable()
        .pointing()
        .agent("Button", self.label.clone(), None);
        if let Some(f) = self.on_click {
            c = c.on_click(move |_, window, cx| f(window, cx));
        }
        c
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_button(r: Bounds<Pixels>, s: ControlState, label: &str, default: bool, pushed: bool, font_size: f32, window: &mut Window, cx: &mut App) {
    let p = theme::palette(cx);
    let bg = p.panel_background;
    if default {
        // A recessed groove the button sits in.
        let ring = px(3.0);
        let outer = style::snap(window, Bounds::from_corners(point(r.left() - ring, r.top() - ring), point(r.right() + ring, r.bottom() + ring)));
        style::fill_rect(window, outer, style::shade(&p, bg, 1.06));
        style::bevel(window, outer, style::shade(&p, bg, 1.3), style::c(p.shine));
    }
    let pressed = s.pressed || pushed;
    let base = p.control_background;
    let (top, bottom) = if pressed {
        (style::shade(&p, base, 1.2), style::shade(&p, base, 1.12))
    } else if s.hovered {
        (style::tint(base, 0.7), style::shade(&p, base, 1.02))
    } else {
        (style::tint(base, 0.85), style::shade(&p, base, 1.06))
    };
    let border = if s.enabled { style::shade(&p, bg, 1.6) } else { style::shade(&p, bg, 1.3) };
    let inner = style::inset(window, r, 1.0);
    style::vgradient(window, inner, top, bottom);
    style::frame_rect(window, r, border);
    if pressed {
        style::bevel(window, inner, style::shade(&p, base, 1.35), style::shade(&p, base, 1.1));
    } else {
        style::bevel(window, inner, style::c(p.shine), style::shade(&p, base, 1.2));
    }
    // Held by the pointer, the label gives a device pixel; one that
    // stays down (a chosen tool) keeps it level with its row.
    let off = if s.pressed && !pushed { style::hairline(window) } else { px(0.0) };
    let color = if s.enabled { style::c(p.control_text) } else { style::disabled_text(&p) };
    let c = r.center();
    let text = style::label(window, cx, c.x + off, c.y + off, style::Align::Center, label.to_string(), font_size, style::ui_font(FontWeight::NORMAL), color);
    if s.focused {
        focus_underline(window, text, &p);
    }
}

/// BeOS marks the control with the keyboard by underlining its label in
/// the navigation colour.
pub(crate) fn focus_underline(window: &mut Window, text: Bounds<Pixels>, p: &olo_theme::Palette) {
    style::hline_above(window, text.left(), text.right(), text.bottom(), style::c(p.keyboard_navigation));
}

/// The sunken well check boxes share.
fn mark_well(window: &mut Window, r: Bounds<Pixels>, p: &olo_theme::Palette, pressed: bool, enabled: bool) {
    let fill = if pressed {
        style::shade(p, p.document_background, 1.12)
    } else if enabled {
        style::c(p.document_background)
    } else {
        style::c(p.panel_background)
    };
    style::fill_rect(window, r, fill);
    style::bevel(window, r, style::shade(p, p.panel_background, 1.18), style::c(p.shine));
    let inner = style::inset(window, r, 1.0);
    style::bevel(window, inner, style::shade(p, p.panel_background, 1.6), style::shade(p, p.panel_background, 1.08));
}

/// BCheckBox.
#[derive(IntoElement)]
pub struct CheckBox {
    id: SharedString,
    label: SharedString,
    checked: bool,
    mixed: bool,
    enabled: bool,
    height: f32,
    on_toggle: Option<Rc<dyn Fn(bool, &mut Window, &mut App)>>,
}

impl CheckBox {
    pub fn new(label: impl Into<SharedString>, checked: bool) -> CheckBox {
        let label = label.into();
        CheckBox { id: label.clone(), label, checked, mixed: false, enabled: true, height: m::MARK_ROW, on_toggle: None }
    }

    /// As tall as the controls beside it (`metrics::CONTROL_HEIGHT` beside
    /// a text field or button): placed alike, its label and theirs land
    /// on the same device pixels at every scale (a shorter box centred
    /// beside them rounds onto another at 1.25×).
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }

    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = id.into();
        self
    }

    /// A dash, not a check: on for some of what it stands for. A click
    /// turns it on.
    pub fn mixed(mut self, on: bool) -> Self {
        self.mixed = on;
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Gets the new value when clicked.
    pub fn on_toggle(mut self, f: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for CheckBox {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        // Its id is its label unless given one: check boxes without a
        // label (a table's columns) shared the id "", and a click on one
        // was lost among them (Preferences ▸ Notifications' apps).
        debug_assert!(!self.id.is_empty(), "a CheckBox without a label needs an id of its own (CheckBox::id)");
        let tw = f32::from(style::text_width(window, self.label.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL))).ceil();
        let w = m::MARK_SIZE + m::MARK_GAP + tw;
        let (label, checked, mixed) = (self.label.clone(), self.checked, self.mixed);
        let mut c = control(self.id.clone(), w, self.height, move |r, s, window, cx| {
            let p = theme::palette(cx);
            // Centred on the label's x-height, on the device grid.
            let top = style::snap_coord(window, px((f32::from(r.center().y) - 6.5).round()));
            let well = Bounds { origin: point(style::snap_coord(window, r.left()), top), size: size(px(m::MARK_SIZE), px(m::MARK_SIZE)) };
            mark_well(window, well, &p, s.pressed, s.enabled);
            let ink = if s.enabled { style::c(p.control_mark) } else { style::disabled_text(&p) };
            if mixed {
                let y = style::snap_coord(window, well.center().y - px(0.75));
                let h = style::snap_coord(window, px(1.5)).max(style::hairline(window));
                style::fill_rect(window, Bounds::from_corners(point(well.left() + px(3.5), y), point(well.right() - px(3.5), y + h)), ink);
            } else if checked {
                let mark = Bounds::from_corners(point(well.left() + px(3.0), well.top() + px(3.5)), point(well.right() - px(2.5), well.bottom() - px(3.5)));
                crate::menu::check_mark(window, mark, ink);
            }
            let color = if s.enabled { style::c(p.panel_text) } else { style::disabled_text(&p) };
            let text = style::label(window, cx, well.right() + px(m::MARK_GAP), r.center().y, style::Align::Left, label.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), color);
            if s.focused {
                focus_underline(window, text, &p);
            }
        })
        .enabled(self.enabled)
        .focusable()
        .agent("CheckBox", self.label.clone(), Some(checked && !mixed));
        if let Some(f) = self.on_toggle {
            let next = mixed || !checked;
            c = c.on_click(move |_, window, cx| f(next, window, cx));
        }
        c
    }
}

/// A check box's look alone, in `well` (`MARK_SIZE` square), checked or
/// not: what isn't a control but shows one (a document's task list).
pub fn check_box_look(window: &mut Window, well: Bounds<Pixels>, p: &olo_theme::Palette, checked: bool) {
    mark_well(window, well, p, false, true);
    if checked {
        let mark = Bounds::from_corners(point(well.left() + px(3.0), well.top() + px(3.5)), point(well.right() - px(2.5), well.bottom() - px(3.5)));
        crate::menu::check_mark(window, mark, style::c(p.control_mark));
    }
}

/// BRadioButton: chosen by a click; the caller keeps which of its group
/// is chosen.
#[derive(IntoElement)]
pub struct RadioButton {
    id: SharedString,
    label: SharedString,
    selected: bool,
    enabled: bool,
    height: f32,
    on_select: Option<Handler>,
}

impl RadioButton {
    pub fn new(label: impl Into<SharedString>, selected: bool) -> RadioButton {
        let label = label.into();
        RadioButton { id: label.clone(), label, selected, enabled: true, height: m::MARK_ROW, on_select: None }
    }

    /// As tall as what's beside it, as [`CheckBox::height`]: beside a
    /// label or a button (`metrics::CONTROL_HEIGHT`), its text and theirs
    /// share a baseline at every scale (20 points tall, centred beside 24,
    /// it was a device pixel off at 1.25× and 1.75×).
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }

    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = id.into();
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    pub fn on_select(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for RadioButton {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        // (As a CheckBox: without a label, an id of its own.)
        debug_assert!(!self.id.is_empty(), "a RadioButton without a label needs an id of its own (RadioButton::id)");
        let tw = f32::from(style::text_width(window, self.label.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL))).ceil();
        let w = 13.0 + m::MARK_GAP + tw;
        let (label, selected) = (self.label.clone(), self.selected);
        let mut c = control(self.id.clone(), w, self.height, move |r, s, window, cx| {
            let p = theme::palette(cx);
            let bg = p.panel_background;
            let cx0 = r.left() + px(6.5);
            let cy0 = px(f32::from(r.center().y).round() - 0.5);
            let circle = |window: &mut Window, x: Pixels, y: Pixels, radius: f32, color: gpui::Hsla| {
                let b = Bounds { origin: point(x - px(radius), y - px(radius)), size: size(px(2.0 * radius), px(2.0 * radius)) };
                window.paint_quad(fill(b, color).corner_radii(px(radius)));
            };
            // The outer ring: dark upper left, light lower right.
            circle(window, cx0 + px(0.5), cy0 + px(0.5), 6.5, style::c(p.shine));
            circle(window, cx0 - px(0.3), cy0 - px(0.3), 6.5, style::shade(&p, bg, 1.25));
            let face = if s.pressed {
                style::shade(&p, p.document_background, 1.12)
            } else if s.enabled {
                style::c(p.document_background)
            } else {
                style::c(bg)
            };
            // The ring a point wide outside the face.
            circle(window, cx0, cy0, 6.5, style::shade(&p, bg, 1.55));
            circle(window, cx0, cy0, 5.5, face);
            if selected {
                let ink = if s.enabled { p.control_mark } else { olo_theme::mix(p.panel_text, p.panel_background, 0.55) };
                circle(window, cx0, cy0, 3.0, style::c(ink));
                // A small highlight keeps the dot from looking flat: the
                // mark's colour, half again as bright.
                let lit = [ink[0], ink[1], ink[2]].map(|v| (f32::from(v) * 1.4).min(255.0) as u8);
                circle(window, cx0 - px(1.0), cy0 - px(1.0), 0.9, style::c([lit[0], lit[1], lit[2], 255]));
            }
            let color = if s.enabled { style::c(p.panel_text) } else { style::disabled_text(&p) };
            let text = style::label(window, cx, r.left() + px(13.0 + m::MARK_GAP), r.center().y, style::Align::Left, label.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), color);
            if s.focused {
                focus_underline(window, text, &p);
            }
        })
        .enabled(self.enabled)
        .focusable()
        .agent("RadioButton", self.label.clone(), Some(selected));
        if let Some(f) = self.on_select {
            c = c.on_click(move |_, window, cx| f(window, cx));
        }
        c
    }
}
