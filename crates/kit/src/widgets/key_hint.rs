//! A key and what it does ("Space  Next"): for views driven from the
//! keyboard because the hands are busy elsewhere (the palm trainer, on
//! the touchpad), where nothing should need the pointer. The key is a
//! raised cap with its name; what it does follows, dim while it does
//! nothing. Agents read it as a label, "Space: Next", and press the key.

use olo_interface::{Bounds, Element, FontWeight, IntoElement, SharedString, Styled as _, agent, canvas, point, px, size};
use olo_kit_base::metrics as m;

use crate::{style, theme};

pub struct KeyHint {
    key: SharedString,
    action: SharedString,
    enabled: bool,
    level_with: Option<f32>,
}

impl KeyHint {
    /// `key` as it's printed on the keyboard ("Space", "Esc", "R").
    pub fn new(key: impl Into<SharedString>, action: impl Into<SharedString>) -> KeyHint {
        KeyHint { key: key.into(), action: crate::words::of(action), enabled: true, level_with: None }
    }

    /// On the baseline of text of `size` points beside it, centred as
    /// it is (by its capitals, on this hint's middle), its cap moved down
    /// with it: a line of text and the hint after it read as one line
    /// (the command palette's query, 20 pt, and "Ctrl Files"; centred
    /// each by its own capitals they sat ~3 pt apart).
    pub fn level_with(mut self, size: f32) -> Self {
        self.level_with = Some(size);
        self
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

impl IntoElement for KeyHint {
    fn into_element(self) -> Element {
        let (bold, plain) = (style::ui_font(FontWeight::BOLD), style::ui_font(FontWeight::NORMAL));
        // At least square; a word gets the inset a field's text has.
        let w = (f32::from(style::text_width(self.key.clone(), style::SMALL_SIZE, bold.clone())) + 2.0 * m::TEXT_INSET).ceil().max(CAP);
        let w = (w / 2.0).ceil() * 2.0;
        let action_w = f32::from(style::text_width(self.action.clone(), style::PLAIN_SIZE, plain.clone())).ceil();
        let look = style::key((&self.key, &self.action, self.enabled, self.level_with.map(f32::to_bits)));
        let (key, action, enabled, level_with) = (self.key, self.action, self.enabled, self.level_with);
        let said = format!("{key}: {action}");
        // The cap and what it does in one, on one baseline: drawn apart,
        // each snapped on its own, they sat a device pixel apart at 1.25×
        // and 1.75× (the layout review's "almost level").
        canvas(move |g, b| {
            let p = theme::palette();
            let own = style::baseline_at(g, b.center().y, style::PLAIN_SIZE);
            let baseline = level_with.map_or(own, |s| style::baseline_at(g, b.center().y, s));
            let cap = style::snap(g, Bounds::new(point(b.left(), b.center().y + (baseline - own) - px(CAP / 2.0)), size(px(w), px(CAP))));
            style::fill_rect(g, cap, style::c(p.control_background));
            style::raised(g, cap, &p);
            let ink = if enabled { style::c(p.control_text) } else { style::disabled_text(&p) };
            style::label_on_baseline(g, cap.center().x, baseline, style::Align::Center, key.clone(), style::SMALL_SIZE, bold.clone(), ink);
            g.agent(agent::Node::label(said.clone(), cap));
            let ink = if enabled { style::c(p.panel_text) } else { style::disabled_text(&p) };
            let r = style::label_on_baseline(g, b.left() + px(w + m::MARK_GAP), baseline, style::Align::Left, action.clone(), style::PLAIN_SIZE, plain.clone(), ink);
            g.agent(agent::Node::label(action.clone(), r));
        })
        .paint_key(look)
        .w(px(w + m::MARK_GAP + action_w))
        .h(px(m::CONTROL_HEIGHT))
        .flex_shrink_0()
    }
}
