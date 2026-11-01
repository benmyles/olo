//! BMenuField: an optional label and a raised field showing the current
//! choice; a click opens the choices as a menu with the current one over
//! the field (as BeOS and macOS do), and choosing one changes it.

use std::rc::Rc;

use gpui::{App, Bounds, FontWeight, IntoElement, PathBuilder, Pixels, RenderOnce, SharedString, Window, point, px};
use olo_kit_base::metrics as m;

use super::control::control;
use crate::{Menu, menu, style, theme};

type OnChange = Rc<dyn Fn(usize, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct MenuField {
    id: SharedString,
    label: Option<SharedString>,
    quiet: bool,
    divider: Option<f32>,
    options: Vec<SharedString>,
    selected: usize,
    width: Option<f32>,
    enabled: bool,
    on_change: Option<OnChange>,
}

impl MenuField {
    pub fn new(id: impl Into<SharedString>, options: Vec<SharedString>, selected: usize) -> MenuField {
        MenuField { id: id.into(), label: None, quiet: false, divider: None, options, selected, width: None, enabled: true, on_change: None }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
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

    /// Gets the index chosen.
    pub fn on_change(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for MenuField {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let font = style::ui_font(FontWeight::NORMAL);
        let widest = self.options.iter().map(|o| f32::from(style::text_width(window, o.clone(), style::PLAIN_SIZE, font.clone()))).fold(0.0, f32::max);
        let label_w = match &self.label {
            Some(l) => self.divider.unwrap_or_else(|| f32::from(style::text_width(window, l.clone(), style::PLAIN_SIZE, font.clone())).ceil() + m::LABEL_GAP),
            None => 0.0,
        };
        let width = self.width.unwrap_or((label_w + m::TEXT_INSET + widest + 12.0 + 18.0).ceil());
        let current = self.options.get(self.selected).cloned().unwrap_or_default();
        let label = self.label.clone();
        let name = self.label.clone().map(|l| SharedString::from(l.trim_end_matches(':').to_string())).unwrap_or_default();
        let (options, selected, on_change) = (self.options.clone(), self.selected, self.on_change.clone());
        let id = self.id.clone();
        let shown = current.clone();
        let quiet = self.quiet;
        control(self.id.clone(), width, m::CONTROL_HEIGHT, move |r, s, window, cx| {
            let p = theme::palette(cx);
            let color = if s.enabled { style::c(p.panel_text) } else { style::disabled_text(&p) };
            let label_color = if quiet || !s.enabled { style::disabled_text(&p) } else { color };
            let named = label.as_ref().map(|l| style::label(window, cx, r.left(), r.center().y, style::Align::Left, l.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), label_color));
            let field = field_bounds(r, label_w);
            // A raised field with a drop-down mark.
            let base = p.menu_background;
            let inner = style::inset(window, field, 1.0);
            style::vgradient(window, inner, style::tint(base, 0.82), style::shade(&p, base, 1.05));
            style::frame_rect(window, field, style::shade(&p, p.panel_background, 1.55));
            style::bevel(window, inner, style::c(p.shine), style::shade(&p, base, 1.2));
            let value = window.with_content_mask(Some(gpui::ContentMask { bounds: Bounds::from_corners(field.origin, point(field.right() - px(15.0 + m::TIGHT), field.bottom())) }), |window| {
                style::label(window, cx, field.left() + px(m::TEXT_INSET), field.center().y, style::Align::Left, shown.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), color)
            });
            // With the keyboard: its label underlined (the choice, when
            // it has none).
            if s.focused {
                super::buttons::focus_underline(window, named.unwrap_or(value), &p);
            }
            let ax = field.right() - px(11.0);
            let ay = px(f32::from(field.center().y).round());
            let mut path = PathBuilder::fill();
            path.move_to(point(ax - px(4.0), ay - px(2.0)));
            path.line_to(point(ax + px(4.0), ay - px(2.0)));
            path.line_to(point(ax, ay + px(2.5)));
            path.close();
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        })
        .enabled(self.enabled)
        .focusable()
        .agent("ComboBox", name, None)
        .on_click(move |at, window, cx| {
            // Opened from the click: the choices, the current one over
            // the field.
            let mut menu = Menu::new(id.to_string());
            for (i, o) in options.iter().enumerate() {
                menu = menu.item(o.to_string(), i.to_string()).checked(i == selected);
            }
            let field = field_bounds(at, label_w);
            let on_change = on_change.clone();
            menu::popup_over(window, cx, menu, field, selected, move |command, window, cx| {
                if let (Ok(i), Some(f)) = (command.parse::<usize>(), &on_change)
                    && i != selected
                {
                    f(i, window, cx);
                }
            });
        })
    }
}

fn field_bounds(r: Bounds<Pixels>, label_w: f32) -> Bounds<Pixels> {
    Bounds::from_corners(point(r.left() + px(label_w), r.top()), r.bottom_right())
}
