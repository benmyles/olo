//! Live graphs, LED cell grids and legends.
//!
//! A [`Graph`] (Haiku's ActivityMonitor, a Mac's Activity Monitor) draws
//! series of timed samples as lines (GPU segments), each over a faint
//! fill if asked, in a sunken document well with a grid in quarters and
//! time lines that move with the samples. Time runs right to left: `now`
//! is the right edge, `span` seconds back the left, so a smooth graph
//! scrolls between samples rather than jumping at each one (the next
//! frame when they have moved a device pixel). Hovering puts a time line
//! under the pointer and says each series' value there; agents read each
//! series' latest value. A [`CellGrid`] is a grid of small lit cells in
//! sunken wells, as Pulse's LED meters on BeOS: a heatmap of things going
//! on. [`legend`] lays out swatches, names and values in rows.

use std::{cell::RefCell, collections::VecDeque, rc::Rc};

use gpui::{
    App, Bounds, Context, Entity, FontWeight, HitboxBehavior, Hsla, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Render, RenderOnce, SharedString, Styled as _, Window,
    canvas, point, px, size,
};
use olo_kit_base::metrics as m;

use crate::{agent, style, theme};

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

/// A graph's state from frame to frame: the pointer over it, and how
/// far its samples move in a device pixel.
#[derive(Default)]
struct GraphState {
    hover: Option<Point<Pixels>>,
    /// Seconds a device pixel stands for, as last drawn.
    per_pixel: f64,
}

type TimeFn = Rc<dyn Fn(f64, &mut Window, &mut App)>;

/// A live graph of [`Series`]; it fills the box it's given.
#[derive(Clone, IntoElement)]
pub struct Graph {
    id: SharedString,
    series: Rc<[Series]>,
    now: f64,
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
    /// A graph of `series` at time `now` (seconds, as their samples').
    pub fn new(id: impl Into<SharedString>, series: impl Into<Rc<[Series]>>, now: f64) -> Graph {
        Graph { id: id.into(), series: series.into(), now, span: 60.0, max: None, unit: None, grid_seconds: 10.0, smooth: true, well: true, on_click: None, on_hover: None }
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

    /// Scroll smoothly between samples, a device pixel at a time (the
    /// default), or only when the app draws it again.
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
    pub fn on_click(mut self, f: impl Fn(f64, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    /// The time under the pointer as it moves over the graph (for a
    /// drill-down).
    pub fn on_hover(mut self, f: impl Fn(f64, &mut Window, &mut App) + 'static) -> Self {
        self.on_hover = Some(Rc::new(f));
        self
    }

    /// At another time (a [`GraphLayer`]'s now).
    pub fn at(mut self, now: f64) -> Self {
        self.now = now;
        self
    }

    /// Drawn on `shelf`'s [`GraphLayer`], not here: this keeps its place
    /// (filling the box it's given, as the graph would) and says where it
    /// is, and what it's clipped to, as it's laid out.
    pub fn on_layer(self, shelf: &GraphShelf) -> impl IntoElement {
        let shelf = shelf.clone();
        canvas(move |b, window, _| shelf.0.borrow_mut().push(Placed { bounds: b, clip: window.content_mask(), graph: self }), |_, _, _, _| {}).size_full()
    }
}

/// Where a view's graphs go on its [`GraphLayer`]: the view puts each
/// with [`Graph::on_layer`] and empties it as it renders again.
#[derive(Clone, Default)]
pub struct GraphShelf(Rc<RefCell<Vec<Placed>>>);

struct Placed {
    bounds: Bounds<Pixels>,
    clip: gpui::ContentMask<Pixels>,
    graph: Graph,
}

impl GraphShelf {
    pub fn clear(&self) {
        self.0.borrow_mut().clear();
    }
}

/// Graphs moving over a view that stays still. A graph scrolling a
/// device pixel at a time otherwise redraws the view it's in each time
/// (every card, legend and figure of a dashboard: its layout, text and
/// paint) for the graph alone to move. Put the view under its layer, both
/// cached (`AnyView::cached`), the view's graphs on the layer's shelf
/// (`Graph::on_layer`): as the graphs move, only the layer draws again
/// and the view's last frame is kept. The view lays the graphs out
/// before the layer draws them (in its prepaint), so moving it (a scroll)
/// moves them with it. The layer draws each at `now`, and must be told
/// when the samples change (`cx.notify()`, or observing what has them).
pub struct GraphLayer {
    shelf: GraphShelf,
    now: Rc<dyn Fn(&App) -> f64>,
}

impl GraphLayer {
    pub fn new(shelf: GraphShelf, now: impl Fn(&App) -> f64 + 'static) -> GraphLayer {
        GraphLayer { shelf, now: Rc::new(now) }
    }
}

impl Render for GraphLayer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (shelf, now) = (self.shelf.clone(), (self.now)(cx));
        canvas(
            move |_, window, cx| {
                let placed: Vec<(gpui::ContentMask<Pixels>, Bounds<Pixels>, Graph)> = shelf.0.borrow().iter().map(|p| (p.clip.clone(), p.bounds, p.graph.clone())).collect();
                placed
                    .into_iter()
                    .map(|(clip, b, graph)| {
                        let mut el = graph.at(now).into_any_element();
                        window.with_content_mask(Some(clip.clone()), |window| el.prepaint_as_root(b.origin, b.size.into(), window, cx));
                        (clip, el)
                    })
                    .collect::<Vec<_>>()
            },
            |_, graphs, window, cx| {
                for (clip, mut el) in graphs {
                    window.with_content_mask(Some(clip), |window| el.paint(window, cx));
                }
            },
        )
        .size_full()
    }
}

impl RenderOnce for Graph {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state: Entity<GraphState> = window.use_keyed_state(SharedString::from(format!("olo-graph-{}", self.id)), cx, |_, _| GraphState::default());
        // The next frame when the samples have moved a device pixel.
        let per_pixel = state.read(cx).per_pixel;
        if self.smooth && per_pixel > 0.0 {
            clock::tick_within(window, cx, per_pixel);
        }
        let hover = state.read(cx).hover;
        let Graph { series, now, span, max, unit, grid_seconds, well, on_click, on_hover, .. } = self;
        canvas(
            move |b, window, _| window.insert_hitbox(b, HitboxBehavior::Normal),
            move |b, hitbox, window, cx| {
                let p = theme::palette(cx);
                let s = window.scale_factor();
                let rect = style::snap(window, b);
                let inner = if well { style::inset(window, rect, 2.0) } else { rect };
                if well {
                    style::fill_rect(window, inner, style::c(p.document_background));
                    style::sunken(window, rect, &p, false);
                }
                let per_pixel = span / (f32::from(inner.size.width) * s).max(1.0) as f64;
                if state.read(cx).per_pixel != per_pixel {
                    state.update(cx, |st, _| st.per_pixel = per_pixel);
                }
                let width = f32::from(inner.size.width);
                let x_of = move |t: f64| inner.right() - px(((now - t) / span) as f32 * width);
                let time_at = move |x: Pixels| now - (f32::from(inner.right() - x) / width) as f64 * span;
                let say = |series: &Series, v: f64| match (&series.format, &unit) {
                    (Some(f), _) | (None, Some(f)) => f(v),
                    (None, None) => format!("{v:.1}"),
                };
                window.with_content_mask(Some(gpui::ContentMask { bounds: inner }), |window| {
                    // The grid, the fills and the lines in one layer: one
                    // place in the scene's order for all of them rather
                    // than one each, found among everything drawn before
                    // (in a layer, quads go under paths under segments).
                    window.paint_layer(inner, |window| {
                        // The grid: quarters across, and a time line every so
                        // often that moves with the samples.
                        let grid = style::c(olo_theme::mix(p.document_text, p.document_background, 0.9));
                        for k in 1..4 {
                            style::hline(window, inner.left(), inner.right(), inner.top() + inner.size.height * (k as f32 / 4.0), grid);
                        }
                        if !well {
                            let base = style::c(olo_theme::mix(p.document_text, p.document_background, 0.75));
                            style::hline_above(window, inner.left(), inner.right(), inner.bottom(), base);
                        }
                        let mut t = (now / grid_seconds).floor() * grid_seconds;
                        while t > now - span {
                            style::vline(window, x_of(t), inner.top(), inner.bottom(), grid);
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
                                        window.paint_area(w[0], w[1], inner.bottom(), under);
                                    }
                                }
                            }
                            for run in &runs {
                                for w in run.windows(2) {
                                    window.paint_segment(w[0], w[1], px(1.5), true, line.color);
                                }
                            }
                        }
                    });
                    // Hovering: the time under the pointer and each value
                    // then.
                    if let Some(pos) = hover.filter(|pos| inner.contains(pos)) {
                        let t = time_at(pos.x);
                        style::vline(window, pos.x, inner.top(), inner.bottom(), style::c(olo_theme::mix(p.document_text, p.document_background, 0.5)));
                        let lines: Vec<(Hsla, String)> = series.iter().map(|line| (line.color, format!("{}: {}", line.name, line.at(t).map(|v| say(line, v)).unwrap_or_else(|| "–".into())))).collect();
                        readout(window, cx, inner, pos, &lines, ago(now - t));
                    }
                });
                // What agents read: each series' latest value.
                let said = series.iter().map(|line| format!("{}: {}", line.name, line.latest().map(|v| say(line, v)).unwrap_or_else(|| "–".into()))).collect::<Vec<_>>().join(", ");
                agent::record(
                    window,
                    cx,
                    agent::Node { role: "Figure", name: Some("Graph".into()), value: Some(said.into()), bounds: rect, enabled: true, clickable: on_click.is_some(), ..Default::default() },
                );

                let (st, hb, hover_f) = (state.clone(), hitbox.clone(), on_hover.clone());
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if !phase.bubble() {
                        return;
                    }
                    let over = hb.is_hovered(window) && inner.contains(&e.position);
                    let now_at = over.then_some(e.position);
                    if st.read(cx).hover != now_at {
                        st.update(cx, |s, cx| {
                            s.hover = now_at;
                            cx.notify();
                        });
                    }
                    if let (Some(pos), Some(f)) = (now_at, &hover_f) {
                        f(time_at(pos.x), window, cx);
                    }
                });
                if let Some(f) = on_click {
                    window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                        if phase.bubble() && e.button == MouseButton::Left && hitbox.is_hovered(window) && inner.contains(&e.position) {
                            f(time_at(e.position.x), window, cx);
                            // (Its own: not the card's it's on too.)
                            cx.stop_propagation();
                        }
                    });
                }
            },
        )
        .size_full()
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
#[derive(IntoElement)]
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

impl RenderOnce for Spectrum {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let Spectrum { series, low, high, floor } = self;
        canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let well = Bounds::new(b.origin, size(b.size.width, (b.size.height - px(SPECTRUM_LABELS)).max(px(0.0))));
                let rect = style::snap(window, well);
                let inner = style::inset(window, rect, 2.0);
                style::fill_rect(window, inner, style::c(p.document_background));
                style::sunken(window, rect, &p, false);
                let (w, h) = (f32::from(inner.size.width), f32::from(inner.size.height));
                let span = (high / low).ln().max(1e-6);
                let x_of = |f: f32| inner.left() + px((f / low).ln() / span * w);
                let y_of = |db: f32| inner.bottom() - px(1.5 + ((db - floor) / -floor).clamp(0.0, 1.0) * (h - 3.0));
                let grid = style::c(olo_theme::mix(p.document_text, p.document_background, 0.9));
                let dim = style::disabled_text(&p);
                let decades: Vec<f32> = [100.0, 1000.0, 10_000.0].into_iter().filter(|f| (low..=high).contains(f)).collect();
                window.with_content_mask(Some(gpui::ContentMask { bounds: inner }), |window| {
                    window.paint_layer(inner, |window| {
                        let mut db = -20.0;
                        while db > floor {
                            style::hline(window, inner.left(), inner.right(), y_of(db), grid);
                            db -= 20.0;
                        }
                        for f in &decades {
                            style::vline(window, x_of(*f), inner.top(), inner.bottom(), grid);
                        }
                        // (A silent one isn't drawn: no line along the floor.)
                        for line in series.iter().filter(|l| l.levels.iter().any(|v| *v > floor)) {
                            let n = line.levels.len().max(1);
                            let pts: Vec<Point<Pixels>> = line.levels.iter().enumerate().map(|(i, db)| point(inner.left() + px((i as f32 + 0.5) / n as f32 * w), y_of(*db))).collect();
                            if line.fill {
                                for pair in pts.windows(2) {
                                    window.paint_area(pair[0], pair[1], inner.bottom(), line.color.opacity(40.0 / 255.0));
                                }
                            }
                            for pair in pts.windows(2) {
                                window.paint_segment(pair[0], pair[1], px(1.5), true, line.color);
                            }
                        }
                    });
                });
                // The frequencies under the well, each under its line.
                let baseline = style::baseline_at(window, rect.bottom() + px(SPECTRUM_LABELS / 2.0), style::SMALL_SIZE);
                for f in &decades {
                    style::label_on_baseline(window, cx, x_of(*f), baseline, style::Align::Center, hertz(*f), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), dim);
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
                agent::record(window, cx, agent::Node { role: "Figure", name: Some("Spectrum".into()), value: Some(said.into()), bounds: rect, enabled: true, ..Default::default() });
            },
        )
        .size_full()
    }
}

/// When a window's graphs move: one clock for all of them, so they move
/// together. Each graph drawn says how often its samples move a device
/// pixel; the clock ticks at the finest of those (a minute across 600
/// pixels is 10 frames a second, not 120 that would draw the same
/// pixels), and each tick redraws the views that have graphs. Graphs of
/// many sizes ticking on their own made a frame for each (twelve small
/// ones and a large one: twice the frames, all but the large one's
/// moving nothing a pixel). Once no graph is drawn, it stops.
mod clock {
    use std::{collections::HashMap, time::Duration};

    use gpui::{App, EntityId, Global, Task, Window, WindowId};

    struct Clock {
        /// The finest step of the graphs drawn before the last tick, and
        /// since.
        pace: f64,
        finest: f64,
        /// The views drawn with graphs since the last tick.
        views: Vec<EntityId>,
        next: Option<Task<()>>,
    }

    #[derive(Default)]
    struct Clocks(HashMap<WindowId, Clock>);

    impl Global for Clocks {}

    /// A graph is being drawn whose samples move a device pixel every
    /// `per_pixel` seconds.
    pub fn tick_within(window: &mut Window, cx: &mut App, per_pixel: f64) {
        let (id, view) = (window.window_handle().window_id(), window.current_view());
        let clock = cx.default_global::<Clocks>().0.entry(id).or_insert_with(|| Clock { pace: f64::MAX, finest: f64::MAX, views: Vec::new(), next: None });
        clock.finest = clock.finest.min(per_pixel);
        if !clock.views.contains(&view) {
            clock.views.push(view);
        }
        if clock.next.is_some() {
            return;
        }
        let wait = Duration::from_secs_f64(clock.pace.min(clock.finest).clamp(1.0 / 240.0, 1.0));
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(wait).await;
            let _ = cx.update(|cx| tick(id, cx));
        });
        if let Some(clock) = cx.default_global::<Clocks>().0.get_mut(&id) {
            clock.next = Some(task);
        }
    }

    fn tick(id: WindowId, cx: &mut App) {
        let clocks = &mut cx.default_global::<Clocks>().0;
        let Some(clock) = clocks.get_mut(&id) else { return };
        clock.next = None;
        let views = std::mem::take(&mut clock.views);
        if views.is_empty() {
            clocks.remove(&id);
            return;
        }
        clock.pace = clock.finest;
        clock.finest = f64::MAX;
        for view in views {
            cx.notify(view);
        }
    }
}

/// The values under the pointer, in a tip-coloured box beside it (kept
/// inside the graph).
fn readout(window: &mut Window, cx: &mut App, inside: Bounds<Pixels>, at: Point<Pixels>, lines: &[(Hsla, String)], when: String) {
    let p = theme::palette(cx);
    let font = style::ui_font(FontWeight::NORMAL);
    let texts: Vec<String> = std::iter::once(when).chain(lines.iter().map(|(_, t)| t.clone())).collect();
    let row = style::SMALL_SIZE + 3.0;
    let w = texts.iter().map(|t| f32::from(style::text_width(window, t.clone(), style::SMALL_SIZE, font.clone()))).fold(0.0, f32::max) + 22.0;
    let h = texts.len() as f32 * row + 6.0;
    let mut r = Bounds { origin: point(at.x + px(10.0), at.y + px(10.0)), size: size(px(w), px(h)) };
    if r.right() > inside.right() {
        r.origin.x -= px(w + 20.0);
    }
    if r.bottom() > inside.bottom() {
        r.origin.y -= r.bottom() - inside.bottom();
    }
    let r = style::snap(window, r);
    window.paint_quad(gpui::fill(r, style::c(p.tooltip_background)).corner_radii(px(2.0)));
    style::frame_rect(window, r, style::c(p.control_border));
    for (i, text) in texts.into_iter().enumerate() {
        let y = r.top() + px(3.0 + i as f32 * row);
        if i > 0 {
            let swatch = style::snap_centred(window, point(r.left() + px(9.0), y + px(row / 2.0 - 1.0)), 7.0, 7.0);
            style::fill_rect(window, swatch, lines[i - 1].0);
        }
        let x = r.left() + px(if i > 0 { 17.0 } else { 5.0 });
        style::label(window, cx, x, y + px(row / 2.0), style::Align::Left, text, style::SMALL_SIZE, font.clone(), style::c(p.tooltip_text));
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

type CellFn = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// A grid of small lit cells in sunken wells, as Pulse's LED meters on
/// BeOS. Its height is fixed (`rows` cells of `cell_size`); with more
/// cells than fit, they shrink and more rows and columns fit in the same
/// space. Each cell can be clicked (and is named for agents).
#[derive(IntoElement)]
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

    pub fn on_click(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
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

impl RenderOnce for CellGrid {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let height = self.rows as f32 * self.cell + (self.rows as f32 - 1.0) * self.gap;
        let CellGrid { cells, cell, gap, slots, fill, on_click, .. } = self;
        let count = cells.len();
        canvas(
            move |b, window, _| {
                let n = count.max(slots);
                let width = f32::from(b.size.width);
                let (size, cols, rows) = fit(cell, gap, n, width, height);
                let wells = if fill { cols * rows } else { n.min(cols * rows) };
                // Filling: spread the columns to both edges.
                let step_x = if fill && cols > 1 { (width - size) / (cols - 1) as f32 } else { size + gap };
                (0..wells)
                    .map(|i| {
                        let (row, col) = (i / cols, i % cols);
                        let r = style::snap(window, Bounds { origin: point(b.left() + px(col as f32 * step_x), b.top() + px(row as f32 * (size + gap))), size: size_of(size) });
                        (r, window.insert_hitbox(r, HitboxBehavior::Normal))
                    })
                    .collect::<Vec<_>>()
            },
            move |_, wells, window, cx| {
                let p = theme::palette(cx);
                for (i, (well, hitbox)) in wells.iter().enumerate() {
                    // The well: sunken, as an unlit LED.
                    style::fill_rect(window, *well, style::shade(&p, p.panel_background, 1.12));
                    style::bevel(window, *well, style::shade(&p, p.panel_background, 1.4), style::c(p.shine));
                    let Some(item) = cells.get(i) else { continue };
                    let inner = style::inset(window, *well, 1.0);
                    if let Some(color) = item.color {
                        let rgb = gpui::Rgba::from(color);
                        let scale = |rgb: gpui::Rgba, k: f32| -> Hsla { gpui::Rgba { r: (rgb.r * k).min(1.0), g: (rgb.g * k).min(1.0), b: (rgb.b * k).min(1.0), a: 1.0 }.into() };
                        let lit = if hitbox.is_hovered(window) { scale(rgb, 1.15) } else { color };
                        style::vgradient(window, inner, lit, scale(gpui::Rgba::from(lit), 0.8));
                        style::hline(window, inner.left(), inner.right(), inner.top(), style::c(p.gloss));
                    }
                    let side = f32::from(well.size.width);
                    if let Some(mark) = item.mark.clone().filter(|_| side >= 11.0) {
                        // Centred on its capitals, kept inside the well.
                        let ink = item.color.map_or(style::c(p.panel_text), |fill| {
                            let c = gpui::Rgba::from(fill);
                            style::c(p.ink_on([(c.r * 255.0) as u8, (c.g * 255.0) as u8, (c.b * 255.0) as u8, 255]))
                        });
                        window.with_content_mask(Some(gpui::ContentMask { bounds: inner }), |window| {
                            style::label(window, cx, inner.center().x, inner.center().y, style::Align::Center, mark, (side * 0.64).round(), style::ui_font(FontWeight::NORMAL), ink);
                        });
                    }
                    // Too small for a plate, and flush with its neighbours:
                    // a hairline in the selection's edge colour round it.
                    if item.selected {
                        let line = style::hairline(window);
                        let out = Bounds::from_corners(point(well.left() - line, well.top() - line), point(well.right() + line, well.bottom() + line));
                        style::frame_rect(window, out, style::c(p.selection_edge()));
                    }
                    agent::record(
                        window,
                        cx,
                        agent::Node { role: "Button", name: Some(item.name.clone()), bounds: *well, enabled: true, clickable: on_click.is_some(), selected: Some(item.selected), ..Default::default() },
                    );
                }
                if let Some(f) = on_click {
                    let n = cells.len();
                    window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                        if !phase.bubble() || e.button != MouseButton::Left {
                            return;
                        }
                        if let Some(i) = wells.iter().take(n).position(|(_, h)| h.is_hovered(window)) {
                            f(i, window, cx);
                            cx.stop_propagation();
                        }
                    });
                }
            },
        )
        .w_full()
        .h(px(height))
        .flex_shrink_0()
    }
}

fn size_of(side: f32) -> gpui::Size<Pixels> {
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
fn legend_places(cx: &App, width: f32, items: &[LegendItem]) -> Vec<(f32, usize)> {
    let (plain, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
    let mut out = Vec::new();
    let (mut x, mut row) = (0.0, 0);
    for it in items {
        let w = swatch_w(it) + style::text_width_app(cx, &it.name, style::DETAIL_SIZE, &plain) + 6.0 + style::text_width_app(cx, &it.value, style::SMALL_SIZE, &bold) + 2.0 * m::SPACING;
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
pub fn legend_height(cx: &App, width: f32, items: &[LegendItem]) -> f32 {
    legend_places(cx, width, items).last().map_or(0.0, |(_, row)| (*row + 1) as f32 * FACT_ROW)
}

/// Swatches, names and values in a row, wrapping, in the document's text
/// colours, drawn in `r`.
pub fn legend(window: &mut Window, cx: &mut App, r: Bounds<Pixels>, items: &[LegendItem]) {
    let p = theme::palette(cx);
    let text = style::c(p.document_text);
    let dim = text.opacity(0.52);
    let places = legend_places(cx, f32::from(r.size.width), items);
    // (Its items go where their lengths put them: lined up with what's
    // above or below only by chance.)
    crate::review::own_grid(window, r);
    let (plain, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
    window.with_content_mask(Some(gpui::ContentMask { bounds: r }), |window| {
        for (it, (x, row)) in items.iter().zip(&places) {
            let mid = r.top() + px(*row as f32 * FACT_ROW + FACT_ROW / 2.0);
            let left = r.left() + px(*x);
            if it.color.a > 0.0 {
                let swatch = style::snap_centred(window, point(left + px(SWATCH / 2.0), mid), SWATCH, SWATCH);
                window.paint_quad(gpui::fill(swatch, it.color).corner_radii(px(2.0)));
            }
            // Name and value on one baseline (their sizes differ).
            let baseline = style::baseline_at(window, mid, style::DETAIL_SIZE);
            let name = style::label_on_baseline(window, cx, left + px(swatch_w(it)), baseline, style::Align::Left, it.name.clone(), style::DETAIL_SIZE, plain.clone(), dim);
            style::label_on_baseline(window, cx, name.right() + px(6.0), baseline, style::Align::Left, it.value.clone(), style::SMALL_SIZE, bold.clone(), text);
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
        let s = Series::new("x", gpui::red(), Rc::new(samples));
        assert_eq!(s.at(0.5), None);
        assert_eq!(s.at(1.5), Some(10.0));
        // A gap reads as nothing.
        assert_eq!(s.at(2.5), None);
        assert_eq!(s.at(9.0), Some(30.0));
        assert_eq!(s.latest(), Some(30.0));
        assert_eq!(ago(0.4), "now");
        assert_eq!(ago(30.0), "30 s ago");
    }

    #[test]
    fn cells_fit() {
        // Two rows of 14: 40 cells in 200 points shrink until they fit.
        let (size, cols, rows) = fit(14.0, 2.0, 40, 200.0, 30.0);
        assert!(cols * rows >= 40, "{size} {cols} {rows}");
        assert_eq!(fit(12.0, 2.0, 3, 200.0, 12.0), (12.0, 14, 1));
    }
}
