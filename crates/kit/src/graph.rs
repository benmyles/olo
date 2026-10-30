//! Live graphs (Haiku's ActivityMonitor, a Mac's Activity Monitor): series
//! of timed samples drawn as lines, each over a faint fill if asked, in a
//! sunken document well with a grid in quarters and time lines that move
//! with the samples. Time runs right to left: `now` is the right edge,
//! `span` seconds back the left, so a graph scrolls smoothly between
//! samples rather than jumping at each one (it asks for the next frame
//! when they have moved a device pixel, while it is shown). Hovering
//! puts a time line under the pointer and says each series' value there
//! (for drill-downs, the time hovered or clicked comes back in
//! [`GraphResponse`]); agents read each series' latest value from the
//! graph's description. Series of different units can share a graph,
//! each scaled to its own top ([`Series::top`]); a plain graph
//! ([`Graph::plain`]) has no well, for a card.
//!
//! ```ignore
//! let series = [Series::new("CPU", c(p.hue(Hue::Blue)), &cpu).fill(true)];
//! let resp = Graph::new("cpu-graph", &series, now).span(60.0).max(100.0).unit(&|v| format!("{v:.0}%")).show(ui, rect);
//! ```

use std::collections::VecDeque;

use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

use crate::style::{self, c};

/// One series: its name, colour and samples as (time in seconds, value),
/// oldest first. A NaN value is a gap.
#[derive(Clone, Copy)]
pub struct Series<'a> {
    pub name: &'a str,
    pub color: Color32,
    pub samples: &'a VecDeque<(f64, f64)>,
    pub fill: bool,
    /// Its own top, for series of different units on one graph (Haiku's
    /// ActivityMonitor scales each source to its own range); else the
    /// graph's.
    pub top: Option<f64>,
    /// How its value reads, when not as the graph's.
    pub format: Option<&'a dyn Fn(f64) -> String>,
}

impl<'a> Series<'a> {
    pub fn new(name: &'a str, color: Color32, samples: &'a VecDeque<(f64, f64)>) -> Self {
        Series { name, color, samples, fill: false, top: None, format: None }
    }

    /// Scale it to `top` of its own.
    pub fn top(mut self, top: f64) -> Self {
        self.top = Some(top);
        self
    }

    pub fn format(mut self, f: &'a dyn Fn(f64) -> String) -> Self {
        self.format = Some(f);
        self
    }

    /// A faint fill under the line (one or two series a graph).
    pub fn fill(mut self, on: bool) -> Self {
        self.fill = on;
        self
    }

    /// The value at time `t`: the latest sample at or before it.
    pub fn at(&self, t: f64) -> Option<f64> {
        let i = self.samples.partition_point(|(st, _)| *st <= t);
        i.checked_sub(1).and_then(|i| self.samples.get(i)).map(|(_, v)| *v).filter(|v| v.is_finite())
    }

    pub fn latest(&self) -> Option<f64> {
        self.samples.back().map(|(_, v)| *v).filter(|v| v.is_finite())
    }
}

/// What happened to a graph this frame.
#[derive(Debug, Default)]
pub struct GraphResponse {
    /// The time under the pointer, while it is over the graph.
    pub hovered: Option<f64>,
    /// The time clicked.
    pub clicked: Option<f64>,
    pub response: Option<egui::Response>,
}

pub struct Graph<'a> {
    id: egui::Id,
    series: &'a [Series<'a>],
    now: f64,
    span: f64,
    max: Option<f64>,
    unit: Option<&'a dyn Fn(f64) -> String>,
    grid_seconds: f64,
    smooth: bool,
    well: bool,
}

/// The next "nice" number at or above `v` (1, 2, 5 × 10ⁿ): a graph's top
/// when it scales to what it shows.
pub fn nice_ceiling(v: f64) -> f64 {
    if v <= 0.0 || !v.is_finite() {
        return 1.0;
    }
    let p = 10f64.powf(v.log10().floor());
    [1.0, 2.0, 5.0, 10.0].iter().map(|k| k * p).find(|&n| n >= v * (1.0 - 1e-9)).unwrap_or(10.0 * p)
}

impl<'a> Graph<'a> {
    /// A graph of `series` at time `now` (seconds, as their samples').
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, series: &'a [Series<'a>], now: f64) -> Self {
        Graph { id: egui::Id::unique(("olo-graph", id)), series, now, span: 60.0, max: None, unit: None, grid_seconds: 10.0, smooth: true, well: true }
    }

    /// How many seconds the graph shows (60 by default).
    pub fn span(mut self, seconds: f64) -> Self {
        self.span = seconds.max(1.0);
        self
    }

    /// A fixed top (100 for a percentage); else it scales to what it shows.
    pub fn max(mut self, max: f64) -> Self {
        self.max = Some(max);
        self
    }

    /// How a value reads (in the hover readout and for agents).
    pub fn unit(mut self, f: &'a dyn Fn(f64) -> String) -> Self {
        self.unit = Some(f);
        self
    }

    /// A time line every this many seconds (10 by default).
    pub fn grid(mut self, seconds: f64) -> Self {
        self.grid_seconds = seconds.max(0.1);
        self
    }

    /// Scroll smoothly between samples, a device pixel at a time (the
    /// default), or only when one comes.
    pub fn smooth(mut self, on: bool) -> Self {
        self.smooth = on;
        self
    }

    /// Without the sunken well: the grid on whatever it is drawn over,
    /// over a base line (a graph in a card).
    pub fn plain(mut self) -> Self {
        self.well = false;
        self
    }

    pub fn show(self, ui: &mut Ui, rect: Rect) -> GraphResponse {
        let p = style::palette(ui.ctx());
        let ppp = ui.ctx().pixels_per_point();
        let snap = |v: f32| (v * ppp).round() / ppp;
        let rect = Rect::from_min_max(pos2(snap(rect.min.x), snap(rect.min.y)), pos2(snap(rect.max.x), snap(rect.max.y)));
        let inner = if self.well { rect.shrink(2.0) } else { rect };
        let painter = ui.painter();
        if self.well {
            painter.rect_filled(inner, 0.0, c(p.document_background));
            style::sunken(painter, rect, &p, false);
        }
        let painter = painter.with_clip_rect(inner);
        let span = self.span;
        let now = self.now;
        let x_of = |t: f64| inner.right() - ((now - t) / span) as f32 * inner.width();
        // The grid: quarters across, and a time line every so often that
        // moves with the samples.
        let grid = c(olo_theme::mix(p.document_text, p.document_background, 0.9));
        for k in 1..4 {
            style::hline(&painter, inner.left(), inner.right(), inner.top() + inner.height() * k as f32 / 4.0, grid);
        }
        if !self.well {
            let base = c(olo_theme::mix(p.document_text, p.document_background, 0.75));
            style::hline_above(&painter, inner.left(), inner.right(), inner.bottom(), base);
        }
        let g = self.grid_seconds;
        let mut t = (now / g).floor() * g;
        while t > now - span {
            style::vline(&painter, x_of(t), inner.top(), inner.bottom(), grid);
            t -= g;
        }
        // The top: fixed, or the nice number over what shows.
        let shown = |s: &Series| s.samples.iter().filter(|(t, _)| *t >= now - span).map(|(_, v)| *v).filter(|v| v.is_finite()).fold(0.0f64, f64::max);
        let top = self.max.unwrap_or_else(|| nice_ceiling(self.series.iter().filter(|s| s.top.is_none()).map(shown).fold(0.0, f64::max)));
        for s in self.series {
            let top = s.top.unwrap_or(top);
            let y_of = |v: f64| inner.bottom() - 1.5 - ((v / top).clamp(0.0, 1.0) as f32) * (inner.height() - 3.0);
            // At most a point a device pixel: past that a line only frays.
            let mut runs: Vec<Vec<Pos2>> = vec![Vec::new()];
            let mut last_col = i64::MIN;
            let start = s.samples.partition_point(|(t, _)| *t < now - span).saturating_sub(1);
            for &(t, v) in s.samples.iter().skip(start) {
                if !v.is_finite() {
                    runs.push(Vec::new());
                    continue;
                }
                let x = x_of(t);
                let col = (x * ppp) as i64;
                let pt = pos2(x, y_of(v));
                let run = runs.last_mut().expect("a run");
                if col == last_col && !run.is_empty() {
                    // Keep the highest of a column (peaks show).
                    let last = run.last_mut().expect("a point");
                    if pt.y < last.y {
                        last.y = pt.y;
                    }
                } else {
                    run.push(pt);
                }
                last_col = col;
            }
            for run in runs.into_iter().filter(|r| r.len() > 1) {
                if s.fill {
                    let fill = Color32::from_rgba_unmultiplied(s.color.r(), s.color.g(), s.color.b(), 40);
                    let base = inner.bottom();
                    // As strips (a filled path must be convex for egui).
                    for w in run.windows(2) {
                        let quad = vec![w[0], w[1], pos2(w[1].x, base), pos2(w[0].x, base)];
                        painter.add(Shape::convex_polygon(quad, fill, Stroke::NONE));
                    }
                }
                painter.add(Shape::line(run, Stroke::new(1.5, s.color)));
            }
        }
        // Hovering: the time under the pointer and each value then.
        let say = |s: &Series, v: f64| match (s.format, self.unit) {
            (Some(f), _) | (None, Some(f)) => f(v),
            (None, None) => format!("{v:.1}"),
        };
        let resp = ui.interact(rect, self.id, Sense::click());
        let mut out = GraphResponse::default();
        let time_at = |x: f32| now - ((inner.right() - x) / inner.width()) as f64 * span;
        if let Some(pos) = resp.hover_pos().filter(|p| inner.contains(*p)) {
            let t = time_at(pos.x);
            out.hovered = Some(t);
            style::vline(&painter, pos.x, inner.top(), inner.bottom(), c(olo_theme::mix(p.document_text, p.document_background, 0.5)));
            let lines: Vec<(Color32, String)> = self
                .series
                .iter()
                .map(|s| {
                    let v = s.at(t).map(|v| say(s, v)).unwrap_or_else(|| "–".into());
                    (s.color, format!("{}: {v}", s.name))
                })
                .collect();
            readout(ui, inner, pos, &lines, ago(now - t));
        }
        if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            out.clicked = Some(time_at(pos.x));
        }
        // What agents read: each series' latest value.
        let said = self
            .series
            .iter()
            .map(|s| format!("{}: {}", s.name, s.latest().map(|v| say(s, v)).unwrap_or_else(|| "–".into())))
            .collect::<Vec<_>>()
            .join(", ");
        resp.widget_info(|| egui::WidgetInfo { current_text_value: Some(said.clone()), ..egui::WidgetInfo::labeled(egui::Role::Figure, true, "Graph") });
        if self.smooth && ui.is_visible() {
            // The next frame when the samples have moved a device pixel:
            // the smallest step there is, so it's as smooth as it can be
            // (a minute across 600 pixels is 10 frames a second, not 120
            // that would draw the same pixels).
            let per_pixel = span / (inner.width() * ppp).max(1.0) as f64;
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(per_pixel.clamp(1.0 / 240.0, 1.0)));
        }
        out.response = Some(resp);
        out
    }
}

/// How long ago `seconds` was, as a readout says it.
fn ago(seconds: f64) -> String {
    match seconds {
        s if s < 1.5 => "now".into(),
        s if s < 90.0 => format!("{s:.0} s ago"),
        s if s < 5400.0 => format!("{:.0} min ago", s / 60.0),
        s => format!("{:.1} h ago", s / 3600.0),
    }
}

/// The values under the pointer, in a tip-coloured box beside it (kept
/// inside the graph).
fn readout(ui: &Ui, inside: Rect, at: Pos2, lines: &[(Color32, String)], when: String) {
    let p = style::palette(ui.ctx());
    let font = egui::FontId::proportional(style::SMALL_SIZE);
    let painter = ui.painter();
    let galleys: Vec<_> = std::iter::once(painter.layout_no_wrap(when, font.clone(), c(p.tooltip_text)))
        .chain(lines.iter().map(|(_, t)| painter.layout_no_wrap(t.clone(), font.clone(), c(p.tooltip_text))))
        .collect();
    let row = style::SMALL_SIZE + 3.0;
    let w = galleys.iter().map(|g| g.size().x).fold(0.0, f32::max) + 22.0;
    let h = galleys.len() as f32 * row + 6.0;
    let mut r = Rect::from_min_size(at + vec2(10.0, 10.0), vec2(w, h));
    if r.right() > inside.right() {
        r = r.translate(vec2(-(w + 20.0), 0.0));
    }
    if r.bottom() > inside.bottom() {
        r = r.translate(vec2(0.0, inside.bottom() - r.bottom()));
    }
    let ppp = ui.ctx().pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let r = Rect::from_min_size(pos2(snap(r.min.x), snap(r.min.y)), vec2(snap(r.width()), snap(r.height())));
    painter.rect_filled(r, 2.0, c(p.tooltip_background));
    style::frame_rect(painter, r, c(p.control_border));
    for (i, g) in galleys.into_iter().enumerate() {
        let y = r.top() + 3.0 + i as f32 * row;
        if i > 0 {
            let swatch = style::snap_centred(ui.ctx(), pos2(r.left() + 9.0, y + row / 2.0 - 1.0), vec2(7.0, 7.0));
            painter.rect_filled(swatch, 0.0, lines[i - 1].0);
        }
        painter.galley(pos2(r.left() + if i > 0 { 17.0 } else { 5.0 }, y), g, c(p.tooltip_text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_tops() {
        assert_eq!(nice_ceiling(0.0), 1.0);
        assert_eq!(nice_ceiling(3.2), 5.0);
        assert_eq!(nice_ceiling(12_000.0), 20_000.0);
        assert_eq!(nice_ceiling(100.0), 100.0);
    }

    #[test]
    fn values_at_a_time() {
        let samples: VecDeque<(f64, f64)> = [(1.0, 10.0), (2.0, f64::NAN), (3.0, 30.0)].into_iter().collect();
        let s = Series::new("x", Color32::RED, &samples);
        assert_eq!(s.at(0.5), None);
        assert_eq!(s.at(1.5), Some(10.0));
        // A gap reads as nothing.
        assert_eq!(s.at(2.5), None);
        assert_eq!(s.at(9.0), Some(30.0));
        assert_eq!(s.latest(), Some(30.0));
        assert_eq!(ago(0.4), "now");
        assert_eq!(ago(30.0), "30 s ago");
    }
}
