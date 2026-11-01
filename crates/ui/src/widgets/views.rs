//! Text and grouping: BStringView (a line of text, or a paragraph) and
//! BBox (an etched frame with its label set into the top edge), drawn as
//! the egui kit draws them.

use gpui::{AnyElement, App, Bounds, FontWeight, Hsla, IntoElement, ParentElement, Pixels, RenderOnce, SharedString, Styled as _, Window, canvas, div, point, px};
use olo_kit_base::metrics as m;
use smallvec::SmallVec;

use crate::{agent, style, theme};

/// BStringView: a line of plain text, its capitals centred in a control's
/// height (so it lines up with the controls beside it).
#[derive(IntoElement)]
pub struct StringView {
    text: SharedString,
    dim: bool,
    bold: bool,
    color: Option<Hsla>,
    height: f32,
}

impl StringView {
    pub fn new(text: impl Into<SharedString>) -> StringView {
        StringView { text: text.into(), dim: false, bold: false, color: None, height: m::CONTROL_HEIGHT }
    }

    /// The row it's centred in (a control's height unless said: a
    /// note's `LABEL_ROW`).
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }

    /// Secondary text (status, hints), in the disabled text's colour.
    pub fn dim(mut self, on: bool) -> Self {
        self.dim = on;
        self
    }

    pub fn bold(mut self, on: bool) -> Self {
        self.bold = on;
        self
    }

    /// A colour of its own (from the palette: an error in `failure`).
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for StringView {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let weight = if self.bold { FontWeight::BOLD } else { FontWeight::NORMAL };
        let w = f32::from(style::text_width(window, self.text.clone(), style::PLAIN_SIZE, style::ui_font(weight))).ceil();
        let (text, dim, color, height) = (self.text, self.dim, self.color, self.height);
        canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let ink = color.unwrap_or_else(|| if dim { style::disabled_text(&p) } else { style::c(p.panel_text) });
                let r = style::label(window, cx, b.left(), b.center().y, style::Align::Left, text.clone(), style::PLAIN_SIZE, style::ui_font(weight), ink);
                agent::record(window, cx, agent::Node::label(text.clone(), r));
            },
        )
        .w(px(w))
        .h(px(height))
        .flex_shrink_0()
    }
}

/// BBox: what it holds in an etched frame, its label (bold) set into the
/// frame's top line. Items in it stack a little tighter.
#[derive(IntoElement)]
pub struct BBox {
    label: Option<SharedString>,
    width: Option<f32>,
    children: SmallVec<[AnyElement; 4]>,
}

impl BBox {
    pub fn new(label: impl Into<SharedString>) -> BBox {
        BBox { label: Some(label.into()), width: None, children: SmallVec::new() }
    }

    /// Without a label.
    pub fn plain() -> BBox {
        BBox { label: None, width: None, children: SmallVec::new() }
    }

    /// What it holds is `w` wide (the frame, its insets wider).
    pub fn content_width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }
}

impl ParentElement for BBox {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for BBox {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let label_h = if self.label.is_some() { m::LABEL_ROW } else { 0.0 };
        let pad = m::BOX_INSET;
        let content_top = if self.label.is_some() { label_h + m::TIGHT } else { pad };
        let label_w = self.label.as_ref().map(|l| f32::from(style::text_width(window, l.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD))).ceil());
        let label = self.label.clone();
        let frame = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                // The frame's top line runs through the middle of the
                // label's row.
                let top = px((label_h / 2.0).floor());
                let frame = Bounds::from_corners(point(b.left(), b.top() + top), b.bottom_right());
                etched(window, frame, &p, label_w.map(|w| (frame.left() + px(pad - 4.0), frame.left() + px(pad + w + 4.0))));
                if let Some(l) = &label {
                    style::label(window, cx, b.left() + px(pad), b.top() + px(label_h / 2.0), style::Align::Left, l.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), style::c(p.panel_text));
                }
            },
        )
        .absolute()
        .inset_0();
        let mut b = div().relative();
        if let Some(w) = self.width {
            b = b.w(px(w + 2.0 * pad));
        }
        b.flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .pt(px(content_top))
            .px(px(pad))
            .pb(px(pad))
            .child(frame)
            .children(self.children)
    }
}

/// The etched rectangle: a dark line, then a light one a line in, on
/// device pixels at any scale; `gap` leaves room in the top line.
fn etched(window: &mut Window, frame: Bounds<Pixels>, p: &olo_theme::Palette, gap: Option<(Pixels, Pixels)>) {
    let dark = style::shade(p, p.panel_background, 1.22);
    let light = style::c(p.shine);
    let (f, i1, i2) = (style::inset(window, frame, 0.0), style::inset(window, frame, 1.0), style::inset(window, frame, 2.0));
    let top = |window: &mut Window, y: Pixels, color: Hsla| match gap {
        Some((a, b)) => {
            style::hline(window, f.left(), a, y, color);
            style::hline(window, b, f.right(), y, color);
        }
        None => style::hline(window, f.left(), f.right(), y, color),
    };
    top(window, f.top(), dark);
    top(window, i1.top(), light);
    style::hline_above(window, f.left(), f.right(), i1.bottom(), dark);
    style::hline_above(window, f.left(), f.right(), f.bottom(), light);
    style::vline(window, f.left(), f.top(), i1.bottom(), dark);
    style::vline(window, i1.left(), i1.top(), i2.bottom(), light);
    style::vline_left_of(window, i1.right(), f.top(), i1.bottom(), dark);
    style::vline_left_of(window, f.right(), f.top(), f.bottom(), light);
}
