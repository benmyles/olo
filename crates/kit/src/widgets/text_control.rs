//! BTextControl: an optional label and a sunken well holding a
//! [`TextField`], its text on the label's baseline; the well's inner line
//! turns the keyboard's colour while the field has it, and a click
//! anywhere in the well gives it the keyboard.

use std::hash::{Hash as _, Hasher as _};

use olo_interface::{Child, Element, FontWeight, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, SharedString, Styled as _, ViewRef, WindowCx, canvas, control_canvas, div, px};
use olo_kit_base::metrics as m;

use super::text_field::TextField;
use crate::{style, theme};

pub struct TextControl {
    field: ViewRef<TextField>,
    holds: Element,
    /// (As the field stood when its owner's `body` ran: an owner that
    /// turns its field off or on says so, `cx.notify()`.)
    enabled: bool,
    label: Option<SharedString>,
    divider: Option<f32>,
    width: Option<f32>,
}

impl TextControl {
    pub fn new(cx: &WindowCx, field: &Child<TextField>) -> TextControl {
        TextControl { field: field.downgrade(), holds: field.element(), enabled: field.read(cx).enabled(), label: None, divider: None, width: None }
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

impl IntoElement for TextControl {
    fn into_element(self) -> Element {
        let font = style::ui_font(FontWeight::NORMAL);
        let divider = self.label.as_ref().map(|l| self.divider.unwrap_or_else(|| f32::from(style::text_width(l.clone(), style::PLAIN_SIZE, font.clone())).ceil() + m::LABEL_GAP));
        // A field that can't be used (`TextField::set_enabled(false)`):
        // its label and well faded.
        let enabled = self.enabled;
        let mut row = div().relative().h(px(m::CONTROL_HEIGHT)).flex_shrink_0();
        row = match self.width {
            Some(w) => row.w(px(w)),
            None => row.w_full(),
        };
        if let Some(label) = self.label.clone() {
            let mut key = std::collections::hash_map::DefaultHasher::new();
            (&label, enabled).hash(&mut key);
            row = row.child(
                canvas(move |g, b| {
                    let p = theme::palette();
                    let ink = if enabled { style::c(p.panel_text) } else { style::disabled_text(&p) };
                    style::label(g, b.left(), b.center().y, style::Align::Left, label.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), ink);
                })
                .paint_key(key.finish())
                .absolute()
                .inset_0(),
            );
        }
        let field = self.field;
        // The well: drawn again, alone, when the keyboard comes to its
        // field or leaves it.
        let well = control_canvas(move |g, b, state| {
            let p = theme::palette();
            let fill = if enabled { style::c(p.document_background) } else { style::c(p.panel_background) };
            // (The field is seen by its frame: the review's gaps.)
            crate::review::frame(g, b);
            style::fill_rect(g, style::inset(g, b, 2.0), fill);
            g.with_element_opacity((!enabled).then_some(super::control::DISABLED_OPACITY), |g| style::sunken(g, b, &p, state.focused));
        })
        .paint_key(enabled as u64)
        .focus_within()
        .absolute()
        .left(px(divider.unwrap_or(0.0)))
        .right(px(0.0))
        .top(px(0.0))
        .bottom(px(0.0))
        .on_mouse_down(MouseButton::Left, move |_, cx| {
            if enabled {
                field.focus(cx);
            }
        })
        .child(div().absolute().left(px(m::TEXT_INSET)).right(px(m::TEXT_INSET)).top(px(0.0)).bottom(px(0.0)).child(self.holds));
        row.child(well)
    }
}
