//! BStatusBar: a label and a trailing note over a sunken bar filled to a
//! fraction; [`level_bar`] is the bar alone (a list cell's).

use olo_interface::{Bounds, Element, FontWeight, Hsla, IntoElement, Painter, Pixels, SharedString, Styled as _, agent, canvas, point, px};
use olo_kit_base::metrics as m;

use crate::{style, theme};

pub struct StatusBar {
    fraction: f32,
    label: Option<SharedString>,
    trailing: Option<SharedString>,
    color: Option<Hsla>,
    height: f32,
}

impl StatusBar {
    pub fn new(fraction: f32) -> StatusBar {
        StatusBar { fraction, label: None, trailing: None, color: None, height: 14.0 }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// A note at the right ("3 of 7").
    pub fn trailing(mut self, text: impl Into<SharedString>) -> Self {
        self.trailing = Some(text.into());
        self
    }

    /// The fill's colour (else the palette's status bar colour).
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    pub fn bar_height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
}

impl IntoElement for StatusBar {
    fn into_element(self) -> Element {
        let top = if self.label.is_some() || self.trailing.is_some() { m::LABEL_ROW + m::TIGHT } else { 0.0 };
        let key = style::key((&self.label, &self.trailing, self.fraction.to_bits(), style::ink_key(self.color)));
        let (label, trailing, fraction, color) = (self.label, self.trailing, self.fraction, self.color);
        canvas(move |g, b| {
            let p = theme::palette();
            let ink = style::c(p.panel_text);
            let font = style::ui_font(FontWeight::NORMAL);
            let y = b.top() + px(m::LABEL_ROW / 2.0);
            // The note whole; the label cut short ("…", its whole in a
            // tip) to the room it leaves: a name that changes as work goes
            // on never runs into it, nor makes the bar's box wider.
            let room = match &trailing {
                Some(t) => style::label(g, b.right(), y, style::Align::Right, t.clone(), style::PLAIN_SIZE, font.clone(), ink).left() - px(m::SPACING),
                None => b.right(),
            };
            if let Some(l) = &label {
                style::label_fit(g, b.left(), y, style::Align::Left, l.clone(), style::PLAIN_SIZE, font.clone(), ink, room - b.left());
            }
            let bar = Bounds::from_corners(point(b.left(), b.top() + px(top)), b.bottom_right());
            level_bar(g, bar, fraction, color, &p);
            g.agent(agent::Node { role: "ProgressBar", name: label.clone(), value: Some(format!("{:.0}%", fraction.clamp(0.0, 1.0) * 100.0).into()), bounds: b, enabled: true, ..Default::default() });
        })
        .paint_key(key)
        .w_full()
        .h(px(top + self.height))
        .flex_shrink_0()
    }
}

/// The bar alone: a sunken well filled to `fraction` (with `fill`, or the
/// palette's status bar colour), glossed along the top.
pub fn level_bar(g: &mut Painter, bar: Bounds<Pixels>, fraction: f32, fill: Option<Hsla>, p: &olo_theme::Palette) {
    let inner = style::inset(g, bar, 1.0);
    style::fill_rect(g, inner, style::c(p.document_background));
    let fill = fill.unwrap_or(style::c(p.status_bar));
    let f = fraction.clamp(0.0, 1.0);
    let right = style::snap_coord(g, inner.left() + inner.size.width * f);
    let done = Bounds::from_corners(inner.origin, point(right, inner.bottom()));
    if done.size.width > px(0.0) {
        // Darker towards the bottom: its sRGB values times 0.8.
        let rgb = olo_interface::Rgba::from(fill);
        let lower: Hsla = olo_interface::Rgba { r: rgb.r * 0.8, g: rgb.g * 0.8, b: rgb.b * 0.8, a: 1.0 }.into();
        style::vgradient(g, done, fill, lower);
        style::hline(g, done.left(), done.right(), done.top(), style::c(p.gloss));
    }
    style::bevel(g, bar, style::shade(p, p.panel_background, 1.45), style::c(p.shine));
}
