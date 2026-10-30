//! A screen's history in the window (the History tab): its actions in a
//! list that grows as the agent works (the history's folder is followed,
//! and only what was added is read, on a worker), narrowed by a search and
//! filters (kind, result, time, as Log Viewer's); the screen as it was at
//! the action chosen or wherever the scrubber is, with what the action was
//! and what came of it under it; and playback in time with the actions
//! (play and pause, speed, stepping action by action), each click shown as
//! a ring where it went and each key or text under the picture. Stretches
//! where nothing happened are skipped while playing. At the newest action
//! the history follows the screen (Latest), each new action chosen as it
//! comes. Frames are built on a worker (`record::frame_at`), the latest
//! asked for winning; the window never waits on the disk.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{RecvTimeoutError, Sender, channel},
    },
    time::{Duration, Instant},
};

use olo_kit::{
    Proxy,
    egui::{self, Align, Color32, ColorImage, Rect, TextureHandle, TextureOptions, Ui, pos2, vec2},
    metrics as m,
    search::{self, Facet, FilterBar, Query, SearchField},
    style::{self, c},
    widgets::{Button, Cell, Column, ColumnListView, EmptyState, ListState, MenuField, Slider, StatusBar},
};

use crate::{
    history::{self, Action},
    record,
};

pub enum ReplayMsg {
    /// What was added to the history (or all of it, `reset`), and the
    /// frames kept so far.
    Loaded { name: String, actions: Vec<Action>, texts: Vec<String>, reset: bool, segments: Arc<Vec<record::Segment>> },
    Frame { name: String, at: u64, image: ColorImage },
    /// Exporting: how far it has got (0–1), then how it ended.
    Progress { name: String, done: f32 },
    Exported { name: String, result: Result<PathBuf, String> },
}

/// How long a click's ring and a key's caption stay over the picture.
const RING_MS: u64 = 700;
const CAPTION_MS: u64 = 1500;
/// While playing, a stretch longer than this with nothing in it is
/// skipped (to just before what comes next).
const SKIP_MS: u64 = 3000;
const SPEEDS: [&str; 4] = ["1×", "2×", "4×", "8×"];
const SPEED: [f32; 4] = [1.0, 2.0, 4.0, 8.0];
/// After a change on disk, how long to gather more before reading; and
/// how often the frames alone are read again (a running screen adds them
/// every second or faster).
const SETTLE: Duration = Duration::from_millis(200);
const FRAMES_EVERY: Duration = Duration::from_secs(1);

/// The kinds of action, as the Kind filter groups them: its value, its
/// name, and the commands in it.
pub const KINDS: [(&str, &str, &[&str]); 6] = [
    ("pointer", "Pointer", &["click", "move", "down", "up", "drag", "scroll", "wheel", "scroll-to"]),
    ("keys", "Keys and Typing", &["key", "type", "set"]),
    ("choices", "Menus and Choices", &["invoke", "select"]),
    ("windows", "Windows", &["focus", "close", "minimize", "zoom", "resize", "place"]),
    ("programs", "Programs", &["run"]),
    ("screen", "Start and Stop", &["start", "stop"]),
];
/// The Time filter: its value, its name, and how far back (ms; 0: since
/// today began).
const TIMES: [(&str, &str, u64); 3] = [("5m", "Last 5 Minutes", 5 * 60_000), ("1h", "Last Hour", 3_600_000), ("today", "Today", 0)];
const KEYS: [&str; 3] = ["kind", "result", "since"];
/// Narrower than this, the transport puts its scrubber over its buttons.
const TRANSPORT_ONE_ROW: f32 = 470.0;
/// How much of a frame picking the actions to show may take.
const PICK_BUDGET: Duration = Duration::from_millis(2);

pub struct Replay {
    pub name: String,
    actions: Vec<Action>,
    /// Each action's text, to search (its command, what it matched, its
    /// error), made on the worker that reads them.
    texts: Vec<String>,
    /// A pick under way: how far through the actions, and what it kept.
    picking: Option<(usize, Vec<usize>)>,
    /// Actions of each kind (as `KINDS` groups them), and failed.
    kind_counts: [usize; KINDS.len()],
    failed: usize,
    segments: Arc<Vec<record::Segment>>,
    loaded: bool,
    query: String,
    parsed: Query,
    shown: Vec<usize>,
    /// The filters' menus, for the actions there were when made.
    facets: Option<(usize, Vec<Facet>)>,
    /// When the shown actions were picked (a time filter moves on).
    picked: Instant,
    list: ListState,
    playhead: u64,
    playing: bool,
    speed: usize,
    /// At the newest action: new ones are chosen as they come.
    follow: bool,
    /// The action last brought into view in the list (only a new one is,
    /// so the list scrolls freely otherwise).
    revealed: Option<usize>,
    texture: Option<TextureHandle>,
    /// The time of the frame showing, and the one last asked for.
    showing: Option<u64>,
    asked: Option<u64>,
    frames: Sender<(Arc<Vec<record::Segment>>, u64)>,
    /// A frame come from the worker, until drawn.
    pending: Option<ColorImage>,
    /// The screen's logical size, from its start (for placing clicks).
    size: Option<(f32, f32)>,
    /// An export under way (how far, and its Cancel), or how the last
    /// one ended.
    exporting: Option<(f32, Arc<AtomicBool>)>,
    exported: Option<Result<PathBuf, String>>,
    focus_search: bool,
    /// The first and last times in it, and its stats (for how many
    /// actions, and when).
    bounds: (u64, u64),
    stats: Option<(usize, Instant, history::Stats)>,
    /// When the screen was last started.
    started: Option<u64>,
    /// Following the history's folder, for as long as this is shown.
    _watch: Option<olo_kit::watch::Watch>,
}

/// Read screen `name`'s history and frames, and again whenever its folder
/// changes: only what was added, on a worker, for as long as the returned
/// watch is kept.
fn follow(name: &str, proxy: Proxy) -> Option<olo_kit::watch::Watch> {
    let (tx, rx) = channel::<()>();
    let n = name.to_owned();
    let spawned = std::thread::Builder::new().name("screen-history".into()).spawn(move || {
        let mut reader = history::Reader::default();
        let mut known = HashMap::new();
        let mut first = true;
        let mut frames_seen = (0, 0, 0);
        let mut frames_read: Option<Instant> = None;
        loop {
            let (actions, reset) = reader.more(&n);
            // The frames: with new actions, or now and then as they come.
            let due = frames_read.is_none_or(|t| t.elapsed() >= FRAMES_EVERY);
            let read_frames = first || reset || !actions.is_empty() || due;
            let segments = if read_frames {
                frames_read = Some(Instant::now());
                let s = record::index_known(&n, &mut known);
                let seen = (s.len(), s.iter().map(|s| s.frames.len()).sum::<usize>(), s.last().and_then(|s| s.frames.last()).map_or(0, |f| f.at));
                let changed = seen != frames_seen;
                frames_seen = seen;
                changed.then_some(s)
            } else {
                None
            };
            if first || reset || !actions.is_empty() || segments.is_some() {
                let segments = Arc::new(segments.unwrap_or_else(|| record::index_known(&n, &mut known)));
                let texts = actions.iter().map(text).collect();
                proxy.send(ReplayMsg::Loaded { name: n.clone(), actions, texts, reset: first || reset, segments });
            }
            first = false;
            // Until the folder changes (or the watch is dropped: done), or
            // frames left unread are due.
            let changed = if read_frames {
                rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
            } else {
                rx.recv_timeout(FRAMES_EVERY.saturating_sub(frames_read.map_or(Duration::ZERO, |t| t.elapsed())))
            };
            match changed {
                Ok(()) => {
                    std::thread::sleep(SETTLE);
                    while rx.try_recv().is_ok() {}
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    if spawned.is_err() {
        return None;
    }
    olo_kit::watch::watch([history::dir(name)], move |_| {
        let _ = tx.send(());
    })
}

/// `ms` as minutes and seconds ("3:07"), or hours too ("1:03:07").
fn duration(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// What a search looks through in `a`: its command, what it matched,
/// and its error.
fn text(a: &Action) -> String {
    format!("{}\n{}\n{}", command(a), a.matched().unwrap_or_default(), a.error.as_deref().unwrap_or(""))
}

/// An action's command line: its kind and arguments, quoted where they
/// need it.
pub fn command(a: &Action) -> String {
    let args: Vec<String> = a.args.iter().map(|x| if x.contains(' ') || x.is_empty() { format!("“{x}”") } else { x.clone() }).collect();
    if args.is_empty() { a.kind.clone() } else { format!("{} {}", a.kind, args.join(" ")) }
}

/// What came of an action, for people: what it matched, the program it
/// started, the window focused after; None when it says nothing.
pub fn outcome(a: &Action) -> Option<String> {
    if let Some(e) = &a.error {
        return Some(e.clone());
    }
    let r = a.result.as_ref()?;
    let mut parts = Vec::new();
    if let Some(m) = a.matched() {
        parts.push(m);
    } else if let Some((x, y)) = r.get("clicked").and_then(|c| Some((c.get(0)?.as_f64()?, c.get(1)?.as_f64()?))) {
        parts.push(format!("at {x:.0},{y:.0}"));
    }
    if let Some(pid) = r.get("pid").and_then(|p| p.as_u64()) {
        parts.push(format!("started, pid {pid}"));
    }
    if let (Some(w), Some(h)) = (r.get("width").and_then(|v| v.as_f64()), r.get("height").and_then(|v| v.as_f64())) {
        parts.push(format!("{w:.0} × {h:.0}"));
    }
    if let Some(title) = r.pointer("/after/window/title").and_then(|t| t.as_str()) {
        parts.push(format!("then “{title}” in front"));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

impl Replay {
    pub fn new(name: &str, proxy: Proxy) -> Replay {
        let (tx, rx) = channel::<(Arc<Vec<record::Segment>>, u64)>();
        {
            let (name, proxy) = (name.to_owned(), proxy.clone());
            let _ = std::thread::Builder::new().name("screen-frames".into()).spawn(move || {
                while let Ok(mut ask) = rx.recv() {
                    // Only the latest matters.
                    while let Ok(newer) = rx.try_recv() {
                        ask = newer;
                    }
                    if let Some((at, f)) = record::frame_at(&ask.0, ask.1) {
                        let image = ColorImage::from_rgba_unmultiplied([f.width, f.height], &f.rgba);
                        proxy.send(ReplayMsg::Frame { name: name.clone(), at, image });
                    }
                }
            });
        }
        Replay {
            name: name.to_owned(),
            actions: Vec::new(),
            texts: Vec::new(),
            picking: None,
            kind_counts: [0; KINDS.len()],
            failed: 0,
            segments: Arc::default(),
            loaded: false,
            query: String::new(),
            parsed: Query::default(),
            shown: Vec::new(),
            facets: None,
            picked: Instant::now(),
            list: ListState::default(),
            playhead: 0,
            playing: false,
            speed: 0,
            follow: true,
            revealed: None,
            texture: None,
            showing: None,
            asked: None,
            frames: tx,
            pending: None,
            size: None,
            exporting: None,
            exported: None,
            focus_search: false,
            bounds: (0, 0),
            stats: None,
            started: None,
            _watch: follow(name, proxy),
        }
    }

    pub fn loaded(&self) -> bool {
        self.loaded
    }

    pub fn actions(&self) -> &[Action] {
        &self.actions
    }

    /// When the screen was last started, by its history.
    pub fn started(&self) -> Option<u64> {
        self.started
    }

    pub fn message(&mut self, msg: ReplayMsg) {
        match msg {
            ReplayMsg::Loaded { name, actions, texts, reset, segments } if name == self.name => {
                let from = if reset { 0 } else { self.actions.len() };
                if reset {
                    self.actions = actions;
                    self.texts = texts;
                    (self.kind_counts, self.failed) = ([0; KINDS.len()], 0);
                } else {
                    self.actions.extend(actions);
                    self.texts.extend(texts);
                }
                self.count(from..self.actions.len());
                self.segments = segments;
                self.bounds = self.find_bounds();
                if reset {
                    self.started = None;
                }
                if let Some(a) = self.actions[from..].iter().rev().find(|a| a.kind == "start" && a.error.is_none()) {
                    self.started = Some(a.at);
                }
                if self.size.is_none() || reset {
                    self.size = self.actions.iter().rev().find(|a| a.kind == "start" && a.error.is_none()).and_then(|a| a.result.as_ref()).and_then(|r| Some((r["width"].as_f64()? as f32, r["height"].as_f64()? as f32)));
                }
                self.loaded = true;
                if from == 0 {
                    self.pick();
                } else if self.picking.is_none() {
                    // (A pick under way comes to them itself.)
                    let now = history::now_ms();
                    let new: Vec<usize> = (from..self.actions.len()).filter(|&i| self.matches(i, now)).collect();
                    self.shown.extend(new);
                }
                let (start, end) = self.range();
                if self.follow {
                    self.playhead = end;
                } else {
                    self.playhead = self.playhead.clamp(start, end);
                }
            }
            ReplayMsg::Frame { name, at, image } if name == self.name => {
                self.pending = Some(image);
                self.showing = Some(at);
            }
            ReplayMsg::Progress { name, done } if name == self.name => {
                if let Some((d, _)) = &mut self.exporting {
                    *d = done;
                }
            }
            ReplayMsg::Exported { name, result } if name == self.name => {
                self.exporting = None;
                self.exported = Some(result);
            }
            _ => {}
        }
    }

    /// Make the history a video at `path`, on a worker.
    pub fn start_export(&mut self, path: PathBuf, proxy: Proxy) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.exporting = Some((0.0, cancel.clone()));
        self.exported = None;
        let name = self.name.clone();
        std::thread::spawn(move || {
            let mut told = -1;
            let result = crate::export::export(&name, &path, &mut |done| {
                let pct = (done * 100.0) as i32;
                if pct != told {
                    told = pct;
                    proxy.send(ReplayMsg::Progress { name: name.clone(), done });
                }
                !cancel.load(Ordering::Relaxed)
            });
            if result.is_err() {
                let _ = std::fs::remove_file(path.with_extension("mp4.part"));
            }
            proxy.send(ReplayMsg::Exported { name, result: result.map(|()| path) });
        });
    }

    pub fn exporting(&self) -> bool {
        self.exporting.is_some()
    }

    /// Whether action `i` is one the search and filters keep (`now`:
    /// for the time filter).
    fn matches(&self, i: usize, now: u64) -> bool {
        let (a, text) = (&self.actions[i], self.texts[i].as_str());
        self.parsed.matches(|key, n| match key {
            None => n.in_text(text),
            Some("kind") => n.is(&a.kind) || KINDS.iter().any(|(v, _, kinds)| n.is(v) && kinds.contains(&a.kind.as_str())),
            Some("result") => (n.is("failed") && a.error.is_some()) || (n.is("ok") && a.error.is_none()),
            Some("since") => TIMES.iter().any(|(v, _, back)| {
                n.is(v) && {
                    let from = if *back == 0 { history::midnight(now) } else { now.saturating_sub(*back) };
                    a.at >= from
                }
            }),
            _ => false,
        })
    }

    /// Pick the actions to show again (the query changed, or time moved
    /// on under a time filter): begun here, gone on with each frame for
    /// as long as it takes.
    fn pick(&mut self) {
        self.parsed = Query::parse(&self.query, &KEYS);
        self.picked = Instant::now();
        self.picking = Some((0, Vec::new()));
        self.go_on_picking();
    }

    /// Pick on through the actions for a little of this frame (a long
    /// history takes a few); the list shows the last pick until done.
    fn go_on_picking(&mut self) -> bool {
        let Some((mut i, mut out)) = self.picking.take() else { return false };
        let (began, now) = (Instant::now(), history::now_ms());
        while i < self.actions.len() {
            if self.matches(i, now) {
                out.push(i);
            }
            i += 1;
            if i % 256 == 0 && began.elapsed() > PICK_BUDGET {
                self.picking = Some((i, out));
                return true;
            }
        }
        self.shown = out;
        false
    }

    /// Count `actions` into the filters' menus.
    fn count(&mut self, actions: std::ops::Range<usize>) {
        for a in &self.actions[actions] {
            if let Some(k) = KINDS.iter().position(|(_, _, kinds)| kinds.contains(&a.kind.as_str())) {
                self.kind_counts[k] += 1;
            }
            self.failed += usize::from(a.error.is_some());
        }
    }

    /// The filters' menus, each value with how many actions have it.
    fn facets(&mut self) -> &[Facet] {
        if self.facets.as_ref().is_none_or(|(n, _)| *n != self.actions.len()) {
            let mut kind = Facet::new("kind", "Kind").any("All Actions").multiple();
            for ((v, label, _), n) in KINDS.iter().zip(self.kind_counts) {
                kind = kind.value(*v, *label, Some(n));
            }
            let result = Facet::new("result", "Result").value("failed", "Failed", Some(self.failed)).value("ok", "Succeeded", Some(self.actions.len() - self.failed));
            let mut since = Facet::new("since", "Time").any("All Time");
            for (v, label, _) in TIMES {
                since = since.value(v, label, None);
            }
            self.facets = Some((self.actions.len(), vec![kind, result, since]));
        }
        &self.facets.as_ref().expect("made above").1
    }

    /// From the first thing kept to the last (worked out as they come).
    fn range(&self) -> (u64, u64) {
        self.bounds
    }

    fn find_bounds(&self) -> (u64, u64) {
        let frames = self.segments.iter().flat_map(|s| s.frames.iter().map(|f| f.at));
        let actions = self.actions.iter().flat_map(|a| [a.at, a.at + a.ms]);
        let (lo, hi) = frames.chain(actions).fold((u64::MAX, 0), |(lo, hi), t| (lo.min(t), hi.max(t)));
        if lo > hi { (0, 0) } else { (lo, hi) }
    }

    /// The time of the frame showing the screen at `t` (segments and their
    /// frames are in time order).
    fn frame_for(&self, t: u64) -> Option<u64> {
        let seg = self.segments.iter().rev().find(|s| s.frames.first().is_some_and(|f| f.at <= t))?;
        let i = seg.frames.partition_point(|f| f.at <= t);
        seg.frames.get(i.checked_sub(1)?).map(|f| f.at)
    }

    /// The next thing after `t`: a frame or an action.
    fn next_after(&self, t: u64) -> Option<u64> {
        let frame = self.segments.iter().find_map(|s| s.frames.get(s.frames.partition_point(|f| f.at <= t)).map(|f| f.at));
        let action = self.actions.get(self.actions.partition_point(|a| a.at <= t)).map(|a| a.at);
        frame.into_iter().chain(action).min()
    }

    /// The last action begun by `t` (actions are in the order they
    /// began, near enough: each is kept as it ends).
    fn action_at(&self, t: u64) -> Option<usize> {
        self.actions.partition_point(|a| a.at <= t).checked_sub(1)
    }

    /// The history's stats, worked out again only as it grows or (while
    /// the screen runs) once a second.
    pub fn stats(&mut self, running: bool) -> &history::Stats {
        let stale = self.stats.as_ref().is_none_or(|(n, at, _)| *n != self.actions.len() || (running && at.elapsed() >= Duration::from_secs(1)));
        if stale {
            let st = history::stats(&self.name, &self.actions, running, history::now_ms());
            self.stats = Some((self.actions.len(), Instant::now(), st));
        }
        &self.stats.as_ref().expect("made above").2
    }

    /// Go `by` actions on (or back) among those shown.
    pub fn step(&mut self, by: isize) {
        let now = self.action_at(self.playhead);
        let pos = now.map(|i| self.shown.partition_point(|&s| s <= i) as isize - 1);
        let to = match pos {
            None | Some(-1) if by > 0 => Some(0),
            Some(p) if p >= 0 => Some((p + by).clamp(0, self.shown.len() as isize - 1) as usize),
            _ => None,
        };
        if let Some(a) = to.and_then(|i| self.shown.get(i)).and_then(|&i| self.actions.get(i)) {
            self.playhead = a.at + a.ms;
            self.playing = false;
            self.follow = self.playhead >= self.range().1;
        }
    }

    pub fn play_pause(&mut self) {
        let (start, end) = self.range();
        self.playing = !self.playing;
        if self.playing {
            self.follow = false;
            if self.playhead >= end {
                self.playhead = start;
            }
        }
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    /// To the newest action, and on with the screen from there.
    pub fn latest(&mut self) {
        self.follow = true;
        self.playing = false;
        self.playhead = self.range().1;
    }

    pub fn focus_search(&mut self) {
        self.focus_search = true;
    }

    pub fn ui(&mut self, ui: &mut Ui, rect: Rect, running: bool) {
        let p = style::palette(ui.ctx());
        if let Some(image) = self.pending.take() {
            match &mut self.texture {
                Some(t) => t.set(image, TextureOptions::LINEAR),
                None => self.texture = Some(ui.ctx().load_texture(format!("replay-{}", self.name), image, TextureOptions::LINEAR)),
            }
        }
        let empty = |ui: &mut Ui, text: &str| {
            ui.put(rect, EmptyState::new(text).icon("agent-screens"));
        };
        if !self.loaded {
            return empty(ui, "Reading the history…");
        }
        if self.go_on_picking() {
            ui.ctx().request_repaint();
        }
        let (start, end) = self.range();
        if end == 0 {
            return empty(ui, "Nothing has been done on this screen yet. Its actions show here as an agent takes them.");
        }
        // A time filter moves on with the clock.
        if self.parsed.terms.iter().any(|t| t.key.as_deref() == Some("since")) && self.picked.elapsed() >= Duration::from_secs(1) {
            self.pick();
        }
        if self.parsed.terms.iter().any(|t| t.key.as_deref() == Some("since")) {
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }

        // The search and its filters, and how many actions they keep.
        let facets = self.facets().to_vec();
        let count = if self.shown.len() == self.actions.len() {
            format!("{} action{}", style::thousands(self.actions.len()), if self.actions.len() == 1 { "" } else { "s" })
        } else {
            format!("{} of {}", style::thousands(self.shown.len()), style::thousands(self.actions.len()))
        };
        let count_w = style::label_width(ui.ctx(), &count, style::plain_font()).ceil();
        let top = Rect::from_min_size(rect.min, vec2(rect.width(), m::CONTROL_HEIGHT));
        let bar_w = FilterBar::width(ui, &self.query, &facets) + m::SPACING + 40.0;
        let one_row = top.width() - bar_w - count_w - 2.0 * m::SPACING >= 220.0;
        let field_w = if one_row { top.width() - bar_w - count_w - 2.0 * m::SPACING } else { top.width() - count_w - m::SPACING };
        let field = Rect::from_min_size(top.min, vec2(field_w, m::CONTROL_HEIGHT));
        let id = ("replay-search", self.name.as_str());
        if std::mem::take(&mut self.focus_search) {
            SearchField::focus(ui.ctx(), id);
        }
        let mut changed = ui.put(field, SearchField::new(id, &mut self.query).hint("Search: words, kind:, result:, since:").facets(&facets)).changed();
        let bar = if one_row {
            Rect::from_min_max(pos2(field.right() + m::SPACING, top.top()), pos2(top.right() - count_w - m::SPACING, top.bottom()))
        } else {
            // Under the search, its first menu's text over the list's
            // first column's, as Log Viewer has it.
            let frame = style::inset(ui.ctx(), top, 2.0).left() - top.left();
            let indent = frame + m::CELL_INSET - m::TEXT_INSET;
            Rect::from_min_size(pos2(top.left() + indent, top.bottom() + m::SPACING), vec2(top.width() - indent, m::CONTROL_HEIGHT))
        };
        changed |= ui.put(bar, FilterBar::new(("replay", self.name.as_str()), &mut self.query, &facets).no_clear()).changed();
        if changed {
            self.pick();
            self.revealed = None;
        }
        style::label(ui.painter(), top.right(), top.center().y, Align::Max, &count, style::plain_font(), style::disabled_text(&p));
        let controls_bottom = if one_row { top.bottom() } else { bar.bottom() };

        // Playing: on through time, past stretches where nothing happens.
        if self.playing {
            let dt = ui.input(|i| i.stable_dt).min(0.1);
            self.playhead += (dt * 1000.0 * SPEED[self.speed]) as u64;
            if let Some(next) = self.next_after(self.playhead)
                && next > self.playhead + SKIP_MS
            {
                self.playhead = next - 1000;
            }
            if self.playhead >= end {
                self.playhead = end;
                self.playing = false;
                self.follow = true;
            }
            ui.ctx().request_repaint();
        }

        let body = Rect::from_min_max(pos2(rect.left(), controls_bottom + m::SPACING), rect.max);
        let list_w = (body.width() * 0.42).clamp(260.0, 440.0).min(body.width() - 240.0).max(200.0);
        let list_r = Rect::from_min_size(body.min, vec2(list_w, body.height()));
        self.list_ui(ui, list_r);

        // The screen as it was, the action under it, and the transport.
        let right = Rect::from_min_max(pos2(list_r.right() + m::SPACING, body.top()), body.max);
        let transport = Rect::from_min_max(pos2(right.left(), right.bottom() - Self::transport_height(right.width())), right.max);
        let detail = Rect::from_min_max(pos2(right.left(), transport.top() - m::SPACING - 2.0 * m::LABEL_ROW), pos2(right.right(), transport.top() - m::SPACING));
        let well = Rect::from_min_max(right.min, pos2(right.right(), detail.top() - m::SPACING));
        let want = self.frame_for(self.playhead);
        if want.is_some() && want != self.showing && want != self.asked {
            self.asked = want;
            let _ = self.frames.send((self.segments.clone(), self.playhead));
        }
        let size = self.texture.as_ref().map(|t| t.size_vec2()).or(self.size.map(|(w, h)| vec2(w, h)));
        let text = if want.is_none() { Some("No picture from then") } else if self.texture.is_none() { Some("Reading…") } else { None };
        let pic = picture_well(ui, well, size, self.texture.as_ref().filter(|_| want.is_some()), text);
        if let Some(pic) = pic {
            // Following a running screen, rings and captions fade as time
            // goes on, not at the playhead (which waits at the newest).
            let live = running && self.follow && self.playhead >= end;
            self.overlay(ui, pic, if live { history::now_ms().max(self.playhead) } else { self.playhead }, live);
        }
        self.detail_ui(ui, detail);
        self.transport_ui(ui, transport, (start, end), running);
    }

    /// The actions shown, the one at the playhead chosen.
    fn list_ui(&mut self, ui: &mut Ui, list_r: Rect) {
        let p = style::palette(ui.ctx());
        let current = self.action_at(self.playhead);
        let selected = current.and_then(|i| self.shown.iter().position(|&s| s == i));
        if selected != self.list.single() {
            self.list.selected = selected.into_iter().collect();
            self.list.cursor = selected;
            self.list.anchor = selected;
        }
        // Only an action newly at the playhead is brought into view (as it
        // plays, or comes in live), so the list scrolls freely otherwise.
        let reveal = if current != self.revealed {
            self.revealed = current;
            selected
        } else {
            None
        };
        let columns = [Column::new("Time", 72.0), Column::new("Action", 80.0)];
        let (actions, shown, query) = (&self.actions, &self.shown, &self.parsed);
        let failure = c(p.failure);
        let dim = style::disabled_text(&p);
        let empty = if self.actions.is_empty() { "No actions yet." } else { "No actions match." };
        let out = ui
            .scope_builder(egui::UiBuilder::new().max_rect(list_r), |ui| {
                ColumnListView::new(("replay-list", &self.name), &columns, shown.len(), &mut self.list).reveal(reveal).empty_text(empty).show(ui, |cell: &Cell| {
                    let a = &actions[shown[cell.row]];
                    match cell.column {
                        0 => cell.text(history::clock(a.at).get(11..19).unwrap_or("")),
                        _ => {
                            let font = style::plain_font();
                            let mut job = if cell.selected {
                                egui::text::LayoutJob::simple_singleline(command(a), font.clone(), cell.text_color)
                            } else {
                                search::highlight(&command(a), query, font.clone(), cell.text_color, search::mark_color(&p))
                            };
                            let (after, color) = match (&a.error, a.matched()) {
                                (Some(e), _) => (format!("  ✗ {e}"), if cell.selected { cell.text_color } else { failure }),
                                (None, Some(m)) => (format!("  → {m}"), if cell.selected { cell.text_color } else { dim }),
                                _ => (String::new(), dim),
                            };
                            job.append(&after, 0.0, egui::TextFormat::simple(font, color));
                            cell.text_job(job);
                        }
                    }
                })
            })
            .inner;
        if out.selection_changed
            && let Some(i) = self.list.single().and_then(|r| self.shown.get(r)).copied()
            && let Some(a) = self.actions.get(i)
        {
            self.playhead = a.at + a.ms;
            self.playing = false;
            self.follow = self.playhead >= self.range().1;
            // Chosen here, so already in view.
            self.revealed = Some(i);
        }
    }

    /// The action at the playhead: what was asked, and what came of it.
    fn detail_ui(&self, ui: &Ui, r: Rect) {
        let p = style::palette(ui.ctx());
        let painter = ui.painter().with_clip_rect(r);
        let (y1, y2) = (r.top() + m::LABEL_ROW / 2.0, r.top() + m::LABEL_ROW * 1.5);
        let Some(a) = self.action_at(self.playhead).and_then(|i| self.actions.get(i)) else {
            style::label_fit(&painter, r.left(), y1, Align::Min, "Before the first action.", style::plain_font(), style::disabled_text(&p), r.width());
            return;
        };
        // Narrow, the time is shorter, so the command keeps its room.
        let when = if r.width() < 380.0 {
            format!("{}  ·  {} ms", history::clock(a.at).get(11..19).unwrap_or(""), a.ms)
        } else {
            format!("{}  ·  took {} ms", history::clock(a.at).get(11..).unwrap_or(""), a.ms)
        };
        let when_w = style::label_width(ui.ctx(), &when, style::plain_font()).ceil();
        style::label(&painter, r.right(), y1, Align::Max, &when, style::plain_font(), style::disabled_text(&p));
        style::label_fit(&painter, r.left(), y1, Align::Min, &command(a), style::bold_font(), c(p.panel_text), (r.width() - when_w - m::SPACING).max(0.0));
        let (text, color) = match (&a.error, outcome(a)) {
            (Some(e), _) => (format!("✗ Failed: {e}"), c(p.failure)),
            (None, Some(o)) => (format!("→ {o}"), c(p.panel_text)),
            (None, None) => ("Done.".to_owned(), style::disabled_text(&p)),
        };
        style::label_fit(&painter, r.left(), y2, Align::Min, &text, style::plain_font(), color, r.width());
    }

    /// How tall the transport is `width` wide: one row, or (narrow) the
    /// scrubber over the buttons.
    fn transport_height(width: f32) -> f32 {
        if width < TRANSPORT_ONE_ROW { 2.0 * m::CONTROL_HEIGHT + m::TIGHT } else { m::CONTROL_HEIGHT }
    }

    /// Stepping, play, speed, the scrubber and Latest; or the export
    /// under way in the scrubber's place.
    fn transport_ui(&mut self, ui: &mut Ui, r: Rect, (start, end): (u64, u64), running: bool) {
        let p = style::palette(ui.ctx());
        let buttons = Rect::from_min_max(pos2(r.left(), r.bottom() - m::CONTROL_HEIGHT), r.max);
        let mut x = r.left();
        let mut button = |ui: &mut Ui, label: &str, w: f32| {
            let b = Rect::from_min_size(pos2(x, buttons.top()), vec2(w, m::CONTROL_HEIGHT));
            x += w + m::TIGHT;
            ui.put(b, Button::new(label).min_width(w)).clicked()
        };
        if button(ui, "◀", 30.0) {
            self.step(-1);
        }
        if button(ui, if self.playing { "Pause" } else { "Play" }, 64.0) {
            self.play_pause();
        }
        if button(ui, "▶", 30.0) {
            self.step(1);
        }
        // At the right: Latest (pushed while following); the speed where
        // there's room for it.
        let latest_w = 64.0;
        let speed_r = Rect::from_min_size(pos2(x, buttons.top()), vec2(58.0, m::CONTROL_HEIGHT));
        let speed_r = if speed_r.right() + m::SPACING + latest_w <= buttons.right() {
            ui.put(speed_r, MenuField::new(("replay-speed", self.name.as_str()), &mut self.speed, &SPEEDS).width(58.0));
            speed_r
        } else {
            Rect::from_min_size(pos2(x - m::TIGHT, buttons.top()), vec2(0.0, m::CONTROL_HEIGHT))
        };
        let latest_r = Rect::from_min_size(pos2(buttons.right() - latest_w, buttons.top()), vec2(latest_w, m::CONTROL_HEIGHT));
        let following = self.follow && self.playhead >= end;
        if ui.add_enabled_ui(!following || running, |ui| ui.put(latest_r, Button::new(if running { "Live" } else { "Latest" }).min_width(latest_w).pushed(following))).inner.clicked() {
            self.latest();
        }
        // The scrubber and the time: between them, or over them.
        let row = if r.height() > m::CONTROL_HEIGHT {
            Rect::from_min_size(r.min, vec2(r.width(), m::CONTROL_HEIGHT))
        } else {
            Rect::from_min_max(pos2(speed_r.right() + m::SPACING, r.top()), pos2(latest_r.left() - m::SPACING, r.bottom()))
        };
        if let Some((done, cancel)) = &self.exporting {
            let cancel_r = Rect::from_min_size(pos2(row.right() - 72.0, row.top()), vec2(72.0, m::CONTROL_HEIGHT));
            if ui.put(cancel_r, Button::new("Cancel").min_width(72.0)).clicked() {
                cancel.store(true, Ordering::Relaxed);
            }
            let status = Rect::from_min_max(row.min, pos2(cancel_r.left() - m::SPACING, row.bottom()));
            let label = format!("Exporting… {:.0}%", done * 100.0);
            ui.put(status, StatusBar::new(*done).label(&label).bar_height(6.0));
            ui.ctx().request_repaint_after(Duration::from_millis(100));
            return;
        }
        let time = format!("{} / {}", duration(self.playhead.saturating_sub(start)), duration(end - start));
        let time_w = style::label_width(ui.ctx(), &format!("{} / {}", duration(end - start), duration(end - start)), style::plain_font()).ceil();
        style::label(ui.painter(), row.right(), row.center().y, Align::Max, &time, style::plain_font(), style::disabled_text(&p));
        let scrub = Rect::from_min_max(row.min, pos2(row.right() - time_w - m::SPACING, row.bottom()));
        if scrub.width() > 40.0 {
            let mut secs = self.playhead.saturating_sub(start) as f32 / 1000.0;
            let total = (end - start) as f32 / 1000.0;
            let slider = Rect::from_center_size(scrub.center(), vec2(scrub.width(), 20.0));
            if ui.put(slider, Slider::new(&mut secs, 0.0..=total.max(0.001))).changed() {
                self.playhead = start + (secs * 1000.0) as u64;
                self.playing = false;
                self.follow = self.playhead >= end;
            }
        }
    }

    /// How the last export ended, for the window to say.
    pub fn take_exported(&mut self) -> Option<Result<PathBuf, String>> {
        self.exported.take()
    }

    /// Each click as a ring where it went, and the newest key or text
    /// under the picture, for a moment after it (`t`; `live`: on the
    /// clock, so drawn again as they fade).
    fn overlay(&self, ui: &Ui, pic: Rect, t: u64, live: bool) {
        let p = style::palette(ui.ctx());
        let painter = ui.painter().with_clip_rect(pic);
        let recent = self.actions.iter().rev().filter(|a| a.at <= t).take_while(|a| t - a.at < CAPTION_MS.max(RING_MS));
        let mut captioned = false;
        for a in recent {
            let since = t - a.at;
            let point = a.result.as_ref().and_then(|r| r.get("clicked")).and_then(|c| Some((c.get(0)?.as_f64()? as f32, c.get(1)?.as_f64()? as f32)));
            if let (Some((x, y)), Some((sw, sh))) = (point, self.size)
                && since < RING_MS
            {
                let at = pos2(pic.left() + x / sw * pic.width(), pic.top() + y / sh * pic.height());
                let grow = since as f32 / RING_MS as f32;
                let color = c(p.keyboard_navigation).gamma_multiply(1.0 - grow);
                painter.circle_stroke(at, 6.0 + 10.0 * grow, egui::Stroke::new(2.0, color));
            }
            if (a.kind == "key" || a.kind == "type") && since < CAPTION_MS && !captioned {
                captioned = true;
                let text = if a.kind == "type" { format!("“{}”", a.args.join(" ")) } else { a.args.join(" ") };
                let g = painter.layout_no_wrap(text, style::bold_font(), Color32::WHITE);
                let r = style::snap(ui.ctx(), Rect::from_center_size(pos2(pic.center().x, pic.bottom() - 24.0), g.size() + vec2(16.0, 8.0)));
                // Fades out over its last quarter.
                let fade = ((CAPTION_MS - since) as f32 / (CAPTION_MS as f32 / 4.0)).min(1.0);
                painter.rect_filled(r, 6.0, Color32::from_black_alpha((170.0 * fade) as u8));
                painter.galley(pos2(r.left() + 8.0, r.top() + 4.0), g, Color32::WHITE.gamma_multiply(fade));
            }
        }
        if live && t.saturating_sub(self.actions.last().map_or(0, |a| a.at)) < CAPTION_MS.max(RING_MS) {
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        }
    }
}

/// A screen's picture fitted into `well` (sunken, the picture centred in
/// it), or `text` in its place. Where the picture went.
pub fn picture_well(ui: &Ui, well: Rect, size: Option<egui::Vec2>, texture: Option<&TextureHandle>, text: Option<&str>) -> Option<Rect> {
    let p = style::palette(ui.ctx());
    let painter = ui.painter();
    style::sunken(painter, well, &p, false);
    let inner = style::inset(ui.ctx(), well, 2.0);
    painter.rect_filled(inner, 0.0, style::shade(&p, p.panel_background, 1.18));
    let size = size.unwrap_or(vec2(16.0, 10.0));
    let aspect = size.x / size.y.max(1.0);
    let room = inner.shrink(m::SPACING);
    let (w, h) = if room.width() / room.height().max(1.0) > aspect { (room.height() * aspect, room.height()) } else { (room.width(), room.width() / aspect) };
    let pic = style::snap(ui.ctx(), Rect::from_center_size(room.center(), vec2(w, h)));
    match (texture, text) {
        (Some(t), None) => {
            painter.rect_filled(pic.expand(1.0), 0.0, c(p.control_border));
            painter.image(t.id(), pic, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            Some(pic)
        }
        (_, text) => {
            let text = text.unwrap_or("");
            style::label_fit(&painter.with_clip_rect(inner), inner.center().x, inner.center().y, Align::Center, text, style::plain_font(), style::disabled_text(&p), inner.width() - 2.0 * m::SPACING);
            None
        }
    }
}
