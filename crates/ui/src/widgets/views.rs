//! Text and grouping: BStringView (a line of text, or a paragraph) and
//! BBox (an etched frame with its label set into the top edge).

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
    wrap: bool,
    size: f32,
}

impl StringView {
    pub fn new(text: impl Into<SharedString>) -> StringView {
        StringView { text: text.into(), dim: false, bold: false, color: None, height: m::CONTROL_HEIGHT, wrap: false, size: style::PLAIN_SIZE }
    }

    /// In the small size (a table's column names, a scale's ends).
    pub fn small(mut self, on: bool) -> Self {
        self.size = if on { style::SMALL_SIZE } else { style::PLAIN_SIZE };
        self
    }

    /// At `points` (a full-screen view's title, `style::TITLE_SIZE`); a
    /// line of it is centred in a row of its own height
    /// (`style::line_row`) unless [`Self::height`] says another.
    pub fn size(mut self, points: f32) -> Self {
        self.size = points;
        self.height = self.height.max(style::line_row(points));
        self
    }

    /// A paragraph: wrapped to the width it's given, as tall as it takes.
    pub fn wrap(mut self, on: bool) -> Self {
        self.wrap = on;
        self
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
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let weight = if self.bold { FontWeight::BOLD } else { FontWeight::NORMAL };
        if self.wrap {
            let p = theme::palette(cx);
            let ink = self.color.unwrap_or_else(|| if self.dim { style::disabled_text(&p) } else { style::c(p.panel_text) });
            let text = self.text.clone();
            let said = canvas(|_, _, _| {}, move |b, _, window, cx| agent::record(window, cx, agent::Node::label(text.clone(), b))).absolute().inset_0();
            let mut d = div().relative().w_full().line_height(gpui::relative(style::LINE_HEIGHT)).font_weight(weight).text_color(ink);
            // (The window's own size unless small: nothing else changes.)
            if self.size != style::PLAIN_SIZE {
                d = d.text_size(px(self.size));
            }
            return d.child(self.text).child(said).into_any_element();
        }
        let size = self.size;
        let w = f32::from(style::text_width(window, self.text.clone(), size, style::ui_font(weight))).ceil();
        let (text, dim, color, height) = (self.text, self.dim, self.color, self.height);
        canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let ink = color.unwrap_or_else(|| if dim { style::disabled_text(&p) } else { style::c(p.panel_text) });
                let r = style::label(window, cx, b.left(), b.center().y, style::Align::Left, text.clone(), size, style::ui_font(weight), ink);
                agent::record(window, cx, agent::Node::label(text.clone(), r));
            },
        )
        .w(px(w))
        .h(px(height))
        .flex_shrink_0()
        .into_any_element()
    }
}

/// A bold section heading, as in preference panels.
pub fn heading(text: impl Into<SharedString>) -> StringView {
    StringView::new(text).bold(true).height(m::LABEL_ROW)
}

/// A groove across the width it's given: a dark line over a light one,
/// in a row six points tall.
#[derive(IntoElement)]
pub struct Separator;

/// A [`Separator`].
pub fn separator() -> Separator {
    Separator
}

impl RenderOnce for Separator {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        canvas(
            |_, _, _| {},
            |b, _, window, cx| {
                let p = theme::palette(cx);
                let y = style::snap_coord(window, b.center().y);
                style::hline_above(window, b.left(), b.right(), y, style::shade(&p, p.panel_background, 1.2));
                style::hline(window, b.left(), b.right(), y, style::c(p.shine));
            },
        )
        .w_full()
        .h(px(6.0))
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
                    // What the controls in it are about, for agents (and
                    // the settings the palette finds: Preferences' index).
                    crate::agent::record(window, cx, crate::agent::Node { role: "Group", name: Some(l.clone()), bounds: b, enabled: true, ..Default::default() });
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

/// What a view shows while it has nothing to show (no chat chosen, an
/// integration not set up): an icon (32 points) over a line in the
/// disabled text's colour, a smaller detail under it, and a button,
/// centred in what it's given.
#[derive(gpui::IntoElement)]
pub struct EmptyState {
    text: SharedString,
    icon: Option<SharedString>,
    detail: Option<SharedString>,
    action: Option<(SharedString, OnAction)>,
}

type OnAction = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

impl EmptyState {
    pub fn new(text: impl Into<SharedString>) -> EmptyState {
        EmptyState { text: text.into(), icon: None, detail: None, action: None }
    }

    pub fn icon(mut self, name: impl Into<SharedString>) -> Self {
        self.icon = Some(name.into());
        self
    }

    /// A line or two more, smaller, under the text.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// A button under it all ("Open Integrations…").
    pub fn action(mut self, label: impl Into<SharedString>, on_click: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.action = Some((label.into(), std::rc::Rc::new(on_click)));
        self
    }
}

impl gpui::RenderOnce for EmptyState {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = crate::theme::palette(cx);
        let dim = style::disabled_text(&p);
        let icon = self.icon.clone();
        let mut col = div().flex().flex_col().items_center().max_w(px(320.0)).gap(px(m::TIGHT)).text_color(dim);
        if let Some(name) = icon {
            col = col.child(
                gpui::canvas(|_, _, _| {}, move |b, _, window, _| {
                    crate::icons::paint(window, b, &name);
                })
                .w(px(32.0))
                .h(px(32.0))
                .mb(px(m::SPACING - m::TIGHT)),
            );
        }
        col = col.child(div().text_center().line_height(gpui::relative(style::LINE_HEIGHT)).child(self.text.clone()));
        if let Some(d) = self.detail.clone() {
            col = col.child(div().text_center().text_size(px(style::DETAIL_SIZE)).line_height(gpui::relative(style::LINE_HEIGHT)).child(d));
        }
        if let Some((label, f)) = self.action {
            col = col.child(div().mt(px(m::SPACING * 1.5 - m::TIGHT)).child(super::Button::new(label).on_click(move |window, cx| f(window, cx))));
        }
        div().size_full().flex().items_center().justify_center().px(px(m::SPACING)).child(col)
    }
}
