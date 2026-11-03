//! A key and what it does ("Space  Next"): for views driven from the
//! keyboard because the hands are busy elsewhere (the palm trainer, on
//! the touchpad), where nothing should need the pointer. The key is a
//! raised cap with its name; what it does follows, dim while it does
//! nothing. Agents read it as a label, "Space: Next", and press the key.

use gpui::{App, FontWeight, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _, Window, canvas, div, px};
use olo_kit_base::metrics as m;

use crate::{agent, style, theme, widgets::StringView};

#[derive(IntoElement)]
pub struct KeyHint {
    key: SharedString,
    action: SharedString,
    enabled: bool,
}

impl KeyHint {
    /// `key` as it's printed on the keyboard ("Space", "Esc", "R").
    pub fn new(key: impl Into<SharedString>, action: impl Into<SharedString>) -> KeyHint {
        KeyHint { key: key.into(), action: action.into(), enabled: true }
    }

    /// Whether the key does anything now (dim if not).
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
}

/// The cap's height: a check box's row, so a row of hints sits as one
/// of check boxes does.
const CAP: f32 = m::MARK_ROW;

impl RenderOnce for KeyHint {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let font = style::ui_font(FontWeight::BOLD);
        // At least square; a word gets the inset a field's text has.
        let w = (f32::from(style::text_width(window, self.key.clone(), style::SMALL_SIZE, font.clone())) + 2.0 * m::TEXT_INSET).ceil().max(CAP);
        let w = (w / 2.0).ceil() * 2.0;
        let (key, action, enabled) = (self.key, self.action, self.enabled);
        let said = format!("{key}: {action}");
        let cap = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let r = style::snap(window, b);
                style::fill_rect(window, r, style::c(p.control_background));
                style::raised(window, r, &p);
                let ink = if enabled { style::c(p.control_text) } else { style::disabled_text(&p) };
                style::label(window, cx, r.center().x, r.center().y, style::Align::Center, key.clone(), style::SMALL_SIZE, font.clone(), ink);
                agent::record(window, cx, agent::Node::label(said.clone(), b));
            },
        )
        .w(px(w))
        .h(px(CAP))
        .flex_shrink_0();
        div().flex().flex_row().items_center().gap(px(m::MARK_GAP)).h(px(m::CONTROL_HEIGHT)).child(cap).child(StringView::new(action).dim(!enabled))
    }
}
