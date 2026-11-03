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
//! comes. Frames are built on a worker (`record::Player`: played forward
//! from the last, back to a keyframe only on a jump), the latest asked
//! for winning; the window never waits on the disk.

use std::{
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{RecvTimeoutError, Sender, channel},
    },
    time::{Duration, Instant},
};

use olo_ui::{
    Proxy, agent,
    gpui::{App, AppContext as _, Context, Entity, EventEmitter, FontWeight, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Task, TextRun, Window, canvas, div, point, px},
    metrics as m,
    search::{self, Facet, FilterBar, Query, SearchEvent, SearchField, search_field},
    style::{self, c},
    theme,
    widgets::{Button, Column, ColumnListView, EmptyState, ListModel, MenuField, Slider, StatusBar},
};

use crate::{
    history::{self, Action},
    picture::{PictureWell, Shot},
    record,
};

pub enum ReplayMsg {
    /// What was added to the history (or all of it, `reset`), and the
    /// frames kept so far.
    Loaded {
        name: String,
        actions: Vec<Action>,
        texts: Vec<String>,
        reset: bool,
        segments: Arc<Vec<record::Segment>>,
    },
    Frame {
        name: String,
        at: u64,
        shot: Arc<Shot>,
    },
    /// Exporting: how far it has got (0–1), then how it ended.
    Progress {
        name: String,
        done: f32,
    },
    Exported {
        name: String,
        result: Result<PathBuf, String>,
    },
}

/// What the window hears from a history: an export ended.
pub struct Exported(pub Result<PathBuf, String>);

impl EventEmitter<Exported> for Replay {}

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
/// The transport's buttons.
const STEP_W: f32 = 30.0;
const PLAY_W: f32 = 64.0;
const SPEED_W: f32 = 58.0;
const LATEST_W: f32 = 64.0;

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
    facets: Option<(usize, Rc<Vec<Facet>>)>,
    /// When the shown actions were picked (a time filter moves on).
    picked: Instant,
    list: Entity<ListModel>,
    search: Entity<SearchField>,
    pub well: Entity<PictureWell>,
    playhead: u64,
    playing: bool,
    /// When playing last moved the playhead on, and when it's drawn
    /// next (`play_on`).
    played: Option<Instant>,
    play_next: Option<(Instant, Task<()>)>,
    /// Live, rings and captions drawn again as they change (`overlay`).
    live_next: Option<Task<()>>,
    speed: usize,
    /// At the newest action: new ones are chosen as they come.
    follow: bool,
    /// The action last brought into view in the list (only a new one is,
    /// so the list scrolls freely otherwise).
    revealed: Option<usize>,
    /// The time of the frame showing, and the one last asked for.
    showing: Option<u64>,
    asked: Option<u64>,
    frames: Sender<(Arc<Vec<record::Segment>>, u64)>,
    /// The screen's logical size, from its start (for placing clicks).
    size: Option<(f32, f32)>,
    /// An export under way (how far, and its Cancel).
    exporting: Option<(f32, Arc<AtomicBool>)>,
    /// The first and last times in it, and its stats (for how many
    /// actions, and when).
    bounds: (u64, u64),
    stats: Option<(usize, Instant, history::Stats)>,
    /// When the screen was last started.
    started: Option<u64>,
    /// The width it's shown in (the window lays it out).
    pub width: f32,
    /// The screen is running (its history grows; Latest is Live).
    pub running: bool,
    /// A time filter moving on with the clock: picked again each second.
    ticking: Option<Task<()>>,
    /// Following the history's folder, for as long as this is shown.
    _watch: Option<olo_ui::watch::Watch>,
}

/// Read screen `name`'s history and frames, and again whenever its folder
/// changes: only what was added, on a worker, for as long as the returned
/// watch is kept.
fn follow(name: &str, proxy: Proxy) -> Option<olo_ui::watch::Watch> {
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
            let changed =
                if read_frames { rx.recv().map_err(|_| RecvTimeoutError::Disconnected) } else { rx.recv_timeout(FRAMES_EVERY.saturating_sub(frames_read.map_or(Duration::ZERO, |t| t.elapsed()))) };
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
    olo_ui::watch::watch([history::dir(name)], move |_| {
        let _ = tx.send(());
    })
}

/// The time of the frame showing the screen at `t` (segments and their
/// frames are in time order).
fn frame_time(segments: &[record::Segment], t: u64) -> Option<u64> {
    let seg = segments.iter().rev().find(|s| s.frames.first().is_some_and(|f| f.at <= t))?;
    let i = seg.frames.partition_point(|f| f.at <= t);
    seg.frames.get(i.checked_sub(1)?).map(|f| f.at)
}

/// Build the frames asked for, the latest winning: played on from the
/// last one shown while the frames kept stay the same.
fn make_frames(name: String, proxy: Proxy) -> Sender<(Arc<Vec<record::Segment>>, u64)> {
    let (tx, rx) = channel::<(Arc<Vec<record::Segment>>, u64)>();
    let _ = std::thread::Builder::new().name("screen-frames".into()).spawn(move || {
        let latest = |mut ask: (Arc<Vec<record::Segment>>, u64)| {
            while let Ok(newer) = rx.try_recv() {
                ask = newer;
            }
            ask
        };
        let Ok(first) = rx.recv() else { return };
        let mut next = Some(latest(first));
        while let Some((segments, mut t)) = next.take() {
            let mut player = record::Player::new(&segments);
            loop {
                if let (Some(at), Some(f)) = (frame_time(&segments, t), player.at(t)) {
                    let shot = Shot { size: [f.width, f.height], rgba: f.rgba.clone() };
                    proxy.send(ReplayMsg::Frame { name: name.clone(), at, shot: Arc::new(shot) });
                }
                let Ok(ask) = rx.recv() else { return };
                let ask = latest(ask);
                if !Arc::ptr_eq(&ask.0, &segments) {
                    next = Some(ask);
                    break;
                }
                t = ask.1;
            }
        }
    });
    tx
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
    pub fn new(name: &str, proxy: Proxy, cx: &mut Context<Replay>) -> Replay {
        let search = search_field(cx, "Search: words, kind:, result:, since:");
        cx.subscribe(&search, |r: &mut Replay, s, e: &SearchEvent, cx| {
            if let SearchEvent::Changed = e {
                r.query = s.read(cx).text(cx).to_owned();
                r.pick();
                r.revealed = None;
                cx.notify();
            }
        })
        .detach();
        let list = ListModel::new(cx, false);
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
            list,
            search,
            well: cx.new(|_| PictureWell::new("Screen")),
            playhead: 0,
            playing: false,
            played: None,
            play_next: None,
            live_next: None,
            speed: 0,
            follow: true,
            revealed: None,
            showing: None,
            asked: None,
            frames: make_frames(name.to_owned(), proxy.clone()),
            size: None,
            exporting: None,
            bounds: (0, 0),
            stats: None,
            started: None,
            width: 600.0,
            running: false,
            ticking: None,
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

    pub fn message(&mut self, msg: ReplayMsg, cx: &mut Context<Self>) {
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
                    self.size = self
                        .actions
                        .iter()
                        .rev()
                        .find(|a| a.kind == "start" && a.error.is_none())
                        .and_then(|a| a.result.as_ref())
                        .and_then(|r| Some((r["width"].as_f64()? as f32, r["height"].as_f64()? as f32)));
                    let shape = self.size;
                    self.well.update(cx, |w, _| w.set_shape(shape));
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
                cx.notify();
            }
            ReplayMsg::Frame { name, at, shot } if name == self.name => {
                self.showing = Some(at);
                self.well.update(cx, |w, cx| w.set_shot(shot, cx));
            }
            ReplayMsg::Progress { name, done } if name == self.name => {
                if let Some((d, _)) = &mut self.exporting {
                    *d = done;
                    cx.notify();
                }
            }
            ReplayMsg::Exported { name, result } if name == self.name => {
                self.exporting = None;
                cx.emit(Exported(result));
                cx.notify();
            }
            _ => {}
        }
    }

    /// Make the history a video at `path`, on a worker.
    pub fn start_export(&mut self, path: PathBuf, proxy: Proxy, cx: &mut Context<Self>) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.exporting = Some((0.0, cancel.clone()));
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
        cx.notify();
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

    /// The filters' menus, each value with how many actions have it (the
    /// search's too).
    fn facets(&mut self, cx: &mut Context<Self>) -> Rc<Vec<Facet>> {
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
            let facets = Rc::new(vec![kind, result, since]);
            self.search.update(cx, |s, cx| s.set_facets(facets.clone(), cx));
            self.facets = Some((self.actions.len(), facets));
        }
        self.facets.as_ref().expect("made above").1.clone()
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
    pub fn step(&mut self, by: isize, cx: &mut Context<Self>) {
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
        cx.notify();
    }

    pub fn play_pause(&mut self, cx: &mut Context<Self>) {
        let (start, end) = self.range();
        self.playing = !self.playing;
        self.played = None;
        if self.playing {
            self.follow = false;
            if self.playhead >= end {
                self.playhead = start;
            }
        }
        cx.notify();
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    /// To the newest action, and on with the screen from there.
    pub fn latest(&mut self, cx: &mut Context<Self>) {
        self.follow = true;
        self.playing = false;
        self.playhead = self.range().1;
        cx.notify();
    }

    pub fn focus_search(&self, window: &mut Window, cx: &mut App) {
        self.search.update(cx, |s, cx| s.focus(window, cx));
    }

    fn scrub(&mut self, secs: f32, cx: &mut Context<Self>) {
        let (start, end) = self.range();
        self.playhead = start + (secs.max(0.0) * 1000.0) as u64;
        self.playing = false;
        self.follow = self.playhead >= end;
        cx.notify();
    }

    /// The actions shown, the one at the playhead chosen; a row chosen
    /// moves the playhead there.
    fn list_view(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let current = self.action_at(self.playhead);
        let selected = current.and_then(|i| self.shown.iter().position(|&s| s == i));
        // Only an action newly at the playhead is brought into view (as it
        // plays, or comes in live), so the list scrolls freely otherwise.
        let reveal = if current != self.revealed {
            self.revealed = current;
            selected
        } else {
            None
        };
        self.list.update(cx, |l, _| {
            if l.state.single() != selected {
                match selected {
                    Some(r) => l.state.select(r),
                    None => l.state.selected.clear(),
                }
                l.state.cursor = selected;
                l.state.anchor = selected;
            }
            if let Some(r) = reveal {
                l.reveal(r);
            }
        });
        let columns = vec![Column::new("Time", 72.0), Column::new("Action", 80.0)];
        let empty = if self.actions.is_empty() { "No actions yet." } else { "No actions match." };
        let me = cx.entity();
        let (cells, chosen) = (me.clone(), me);
        ColumnListView::new(format!("replay-list-{}", self.name), &self.list, columns, self.shown.len(), move |cell, window, cx| {
            let (time, line, after) = {
                let r = cells.read(cx);
                let Some(a) = r.shown.get(cell.row).and_then(|&i| r.actions.get(i)) else { return };
                let after = match (&a.error, a.matched()) {
                    (Some(e), _) => Some((format!("  ✗ {e}"), true)),
                    (None, Some(m)) => Some((format!("  → {m}"), false)),
                    _ => None,
                };
                (history::clock(a.at).get(11..19).unwrap_or("").to_owned(), command(a), after)
            };
            if cell.column == 0 {
                cell.text(window, cx, time);
                return;
            }
            let p = theme::palette(cx);
            let font = style::ui_font(FontWeight::NORMAL);
            let mut runs: Vec<TextRun> = if cell.selected {
                vec![TextRun { len: line.len(), font: font.clone(), color: cell.text_color, background_color: None, underline: None, strikethrough: None, size: None }]
            } else {
                let query = cells.read(cx).parsed.clone();
                search::highlight(&line, &query, &font, cell.text_color, search::mark_color(&p))
            };
            let mut text = line;
            if let Some((after, failed)) = after {
                let color = if cell.selected {
                    cell.text_color
                } else if failed {
                    c(p.failure)
                } else {
                    style::disabled_text(&p)
                };
                runs.push(TextRun { len: after.len(), font, color, background_color: None, underline: None, strikethrough: None, size: None });
                text.push_str(&after);
            }
            cell.text_runs(window, cx, text, &runs);
        })
        .empty_text(empty)
        .on_select(move |_, cx| {
            chosen.update(cx, |r, cx| {
                let Some(i) = r.list.read(cx).state.single().and_then(|row| r.shown.get(row)).copied() else { return };
                let Some(a) = r.actions.get(i) else { return };
                r.playhead = a.at + a.ms;
                r.playing = false;
                r.follow = r.playhead >= r.range().1;
                // Chosen here, so already in view.
                r.revealed = Some(i);
                cx.notify();
            })
        })
    }

    /// Stepping, play, speed, the scrubber and Latest; or the export
    /// under way in the scrubber's place.
    fn transport(&mut self, width: f32, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let me = cx.entity();
        let (start, end) = self.range();
        let following = self.follow && self.playhead >= end;
        let running = self.running;
        let two_rows = width < TRANSPORT_ONE_ROW;
        let on = |me: &Entity<Replay>, f: fn(&mut Replay, &mut Context<Replay>)| {
            let me = me.clone();
            move |_: &mut Window, cx: &mut App| me.update(cx, f)
        };
        let mut buttons = div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(m::CONTROL_HEIGHT))
            .gap(px(m::TIGHT))
            .child(Button::new("◀").id("replay-back").min_width(STEP_W).on_click(on(&me, |r, cx| r.step(-1, cx))))
            .child(Button::new(if self.playing { "Pause" } else { "Play" }).id("replay-play").min_width(PLAY_W).on_click(on(&me, Replay::play_pause)))
            .child(Button::new("▶").id("replay-next").min_width(STEP_W).on_click(on(&me, |r, cx| r.step(1, cx))));
        // The speed where there's room for it.
        let used = 2.0 * STEP_W + PLAY_W + 2.0 * m::TIGHT;
        if used + m::TIGHT + SPEED_W + m::SPACING + LATEST_W <= width {
            let speeds: Vec<SharedString> = SPEEDS.iter().map(|s| SharedString::from(*s)).collect();
            let set = me.clone();
            buttons = buttons.child(div().child(MenuField::new(format!("replay-speed-{}", self.name), speeds, self.speed).width(SPEED_W).on_change(move |i, _, cx| {
                set.update(cx, |r, cx| {
                    r.speed = i;
                    cx.notify();
                })
            })));
        }
        let latest = Button::new(if running { "Live" } else { "Latest" }).id("replay-latest").min_width(LATEST_W).pushed(following).enabled(!following || running).on_click(on(&me, Replay::latest));
        // The scrubber and the time; or the export under way.
        let middle = if let Some((done, cancel)) = &self.exporting {
            let cancel = cancel.clone();
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(m::SPACING))
                .h(px(m::CONTROL_HEIGHT))
                .child(div().flex_1().child(StatusBar::new(*done).label(format!("Exporting… {:.0}%", done * 100.0)).bar_height(6.0)))
                .child(Button::new("Cancel").id("replay-cancel-export").min_width(72.0).on_click(move |_, _| cancel.store(true, Ordering::Relaxed)))
        } else {
            let time = format!("{} / {}", duration(self.playhead.saturating_sub(start)), duration(end - start));
            let widest = format!("{} / {}", duration(end - start), duration(end - start));
            let time_w = style::text_width_app(cx, &widest, style::PLAIN_SIZE, &style::ui_font(FontWeight::NORMAL)).ceil();
            let total = ((end - start) as f32 / 1000.0).max(0.001);
            let secs = self.playhead.saturating_sub(start) as f32 / 1000.0;
            let scrub = me.clone();
            let label = canvas(
                |_, _, _| {},
                move |b, _, window, cx| {
                    let p = theme::palette(cx);
                    let r = style::label(window, cx, b.right(), b.center().y, style::Align::Right, time.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                    agent::record(window, cx, agent::Node::label(time.clone(), r));
                },
            )
            .w(px(time_w))
            .h(px(m::CONTROL_HEIGHT))
            .flex_shrink_0();
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(m::SPACING))
                .h(px(m::CONTROL_HEIGHT))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Slider::new(format!("replay-scrub-{}", self.name), secs.min(total), 0.0..=total).on_change(move |v, _, cx| scrub.update(cx, |r, cx| r.scrub(v, cx)))),
                )
                .child(label)
        };
        if two_rows {
            div().flex().flex_col().gap(px(m::TIGHT)).child(middle).child(div().flex().flex_row().items_center().child(buttons).child(div().flex_1()).child(latest))
        } else {
            div().flex().flex_row().items_center().gap(px(m::SPACING)).child(buttons).child(div().flex_1().min_w_0().child(middle)).child(latest)
        }
    }

    /// The action at the playhead: what was asked, and what came of it.
    fn detail(&self) -> impl IntoElement + use<> {
        let action = self.action_at(self.playhead).and_then(|i| self.actions.get(i)).cloned();
        canvas(
            |_, _, _| {},
            move |r, _, window, cx| {
                let p = theme::palette(cx);
                let (plain, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
                let (y1, y2) = (r.top() + px(m::LABEL_ROW / 2.0), r.top() + px(m::LABEL_ROW * 1.5));
                window.with_content_mask(Some(olo_ui::gpui::ContentMask { bounds: r }), |window| {
                    let Some(a) = &action else {
                        let t = style::label_fit(window, cx, r.left(), y1, style::Align::Left, "Before the first action.", style::PLAIN_SIZE, plain.clone(), style::disabled_text(&p), r.size.width);
                        agent::record(window, cx, agent::Node::label("Before the first action.", t));
                        return;
                    };
                    // Narrow, the time is shorter, so the command keeps its room.
                    let when = if f32::from(r.size.width) < 380.0 {
                        format!("{}  ·  {} ms", history::clock(a.at).get(11..19).unwrap_or(""), a.ms)
                    } else {
                        format!("{}  ·  took {} ms", history::clock(a.at).get(11..).unwrap_or(""), a.ms)
                    };
                    let w = style::label(window, cx, r.right(), y1, style::Align::Right, when.clone(), style::PLAIN_SIZE, plain.clone(), style::disabled_text(&p));
                    let cmd = command(a);
                    let t = style::label_fit(
                        window,
                        cx,
                        r.left(),
                        y1,
                        style::Align::Left,
                        cmd.clone(),
                        style::PLAIN_SIZE,
                        bold.clone(),
                        c(p.panel_text),
                        (w.left() - px(m::SPACING) - r.left()).max(px(0.0)),
                    );
                    agent::record(window, cx, agent::Node { value: Some(when.into()), ..agent::Node::label(cmd, t) });
                    let (text, color) = match (&a.error, outcome(a)) {
                        (Some(e), _) => (format!("✗ Failed: {e}"), c(p.failure)),
                        (None, Some(o)) => (format!("→ {o}"), c(p.panel_text)),
                        (None, None) => ("Done.".to_owned(), style::disabled_text(&p)),
                    };
                    let t = style::label_fit(window, cx, r.left(), y2, style::Align::Left, text.clone(), style::PLAIN_SIZE, plain.clone(), color, r.size.width);
                    agent::record(window, cx, agent::Node::label(text, t));
                });
            },
        )
        .w_full()
        .h(px(2.0 * m::LABEL_ROW))
        .flex_shrink_0()
    }

    /// Whether a click's ring is growing or a caption fading at `t` (drawn
    /// every frame then), and else how long until a caption shown now
    /// starts to fade.
    fn overlay_motion(&self, t: u64) -> (bool, Option<u64>) {
        let fade_from = CAPTION_MS - CAPTION_MS / 4;
        let mut still = None;
        let mut captioned = false;
        for a in self.actions[..self.actions.partition_point(|a| a.at <= t)].iter().rev().take_while(|a| t - a.at < CAPTION_MS.max(RING_MS)) {
            let since = t - a.at;
            if since < RING_MS && a.result.as_ref().is_some_and(|r| r.get("clicked").is_some()) {
                return (true, None);
            }
            if (a.kind == "key" || a.kind == "type") && !captioned {
                captioned = true;
                if since >= fade_from {
                    return (true, None);
                }
                still = Some(fade_from - since);
            }
        }
        (false, still)
    }

    /// Playing: drawn again when something shown next changes (the next
    /// frame or action, the scrubber's next device pixel, the time's next
    /// second; every frame while a ring or caption fades), not every
    /// frame: a long history plays at a few frames a second.
    fn play_on(&mut self, scrub_w: f32, window: &mut Window, cx: &mut Context<Self>) {
        let (start, end) = self.range();
        let t = self.playhead;
        let (moving, still) = self.overlay_motion(t);
        if moving {
            self.play_next = None;
            window.request_animation_frame();
            return;
        }
        let pixel = (end - start) as f32 / (scrub_w * window.scale_factor()).max(1.0);
        let second = 1000 - t.saturating_sub(start) % 1000;
        let next = self.next_after(t).map_or(u64::MAX, |n| n - t);
        let wait = (pixel.min(second as f32).min(next as f32).min(still.unwrap_or(u64::MAX) as f32) / SPEED[self.speed]).max(1000.0 / 120.0);
        let due = Instant::now() + Duration::from_secs_f32(wait / 1000.0);
        // (Drawn again before then, say as a frame came: the timer set
        // stands, unless this is sooner.)
        if self.play_next.as_ref().is_some_and(|(at, _)| *at <= due) {
            return;
        }
        let timer = cx.spawn(async move |me, cx| {
            cx.background_executor().timer(Duration::from_secs_f32(wait / 1000.0)).await;
            let _ = me.update(cx, |r, cx| {
                r.play_next = None;
                cx.notify();
            });
        });
        self.play_next = Some((due, timer));
    }

    /// Each click as a ring where it went, and the newest key or text
    /// under the picture, for a moment after it (`t`; `live`: on the
    /// clock, so drawn again as they fade).
    fn overlay(&mut self, t: u64, live: bool, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let recent: Vec<Action> = self.actions.iter().rev().filter(|a| a.at <= t).take_while(|a| t - a.at < CAPTION_MS.max(RING_MS)).cloned().collect();
        // Live, rings and captions go on the clock: every frame while
        // they move, else once as a caption starts to fade and once as
        // the last goes.
        if live && !recent.is_empty() {
            match self.overlay_motion(t) {
                (true, _) => window.request_animation_frame(),
                (false, still) => {
                    let gone = recent.iter().map(|a| CAPTION_MS.max(RING_MS) - (t - a.at)).max().unwrap_or(0);
                    let wait = still.unwrap_or(gone).min(gone).max(8);
                    self.live_next = Some(cx.spawn(async move |me, cx| {
                        cx.background_executor().timer(Duration::from_millis(wait)).await;
                        let _ = me.update(cx, |_, cx| cx.notify());
                    }));
                }
            }
        }
        let (size, well) = (self.size, self.well.clone());
        canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                let Some(pic) = well.read(cx).shown() else { return };
                let p = theme::palette(cx);
                let bold = style::ui_font(FontWeight::BOLD);
                window.with_content_mask(Some(olo_ui::gpui::ContentMask { bounds: pic }), |window| {
                    let mut captioned = false;
                    for a in &recent {
                        let since = t - a.at;
                        let at = a.result.as_ref().and_then(|r| r.get("clicked")).and_then(|c| Some((c.get(0)?.as_f64()? as f32, c.get(1)?.as_f64()? as f32)));
                        if let (Some((x, y)), Some((sw, sh))) = (at, size)
                            && since < RING_MS
                        {
                            let center = point(pic.left() + pic.size.width * (x / sw), pic.top() + pic.size.height * (y / sh));
                            let grow = since as f32 / RING_MS as f32;
                            let color = c(p.keyboard_navigation).opacity(1.0 - grow);
                            let radius = px(6.0 + 10.0 * grow);
                            window.paint_quad(
                                olo_ui::gpui::outline(
                                    olo_ui::gpui::Bounds::centered_at(center, olo_ui::gpui::size(radius * 2.0 + px(2.0), radius * 2.0 + px(2.0))),
                                    color,
                                    olo_ui::gpui::BorderStyle::Solid,
                                )
                                .corner_radii(radius + px(1.0))
                                .border_widths(px(2.0)),
                            );
                        }
                        if (a.kind == "key" || a.kind == "type") && since < CAPTION_MS && !captioned {
                            captioned = true;
                            let text = if a.kind == "type" { format!("“{}”", a.args.join(" ")) } else { a.args.join(" ") };
                            // Fades out over its last quarter.
                            let fade = ((CAPTION_MS - since) as f32 / (CAPTION_MS as f32 / 4.0)).min(1.0);
                            let w = f32::from(style::text_width(window, text.clone(), style::PLAIN_SIZE, bold.clone()));
                            let plate = style::snap_centred(window, point(pic.center().x, pic.bottom() - px(24.0)), w + 16.0, style::PLAIN_SIZE * style::LINE_HEIGHT + 8.0);
                            window.paint_quad(olo_ui::gpui::fill(plate, style::c([0, 0, 0, (170.0 * fade) as u8])).corner_radii(px(6.0)));
                            style::label(window, cx, plate.center().x, plate.center().y, style::Align::Center, text, style::PLAIN_SIZE, bold.clone(), olo_ui::gpui::white().opacity(fade));
                        }
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }
}

impl Render for Replay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = |text: &'static str| div().size_full().child(EmptyState::new(text).icon("agent-screens")).into_any_element();
        if !self.loaded {
            return empty("Reading the history…");
        }
        if self.go_on_picking() {
            window.request_animation_frame();
        }
        let (_, end) = self.range();
        if end == 0 {
            return empty("Nothing has been done on this screen yet. Its actions show here as an agent takes them.");
        }
        // A time filter moves on with the clock.
        let timed = self.parsed.terms.iter().any(|t| t.key.as_deref() == Some("since"));
        if timed && self.ticking.is_none() {
            self.ticking = Some(cx.spawn(async move |me, cx| {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let _ = me.update(cx, |r, cx| {
                    r.ticking = None;
                    if r.parsed.terms.iter().any(|t| t.key.as_deref() == Some("since")) {
                        r.pick();
                        cx.notify();
                    }
                });
            }));
        }

        // Playing: on through time, past stretches where nothing happens.
        if self.playing {
            let now = Instant::now();
            // (A stall of seconds doesn't jump the playhead as far.)
            let dt = self.played.map_or(0.0, |t| now.duration_since(t).as_secs_f32().min(2.0));
            self.played = Some(now);
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
        }
        if !self.playing {
            self.play_next = None;
        }

        // The search and its filters, and how many actions they keep.
        let me = cx.entity();
        let facets = self.facets(cx);
        let width = self.width;
        let count = if self.shown.len() == self.actions.len() {
            format!("{} action{}", style::thousands(self.actions.len()), if self.actions.len() == 1 { "" } else { "s" })
        } else {
            format!("{} of {}", style::thousands(self.shown.len()), style::thousands(self.actions.len()))
        };
        let font = style::ui_font(FontWeight::NORMAL);
        let count_w = style::text_width_app(cx, &count, style::PLAIN_SIZE, &font).ceil();
        let bar_w = FilterBar::width(cx, &self.query, &facets) + m::SPACING + 40.0;
        let one_row = width - bar_w - count_w - 2.0 * m::SPACING >= 220.0;
        let field_w = if one_row { width - bar_w - count_w - 2.0 * m::SPACING } else { width - count_w - m::SPACING };
        let filters_me = me.clone();
        let filters = FilterBar::new(format!("replay-{}", self.name), self.query.clone(), facets).no_clear().on_change(move |text, _, cx| {
            filters_me.update(cx, |r, cx| {
                r.search.update(cx, |s, cx| s.set_text(text.clone(), cx));
                r.query = text;
                r.pick();
                r.revealed = None;
                cx.notify();
            })
        });
        let count_label = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let r = style::label(window, cx, b.right(), b.center().y, style::Align::Right, count.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                agent::record(window, cx, agent::Node::label(count.clone(), r));
            },
        )
        .w(px(count_w))
        .h(px(m::CONTROL_HEIGHT))
        .flex_shrink_0();
        let mut top = div().flex().flex_row().items_center().gap(px(m::SPACING)).h(px(m::CONTROL_HEIGHT)).child(div().w(px(field_w)).child(self.search.clone()));
        let controls = if one_row {
            top = top.child(div().flex_1().min_w_0().child(filters.max_width(bar_w))).child(count_label);
            div().child(top)
        } else {
            // Under the search, its first menu's text over the list's
            // first column's, as Log Viewer has it.
            top = top.child(div().flex_1()).child(count_label);
            let frame = f32::from(style::inset(window, olo_ui::gpui::Bounds::default(), 2.0).left());
            let indent = frame + m::CELL_INSET - m::TEXT_INSET;
            div().flex().flex_col().gap(px(m::SPACING)).child(top).child(div().pl(px(indent)).child(filters.max_width(width - indent)))
        };

        // The screen as it was, the action under it, and the transport.
        let list_w = (width * 0.42).clamp(260.0, 440.0).min(width - 240.0).max(200.0);
        let right_w = width - list_w - m::SPACING;
        if self.playing {
            self.play_on(right_w, window, cx);
        }
        let want = frame_time(&self.segments, self.playhead);
        if want.is_some() && want != self.showing && want != self.asked {
            self.asked = want;
            let _ = self.frames.send((self.segments.clone(), self.playhead));
        }
        let text: Option<SharedString> = if want.is_none() {
            Some("No picture from then".into())
        } else if !self.well.read(cx).has_shot() {
            Some("Reading…".into())
        } else {
            None
        };
        self.well.update(cx, |w, cx| w.set_text(text, cx));
        // Following a running screen, rings and captions fade as time
        // goes on, not at the playhead (which waits at the newest).
        let live = self.running && self.follow && self.playhead >= end;
        let t = if live { history::now_ms().max(self.playhead) } else { self.playhead };
        let overlay = self.overlay(t, live, window, cx);
        let list = self.list_view(cx);
        let detail = self.detail();
        let transport = self.transport(right_w, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(controls)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING))
                    // (A list fills a flex column: in a plain box stretched to
                    // its width, its own `size_full` comes out none wide.)
                    .child(div().w(px(list_w)).flex_shrink_0().flex().flex_col().child(div().flex_1().min_h_0().flex().flex_col().child(list)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(m::SPACING))
                            .child(div().flex_1().min_h_0().relative().child(self.well.clone()).child(overlay))
                            .child(detail)
                            .child(transport),
                    ),
            )
            .into_any_element()
    }
}
