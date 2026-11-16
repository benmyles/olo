//! A forecast's chart: time running left to right from now into what's
//! ahead (Weather's hours and days; anything planned by the hour), in
//! panels stacked on one time axis.
//!
//! A [`Graph`](super::Graph) is a monitor's: what was, the present at its
//! right edge, moving. A forecast is read the other way, and many things
//! about one hour at once (the temperature, the clouds' base, the rain's
//! chance, the wind), so a [`Timeline`] is panels of [`Track`]s (lines,
//! columns, a low-to-high range per step) over one axis of hours and
//! days, with strips of pictures or wind arrows between them, the nights
//! shaded, the present marked, spans named across all of it (an
//! assistant's "a good time to run"), and one line under the pointer
//! through every panel, with a readout of every track's value then. It is
//! one drawing: hovering draws only it again, nothing else.
//!
//! Times are seconds (Unix time); `local` turns one into the place's own
//! clock (seconds from its 1970-01-01 midnight), for the hours and day
//! names on the axis and in the readout, which may be another time zone's
//! than the computer's. Agents read each panel: its tracks' first value,
//! their lowest and highest and when.

use std::rc::Rc;

use olo_interface::{
    Bounds, Element, FontWeight, Hsla, InteractiveElement as _, IntoElement, MouseMoveEvent, Painter, Pixels, Point, SharedString, Styled as _, WindowCx, agent, canvas, point, px, size,
};

use crate::{icons, style, theme};

/// How a value reads (with its unit).
pub type Say = Rc<dyn Fn(f64) -> String>;

/// How a track draws its points.
#[derive(Clone)]
pub enum Mark {
    /// A line through them, `width` points wide (a NaN value: a gap).
    Line { width: f32 },
    /// A column for each, its step wide from its time, from the panel's
    /// floor up to its value; each its own colour if `colors` has one.
    Columns { colors: Option<Rc<[Hsla]>> },
    /// A column for each from `lows`' value up to its own (a cloud layer's
    /// base to its top), its colour as opaque as `alpha` says (0 to 1).
    Ranges { lows: Rc<[f64]>, alpha: Option<Rc<[f32]>> },
    /// A column for each from its value up to the panel's top (cloud over
    /// its base), as opaque as `alpha` says, its bottom edge solid: values
    /// that leap from one step to the next read as the clear air under
    /// them coming and going, not a line's zigzag.
    Above { alpha: Option<Rc<[f32]>> },
    /// Not drawn: read under the pointer and by agents only (what another
    /// track shows another way, as the cover that shades the cloud).
    Quiet,
}

/// One thing a panel shows: its name, colour and points, `(time, value)`.
#[derive(Clone)]
pub struct Track {
    pub name: SharedString,
    pub color: Hsla,
    pub points: Rc<[(f64, f64)]>,
    pub mark: Mark,
    /// On the panel's right-hand scale.
    pub right: bool,
    /// How its values read, when not as its scale's.
    pub say: Option<Say>,
}

impl Track {
    pub fn line(name: impl Into<SharedString>, color: Hsla, points: impl Into<Rc<[(f64, f64)]>>) -> Track {
        Track { name: name.into(), color, points: points.into(), mark: Mark::Line { width: 2.0 }, right: false, say: None }
    }

    pub fn columns(name: impl Into<SharedString>, color: Hsla, points: impl Into<Rc<[(f64, f64)]>>) -> Track {
        Track { mark: Mark::Columns { colors: None }, ..Track::line(name, color, points) }
    }

    /// From each of `lows` up to its point's value.
    pub fn ranges(name: impl Into<SharedString>, color: Hsla, points: impl Into<Rc<[(f64, f64)]>>, lows: impl Into<Rc<[f64]>>) -> Track {
        Track { mark: Mark::Ranges { lows: lows.into(), alpha: None }, ..Track::line(name, color, points) }
    }

    /// From each point's value up to the panel's top ([`Mark::Above`]).
    pub fn above(name: impl Into<SharedString>, color: Hsla, points: impl Into<Rc<[(f64, f64)]>>) -> Track {
        Track { mark: Mark::Above { alpha: None }, ..Track::line(name, color, points) }
    }

    /// Read out but not drawn ([`Mark::Quiet`]); `say` says how its values
    /// read (it has no scale of its own).
    pub fn quiet(name: impl Into<SharedString>, color: Hsla, points: impl Into<Rc<[(f64, f64)]>>) -> Track {
        Track { mark: Mark::Quiet, ..Track::line(name, color, points) }
    }

    /// A thinner line (what goes with another: gusts with the wind).
    pub fn thin(mut self) -> Self {
        if let Mark::Line { width } = &mut self.mark {
            *width = 1.0;
        }
        self
    }

    /// Each column its own colour.
    pub fn colors(mut self, colors: impl Into<Rc<[Hsla]>>) -> Self {
        if let Mark::Columns { colors: c } = &mut self.mark {
            *c = Some(colors.into());
        }
        self
    }

    /// Each range (or column above) as opaque as this says (0 to 1).
    pub fn alpha(mut self, alpha: impl Into<Rc<[f32]>>) -> Self {
        if let Mark::Ranges { alpha: a, .. } | Mark::Above { alpha: a } = &mut self.mark {
            *a = Some(alpha.into());
        }
        self
    }

    pub fn on_right(mut self) -> Self {
        self.right = true;
        self
    }

    pub fn say(mut self, f: impl Fn(f64) -> String + 'static) -> Self {
        self.say = Some(Rc::new(f));
        self
    }

    /// Its point for time `t`: a column's whose step holds it, else the
    /// nearest; None where there's no value.
    fn at(&self, t: f64, step: f64) -> Option<usize> {
        let i = match self.mark {
            Mark::Line { .. } => {
                let i = self.points.partition_point(|(pt, _)| *pt <= t);
                match (i.checked_sub(1), (i < self.points.len()).then_some(i)) {
                    (Some(a), Some(b)) => Some(if t - self.points[a].0 <= self.points[b].0 - t { a } else { b }),
                    (a, b) => a.or(b),
                }
            }
            _ => self.points.partition_point(|(pt, _)| *pt <= t).checked_sub(1).filter(|&i| t < self.points[i].0 + step),
        };
        i.filter(|&i| self.points[i].1.is_finite())
    }
}

/// A panel's scale: fixed ends, or what its tracks show (from 0 if
/// `from_zero`), and how its values read.
#[derive(Clone)]
pub struct Scale {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub from_zero: bool,
    pub say: Say,
}

impl Scale {
    pub fn new(say: impl Fn(f64) -> String + 'static) -> Scale {
        Scale { min: None, max: None, from_zero: false, say: Rc::new(say) }
    }

    pub fn range(mut self, min: f64, max: f64) -> Self {
        (self.min, self.max) = (Some(min), Some(max));
        self
    }

    /// At least up to `max` (more if a value is higher).
    pub fn at_least(mut self, max: f64) -> Self {
        self.max = Some(max);
        self.from_zero = true;
        self
    }

    pub fn from_zero(mut self) -> Self {
        self.from_zero = true;
        self
    }

    /// Its ends for `values`, nice numbers, and its ticks between.
    fn ends(&self, values: impl Iterator<Item = f64> + Clone) -> (f64, f64, f64) {
        let (lo, hi) = values.filter(|v| v.is_finite()).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
        let (mut lo, mut hi) = if lo.is_finite() { (lo, hi) } else { (0.0, 1.0) };
        if self.from_zero {
            lo = lo.min(0.0);
        }
        if let Some(m) = self.min {
            lo = m;
        }
        match (self.max, self.from_zero) {
            (Some(m), true) => hi = hi.max(m),
            (Some(m), false) => hi = m,
            _ => {}
        }
        if hi - lo < 1e-9 {
            hi = lo + 1.0;
        }
        // Three or four ticks of a nice step.
        let step = nice_step((hi - lo) / 3.0);
        let lo = if self.min.is_some() { lo } else { (lo / step).floor() * step };
        let hi = if self.max.is_some() && !self.from_zero { hi } else { (hi / step).ceil() * step };
        (lo, hi, step)
    }
}

/// The nice number (1, 2, 2.5, 5 × 10ⁿ) nearest above `v`.
fn nice_step(v: f64) -> f64 {
    if !(v > 0.0) {
        return 1.0;
    }
    let p = 10f64.powf(v.log10().floor());
    let f = v / p;
    p * if f <= 1.0 {
        1.0
    } else if f <= 2.0 {
        2.0
    } else if f <= 2.5 {
        2.5
    } else if f <= 5.0 {
        5.0
    } else {
        10.0
    }
}

/// What a panel is.
#[derive(Clone)]
pub enum Kind {
    /// Tracks against its scales.
    Plot { tracks: Vec<Track>, left: Scale, right: Option<Scale>, guides: Vec<(f64, SharedString)> },
    /// A picture at each time (a condition's icon).
    Icons(Rc<[(f64, SharedString)]>),
    /// An arrow at each time, pointing where the wind goes from the
    /// direction it comes from (degrees, clockwise from north).
    Arrows(Rc<[(f64, f64)]>, Hsla),
}

/// One band of the chart: a title over a plot or a strip.
#[derive(Clone)]
pub struct Panel {
    pub title: SharedString,
    pub height: f32,
    pub kind: Kind,
}

impl Panel {
    /// A plot `height` points tall, on `left`'s scale.
    pub fn plot(title: impl Into<SharedString>, height: f32, left: Scale) -> Panel {
        Panel { title: title.into(), height, kind: Kind::Plot { tracks: Vec::new(), left, right: None, guides: Vec::new() } }
    }

    pub fn icons(points: impl Into<Rc<[(f64, SharedString)]>>) -> Panel {
        Panel { title: SharedString::default(), height: ICON_ROW, kind: Kind::Icons(points.into()) }
    }

    pub fn arrows(points: impl Into<Rc<[(f64, f64)]>>, color: Hsla) -> Panel {
        Panel { title: SharedString::default(), height: ARROW_ROW, kind: Kind::Arrows(points.into(), color) }
    }

    pub fn track(mut self, track: Track) -> Self {
        if let Kind::Plot { tracks, .. } = &mut self.kind {
            tracks.push(track);
        }
        self
    }

    /// A second scale, on the right, for the tracks put [on
    /// it](Track::on_right).
    pub fn right(mut self, scale: Scale) -> Self {
        if let Kind::Plot { right, .. } = &mut self.kind {
            *right = Some(scale);
        }
        self
    }

    /// A line across at `value` (on the left scale), named: freezing,
    /// fog's visibility.
    pub fn guide(mut self, value: f64, name: impl Into<SharedString>) -> Self {
        if let Kind::Plot { guides, .. } = &mut self.kind {
            guides.push((value, name.into()));
        }
        self
    }

    /// How tall it is with its title.
    fn extent(&self) -> f32 {
        let title = if self.title.is_empty() { 0.0 } else { TITLE_ROW };
        title + self.height + GAP
    }
}

/// A panel's title row, a strip's rows, and the gap under each.
const TITLE_ROW: f32 = 20.0;
const ICON_ROW: f32 = 24.0;
const ARROW_ROW: f32 = 18.0;
const GAP: f32 = 6.0;
/// How far in from a plot's top and bottom its scale's ends are.
const PAD: f32 = 2.0;
/// The rows of spans' names over the first plot, and each one's height.
const SPAN_ROWS: usize = 2;
const SPAN_ROW: f32 = 16.0;
/// The axis's two rows: the hours, the days.
const AXIS: f32 = 34.0;
/// How tall the axis is ([`Timeline::axis`]).
pub const AXIS_HEIGHT: f32 = AXIS;
/// Room each side for the scales' values.
const GUTTER: f32 = 44.0;

/// A span named across the chart: a time to go out, a storm.
#[derive(Clone)]
pub struct Span {
    pub start: f64,
    pub end: f64,
    pub name: SharedString,
}

/// The chart; it is as tall as its panels and fills the width it's given.
#[derive(Clone)]
pub struct Timeline {
    id: SharedString,
    start: f64,
    end: f64,
    step: f64,
    panels: Rc<[Panel]>,
    now: Option<f64>,
    nights: Rc<[(f64, f64)]>,
    spans: Rc<[Span]>,
    local: Rc<dyn Fn(f64) -> i64>,
    key: u64,
    on_hover: Option<Rc<dyn Fn(&Option<f64>, &mut WindowCx)>>,
    /// The axis over the panels rather than under them.
    axis_on_top: bool,
}

/// What the chart keeps between drawings: the pointer over it, and
/// where its plots are.
#[derive(Default)]
struct State {
    /// The pointer, where the chart is drawn (not the window's: a scroll
    /// view round it moves what it shows, not where it's drawn).
    hover: Option<Point<Pixels>>,
    plots: Bounds<Pixels>,
    /// Where the chart was drawn, and what shows of it (as the pointer
    /// last moved), for the readout.
    drawn_at: Point<Pixels>,
    shown: Bounds<Pixels>,
    start: f64,
    end: f64,
}

impl State {
    fn time_at(&self, x: Pixels) -> f64 {
        self.start + (f32::from(x - self.plots.left()) / f32::from(self.plots.size.width).max(1.0)) as f64 * (self.end - self.start)
    }
}

impl Timeline {
    /// From `start` to `end` (seconds), each point `step` seconds apart
    /// (an hour: what a column spans and the pointer snaps to); `key`
    /// says what it shows (what changes it: the data, the units), so it's
    /// drawn again only when that changes or the pointer moves over it.
    pub fn new(id: impl Into<SharedString>, start: f64, end: f64, step: f64, panels: impl Into<Rc<[Panel]>>, key: u64) -> Timeline {
        Timeline {
            id: id.into(),
            start,
            end: end.max(start + 1.0),
            step: step.max(1.0),
            panels: panels.into(),
            now: None,
            nights: Rc::from(Vec::new()),
            spans: Rc::from(Vec::new()),
            local: Rc::new(|t| t as i64),
            key,
            on_hover: None,
            axis_on_top: false,
        }
    }

    /// The axis over the panels (the days, then the hours over the
    /// plots), not under them: where it's read first, and where a scroll
    /// view can keep a copy of it ([`Self::axis`]) as the panels scroll
    /// under it (Ben, 2026-11-15: "the timeline should be at the top and
    /// stay there as you scroll the data so that you always know when it
    /// is").
    pub fn axis_on_top(mut self, on: bool) -> Self {
        self.axis_on_top = on;
        self
    }

    /// The axis alone, as the chart draws it over its panels, the same
    /// width as the chart at the same place across (its gutters): for a
    /// scroll view to keep in sight (`ScrollView::sticky`), on the
    /// document's ground, a line under it. [`AXIS_HEIGHT`] tall.
    pub fn axis(&self) -> Element {
        let (start, end, now, local) = (self.start, self.end, self.now, self.local.clone());
        canvas(move |g, b| {
            let p = theme::palette();
            let rect = style::snap(g, b);
            style::fill_rect(g, rect, style::c(p.document_background));
            let plots = plots_of(rect);
            paint_axis(g, rect.top(), plots, start, end, now, &*local, true, false);
            style::hline_above(g, rect.left(), rect.right(), rect.bottom(), style::c(olo_theme::mix(p.document_text, p.document_background, 0.78)));
        })
        .paint_key(style::key((self.key, "axis")))
        .w_full()
        .h(px(AXIS))
        .flex_shrink_0()
        .into_element()
    }

    /// The present, marked with a line.
    pub fn now(mut self, t: f64) -> Self {
        self.now = Some(t);
        self
    }

    /// Spans shaded as night (sunset to sunrise).
    pub fn nights(mut self, nights: impl Into<Rc<[(f64, f64)]>>) -> Self {
        self.nights = nights.into();
        self
    }

    pub fn spans(mut self, spans: impl Into<Rc<[Span]>>) -> Self {
        self.spans = spans.into();
        self
    }

    /// The place's clock: a time in seconds from its own 1970-01-01
    /// midnight.
    pub fn local(mut self, f: impl Fn(f64) -> i64 + 'static) -> Self {
        self.local = Rc::new(f);
        self
    }

    /// The time under the pointer as it moves (None as it leaves).
    pub fn on_hover(mut self, f: impl Fn(&Option<f64>, &mut WindowCx) + 'static) -> Self {
        self.on_hover = Some(Rc::new(f));
        self
    }

    /// How tall it is.
    pub fn height(&self) -> f32 {
        self.panels.iter().map(Panel::extent).sum::<f32>() + AXIS
    }
}

/// A local time's hour, minute, weekday (0 Sunday) and day of the month.
pub fn parts(local: i64) -> (u32, u32, usize, u32) {
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    // (1970-01-01 was a Thursday.)
    let weekday = (days + 4).rem_euclid(7) as usize;
    // Howard Hinnant's civil_from_days, the day of the month.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    ((secs / 3600) as u32, ((secs % 3600) / 60) as u32, weekday, day)
}

pub const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// "Sat 15, 14:00".
fn when(local: i64) -> String {
    let (h, m, wd, d) = parts(local);
    format!("{} {d}, {h:02}:{m:02}", WEEKDAYS[wd])
}

impl IntoElement for Timeline {
    fn into_element(self) -> Element {
        let height = self.height();
        let Timeline { id, start, end, step, panels, now, nights, spans, local, key, on_hover, axis_on_top } = self;
        let span = end - start;
        canvas(move |g, b| {
            let p = theme::palette();
            let s = g.scale_factor();
            let rect = style::snap(g, b);
            let plots = plots_of(rect);
            let hover = {
                let state = g.node_state::<State>();
                (state.plots, state.drawn_at, state.start, state.end) = (plots, b.origin, start, end);
                state.hover
            };
            let width = f32::from(plots.size.width);
            let x_of = |t: f64| plots.left() + px(((t - start) / span) as f32 * width);
            // A time's x on whole device pixels (columns side by side
            // share their edges).
            let snap_x = |t: f64| px((f32::from(x_of(t)) * s).round() / s);
            let text = style::c(p.document_text);
            let dim = style::disabled_text(&p);
            let grid = style::c(olo_theme::mix(p.document_text, p.document_background, 0.9));
            let midnight_line = style::c(olo_theme::mix(p.document_text, p.document_background, 0.78));
            let night = style::c(olo_theme::mix(p.document_background, [0, 0, 0, 255], if p.is_dark() { 0.28 } else { 0.055 }));
            let small = style::ui_font(FontWeight::NORMAL);
            let bold = style::ui_font(FontWeight::BOLD);
            // The local midnights shown.
            let midnights: Vec<f64> = {
                let first = local(start);
                let mut m = first.div_euclid(86_400) * 86_400 + 86_400;
                let mut out = Vec::new();
                while (m - first) as f64 + start < end {
                    out.push(start + (m - first) as f64);
                    m += 86_400;
                }
                out
            };
            let mut y = rect.top();
            if axis_on_top {
                paint_axis(g, y, plots, start, end, now, &*local, true, true);
                y += px(AXIS + GAP);
            }
            // Each plot's area, for the shading, spans and the pointer's
            // line drawn over all of them.
            let mut areas: Vec<Bounds<Pixels>> = Vec::new();
            for panel in panels.iter() {
                if !panel.title.is_empty() {
                    let cy = y + px(TITLE_ROW / 2.0);
                    let title = style::label(g, plots.left(), cy, style::Align::Left, panel.title.clone(), style::SMALL_SIZE, bold.clone(), text);
                    crate::agent::record(g, agent::Node { role: "Heading", name: Some(panel.title.clone()), bounds: title, enabled: true, ..Default::default() });
                    // Its legend at the right: each track's swatch and name
                    // (none where it would run into the title: the readout
                    // under the pointer names them).
                    if let Kind::Plot { tracks, .. } = &panel.kind {
                        let shown: Vec<&Track> = tracks.iter().rev().filter(|t| !matches!(t.mark, Mark::Quiet)).collect();
                        let gap = px(olo_kit_base::metrics::TIGHT + 14.0);
                        let widths: Vec<Pixels> = shown.iter().map(|t| style::text_width(t.name.clone(), style::SMALL_SIZE, small.clone())).collect();
                        let needs = widths.iter().fold(px(0.0), |a, w| a + *w + gap) - px(olo_kit_base::metrics::TIGHT);
                        if plots.right() - needs >= title.right() + px(olo_kit_base::metrics::SPACING) {
                            let mut x = plots.right();
                            for (t, w) in shown.iter().zip(&widths) {
                                x -= *w;
                                style::label(g, x, cy, style::Align::Left, t.name.clone(), style::SMALL_SIZE, small.clone(), dim);
                                // (As solid as it's drawn: a range or column above as its
                                // mean opacity.)
                                let opacity = match &t.mark {
                                    Mark::Ranges { alpha: Some(a), .. } | Mark::Above { alpha: Some(a) } if !a.is_empty() => (a.iter().sum::<f32>() / a.len() as f32 + 0.35).min(1.0),
                                    _ => 1.0,
                                };
                                let swatch = style::snap_centred(g, point(x - px(9.0), cy), 10.0, if matches!(t.mark, Mark::Line { .. }) { 2.0 } else { 8.0 });
                                style::fill_rect(g, swatch, t.color.opacity(opacity));
                                x -= gap;
                            }
                        }
                    }
                    y += px(TITLE_ROW);
                }
                let area = style::snap(g, Bounds { origin: point(plots.left(), y), size: size(plots.size.width, px(panel.height)) });
                match &panel.kind {
                    Kind::Plot { tracks, left, right, guides } => {
                        areas.push(area);
                        // The ground, the nights, the grid.
                        style::fill_rect(g, area, style::c(p.document_background));
                        for &(a, z) in nights.iter() {
                            let (a, z) = (a.max(start), z.min(end));
                            if z > a {
                                style::fill_rect(g, Bounds::from_corners(point(snap_x(a), area.top()), point(snap_x(z), area.bottom())), night);
                            }
                        }
                        // A scale's ends by what shows of its tracks (not
                        // the hours beyond the chart's end).
                        let ends = |scale: &Scale, on_right: bool| {
                            let mut values = Vec::new();
                            for t in tracks.iter().filter(|t| t.right == on_right && !matches!(t.mark, Mark::Quiet)) {
                                let lows = match &t.mark {
                                    Mark::Ranges { lows, .. } => Some(lows.clone()),
                                    _ => None,
                                };
                                for (i, &(pt, v)) in t.points.iter().enumerate() {
                                    if pt + step > start && pt < end {
                                        values.push(v);
                                        if let Some(l) = lows.as_ref().and_then(|l| l.get(i)) {
                                            values.push(*l);
                                        }
                                    }
                                }
                            }
                            scale.ends(values.into_iter())
                        };
                        let (lo, hi, tick) = ends(left, false);
                        let right_ends = right.as_ref().map(|r| ends(r, true));
                        // (A little in from the top and bottom: a line at
                        // the scale's end isn't cut in half.)
                        let h = (f32::from(area.size.height) - 2.0 * PAD).max(1.0);
                        let y_in = move |v: f64, (lo, hi): (f64, f64)| area.bottom() - px(PAD + ((v - lo) / (hi - lo)).clamp(0.0, 1.0) as f32 * h);
                        // Ticks of the left scale, and their values.
                        let mut v = lo;
                        while v <= hi + tick * 1e-6 {
                            let ty = y_in(v, (lo, hi));
                            if v > lo + tick * 1e-6 && v < hi - tick * 1e-6 {
                                style::hline(g, area.left(), area.right(), ty, grid);
                            }
                            // (Each on its line, the ends' too: the gutter has
                            // room above and below a plot, and the values
                            // stay evenly spaced.)
                            style::label(g, area.left() - px(6.0), ty, style::Align::Right, (left.say)(v), style::SMALL_SIZE, small.clone(), dim);
                            v += tick;
                        }
                        if let (Some(scale), Some((rlo, rhi, rtick))) = (right, right_ends) {
                            let mut v = rlo;
                            while v <= rhi + rtick * 1e-6 {
                                let ty = y_in(v, (rlo, rhi));
                                style::label(g, area.right() + px(6.0), ty, style::Align::Left, (scale.say)(v), style::SMALL_SIZE, small.clone(), dim);
                                v += rtick;
                            }
                        }
                        for &m in &midnights {
                            style::vline(g, snap_x(m), area.top(), area.bottom(), midnight_line);
                        }
                        // A guide's line under what's plotted, its name over
                        // it (after the tracks, below).
                        let guide_ys: Vec<(Pixels, &SharedString)> =
                            guides.iter().map(|(value, name)| (y_in(*value, (lo, hi)), name)).filter(|(gy, _)| *gy > area.top() && *gy < area.bottom()).collect();
                        for &(gy, _) in &guide_ys {
                            style::hline(g, area.left(), area.right(), gy, midnight_line);
                        }
                        g.with_content_mask(Some(olo_interface::ContentMask { bounds: area }), |g| {
                            // Columns and ranges first, then the lines over
                            // them, the first named on top.
                            let order = tracks.iter().filter(|t| !matches!(t.mark, Mark::Line { .. })).chain(tracks.iter().rev().filter(|t| matches!(t.mark, Mark::Line { .. })));
                            for t in order {
                                let scale = if t.right { right_ends.map_or((lo, hi), |(a, b, _)| (a, b)) } else { (lo, hi) };
                                match &t.mark {
                                    Mark::Columns { colors } => {
                                        // (Standing on the bottom when the scale starts at nothing.)
                                        let floor = if scale.0 >= 0.0 { area.bottom() } else { y_in(0.0f64.min(scale.1), scale) };
                                        for (i, &(pt, v)) in t.points.iter().enumerate() {
                                            if !v.is_finite() || v <= 0.0 || pt + step < start || pt > end {
                                                continue;
                                            }
                                            let top = px((f32::from(y_in(v, scale)) * s).round() / s);
                                            // At least a device pixel tall: a trace shows.
                                            let top = top.min(floor - px(1.0 / s));
                                            let (l, r) = (snap_x(pt), snap_x(pt + step));
                                            // A device pixel between columns, when there's room.
                                            let r = if f32::from(r - l) * s >= 4.0 { r - px(1.0 / s) } else { r };
                                            let color = colors.as_ref().and_then(|c| c.get(i).copied()).unwrap_or(t.color);
                                            style::fill_rect(g, Bounds::from_corners(point(l, top), point(r, floor)), color);
                                        }
                                    }
                                    Mark::Ranges { lows, alpha } => {
                                        for (i, &(pt, v)) in t.points.iter().enumerate() {
                                            let lo_v = lows.get(i).copied().unwrap_or(f64::NAN);
                                            if !v.is_finite() || !lo_v.is_finite() || pt + step < start || pt > end {
                                                continue;
                                            }
                                            let a = alpha.as_ref().and_then(|a| a.get(i).copied()).unwrap_or(1.0);
                                            if a <= 0.0 {
                                                continue;
                                            }
                                            let top = px((f32::from(y_in(v, scale)) * s).round() / s);
                                            let bottom = px((f32::from(y_in(lo_v, scale)) * s).round() / s).max(top + px(2.0 / s));
                                            let (l, r) = (snap_x(pt), snap_x(pt + step));
                                            style::fill_rect(g, Bounds::from_corners(point(l, top), point(r, bottom)), t.color.opacity(a.clamp(0.0, 1.0)));
                                        }
                                    }
                                    Mark::Above { alpha } => {
                                        // (The edge a device pixel or two, by the scale.)
                                        let edge = px((1.5 * s).round().max(1.0) / s);
                                        for (i, &(pt, v)) in t.points.iter().enumerate() {
                                            if !v.is_finite() || pt + step < start || pt > end {
                                                continue;
                                            }
                                            let a = alpha.as_ref().and_then(|a| a.get(i).copied()).unwrap_or(1.0).clamp(0.0, 1.0);
                                            if a <= 0.0 {
                                                continue;
                                            }
                                            let bottom = px((f32::from(y_in(v, scale)) * s).round() / s).max(area.top() + edge);
                                            let (l, r) = (snap_x(pt), snap_x(pt + step));
                                            style::fill_rect(g, Bounds::from_corners(point(l, area.top()), point(r, bottom)), t.color.opacity(a));
                                            style::fill_rect(g, Bounds::from_corners(point(l, bottom - edge), point(r, bottom)), t.color.opacity((a + 0.35).min(1.0)));
                                        }
                                    }
                                    Mark::Quiet => {}
                                    Mark::Line { width } => {
                                        let mut pieces = Vec::new();
                                        let mut last: Option<Point<Pixels>> = None;
                                        for &(pt, v) in t.points.iter() {
                                            if !v.is_finite() {
                                                last = None;
                                                continue;
                                            }
                                            // (Drawn at the middle of its step, over the columns'.)
                                            let here = point(x_of(pt + step / 2.0), y_in(v, scale));
                                            if let Some(prev) = last {
                                                pieces.push((prev, here));
                                            }
                                            last = Some(here);
                                        }
                                        g.paint_segments(&pieces, px(*width), true, t.color);
                                    }
                                }
                            }
                        });
                        for (gy, name) in guide_ys {
                            // (On a ground of its own: a line may pass behind it.)
                            let w = style::text_width(name.clone(), style::SMALL_SIZE, small.clone());
                            let ground = style::snap(g, Bounds::from_corners(point(area.right() - px(6.0) - w, gy - px(14.0)), point(area.right() - px(2.0), gy - px(1.0))));
                            style::fill_rect(g, ground, style::c(p.document_background).opacity(0.85));
                            style::label(g, area.right() - px(4.0), gy - px(7.0), style::Align::Right, name.clone(), style::SMALL_SIZE, small.clone(), dim);
                        }
                        // What agents read: each track's first value, its
                        // lowest and highest and when.
                        let reads: Vec<String> = tracks
                            .iter()
                            .filter_map(|t| {
                                let say = |v: f64| t.say.as_ref().map(|f| f(v)).unwrap_or_else(|| if t.right { right.as_ref().map(|r| (r.say)(v)).unwrap_or_default() } else { (left.say)(v) });
                                let shown: Vec<(f64, f64)> = t.points.iter().copied().filter(|(pt, v)| v.is_finite() && *pt + step > start && *pt < end).collect();
                                let first = shown.first()?;
                                let lowest = shown.iter().copied().fold(*first, |a, b| if b.1 < a.1 { b } else { a });
                                let highest = shown.iter().copied().fold(*first, |a, b| if b.1 > a.1 { b } else { a });
                                Some(format!("{}: {} at first, lowest {} ({}), highest {} ({})", t.name, say(first.1), say(lowest.1), when(local(lowest.0)), say(highest.1), when(local(highest.0))))
                            })
                            .collect();
                        crate::agent::record(
                            g,
                            agent::Node { role: "Figure", name: Some(panel.title.clone()), value: Some(reads.join("; ").into()), bounds: area, enabled: true, ..Default::default() },
                        );
                    }
                    Kind::Icons(points) => {
                        // (Placed by their hours, not on the window's grid:
                        // they line up with what's over or under them only
                        // by chance.)
                        crate::review::own_grid(g, area);
                        // As many as fit: one every so many steps.
                        let side = (panel.height - 4.0).min(24.0).max(16.0).floor();
                        let per = ((side + 4.0) / (width / (span / step) as f32)).ceil().max(1.0) as usize;
                        for (i, (pt, name)) in points.iter().enumerate() {
                            if i % per != 0 || *pt < start || *pt + step > end + 1.0 {
                                continue;
                            }
                            let c = point(x_of(pt + step / 2.0), area.center().y);
                            let r = style::snap_centred(g, c, side, side);
                            icons::paint(g, r, name);
                        }
                    }
                    Kind::Arrows(points, color) => {
                        let per = (16.0 / (width / (span / step) as f32)).ceil().max(1.0) as usize;
                        for (i, &(pt, from)) in points.iter().enumerate() {
                            if i % per != 0 || !from.is_finite() || pt < start || pt + step > end + 1.0 {
                                continue;
                            }
                            arrow(g, point(x_of(pt + step / 2.0), area.center().y), from, *color);
                        }
                    }
                }
                y += px(panel.height + GAP);
            }
            if !axis_on_top {
                paint_axis(g, y, plots, start, end, now, &*local, false, true);
            }
            // The spans and the present over every plot.
            let accent = style::c(p.hues.yellow);
            // Where each row of names is taken to (a name that would run
            // into the one before it goes in the row under; with no room in
            // either, the span shows unnamed, its name still read out).
            let mut rows = [px(f32::MIN); SPAN_ROWS];
            for sp in spans.iter() {
                let (a, z) = (sp.start.max(start), sp.end.min(end));
                if z <= a {
                    continue;
                }
                for area in &areas {
                    style::fill_rect(g, Bounds::from_corners(point(snap_x(a), area.top()), point(snap_x(z), area.bottom())), accent.opacity(0.22));
                }
                let Some(first) = areas.first() else { continue };
                let said = format!("{}: {} to {}", sp.name, when(local(sp.start)), when(local(sp.end)));
                let x = snap_x(a);
                // Its name at its start, on a ground of its own, as far as
                // it needs (into the hours after, if it's short).
                let room = (snap_x(z) - x).max(px(160.0)).min(first.right() - x - px(4.0));
                let w = style::text_width(sp.name.clone(), style::SMALL_SIZE, bold.clone()).min(room);
                match rows.iter().position(|taken| x >= *taken + px(4.0)) {
                    Some(row) if room > px(24.0) => {
                        let top = first.top() + px(row as f32 * SPAN_ROW);
                        rows[row] = x + w + px(6.0);
                        let ground = style::snap(g, Bounds::from_corners(point(x, top), point(x + w + px(6.0), top + px(SPAN_ROW - 1.0))));
                        style::fill_rect(g, ground, accent.opacity(0.85));
                        // (Placed by its hours over the plot, not beside
                        // the scale's numbers: not held level with them.)
                        crate::review::own_grid(g, ground);
                        let r = style::label_fit(
                            g,
                            x + px(3.0),
                            top + px((SPAN_ROW - 1.0) / 2.0),
                            style::Align::Left,
                            sp.name.clone(),
                            style::SMALL_SIZE,
                            bold.clone(),
                            style::ink_on(p.hues.yellow),
                            room,
                        );
                        crate::agent::record(g, agent::Node::label(said, r));
                    }
                    _ => crate::agent::record(g, agent::Node::label(said, Bounds::from_corners(point(x, first.top()), point(snap_x(z), first.bottom())))),
                }
            }
            if let Some(n) = now.filter(|n| *n >= start && *n <= end) {
                let x = snap_x(n);
                for area in &areas {
                    style::vline(g, x, area.top(), area.bottom(), style::c(p.hues.red));
                }
            }
            // The pointer: a line through every plot at its step, and
            // every track's value then.
            if let Some(pos) = hover.filter(|pos| pos.x >= plots.left() && pos.x < plots.right()) {
                let t = start + (f32::from(pos.x - plots.left()) / width) as f64 * span;
                let at = start + ((t - start) / step).floor() * step;
                let x = x_of(at + step / 2.0);
                let line = style::c(olo_theme::mix(p.document_text, p.document_background, 0.35));
                for area in &areas {
                    style::vline(g, x, area.top(), area.bottom(), line);
                }
                let mut lines: Vec<(Hsla, String)> = Vec::new();
                for panel in panels.iter() {
                    if let Kind::Plot { tracks, left, right, .. } = &panel.kind {
                        for tr in tracks {
                            let Some(i) = tr.at(at + step / 2.0, step) else { continue };
                            let v = tr.points[i].1;
                            let say = tr.say.as_ref().map(|f| f(v)).unwrap_or_else(|| if tr.right { right.as_ref().map(|r| (r.say)(v)).unwrap_or_default() } else { (left.say)(v) });
                            let text = match &tr.mark {
                                Mark::Ranges { lows, .. } => match lows.get(i).filter(|l| l.is_finite()) {
                                    Some(&l) => {
                                        let low = tr.say.as_ref().map(|f| f(l)).unwrap_or_else(|| (left.say)(l));
                                        format!("{}: {low} to {say}", tr.name)
                                    }
                                    None => format!("{}: {say}", tr.name),
                                },
                                _ => format!("{}: {say}", tr.name),
                            };
                            lines.push((tr.color, text));
                        }
                    }
                }
                // Kept inside what shows of the chart (it may be taller than its
                // scroll view); only while hovered, so scrolling alone never
                // draws it again.
                let shown = g.node_state::<State>().shown;
                let inside = Bounds::from_corners(point(rect.left(), rect.top().max(shown.top())), point(rect.right(), rect.bottom().min(shown.bottom())));
                super::charts::readout(g, inside, pos, &lines, when(local(at)));
            }
        })
        .paint_key(key)
        .id(id)
        .on_mouse_move(move |e: &MouseMoveEvent, cx| {
            let (shown, window) = (cx.listener_shown(), cx.listener_bounds().origin);
            let state = cx.node_state::<State>();
            state.shown = shown;
            let pos = point(e.position.x - (window.x - state.drawn_at.x), e.position.y - (window.y - state.drawn_at.y));
            let inside = pos.x >= state.plots.left() && pos.x < state.plots.right();
            let at = inside.then_some(pos);
            let time = at.map(|p| state.time_at(p.x));
            if state.hover != at {
                state.hover = at;
                cx.repaint();
            }
            if let Some(f) = &on_hover {
                f(&time, cx);
            }
        })
        .on_hover(|over, cx| {
            if !over && cx.node_state::<State>().hover.take().is_some() {
                cx.repaint();
            }
        })
        .w_full()
        .h(px(height))
        .flex_shrink_0()
    }
}

/// Where the plots are in the chart's `rect`: its width less a gutter
/// each side (the scales' numbers).
fn plots_of(rect: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds { origin: point(rect.left() + px(GUTTER), rect.top()), size: size((rect.size.width - px(2.0 * GUTTER)).max(px(1.0)), rect.size.height) }
}

/// The axis at `top` (its [`AXIS`] rows) for the plots `plots`, from
/// `start` to `end`: the hours, ticked toward the plots, and the days
/// named over their stretches. Over the plots (`on_top`): the days, then
/// the hours by the plots under them, ticked down; under them, the
/// other way. `record`: its day names for agents (once: not the copy a
/// scroll view keeps in sight).
#[allow(clippy::too_many_arguments)]
fn paint_axis(g: &mut Painter, top: Pixels, plots: Bounds<Pixels>, start: f64, end: f64, now: Option<f64>, local: &dyn Fn(f64) -> i64, on_top: bool, record: bool) {
    let p = theme::palette();
    let s = g.scale_factor();
    let span = end - start;
    let width = f32::from(plots.size.width);
    let x_of = |t: f64| plots.left() + px(((t - start) / span) as f32 * width);
    let snap_x = |t: f64| px((f32::from(x_of(t)) * s).round() / s);
    let text = style::c(p.document_text);
    let dim = style::disabled_text(&p);
    let midnight_line = style::c(olo_theme::mix(p.document_text, p.document_background, 0.78));
    let small = style::ui_font(FontWeight::NORMAL);
    let bold = style::ui_font(FontWeight::BOLD);
    let hours = (span / 3600.0).max(1.0);
    let midnights: Vec<f64> = {
        let first = local(start);
        let mut m = first.div_euclid(86_400) * 86_400 + 86_400;
        let mut out = Vec::new();
        while (m - first) as f64 + start < end {
            out.push(start + (m - first) as f64);
            m += 86_400;
        }
        out
    };
    let every = [1u32, 2, 3, 6, 12].into_iter().find(|h| width / (hours as f32) * (*h as f32) >= 40.0).unwrap_or(24);
    let (hour_y, day_y) = if on_top { (top + px(25.0), top + px(8.0)) } else { (top + px(8.0), top + px(25.0)) };
    // The ticks, in the gap between the axis and the plots.
    let (tick_a, tick_z) = if on_top { (top + px(AXIS), top + px(AXIS + 3.0)) } else { (top - px(GAP), top - px(GAP - 3.0)) };
    let first_hour = (start / 3600.0).ceil() * 3600.0;
    let mut t = first_hour;
    while t <= end {
        let (h, _, _, _) = parts(local(t));
        if h % every == 0 {
            let x = snap_x(t);
            style::vline(g, x, tick_a, tick_z, midnight_line);
            style::label(g, x, hour_y, style::Align::Center, format!("{h:02}"), style::SMALL_SIZE, small.clone(), dim);
        }
        t += 3600.0;
    }
    // Each day named over its own stretch (the first from the start).
    let mut starts = vec![start];
    starts.extend(midnights.iter().copied());
    for (i, &d) in starts.iter().enumerate() {
        let next = starts.get(i + 1).copied().unwrap_or(end);
        let (_, _, wd, day) = parts(local(d + 1.0));
        let name = if d == start && now.is_some_and(|n| (n - start).abs() < 3.0 * 3600.0) { "Today".to_owned() } else { format!("{} {day}", WEEKDAYS[wd]) };
        let room = x_of(next) - x_of(d);
        if f32::from(room) >= 34.0 {
            let named = style::label_fit(g, x_of(d) + px(4.0), day_y, style::Align::Left, name, style::SMALL_SIZE, bold.clone(), text, room - px(6.0));
            if record {
                crate::agent::record(g, agent::Node::label(format!("{} {day}", WEEKDAYS[wd]), named));
            }
        }
    }
}

/// A wind arrow at `c`, pointing where wind from `from` degrees goes.
fn arrow(g: &mut Painter, c: Point<Pixels>, from: f64, color: Hsla) {
    let to = (from + 180.0).to_radians() as f32;
    let (dx, dy) = (to.sin(), -to.cos());
    let len = 6.0;
    let head = point(c.x + px(dx * len), c.y + px(dy * len));
    let tail = point(c.x - px(dx * len), c.y - px(dy * len));
    g.paint_segment(tail, head, px(1.5), true, color);
    for side in [-1.0f32, 1.0] {
        let a = to + std::f32::consts::PI + side * 0.5;
        let end = point(head.x + px(a.sin() * 4.5), head.y - px(a.cos() * 4.5));
        g.paint_segment(head, end, px(1.5), true, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_times_read_as_the_places_clock() {
        // 2026-11-14 is a Saturday.
        let t = 1_794_614_400; // 2026-11-14T00:00:00Z
        assert_eq!(parts(t), (0, 0, 6, 14));
        assert_eq!(parts(t + 14 * 3600 + 30 * 60), (14, 30, 6, 14));
        assert_eq!(parts(t - 1), (23, 59, 5, 13));
        assert_eq!(when(t + 3600), "Sat 14, 01:00");
    }

    #[test]
    fn scales_end_on_nice_numbers() {
        let s = Scale::new(|v| format!("{v}"));
        assert_eq!(s.ends([3.2, 17.9].into_iter()), (0.0, 20.0, 5.0));
        let z = Scale::new(|v| format!("{v}")).from_zero();
        assert_eq!(z.ends([2.0, 7.0].into_iter()), (0.0, 7.5, 2.5));
        let p = Scale::new(|v| format!("{v}")).range(0.0, 100.0);
        assert_eq!(p.ends([20.0].into_iter()).0, 0.0);
        assert_eq!(p.ends([20.0].into_iter()).1, 100.0);
    }

    #[test]
    fn a_column_is_found_by_the_step_it_spans() {
        let t = Track::columns("Rain", olo_interface::hsla(0.0, 0.0, 0.0, 1.0), vec![(0.0, 1.0), (3600.0, f64::NAN), (7200.0, 2.0)]);
        assert_eq!(t.at(1800.0, 3600.0), Some(0));
        assert_eq!(t.at(5000.0, 3600.0), None);
        assert_eq!(t.at(7300.0, 3600.0), Some(2));
        assert_eq!(t.at(20000.0, 3600.0), None);
    }
}
