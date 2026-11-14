//! Live graphs, LED cell grids and legends.
//!
//! A [`Graph`] (Haiku's ActivityMonitor, a Mac's Activity Monitor) draws
//! series of timed samples as lines (GPU segments), each over a faint
//! fill if asked, in a sunken document well with a grid in quarters and
//! time lines that move with the samples. Time runs right to left: `now`
//! is the right edge, `span` seconds back the left. A live one
//! ([`Graph::live`]) reads the time from a clock each time it's drawn
//! and draws itself again when its samples have moved a device pixel
//! (`Painter::paint_again_within`: the window's clock, one tick for all
//! that move in it), so it scrolls between samples rather than jumping
//! at each one, and only it is drawn: not the view it's in, nor that
//! view's `body`. Hovering puts a time line under the pointer and says
//! each series' value there; agents read each series' latest value. A
//! [`CellGrid`] is a grid of small lit cells in sunken wells, as Pulse's
//! LED meters on BeOS: a heatmap of things going on. [`legend`] lays out
//! swatches, names and values in rows.

use std::{collections::VecDeque, rc::Rc};

use olo_interface::{
    Bounds, Element, FontWeight, Hsla, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, Painter, Pixels, Point, SharedString, Styled as _, WindowCx, agent, canvas,
    point, px, size,
};
use olo_kit_base::metrics as m;

use crate::{style, theme};

/// One series: its name, colour and samples as (time in seconds, value),
/// oldest first. A NaN value is a gap.
#[derive(Clone)]
pub struct Series {
    pub name: SharedString,
    pub color: Hsla,
    pub samples: Rc<VecDeque<(f64, f64)>>,
    pub fill: bool,
    /// Its own top, for series of different units on one graph (Haiku's
    /// ActivityMonitor scales each source to its own range); else the
    /// graph's.
    pub top: Option<f64>,
    /// How its value reads, when not as the graph's.
    pub format: Option<Rc<dyn Fn(f64) -> String>>,
}

impl Series {
    pub fn new(name: impl Into<SharedString>, color: Hsla, samples: Rc<VecDeque<(f64, f64)>>) -> Series {
        Series { name: name.into(), color, samples, fill: false, top: None, format: None }
    }

    /// Scale it to `top` of its own.
    pub fn top(mut self, top: f64) -> Self {
        self.top = Some(top);
        self
    }

    pub fn format(mut self, f: impl Fn(f64) -> String + 'static) -> Self {
        self.format = Some(Rc::new(f));
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

/// The next "nice" number at or above `v` (1, 2, 5 × 10ⁿ): a graph's top
/// when it scales to what it shows.
pub fn nice_ceiling(v: f64) -> f64 {
    if v <= 0.0 || !v.is_finite() {
        return 1.0;
    }
    let p = 10f64.powf(v.log10().floor());
    [1.0, 2.0, 5.0, 10.0].iter().map(|k| k * p).find(|&n| n >= v * (1.0 - 1e-9)).unwrap_or(10.0 * p)
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

/// What a graph keeps from one drawing to the next: the pointer over it,
/// and where and when it was last drawn (what its handlers go by).
#[derive(Default)]
struct GraphState {
    hover: Option<Point<Pixels>>,
    /// Where its samples are drawn.
    inner: Bounds<Pixels>,
    /// The time at its right edge, and the seconds across it.
    now: f64,
    span: f64,
}

impl GraphState {
    /// The time at `x` (in the window).
    fn time_at(&self, x: Pixels) -> f64 {
        self.now - (f32::from(self.inner.right() - x) / f32::from(self.inner.size.width).max(1.0)) as f64 * self.span
    }
}

type TimeFn = Rc<dyn Fn(&f64, &mut WindowCx)>;

/// A live graph of [`Series`]; it fills the box it's given.
#[derive(Clone)]
pub struct Graph {
    id: SharedString,
    series: Rc<[Series]>,
    now: f64,
    clock: Option<Rc<dyn Fn() -> f64>>,
    span: f64,
    max: Option<f64>,
    unit: Option<Rc<dyn Fn(f64) -> String>>,
    grid_seconds: f64,
    smooth: bool,
    well: bool,
    on_click: Option<TimeFn>,
    on_hover: Option<TimeFn>,
}

impl Graph {
    /// A graph of `series` at time `now` (seconds, as their samples'):
    /// still until what holds it says what it shows again.
    pub fn new(id: impl Into<SharedString>, series: impl Into<Rc<[Series]>>, now: f64) -> Graph {
        Graph { id: id.into(), series: series.into(), now, clock: None, span: 60.0, max: None, unit: None, grid_seconds: 10.0, smooth: true, well: true, on_click: None, on_hover: None }
    }

    /// A graph of `series` that moves as time passes: `clock` says what
    /// time it is (seconds, as their samples') each time it's drawn,
    /// and it draws itself again, alone, when its samples have moved a
    /// device pixel (a minute across 600 pixels: ten times a second),
    /// unless it isn't [`Self::smooth`]. Nothing runs while its window
    /// isn't shown.
    pub fn live(id: impl Into<SharedString>, series: impl Into<Rc<[Series]>>, clock: impl Fn() -> f64 + 'static) -> Graph {
        let clock: Rc<dyn Fn() -> f64> = Rc::new(clock);
        Graph { clock: Some(clock.clone()), ..Graph::new(id, series, clock()) }
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
    pub fn unit(mut self, f: impl Fn(f64) -> String + 'static) -> Self {
        self.unit = Some(Rc::new(f));
        self
    }

    /// A time line every this many seconds (10 by default).
    pub fn grid(mut self, seconds: f64) -> Self {
        self.grid_seconds = seconds.max(0.1);
        self
    }

    /// A [live](Self::live) graph scrolls smoothly between samples, a
    /// device pixel at a time (the default), or moves only when what
    /// holds it says what it shows again.
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

    /// The time clicked.
    pub fn on_click(mut self, f: impl Fn(&f64, &mut WindowCx) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    /// The time under the pointer as it moves over the graph (for a
    /// drill-down).
    pub fn on_hover(mut self, f: impl Fn(&f64, &mut WindowCx) + 'static) -> Self {
        self.on_hover = Some(Rc::new(f));
        self
    }
}

impl IntoElement for Graph {
    fn into_element(self) -> Element {
        let Graph { id, series, now, clock, span, max, unit, grid_seconds, smooth, well, on_click, on_hover } = self;
        let clickable = on_click.is_some();
        let mut element = canvas(move |g, b| {
            let p = theme::palette();
            let s = g.scale_factor();
            let now = clock.as_ref().map_or(now, |clock| clock());
            let rect = style::snap(g, b);
            let inner = if well { style::inset(g, rect, 2.0) } else { rect };
            if well {
                style::fill_rect(g, inner, style::c(p.document_background));
                style::sunken(g, rect, &p, false);
            }
            let hover = {
                let state = g.node_state::<GraphState>();
                (state.inner, state.now, state.span) = (inner, now, span);
                state.hover
            };
            // The next drawing when the samples have moved a device
            // pixel.
            if smooth && clock.is_some() {
                g.paint_again_within(span / (f32::from(inner.size.width) * s).max(1.0) as f64);
            }
            let width = f32::from(inner.size.width);
            let x_of = move |t: f64| inner.right() - px(((now - t) / span) as f32 * width);
            let time_at = move |x: Pixels| now - (f32::from(inner.right() - x) / width) as f64 * span;
            let say = |series: &Series, v: f64| match (&series.format, &unit) {
                (Some(f), _) | (None, Some(f)) => f(v),
                (None, None) => format!("{v:.1}"),
            };
            g.with_content_mask(Some(olo_interface::ContentMask { bounds: inner }), |g| {
                // The grid, the fills and the lines in one layer: one
                // place in the scene's order for all of them rather
                // than one each, found among everything drawn before
                // (in a layer, quads go under paths under segments).
                g.paint_layer(inner, |g| {
                    // The grid: quarters across, and a time line every so
                    // often that moves with the samples.
                    let grid = style::c(olo_theme::mix(p.document_text, p.document_background, 0.9));
                    for k in 1..4 {
                        style::hline(g, inner.left(), inner.right(), inner.top() + inner.size.height * (k as f32 / 4.0), grid);
                    }
                    if !well {
                        let base = style::c(olo_theme::mix(p.document_text, p.document_background, 0.75));
                        style::hline_above(g, inner.left(), inner.right(), inner.bottom(), base);
                    }
                    let mut t = (now / grid_seconds).floor() * grid_seconds;
                    while t > now - span {
                        style::vline(g, x_of(t), inner.top(), inner.bottom(), grid);
                        t -= grid_seconds;
                    }
                    // The top: fixed, or the nice number over what shows.
                    let shown = |series: &Series| series.samples.iter().filter(|(t, _)| *t >= now - span).map(|(_, v)| *v).filter(|v| v.is_finite()).fold(0.0f64, f64::max);
                    let top = max.unwrap_or_else(|| nice_ceiling(series.iter().filter(|s| s.top.is_none()).map(shown).fold(0.0, f64::max)));
                    for line in series.iter() {
                        let top = line.top.unwrap_or(top);
                        let h = f32::from(inner.size.height);
                        let y_of = |v: f64| inner.bottom() - px(1.5 + (v / top).clamp(0.0, 1.0) as f32 * (h - 3.0));
                        // At most a point a device pixel: past that a line
                        // only frays.
                        let mut runs: Vec<Vec<Point<Pixels>>> = vec![Vec::new()];
                        let mut last_col = i64::MIN;
                        let start = line.samples.partition_point(|(t, _)| *t < now - span).saturating_sub(1);
                        for &(t, v) in line.samples.iter().skip(start) {
                            if !v.is_finite() {
                                runs.push(Vec::new());
                                continue;
                            }
                            let x = x_of(t);
                            let col = (f32::from(x) * s) as i64;
                            let pt = point(x, y_of(v));
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
                        let runs: Vec<_> = runs.into_iter().filter(|r| r.len() > 1).collect();
                        if line.fill {
                            let under = line.color.opacity(40.0 / 255.0);
                            for run in &runs {
                                for w in run.windows(2) {
                                    g.paint_area(w[0], w[1], inner.bottom(), under);
                                }
                            }
                        }
                        for run in &runs {
                            for w in run.windows(2) {
                                g.paint_segment(w[0], w[1], px(1.5), true, line.color);
                            }
                        }
                    }
                });
                // Hovering: the time under the pointer and each value
                // then.
                if let Some(pos) = hover.filter(|pos| inner.contains(pos)) {
                    let t = time_at(pos.x);
                    style::vline(g, pos.x, inner.top(), inner.bottom(), style::c(olo_theme::mix(p.document_text, p.document_background, 0.5)));
                    let lines: Vec<(Hsla, String)> = series.iter().map(|line| (line.color, format!("{}: {}", line.name, line.at(t).map(|v| say(line, v)).unwrap_or_else(|| "–".into())))).collect();
                    readout(g, inner, pos, &lines, ago(now - t));
                }
            });
            // What agents read: each series' latest value.
            let said = series.iter().map(|line| format!("{}: {}", line.name, line.latest().map(|v| say(line, v)).unwrap_or_else(|| "–".into()))).collect::<Vec<_>>().join(", ");
            g.agent(agent::Node { role: "Figure", name: Some("Graph".into()), value: Some(said.into()), bounds: rect, enabled: true, clickable, ..Default::default() });
        })
        .id(id)
        // The pointer over it: the time line and the readout follow it
        // (only the graph is drawn again), and are gone when it leaves.
        .on_mouse_move(move |e: &MouseMoveEvent, cx| {
            let state = cx.node_state::<GraphState>();
            let at = state.inner.contains(&e.position).then_some(e.position);
            let time = at.map(|pos| state.time_at(pos.x));
            if state.hover != at {
                state.hover = at;
                cx.repaint();
            }
            if let (Some(time), Some(f)) = (time, &on_hover) {
                f(&time, cx);
            }
        })
        .on_hover(|over, cx| {
            if !over && cx.node_state::<GraphState>().hover.take().is_some() {
                cx.repaint();
            }
        })
        .size_full();
        if let Some(f) = on_click {
            element = element.on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, cx| {
                let state = cx.node_state::<GraphState>();
                if state.inner.contains(&e.position) {
                    let time = state.time_at(e.position.x);
                    f(&time, cx);
                    // (Its own: not the card's it's on too.)
                    cx.stop_propagation();
                }
            });
        }
        element
    }
}

/// One spectrum of a [`Spectrum`]: its name, colour and level in each
/// band (dB), lowest band first.
#[derive(Clone)]
pub struct SpectrumSeries {
    pub name: SharedString,
    pub color: Hsla,
    pub levels: Rc<[f32]>,
    pub fill: bool,
}

impl SpectrumSeries {
    pub fn new(name: impl Into<SharedString>, color: Hsla, levels: impl Into<Rc<[f32]>>) -> SpectrumSeries {
        SpectrumSeries { name: name.into(), color, levels: levels.into(), fill: false }
    }

    /// A faint fill under the line.
    pub fn fill(mut self, on: bool) -> Self {
        self.fill = on;
        self
    }
}

/// Spectra, in a [`Graph`]'s look (a sunken well, a faint grid): each
/// series' level in bands spaced evenly in log frequency from `low` to
/// `high` Hz, as a line (over a faint fill if asked), from `floor` dB at
/// the bottom to 0 at the top; lines at 100 Hz, 1 kHz and 10 kHz, named
/// under the well (it keeps [`SPECTRUM_LABELS`] of its height for them),
/// and every 20 dB. What a sound is doing as it plays: Preferences ▸ Sound
/// ▸ Voice Processing ▸ Try It, the original and the processed. Agents
/// read each series' loudest band.
pub struct Spectrum {
    series: Rc<[SpectrumSeries]>,
    low: f32,
    high: f32,
    floor: f32,
}

/// The height under a [`Spectrum`]'s well for its frequencies.
pub const SPECTRUM_LABELS: f32 = m::LABEL_ROW;

impl Spectrum {
    pub fn new(series: impl Into<Rc<[SpectrumSeries]>>, low: f32, high: f32) -> Spectrum {
        Spectrum { series: series.into(), low, high, floor: -90.0 }
    }

    /// The level at the bottom, dB (−90 unless set).
    pub fn floor(mut self, db: f32) -> Self {
        self.floor = db;
        self
    }
}

/// A frequency as said: "800 Hz", "1.2 kHz".
fn hertz(f: f32) -> String {
    if f >= 1000.0 { format!("{:.1} kHz", f / 1000.0).replace(".0 kHz", " kHz") } else { format!("{f:.0} Hz") }
}

impl IntoElement for Spectrum {
    fn into_element(self) -> Element {
        let Spectrum { series, low, high, floor } = self;
        canvas(move |g, b| {
            let p = theme::palette();
            let well = Bounds::new(b.origin, size(b.size.width, (b.size.height - px(SPECTRUM_LABELS)).max(px(0.0))));
            let rect = style::snap(g, well);
            let inner = style::inset(g, rect, 2.0);
            style::fill_rect(g, inner, style::c(p.document_background));
            style::sunken(g, rect, &p, false);
            let (w, h) = (f32::from(inner.size.width), f32::from(inner.size.height));
            let span = (high / low).ln().max(1e-6);
            let x_of = |f: f32| inner.left() + px((f / low).ln() / span * w);
            let y_of = |db: f32| inner.bottom() - px(1.5 + ((db - floor) / -floor).clamp(0.0, 1.0) * (h - 3.0));
            let grid = style::c(olo_theme::mix(p.document_text, p.document_background, 0.9));
            let dim = style::disabled_text(&p);
            let decades: Vec<f32> = [100.0, 1000.0, 10_000.0].into_iter().filter(|f| (low..=high).contains(f)).collect();
            g.with_content_mask(Some(olo_interface::ContentMask { bounds: inner }), |g| {
                g.paint_layer(inner, |g| {
                    let mut db = -20.0;
                    while db > floor {
                        style::hline(g, inner.left(), inner.right(), y_of(db), grid);
                        db -= 20.0;
                    }
                    for f in &decades {
                        style::vline(g, x_of(*f), inner.top(), inner.bottom(), grid);
                    }
                    // (A silent one isn't drawn: no line along the floor.)
                    for line in series.iter().filter(|l| l.levels.iter().any(|v| *v > floor)) {
                        let n = line.levels.len().max(1);
                        let pts: Vec<Point<Pixels>> = line.levels.iter().enumerate().map(|(i, db)| point(inner.left() + px((i as f32 + 0.5) / n as f32 * w), y_of(*db))).collect();
                        if line.fill {
                            for pair in pts.windows(2) {
                                g.paint_area(pair[0], pair[1], inner.bottom(), line.color.opacity(40.0 / 255.0));
                            }
                        }
                        for pair in pts.windows(2) {
                            g.paint_segment(pair[0], pair[1], px(1.5), true, line.color);
                        }
                    }
                });
            });
            // The frequencies under the well, each under its line.
            let baseline = style::baseline_at(g, rect.bottom() + px(SPECTRUM_LABELS / 2.0), style::SMALL_SIZE);
            for f in &decades {
                style::label_on_baseline(g, x_of(*f), baseline, style::Align::Center, hertz(*f), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), dim);
            }
            let ratio = (high / low).powf(1.0 / series.first().map_or(1, |s| s.levels.len().max(1)) as f32);
            let said = series
                .iter()
                .map(|s| match s.levels.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)) {
                    Some((i, db)) if *db > floor => format!("{}: loudest at {}, {:.0} dB", s.name, hertz(low * ratio.powf(i as f32 + 0.5)), db),
                    _ => format!("{}: silent", s.name),
                })
                .collect::<Vec<_>>()
                .join("; ");
            g.agent(agent::Node { role: "Figure", name: Some("Spectrum".into()), value: Some(said.into()), bounds: rect, enabled: true, ..Default::default() });
        })
        .size_full()
    }
}

/// The values under the pointer, in a tip-coloured box beside it (kept
/// inside the graph).
fn readout(g: &mut Painter, inside: Bounds<Pixels>, at: Point<Pixels>, lines: &[(Hsla, String)], when: String) {
    let p = theme::palette();
    let font = style::ui_font(FontWeight::NORMAL);
    let texts: Vec<String> = std::iter::once(when).chain(lines.iter().map(|(_, t)| t.clone())).collect();
    let row = style::SMALL_SIZE + 3.0;
    let w = texts.iter().map(|t| f32::from(style::text_width(t.clone(), style::SMALL_SIZE, font.clone()))).fold(0.0, f32::max) + 22.0;
    let h = texts.len() as f32 * row + 6.0;
    let mut r = Bounds { origin: point(at.x + px(10.0), at.y + px(10.0)), size: size(px(w), px(h)) };
    if r.right() > inside.right() {
        r.origin.x -= px(w + 20.0);
    }
    if r.bottom() > inside.bottom() {
        r.origin.y -= r.bottom() - inside.bottom();
    }
    let r = style::snap(g, r);
    g.paint_quad(olo_interface::fill(r, style::c(p.tooltip_background)).corner_radii(px(2.0)));
    style::frame_rect(g, r, style::c(p.control_border));
    for (i, text) in texts.into_iter().enumerate() {
        let y = r.top() + px(3.0 + i as f32 * row);
        if i > 0 {
            let swatch = style::snap_centred(g, point(r.left() + px(9.0), y + px(row / 2.0 - 1.0)), 7.0, 7.0);
            style::fill_rect(g, swatch, lines[i - 1].0);
        }
        let x = r.left() + px(if i > 0 { 17.0 } else { 5.0 });
        style::label(g, x, y + px(row / 2.0), style::Align::Left, text, style::SMALL_SIZE, font.clone(), style::c(p.tooltip_text));
    }
}

/// One cell of a [`CellGrid`].
#[derive(Debug, Clone, Default)]
pub struct GridCell {
    /// Lit in this colour (from the palette); None: an empty well.
    pub color: Option<Hsla>,
    /// A small mark drawn in it when there's room ("✦").
    pub mark: Option<SharedString>,
    /// What it is, for agents.
    pub name: SharedString,
    pub selected: bool,
}

type CellFn = Rc<dyn Fn(&usize, &mut WindowCx)>;

/// A grid of small lit cells in sunken wells, as Pulse's LED meters on
/// BeOS. Its height is fixed (`rows` cells of `cell_size`); with more
/// cells than fit, they shrink and more rows and columns fit in the same
/// space. Each cell can be clicked (and is named for agents).
pub struct CellGrid {
    cells: Rc<[GridCell]>,
    rows: usize,
    cell: f32,
    gap: f32,
    slots: usize,
    fill: bool,
    on_click: Option<CellFn>,
}

impl CellGrid {
    pub fn new(cells: impl Into<Rc<[GridCell]>>) -> CellGrid {
        CellGrid { cells: cells.into(), rows: 1, cell: 12.0, gap: 2.0, slots: 0, fill: false, on_click: None }
    }

    /// Rows at full size (the height it takes).
    pub fn rows(mut self, rows: usize) -> Self {
        self.rows = rows.max(1);
        self
    }

    pub fn cell_size(mut self, size: f32) -> Self {
        self.cell = size;
        self
    }

    /// Show at least this many wells, lit or not (a strip of the last 25).
    pub fn slots(mut self, n: usize) -> Self {
        self.slots = n;
        self
    }

    /// Wells across the whole area, however few cells are lit (a meter
    /// of what's going on now).
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&usize, &mut WindowCx) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

/// The cell size and columns and rows that fit `n` cells in `width` ×
/// `height`.
fn fit(cell: f32, gap: f32, n: usize, width: f32, height: f32) -> (f32, usize, usize) {
    let mut size = cell;
    loop {
        let cols = (((width + gap) / (size + gap)).floor() as usize).max(1);
        let rows = (((height + gap) / (size + gap)).floor() as usize).max(1);
        if cols * rows >= n || size <= 4.0 {
            return (size, cols, rows);
        }
        size -= 1.0;
    }
}

/// What a grid keeps from one drawing to the next: where its wells
/// were drawn, and which the pointer is over.
#[derive(Default)]
struct GridState {
    wells: Vec<Bounds<Pixels>>,
    hover: Option<usize>,
}

/// A well's size and the gap between two (points) on device pixels at
/// `scale`: the gap a whole number of them, the same between every two
/// wells, and the wells as big as then fit `cols` across `width` and
/// `rows` down `height`. (Each well snapped by itself, 14-point steps
/// at 1.25× were 17 and 18 pixels by turns, the gaps 2 and 3; and a
/// selected well's frame, a pixel of its gap, touched its neighbour
/// where the gap was 2.)
fn on_pixels(size: f32, gap: f32, cols: usize, rows: usize, width: f32, height: f32, scale: f32) -> (f32, f32) {
    let gap_px = (gap * scale).round().max(1.0);
    let room = |length: f32, count: usize| ((length * scale - (count as f32 - 1.0) * gap_px) / count as f32).floor();
    let size_px = (size * scale).round().min(room(width, cols)).min(room(height, rows)).max(1.0);
    (size_px / scale, gap_px / scale)
}

impl IntoElement for CellGrid {
    fn into_element(self) -> Element {
        let height = self.rows as f32 * self.cell + (self.rows as f32 - 1.0) * self.gap;
        let CellGrid { cells, cell, gap, slots, fill, on_click, .. } = self;
        let count = cells.len();
        let clickable = on_click.is_some();
        let mut element = canvas(move |g, b| {
            let p = theme::palette();
            let n = count.max(slots);
            let width = f32::from(b.size.width);
            let (size, cols, rows) = fit(cell, gap, n, width, height);
            let (size, gap) = on_pixels(size, gap, cols, rows, width, height, g.scale_factor());
            let shown = if fill { cols * rows } else { n.min(cols * rows) };
            // Filling: spread the columns to both edges.
            let step_x = if fill && cols > 1 { (width - size) / (cols - 1) as f32 } else { size + gap };
            let wells: Vec<Bounds<Pixels>> = (0..shown)
                .map(|i| {
                    let (row, col) = (i / cols, i % cols);
                    style::snap(g, Bounds { origin: point(b.left() + px(col as f32 * step_x), b.top() + px(row as f32 * (size + gap))), size: size_of(size) })
                })
                .collect();
            let hover = {
                let state = g.node_state::<GridState>();
                state.wells.clone_from(&wells);
                state.hover
            };
            // (Its wells are spaced by the grid, a couple of points apart
            // as LEDs are: not controls crowded together.)
            crate::review::own_grid(g, b);
            for (i, well) in wells.iter().enumerate() {
                // The well: sunken, as an unlit LED.
                style::fill_rect(g, *well, style::shade(&p, p.panel_background, 1.12));
                style::bevel(g, *well, style::shade(&p, p.panel_background, 1.4), style::c(p.shine));
                let Some(item) = cells.get(i) else { continue };
                let inner = style::inset(g, *well, 1.0);
                if let Some(color) = item.color {
                    let rgb = olo_interface::Rgba::from(color);
                    let scale = |rgb: olo_interface::Rgba, k: f32| -> Hsla { olo_interface::Rgba { r: (rgb.r * k).min(1.0), g: (rgb.g * k).min(1.0), b: (rgb.b * k).min(1.0), a: 1.0 }.into() };
                    let lit = if hover == Some(i) { scale(rgb, 1.15) } else { color };
                    style::vgradient(g, inner, lit, scale(olo_interface::Rgba::from(lit), 0.8));
                    style::hline(g, inner.left(), inner.right(), inner.top(), style::c(p.gloss));
                }
                let side = f32::from(well.size.width);
                if let Some(mark) = item.mark.clone().filter(|_| side >= 11.0) {
                    // Centred on its capitals, kept inside the well.
                    let ink = item.color.map_or(style::c(p.panel_text), |fill| {
                        let c = olo_interface::Rgba::from(fill);
                        style::c(p.ink_on([(c.r * 255.0) as u8, (c.g * 255.0) as u8, (c.b * 255.0) as u8, 255]))
                    });
                    // (Kept from its neighbours' wells; its line is taller
                    // than a small well, its ink isn't: not cut above
                    // or below.)
                    let within = Bounds::from_corners(point(inner.left(), inner.top() - px(side)), point(inner.right(), inner.bottom() + px(side)));
                    g.with_content_mask(Some(olo_interface::ContentMask { bounds: within }), |g| {
                        style::label(g, inner.center().x, inner.center().y, style::Align::Center, mark, (side * 0.64).round(), style::ui_font(FontWeight::NORMAL), ink);
                    });
                }
                // Too small for a plate, and flush with its neighbours:
                // a hairline in the selection's edge colour round it.
                if item.selected {
                    let line = style::hairline(g);
                    let out = Bounds::from_corners(point(well.left() - line, well.top() - line), point(well.right() + line, well.bottom() + line));
                    style::frame_rect(g, out, style::c(p.selection_edge()));
                }
                g.agent(agent::Node { role: "Button", name: Some(item.name.clone()), bounds: *well, enabled: true, clickable, selected: Some(item.selected), ..Default::default() });
            }
        })
        // The lit cell under the pointer is brighter (only the grid is
        // drawn again, and only as the pointer goes from cell to cell).
        .on_mouse_move(move |e: &MouseMoveEvent, cx| {
            let state = cx.node_state::<GridState>();
            let over = state.wells.iter().take(count).position(|well| well.contains(&e.position));
            if state.hover != over {
                state.hover = over;
                cx.repaint();
            }
        })
        .on_hover(|over, cx| {
            if !over && cx.node_state::<GridState>().hover.take().is_some() {
                cx.repaint();
            }
        })
        .w_full()
        .h(px(height))
        .flex_shrink_0();
        if let Some(f) = on_click {
            element = element.on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, cx| {
                let over = cx.node_state::<GridState>().wells.iter().take(count).position(|well| well.contains(&e.position));
                if let Some(i) = over {
                    f(&i, cx);
                    cx.stop_propagation();
                }
            });
        }
        element
    }
}

fn size_of(side: f32) -> olo_interface::Size<Pixels> {
    size(px(side), px(side))
}

/// A legend's row height.
pub const FACT_ROW: f32 = 18.0;
const SWATCH: f32 = 8.0;

/// One item of a [`legend`]: its colour, what it is and its value now.
/// A transparent colour is a figure with no line: no swatch.
#[derive(Clone, Debug)]
pub struct LegendItem {
    pub color: Hsla,
    pub name: SharedString,
    pub value: SharedString,
}

fn swatch_w(it: &LegendItem) -> f32 {
    if it.color.a == 0.0 { 0.0 } else { SWATCH + 5.0 }
}

/// Where each item goes in `width`, flowing onto as many rows as need.
fn legend_places(width: f32, items: &[LegendItem]) -> Vec<(f32, usize)> {
    let (plain, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
    let mut out = Vec::new();
    let (mut x, mut row) = (0.0, 0);
    for it in items {
        let w = swatch_w(it) + style::text_width_app(&it.name, style::DETAIL_SIZE, &plain) + 6.0 + style::text_width_app(&it.value, style::SMALL_SIZE, &bold) + 2.0 * m::SPACING;
        if x > 0.0 && x + w > width {
            x = 0.0;
            row += 1;
        }
        out.push((x, row));
        x += w;
    }
    out
}

/// How tall a [`legend`] of `items` is in `width`.
pub fn legend_height(width: f32, items: &[LegendItem]) -> f32 {
    legend_places(width, items).last().map_or(0.0, |(_, row)| (*row + 1) as f32 * FACT_ROW)
}

/// Swatches, names and values in a row, wrapping, in the document's text
/// colours, drawn in `r`.
pub fn legend(g: &mut Painter, r: Bounds<Pixels>, items: &[LegendItem]) {
    let p = theme::palette();
    let text = style::c(p.document_text);
    let dim = text.opacity(0.52);
    let places = legend_places(f32::from(r.size.width), items);
    // (Its items go where their lengths put them: lined up with what's
    // above or below only by chance.)
    crate::review::own_grid(g, r);
    let (plain, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
    g.with_content_mask(Some(olo_interface::ContentMask { bounds: r }), |g| {
        for (it, (x, row)) in items.iter().zip(&places) {
            let mid = r.top() + px(*row as f32 * FACT_ROW + FACT_ROW / 2.0);
            let left = r.left() + px(*x);
            if it.color.a > 0.0 {
                let swatch = style::snap_centred(g, point(left + px(SWATCH / 2.0), mid), SWATCH, SWATCH);
                g.paint_quad(olo_interface::fill(swatch, it.color).corner_radii(px(2.0)));
            }
            // Name and value on one baseline (their sizes differ).
            let baseline = style::baseline_at(g, mid, style::DETAIL_SIZE);
            let name = style::label_on_baseline(g, left + px(swatch_w(it)), baseline, style::Align::Left, it.name.clone(), style::DETAIL_SIZE, plain.clone(), dim);
            style::label_on_baseline(g, name.right() + px(6.0), baseline, style::Align::Left, it.value.clone(), style::SMALL_SIZE, bold.clone(), text);
        }
    });
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
        let s = Series::new("x", olo_interface::red(), Rc::new(samples));
        assert_eq!(s.at(0.5), None);
        assert_eq!(s.at(1.5), Some(10.0));
        // A gap reads as nothing.
        assert_eq!(s.at(2.5), None);
        assert_eq!(s.at(9.0), Some(30.0));
        assert_eq!(s.latest(), Some(30.0));
        assert_eq!(ago(0.4), "now");
        assert_eq!(ago(30.0), "30 s ago");
    }

    /// Wells and their gaps are whole device pixels: as they were at
    /// 1×, 1.5× and 2×, and at 1.25× three pixels apart, not two and
    /// three by turns.
    #[test]
    fn cells_are_on_device_pixels() {
        // Two rows of 14-point wells, 2 apart, in 30 points.
        for (scale, well, gap) in [(1.0, 14.0, 2.0), (1.25, 17.0, 3.0), (1.5, 21.0, 3.0), (1.75, 24.0, 4.0), (2.0, 28.0, 4.0)] {
            let (size, between) = on_pixels(14.0, 2.0, 39, 2, 628.0, 30.0, scale);
            assert_eq!((size * scale, between * scale), (well, gap), "at {scale}×");
            assert!(2.0 * size + between <= 30.0, "at {scale}×");
        }
        // A strip of 12-point wells.
        assert_eq!(on_pixels(12.0, 2.0, 25, 1, 628.0, 12.0, 1.25), (15.0 / 1.25, 3.0 / 1.25));
    }

    #[test]
    fn cells_fit() {
        // Two rows of 14: 40 cells in 200 points shrink until they fit.
        let (size, cols, rows) = fit(14.0, 2.0, 40, 200.0, 30.0);
        assert!(cols * rows >= 40, "{size} {cols} {rows}");
        assert_eq!(fit(12.0, 2.0, 3, 200.0, 12.0), (12.0, 14, 1));
    }
}

/// Bars side by side across `bounds`, one a level (0 to 1) each, rising
/// from its foot, a device pixel apart: Pulse's processors.
pub fn paint_bars(g: &mut Painter, bounds: Bounds<Pixels>, levels: &[f32], color: Hsla) {
    if levels.is_empty() {
        return;
    }
    let s = g.scale_factor();
    let gap = (1.0 / s).max(f32::from(bounds.size.width) / levels.len() as f32 * 0.15).min(2.0);
    let w = (f32::from(bounds.size.width) - gap * (levels.len() - 1) as f32) / levels.len() as f32;
    let h = f32::from(bounds.size.height);
    for (i, v) in levels.iter().enumerate() {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        let x = f32::from(bounds.left()) + i as f32 * (w + gap);
        let top = f32::from(bounds.bottom()) - h * v;
        let r = style::snap(g, Bounds::from_corners(point(px(x), px(top)), point(px(x + w), bounds.bottom())));
        style::fill_rect(g, r, color);
    }
}

/// Levels (0 to 1) across `bounds`, each a band of its own over the one
/// before (the first fullest): a meter, as a volume's fullness.
pub fn paint_meter(g: &mut Painter, bounds: Bounds<Pixels>, levels: &[f32], color: Hsla) {
    if levels.is_empty() {
        return;
    }
    let h = f32::from(bounds.size.height) / levels.len() as f32;
    for (i, v) in levels.iter().enumerate() {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        let top = f32::from(bounds.top()) + h * i as f32;
        let c = if i == 0 { color } else { color.opacity(0.6) };
        let r = style::snap(g, Bounds::from_corners(point(bounds.left(), px(top)), point(bounds.left() + px(f32::from(bounds.size.width) * v), px(top + h))));
        style::fill_rect(g, r, c);
    }
}

/// The sunken well a small graph sits in (the document's ground, its
/// edge sunken), and the inside it leaves.
pub fn paint_well(g: &mut Painter, bounds: Bounds<Pixels>, p: &olo_theme::Palette) -> Bounds<Pixels> {
    let rect = style::snap(g, bounds);
    let inner = style::inset(g, rect, 2.0);
    style::fill_rect(g, inner, style::c(p.document_background));
    style::sunken(g, rect, p, false);
    inner
}

/// Levels (0 to 1) as a heatmap: a grid of cells across `bounds`, one a
/// level, each `ground` mixed toward `hot` as far as its level (a
/// processor's cores, idle to busy), laid out in as many rows as makes
/// the cells biggest, a device pixel apart.
pub fn paint_heatmap(g: &mut Painter, bounds: Bounds<Pixels>, levels: &[f32], ground: olo_theme::Rgba, hot: olo_theme::Rgba) {
    paint_heatmap_by(g, bounds, levels, |v| olo_theme::mix(ground, hot, 0.12 + 0.88 * if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }));
}

/// A level of use as a colour: the palette's green at none, its yellow
/// at half, its red at all (where full is what to watch: a Token Pool
/// quota used up).
pub fn usage_colour(p: &olo_theme::Palette, used: f32) -> olo_theme::Rgba {
    use olo_theme::Hue;
    let u = used.clamp(0.0, 1.0);
    if u < 0.5 { olo_theme::mix(p.hue(Hue::Green), p.hue(Hue::Yellow), u * 2.0) } else { olo_theme::mix(p.hue(Hue::Yellow), p.hue(Hue::Red), (u - 0.5) * 2.0) }
}

/// A heatmap whose cells are coloured by `colour` from their level (as
/// given: NaN for none), laid out as `paint_heatmap`'s.
pub fn paint_heatmap_by(g: &mut Painter, bounds: Bounds<Pixels>, levels: &[f32], colour: impl Fn(f32) -> olo_theme::Rgba) {
    let n = levels.len();
    if n == 0 {
        return;
    }
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let (rows, cols) = heatmap_grid(n, w, h);
    let gap = 1.0 / g.scale_factor();
    let (cw, ch) = ((w - gap * (cols - 1) as f32) / cols as f32, (h - gap * (rows - 1) as f32) / rows as f32);
    for (i, v) in levels.iter().enumerate() {
        let (r, c) = (i / cols, i % cols);
        let x = f32::from(bounds.left()) + c as f32 * (cw + gap);
        let y = f32::from(bounds.top()) + r as f32 * (ch + gap);
        let cell = style::snap(g, Bounds::from_corners(point(px(x), px(y)), point(px(x + cw), px(y + ch))));
        style::fill_rect(g, cell, style::c(colour(*v)));
    }
}

/// The rows and columns `n` cells are laid out in across `w` by `h`: as
/// many rows as make the cells biggest (squarest on a tie).
pub fn heatmap_grid(n: usize, w: f32, h: f32) -> (usize, usize) {
    let n = n.max(1);
    (1..=n)
        .map(|rows| (rows, n.div_ceil(rows)))
        .max_by(|a, b| {
            let size = |(r, c): (usize, usize)| (w / c as f32).min(h / r as f32);
            size(*a).total_cmp(&size(*b))
        })
        .unwrap_or((1, n))
}

#[cfg(test)]
mod heatmap_tests {
    /// Sixteen cores: four by four in a tray icon's square, one row of
    /// sixteen in a row's strip.
    #[test]
    fn a_heatmaps_grid() {
        assert_eq!(super::heatmap_grid(16, 14.0, 14.0), (4, 4));
        assert_eq!(super::heatmap_grid(16, 150.0, 12.0), (1, 16));
        assert_eq!(super::heatmap_grid(16, 150.0, 30.0), (2, 8));
        assert_eq!(super::heatmap_grid(1, 20.0, 20.0), (1, 1));
    }
}

/// A small graph of timed `points` (seconds, level 0 to 1, oldest first),
/// `now` at `bounds`' right edge and `span` seconds back its left, filled
/// under its line if `fill`: it scrolls as time passes, not a step a
/// reading (Deskbar's widgets; ask for the next drawing with
/// `Painter::paint_again_within`). A NaN level is a gap.
pub fn paint_trace(g: &mut Painter, bounds: Bounds<Pixels>, points: &[(f64, f32)], now: f64, span: f64, color: Hsla, fill: bool) {
    if points.len() < 2 || span <= 0.0 {
        return;
    }
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    let at = |t: f64, v: f32| point(bounds.right() - px(((now - t) / span) as f32 * w), bounds.bottom() - px(0.75 + v.clamp(0.0, 1.0) * (h - 1.5)));
    let start = points.partition_point(|(t, _)| *t < now - span).saturating_sub(1);
    g.with_content_mask(Some(olo_interface::ContentMask { bounds }), |g| {
        g.paint_layer(bounds, |g| {
            for pair in points[start..].windows(2) {
                let ((t0, v0), (t1, v1)) = (pair[0], pair[1]);
                if !v0.is_finite() || !v1.is_finite() {
                    continue;
                }
                let (a, b) = (at(t0, v0), at(t1, v1));
                if fill {
                    g.paint_area(a, b, bounds.bottom(), color.opacity(0.3));
                }
                g.paint_segment(a, b, px(1.25), true, color);
            }
        });
    });
}
