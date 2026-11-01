//! BTextControl: an optional label and a sunken well holding a
//! [`TextField`], its text on the label's baseline; the well's inner line
//! turns the keyboard's colour while the field has it, and a click
//! anywhere in the well gives it the keyboard.

use gpui::{App, Entity, FontWeight, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, RenderOnce, SharedString, Styled as _, Window, canvas, div, px};
use olo_kit_base::metrics as m;

use super::text_field::TextField;
use crate::{style, theme};

#[derive(IntoElement)]
pub struct TextControl {
    field: Entity<TextField>,
    label: Option<SharedString>,
    divider: Option<f32>,
    width: Option<f32>,
}

impl TextControl {
    pub fn new(field: Entity<TextField>) -> TextControl {
        TextControl { field, label: None, divider: None, width: None }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Where the well begins, from the control's left edge.
    pub fn divider(mut self, x: f32) -> Self {
        self.divider = Some(x);
        self
    }

    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }
}

impl RenderOnce for TextControl {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let font = style::ui_font(FontWeight::NORMAL);
        let divider = self.label.as_ref().map(|l| self.divider.unwrap_or_else(|| f32::from(style::text_width(window, l.clone(), style::PLAIN_SIZE, font.clone())).ceil() + m::LABEL_GAP));
        let focused = self.field.read(cx).is_focused(window);
        // A field that can't be used (`TextField::set_enabled(false)`):
        // its label and well faded, as the egui kit's.
        let enabled = self.field.read(cx).enabled();
        let label = self.label.clone();
        let mut row = div().relative().h(px(m::CONTROL_HEIGHT)).flex_shrink_0();
        row = match self.width {
            Some(w) => row.w(px(w)),
            None => row.w_full(),
        };
        let well_left = divider.unwrap_or(0.0);
        let well = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let fill = if enabled { style::c(p.document_background) } else { style::c(p.panel_background) };
                style::fill_rect(window, style::inset(window, b, 2.0), fill);
                window.with_element_opacity((!enabled).then_some(super::control::DISABLED_OPACITY), |window| style::sunken(window, b, &p, focused));
            },
        )
        .absolute()
        .inset_0();
        let labels = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                if let Some(l) = &label {
                    let p = theme::palette(cx);
                    let ink = if enabled { style::c(p.panel_text) } else { style::disabled_text(&p) };
                    style::label(window, cx, b.left(), b.center().y, style::Align::Left, l.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), ink);
                }
            },
        )
        .absolute()
        .inset_0();
        let field = self.field.clone();
        row.child(labels).child(
            div()
                .absolute()
                .left(px(well_left))
                .right(px(0.0))
                .top(px(0.0))
                .bottom(px(0.0))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    if field.read(cx).enabled() {
                        let handle = gpui::Focusable::focus_handle(&field, cx);
                        window.focus(&handle);
                    }
                })
                .child(well)
                .child(div().absolute().left(px(m::TEXT_INSET)).right(px(m::TEXT_INSET)).top(px(0.0)).bottom(px(0.0)).child(self.field)),
        )
    }
}
