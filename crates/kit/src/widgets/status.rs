//! BStatusBar: a label and a trailing note over a sunken bar filled to a
//! fraction; [`level_bar`] is the bar alone (a list cell's). Where how
//! far along isn't known (something asked of another machine, and waited
//! for), the bar is [`StatusBar::busy`]: a block that goes along it and
//! back, moved by the app while it waits.

use olo_interface::{Bounds, Element, FontWeight, Hsla, IntoElement, Painter, Pixels, SharedString, Styled as _, agent, canvas, point, px};
use olo_kit_base::metrics as m;

use crate::{style, theme};

pub struct StatusBar {
    fraction: f32,
    label: Option<SharedString>,
    trailing: Option<SharedString>,
    color: Option<Hsla>,
    height: f32,
    /// Busy, and where its block is along its run.
    busy: Option<f32>,
}

/// How much of a busy bar its block takes.
const BUSY_BLOCK: f32 = 0.25;

impl StatusBar {
    pub fn new(fraction: f32) -> StatusBar {
        StatusBar { fraction, label: None, trailing: None, color: None, height: 14.0, busy: None }
    }

    /// A bar for work whose end isn't known: a block of its colour, a
    /// quarter of its length, at `phase` of its run there and back
    /// (whole numbers: at the left; halves: at the right). The app moves
    /// it while it waits, by a timer of its own a few times a second
    /// (`cx.every`, cancelled when the wait ends): nothing is drawn
    /// between its ticks, and nothing at all once it is over. The bar a
    /// stage of work has before its steps are counted, in the place its
    /// filled bar takes after.
    pub fn busy(phase: f32) -> StatusBar {
        StatusBar { busy: Some(phase), ..StatusBar::new(0.0) }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(crate::words::of(label));
        self
    }

    /// A note at the right ("3 of 7").
    pub fn trailing(mut self, text: impl Into<SharedString>) -> Self {
        self.trailing = Some(crate::words::of(text));
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
        let key = style::key((&self.label, &self.trailing, self.fraction.to_bits(), self.busy.map(f32::to_bits), style::ink_key(self.color)));
        let (label, trailing, fraction, color, busy) = (self.label, self.trailing, self.fraction, self.color, self.busy);
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
            let value: SharedString = match busy {
                Some(phase) => {
                    busy_bar(g, bar, phase, color, &p);
                    crate::words::tr("busy").into()
                }
                None => {
                    level_bar(g, bar, fraction, color, &p);
                    format!("{:.0}%", fraction.clamp(0.0, 1.0) * 100.0).into()
                }
            };
            g.agent(agent::Node { role: "ProgressBar", name: label.clone(), value: Some(value), bounds: b, enabled: true, ..Default::default() });
        })
        .paint_key(key)
        .w_full()
        .h(px(top + self.height))
        .flex_shrink_0()
    }
}

/// Where a busy bar's block is, as a part of the bar's length (its left
/// and its right), at `phase` of its run: there and back, a whole number
/// a turn.
pub fn busy_span(phase: f32) -> (f32, f32) {
    let turn = phase.rem_euclid(1.0);
    let along = 1.0 - (2.0 * turn - 1.0).abs();
    let left = along * (1.0 - BUSY_BLOCK);
    (left, left + BUSY_BLOCK)
}

/// A busy bar alone ([`StatusBar::busy`]): the well, and its block.
pub fn busy_bar(g: &mut Painter, bar: Bounds<Pixels>, phase: f32, fill: Option<Hsla>, p: &olo_theme::Palette) {
    let inner = style::inset(g, bar, 1.0);
    style::fill_rect(g, inner, style::c(p.document_background));
    let fill = fill.unwrap_or(style::c(p.status_bar));
    let (from, to) = busy_span(phase);
    let block =
        Bounds::from_corners(point(style::snap_coord(g, inner.left() + inner.size.width * from), inner.top()), point(style::snap_coord(g, inner.left() + inner.size.width * to), inner.bottom()));
    let rgb = olo_interface::Rgba::from(fill);
    let lower: Hsla = olo_interface::Rgba { r: rgb.r * 0.8, g: rgb.g * 0.8, b: rgb.b * 0.8, a: 1.0 }.into();
    style::vgradient(g, block, fill, lower);
    style::hline(g, block.left(), block.right(), block.top(), style::c(p.gloss));
    style::bevel(g, bar, style::shade(p, p.panel_background, 1.45), style::c(p.shine));
}

/// The bar alone: a sunken well filled to `fraction` (with `fill`, or the
/// palette's status bar colour), glossed along the top. What's filled.
pub fn level_bar(g: &mut Painter, bar: Bounds<Pixels>, fraction: f32, fill: Option<Hsla>, p: &olo_theme::Palette) -> Bounds<Pixels> {
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
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A busy bar's block goes from the bar's left to its right and
    /// back in a turn, a quarter of the bar long, never past either end.
    #[test]
    fn a_busy_block_goes_there_and_back() {
        let close = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() < 1e-5 && (a.1 - b.1).abs() < 1e-5;
        assert!(close(busy_span(0.0), (0.0, 0.25)));
        assert!(close(busy_span(0.25), (0.375, 0.625)));
        assert!(close(busy_span(0.5), (0.75, 1.0)));
        assert!(close(busy_span(0.75), (0.375, 0.625)));
        assert!(close(busy_span(1.0), (0.0, 0.25)));
        assert!(close(busy_span(7.5), (0.75, 1.0)));
        assert!(close(busy_span(-0.25), busy_span(0.75)));
        for k in 0..400 {
            let (from, to) = busy_span(k as f32 * 0.013);
            assert!(from >= 0.0 && to <= 1.0 + 1e-5 && (to - from - 0.25).abs() < 1e-5, "{from} to {to}");
        }
    }
}
