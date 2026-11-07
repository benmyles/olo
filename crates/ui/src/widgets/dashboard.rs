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
//! Card::new("cpu", "CPU").figure("12%").caption("12 cores, 2.4 GHz").child(Graph::new("cpu", series, now).plain())
//! ```

use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, Entity, FontWeight, HitboxBehavior, Hsla, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, RenderOnce, SharedString, Styled as _, Window,
    canvas, div, point, px, size,
};
use olo_kit_base::metrics as m;
use smallvec::SmallVec;

use crate::{agent, review, style, theme};

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

type ClickFn = Rc<dyn Fn(&mut Window, &mut App)>;

/// A card: its paper and head drawn behind what it holds (its children,
/// top to bottom, inside its inset).
#[derive(IntoElement)]
pub struct Card {
    id: SharedString,
    title: SharedString,
    figure: Option<SharedString>,
    caption: Option<SharedString>,
    selected: bool,
    figure_ink: Option<Hsla>,
    on_click: Option<ClickFn>,
    children: SmallVec<[AnyElement; 2]>,
}

struct CardState {
    hovered: bool,
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
    pub fn on_click(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    /// How tall a card is with `content` of this height under its head.
    pub fn height(content: f32, caption: bool) -> f32 {
        head(caption) + content + m::CARD_INSET
    }
}

impl ParentElement for Card {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Card {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Card { id, title, figure, caption, selected, figure_ink, on_click, children } = self;
        let state: Option<Entity<CardState>> = on_click.is_some().then(|| window.use_keyed_state(SharedString::from(format!("olo-card-{id}")), cx, |_, _| CardState { hovered: false }));
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
        let paper = canvas(
            {
                let clickable = on_click.is_some();
                move |b, window, _| clickable.then(|| window.insert_hitbox(b, HitboxBehavior::Normal))
            },
            {
                let (title, figure) = (title.clone(), figure.clone());
                move |b, hitbox, window, cx| {
                    let p = theme::palette(cx);
                    let hovered = hitbox.as_ref().is_some_and(|h| h.is_hovered(window));
                    style::card(window, b, &p, hovered, selected);
                    // A card is laid out by itself, not level with the one
                    // beside it.
                    review::own_grid(window, b);
                    if let (Some(h), Some(state), Some(click)) = (hitbox, state.clone(), on_click.clone()) {
                        agent::record(window, cx, agent::Node { value: figure.clone(), ..agent::Node::button(title.clone(), b) });
                        if state.read(cx).hovered != hovered {
                            state.update(cx, |s, _| s.hovered = hovered);
                        }
                        let hb = h.clone();
                        // (The view it's in drawn again, not the whole
                        // window: control.rs.)
                        let view = window.current_view();
                        window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                            if phase.bubble() && hb.is_hovered(window) != state.read(cx).hovered {
                                cx.notify(view);
                            }
                        });
                        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                            if phase.bubble() && e.button == MouseButton::Left && h.is_hovered(window) {
                                click(window, cx);
                            }
                        });
                    }
                }
            },
        )
        .absolute()
        .inset_0();
        // The head: laid out with what's under it, so its title starts
        // where they do on whatever device pixel layout puts them.
        let head_h = head(has_caption) - m::CARD_INSET;
        let head = canvas(
            |_, _, _| {},
            move |inner, _, window, cx| {
                let p = theme::palette(cx);
                let (ink, dim) = inks(&p);
                let (bold, plain) = (style::ui_font(FontWeight::BOLD), style::ui_font(FontWeight::NORMAL));
                let title_mid = inner.top() + px(TITLE_ROW / 2.0);
                let figure_w = figure.as_ref().map_or(0.0, |f| f32::from(style::text_width(window, f.clone(), FIGURE_SIZE, bold.clone())) + m::SPACING);
                let t = style::label_fit(window, cx, inner.left(), title_mid, style::Align::Left, title.clone(), style::PLAIN_SIZE, bold.clone(), ink, (inner.size.width - px(figure_w)).max(px(0.0)));
                agent::record(window, cx, agent::Node { role: "Heading", name: Some(title.clone()), value: figure.clone(), bounds: t, enabled: true, ..Default::default() });
                if let Some(f) = &figure {
                    let fink = figure_ink.filter(|_| !selected).unwrap_or(ink);
                    let baseline = style::baseline_at(window, title_mid, style::PLAIN_SIZE);
                    style::label_on_baseline(window, cx, inner.right(), baseline, style::Align::Right, f.clone(), FIGURE_SIZE, bold.clone(), fink);
                }
                if let Some(cap) = &caption {
                    let r = style::label_fit(
                        window,
                        cx,
                        inner.left(),
                        inner.top() + px(TITLE_ROW + CAPTION_ROW / 2.0),
                        style::Align::Left,
                        cap.clone(),
                        style::SMALL_SIZE,
                        plain.clone(),
                        dim,
                        inner.size.width,
                    );
                    agent::record(window, cx, agent::Node::label(cap.clone(), r));
                }
            },
        )
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
pub fn facts_label_width<'a>(cx: &App, labels: impl IntoIterator<Item = &'a str>) -> f32 {
    let font = style::ui_font(FontWeight::NORMAL);
    labels.into_iter().map(|l| style::text_width_app(cx, l, style::PLAIN_SIZE, &font)).fold(0.0, f32::max).ceil() + m::LABEL_GAP
}

pub fn facts_height(rows: usize) -> f32 {
    rows as f32 * FACT_ROW
}

/// Rows of label and value, the labels quiet in a column as wide as the
/// widest, values after them (cut short with "…" where they don't fit;
/// agents read all of each), [`FACT_ROW`] apart: on a card, in its ink.
#[derive(IntoElement)]
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

impl RenderOnce for Facts {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let Facts { rows, colors } = self;
        let h = facts_height(rows.len());
        canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let (dim, text) = colors.unwrap_or_else(|| {
                    let ink = style::card_ink(&p);
                    (ink.dim, ink.text)
                });
                let font = style::ui_font(FontWeight::NORMAL);
                let label_w = px(facts_label_width(cx, rows.iter().map(|(l, _)| l.as_ref())));
                window.with_content_mask(Some(gpui::ContentMask { bounds: b }), |window| {
                    for (i, (label, value)) in rows.iter().enumerate() {
                        let mid = b.top() + px(i as f32 * FACT_ROW + FACT_ROW / 2.0);
                        let l = style::label(window, cx, b.left(), mid, style::Align::Left, label.clone(), style::PLAIN_SIZE, font.clone(), dim);
                        let v = style::label_fit(window, cx, b.left() + label_w, mid, style::Align::Left, value.clone(), style::PLAIN_SIZE, font.clone(), text, (b.size.width - label_w).max(px(0.0)));
                        agent::record(window, cx, agent::Node { value: Some(value.clone()), ..agent::Node::label(label.clone(), l.union(&v)) });
                    }
                });
            },
        )
        .w_full()
        .h(px(h))
        .flex_shrink_0()
    }
}

/// A bar of parts of a whole (memory: used, cached, free), each in its
/// colour, the rest the well's; rounded at the ends.
pub fn meter(window: &mut Window, cx: &App, r: Bounds<Pixels>, parts: &[(Hsla, f64)], total: f64) {
    let p = theme::palette(cx);
    let ink = style::card_ink(&p);
    let r = style::snap(window, r);
    let radius = px(3.0);
    window.paint_quad(gpui::fill(r, ink.well).corner_radii(radius));
    if total <= 0.0 {
        return;
    }
    let s = window.scale_factor();
    let mut x = r.left();
    let mut sum = 0.0;
    for (color, v) in parts {
        sum += v.max(0.0);
        let end = px(((f32::from(r.left()) + (sum / total).clamp(0.0, 1.0) as f32 * f32::from(r.size.width)) * s).round() / s);
        if end > x {
            // The bar's ends are round; so are a part's that are its.
            let (left, right) = (if x <= r.left() { radius } else { px(0.0) }, if end >= r.right() - px(0.5 / s) { radius } else { px(0.0) });
            let corners = gpui::Corners { top_left: left, bottom_left: left, top_right: right, bottom_right: right };
            window.paint_quad(gpui::fill(Bounds::from_corners(point(x, r.top()), point(end, r.bottom())), *color).corner_radii(corners));
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
