//! Dashboards (Activity Monitor's sections; any app's overview): cards on
//! the card gutter, each with a title, a figure at its right ("12.3%") and
//! a quiet caption under the title, then what it shows: a [`Graph`]
//! (plain, without its well), [`facts`] (label: value rows), a [`legend`]
//! of swatches and values, a [`meter`] of parts of a whole. [`flow`] lays
//! cards out in as many columns as fit. Agents read each card's title as
//! a heading and everything in it as labels; a clickable card is a button
//! named by its title.
//!
//! [`Graph`]: crate::graph::Graph
//!
//! ```ignore
//! let card = Card::new("CPU").figure("12%").caption("12 cores, 2.4 GHz").clickable(true).show(ui, rect);
//! Graph::new("cpu", &series, now).plain().show(ui, card.content);
//! ```

use egui::{Align, Color32, FontId, Rect, Sense, Ui, pos2, vec2};

use crate::{
    metrics as m,
    style::{self, c},
};

/// Between cards, and round them on the gutter.
pub const GAP: f32 = 10.0;
/// The title's row.
pub const TITLE_ROW: f32 = 20.0;
/// The caption's row.
pub const CAPTION_ROW: f32 = 16.0;
/// A figure's size: bigger than a title, for reading across a room.
pub const FIGURE_SIZE: f32 = 18.0;
/// A fact's row, and a legend's.
pub const FACT_ROW: f32 = 18.0;

pub struct Card<'a> {
    title: &'a str,
    figure: Option<&'a str>,
    caption: Option<&'a str>,
    clickable: bool,
    selected: bool,
}

pub struct CardResponse {
    /// Where its content goes.
    pub content: Rect,
    pub clicked: bool,
    pub hovered: bool,
}

impl<'a> Card<'a> {
    pub fn new(title: &'a str) -> Self {
        Card { title, figure: None, caption: None, clickable: false, selected: false }
    }
    pub fn figure(mut self, figure: &'a str) -> Self {
        self.figure = Some(figure);
        self
    }
    pub fn caption(mut self, caption: &'a str) -> Self {
        self.caption = (!caption.is_empty()).then_some(caption);
        self
    }
    /// It takes clicks (lifts under the pointer): a way into more.
    pub fn clickable(mut self, on: bool) -> Self {
        self.clickable = on;
        self
    }
    pub fn selected(mut self, on: bool) -> Self {
        self.selected = on;
        self
    }

    /// How tall a card is with `content` of this height under its head.
    pub fn height(content: f32, caption: bool) -> f32 {
        head(caption) + content + m::CARD_INSET
    }

    pub fn show(self, ui: &mut Ui, rect: Rect) -> CardResponse {
        let p = style::palette(ui.ctx());
        let resp = self.clickable.then(|| ui.interact(rect, egui::Id::unique(("olo-card", self.title, rect.min.x as i32, rect.min.y as i32)), Sense::click()));
        let hovered = resp.as_ref().is_some_and(|r| r.hovered());
        style::card(ui.painter(), rect, &p, hovered, self.selected);
        // A card is laid out by itself, not level with the one beside it.
        crate::layout_review::own_grid(ui, rect);
        let ink = if self.selected { style::selection_ink(&p).text } else { style::card_ink(&p).text };
        let dim = if self.selected { style::selection_ink(&p).dim } else { style::card_ink(&p).dim };
        let painter = ui.painter().with_clip_rect(rect);
        let inner = rect.shrink(m::CARD_INSET);
        let title_mid = inner.top() + TITLE_ROW / 2.0;
        let figure_w = self.figure.map_or(0.0, |f| style::label_width(ui.ctx(), f, figure_font()) + m::SPACING);
        let (title_r, baseline) =
            style::label_fit_with_baseline(&painter, inner.left(), title_mid, Align::Min, self.title, style::bold_font(), ink, (inner.width() - figure_w).max(0.0));
        style::describe_text(ui.ctx(), egui::accesskit::Role::Heading, self.title, title_r, None);
        if let Some(f) = self.figure {
            style::label_on_baseline(&painter, inner.right(), baseline, Align::Max, f, figure_font(), ink);
        }
        if let Some(cap) = self.caption {
            style::label_fit(&painter, inner.left(), inner.top() + TITLE_ROW + CAPTION_ROW / 2.0, Align::Min, cap, style::small_font(), dim, inner.width());
        }
        let content = Rect::from_min_max(pos2(inner.left(), rect.top() + head(self.caption.is_some())), inner.max);
        if let Some(r) = &resp {
            r.widget_info(|| egui::WidgetInfo { current_text_value: self.figure.map(str::to_owned), ..egui::WidgetInfo::labeled(egui::Role::Button, true, self.title) });
        }
        CardResponse { content, clicked: resp.is_some_and(|r| r.clicked()), hovered }
    }
}

fn head(caption: bool) -> f32 {
    m::CARD_INSET + TITLE_ROW + if caption { CAPTION_ROW } else { 0.0 } + m::TIGHT
}

pub fn figure_font() -> FontId {
    FontId::new(FIGURE_SIZE, style::bold())
}

/// `rows` of label and value, the labels quiet in a column as wide as
/// the widest, values after them (cut short with "…" where they don't
/// fit; agents read all of each). Returns the height used.
pub fn facts(ui: &Ui, r: Rect, rows: &[(&str, &str)]) -> f32 {
    let p = style::palette(ui.ctx());
    let ink = style::card_ink(&p);
    facts_in(ui, r, rows, ink.dim, ink.text)
}

/// [`facts`] in other colours (on a panel rather than a card: the
/// panel's disabled text and its text).
pub fn facts_in(ui: &Ui, r: Rect, rows: &[(&str, &str)], dim: Color32, text: Color32) -> f32 {
    let painter = ui.painter().with_clip_rect(r);
    let font = style::plain_font();
    let label_w = facts_label_width(ui, rows.iter().map(|(l, _)| *l));
    for (i, (label, value)) in rows.iter().enumerate() {
        let mid = r.top() + i as f32 * FACT_ROW + FACT_ROW / 2.0;
        style::label(&painter, r.left(), mid, Align::Min, *label, font.clone(), dim);
        style::label_fit(&painter, r.left() + label_w, mid, Align::Min, value, font.clone(), text, (r.width() - label_w).max(0.0));
    }
    rows.len() as f32 * FACT_ROW
}

/// How far [`facts`]' values stand from their labels' left: the widest
/// label and a gap (to line a control up with the values).
pub fn facts_label_width<'a>(ui: &Ui, labels: impl IntoIterator<Item = &'a str>) -> f32 {
    labels.into_iter().map(|l| style::label_width(ui.ctx(), l, style::plain_font())).fold(0.0, f32::max).ceil() + m::LABEL_GAP
}

pub fn facts_height(rows: usize) -> f32 {
    rows as f32 * FACT_ROW
}

/// Facts in two columns side by side (the first takes the odd one).
pub fn facts_two(ui: &Ui, r: Rect, rows: &[(&str, &str)]) -> f32 {
    let half = rows.len().div_ceil(2);
    let w = ((r.width() - 2.0 * m::SPACING) / 2.0).floor();
    let left = Rect::from_min_size(r.min, vec2(w, r.height()));
    let right = Rect::from_min_size(pos2(r.left() + w + 2.0 * m::SPACING, r.top()), vec2(r.width() - w - 2.0 * m::SPACING, r.height()));
    let a = facts(ui, left, &rows[..half]);
    let b = facts(ui, right, &rows[half..]);
    a.max(b)
}

/// One item of a [`legend`]: its colour, what it is and its value now.
/// A transparent colour is a figure with no line: no swatch.
pub struct LegendItem<'a> {
    pub color: Color32,
    pub name: &'a str,
    pub value: String,
}

fn legend_widths(ui: &Ui, items: &[LegendItem]) -> Vec<f32> {
    let font = style::small_font();
    items
        .iter()
        .map(|it| swatch_w(it) + style::label_width(ui.ctx(), it.name, font.clone()) + 6.0 + style::label_width(ui.ctx(), &it.value, small_bold()) + 2.0 * m::SPACING)
        .collect()
}

const SWATCH: f32 = 8.0;

fn swatch_w(it: &LegendItem) -> f32 {
    if it.color.a() == 0 { 0.0 } else { SWATCH + 5.0 }
}

/// Where each item goes in `width`, flowing onto as many rows as need.
fn legend_places(ui: &Ui, width: f32, items: &[LegendItem]) -> Vec<(f32, usize)> {
    let mut out = Vec::new();
    let (mut x, mut row) = (0.0, 0);
    for w in legend_widths(ui, items) {
        if x > 0.0 && x + w > width {
            x = 0.0;
            row += 1;
        }
        out.push((x, row));
        x += w;
    }
    out
}

pub fn legend_height(ui: &Ui, width: f32, items: &[LegendItem]) -> f32 {
    legend_places(ui, width, items).last().map_or(0.0, |(_, row)| (*row + 1) as f32 * FACT_ROW)
}

/// Swatches, names and values in a row, wrapping. Returns the height.
pub fn legend(ui: &Ui, r: Rect, items: &[LegendItem]) -> f32 {
    let p = style::palette(ui.ctx());
    let ink = style::card_ink(&p);
    let painter = ui.painter().with_clip_rect(r);
    let places = legend_places(ui, r.width(), items);
    for (it, (x, row)) in items.iter().zip(&places) {
        let mid = r.top() + *row as f32 * FACT_ROW + FACT_ROW / 2.0;
        if it.color.a() > 0 {
            let swatch = style::snap_centred(ui.ctx(), pos2(r.left() + x + SWATCH / 2.0, mid), vec2(SWATCH, SWATCH));
            painter.rect_filled(swatch, 2.0, it.color);
        }
        let (name, baseline) = style::label_with_baseline(&painter, r.left() + x + swatch_w(it), mid, Align::Min, it.name, style::small_font(), ink.dim);
        style::label_on_baseline(&painter, name.right() + 6.0, baseline, Align::Min, &it.value, small_bold(), ink.text);
    }
    places.last().map_or(0.0, |(_, row)| (*row + 1) as f32 * FACT_ROW)
}

fn small_bold() -> FontId {
    FontId::new(style::SMALL_SIZE, style::bold())
}

/// A bar of parts of a whole (memory: used, cached, free), each in its
/// colour, the rest the well's. Rounded at the ends.
pub fn meter(ui: &Ui, r: Rect, parts: &[(Color32, f64)], total: f64) {
    let p = style::palette(ui.ctx());
    let ink = style::card_ink(&p);
    let r = style::snap(ui.ctx(), r);
    let painter = ui.painter();
    painter.rect_filled(r, 3.0, ink.well);
    if total <= 0.0 {
        return;
    }
    let ppp = ui.ctx().pixels_per_point();
    let mut x = r.left();
    let mut sum = 0.0;
    for (color, v) in parts {
        sum += v.max(0.0);
        let end = ((r.left() + (sum / total).clamp(0.0, 1.0) as f32 * r.width()) * ppp).round() / ppp;
        if end > x {
            // The bar's ends are round; so are a part's that are its.
            let (left, right) = (if x <= r.left() { 3 } else { 0 }, if end >= r.right() - 0.5 / ppp { 3 } else { 0 });
            let radius = egui::CornerRadius { nw: left, sw: left, ne: right, se: right };
            painter.rect_filled(Rect::from_min_max(pos2(x, r.top()), pos2(end, r.bottom())), radius, *color);
        }
        x = end;
    }
}

/// Rects for `n` cards in `area`: as many columns of at least `min_width`
/// as fit (at most `max_columns`), `height` tall, [`GAP`] apart.
pub fn flow(area: Rect, n: usize, min_width: f32, max_columns: usize, height: f32) -> Vec<Rect> {
    let cols = (((area.width() + GAP) / (min_width + GAP)).floor() as usize).clamp(1, max_columns.max(1));
    let w = ((area.width() - GAP * (cols - 1) as f32) / cols as f32).floor();
    (0..n)
        .map(|i| {
            let (col, row) = (i % cols, i / cols);
            let x = area.left() + col as f32 * (w + GAP);
            // The last column takes what rounding left.
            let w = if col == cols - 1 { area.right() - x } else { w };
            Rect::from_min_size(pos2(x, area.top() + row as f32 * (height + GAP)), vec2(w, height))
        })
        .collect()
}

/// How many rows [`flow`] makes.
pub fn flow_rows(width: f32, n: usize, min_width: f32, max_columns: usize) -> usize {
    let cols = (((width + GAP) / (min_width + GAP)).floor() as usize).clamp(1, max_columns.max(1));
    n.div_ceil(cols)
}

/// The gutter cards stand on, filling `r`.
pub fn gutter(ui: &Ui, r: Rect) {
    let p = style::palette(ui.ctx());
    ui.painter().rect_filled(r, 0.0, c(style::card_paper(&p).0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flows_into_columns() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(620.0, 1000.0));
        let r = flow(area, 5, 200.0, 3, 100.0);
        assert_eq!(r.len(), 5);
        // Three columns of 200 fit in 620 with the gaps (610: two).
        assert_eq!(r[2].right(), 620.0);
        assert_eq!(r[3].top(), 110.0);
        assert_eq!(flow_rows(620.0, 5, 200.0, 3), 2);
        assert_eq!(flow_rows(610.0, 5, 200.0, 3), 3);
        assert_eq!(flow_rows(300.0, 5, 200.0, 3), 5);
    }
}
