//! A key and what it does ("Space  Next"): for views driven from the
//! keyboard because the hands are busy elsewhere (the palm trainer, on
//! the touchpad), where nothing should need the pointer. The key is a
//! raised cap with its name; what it does follows, dim while it does
//! nothing. Agents read it as a label, "Space: Next", and press the key.

use gpui::{App, Bounds, FontWeight, IntoElement, RenderOnce, SharedString, Styled as _, Window, canvas, point, px, size};
use olo_kit_base::metrics as m;

use crate::{agent, style, theme};

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
        let (bold, plain) = (style::ui_font(FontWeight::BOLD), style::ui_font(FontWeight::NORMAL));
        // At least square; a word gets the inset a field's text has.
        let w = (f32::from(style::text_width(window, self.key.clone(), style::SMALL_SIZE, bold.clone())) + 2.0 * m::TEXT_INSET).ceil().max(CAP);
        let w = (w / 2.0).ceil() * 2.0;
        let action_w = f32::from(style::text_width(window, self.action.clone(), style::PLAIN_SIZE, plain.clone())).ceil();
        let (key, action, enabled) = (self.key, self.action, self.enabled);
        let said = format!("{key}: {action}");
        // The cap and what it does in one, on one baseline: drawn apart,
        // each snapped on its own, they sat a device pixel apart at 1.25×
        // and 1.75× (the layout review's "almost level").
        canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let cap = style::snap(window, Bounds::new(point(b.left(), b.center().y - px(CAP / 2.0)), size(px(w), px(CAP))));
                style::fill_rect(window, cap, style::c(p.control_background));
                style::raised(window, cap, &p);
                let baseline = style::baseline_at(window, b.center().y, style::PLAIN_SIZE);
                let ink = if enabled { style::c(p.control_text) } else { style::disabled_text(&p) };
                style::label_on_baseline(window, cx, cap.center().x, baseline, style::Align::Center, key.clone(), style::SMALL_SIZE, bold.clone(), ink);
                agent::record(window, cx, agent::Node::label(said.clone(), cap));
                let ink = if enabled { style::c(p.panel_text) } else { style::disabled_text(&p) };
                let r = style::label_on_baseline(window, cx, b.left() + px(w + m::MARK_GAP), baseline, style::Align::Left, action.clone(), style::PLAIN_SIZE, plain.clone(), ink);
                agent::record(window, cx, agent::Node::label(action.clone(), r));
            },
        )
        .w(px(w + m::MARK_GAP + action_w))
        .h(px(m::CONTROL_HEIGHT))
        .flex_shrink_0()
    }
}
