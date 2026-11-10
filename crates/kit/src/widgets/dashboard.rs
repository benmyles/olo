//! Dashboards (Activity Monitor's sections; any app's overview): cards
//! on the card gutter, each with a title, a figure at
//! its right ("12.3%") and a quiet caption under the title, then what it
//! shows: a [`Graph`](super::Graph) (`.plain()`, without its well),
//! [`Facts`] (label: value rows), a [`legend`](super::legend), a
//! [`meter`] of parts of a whole. [`flow`] places cards in as many
//! columns as fit, for a dashboard laid out by hand; in a flex row that
//! wraps, a card with `flex_basis` half the row and a `min_w` goes two
//! abreast when there's room, and cards side by side are as tall as the
//! tallest. Agents read each card's title as a heading and its facts as
//! labels; a clickable card is a button named by its title.
//!
//! ```ignore
//! Card::new("cpu", "CPU").figure("12%").caption("12 cores, 2.4 GHz").child(Graph::live("cpu", series, clock).plain())
//! ```

use std::rc::Rc;

use olo_interface::{
    Bounds, Element, FontWeight, Hsla, InteractiveElement as _, IntoElement, MouseButton, Painter, ParentElement, Pixels, SharedString, Styled as _, WindowCx, agent, canvas, control_canvas, div,
    point, px, size,
};
use olo_kit_base::metrics as m;
use smallvec::SmallVec;

use crate::{review, style, theme};

/// Between cards, and round them on the gutter.
pub const GAP: f32 = 10.0;
/// The title's row.
pub const TITLE_ROW: f32 = 20.0;
/// The caption's row.
pub const CAPTION_ROW: f32 = 16.0;
/// A figure's size: bigger than a title, for reading across a room.
pub const FIGURE_SIZE: f32 = 18.0;
pub use super::charts::FACT_ROW;

/// The gutter cards stand on.
pub fn gutter(p: &olo_theme::Palette) -> olo_theme::Rgba {
    style::card_paper(p).0
}

/// How far a card's content is from its top.
fn head(caption: bool) -> f32 {
    m::CARD_INSET + TITLE_ROW + if caption { CAPTION_ROW } else { 0.0 } + m::TIGHT
}

type ClickFn = Rc<dyn Fn(&(), &mut WindowCx)>;

/// A card: its paper and head drawn behind what it holds (its children,
/// top to bottom, inside its inset).
pub struct Card {
    id: SharedString,
    title: SharedString,
    figure: Option<SharedString>,
    caption: Option<SharedString>,
    selected: bool,
    figure_ink: Option<Hsla>,
    on_click: Option<ClickFn>,
    children: SmallVec<[Element; 2]>,
}

impl Card {
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Card {
        Card { id: id.into(), title: title.into(), figure: None, caption: None, selected: false, figure_ink: None, on_click: None, children: SmallVec::new() }
    }

    pub fn figure(mut self, figure: impl Into<SharedString>) -> Self {
        self.figure = Some(figure.into());
        self
    }

    pub fn caption(mut self, caption: impl Into<SharedString>) -> Self {
        let caption = caption.into();
        self.caption = (!caption.is_empty()).then_some(caption);
        self
    }

    pub fn selected(mut self, on: bool) -> Self {
        self.selected = on;
        self
    }

    /// The figure in a colour of its own (the palette's `warning` or
    /// `failure`, for a card that needs attention); a selected card's is
    /// its text's.
    pub fn figure_ink(mut self, ink: Hsla) -> Self {
        self.figure_ink = Some(ink);
        self
    }

    /// It takes clicks (lifts under the pointer): a way into more.
    pub fn on_click(mut self, f: impl Fn(&(), &mut WindowCx) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    /// How tall a card is with `content` of this height under its head.
    pub fn height(content: f32, caption: bool) -> f32 {
        head(caption) + content + m::CARD_INSET
    }
}

impl ParentElement for Card {
    fn extend(&mut self, elements: impl IntoIterator<Item = Element>) {
        self.children.extend(elements);
    }
}

impl IntoElement for Card {
    fn into_element(self) -> Element {
        let Card { id, title, figure, caption, selected, figure_ink, on_click, children } = self;
        let has_caption = caption.is_some();
        // The inks: the card's, or the selection's.
        let inks = move |p: &olo_theme::Palette| {
            if selected {
                let text = style::c(p.list_selected_item_text);
                (text, text.opacity(150.0 / 255.0))
            } else {
                let ink = style::card_ink(p);
                (ink.text, ink.dim)
            }
        };
        // Its paper: one that takes clicks lifts under the pointer (it
        // alone is drawn again), and is a button to agents.
        let paper = match on_click {
            Some(click) => {
                let (title, figure) = (title.clone(), figure.clone());
                control_canvas(move |g, b, state| {
                    let p = theme::palette();
                    style::card(g, b, &p, state.hovered, selected);
                    review::own_grid(g, b);
                    g.agent(agent::Node { value: figure.clone(), ..agent::Node::button(title.clone(), b) });
                })
                .id(id)
                .on_mouse_down(MouseButton::Left, move |_, cx| click(&(), cx))
            }
            None => canvas(move |g, b| {
                let p = theme::palette();
                style::card(g, b, &p, false, selected);
                // A card is laid out by itself, not level with the one
                // beside it.
                review::own_grid(g, b);
            }),
        }
        .paint_key(style::key((&title, &figure, selected)))
        .absolute()
        .inset_0();
        // The head: laid out with what's under it, so its title starts
        // where they do on whatever device pixel layout puts them.
        let head_h = head(has_caption) - m::CARD_INSET;
        let head_key = style::key((&title, &figure, &caption, selected, style::ink_key(figure_ink)));
        let head = canvas(move |g, inner| {
            let p = theme::palette();
            let (ink, dim) = inks(&p);
            let (bold, plain) = (style::ui_font(FontWeight::BOLD), style::ui_font(FontWeight::NORMAL));
            let title_mid = inner.top() + px(TITLE_ROW / 2.0);
            let figure_w = figure.as_ref().map_or(0.0, |f| f32::from(style::text_width(f.clone(), FIGURE_SIZE, bold.clone())) + m::SPACING);
            let t = style::label_fit(g, inner.left(), title_mid, style::Align::Left, title.clone(), style::PLAIN_SIZE, bold.clone(), ink, (inner.size.width - px(figure_w)).max(px(0.0)));
            g.agent(agent::Node { role: "Heading", name: Some(title.clone()), value: figure.clone(), bounds: t, enabled: true, ..Default::default() });
            if let Some(f) = &figure {
                let fink = figure_ink.filter(|_| !selected).unwrap_or(ink);
                let baseline = style::baseline_at(g, title_mid, style::PLAIN_SIZE);
                style::label_on_baseline(g, inner.right(), baseline, style::Align::Right, f.clone(), FIGURE_SIZE, bold.clone(), fink);
            }
            if let Some(cap) = &caption {
                let r = style::label_fit(g, inner.left(), inner.top() + px(TITLE_ROW + CAPTION_ROW / 2.0), style::Align::Left, cap.clone(), style::SMALL_SIZE, plain.clone(), dim, inner.size.width);
                g.agent(agent::Node::label(cap.clone(), r));
            }
        })
        .paint_key(head_key)
        .w_full()
        .h(px(head_h))
        .flex_shrink_0();
        // (It grows to fill its place: cards side by side are as tall
        // as the tallest.)
        div().relative().flex().flex_col().flex_grow().min_h(px(Card::height(0.0, has_caption))).p(px(m::CARD_INSET)).child(paper).child(head).children(children)
    }
}

/// How far facts' values stand from their labels' left: the widest
/// label and a gap (to line a control up with the values).
pub fn facts_label_width<'a>(labels: impl IntoIterator<Item = &'a str>) -> f32 {
    let font = style::ui_font(FontWeight::NORMAL);
    labels.into_iter().map(|l| style::text_width_app(l, style::PLAIN_SIZE, &font)).fold(0.0, f32::max).ceil() + m::LABEL_GAP
}

pub fn facts_height(rows: usize) -> f32 {
    rows as f32 * FACT_ROW
}

/// Rows of label and value, the labels quiet in a column as wide as the
/// widest, values after them (cut short with "…" where they don't fit;
/// agents read all of each), [`FACT_ROW`] apart: on a card, in its ink.
pub struct Facts {
    rows: Rc<[(SharedString, SharedString)]>,
    colors: Option<(Hsla, Hsla)>,
}

impl Facts {
    pub fn new(rows: impl Into<Rc<[(SharedString, SharedString)]>>) -> Facts {
        Facts { rows: rows.into(), colors: None }
    }

    /// On a panel rather than a card: its labels' colour and its values'
    /// (the panel's disabled text and its text).
    pub fn colors(mut self, label: Hsla, value: Hsla) -> Self {
        self.colors = Some((label, value));
        self
    }
}

impl IntoElement for Facts {
    fn into_element(self) -> Element {
        let Facts { rows, colors } = self;
        let h = facts_height(rows.len());
        let key = style::key((&rows, colors.map(|(label, value)| (style::ink_key(Some(label)), style::ink_key(Some(value))))));
        canvas(move |g, b| {
            let p = theme::palette();
            let (dim, text) = colors.unwrap_or_else(|| {
                let ink = style::card_ink(&p);
                (ink.dim, ink.text)
            });
            let font = style::ui_font(FontWeight::NORMAL);
            let label_w = px(facts_label_width(rows.iter().map(|(l, _)| l.as_ref())));
            g.with_content_mask(Some(olo_interface::ContentMask { bounds: b }), |g| {
                for (i, (label, value)) in rows.iter().enumerate() {
                    let mid = b.top() + px(i as f32 * FACT_ROW + FACT_ROW / 2.0);
                    let l = style::label(g, b.left(), mid, style::Align::Left, label.clone(), style::PLAIN_SIZE, font.clone(), dim);
                    let v = style::label_fit(g, b.left() + label_w, mid, style::Align::Left, value.clone(), style::PLAIN_SIZE, font.clone(), text, (b.size.width - label_w).max(px(0.0)));
                    g.agent(agent::Node { value: Some(value.clone()), ..agent::Node::label(label.clone(), l.union(&v)) });
                }
            });
        })
        .paint_key(key)
        .w_full()
        .h(px(h))
        .flex_shrink_0()
    }
}

/// A bar of parts of a whole (memory: used, cached, free), each in its
/// colour, the rest the well's; rounded at the ends.
pub fn meter(g: &mut Painter, r: Bounds<Pixels>, parts: &[(Hsla, f64)], total: f64) {
    let p = theme::palette();
    let ink = style::card_ink(&p);
    let r = style::snap(g, r);
    let radius = px(3.0);
    g.paint_quad(olo_interface::fill(r, ink.well).corner_radii(radius));
    if total <= 0.0 {
        return;
    }
    let s = g.scale_factor();
    let mut x = r.left();
    let mut sum = 0.0;
    for (color, v) in parts {
        sum += v.max(0.0);
        let end = px(((f32::from(r.left()) + (sum / total).clamp(0.0, 1.0) as f32 * f32::from(r.size.width)) * s).round() / s);
        if end > x {
            // The bar's ends are round; so are a part's that are its.
            let (left, right) = (if x <= r.left() { radius } else { px(0.0) }, if end >= r.right() - px(0.5 / s) { radius } else { px(0.0) });
            let corners = olo_interface::Corners { top_left: left, bottom_left: left, top_right: right, bottom_right: right };
            g.paint_quad(olo_interface::fill(Bounds::from_corners(point(x, r.top()), point(end, r.bottom())), *color).corner_radii(corners));
        }
        x = end;
    }
}

/// Places for `n` cards in `area`: as many columns of at least
/// `min_width` as fit (at most `max_columns`), `height` tall, [`GAP`]
/// apart.
pub fn flow(area: Bounds<Pixels>, n: usize, min_width: f32, max_columns: usize, height: f32) -> Vec<Bounds<Pixels>> {
    let width = f32::from(area.size.width);
    let cols = (((width + GAP) / (min_width + GAP)).floor() as usize).clamp(1, max_columns.max(1));
    let w = ((width - GAP * (cols - 1) as f32) / cols as f32).floor();
    (0..n)
        .map(|i| {
            let (col, row) = (i % cols, i / cols);
            let x = area.left() + px(col as f32 * (w + GAP));
            // The last column takes what rounding left.
            let w = if col == cols - 1 { area.right() - x } else { px(w) };
            Bounds { origin: point(x, area.top() + px(row as f32 * (height + GAP))), size: size(w, px(height)) }
        })
        .collect()
}

/// How many rows [`flow`] makes.
pub fn flow_rows(width: f32, n: usize, min_width: f32, max_columns: usize) -> usize {
    let cols = (((width + GAP) / (min_width + GAP)).floor() as usize).clamp(1, max_columns.max(1));
    n.div_ceil(cols)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flows_into_columns() {
        let area = Bounds { origin: point(px(0.0), px(0.0)), size: size(px(620.0), px(1000.0)) };
        let r = flow(area, 5, 200.0, 3, 100.0);
        assert_eq!(r.len(), 5);
        // Three columns of 200 fit in 620 with the gaps (610: two).
        assert_eq!(r[2].right(), px(620.0));
        assert_eq!(r[3].top(), px(110.0));
        assert_eq!(flow_rows(620.0, 5, 200.0, 3), 2);
        assert_eq!(flow_rows(610.0, 5, 200.0, 3), 3);
        assert_eq!(flow_rows(300.0, 5, 200.0, 3), 5);
    }
}
