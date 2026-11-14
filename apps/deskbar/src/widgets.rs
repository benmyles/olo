//! Deskbar's live widgets (`olo_protocol::deskbar`, Preferences ▸
//! Deskbar), as BeOS's Pulse and ProcessController put their bars in its
//! tray: each one measures something (`meters::Reader`), on a worker
//! that reads each as often as it asks and only while one is shown (no
//! widgets, no thread; the displays asleep or the screen locked,
//! nothing read), and keeps what it read for its graph. What a widget
//! draws is worked out once a reading (`Shown`), not each frame. Drawn
//! in the tray (an icon's place), in rows under it (each on its own, or
//! side by side, small), or at the Deskbar's foot.
//!
//! They move smoothly, as Activity Monitor's graphs do: a graph scrolls
//! a device pixel at a time, an interval behind its latest reading so
//! its line reaches the edge (still once readings stop), and bars,
//! meters and heatmaps ease to each reading. They're drawn on a layer
//! over the bar (`Layer`), so as they move only they are drawn again,
//! not the bar's applications, tray and words; a reading draws the
//! layer alone too.

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    rc::Rc,
    sync::Arc,
    sync::{OnceLock, mpsc},
    time::Instant,
};

use olo_kit::palette::Rgba;
use olo_kit::{
    Bounds, Cx, Element, FontWeight, Hsla, Painter, Pixels, Styled as _, View, agent, canvas, metrics as m,
    palette::Palette,
    point, px,
    style::{self, Align, c},
    theme,
    widgets::charts,
};
use olo_protocol::deskbar::{Look, MAX_SPAN_S, Measure, Size, Widget};
use olo_system::{
    meters::{self, Reader, Reading, Unit},
    procs::{Rank, Sampler},
};

/// Seconds since Deskbar started: when each reading was made, and the
/// time a drawing shows.
pub fn clock() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// How long bars, meters and heatmaps take to reach a new reading: the
/// time between readings (so one read every second is always on its
/// way, as its graph is), but no more than this.
const MOVE_S: f64 = 1.0;

/// How far along its way a level is, `u` of the way through its time:
/// still at either end, so one reading's move runs into the next's
/// without a jolt.
fn ease(u: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    u * u * (3.0 - 2.0 * u)
}

/// What a widget shows now, made once a reading: each series' levels
/// over its history (0 to 1, oldest first, with when each was read),
/// each one's level now, and its value in words.
pub struct Shown {
    pub widget: Widget,
    series: Vec<Vec<(f64, f32)>>,
    /// Each series' level at the latest reading, and as drawn when it
    /// came: drawn moving from one to the other.
    now: Vec<f32>,
    was: Vec<f32>,
    /// When the latest reading was made (`clock`); None, nothing read.
    at: Option<f64>,
    pub value: String,
    /// Shorter, each shorter than the one before, for a row whose
    /// value and label don't both fit: "↓4.1K ↑674"; a Token Pool
    /// widget's of several quotas "24% · 46%", then the least "24%".
    shorter: Vec<String>,
    short: String,
    latest: Option<Reading>,
}

impl Shown {
    fn of(w: &Widget, readings: &VecDeque<(f64, Reading)>, was: Vec<f32>) -> Shown {
        let (at, latest) = readings.back().map_or((None, None), |(t, r)| (Some(*t), Some(r.clone())));
        // A rate's top: the highest any series has been in the history.
        let top = readings.iter().flat_map(|(_, r)| r.values.iter().copied()).filter(|v| v.is_finite()).fold(0.0, f64::max);
        let mean = |r: &Reading| r.values.iter().sum::<f64>() / r.values.len().max(1) as f64;
        let cores = matches!(w.measure, Measure::Cpu { cores: true });
        // (A processor's cores in a graph: one line, their mean.)
        let n = if cores { 1 } else { latest.as_ref().map_or(0, |r| r.values.len()) };
        let series = (0..n).map(|i| readings.iter().map(|(t, r)| (*t, (if cores { Some(mean(r)) } else { r.values.get(i).copied() }).map_or(f32::NAN, |v| r.level(v, top)))).collect()).collect();
        let now = latest.as_ref().map(|r| r.values.iter().map(|v| r.level(*v, top)).collect()).unwrap_or_default();
        let value = latest.as_ref().map_or_else(|| "–".into(), |r| meters::value_text(&w.measure, r));
        // (A quota's: what's left of it all, or used, in figures.)
        let pool = match (&w.measure, latest.as_ref().and_then(|r| r.left)) {
            (Measure::Pool { used, .. }, Some(left)) => Some(if *used { 100.0 - left * 100.0 } else { left * 100.0 }),
            _ => None,
        };
        let short = match pool {
            Some(v) => format!("{v:.0}"),
            None => latest.as_ref().map_or_else(|| "–".into(), |r| short_value(r, if cores { Some(mean(r)) } else { r.values.first().copied() })),
        };
        let shorter = match (&w.measure, latest.as_ref()) {
            (_, Some(r)) if pool.is_some() => {
                let least = format!("{:.0}%", pool.unwrap_or(0.0));
                // (Several quotas' words, "24% · 46% left": their figures.)
                let each = r.words.as_deref().filter(|w| w.contains(" · ")).map(|w| w.trim_end_matches(" left").trim_end_matches(" used").to_owned());
                each.into_iter().chain([least]).collect()
            }
            (Measure::Network { .. }, Some(r)) if r.values.len() == 2 => vec![format!("↓{} ↑{}", short_value(r, Some(r.values[0])), short_value(r, Some(r.values[1])))],
            (Measure::Disk { .. }, Some(r)) if r.values.len() == 2 => vec![format!("R{} W{}", short_value(r, Some(r.values[0])), short_value(r, Some(r.values[1])))],
            _ => Vec::new(),
        };
        Shown { widget: w.clone(), series, now, was, at, value, shorter, short, latest }
    }

    /// Seconds between its readings.
    fn every(&self) -> f64 {
        self.widget.every().as_secs_f64()
    }

    fn move_time(&self) -> f64 {
        MOVE_S.min(self.every())
    }

    /// Its levels as drawn at `t`: on their way from what was drawn to
    /// the latest reading's.
    fn levels_at(&self, t: f64) -> Vec<f32> {
        let k = self.at.map_or(1.0, |at| ease((t - at) / self.move_time())) as f32;
        self.now
            .iter()
            .enumerate()
            .map(|(i, v)| match self.was.get(i) {
                Some(w) if w.is_finite() && v.is_finite() => w + (v - w) * k,
                _ => *v,
            })
            .collect()
    }

    /// How long until the next frame for its levels moving `extent`
    /// device pixels from nothing to all (a bar's height): when they've
    /// moved another pixel (no sooner than a 60th of a second), and one
    /// last as they arrive; None once they're there.
    fn next_frame(&self, t: f64, extent: f32) -> Option<f64> {
        let time = self.move_time();
        let u = (t - self.at?) / time;
        if !(0.0..1.0).contains(&u) {
            return None;
        }
        let most = self.now.iter().zip(&self.was).map(|(a, b)| (a - b).abs()).filter(|d| d.is_finite()).fold(0.0, f32::max) as f64 * extent as f64;
        if most < 0.5 {
            return None;
        }
        // Where it's a pixel on, found by halving.
        let goal = ease(u) + 1.0 / most;
        if goal >= 1.0 {
            return Some(((1.0 - u) * time).max(1.0 / 240.0));
        }
        let (mut lo, mut hi) = (u, 1.0);
        for _ in 0..20 {
            let mid = (lo + hi) / 2.0;
            if ease(mid) < goal { lo = mid } else { hi = mid }
        }
        Some(((hi - u) * time).max(1.0 / 60.0))
    }

    /// The time its graph shows at its right edge at `t`: an interval
    /// behind the latest reading, so its line reaches the edge and
    /// scrolls into it rather than jump a step a reading (Activity
    /// Monitor's `now`), and stands still once no reading comes.
    fn graph_now(&self, t: f64) -> f64 {
        let every = self.every();
        self.at.map_or(t, |at| at + (t - at).min(every) - every)
    }

    /// Its tip and what agents read: what it measures in full, then each
    /// series' value ("Network: every interface\nDown: 1.2 MB/s"), then
    /// the processes using most of it, once they're known (`top`, while
    /// the pointer is on it).
    pub fn tip(&self, sources: &meters::Sources, top: Option<&[(String, f64)]>) -> String {
        let mut out = self.reading_tip(sources);
        if let Some(top) = top.filter(|t| !t.is_empty()) {
            let cores = sources.cores.max(1) as f64;
            out.push_str("\n\nUsing the most:");
            for (name, v) in top {
                let v = match self.widget.measure {
                    // (Of the whole processor, as the widget's.)
                    Measure::Cpu { .. } => percent(v / cores),
                    Measure::Gpu { .. } => percent(*v),
                    Measure::Memory { .. } => bytes_text(*v),
                    _ => meters::rate_text(*v),
                };
                out.push_str(&format!("\n{name}: {v}"));
            }
        }
        out
    }

    fn reading_tip(&self, sources: &meters::Sources) -> String {
        let w = &self.widget;
        let mut out = meters::describe(&w.measure, sources);
        let Some(r) = &self.latest else { return out };
        // (Said in full by what read it: a quota's accounts.)
        if !r.detail.is_empty() {
            for line in &r.detail {
                out.push('\n');
                out.push_str(line);
            }
            return out;
        }
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
    /// What each has read, newest last, with when, and as what (another
    /// measure or rate starts again).
    readings: HashMap<u32, (VecDeque<(f64, Reading)>, (Measure, u32))>,
    shown: HashMap<u32, Arc<Shown>>,
    worker: Option<mpsc::Sender<Tell>>,
    paused: bool,
    /// The widget the pointer is on, and the processes using most of
    /// what it measures (None until known).
    hovered: Option<u32>,
    top: Option<Vec<(String, f64)>>,
    /// Token Pool's meters followed while a widget shows them: the
    /// worker reads those widgets again as the pool writes.
    pool_watch: Option<olo_kit::watch::Watch>,
    /// Each Token Pool widget's quota: out on every account as last read
    /// (for a notification as that changes).
    out: HashMap<u32, bool>,
}

impl Live {
    pub fn new() -> Live {
        Live { widgets: Vec::new(), readings: HashMap::new(), shown: HashMap::new(), worker: None, paused: false, hovered: None, top: None, pool_watch: None, out: HashMap::new() }
    }

    /// What widget `w` shows (nothing read yet: nothing).
    pub fn shown(&self, w: &Widget) -> Arc<Shown> {
        self.shown.get(&w.id).filter(|s| s.widget == *w).cloned().unwrap_or_else(|| Arc::new(Shown::of(w, &VecDeque::new(), Vec::new())))
    }

    /// The widgets as the settings now say: what each read kept if it
    /// measures the same at the same rate.
    pub fn set(&mut self, widgets: Vec<Widget>, proxy: &olo_kit::Proxy) {
        if widgets == self.widgets {
            return;
        }
        self.readings.retain(|id, (_, of)| widgets.iter().any(|w| w.id == *id && *of == (w.measure.clone(), w.every_ms)));
        self.shown.clear();
        for w in &widgets {
            if let Some((r, _)) = self.readings.get_mut(&w.id) {
                while r.len() > kept(w) {
                    r.pop_front();
                }
                self.shown.insert(w.id, Arc::new(Shown::of(w, r, Vec::new())));
            }
        }
        self.widgets = widgets;
        self.tell(proxy);
    }

    /// Nothing read while the displays are asleep or the screen locked.
    pub fn pause(&mut self, on: bool, proxy: &olo_kit::Proxy) {
        if self.paused != on {
            self.paused = on;
            self.tell(proxy);
        }
    }

    fn tell(&mut self, proxy: &olo_kit::Proxy) {
        let wanted = if self.paused { Vec::new() } else { self.widgets.clone() };
        if !wanted.iter().any(|w| matches!(w.measure, Measure::Pool { .. })) {
            self.pool_watch = None;
        }
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
        if self.pool_watch.is_none() && wanted.iter().any(|w| matches!(w.measure, Measure::Pool { .. })) {
            let tx = std::sync::Mutex::new(worker.clone());
            self.pool_watch = olo_kit::watch::watch([olo_protocol::pool::PoolMeters::path()], move |_| {
                let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(Tell::Pool);
            });
        }
        let _ = worker.send(Tell::Widgets(wanted));
    }

    /// The pointer is on widget `id` (None: on none): while it is, the
    /// worker looks at which processes use most of what it measures.
    pub fn hover(&mut self, id: Option<u32>) {
        if self.hovered == id {
            return;
        }
        self.hovered = id;
        self.top = None;
        let rank = id.and_then(|id| self.widgets.iter().find(|w| w.id == id)).and_then(rank);
        if let Some(worker) = &self.worker {
            let _ = worker.send(Tell::Hover(rank.map(|r| (id.unwrap_or(0), r))));
        }
    }

    /// The processes using most of what widget `id` measures, as looked.
    pub fn set_top(&mut self, id: u32, top: Option<Vec<(String, f64)>>) {
        if self.hovered == Some(id) && top.is_some() {
            self.top = top;
        }
    }

    /// Widget `w`'s tip now.
    pub fn tip(&self, w: &Widget, sources: &meters::Sources) -> String {
        let top = self.top.as_deref().filter(|_| self.hovered == Some(w.id));
        self.shown(w).tip(sources, top)
    }

    pub fn hovered(&self) -> Option<u32> {
        self.hovered
    }

    /// Readings come in, each with when it was made: each widget's
    /// drawing made again, moving on from where it's drawn now. What to
    /// tell the person: a Token Pool widget's quota (one asking to say
    /// so) out on every account, or back.
    pub fn add(&mut self, batch: Vec<(u32, f64, Reading)>) -> Vec<PoolNews> {
        let t = clock();
        let mut news = Vec::new();
        for (id, at, r) in batch {
            let Some(w) = self.widgets.iter().find(|w| w.id == id) else { continue };
            if let Measure::Pool { notify: true, .. } = &w.measure
                && let Some(left) = r.left
            {
                let out = left <= 0.0005;
                // (Not as it's first read: only a change.)
                if self.out.insert(id, out).is_some_and(|was| was != out) {
                    news.push(PoolNews { measure: w.measure.clone(), out, back: r.note.clone() });
                }
            }
            let was = self.shown.get(&id).filter(|s| s.widget == *w).map(|s| s.levels_at(t)).unwrap_or_default();
            let (readings, _) = self.readings.entry(id).or_insert_with(|| (VecDeque::new(), (w.measure.clone(), w.every_ms)));
            readings.push_back((at, r));
            while readings.len() > kept(w) {
                readings.pop_front();
            }
            self.shown.insert(id, Arc::new(Shown::of(w, readings, was)));
        }
        news
    }
}

/// A Token Pool quota out on every account, or back: told as a
/// notification (`Measure::Pool`'s `notify`).
pub struct PoolNews {
    pub measure: Measure,
    pub out: bool,
    /// When the first account comes back ("Work back in 2 h").
    pub back: Option<String>,
}

impl PoolNews {
    /// Its notification's words: summary and body.
    pub fn said(&self) -> (String, String) {
        let Measure::Pool { provider, quotas, .. } = &self.measure else { return (String::new(), String::new()) };
        let name = olo_protocol::pool::provider_name(provider);
        let what = match quotas.as_slice() {
            [q] => format!("{name}'s {q} quota"),
            [] => format!("{name}'s quota"),
            _ => format!("One of {name}'s quotas"),
        };
        if self.out {
            (
                format!("{what} is used up on every account"),
                match &self.back {
                    Some(b) => format!("{b}. Until then Token Pool asks {} to wait.", olo_protocol::pool::client_name(provider)),
                    None => format!("Token Pool asks {} to wait until one resets.", olo_protocol::pool::client_name(provider)),
                },
            )
        } else {
            (format!("{what} is back"), "An account can answer again.".into())
        }
    }
}

/// "0.4%", "12%".
fn percent(v: f64) -> String {
    if v < 9.95 { format!("{v:.1}%") } else { format!("{v:.0}%") }
}

/// "840 MB", "1.2 GB".
fn bytes_text(v: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let (mut v, mut i) = (v.max(0.0), 0);
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 || v >= 100.0 { format!("{v:.0} {}", UNITS[i]) } else { format!("{v:.1} {}", UNITS[i]) }
}

/// What a widget's tip ranks processes by: the processor's, memory's,
/// a GPU's, the network's and the disks' (not a volume's fullness, a
/// temperature or the battery's power, which no process has a share of).
pub fn rank(w: &Widget) -> Option<Rank> {
    match &w.measure {
        Measure::Cpu { .. } => Some(Rank::Cpu),
        Measure::Memory { .. } => Some(Rank::Memory),
        Measure::Gpu { pci } => Some(Rank::Gpu(pci.clone())),
        Measure::Network { .. } => Some(Rank::Network),
        Measure::Disk { .. } => Some(Rank::Disk),
        _ => None,
    }
}

/// What the worker is told: the widgets to read, or the one the pointer
/// is on, whose tip lists the processes using most.
pub enum Tell {
    Widgets(Vec<Widget>),
    Hover(Option<(u32, Rank)>),
    /// Token Pool wrote its meters: its widgets are read again now.
    Pool,
}

/// How many processes a tip lists.
const TOP: usize = 5;
/// A tip's processes: looked at again this often while it shows, the
/// first two this far apart (a rate needs a look before it), so they're
/// there as the tip shows (`tip`'s pause is 500 ms).
const TOP_EVERY: std::time::Duration = std::time::Duration::from_secs(1);
const TOP_FIRST: std::time::Duration = std::time::Duration::from_millis(350);

/// How many readings a widget keeps: its graph's span, and the one
/// before it and the next (its right edge an interval behind).
fn kept(w: &Widget) -> usize {
    w.history() + 2
}

/// The worker: each widget read when it's due, the readings sent
/// together; waits for the next due (or a new list) and does nothing
/// else.
fn read_widgets(rx: mpsc::Receiver<Tell>, proxy: olo_kit::Proxy) {
    let mut widgets: Vec<Widget> = Vec::new();
    let mut readers: HashMap<u32, (Measure, Reader)> = HashMap::new();
    // When each is next read, and what it was set to read then.
    let mut due: HashMap<u32, ((Measure, u32), Instant)> = HashMap::new();
    // The widget whose tip lists processes, what by, its sampler (none
    // while no tip does: its memory of every process goes) and when it
    // next looks.
    let mut hover: Option<(u32, Rank, Sampler, Instant)> = None;
    loop {
        let next = due.values().map(|(_, at)| *at).chain(hover.as_ref().map(|h| h.3)).min();
        let got = match next {
            None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
            Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
        };
        match got {
            Ok(Tell::Hover(on)) => {
                hover = on.map(|(id, rank)| (id, rank, Sampler::new(), Instant::now()));
                continue;
            }
            Ok(Tell::Pool) => {
                let now = Instant::now();
                for w in widgets.iter().filter(|w| matches!(w.measure, Measure::Pool { .. })) {
                    if let Some((_, at)) = due.get_mut(&w.id) {
                        *at = now;
                    }
                }
                continue;
            }
            Ok(Tell::Widgets(list)) => {
                widgets = list;
                readers.retain(|id, (m, _)| widgets.iter().any(|w| w.id == *id && w.measure == *m));
                // (One measuring something else, or as often, starts
                // again: read at once.)
                due.retain(|id, (of, _)| widgets.iter().any(|w| w.id == *id && *of == (w.measure.clone(), w.every_ms)));
                let now = Instant::now();
                for w in &widgets {
                    due.entry(w.id).or_insert(((w.measure.clone(), w.every_ms), now));
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        let mut batch = Vec::new();
        for w in &widgets {
            let Some((_, at)) = due.get_mut(&w.id) else { continue };
            if *at > now {
                continue;
            }
            let (_, reader) = readers.entry(w.id).or_insert_with(|| (w.measure.clone(), Reader::new()));
            if let Some(r) = reader.read(&w.measure) {
                batch.push((w.id, clock(), r));
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
        if let Some((id, rank, sampler, at)) = hover.as_mut()
            && *at <= Instant::now()
        {
            let top = sampler.top(rank, TOP);
            // (The first look of a rate: the next soon, to have one.)
            *at = Instant::now() + if top.is_none() { TOP_FIRST } else { TOP_EVERY };
            proxy.send(crate::Msg::Top(*id, top));
        }
    }
}

/// A place's widgets in rows: each on its own, or beside the one before
/// it (`Widget::beside`).
pub fn rows(widgets: &[Widget]) -> Vec<Vec<Widget>> {
    let mut rows: Vec<Vec<Widget>> = Vec::new();
    for w in widgets {
        match rows.last_mut() {
            Some(row) if w.beside => row.push(w.clone()),
            _ => rows.push(vec![w.clone()]),
        }
    }
    rows
}

/// A row's height: a widget on its own by its size (one with only words
/// is shorter); side by side, the tallest's (each its label over its
/// drawing).
pub fn row_height(row: &[Widget]) -> f32 {
    if let [w] = row {
        return match (w.size, w.look == Look::Number) {
            (Size::Small, _) => 24.0,
            (Size::Medium, true) => 28.0,
            (Size::Large, true) => 36.0,
            (Size::Medium, false) => 40.0,
            (Size::Large, false) => 56.0,
        };
    }
    row.iter()
        .map(|w| match w.size {
            Size::Small => 32.0,
            Size::Medium => 40.0,
            Size::Large => 52.0,
        })
        .fold(0.0, f32::max)
}

/// The cells of a row of `n` widgets side by side across `r`, inset
/// `inset` at each end.
pub fn cells(r: Bounds<Pixels>, n: usize, inset: f32) -> Vec<Bounds<Pixels>> {
    let n = n.max(1);
    let left = f32::from(r.left()) + inset;
    let w = (f32::from(r.size.width) - 2.0 * inset - m::TIGHT * (n - 1) as f32) / n as f32;
    (0..n).map(|i| Bounds::from_corners(point(px(left + i as f32 * (w + m::TIGHT)), r.top()), point(px(left + i as f32 * (w + m::TIGHT) + w), r.bottom()))).collect()
}

/// A widget's colours: its hue (the accent with none; a quota running
/// low, orange under a quarter left and red under a tenth, if it
/// warns), and a second series' (up after down, writes after reads,
/// swap after memory).
fn colors(s: &Shown, p: &Palette) -> (Hsla, Hsla) {
    use olo_kit::palette::Hue;
    let w = &s.widget;
    let second = match w.hue {
        Some(Hue::Orange) => p.hue(Hue::Purple),
        _ => p.hue(Hue::Orange),
    };
    (c(base(s, p)), c(second))
}

/// What a quota left under is drawn in, as it runs low.
const LOW: f64 = 0.25;
const VERY_LOW: f64 = 0.10;

/// A widget's own colour as it is now (see `colors`).
fn base(s: &Shown, p: &Palette) -> Rgba {
    use olo_kit::palette::Hue;
    let w = &s.widget;
    if let Measure::Pool { warn: true, .. } = w.measure
        && let Some(left) = s.latest.as_ref().and_then(|r| r.left)
    {
        if left < VERY_LOW {
            return p.hue(Hue::Red);
        }
        if left < LOW {
            return p.hue(Hue::Orange);
        }
    }
    w.hue.map_or(p.control_highlight, |h| p.hue(h))
}

/// Its label in a row ("CPU", "Network", "Claude 5-hour").
pub fn label(w: &Widget) -> String {
    match &w.measure {
        Measure::Temperature { .. } => "Temp".into(),
        Measure::Pool { provider, quotas, .. } if quotas.is_empty() => olo_protocol::pool::provider_name(provider),
        Measure::Pool { provider, quotas, .. } => format!("{} {}", olo_protocol::pool::provider_name(provider), quotas.join(", ")),
        m => m.kind_name().into(),
    }
}

/// Its label where there's less room, beside others: "Mem", "Net",
/// "Claude 5h".
fn short_label(w: &Widget) -> String {
    match &w.measure {
        Measure::Cpu { .. } => "CPU".into(),
        Measure::Memory { .. } => "Mem".into(),
        Measure::Gpu { .. } => "GPU".into(),
        Measure::Disk { .. } => "I/O".into(),
        Measure::Space { .. } => "Disk".into(),
        Measure::Network { .. } => "Net".into(),
        Measure::Temperature { .. } => "Temp".into(),
        Measure::Power => "Pwr".into(),
        Measure::Pool { provider, quotas, .. } => {
            let name = olo_protocol::pool::provider_name(provider);
            if quotas.is_empty() { name } else { format!("{name} {}", quotas.iter().map(|q| short_quota(q)).collect::<Vec<_>>().join("/")) }
        }
    }
}

/// A quota said short: "5h", "Wk", "Wk Fable".
fn short_quota(q: &str) -> String {
    let q = q.replace("-hour", "h").replace(" hour", "h").replace("-day", "d");
    match q.strip_prefix("Weekly") {
        Some(rest) => format!("Wk{rest}"),
        None => q,
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

/// Paint `s`'s drawing (not its words) in `r` as it is at `t`: a graph,
/// bars, a heatmap or a meter in a sunken well; and, while it moves, ask
/// for the frame where it has moved a device pixel (the window's one
/// clock, `charts::tick_within`), none once it's still.
fn paint_drawing(g: &mut Painter, r: Bounds<Pixels>, s: &Shown, p: &Palette, t: f64) {
    let inner = charts::paint_well(g, r, p);
    let w = &s.widget;
    let (first, second) = colors(s, p);
    let scale = g.scale_factor();
    let (wide, tall) = (f32::from(inner.size.width) * scale, f32::from(inner.size.height) * scale);
    // How far its levels move from nothing to all: device pixels (a
    // heatmap's cells, steps of colour).
    let extent = match w.look {
        Look::Graph => {
            let (now, span) = (s.graph_now(t), w.span_s.clamp(1, MAX_SPAN_S) as f64);
            // (The second under the first: up under down.)
            for (i, points) in s.series.iter().enumerate().rev() {
                charts::paint_trace(g, inner, points, now, span, if i == 0 { first } else { second }, i == 0);
            }
            if s.at.is_some_and(|at| t - at < s.every()) {
                g.paint_again_within(span / wide.max(1.0) as f64);
            }
            return;
        }
        Look::Bars => {
            charts::paint_bars(g, inner, &s.levels_at(t), first);
            tall
        }
        // (Idle, the well's own ground; busy, the widget's hue. A Token
        // Pool widget's quotas each by how used it is: green to red; Ben,
        // 2026-11-13: "show each selected quota type as a different color
        // with the color representing usage where red is 100% used and
        // green is 0% used".)
        Look::Heatmap => {
            match w.measure {
                Measure::Pool { used, .. } => {
                    let ground = p.document_background;
                    charts::paint_heatmap_by(g, inner, &s.levels_at(t), |v| if v.is_finite() { charts::usage_colour(p, if used { v } else { 1.0 - v }) } else { ground });
                }
                _ => charts::paint_heatmap(g, inner, &s.levels_at(t), p.document_background, base(s, p)),
            }
            16.0
        }
        Look::Meter => {
            charts::paint_meter(g, inner, &s.levels_at(t), first);
            wide
        }
        Look::Number => return,
    };
    if let Some(next) = s.next_frame(t, extent) {
        g.paint_again_within(next);
    }
}

/// Paint `s` in a tray slot `r` (an icon's square): its drawing, or its
/// value in small figures.
fn paint_tray(g: &mut Painter, r: Bounds<Pixels>, s: &Shown, p: &Palette, t: f64) {
    if s.widget.look == Look::Number {
        let (first, _) = colors(s, p);
        style::label(g, r.center().x, r.center().y, Align::Center, s.short.clone(), 9.0, style::ui_font(FontWeight::BOLD), first);
        return;
    }
    paint_drawing(g, r, s, p, t);
}

/// Paint `s`'s row `r`: its label at the left, its value at the right,
/// and its drawing: between them in a small row, under them in a medium
/// or large one, the whole row's with neither.
fn paint_row(g: &mut Painter, r: Bounds<Pixels>, s: &Shown, p: &Palette, inset: f32, t: f64) {
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
    // drawing between them too), else shorter; then beside its short
    // label ("Claude 5h/Wk"), longest first; else the shortest without.
    let room = f32::from(right - left) - if w.size == Size::Small && !number { 24.0 } else { 0.0 };
    let values: Vec<&String> = std::iter::once(&s.value).chain(&s.shorter).collect();
    let shown_w = |value: &str| if w.value || number { style::text_width_app(value, size, &plain) + m::SPACING } else { 0.0 };
    let fits = |value: &str, name: &str| shown_w(value) + style::text_width_app(name, style::PLAIN_SIZE, &bold) <= room;
    let names = if w.label { vec![label(w), short_label(w)] } else { Vec::new() };
    let chosen = names.iter().find_map(|name| values.iter().find(|v| fits(v, name)).map(|v| ((*v).clone(), Some(name.clone()))));
    let (value, name) = chosen.unwrap_or_else(|| {
        let shortest = if w.label { values.last() } else { values.first() };
        (shortest.map_or_else(String::new, |v| (*v).clone()), None)
    });
    let show_label = name.is_some();
    let mut label_w = px(0.0);
    if let Some(name) = name {
        label_w = style::label(g, left, text_y, Align::Left, name, style::PLAIN_SIZE, bold, c(p.panel_text)).size.width;
    }
    let mut value_w = px(0.0);
    if w.value || number {
        value_w = style::label(g, right, text_y, Align::Right, value, size, plain, c(p.panel_text)).size.width;
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
    paint_drawing(g, drawing, s, p, t);
}

/// The line a cell's words are on, from its top.
const CELL_WORDS: f32 = 14.0;

/// Paint `s` in cell `r` of a row it shares, small: its label over its
/// drawing, its value at the label's right if both fit (else in its
/// tip); a number's value under its label.
fn paint_cell(g: &mut Painter, r: Bounds<Pixels>, s: &Shown, p: &Palette, t: f64) {
    let w = &s.widget;
    let number = w.look == Look::Number;
    let (bold, plain) = (style::ui_font(FontWeight::BOLD), style::ui_font(FontWeight::NORMAL));
    let size = style::SMALL_SIZE;
    let room = f32::from(r.size.width);
    let text_y = r.top() + px(4.0 + CELL_WORDS / 2.0);
    let width = |text: &str, font| style::text_width_app(text, size, font);
    let name = [label(w), short_label(w)].into_iter().find(|l| width(l, &bold) <= room).unwrap_or_else(|| short_label(w));
    let name_w = if w.label { width(&name, &bold) + m::TIGHT } else { 0.0 };
    // (In the words' line unless it's a number, whose value is under;
    // never the tray's figures without their unit, "3" for 3%.)
    let value = (w.value && !number).then(|| std::iter::once(&s.value).chain(&s.shorter).find(|v| width(v, &plain) + name_w <= room).cloned()).flatten();
    g.with_content_mask(Some(olo_kit::ContentMask { bounds: r }), |g| {
        if w.label {
            style::label(g, r.left(), text_y, Align::Left, name, size, bold.clone(), c(p.panel_text));
        }
        if let Some(value) = value {
            let (x, align) = if w.label { (r.right(), Align::Right) } else { (r.left(), Align::Left) };
            style::label(g, x, text_y, align, value, size, plain.clone(), c(p.panel_text));
        }
    });
    let body = Bounds::from_corners(point(r.left(), r.top() + px(6.0 + CELL_WORDS)), point(r.right(), r.bottom() - px(4.0)));
    if number {
        let (first, _) = colors(s, p);
        let big = match w.size {
            Size::Small => style::PLAIN_SIZE,
            Size::Medium => 14.0,
            Size::Large => 16.0,
        };
        let text = if width(&s.value, &bold) * big / size <= room { s.value.clone() } else { s.short.clone() };
        g.with_content_mask(Some(olo_kit::ContentMask { bounds: r }), |g| {
            style::label(g, body.left(), body.center().y, Align::Left, text, big, bold, first);
        });
        return;
    }
    paint_drawing(g, body, s, p, t);
}

/// Where on the bar a widget is drawn.
#[derive(Clone, PartialEq)]
pub enum Spot {
    /// An icon's place in the tray.
    Tray(Bounds<Pixels>),
    /// A row of its own, its words inset as the tray's.
    Row(Bounds<Pixels>),
    /// A cell of a row it shares.
    Cell(Bounds<Pixels>),
}

#[derive(Clone, PartialEq)]
pub struct Placed {
    pub widget: Widget,
    pub spot: Spot,
}

/// Where the bar put its widgets as it was last laid out: it fills it,
/// the layer draws from it.
#[derive(Clone, Default)]
pub struct Shelf(Rc<RefCell<Vec<Placed>>>);

impl Shelf {
    pub fn set(&self, placed: Vec<Placed>) {
        let mut shelf = self.0.borrow_mut();
        if *shelf != placed {
            *shelf = placed;
        }
    }
}

/// A bar's widgets, drawn over it as a view of their own: drawn again
/// when the bar says a reading came (or lays them out anew: the bar's
/// `body` runs first), and as they move (each asks for its next frame),
/// without the bar under them being drawn.
pub struct Layer {
    shelf: Shelf,
    model: crate::Model,
}

impl Layer {
    pub fn new(shelf: Shelf, model: crate::Model) -> Layer {
        Layer { shelf, model }
    }
}

impl View for Layer {
    type Message = ();

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
        use crate::ModelExt as _;
        let (shelf, model) = (self.shelf.clone(), self.model.clone());
        canvas(move |g, _| {
            // (What the bar put there as it was last laid out.)
            let placed = shelf.0.borrow().clone();
            let (t, p) = (clock(), theme::palette());
            for Placed { widget, spot } in placed {
                let s = model.read().live.shown(&widget);
                let r = match spot {
                    Spot::Tray(r) => {
                        paint_tray(g, r, &s, &p, t);
                        r
                    }
                    Spot::Row(r) => {
                        paint_row(g, r, &s, &p, crate::TRAY_TEXT, t);
                        r
                    }
                    Spot::Cell(r) => {
                        paint_cell(g, r, &s, &p, t);
                        r
                    }
                };
                let name = label(&s.widget);
                agent::record(g, agent::Node { role: "Meter", name: Some(name.into()), value: Some(s.value.clone().into()), bounds: r, enabled: true, clickable: true, ..Default::default() });
            }
        })
        .size_full()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moving(was: f32, now: f32, every_ms: u32) -> Shown {
        let mut widget = Widget::new(1, Measure::Cpu { cores: true });
        widget.every_ms = every_ms;
        Shown { widget, series: Vec::new(), now: vec![now], was: vec![was], at: Some(10.0), value: String::new(), shorter: Vec::new(), short: String::new(), latest: None }
    }

    /// Read every second, a level is on its way all that second: from
    /// where it was, still at each end, there as the next comes.
    #[test]
    fn levels_move_the_whole_interval() {
        let s = moving(0.2, 0.6, 1000);
        assert_eq!(s.levels_at(10.0), vec![0.2]);
        assert!((s.levels_at(10.5)[0] - 0.4).abs() < 1e-6);
        assert!(s.levels_at(10.1)[0] < 0.25, "slow away");
        assert_eq!(s.levels_at(11.0), vec![0.6]);
        assert_eq!(s.levels_at(30.0), vec![0.6]);
        // Read every ten seconds, it gets there in one.
        assert_eq!(moving(0.0, 1.0, 10_000).levels_at(11.0), vec![1.0]);
    }

    /// A frame each device pixel it moves, none once it's there, and
    /// none for a move under half a pixel.
    #[test]
    fn frames_follow_the_pixels() {
        let s = moving(0.0, 1.0, 1000);
        let mid = s.next_frame(10.5, 100.0).unwrap();
        // (A pixel of 100 at its fastest, 1.5 the mean pace: 1/150 s, held
        // to 1/60.)
        assert!((mid - 1.0 / 60.0).abs() < 1e-9, "{mid}");
        let slow = s.next_frame(10.5, 10.0).unwrap();
        assert!((0.05..0.08).contains(&slow), "{slow}");
        // (The last, as it arrives: no sooner than a 240th.)
        assert!(s.next_frame(10.999, 10.0).unwrap() <= 1.0 / 240.0 + 1e-9);
        assert_eq!(s.next_frame(11.0, 100.0), None);
        assert_eq!(moving(0.5, 0.502, 1000).next_frame(10.2, 100.0), None);
    }
}
