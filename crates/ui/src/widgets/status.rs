//! BStatusBar: a label and a trailing note over a sunken bar filled to a
//! fraction; [`level_bar`] is the bar alone (a
//! list cell's).

use gpui::{App, Bounds, FontWeight, Hsla, IntoElement, Pixels, RenderOnce, SharedString, Styled as _, Window, canvas, point, px};
use olo_kit_base::metrics as m;

use crate::{agent, style, theme};

#[derive(IntoElement)]
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

impl RenderOnce for StatusBar {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let top = if self.label.is_some() || self.trailing.is_some() { m::LABEL_ROW + m::TIGHT } else { 0.0 };
        let (label, trailing, fraction, color) = (self.label, self.trailing, self.fraction, self.color);
        canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let ink = style::c(p.panel_text);
                let font = style::ui_font(FontWeight::NORMAL);
                let y = b.top() + px(m::LABEL_ROW / 2.0);
                if let Some(l) = &label {
                    style::label(window, cx, b.left(), y, style::Align::Left, l.clone(), style::PLAIN_SIZE, font.clone(), ink);
                }
                if let Some(t) = &trailing {
                    style::label(window, cx, b.right(), y, style::Align::Right, t.clone(), style::PLAIN_SIZE, font.clone(), ink);
                }
                let bar = Bounds::from_corners(point(b.left(), b.top() + px(top)), b.bottom_right());
                level_bar(window, bar, fraction, color, &p);
                agent::record(
                    window,
                    cx,
                    agent::Node { role: "ProgressBar", name: label.clone(), value: Some(format!("{:.0}%", fraction.clamp(0.0, 1.0) * 100.0).into()), bounds: b, enabled: true, ..Default::default() },
                );
            },
        )
        .w_full()
        .h(px(top + self.height))
        .flex_shrink_0()
    }
}

/// The bar alone: a sunken well filled to `fraction` (with `fill`, or the
/// palette's status bar colour), glossed along the top.
pub fn level_bar(window: &mut Window, bar: Bounds<Pixels>, fraction: f32, fill: Option<Hsla>, p: &olo_theme::Palette) {
    let inner = style::inset(window, bar, 1.0);
    style::fill_rect(window, inner, style::c(p.document_background));
    let fill = fill.unwrap_or(style::c(p.status_bar));
    let f = fraction.clamp(0.0, 1.0);
    let right = style::snap_coord(window, inner.left() + inner.size.width * f);
    let done = Bounds::from_corners(inner.origin, point(right, inner.bottom()));
    if done.size.width > px(0.0) {
        // Darker towards the bottom: its sRGB values times 0.8.
        let rgb = gpui::Rgba::from(fill);
        let lower: Hsla = gpui::Rgba { r: rgb.r * 0.8, g: rgb.g * 0.8, b: rgb.b * 0.8, a: 1.0 }.into();
        style::vgradient(window, done, fill, lower);
        style::hline(window, done.left(), done.right(), done.top(), style::c(p.gloss));
    }
    style::bevel(window, bar, style::shade(p, p.panel_background, 1.45), style::c(p.shine));
}
