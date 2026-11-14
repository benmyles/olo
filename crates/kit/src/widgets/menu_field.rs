//! BMenuField: an optional label and a raised field showing the current
//! choice; a click opens the choices as a menu with the current one over
//! the field (as BeOS and macOS do), and choosing one changes it.
//!
//! Or several choices ([`MenuField::many`]): the field says those chosen
//! ("Weekly, Weekly Fable", cut short with "…"), and its menu opens under
//! it with what none chosen means first, then each choice checked or not;
//! choosing one turns it on or off. One row however many there are, as
//! long as they come (a Token Pool's quotas: Ben, 2026-11-13, his ran out
//! of the window as a row of check boxes).

use std::rc::Rc;

use olo_interface::{Bounds, ContentMask, Corner, Element, FontWeight, Hsla, IntoElement, Painter, PathBuilder, Pixels, SharedString, WindowCx, point, px};
use olo_kit_base::metrics as m;

use super::control::control;
use crate::{Menu, menu, style, theme};

type OnChange = Rc<dyn Fn(&usize, &mut WindowCx)>;

pub struct MenuField {
    id: SharedString,
    label: Option<SharedString>,
    /// What agents (and the settings index) call it when it shows no
    /// label of its own (`name`).
    name: Option<SharedString>,
    quiet: bool,
    divider: Option<f32>,
    options: Vec<SharedString>,
    selected: usize,
    width: Option<f32>,
    enabled: bool,
    on_change: Option<OnChange>,
    /// Several chosen ([`MenuField::many`]): which, and what none means.
    many: Option<(Vec<bool>, SharedString)>,
}

/// What a field of several hands its `on_change` for its first item:
/// none chosen.
pub const NONE_CHOSEN: usize = usize::MAX;

impl MenuField {
    pub fn new(id: impl Into<SharedString>, options: Vec<SharedString>, selected: usize) -> MenuField {
        MenuField { id: id.into(), label: None, name: None, quiet: false, divider: None, options, selected, width: None, enabled: true, on_change: None, many: None }
    }

    /// A field of several choices, those `chosen` on (one for each of
    /// `options`), `none` what it says with none (its menu's first item).
    /// `on_change` gets the choice to turn on or off, or [`NONE_CHOSEN`].
    pub fn many(id: impl Into<SharedString>, options: Vec<SharedString>, chosen: Vec<bool>, none: impl Into<SharedString>) -> MenuField {
        MenuField { many: Some((chosen, none.into())), ..MenuField::new(id, options, NONE_CHOSEN) }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// What it's called: its label, else its `name` (an undo step's name,
    /// the settings index's).
    pub fn title(&self) -> Option<SharedString> {
        self.label.clone().or_else(|| self.name.clone())
    }

    /// Its name for agents, with no label shown: a field in a grid, named
    /// by its row and column ("Processors in Power Saver").
    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Its label quiet, as facts' are (a field set among them, its
    /// choice on their values' line with [`Self::divider`]).
    pub fn quiet_label(mut self) -> Self {
        self.quiet = true;
        self
    }

    /// Where the field starts after the label (lining fields up).
    pub fn divider(mut self, x: f32) -> Self {
        self.divider = Some(x);
        self
    }

    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Gets the index chosen (`cx.msg_with(|i: &usize| Msg::Chose(*i))`).
    pub fn on_change(mut self, f: impl Fn(&usize, &mut WindowCx) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

impl IntoElement for MenuField {
    fn into_element(self) -> Element {
        let font = style::ui_font(FontWeight::NORMAL);
        let widest = self.options.iter().chain(self.many.as_ref().map(|(_, none)| none)).map(|o| f32::from(style::text_width(o.clone(), style::PLAIN_SIZE, font.clone()))).fold(0.0, f32::max);
        let label_w = match &self.label {
            Some(l) => self.divider.unwrap_or_else(|| f32::from(style::text_width(l.clone(), style::PLAIN_SIZE, font.clone())).ceil() + m::LABEL_GAP),
            None => 0.0,
        };
        let width = self.width.unwrap_or((label_w + m::TEXT_INSET + widest + 12.0 + 18.0).ceil());
        let current = match &self.many {
            Some((chosen, none)) => {
                let on: Vec<&str> = self.options.iter().zip(chosen).filter(|(_, c)| **c).map(|(o, _)| o.as_ref()).collect();
                if on.is_empty() { none.clone() } else { SharedString::from(on.join(", ")) }
            }
            None => self.options.get(self.selected).cloned().unwrap_or_default(),
        };
        let many = self.many.clone();
        let label = self.label.clone();
        let name = self.label.clone().map(|l| SharedString::from(l.trim_end_matches(':').to_string())).or_else(|| self.name.clone()).unwrap_or_default();
        let (options, selected, on_change) = (self.options.clone(), self.selected, self.on_change.clone());
        let id = self.id.clone();
        let shown = current.clone();
        let quiet = self.quiet;
        control(self.id.clone(), width, m::CONTROL_HEIGHT, move |g, r, s| {
            let p = theme::palette();
            let color = if s.enabled { style::c(p.panel_text) } else { style::disabled_text(&p) };
            let label_color = if quiet || !s.enabled { style::disabled_text(&p) } else { color };
            let named = label.as_ref().map(|l| style::label(g, r.left(), r.center().y, style::Align::Left, l.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), label_color));
            let field = field_bounds(r, label_w);
            let inside = paint_field(g, field, color, &p);
            // (Cut short with "…" when the field is narrower than its
            // choice: it's no wider than what it's in.)
            let room = inside.right() - (inside.left() + px(m::TEXT_INSET));
            let value = g.with_content_mask(Some(ContentMask { bounds: inside }), |g| {
                style::label_fit(g, inside.left() + px(m::TEXT_INSET), field.center().y, style::Align::Left, shown.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), color, room)
            });
            // With the keyboard: its label underlined (the choice, when
            // it has none).
            if s.focused {
                super::buttons::focus_underline(g, named.unwrap_or(value), &p);
            }
        })
        .look((&self.label, &current, quiet, label_w.to_bits()))
        .enabled(self.enabled)
        .shrinks()
        .focusable()
        .agent_value("ComboBox", name, current)
        .on_click(move |_, cx| {
            let field = field_bounds(cx.listener_bounds(), label_w);
            // Several: under the field, none first, then each to turn on
            // or off.
            if let Some((chosen, none)) = &many {
                let mut menu = Menu::new(id.to_string()).item(none.to_string(), "none").checked(!chosen.contains(&true)).separator();
                for (i, o) in options.iter().enumerate() {
                    menu = menu.item(o.to_string(), i.to_string()).checked(chosen.get(i).copied().unwrap_or(false));
                }
                let on_change = on_change.clone();
                menu::popup(cx, menu, field, Corner::BottomLeft, move |command, cx| {
                    let i = if command == "none" { NONE_CHOSEN } else { command.parse::<usize>().unwrap_or(NONE_CHOSEN) };
                    if let Some(f) = &on_change {
                        f(&i, cx);
                    }
                });
                return;
            }
            // Opened from the click: the choices, the current one over
            // the field.
            let mut menu = Menu::new(id.to_string());
            for (i, o) in options.iter().enumerate() {
                menu = menu.item(o.to_string(), i.to_string()).checked(i == selected);
            }
            let on_change = on_change.clone();
            menu::popup_over(cx, menu, field, selected, move |command, cx| {
                if let (Ok(i), Some(f)) = (command.parse::<usize>(), &on_change)
                    && i != selected
                {
                    f(&i, cx);
                }
            });
        })
        .into_element()
    }
}

/// A pop-up field's look, filling `field`: raised, with the drop-down
/// mark at its right in `ink`. Returns where its choice goes (left of the
/// mark). A menu field's, and a tool bar's pickers'.
pub(crate) fn paint_field(g: &mut Painter, field: Bounds<Pixels>, ink: Hsla, p: &olo_theme::Palette) -> Bounds<Pixels> {
    let base = p.menu_background;
    let inner = style::inset(g, field, 1.0);
    style::vgradient(g, inner, style::tint(base, 0.82), style::shade(p, base, 1.05));
    style::frame_rect(g, field, style::shade(p, p.panel_background, 1.55));
    style::bevel(g, inner, style::c(p.shine), style::shade(p, base, 1.2));
    let ax = field.right() - px(11.0);
    let ay = px(f32::from(field.center().y).round());
    let mut path = PathBuilder::fill();
    path.move_to(point(ax - px(4.0), ay - px(2.0)));
    path.line_to(point(ax + px(4.0), ay - px(2.0)));
    path.line_to(point(ax, ay + px(2.5)));
    path.close();
    if let Ok(path) = path.build() {
        g.paint_path(path, ink);
    }
    Bounds::from_corners(field.origin, point(field.right() - px(15.0 + m::TIGHT), field.bottom()))
}

fn field_bounds(r: Bounds<Pixels>, label_w: f32) -> Bounds<Pixels> {
    Bounds::from_corners(point(r.left() + px(label_w), r.top()), r.bottom_right())
}
