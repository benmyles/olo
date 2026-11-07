//! Deskbar's live widgets (`olo_protocol::deskbar`, Preferences ▸
//! Deskbar), as BeOS's Pulse and ProcessController put their bars in its
//! tray: each one measures something (`meters::Reader`), on a worker
//! that reads each as often as it asks and only while one is shown (no
//! widgets, no thread; the displays asleep or the screen locked,
//! nothing read), and keeps what it read for its graph. What a widget
//! draws is worked out once a reading (`Shown`), not each frame: the bar
//! is drawn again as the pointer moves too. Drawn in the tray (an icon's
//! place), in rows under it, or at the Deskbar's foot.

use std::{
    collections::{HashMap, VecDeque},
    rc::Rc,
    sync::mpsc,
    time::Instant,
};

use olo_protocol::deskbar::{Look, Measure, Size, Widget};
use olo_system::meters::{self, Reader, Reading, Unit};
use olo_ui::{
    gpui::{App, Bounds, FontWeight, Hsla, Pixels, Window, point, px},
    metrics as m,
    palette::Palette,
    style::{self, Align, c},
    widgets::charts,
};

/// What a widget shows now, made once a reading: each series' levels
/// over its history (0 to 1, oldest first), each one's level now, and
/// its value in words.
pub struct Shown {
    pub widget: Widget,
    series: Vec<Vec<f32>>,
    now: Vec<f32>,
    pub value: String,
    /// Shorter, for a row whose value and label don't both fit: "↓4.1K
    /// ↑674".
    compact: String,
    short: String,
    latest: Option<Reading>,
}

impl Shown {
    fn of(w: &Widget, readings: &VecDeque<Reading>) -> Shown {
        let latest = readings.back().cloned();
        // A rate's top: the highest any series has been in the history.
        let top = readings.iter().flat_map(|r| r.values.iter().copied()).filter(|v| v.is_finite()).fold(0.0, f64::max);
        let mean = |r: &Reading| r.values.iter().sum::<f64>() / r.values.len().max(1) as f64;
        let cores = matches!(w.measure, Measure::Cpu { cores: true });
        // (A processor's cores in a graph: one line, their mean.)
        let n = if cores { 1 } else { latest.as_ref().map_or(0, |r| r.values.len()) };
        let series = (0..n).map(|i| readings.iter().map(|r| (if cores { Some(mean(r)) } else { r.values.get(i).copied() }).map_or(f32::NAN, |v| r.level(v, top))).collect()).collect();
        let now = latest.as_ref().map(|r| r.values.iter().map(|v| r.level(*v, top)).collect()).unwrap_or_default();
        let value = latest.as_ref().map_or_else(|| "–".into(), |r| meters::value_text(&w.measure, r));
        let short = latest.as_ref().map_or_else(|| "–".into(), |r| short_value(r, if cores { Some(mean(r)) } else { r.values.first().copied() }));
        let compact = match (&w.measure, latest.as_ref()) {
            (Measure::Network { .. }, Some(r)) if r.values.len() == 2 => format!("↓{} ↑{}", short_value(r, Some(r.values[0])), short_value(r, Some(r.values[1]))),
            (Measure::Disk { .. }, Some(r)) if r.values.len() == 2 => format!("R{} W{}", short_value(r, Some(r.values[0])), short_value(r, Some(r.values[1]))),
            _ => value.clone(),
        };
        Shown { widget: w.clone(), series, now, value, compact, short, latest }
    }

    /// Its tip and what agents read: what it measures in full, then each
    /// series' value ("Network: every interface\nDown: 1.2 MB/s").
    pub fn tip(&self, sources: &meters::Sources) -> String {
        let w = &self.widget;
        let mut out = meters::describe(&w.measure, sources);
        let Some(r) = &self.latest else { return out };
        if r.values.len() > 1 && !matches!(w.measure, Measure::Cpu { cores: true }) {
            for (name, v) in r.names.iter().zip(&r.values) {
                out.push_str(&format!("\n{name}: {}", meters::unit_text(r.unit, *v)));
            }
        } else {
            out.push_str(&format!(": {}", self.value));
        }
        if let Some(note) = r.note.as_ref().filter(|_| !r.values.is_empty()) {
            out.push_str(&format!(" ({note})"));
        }
        out
    }
}

/// The widgets shown, what each has read, and the worker reading them.
pub struct Live {
    pub widgets: Vec<Widget>,
    /// What each has read, newest last, and as what (another measure or
    /// rate starts again).
    readings: HashMap<u32, (VecDeque<Reading>, (Measure, u32))>,
    shown: HashMap<u32, Rc<Shown>>,
    worker: Option<mpsc::Sender<Vec<Widget>>>,
    paused: bool,
}

impl Live {
    pub fn new() -> Live {
        Live { widgets: Vec::new(), readings: HashMap::new(), shown: HashMap::new(), worker: None, paused: false }
    }

    /// What widget `w` shows (nothing read yet: nothing).
    pub fn shown(&self, w: &Widget) -> Rc<Shown> {
        self.shown.get(&w.id).filter(|s| s.widget == *w).cloned().unwrap_or_else(|| Rc::new(Shown::of(w, &VecDeque::new())))
    }

    /// The widgets as the settings now say: what each read kept if it
    /// measures the same at the same rate.
    pub fn set(&mut self, widgets: Vec<Widget>, proxy: &olo_ui::Proxy) {
        if widgets == self.widgets {
            return;
        }
        self.readings.retain(|id, (_, of)| widgets.iter().any(|w| w.id == *id && *of == (w.measure.clone(), w.every_ms)));
        self.shown.clear();
        for w in &widgets {
            if let Some((r, _)) = self.readings.get_mut(&w.id) {
                while r.len() > w.history() {
                    r.pop_front();
                }
                self.shown.insert(w.id, Rc::new(Shown::of(w, r)));
            }
        }
        self.widgets = widgets;
        self.tell(proxy);
    }

    /// Nothing read while the displays are asleep or the screen locked.
    pub fn pause(&mut self, on: bool, proxy: &olo_ui::Proxy) {
        if self.paused != on {
            self.paused = on;
            self.tell(proxy);
        }
    }

    fn tell(&mut self, proxy: &olo_ui::Proxy) {
        let wanted = if self.paused { Vec::new() } else { self.widgets.clone() };
        if wanted.is_empty() {
            // (Its sender gone, the worker ends.)
            self.worker = None;
            return;
        }
        let worker = self.worker.get_or_insert_with(|| {
            let (tx, rx) = mpsc::channel();
            let proxy = proxy.clone();
            if let Err(e) = std::thread::Builder::new().name("deskbar-widgets".into()).spawn(move || read_widgets(rx, proxy)) {
                tracing::warn!("Deskbar's widgets can't be read: {e}");
            }
            tx
        });
        let _ = worker.send(wanted);
    }

    /// Readings come in: each widget's drawing made again.
    pub fn add(&mut self, batch: Vec<(u32, Reading)>) {
        for (id, r) in batch {
            let Some(w) = self.widgets.iter().find(|w| w.id == id) else { continue };
            let (readings, _) = self.readings.entry(id).or_insert_with(|| (VecDeque::new(), (w.measure.clone(), w.every_ms)));
            readings.push_back(r);
            while readings.len() > w.history() {
                readings.pop_front();
            }
            self.shown.insert(id, Rc::new(Shown::of(w, readings)));
        }
    }
}

/// The worker: each widget read when it's due, the readings sent
/// together; waits for the next due (or a new list) and does nothing
/// else.
fn read_widgets(rx: mpsc::Receiver<Vec<Widget>>, proxy: olo_ui::Proxy) {
    let mut widgets: Vec<Widget> = Vec::new();
    let mut readers: HashMap<u32, (Measure, Reader)> = HashMap::new();
    let mut due: HashMap<u32, Instant> = HashMap::new();
    loop {
        let next = due.values().min().copied();
        let got = match next {
            None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
            Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
        };
        match got {
            Ok(list) => {
                widgets = list;
                readers.retain(|id, (m, _)| widgets.iter().any(|w| w.id == *id && w.measure == *m));
                due.retain(|id, _| widgets.iter().any(|w| w.id == *id));
                let now = Instant::now();
                for w in &widgets {
                    due.entry(w.id).or_insert(now);
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        let mut batch = Vec::new();
        for w in &widgets {
            let Some(at) = due.get_mut(&w.id) else { continue };
            if *at > now {
                continue;
            }
            let (_, reader) = readers.entry(w.id).or_insert_with(|| (w.measure.clone(), Reader::new()));
            if let Some(r) = reader.read(&w.measure) {
                batch.push((w.id, r));
            }
            // A whole period on (late, from now: they don't bunch up).
            *at += w.every();
            if *at <= now {
                *at = now + w.every();
            }
        }
        if !batch.is_empty() {
            proxy.send(crate::Msg::Meters(batch));
        }
    }
}

/// A widget's row height, by its size (one with only words is shorter).
pub fn row_height(w: &Widget) -> f32 {
    match (w.size, w.look == Look::Number) {
        (Size::Small, _) => 24.0,
        (Size::Medium, true) => 28.0,
        (Size::Large, true) => 36.0,
        (Size::Medium, false) => 40.0,
        (Size::Large, false) => 56.0,
    }
}

/// A widget's colours: its hue (the accent with none), and a second
/// series' (up after down, writes after reads, swap after memory).
fn colors(w: &Widget, p: &Palette) -> (Hsla, Hsla) {
    use olo_ui::palette::Hue;
    let first = w.hue.map_or(p.control_highlight, |h| p.hue(h));
    let second = match w.hue {
        Some(Hue::Orange) => p.hue(Hue::Purple),
        _ => p.hue(Hue::Orange),
    };
    (c(first), c(second))
}

/// Its label in a row ("CPU", "Network").
pub fn label(w: &Widget) -> &'static str {
    match w.measure {
        Measure::Temperature { .. } => "Temp",
        _ => w.measure.kind_name(),
    }
}

/// A value for the tray, an icon's width: "23", "2M", "63°".
fn short_value(r: &Reading, v: Option<f64>) -> String {
    let Some(v) = v else { return r.note.clone().map_or_else(|| "–".into(), |n| n.chars().take(3).collect()) };
    match r.unit {
        Unit::Percent | Unit::Watts => format!("{v:.0}"),
        Unit::Celsius => format!("{v:.0}°"),
        Unit::BytesPerSecond => {
            let (v, s) = if v >= 1e9 {
                (v / 1e9, "G")
            } else if v >= 1e6 {
                (v / 1e6, "M")
            } else if v >= 1e3 {
                (v / 1e3, "K")
            } else {
                (v, "")
            };
            format!("{v:.0}{s}")
        }
    }
}

/// Paint `s`'s drawing (not its words) in `r`: a graph, bars or a meter
/// in a sunken well.
fn paint_drawing(window: &mut Window, r: Bounds<Pixels>, s: &Shown, p: &Palette) {
    let inner = charts::paint_well(window, r, p);
    let w = &s.widget;
    let (first, second) = colors(w, p);
    match w.look {
        Look::Graph => {
            // (The second under the first: up under down.)
            for (i, levels) in s.series.iter().enumerate().rev() {
                charts::paint_sparkline(window, inner, levels, w.history(), if i == 0 { first } else { second }, i == 0);
            }
        }
        Look::Bars => charts::paint_bars(window, inner, &s.now, first),
        Look::Meter => charts::paint_meter(window, inner, &s.now, first),
        Look::Number => {}
    }
}

/// Paint `s` in a tray slot `r` (an icon's square): its drawing, or its
/// value in small figures.
pub fn paint_tray(window: &mut Window, cx: &mut App, r: Bounds<Pixels>, s: &Shown, p: &Palette) {
    if s.widget.look == Look::Number {
        let (first, _) = colors(&s.widget, p);
        style::label(window, cx, r.center().x, r.center().y, Align::Center, s.short.clone(), 9.0, style::ui_font(FontWeight::BOLD), first);
        return;
    }
    paint_drawing(window, r, s, p);
}

/// Paint `s`'s row `r`: its label at the left, its value at the right,
/// and its drawing: between them in a small row, under them in a medium
/// or large one, the whole row's with neither.
pub fn paint_row(window: &mut Window, cx: &mut App, r: Bounds<Pixels>, s: &Shown, p: &Palette, inset: f32) {
    let w = &s.widget;
    let left = r.left() + px(inset);
    let right = r.right() - px(inset);
    let words = w.label || w.value;
    let number = w.look == Look::Number;
    let size = match (number, w.size) {
        (true, Size::Large) => 16.0,
        (true, Size::Medium) => 14.0,
        _ => style::PLAIN_SIZE,
    };
    // (Words over the drawing: the top line of a medium or large row.)
    let text_y = if number || w.size == Size::Small || !words { r.center().y } else { r.top() + px(4.0 + m::LABEL_ROW / 2.0) };
    let (bold, plain) = (style::ui_font(FontWeight::BOLD), style::ui_font(FontWeight::NORMAL));
    // Its value in full if it fits beside its label (a small row's
    // drawing between them too), else shorter, else without the label.
    let room = f32::from(right - left) - if w.size == Size::Small && !number { 24.0 } else { 0.0 };
    let label_w = if w.label { style::text_width_app(cx, label(w), style::PLAIN_SIZE, &bold) + m::SPACING } else { 0.0 };
    let value = if !w.label || style::text_width_app(cx, &s.value, size, &plain) + label_w <= room { s.value.clone() } else { s.compact.clone() };
    let show_label = w.label && style::text_width_app(cx, &value, size, &plain) + label_w <= room;
    let mut label_w = px(0.0);
    if show_label {
        label_w = style::label(window, cx, left, text_y, Align::Left, label(w), style::PLAIN_SIZE, bold, c(p.panel_text)).size.width;
    }
    let mut value_w = px(0.0);
    if w.value || number {
        value_w = style::label(window, cx, right, text_y, Align::Right, value, size, plain, c(p.panel_text)).size.width;
    }
    if number {
        return;
    }
    let drawing = if !words {
        Bounds::from_corners(point(left, r.top() + px(4.0)), point(right, r.bottom() - px(4.0)))
    } else if w.size == Size::Small {
        let a = left + label_w + if show_label { px(m::TIGHT) } else { px(0.0) };
        let b = right - value_w - if w.value { px(m::TIGHT) } else { px(0.0) };
        Bounds::from_corners(point(a, r.top() + px(5.0)), point(b.max(a + px(8.0)), r.bottom() - px(5.0)))
    } else {
        Bounds::from_corners(point(left, r.top() + px(8.0 + m::LABEL_ROW)), point(right, r.bottom() - px(4.0)))
    };
    paint_drawing(window, drawing, s, p);
}
