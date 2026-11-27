//! A screen's history in the pane (the History tab): its actions in a
//! list that grows as the agent works (followed by the source's worker,
//! reading only what was added), narrowed by a search and filters (kind,
//! result, time, as Log Viewer's); the screen as it was at the action
//! chosen or wherever the scrubber is, with what the action was and what
//! came of it under it; and playback in time with the actions (play and
//! pause, speed, stepping action by action), each click shown as a ring
//! where it went and each key or text under the picture. Stretches where
//! nothing happened are skipped while playing. At the newest action the
//! history follows the screen (Latest), each new action chosen as it
//! comes. Frames are built on a worker (played forward from the last,
//! back to a keyframe only on a jump), the latest asked for winning; the
//! pane never waits on the disk or network.

use std::{
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Sender, channel},
    },
    time::{Duration, Instant},
};

use olo_kit::{
    Child, Cx, Element, FontWeight, IntoElement as _, Messenger, ParentElement as _, SharedString, Styled as _, TextRun, View, WindowCx, agent, canvas, div, metrics as m, point, px,
    search::{self, Facet, Query, SearchEvent, SearchField, search_field},
    style::{self, c},
    theme,
    widgets::{Button, Column, ColumnListView, EmptyState, ListModel, MenuField, Slider, StatusBar},
};
use olo_screen_codec::FrameIndex;
use olo_sync::Mutex;

use crate::{
    history::{self, Action},
    picture::{PictureWell, Shot},
    source::{ExportCancel, HistoryEvent, HistoryHandle, ScreenId, ScreenSource},
};

/// Maximum actions buffered inside the coalesced UI mailbox between paints so
/// a stalled window thread can never accumulate unbounded history batches.
const MAX_PENDING_ACTIONS: usize = 50_000;

/// Single coalesced mailbox between the history follower worker and `Replay`
/// so a busy UI receives at most one wake message and one latest
/// `Arc<FrameIndex>` instead of queueing repeated clones.
#[derive(Default)]
struct PendingHistory {
    has_loaded: bool,
    actions: Vec<Action>,
    texts: Vec<String>,
    reset: bool,
    index_reset: bool,
    frames: Option<Arc<FrameIndex>>,
    failed: Option<String>,
}

impl PendingHistory {
    fn push_loaded(&mut self, actions: Vec<Action>, texts: Vec<String>, reset: bool, index_reset: bool, frames: Arc<FrameIndex>) {
        self.has_loaded = true;
        self.failed = None;
        self.frames = Some(frames);
        if reset {
            self.actions = actions;
            self.texts = texts;
            self.reset = true;
            self.index_reset = true;
        } else {
            self.actions.extend(actions);
            self.texts.extend(texts);
            self.index_reset |= index_reset;
        }
        if self.actions.len() > MAX_PENDING_ACTIONS && self.actions.len() == self.texts.len() {
            let drop_n = self.actions.len() - MAX_PENDING_ACTIONS;
            self.actions.drain(..drop_n);
            self.texts.drain(..drop_n);
            self.reset = true;
        }
    }

    fn push_failed(&mut self, message: String) {
        self.failed = Some(message);
    }

    fn take(&mut self) -> PendingHistory {
        std::mem::take(self)
    }
}

pub(crate) enum ReplayMsg {
    /// Wake the UI to drain the single coalesced `PendingHistory` mailbox.
    HistoryReady,
    Frame {
        epoch: u64,
        want: u64,
        actual_at: u64,
        shot: Arc<Shot>,
    },
    /// Frame fetch for `(epoch, want)` failed or returned nothing.
    FrameMissed {
        epoch: u64,
        want: u64,
        reason: Option<String>,
    },
}

/// Emitted whenever history updates arrive so the pane can refresh its header
/// counts/timestamps and inform the host sidebar.
pub(crate) struct ReplayUpdated;

/// How long a click's ring and a key's caption stay over the picture.
const RING_MS: u64 = 700;
const CAPTION_MS: u64 = 1500;
/// While playing, a stretch longer than this with nothing in it is
/// skipped (to just before what comes next).
const SKIP_MS: u64 = 3000;
const SPEEDS: [&str; 4] = ["1×", "2×", "4×", "8×"];
const SPEED: [f32; 4] = [1.0, 2.0, 4.0, 8.0];

/// The kinds of action, as the Kind filter groups them: its value, its
/// name, and the commands in it.
pub(crate) const KINDS: [(&str, &str, &[&str]); 6] = [
    ("pointer", "Pointer", &["click", "move", "down", "up", "drag", "scroll", "wheel", "scroll-to"]),
    // ("code": a one-time code from the person's phone, put in the tab
    // waiting for it by the phone service, olo-phone.)
    ("keys", "Keys and Typing", &["key", "type", "set", "code"]),
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

/// Pure tracking of the displayed, requested, and terminally missed frame
/// across scrubs and history resets so late worker replies can never overwrite
/// the current playhead and a terminal miss leaves a visible status.
#[derive(Debug, Default)]
struct FrameTracker {
    index_gen: u64,
    showing: Option<u64>,
    showing_actual: Option<u64>,
    asked: Option<u64>,
    missed: Option<u64>,
    missed_reason: Option<String>,
}

impl FrameTracker {
    fn on_reset(&mut self) {
        self.index_gen = self.index_gen.wrapping_add(1);
        self.showing = None;
        self.showing_actual = None;
        self.asked = None;
        self.missed = None;
        self.missed_reason = None;
    }

    fn on_index_grown(&mut self) {
        self.missed = None;
        self.missed_reason = None;
    }

    /// Decide whether `want` needs to be fetched from the worker.
    fn request_for(&mut self, want: Option<u64>) -> Option<u64> {
        let Some(want_at) = want else {
            self.asked = None;
            return None;
        };
        if self.showing == Some(want_at) {
            self.asked = Some(want_at);
            self.missed = None;
            self.missed_reason = None;
            return None;
        }
        if self.asked == Some(want_at) || self.missed == Some(want_at) {
            return None;
        }
        self.asked = Some(want_at);
        self.missed = None;
        self.missed_reason = None;
        Some(want_at)
    }

    /// Accept a decoded frame reply only if its index generation and target
    /// timestamp still match the current playhead's desired frame.
    fn accept_frame(&mut self, epoch: u64, want: u64, actual_at: u64, current_want: Option<u64>) -> bool {
        if epoch != self.index_gen || current_want != Some(want) {
            return false;
        }
        self.showing = Some(want);
        self.showing_actual = Some(actual_at);
        self.missed = None;
        self.missed_reason = None;
        true
    }

    /// Record a terminal frame miss or per-request error for `(epoch, want)`.
    /// Returns `true` if the visible status changed (so the view should redraw
    /// out of "Reading…" or show the host's error words without retrying).
    fn record_miss(&mut self, epoch: u64, want: u64, current_want: Option<u64>, reason: Option<String>) -> bool {
        if epoch != self.index_gen {
            return false;
        }
        if self.asked == Some(want) {
            self.asked = None;
        }
        if current_want == Some(want) && (self.missed != Some(want) || self.missed_reason != reason) {
            self.missed = Some(want);
            if reason.is_some() {
                self.showing = None;
                self.showing_actual = None;
            }
            self.missed_reason = reason;
            return true;
        }
        false
    }
}

/// Monotonic effective timestamp for every action so [`Action::is_oversized`]
/// placeholders (`at == 0`) keep their exact 0-based index without dragging
/// bounds or breaking binary search.
fn effective_action_times(actions: &[Action], frames_lo: Option<u64>) -> Vec<u64> {
    let first_real = actions.iter().find(|a| !a.is_oversized()).map(|a| a.at).or(frames_lo).unwrap_or(0);
    let mut out = Vec::with_capacity(actions.len());
    let mut last = first_real;
    for a in actions {
        if a.is_oversized() {
            out.push(last);
        } else {
            last = last.max(a.at);
            out.push(a.at);
        }
    }
    out
}

/// Validate a [`HistoryEvent::Loaded`] batch so inconsistent `(actions, texts)`
/// lengths from a buggy adapter are rejected cleanly instead of panicking on
/// the UI thread.
fn validate_loaded_batch(actions: &[Action], texts: &[String]) -> Result<(), String> {
    if actions.len() != texts.len() {
        return Err(format!("inconsistent history batch: {} actions vs {} search texts", actions.len(), texts.len()));
    }
    Ok(())
}

pub(crate) struct Replay {
    source: Arc<dyn ScreenSource>,
    id: ScreenId,
    id_key: String,
    pub(crate) name: String,
    actions: Vec<Action>,
    /// Monotonic effective timestamps aligned with `actions` so oversized
    /// placeholders (`at == 0`) do not break `partition_point` or scrubbing.
    action_times: Vec<u64>,
    /// Explicitly chosen oversized placeholder index (cleared when playhead
    /// moves via scrub/play/latest or selecting a non-oversized action).
    selected_oversized: Option<usize>,
    /// Each action's text, to search (its command, what it matched, its
    /// error), made on the worker that reads them.
    texts: Vec<String>,
    /// A pick under way: how far through the actions, and what it kept.
    picking: Option<(usize, Vec<usize>)>,
    /// Actions of each kind (as `KINDS` groups them), and failed.
    kind_counts: [usize; KINDS.len()],
    failed: usize,
    frames_index: Arc<FrameIndex>,
    loaded: bool,
    /// Plain sentence shown when history isn't available for this source,
    /// rather than faking an empty history.
    unavailable: Option<String>,
    query: String,
    parsed: Query,
    shown: Vec<usize>,
    /// The filters' menus, for the actions there were when made.
    facets: Option<(usize, Rc<Vec<Facet>>)>,
    /// When the shown actions were picked (a time filter moves on).
    picked: Instant,
    list: ListModel,
    search: Child<SearchField>,
    pub(crate) well: Child<PictureWell>,
    playhead: u64,
    playing: bool,
    /// When playing last moved the playhead on, and when it's drawn
    /// next (`play_on`).
    played: Option<Instant>,
    play_next: Option<(Instant, olo_kit::TimerId)>,
    /// Live, rings and captions drawn again as they change (`overlay`).
    live_next: Option<olo_kit::TimerId>,
    speed: usize,
    /// At the newest action: new ones are chosen as they come.
    follow: bool,
    /// The action last brought into view in the list (only a new one is,
    /// so the list scrolls freely otherwise).
    revealed: Option<usize>,
    /// Showing/asked/missed frame state guarded by history reset generation.
    frame_track: FrameTracker,
    frames_tx: Sender<(u64, u64, u64)>,
    /// The screen's logical size, from its start (for placing clicks).
    size: Option<(f32, f32)>,
    /// Active export progress & cancel handle mirrored from [`crate::ScreenPane`]
    /// so exports outlive screen switches and reconnect when returning.
    exporting: Option<(f32, ExportCancel)>,
    /// The first and last times in it, and its stats (for how many
    /// actions, and when).
    bounds: (u64, u64),
    stats: Option<(usize, Instant, history::Stats)>,
    /// When the screen was last started.
    started: Option<u64>,
    /// The width it's shown in (the pane lays it out).
    pub(crate) width: f32,
    /// The screen is running (its history grows; Latest is Live).
    pub(crate) running: bool,
    /// A time filter moving on with the clock: picked again each second.
    ticking: Option<olo_kit::TimerId>,
    /// The timer the rest of the actions are picked on.
    picking_on: Option<olo_kit::TimerId>,
    /// Coalesced mailbox between the history follower worker and this view.
    pending_history: Arc<Mutex<PendingHistory>>,
    history_queued: Arc<AtomicBool>,
    /// Following the history through the source, for as long as this is shown.
    history: Box<dyn HistoryHandle>,
    history_active: bool,
}

/// Build the frames asked for, the latest winning: played on from the
/// last one shown while the frames kept stay the same (through `source`).
fn make_frames(source: Arc<dyn ScreenSource>, me: Messenger<ReplayMsg>) -> Sender<(u64, u64, u64)> {
    let (tx, rx) = channel::<(u64, u64, u64)>();
    let _ = std::thread::Builder::new().name("screen-frames".into()).spawn(move || {
        let latest = |mut ask: (u64, u64, u64)| {
            while let Ok(newer) = rx.try_recv() {
                ask = newer;
            }
            ask
        };
        let Ok(first) = rx.recv() else { return };
        let mut ask = latest(first);
        loop {
            let (epoch, want, playhead) = ask;
            match source.frame_at(playhead) {
                Ok(Some((actual_at, f))) => {
                    let shot = Shot { size: [f.width, f.height], rgba: f.rgba };
                    me.send(ReplayMsg::Frame { epoch, want, actual_at, shot: Arc::new(shot) });
                }
                Ok(None) => {
                    // If superseded by a newer queued scrub (unblocked via
                    // `source.cancel_frame_at()`), jump straight to the latest
                    // without flashing a spurious miss.
                    if let Ok(next) = rx.try_recv() {
                        ask = latest(next);
                        continue;
                    }
                    me.send(ReplayMsg::FrameMissed { epoch, want, reason: None });
                }
                Err(reason) => {
                    if let Ok(next) = rx.try_recv() {
                        ask = latest(next);
                        continue;
                    }
                    me.send(ReplayMsg::FrameMissed { epoch, want, reason: Some(reason) });
                }
            }
            let Ok(next) = rx.recv() else { return };
            ask = latest(next);
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
pub(crate) fn search_text(a: &Action) -> String {
    if a.is_oversized() {
        return "Action too large to show\noversized".to_owned();
    }
    format!("{}\n{}\n{}", command(a), a.matched().unwrap_or_default(), a.error.as_deref().unwrap_or(""))
}

/// Whether `action` (with precomputed [`search_text`] `text`) is one the
/// search and filters keep (`now`: for the time filter).
fn action_matches(action: &Action, text: &str, query: &Query, now: u64) -> bool {
    query.matches(|key, n| match key {
        None => n.in_text(text),
        Some("kind") => n.is(&action.kind) || KINDS.iter().any(|(v, _, kinds)| n.is(v) && kinds.contains(&action.kind.as_str())),
        Some("result") => (n.is("failed") && action.error.is_some()) || (n.is("ok") && action.error.is_none()),
        Some("since") => TIMES.iter().any(|(v, _, back)| {
            n.is(v) && {
                let from = if *back == 0 { history::midnight(now) } else { now.saturating_sub(*back) };
                action.at >= from
            }
        }),
        _ => false,
    })
}

/// An action's command line: its kind and arguments, quoted where they
/// need it.
pub(crate) fn command(a: &Action) -> String {
    if a.is_oversized() {
        return "Action too large to show".to_owned();
    }
    let args: Vec<String> = a.args.iter().map(|x| if x.contains(' ') || x.is_empty() { format!("“{x}”") } else { x.clone() }).collect();
    if args.is_empty() { a.kind.clone() } else { format!("{} {}", a.kind, args.join(" ")) }
}

/// What came of an action, for people: what it matched, the program it
/// started, the window focused after; None when it says nothing.
pub(crate) fn outcome(a: &Action) -> Option<String> {
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
    // (Its answer read while the screen was still changing.)
    if r.pointer("/after/settled").and_then(|v| v.as_bool()) == Some(false) {
        parts.push("still changing".into());
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

impl Replay {
    pub(crate) fn new(source: Arc<dyn ScreenSource>, cx: &mut Cx<Replay>) -> Replay {
        let info = source.info();
        let id = info.id.clone();
        let id_key = id.key();
        let name = id.name.clone();
        let me = cx.messenger();
        let search = search_field(cx, "Search: words, kind:, result:, since:");
        cx.subscribe(&search, |r: &mut Replay, e: &SearchEvent, cx| {
            if let SearchEvent::Changed = e {
                r.query = r.search.read(cx).text(cx).to_owned();
                r.pick();
                r.revealed = None;
                if !r.source.info().capabilities.can_export_discontiguous {
                    cx.emit(ReplayUpdated);
                }
                cx.invalidate();
            }
        });
        // (Several rows chosen, ⇧ or ⌘: a part of the history to export.)
        let list = ListModel::new(true);
        let frames_tx = make_frames(source.clone(), me.clone());
        let pending_history = Arc::new(Mutex::new(PendingHistory::default()));
        let history_queued = Arc::new(AtomicBool::new(false));
        let hist_me = me.clone();
        let pending_cb = pending_history.clone();
        let queued_cb = history_queued.clone();
        let history = source.follow_history(Box::new(move |ev| {
            {
                let mut slot = pending_cb.lock().unwrap_or_else(|e| e.into_inner());
                match ev {
                    HistoryEvent::Loaded { actions, texts, reset, index_reset, frames } => slot.push_loaded(actions, texts, reset, index_reset, frames),
                    HistoryEvent::Failed(message) => slot.push_failed(message),
                }
            }
            if !queued_cb.swap(true, Ordering::AcqRel) {
                hist_me.send(ReplayMsg::HistoryReady);
            }
        }));
        Replay {
            source,
            id,
            id_key,
            name,
            actions: Vec::new(),
            action_times: Vec::new(),
            selected_oversized: None,
            texts: Vec::new(),
            picking: None,
            kind_counts: [0; KINDS.len()],
            failed: 0,
            frames_index: Arc::default(),
            loaded: false,
            unavailable: None,
            query: String::new(),
            parsed: Query::default(),
            shown: Vec::new(),
            facets: None,
            picked: Instant::now(),
            list,
            search,
            well: cx.new_view(|cx| PictureWell::new("Screen", cx)),
            playhead: 0,
            playing: false,
            played: None,
            play_next: None,
            live_next: None,
            speed: 0,
            follow: true,
            revealed: None,
            frame_track: FrameTracker::default(),
            frames_tx,
            size: info.size.map(|(w, h)| (w as f32, h as f32)),
            exporting: None,
            bounds: (0, 0),
            stats: None,
            started: None,
            width: 600.0,
            running: info.is_running(),
            ticking: None,
            picking_on: None,
            pending_history,
            history_queued,
            history,
            history_active: true,
        }
    }

    pub(crate) fn set_history_active(&mut self, active: bool) {
        if self.history_active != active {
            self.history_active = active;
            self.history.set_active(active);
        }
    }

    pub(crate) fn set_exporting(&mut self, state: Option<(f32, ExportCancel)>, cx: &mut Cx<Self>) {
        let changed = match (&self.exporting, &state) {
            (None, None) => false,
            (Some((a, ac)), Some((b, bc))) => (a - b).abs() > f32::EPSILON || !Arc::ptr_eq(&ac.flag(), &bc.flag()),
            _ => true,
        };
        if changed {
            self.exporting = state;
            cx.invalidate();
        }
    }

    pub(crate) fn id(&self) -> &ScreenId {
        &self.id
    }

    pub(crate) fn id_key(&self) -> &str {
        &self.id_key
    }

    pub(crate) fn source(&self) -> &Arc<dyn ScreenSource> {
        &self.source
    }

    pub(crate) fn loaded(&self) -> bool {
        self.loaded
    }

    pub(crate) fn unavailable(&self) -> Option<&str> {
        self.unavailable.as_deref()
    }

    pub(crate) fn actions(&self) -> &[Action] {
        &self.actions
    }

    /// When the screen was last started, by its history.
    pub(crate) fn started(&self) -> Option<u64> {
        self.started
    }

    /// Where it's at (the time shown): for a picture of it then (Take Screenshot…).
    pub(crate) fn playhead(&self) -> u64 {
        self.playhead
    }

    fn told(&mut self, msg: ReplayMsg, cx: &mut Cx<Self>) {
        match msg {
            ReplayMsg::HistoryReady => {
                self.history_queued.store(false, Ordering::Release);
                let batch = self.pending_history.lock().unwrap_or_else(|e| e.into_inner()).take();
                if let Some(message) = batch.failed {
                    tracing::warn!("screen {} history: {message}", self.id_key);
                    self.unavailable = Some(message);
                    self.loaded = true;
                    cx.invalidate();
                    return;
                }
                if !batch.has_loaded {
                    return;
                }
                let PendingHistory { actions, texts, reset, index_reset, frames, .. } = batch;
                let frames = frames.unwrap_or_else(|| self.frames_index.clone());
                if let Err(err) = validate_loaded_batch(&actions, &texts) {
                    tracing::warn!("screen {} history: {err}", self.id_key);
                    self.unavailable = Some(err);
                    self.loaded = true;
                    cx.invalidate();
                    return;
                }
                let from = if reset { 0 } else { self.actions.len() };
                if reset {
                    self.actions = actions;
                    self.texts = texts;
                    self.selected_oversized = None;
                    (self.kind_counts, self.failed) = ([0; KINDS.len()], 0);
                } else {
                    self.actions.extend(actions);
                    self.texts.extend(texts);
                }
                if reset || index_reset {
                    // Recording replaced or reset: invalidate cached frame state and drop
                    // decoded pixels from the old index generation immediately so the next
                    // paint fetches fresh pixels instead of displaying stale ones.
                    self.frame_track.on_reset();
                    self.well.update(cx, |w, cx| w.clear(None, cx));
                } else if frames.len() != self.frames_index.len() {
                    self.frame_track.on_index_grown();
                }
                self.count(from..self.actions.len());
                self.frames_index = frames;
                self.action_times = effective_action_times(&self.actions, self.frames_index.bounds().map(|(lo, _)| lo));
                self.bounds = self.find_bounds();
                if reset {
                    self.started = None;
                }
                if let Some(a) = self.actions[from..].iter().rev().find(|a| a.kind == "start" && a.error.is_none()) {
                    self.started = Some(a.at);
                }
                if self.size.is_none() || reset {
                    let from_history = self
                        .actions
                        .iter()
                        .rev()
                        .find(|a| a.kind == "start" && a.error.is_none())
                        .and_then(|a| a.result.as_ref())
                        .and_then(|r| Some((r["width"].as_f64()? as f32, r["height"].as_f64()? as f32)));
                    self.size = from_history.or_else(|| self.source.info().size.map(|(w, h)| (w as f32, h as f32)));
                    let shape = self.size;
                    self.well.update(cx, |w, _| w.set_shape(shape));
                }
                self.loaded = true;
                self.unavailable = None;
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
                cx.emit(ReplayUpdated);
                cx.invalidate();
            }
            ReplayMsg::Frame { epoch, want, actual_at, shot } => {
                let current_want = self.frames_index.frame_time(self.playhead);
                if self.frame_track.accept_frame(epoch, want, actual_at, current_want) {
                    self.well.update(cx, |w, cx| w.set_shot(shot, cx));
                }
            }
            ReplayMsg::FrameMissed { epoch, want, reason } => {
                let current_want = self.frames_index.frame_time(self.playhead);
                if self.frame_track.record_miss(epoch, want, current_want, reason.clone()) {
                    if let Some(err) = reason {
                        self.well.update(cx, |w, cx| w.clear(Some(SharedString::from(err)), cx));
                    }
                    cx.invalidate();
                }
            }
        }
    }

    /// The actions an export takes (their numbers in the history): the
    /// rows chosen, when there are several; else those the list shows,
    /// when a search or filter leaves some out; else all (None).
    pub(crate) fn export_part(&self) -> Option<Vec<usize>> {
        let rows = &self.list.state().selected;
        if rows.len() > 1 {
            return Some(rows.iter().filter_map(|&r| self.shown.get(r).copied()).collect());
        }
        (self.shown.len() < self.actions.len()).then(|| self.shown.clone())
    }

    /// The size of the screen's last picture (what an export is, unzoomed).
    pub(crate) fn screen_size(&self) -> Option<(usize, usize)> {
        self.frames_index.last_size
    }

    /// Whether any recorded frames exist in the history index (independent of
    /// `last_size`, which may be `None` when the newest frame failed header checks).
    pub(crate) fn has_frames(&self) -> bool {
        !self.frames_index.is_empty()
    }

    /// Pixel size of the currently displayed history frame, if any (used as a
    /// fallback for screenshot zoom labels when `screen_size()` is `None`).
    pub(crate) fn current_shot_size(&self, cx: &WindowCx) -> Option<(usize, usize)> {
        self.well.read(cx).current_shot().map(|s| (s.size[0], s.size[1]))
    }

    /// Whether the current selection/filter has gaps that this host cannot
    /// export (an older host helper lacking `"discontiguous_part"`).
    pub(crate) fn export_discontiguous_blocked(&self) -> bool {
        self.export_part().is_some_and(|p| crate::export::has_gaps(&p)) && !self.source.info().capabilities.can_export_discontiguous
    }

    /// The time shown, as the scrubber says it ("1:42").
    pub(crate) fn time_shown(&self) -> String {
        let (start, _) = self.range();
        duration(self.playhead.saturating_sub(start))
    }

    /// Whether action `i` is one the search and filters keep (`now`:
    /// for the time filter).
    fn matches(&self, i: usize, now: u64) -> bool {
        action_matches(&self.actions[i], self.texts[i].as_str(), &self.parsed, now)
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
    fn facets(&mut self, cx: &mut Cx<Self>) -> Rc<Vec<Facet>> {
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
        let mut lo = u64::MAX;
        let mut hi = 0u64;
        if let Some((f_lo, f_hi)) = self.frames_index.bounds() {
            lo = lo.min(f_lo);
            hi = hi.max(f_hi);
        }
        for a in &self.actions {
            if a.is_oversized() {
                continue;
            }
            lo = lo.min(a.at);
            hi = hi.max(a.at.saturating_add(a.ms));
        }
        if lo > hi { (0, 0) } else { (lo, hi) }
    }

    /// The next thing after `t`: a frame or an action.
    fn next_after(&self, t: u64) -> Option<u64> {
        let frame = self.frames_index.next_after(t);
        let mut i = self.action_times.partition_point(|&at| at <= t);
        while let Some(a) = self.actions.get(i) {
            if !a.is_oversized() {
                break;
            }
            i += 1;
        }
        let action = self.actions.get(i).map(|a| a.at);
        frame.into_iter().chain(action).min()
    }

    /// The last action begun by `t` (actions are in the order they
    /// began, near enough: each is kept as it ends).
    fn action_at(&self, t: u64) -> Option<usize> {
        if let Some(i) = self.selected_oversized
            && self.actions.get(i).is_some_and(Action::is_oversized)
        {
            return Some(i);
        }
        let mut i = self.action_times.partition_point(|&at| at <= t).checked_sub(1)?;
        while self.actions.get(i).is_some_and(Action::is_oversized) {
            if i == 0 {
                return None;
            }
            i -= 1;
        }
        Some(i)
    }

    /// The history's stats, worked out again only as it grows or (while
    /// the screen runs) once a second.
    pub(crate) fn stats(&mut self, running: bool) -> &history::Stats {
        let stale = self.stats.as_ref().is_none_or(|(n, at, _)| *n != self.actions.len() || (running && at.elapsed() >= Duration::from_secs(1)));
        if stale {
            let st = history::stats(&self.name, &self.actions, running, history::now_ms());
            self.stats = Some((self.actions.len(), Instant::now(), st));
        }
        &self.stats.as_ref().expect("made above").2
    }

    /// Go `by` actions on (or back) among those shown.
    pub(crate) fn step(&mut self, by: isize, cx: &mut Cx<Self>) {
        let now = self.action_at(self.playhead);
        let pos = now.map(|i| self.shown.partition_point(|&s| s <= i) as isize - 1);
        let to = match pos {
            None | Some(-1) if by > 0 => Some(0),
            Some(p) if p >= 0 => Some((p + by).clamp(0, self.shown.len() as isize - 1) as usize),
            _ => None,
        };
        if let Some(&idx) = to.and_then(|i| self.shown.get(i))
            && let Some(a) = self.actions.get(idx)
        {
            let (start, end) = self.range();
            if a.is_oversized() {
                self.selected_oversized = Some(idx);
                if let Some(&t) = self.action_times.get(idx) {
                    self.playhead = t.clamp(start, end);
                }
                self.follow = false;
            } else {
                self.selected_oversized = None;
                self.playhead = a.at.saturating_add(a.ms);
                self.follow = self.playhead >= end;
            }
            self.playing = false;
        }
        cx.invalidate();
    }

    pub(crate) fn play_pause(&mut self, cx: &mut Cx<Self>) {
        let (start, end) = self.range();
        self.selected_oversized = None;
        self.playing = !self.playing;
        self.played = None;
        if self.playing {
            self.follow = false;
            if self.playhead >= end {
                self.playhead = start;
            }
        }
        cx.invalidate();
    }

    pub(crate) fn playing(&self) -> bool {
        self.playing
    }

    /// To the newest action, and on with the screen from there.
    pub(crate) fn latest(&mut self, cx: &mut Cx<Self>) {
        self.selected_oversized = None;
        self.follow = true;
        self.playing = false;
        self.playhead = self.range().1;
        cx.invalidate();
    }

    pub(crate) fn focus_search(&self, cx: &mut WindowCx) {
        self.search.update(cx, |s, cx| s.focus(cx));
    }

    fn scrub(&mut self, secs: f32, cx: &mut Cx<Self>) {
        let (start, end) = self.range();
        self.selected_oversized = None;
        self.playhead = start.saturating_add((secs.max(0.0) * 1000.0) as u64);
        self.playing = false;
        self.follow = self.playhead >= end;
        cx.invalidate();
    }

    /// The actions shown, the one at the playhead chosen; a row chosen
    /// moves the playhead there.
    fn list_view(&mut self, cx: &mut Cx<Self>) -> Element {
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
        {
            let mut state = self.list.state_mut();
            if state.single() != selected {
                match selected {
                    Some(r) => state.select(r),
                    None => state.selected.clear(),
                }
                state.cursor = selected;
                state.anchor = selected;
            }
        }
        if let Some(r) = reveal {
            self.list.reveal(cx, r);
        }
        let columns = vec![Column::new("Time", 72.0), Column::new("Action", 80.0)];
        let empty = if self.actions.is_empty() { "No actions yet." } else { "No actions match." };
        ColumnListView::new(cx, format!("replay-list-{}", self.id_key), &self.list, columns, self.shown.len(), move |r: &Replay, cell, g| {
            let (time, line, after, oversized) = {
                let Some(a) = r.shown.get(cell.row).and_then(|&i| r.actions.get(i)) else { return };
                if a.is_oversized() {
                    ("—".to_owned(), command(a), None, true)
                } else {
                    let after = match (&a.error, a.matched()) {
                        (Some(e), _) => Some((format!("  ✗ {e}"), true)),
                        (None, Some(m)) => Some((format!("  → {m}"), false)),
                        _ => None,
                    };
                    (history::clock(a.at).get(11..19).unwrap_or("").to_owned(), command(a), after, false)
                }
            };
            if cell.column == 0 {
                cell.text(g, time);
                return;
            }
            let p = theme::palette();
            let font = style::ui_font(FontWeight::NORMAL);
            let base_color = if !cell.selected && oversized { style::disabled_text(&p) } else { cell.text_color };
            let mut runs: Vec<TextRun> = if cell.selected {
                vec![TextRun { len: line.len(), font: font.clone(), color: cell.text_color, background_color: None, underline: None, strikethrough: None, size: None }]
            } else {
                let query = r.parsed.clone();
                search::highlight(&line, &query, &font, base_color, search::mark_color(&p))
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
            cell.text_runs(g, text, &runs);
        })
        .empty_text(empty)
        .on_select(cx.listener(|r: &mut Replay, _: &(), cx| {
            if let Some(i) = r.list.state().single().and_then(|row| r.shown.get(row)).copied()
                && let Some(a) = r.actions.get(i)
            {
                let (start, end) = r.range();
                if a.is_oversized() {
                    r.selected_oversized = Some(i);
                    if let Some(&t) = r.action_times.get(i) {
                        r.playhead = t.clamp(start, end);
                    }
                    r.follow = false;
                } else {
                    r.selected_oversized = None;
                    r.playhead = a.at.saturating_add(a.ms);
                    r.follow = r.playhead >= end;
                }
                r.playing = false;
                // Chosen here, so already in view.
                r.revealed = Some(i);
            }
            if !r.source.info().capabilities.can_export_discontiguous {
                cx.emit(ReplayUpdated);
            }
            cx.invalidate();
        }))
        .into_any_element()
    }

    /// Stepping, play, speed, the scrubber and Latest; or the export
    /// under way in the scrubber's place.
    fn transport(&mut self, width: f32, cx: &mut Cx<Self>) -> Element {
        let (start, end) = self.range();
        let following = self.follow && self.playhead >= end;
        let running = self.running;
        let two_rows = width < TRANSPORT_ONE_ROW;
        let on = |f: fn(&mut Replay, &mut Cx<Replay>)| cx.listener(move |r: &mut Replay, _: &olo_kit::ClickEvent, cx| f(r, cx));
        let mut buttons = div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(m::CONTROL_HEIGHT))
            .gap(px(m::TIGHT))
            .child(Button::new("◀").id("replay-back").min_width(STEP_W).on_click(on(|r, cx| r.step(-1, cx))))
            .child(Button::new(if self.playing { "Pause" } else { "Play" }).id("replay-play").min_width(PLAY_W).on_click(on(Replay::play_pause)))
            .child(Button::new("▶").id("replay-next").min_width(STEP_W).on_click(on(|r, cx| r.step(1, cx))));
        // The speed where there's room for it.
        let used = 2.0 * STEP_W + PLAY_W + 2.0 * m::TIGHT;
        if used + m::TIGHT + SPEED_W + m::SPACING + LATEST_W <= width {
            let speeds: Vec<SharedString> = SPEEDS.iter().map(|s| SharedString::from(*s)).collect();
            buttons = buttons.child(div().child(MenuField::new(format!("replay-speed-{}", self.id_key), speeds, self.speed).width(SPEED_W).on_change(cx.listener(|r: &mut Replay, i: &usize, cx| {
                r.speed = *i;
                cx.invalidate();
            }))));
        }
        let latest = Button::new(if running { "Live" } else { "Latest" }).id("replay-latest").min_width(LATEST_W).pushed(following).enabled(!following || running).on_click(on(Replay::latest));
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
                .child(Button::new("Cancel").id("replay-cancel-export").min_width(72.0).on_click(move |_, _| cancel.cancel()))
        } else {
            let span = end.saturating_sub(start);
            let time = format!("{} / {}", duration(self.playhead.saturating_sub(start)), duration(span));
            let widest = format!("{} / {}", duration(span), duration(span));
            let time_w = style::text_width_app(&widest, style::PLAIN_SIZE, &style::ui_font(FontWeight::NORMAL)).ceil();
            let total = (span as f32 / 1000.0).max(0.001);
            let secs = self.playhead.saturating_sub(start) as f32 / 1000.0;
            let label = canvas(move |g, b| {
                let p = theme::palette();
                let r = style::label(g, b.right(), b.center().y, style::Align::Right, time.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                agent::record(g, agent::Node::label(time.clone(), r));
            })
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
                        .child(Slider::new(format!("replay-scrub-{}", self.id_key), secs.min(total), 0.0..=total).on_change(cx.listener(|r: &mut Replay, v: &f32, cx| r.scrub(*v, cx)))),
                )
                .child(label)
        };
        if two_rows {
            div().flex().flex_col().gap(px(m::TIGHT)).child(middle).child(div().flex().flex_row().items_center().child(buttons).child(div().flex_1()).child(latest)).into_any_element()
        } else {
            div().flex().flex_row().items_center().gap(px(m::SPACING)).child(buttons).child(div().flex_1().min_w_0().child(middle)).child(latest).into_any_element()
        }
    }

    /// The action at the playhead: what was asked, and what came of it.
    fn detail(&self) -> Element {
        let action = self.action_at(self.playhead).and_then(|i| self.actions.get(i)).cloned();
        canvas(move |g, r| {
            let p = theme::palette();
            let (plain, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
            let (y1, y2) = (r.top() + px(m::LABEL_ROW / 2.0), r.top() + px(m::LABEL_ROW * 1.5));
            // (Cut at its sides; a point more above and below, where a
            // line snapped to the device's pixels at 1.5× starts just
            // over its row.)
            let mask = olo_kit::Bounds::from_corners(point(r.left(), r.top() - px(1.0)), point(r.right(), r.bottom() + px(1.0)));
            g.with_content_mask(Some(olo_kit::ContentMask { bounds: mask }), |g| {
                let Some(a) = &action else {
                    let t = style::label_fit(g, r.left(), y1, style::Align::Left, "Before the first action.", style::PLAIN_SIZE, plain.clone(), style::disabled_text(&p), r.size.width);
                    agent::record(g, agent::Node::label("Before the first action.", t));
                    return;
                };
                if a.is_oversized() {
                    let cmd = command(a);
                    let t = style::label_fit(g, r.left(), y1, style::Align::Left, cmd.clone(), style::PLAIN_SIZE, bold.clone(), style::disabled_text(&p), r.size.width);
                    agent::record(g, agent::Node::label(cmd, t));
                    let body = "This action exceeded the host’s size limit and can’t be shown here.";
                    let t = style::label_fit(g, r.left(), y2, style::Align::Left, body, style::PLAIN_SIZE, plain.clone(), style::disabled_text(&p), r.size.width);
                    agent::record(g, agent::Node::label(body, t));
                    return;
                }
                // Narrow, the time is shorter, so the command keeps its room.
                let when = if f32::from(r.size.width) < 380.0 {
                    format!("{}  ·  {} ms", history::clock(a.at).get(11..19).unwrap_or(""), a.ms)
                } else {
                    format!("{}  ·  took {} ms", history::clock(a.at).get(11..).unwrap_or(""), a.ms)
                };
                let w = style::label(g, r.right(), y1, style::Align::Right, when.clone(), style::PLAIN_SIZE, plain.clone(), style::disabled_text(&p));
                let cmd = command(a);
                let t = style::label_fit(g, r.left(), y1, style::Align::Left, cmd.clone(), style::PLAIN_SIZE, bold.clone(), c(p.panel_text), (w.left() - px(m::SPACING) - r.left()).max(px(0.0)));
                agent::record(g, agent::Node { value: Some(when.into()), ..agent::Node::label(cmd, t) });
                let (text, color) = match (&a.error, outcome(a)) {
                    (Some(e), _) => (format!("✗ Failed: {e}"), c(p.failure)),
                    (None, Some(o)) => (format!("→ {o}"), c(p.panel_text)),
                    (None, None) => ("Done.".to_owned(), style::disabled_text(&p)),
                };
                let t = style::label_fit(g, r.left(), y2, style::Align::Left, text.clone(), style::PLAIN_SIZE, plain.clone(), color, r.size.width);
                agent::record(g, agent::Node::label(text, t));
            });
        })
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
        let end = self.action_times.partition_point(|&at| at <= t);
        for a in self.actions[..end].iter().rev().filter(|a| !a.is_oversized() && a.at <= t).take_while(|a| t.saturating_sub(a.at) < CAPTION_MS.max(RING_MS)) {
            let since = t.saturating_sub(a.at);
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
    fn play_on(&mut self, scrub_w: f32, cx: &mut Cx<Self>) {
        let (start, end) = self.range();
        let t = self.playhead;
        let (moving, still) = self.overlay_motion(t);
        if moving {
            self.stop_play_timer(cx);
            Self::next_frame(cx);
            return;
        }
        let pixel = (end - start) as f32 / (scrub_w * cx.scale_factor()).max(1.0);
        let second = 1000 - t.saturating_sub(start) % 1000;
        let next = self.next_after(t).map_or(u64::MAX, |n| n - t);
        let wait = (pixel.min(second as f32).min(next as f32).min(still.unwrap_or(u64::MAX) as f32) / SPEED[self.speed]).max(1000.0 / 120.0);
        let due = Instant::now() + Duration::from_secs_f32(wait / 1000.0);
        // (Drawn again before then, say as a frame came: the timer set
        // stands, unless this is sooner.)
        if self.play_next.as_ref().is_some_and(|(at, _)| *at <= due) {
            return;
        }
        self.stop_play_timer(cx);
        let timer = cx.after_do(Duration::from_secs_f32(wait / 1000.0), |r, cx| {
            r.play_next = None;
            cx.invalidate();
        });
        self.play_next = Some((due, timer));
    }

    fn stop_play_timer(&mut self, cx: &mut Cx<Self>) {
        if let Some((_, timer)) = self.play_next.take() {
            cx.cancel(timer);
        }
    }

    /// Shown again as the next frame starts: one step of what moves.
    fn next_frame(cx: &mut Cx<Self>) {
        let me = cx.view_ref();
        cx.on_next_frame(move |cx| me.update(cx, |_, cx| cx.invalidate()));
    }

    /// Each click as a ring where it went, and the newest key or text
    /// under the picture, for a moment after it (`t`; `live`: on the
    /// clock, so drawn again as they fade).
    fn overlay(&mut self, t: u64, live: bool, cx: &mut Cx<Self>) -> Element {
        let recent: Vec<Action> = self.actions.iter().rev().filter(|a| !a.is_oversized() && a.at <= t).take_while(|a| t.saturating_sub(a.at) < CAPTION_MS.max(RING_MS)).cloned().collect();
        // Live, rings and captions go on the clock: every frame while
        // they move, else once as a caption starts to fade and once as
        // the last goes.
        if let Some(timer) = self.live_next.take() {
            cx.cancel(timer);
        }
        if live && !recent.is_empty() {
            match self.overlay_motion(t) {
                (true, _) => Self::next_frame(cx),
                (false, still) => {
                    let gone = recent.iter().map(|a| CAPTION_MS.max(RING_MS) - (t - a.at)).max().unwrap_or(0);
                    let wait = still.unwrap_or(gone).min(gone).max(8);
                    self.live_next = Some(cx.after_do(Duration::from_millis(wait), |r, cx| {
                        r.live_next = None;
                        cx.invalidate();
                    }));
                }
            }
        }
        let (size, shown) = (self.size, self.well.read(cx).shown_at());
        canvas(move |g, _| {
            let Some(pic) = shown.get() else { return };
            let p = theme::palette();
            let bold = style::ui_font(FontWeight::BOLD);
            g.with_content_mask(Some(olo_kit::ContentMask { bounds: pic }), |g| {
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
                        g.paint_quad(
                            olo_kit::outline(olo_kit::Bounds::centered_at(center, olo_kit::size(radius * 2.0 + px(2.0), radius * 2.0 + px(2.0))), color, olo_kit::BorderStyle::Solid)
                                .corner_radii(radius + px(1.0))
                                .border_widths(px(2.0)),
                        );
                    }
                    if (a.kind == "key" || a.kind == "type") && since < CAPTION_MS && !captioned {
                        captioned = true;
                        let text = if a.kind == "type" { format!("“{}”", a.args.join(" ")) } else { a.args.join(" ") };
                        // Fades out over its last quarter.
                        let fade = ((CAPTION_MS - since) as f32 / (CAPTION_MS as f32 / 4.0)).min(1.0);
                        let w = f32::from(style::text_width(text.clone(), style::PLAIN_SIZE, bold.clone()));
                        let plate = style::snap_centred(g, point(pic.center().x, pic.bottom() - px(24.0)), w + 16.0, style::PLAIN_SIZE * style::LINE_HEIGHT + 8.0);
                        g.paint_quad(olo_kit::fill(plate, style::c([0, 0, 0, (170.0 * fade) as u8])).corner_radii(px(6.0)));
                        style::label(g, plate.center().x, plate.center().y, style::Align::Center, text, style::PLAIN_SIZE, bold.clone(), olo_kit::white().opacity(fade));
                    }
                }
            });
        })
        .absolute()
        .inset_0()
        .into_any_element()
    }
}

impl View for Replay {
    type Message = ReplayMsg;

    fn message(&mut self, msg: ReplayMsg, cx: &mut Cx<Self>) {
        self.told(msg, cx);
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let empty = |text: &'static str| div().size_full().child(EmptyState::new(text).icon("agent-screens")).into_any_element();
        if !self.loaded {
            return empty("Reading the history…");
        }
        if let Some(reason) = &self.unavailable {
            return div().size_full().child(EmptyState::new(reason.clone()).icon("agent-screens")).into_any_element();
        }
        // (On a timer, not the next frame: a stretch that picks nothing
        // new draws nothing, and a window that draws nothing is shown no
        // frame to go on at.)
        if self.go_on_picking() && self.picking_on.is_none() {
            self.picking_on = Some(cx.after_do(Duration::from_millis(1), |r, cx| {
                r.picking_on = None;
                cx.invalidate();
            }));
        }
        let (_, end) = self.range();
        if end == 0 && self.actions.is_empty() {
            return empty("Nothing has been done on this screen yet. Its actions show here as an agent takes them.");
        }
        // A time filter moves on with the clock.
        let timed = self.parsed.terms.iter().any(|t| t.key.as_deref() == Some("since"));
        if timed && self.ticking.is_none() {
            self.ticking = Some(cx.after_do(Duration::from_secs(1), |r, cx| {
                r.ticking = None;
                if r.parsed.terms.iter().any(|t| t.key.as_deref() == Some("since")) {
                    r.pick();
                    cx.invalidate();
                }
            }));
        }

        // Playing: on through time, past stretches where nothing happens.
        if self.playing {
            let now = Instant::now();
            // (A stall of seconds doesn't jump the playhead as far.)
            let dt = self.played.map_or(0.0, |t| now.duration_since(t).as_secs_f32().min(2.0));
            self.played = Some(now);
            self.playhead = self.playhead.saturating_add((dt * 1000.0 * SPEED[self.speed]) as u64);
            if let Some(next) = self.next_after(self.playhead)
                && next > self.playhead.saturating_add(SKIP_MS)
            {
                self.playhead = next.saturating_sub(1000);
            }
            if self.playhead >= end {
                self.playhead = end;
                self.playing = false;
                self.follow = true;
            }
        }
        if !self.playing {
            self.stop_play_timer(cx);
        }

        // The search and its filters, and how many actions they keep.
        let facets = self.facets(cx);
        let width = self.width;
        let count = if self.shown.len() == self.actions.len() {
            format!("{} action{}", style::thousands(self.actions.len()), if self.actions.len() == 1 { "" } else { "s" })
        } else {
            format!("{} of {}", style::thousands(self.shown.len()), style::thousands(self.actions.len()))
        };
        // (Under the search, its first menu's text over the list's first
        // column's, as Log Viewer has it.)
        let controls = search::header(cx, &self.search, format!("replay-{}", self.id_key), facets, width).no_clear().count(count).indent(search::list_indent(cx));

        // The screen as it was, the action under it, and the transport.
        let list_w = (width * 0.42).clamp(260.0, 440.0).min(width - 240.0).max(200.0);
        let right_w = width - list_w - m::SPACING;
        if self.playing {
            self.play_on(right_w, cx);
        }
        let want = self.frames_index.frame_time(self.playhead);
        if let Some(want_at) = self.frame_track.request_for(want) {
            // Unblock `make_frames` immediately if it is already waiting on a
            // superseded remote `frame_at`, then enqueue the new playhead.
            self.source.cancel_frame_at();
            let _ = self.frames_tx.send((self.frame_track.index_gen, want_at, self.playhead));
        }
        let text: Option<SharedString> = if want.is_none() {
            Some("No picture from then".into())
        } else if want == self.frame_track.missed {
            if let Some(reason) = &self.frame_track.missed_reason {
                Some(SharedString::from(reason.clone()))
            } else if !self.well.read(cx).has_shot() {
                Some("No picture from then".into())
            } else {
                None
            }
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
        let overlay = self.overlay(t, live, cx);
        let list = self.list_view(cx);
        let detail = self.detail();
        let transport = self.transport(right_w, cx);
        let discontiguous_note = self.export_discontiguous_blocked().then(|| {
            let note = SharedString::from("Update Olo on this host to export non-adjacent actions.");
            canvas(move |g, b| {
                let p = theme::palette();
                let r = style::label_fit(g, b.left(), b.center().y, style::Align::Left, note.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p), b.size.width);
                agent::record(g, agent::Node::label(note.clone(), r));
            })
            .w_full()
            .h(px(m::LABEL_ROW))
            .flex_shrink_0()
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(controls)
            .children(discontiguous_note)
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
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(m::SPACING)).child(div().flex_1().min_h_0().relative().child(&self.well).child(overlay)).child(detail).child(transport)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(at: u64, ms: u64, kind: &str, args: &[&str], result: Option<serde_json::Value>, error: Option<&str>) -> Action {
        Action { at, ms, kind: kind.into(), args: args.iter().map(|s| (*s).to_owned()).collect(), result, error: error.map(str::to_owned) }
    }

    fn filter(actions: &[Action], query_str: &str, now: u64) -> Vec<usize> {
        let query = Query::parse(query_str, &KEYS);
        actions
            .iter()
            .enumerate()
            .filter(|(_, a)| {
                let text = search_text(a);
                action_matches(a, &text, &query, now)
            })
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn filtering_by_kind_result_time_and_text() {
        let now = 1_750_000_000_000u64;
        let actions = vec![
            act(now - 2 * 3_600_000, 100, "start", &[], Some(serde_json::json!({"width": 1440, "height": 900})), None),
            act(now - 20 * 60_000, 50, "run", &["olo-chrome"], Some(serde_json::json!({"pid": 42})), None),
            act(now - 2 * 60_000, 30, "click", &["--name", "Save Changes"], Some(serde_json::json!({"matched": {"role": "Button", "name": "Save Changes", "center": [120.0, 80.0]}})), None),
            act(now - 60_000, 10, "click", &["--name", "Missing"], None, Some("nothing matches")),
            act(now - 10_000, 20, "type", &["hello world"], None, None),
            act(now - 5_000, 15, "code", &["123456"], None, None),
        ];

        assert_eq!(filter(&actions, "", now), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(filter(&actions, "kind:pointer", now), vec![2, 3]);
        assert_eq!(filter(&actions, "kind:keys", now), vec![4, 5]);
        assert_eq!(filter(&actions, "kind:run", now), vec![1]);
        assert_eq!(filter(&actions, "result:failed", now), vec![3]);
        assert_eq!(filter(&actions, "result:ok kind:pointer", now), vec![2]);
        assert_eq!(filter(&actions, "since:5m", now), vec![2, 3, 4, 5]);
        assert_eq!(filter(&actions, "since:1h", now), vec![1, 2, 3, 4, 5]);
        assert_eq!(filter(&actions, "save", now), vec![2]);
        assert_eq!(filter(&actions, "nothing matches", now), vec![3]);
    }

    #[test]
    fn command_and_outcome_formatting() {
        let a = act(
            1_000,
            40,
            "click",
            &["--name", "Save Changes"],
            Some(serde_json::json!({"matched": {"role": "Button", "name": "Save Changes", "center": [120.0, 80.0]}, "after": {"window": {"title": "Notes"}, "settled": false}})),
            None,
        );
        assert_eq!(command(&a), "click --name “Save Changes”");
        assert_eq!(outcome(&a).as_deref(), Some("Button \"Save Changes\" at 120,80 · then “Notes” in front · still changing"));

        let started = act(2_000, 10, "run", &["olo-chrome"], Some(serde_json::json!({"pid": 1234})), None);
        assert_eq!(outcome(&started).as_deref(), Some("started, pid 1234"));

        let failed = act(3_000, 5, "click", &[], None, Some("timed out"));
        assert_eq!(outcome(&failed).as_deref(), Some("timed out"));

        assert_eq!(duration(65_000), "1:05");
        assert_eq!(duration(3_665_000), "1:01:05");
    }

    #[test]
    fn astra_f18_late_and_prereset_frame_replies_are_discarded_and_miss_leaves_reading() {
        let mut tr = FrameTracker::default();
        // Display frame A (1000).
        assert_eq!(tr.request_for(Some(1_000)), Some(1_000));
        assert!(tr.accept_frame(0, 1_000, 1_000, Some(1_000)));

        // Scrub to B (2000), then immediately back to A (1000) while B is in flight.
        assert_eq!(tr.request_for(Some(2_000)), Some(2_000));
        assert_eq!(tr.request_for(Some(1_000)), None);
        // Late reply for B must NOT overwrite A!
        assert!(!tr.accept_frame(0, 2_000, 2_000, Some(1_000)));
        assert_eq!(tr.showing, Some(1_000));

        // History reset bumps generation: reply from gen 0 is rejected even if timestamp matches.
        tr.on_reset();
        assert_eq!(tr.request_for(Some(1_000)), Some(1_000));
        assert!(!tr.accept_frame(0, 1_000, 1_000, Some(1_000)));
        assert!(tr.accept_frame(1, 1_000, 995, Some(1_000)));
        assert_eq!(tr.showing_actual, Some(995));

        // Terminal miss on C (3000) marks `missed` so UI leaves "Reading…".
        assert_eq!(tr.request_for(Some(3_000)), Some(3_000));
        assert!(tr.record_miss(1, 3_000, Some(3_000), None));
        assert_eq!(tr.missed, Some(3_000));
        assert_eq!(tr.missed_reason, None);
        assert_eq!(tr.request_for(Some(3_000)), None);

        // Per-request FrameError records exact error words, clears stale `showing`, and does NOT retry.
        assert_eq!(tr.request_for(Some(4_000)), Some(4_000));
        assert!(tr.record_miss(1, 4_000, Some(4_000), Some("frame exceeds wire limit".into())));
        assert_eq!(tr.missed, Some(4_000));
        assert_eq!(tr.missed_reason.as_deref(), Some("frame exceeds wire limit"));
        assert_eq!(tr.showing, None);
        assert_eq!(tr.request_for(Some(4_000)), None, "FrameError must not trigger retry loop");
        // Scrubbing to another valid frame clears the error and requests the new timestamp.
        assert_eq!(tr.request_for(Some(5_000)), Some(5_000));
        assert_eq!(tr.missed, None);
        assert_eq!(tr.missed_reason, None);
    }

    #[test]
    fn oversized_action_placeholder_preserves_index_and_monotonic_times() {
        let oversized = Action { at: 0, ms: 0, kind: "oversized".into(), args: vec![], result: None, error: None };
        assert!(oversized.is_oversized());
        assert_eq!(command(&oversized), "Action too large to show");
        assert!(search_text(&oversized).contains("Action too large to show"));
        assert!(search_text(&oversized).contains("oversized"));

        let actions = vec![oversized.clone(), act(1_000, 20, "click", &["--name", "Save"], None, None), oversized.clone(), act(3_000, 10, "type", &["hello"], None, None)];
        let times = effective_action_times(&actions, None);
        assert_eq!(times, vec![1_000, 1_000, 1_000, 3_000], "times must stay monotonic without dragging bounds to 0");
        // Searching for "oversized" matches indices 0 and 2 directly, keeping exact 0-based indices.
        assert_eq!(filter(&actions, "oversized", 5_000), vec![0, 2]);
        assert_eq!(filter(&actions, "hello", 5_000), vec![3]);
    }

    #[test]
    fn review2_f12_and_f40_pending_history_coalesces_and_preserves_index_reset() {
        let mut pending = PendingHistory::default();
        let idx1 = Arc::new(FrameIndex { frames: vec![olo_screen_codec::FrameMark { at: 1_000, key: true }], last_size: Some((640, 480)) });
        let idx2 = Arc::new(FrameIndex { frames: vec![olo_screen_codec::FrameMark { at: 1_000, key: true }], last_size: Some((640, 480)) });
        let a1 = act(1_000, 10, "click", &[], None, None);
        let a2 = act(2_000, 10, "type", &["hi"], None, None);
        pending.push_loaded(vec![a1.clone()], vec![search_text(&a1)], false, true, idx1);
        pending.push_loaded(vec![a2.clone()], vec![search_text(&a2)], false, false, idx2.clone());
        let drained = pending.take();
        assert!(drained.has_loaded);
        assert!(drained.index_reset, "coalesced batch must preserve index_reset from earlier replacement");
        assert_eq!(drained.actions.len(), 2);
        assert!(Arc::ptr_eq(drained.frames.as_ref().unwrap(), &idx2), "coalesced batch must retain only the latest Arc<FrameIndex>");
    }

    #[test]
    fn astra_f26_mismatched_loaded_batch_is_rejected() {
        let actions = vec![act(1_000, 10, "click", &[], None, None)];
        assert!(validate_loaded_batch(&actions, &[]).is_err());
        let texts = vec![search_text(&actions[0])];
        assert!(validate_loaded_batch(&actions, &texts).is_ok());
    }
}
