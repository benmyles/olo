//! [`ScreenPane`]: the chosen screen under a header saying what it is and how
//! it's doing, in four tabs: Live (the screen as it is, with the agent's
//! pointer, and Take Control to step in: the person's pointer and keys over
//! the picture then go to that screen, never to their own, until they choose
//! Stop Controlling), History (`replay`: its actions as they come, to search,
//! filter and play back), Stats (`stats`) and Passwords (`shares`: what of
//! the person's Passwords the screen has, besides or without what every
//! screen has, Screen ▸ Passwords for Every Screen…; or a plain explanation
//! for remote host screens until Step 8).
//!
//! Shared between `apps/screen` (Agent Screens) and `apps/rex` (Rex's
//! Activity sidebar). All background work delivers messages through
//! `cx.messenger()`, so host windows need no message-proxy forwarding and
//! only subscribe to [`ScreenPaneEvent`].

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use olo_capture::live::XRGB8888;
use olo_kit::{
    Bounds, Child, Cx, Element, ExternalBuffer, FocusHandle, FontWeight, Hsla, InteractiveElement as _, IntoElement as _, KeyDownEvent, KeyUpEvent, Keystroke, Menu, Messenger, Modifiers,
    ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Painter, ParentElement as _, PathBuilder, Pixels, Point, ScrollDelta, ScrollHandle, ScrollWheelEvent,
    SharedString, Styled as _, TextRun, View, WindowCx, agent, canvas, div, file_panel, icons, keys, metrics as m, point, px, size,
    style::{self, c},
    theme,
    widgets::{Button, DISABLED_OPACITY, TabView, TextControl, TextField, TextFieldEvent, open_alert},
};
use olo_protocol::SyntheticEvent;
use olo_sync::Mutex;
use serde::{Deserialize, Serialize};

use crate::{
    export, history,
    picture::{PictureWell, Shot},
    replay::{self, Replay, ReplayUpdated},
    screens,
    shares::{self, Shares},
    source::{ExportCancel, LiveEvent, LiveHandle, LiveStatus, PasswordsMode, ScreenId, ScreenInfo, ScreenLocation, ScreenSource, ScreenState},
    stats,
};

pub const HEADER_H: f32 = 40.0;

/// Shared header bar layout used by both [`ScreenPane::header`] and host
/// headers so icon, title, subtitle, and right-aligned action buttons always
/// sit on the exact same vertical coordinates (no 2 pt jump between views).
pub fn header_bar(face: Element, buttons: impl IntoIterator<Item = Element>) -> Element {
    div().flex().flex_row().items_center().gap(px(m::SPACING)).h(px(HEADER_H)).flex_shrink_0().child(face).children(buttons).into_any_element()
}

/// Capture `(expected_started, expected_last_used)` by value when opening a
/// Stop confirmation dialog, shared by [`ScreenPane::ask_stop`] and
/// `apps/screen`'s Stop path (Final pass B.3 / Astra #4).
///
/// Sends `Some(0)` for a screen known to have done nothing (`last_used == None`
/// and not known to have `actions > 0`), and fails closed for a remote running
/// screen whose `started` generation is unknown.
pub fn capture_stop_expectations(
    is_local: bool,
    started: Option<u64>,
    replay_last_used: Option<u64>,
    info_last_used: Option<u64>,
    actions: Option<usize>,
) -> Result<(Option<u64>, Option<u64>), String> {
    if !is_local && started.is_none() {
        return Err("Couldn’t confirm which running session to stop because its start time isn’t known yet.".into());
    }
    let observed = match (replay_last_used, info_last_used) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    let expected_last_used = match observed {
        Some(ts) => Some(ts),
        None if actions.is_none_or(|n| n == 0) => Some(0),
        None => None,
    };
    Ok((started, expected_last_used))
}
/// The field a screen's name is changed in.
const RENAME_W: f32 = 320.0;
/// The modifiers the screen is kept in step with: Ctrl, Shift, Alt.
const MODIFIERS: [u32; 3] = [29, 42, 56];

/// The size an export was last made at (each pixel this many times over).
static LAST_ZOOM: AtomicUsize = AtomicUsize::new(1);
/// And a screenshot.
static LAST_SHOT_ZOOM: AtomicUsize = AtomicUsize::new(1);
/// Tells a live watch's frames from an earlier watch's.
static WATCHES: AtomicU64 = AtomicU64::new(1);

/// The evdev code of the key the kit names `key` ("a", "f3", "!"): its place
/// on a US keyboard (the agent's), Shift aside.
fn evdev(key: &str) -> Option<u32> {
    match key {
        "insert" => return Some(110),
        "space" => return Some(57),
        _ => {}
    }
    match keys::evdev(key) {
        0 => {
            let mut chars = key.chars();
            let (Some(ch), None) = (chars.next(), chars.next()) else { return None };
            screens::key_for(ch).map(|(code, _)| code)
        }
        code => Some(code),
    }
}

/// A wheel's notches one way or the other: 1, −1, or none (`signum`
/// makes 0 one).
fn notch(v: f32) -> i32 {
    if v > 0.0 {
        1
    } else if v < 0.0 {
        -1
    } else {
        0
    }
}

/// Whether `k` matches the resolved Take Control / Stop Controlling shortcut
/// (`screen.control`, ⌘K by default), preserving keyboard-profile overrides
/// without calling `olo_kit::menu::shortcut_command` (which would re-enter the
/// host window's `MenuBar::of` callback and panic on `self.pane.read(cx)` while
/// [`ScreenPane`] is borrowed for key delivery).
fn is_control_shortcut(k: &Keystroke, raw: bool) -> bool {
    let mut menus = [Menu::new("").item("", "control").key("k")];
    olo_kit::menu::resolve(&mut menus, raw);
    olo_kit::menu::find_shortcut(&menus, k).as_deref() == Some("control")
}

/// The chosen screen's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Tab {
    #[default]
    Live,
    History,
    Stats,
    Passwords,
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Live => "Live",
            Tab::History => "History",
            Tab::Stats => "Stats",
            Tab::Passwords => "Passwords",
        }
    }
}

/// Snapshot of menu-item enablement and checked state for a host's menu bar.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScreenMenuState {
    pub has_screen: bool,
    pub running: bool,
    pub shown_tab: Tab,
    pub has_history: bool,
    pub playing: bool,
    pub can_export: bool,
    pub can_screenshot: bool,
    pub can_clear: bool,
    pub can_control: bool,
    pub controlling: bool,
    pub can_move_windows: bool,
    pub giving: bool,
    pub busy: bool,
    pub can_airplay: bool,
}

/// Events emitted by [`ScreenPane`] via `cx.emit` for its host window.
#[derive(Debug, Clone)]
pub enum ScreenPaneEvent {
    /// Host with multi-selection / shared start guard should start the screen
    /// (emitted when [`ScreenPane::set_host_lifecycle`] is enabled).
    StartRequested(ScreenId),
    /// Host with multi-selection should confirm & stop the screen (emitted
    /// when [`ScreenPane::set_host_lifecycle`] is enabled).
    StopRequested(ScreenId),
    /// A stopped screen was started from the pane.
    Started(ScreenInfo),
    /// A running screen was stopped from the pane.
    Stopped(ScreenId),
    /// A stopped screen's history was cleared from the pane.
    Cleared(ScreenId),
    /// The screen's display title changed optimistically via inline rename.
    Renamed(ScreenId, Option<String>),
    /// Persisting a rename failed; host should refresh its list.
    RenameFailed(ScreenId),
    /// New action activity was observed in the screen's history (`last_used_ms`).
    Used(ScreenId, u64),
    /// Take Control was entered (`true`) or released (`false`).
    Controlling(ScreenId, bool),
    /// Active tab changed.
    TabChanged(Tab),
    /// User asked to open/focus a remote host in Rex (`Show in Rex`).
    OpenHost(String),
}

/// Draw a snapped filled semantic status dot (used identically in the pane
/// header and multi-host sidebar).
pub fn paint_status_dot(g: &mut Painter, center: Point<Pixels>, diameter_pt: f32, color: Hsla) {
    let d = style::snap_centred(g, center, diameter_pt, diameter_pt);
    g.paint_quad(olo_kit::fill(d, color).corner_radii(d.size.width / 2.0));
}

/// Draw a snapped hollow 1 pt semantic status ring for a disconnected host's
/// screen (kept as a ring when selected and in the pane header). Uses the exact
/// same `snap_centred` outer box and rounded-quad SDF shader as
/// [`paint_status_dot`] with an inset 1 pt stroke so outer diameter and center
/// match the solid dot to the device pixel at every scale.
pub fn paint_status_ring(g: &mut Painter, center: Point<Pixels>, diameter_pt: f32, color: Hsla) {
    let d = style::snap_centred(g, center, diameter_pt, diameter_pt);
    let scale = g.scale_factor();
    let stroke = px((1.0 * scale).round().max(1.0) / scale);
    g.paint_quad(olo_kit::outline(d, color, olo_kit::BorderStyle::Solid).border_widths(stroke).corner_radii(d.size.width / 2.0));
}

/// Tertiary ink guaranteed >= 3.2:1 contrast against `bg` in both light and
/// dark modes (used for disconnected host rings and labels).
pub fn tertiary_ink(p: &olo_theme::Palette, bg: olo_theme::Rgba) -> Hsla {
    let raw = olo_theme::mix(p.panel_text, bg, 0.50);
    c(olo_theme::readable(raw, bg, 3.2))
}

/// Left-anchored BeOS diagonal barber-pole indeterminate progress bar inside a
/// closed 11 pt recessed trough, filled with neutral slate (`style::neutral_bar`)
/// and bounded by a four-sided border legible in both light and dark modes.
fn barber_bar(width_pt: f32, height_pt: f32, label: SharedString) -> Element {
    canvas(move |g, b| {
        let p = theme::palette();
        let snapped = style::snap(g, b);
        let scale = g.scale_factor();
        let border = px((1.0 * scale).round().max(1.0) / scale);
        let inner = Bounds::from_corners(point(snapped.left() + border, snapped.top() + border), point(snapped.right() - border, snapped.bottom() - border));
        let dark = p.mode == olo_theme::ThemeMode::Dark;
        let well_bg = p.shade(p.panel_background, 1.18);
        let track_bg = if dark { c(olo_theme::mix(p.panel_background, well_bg, 0.45)) } else { c(p.document_background) };
        let fill_c = style::neutral_bar(&p);
        style::fill_rect(g, inner, track_bg);
        g.with_content_mask(Some(olo_kit::ContentMask { bounds: inner }), |g| {
            let h = f32::from(inner.size.height);
            let w = f32::from(inner.size.width);
            let pitch = 10.0f32;
            let stripe_w = 5.0f32;
            let mut x = -h;
            while x < w + h {
                let mut poly = PathBuilder::fill();
                let p0 = point(inner.left() + px(x), inner.bottom());
                let p1 = point(inner.left() + px(x + stripe_w), inner.bottom());
                let p2 = point(inner.left() + px(x + stripe_w + h), inner.top());
                let p3 = point(inner.left() + px(x + h), inner.top());
                poly.add_polygon(&[p0, p1, p2, p3], true);
                if let Ok(built) = poly.build() {
                    g.paint_path(built, fill_c);
                }
                x += pitch;
            }
        });
        // Closed four-sided border legible against both the dark well and light well.
        let frame_rgba = if dark { olo_theme::readable(p.shade(well_bg, 1.55), well_bg, 2.2) } else { olo_theme::readable(p.shade(well_bg, 1.45), well_bg, 2.2) };
        g.paint_quad(olo_kit::outline(snapped, c(frame_rgba), olo_kit::BorderStyle::Solid).border_widths(border));
        g.agent(agent::Node { role: "ProgressBar", name: Some(label.clone()), value: Some("busy".into()), bounds: snapped, enabled: true, ..Default::default() });
    })
    .w(px(width_pt))
    .h(px(height_pt))
    .flex_shrink_0()
    .into_any_element()
}

/// Monotonic per-screenshot ID so each active screenshot request owns its own
/// [`ExportCancel`] handle independent of scrubbing or other saves.
static SHOTS: AtomicU64 = AtomicU64::new(1);

struct ActiveExport {
    done: f32,
    cancel: ExportCancel,
    _path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlAcquire {
    Noop,
    Acquired,
    Pending,
}

type DetachedControl = (bool, Option<(Arc<dyn ScreenSource>, Vec<SyntheticEvent>)>);

/// Pure, Wayland-free state machine driving [`ScreenPane`] — owns screen
/// selection, tab fallback, Take Control locking, keyboard/modifier/pointer
/// translation, pane-scoped exports, and header/status text formatting so all
/// pane logic can be unit-tested directly against a mock [`ScreenSource`].
struct PaneState {
    source: Option<Arc<dyn ScreenSource>>,
    info: Option<ScreenInfo>,
    /// Monotonic counter bumped on every `set_source` change and `update_info`
    /// so delayed Rename RPC replies never overwrite newer Follow metadata (B.8).
    info_rev: u64,
    tab: Tab,
    controlling: bool,
    /// True while waiting for an asynchronous remote host to confirm Take
    /// Control (neither `controlling` nor `Tab::Live` commits until granted).
    control_pending: bool,
    /// The screen it said the person controls (or requested control of).
    controlled_source: Option<Arc<dyn ScreenSource>>,
    /// Its clipboard and the person's as one, while they control it.
    joined_clipboard: Option<Box<dyn Send>>,
    /// Modifiers held down on the screen while controlling, and keys.
    held: [bool; 3],
    down: Vec<(String, u32)>,
    /// A button held on the screen (a drag goes on outside the picture).
    pressed: HashSet<u32>,
    /// Current pixel size of the live picture (updated on every live frame so
    /// pointer mapping follows remote screen resizes without stale metadata).
    live_pixel_size: Option<(usize, usize)>,
    /// Whether the pane's host window/container is currently visible (when
    /// false, the live stream is paused so nothing is captured off-screen).
    visible: bool,
    live_status: LiveStatus,
    /// Host-reported reason why control was refused or released (shown beside
    /// Take Control where `control_disabled_reason` also lives).
    control_note: Option<String>,
    failure: Option<String>,
    busy: bool,
    /// Active video exports keyed by screen ID so exports survive switching
    /// away from a screen, block duplicate exports, and reconnect progress/Cancel
    /// when returning to that screen.
    exports: HashMap<ScreenId, ActiveExport>,
    /// Per-request cancellation handles for in-flight screenshot workers so
    /// closing the pane cancels every pending save immediately.
    shot_cancels: HashMap<u64, ExportCancel>,
    /// The person's display-workspaces (`PaneMsg::Spaces`), and a screen's
    /// windows being given to one.
    spaces: Vec<(String, u32, String)>,
    giving: bool,
}

impl PaneState {
    fn new(tab: Tab) -> PaneState {
        PaneState {
            source: None,
            info: None,
            info_rev: 0,
            tab,
            controlling: false,
            control_pending: false,
            controlled_source: None,
            joined_clipboard: None,
            held: [false; 3],
            down: Vec::new(),
            pressed: HashSet::new(),
            live_pixel_size: None,
            visible: true,
            live_status: LiveStatus::Connecting,
            control_note: None,
            failure: None,
            busy: false,
            exports: HashMap::new(),
            shot_cancels: HashMap::new(),
            spaces: Vec::new(),
            giving: false,
        }
    }

    fn source(&self) -> Option<&Arc<dyn ScreenSource>> {
        self.source.as_ref()
    }

    fn info(&self) -> Option<&ScreenInfo> {
        self.info.as_ref()
    }

    fn screen_id(&self) -> Option<&ScreenId> {
        self.info.as_ref().map(|i| &i.id)
    }

    fn is_running(&self) -> bool {
        self.info.as_ref().is_some_and(ScreenInfo::is_running)
    }

    fn tab(&self) -> Tab {
        self.tab
    }

    /// The tab showing: Live only for a running screen.
    fn shown_tab(&self) -> Tab {
        if self.tab == Tab::Live && !self.is_running() { Tab::History } else { self.tab }
    }

    fn tabs(&self) -> Vec<Tab> {
        if self.is_running() { vec![Tab::Live, Tab::History, Tab::Stats, Tab::Passwords] } else { vec![Tab::History, Tab::Stats, Tab::Passwords] }
    }

    fn controlling(&self) -> bool {
        self.controlling
    }

    fn busy(&self) -> bool {
        self.busy
    }

    fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    fn set_source(&mut self, source: Option<Arc<dyn ScreenSource>>) -> bool {
        // Tear down old bindings whenever the `ScreenSource` instance itself
        // changes (even for the same `ScreenId`) so control/input/watch never
        // straddle two source instances; metadata-only refreshes use `update_info`.
        let same_instance = match (&self.source, &source) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        let changed = !same_instance;
        if changed {
            self.info_rev = self.info_rev.wrapping_add(1);
            self.stop_controlling();
            self.live_pixel_size = None;
            self.failure = None;
            self.control_note = None;
            self.live_status = LiveStatus::Connecting;
            self.busy = false;
        }
        self.info = source.as_ref().map(|s| s.info());
        self.source = source;
        if !self.is_running() || !self.info.as_ref().is_some_and(|i| i.capabilities.can_control) {
            self.stop_controlling();
        }
        changed
    }

    fn update_info(&mut self, info: ScreenInfo) -> bool {
        if self.info.as_ref().is_none_or(|cur| cur.id != info.id) {
            return false;
        }
        let was_disconnected = self.info.as_ref().is_some_and(|cur| cur.capabilities.host_disconnected);
        let stopped = !info.is_running();
        let disconnected = info.capabilities.host_disconnected;
        let lost_control_cap = !info.capabilities.can_control;
        self.info_rev = self.info_rev.wrapping_add(1);
        self.info = Some(info);
        if stopped || disconnected || lost_control_cap {
            self.stop_controlling();
        }
        if was_disconnected && !disconnected {
            // Host came back: clear stale error/disconnected state so the live
            // stream opens cleanly with the loading barber-pole view.
            self.failure = None;
            self.control_note = None;
            self.live_status = LiveStatus::Connecting;
        }
        true
    }

    fn set_tab(&mut self, tab: Tab) -> bool {
        let released = if tab != Tab::Live { self.stop_controlling() } else { false };
        self.tab = tab;
        released
    }

    fn set_visible(&mut self, visible: bool) -> bool {
        let released = if !visible { self.stop_controlling() } else { false };
        self.visible = visible;
        released
    }

    fn running_screen_id(&self) -> Option<ScreenId> {
        self.is_running().then(|| self.info.as_ref().map(|i| i.id.clone())).flatten()
    }

    fn should_watch_live(&self) -> Option<ScreenId> {
        (self.visible && self.shown_tab() == Tab::Live && self.is_running() && !self.info.as_ref().is_some_and(|i| i.capabilities.host_disconnected))
            .then(|| self.info.as_ref().map(|i| i.id.clone()))
            .flatten()
    }

    /// Whether the current screen's history follower should be open/active.
    /// Local screens follow eagerly (zero-cost `inotify`, feeding the Live
    /// header and status bar); lazy remote screens open `screen history` only
    /// while History or Stats is shown or an export runs for that screen.
    fn should_follow_history(&self) -> bool {
        let Some(src) = &self.source else { return false };
        if self.info.as_ref().is_some_and(|i| i.capabilities.host_disconnected) {
            return false;
        }
        if !src.lazy_history() {
            return true;
        }
        (self.visible && matches!(self.shown_tab(), Tab::History | Tab::Stats)) || self.is_exporting_current()
    }

    /// Record the latest live picture's pixel dimensions and refresh `info.size`
    /// so pointer coordinate mapping tracks remote screen resizes dynamically.
    fn on_live_frame_size(&mut self, px_w: usize, px_h: usize) {
        if px_w == 0 || px_h == 0 {
            return;
        }
        self.live_pixel_size = Some((px_w, px_h));
        if let Some(info) = self.info.as_mut() {
            let scale = info.scale.max(0.5);
            info.size = Some(((px_w as f64 / scale).round() as i32, (px_h as f64 / scale).round() as i32));
        }
    }

    /// Logical screen size used to map live pointer coordinates into the
    /// remote/local screen's input space.
    fn live_logical_size(&self) -> (f64, f64) {
        if let Some((pw, ph)) = self.live_pixel_size {
            let scale = self.info.as_ref().map_or(1.0, |i| i.scale.max(0.5));
            return (pw as f64 / scale, ph as f64 / scale);
        }
        self.info.as_ref().and_then(|i| i.size).map_or((1440.0, 900.0), |(w, h)| (w as f64, h as f64))
    }

    fn is_exporting_current(&self) -> bool {
        self.screen_id().is_some_and(|id| self.exports.contains_key(id))
    }

    fn export_for_current(&self) -> Option<(f32, ExportCancel)> {
        let id = self.screen_id()?;
        self.exports.get(id).map(|e| (e.done, e.cancel.clone()))
    }

    fn export_done_current(&self) -> Option<f32> {
        let id = self.screen_id()?;
        self.exports.get(id).map(|e| e.done)
    }

    fn begin_export(&mut self, id: ScreenId, path: PathBuf) -> Option<ExportCancel> {
        if self.exports.contains_key(&id) {
            return None;
        }
        let cancel = ExportCancel::new();
        self.exports.insert(id, ActiveExport { done: 0.0, cancel: cancel.clone(), _path: path });
        Some(cancel)
    }

    fn register_shot(&mut self) -> (u64, ExportCancel) {
        let id = SHOTS.fetch_add(1, Ordering::Relaxed);
        let cancel = ExportCancel::new();
        self.shot_cancels.insert(id, cancel.clone());
        (id, cancel)
    }

    fn finish_shot(&mut self, id: u64) {
        self.shot_cancels.remove(&id);
    }

    fn cancel_all_shots(&mut self) {
        for (_, cancel) in self.shot_cancels.drain() {
            cancel.cancel();
        }
    }

    fn update_export_progress(&mut self, id: &ScreenId, done: f32) -> bool {
        if let Some(ex) = self.exports.get_mut(id) {
            ex.done = done;
        }
        self.screen_id() == Some(id)
    }

    fn finish_export(&mut self, id: &ScreenId) -> bool {
        self.exports.remove(id);
        self.screen_id() == Some(id)
    }

    /// Apply a completed `start()` reply guarded by originating screen ID so a
    /// late reply for screen A never alters screen B's busy state or active tab.
    fn on_started_reply(&mut self, orig_id: &ScreenId, info: Option<ScreenInfo>) -> bool {
        if self.screen_id() != Some(orig_id) {
            return false;
        }
        self.busy = false;
        if let Some(info) = info {
            self.update_info(info);
            self.tab = Tab::Live;
        }
        true
    }

    /// Apply a completed `stop()` reply for `id`, releasing control if the user
    /// retook control of `id` while stop was in flight. Returns `true` if
    /// control was released.
    fn on_stopped_reply(&mut self, id: &ScreenId) -> bool {
        let Some(mut info) = self.info.clone() else { return false };
        if &info.id != id {
            return false;
        }
        let was_controlling = self.controlling;
        info.state = ScreenState::Stopped;
        info.controlled = false;
        self.update_info(info);
        was_controlling && !self.controlling
    }

    fn take_control(&mut self) -> Result<ControlAcquire, String> {
        if !self.visible || self.controlling || self.control_pending {
            return Ok(ControlAcquire::Noop);
        }
        let Some(info) = &self.info else { return Ok(ControlAcquire::Noop) };
        if !info.is_running() || !info.capabilities.can_control || info.capabilities.host_disconnected {
            return Ok(ControlAcquire::Noop);
        }
        let Some(source) = self.source.clone() else { return Ok(ControlAcquire::Noop) };
        self.control_note = None;
        // Acquire control BEFORE committing the switch to Tab::Live so a
        // refused/failed lock leaves the previous tab untouched.
        // (Agents' actions on it wait meanwhile: olo-screen reads it.)
        source.set_controlled(true)?;
        if source.async_control() {
            // Remote host grants control asynchronously via `LiveEvent::Control { held: true }`:
            // keep the current tab and do not intercept keys until granted.
            self.control_pending = true;
            self.controlled_source = Some(source);
            return Ok(ControlAcquire::Pending);
        }
        self.tab = Tab::Live;
        self.controlling = true;
        self.controlled_source = Some(source.clone());
        self.joined_clipboard = source.join_clipboard();
        Ok(ControlAcquire::Acquired)
    }

    /// Update live stream status; losing or reconnecting the stream explicitly
    /// releases control on the source so neither a local fallback error nor a
    /// remote compositor restart can leave the backend holding `.control`.
    fn on_live_status(&mut self, status: LiveStatus) -> bool {
        let lost = matches!(status, LiveStatus::Disconnected(_) | LiveStatus::Reconnecting);
        self.live_status = status;
        if lost { self.stop_controlling() } else { false }
    }

    /// Record a live stream failure and explicitly release control on the source.
    fn on_live_failed(&mut self, message: String) -> bool {
        self.failure = Some(message);
        self.stop_controlling()
    }

    /// Host granted, refused, or dropped control asynchronously.
    /// Returns `Some(true)` if control was just granted, `Some(false)` if active
    /// control was just lost, or `None` if active control state did not change.
    fn on_remote_control(&mut self, held: bool, reason: Option<String>) -> Option<bool> {
        if held {
            self.control_note = None;
            if self.controlling {
                return None;
            }
            let same_source = match (&self.source, &self.controlled_source) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            };
            if self.control_pending && same_source && self.is_running() && self.visible {
                self.control_pending = false;
                self.controlling = true;
                self.tab = Tab::Live;
                if let Some(source) = &self.controlled_source {
                    self.joined_clipboard = source.join_clipboard();
                }
                return Some(true);
            }
            // Grant arrived after the user cancelled/switched/hid: release it
            // back on the host immediately so agents aren't locked out.
            self.control_pending = false;
            if let Some(source) = self.controlled_source.take().or_else(|| self.source.clone()) {
                let _ = source.release_control(Vec::new());
            }
            return None;
        }
        if reason.is_some() {
            self.control_note = reason;
        }
        let was_controlling = self.controlling;
        self.control_pending = false;
        self.controlling = false;
        self.controlled_source = None;
        self.joined_clipboard = None;
        self.down.clear();
        self.pressed.clear();
        self.held = [false; 3];
        was_controlling.then_some(false)
    }

    /// Whether the pane is currently bound to the EXACT screen source and
    /// session generation captured when a Stop dialog opened (Final pass B.2 /
    /// Astra #5, #24: a stale Stop dialog must never release control on a
    /// replacement session under the same screen name).
    fn matches_captured_session(&self, id: &ScreenId, source: &Arc<dyn ScreenSource>, expected_started: Option<u64>) -> bool {
        self.screen_id() == Some(id) && self.source.as_ref().is_some_and(|cur| Arc::ptr_eq(cur, source) && (expected_started.is_none() || cur.session_started() == expected_started))
    }

    /// Drain held keys/buttons and detach the controlled source so a worker
    /// thread (such as `ask_stop`) can run `release_control_and_wait` before
    /// executing a command that requires `.control` to be gone.
    fn detach_control_for_worker(&mut self) -> DetachedControl {
        if !self.controlling && !self.control_pending {
            return (false, None);
        }
        let was_controlling = self.controlling;
        let mut up: Vec<SyntheticEvent> = self.down.drain(..).map(|(_, key)| SyntheticEvent::Key { key, pressed: false }).collect();
        up.extend(MODIFIERS.iter().zip(self.held).filter(|(_, held)| *held).map(|(&key, _)| SyntheticEvent::Key { key, pressed: false }));
        up.extend(self.pressed.drain().map(|button| SyntheticEvent::Button { button, pressed: false }));
        let source = self.controlled_source.take();
        self.held = [false; 3];
        self.controlling = false;
        self.control_pending = false;
        self.joined_clipboard = None;
        (was_controlling, source.map(|s| (s, up)))
    }

    fn stop_controlling(&mut self) -> bool {
        let (was_controlling, pending) = self.detach_control_for_worker();
        if let Some((source, up)) = pending {
            let _ = source.release_control(up);
        }
        was_controlling
    }

    /// Send `events` to the screen being controlled.
    fn send_input(&self, events: Vec<SyntheticEvent>) {
        if !self.controlling || events.is_empty() {
            return;
        }
        if let Some(source) = &self.source {
            source.send_input(events);
        }
    }

    /// Press or let go of Ctrl, Shift and Alt on the screen to match
    /// `modifiers`.
    fn sync_modifiers(&mut self, modifiers: &Modifiers, out: &mut Vec<SyntheticEvent>) {
        let now = [modifiers.control, modifiers.shift, modifiers.alt];
        for (i, key) in MODIFIERS.into_iter().enumerate() {
            if now[i] != self.held[i] {
                out.push(SyntheticEvent::Key { key, pressed: now[i] });
                self.held[i] = now[i];
            }
        }
    }

    fn handle_key(&mut self, k: &Keystroke, pressed: bool, held: bool) -> bool {
        if !self.controlling {
            return false;
        }
        // (The screen repeats a key held itself.)
        if held {
            return true;
        }
        let mut out = Vec::new();
        self.sync_modifiers(&k.modifiers, &mut out);
        // A character that needs Shift on the agent's keyboard come
        // without it (typed by an input method, a virtual keyboard):
        // typed there with Shift, at once.
        let mut chars = k.key.chars();
        let shifted = match (chars.next(), chars.next()) {
            (Some(ch), None) => screens::key_for(ch).is_some_and(|(_, shift)| shift) && !k.modifiers.shift,
            _ => false,
        };
        if shifted {
            if pressed && let Some(code) = evdev(&k.key) {
                out.extend([(42, true), (code, true), (code, false), (42, false)].map(|(key, pressed)| SyntheticEvent::Key { key, pressed }));
            }
        } else if pressed {
            if let Some(code) = evdev(&k.key) {
                self.down.push((k.key.clone(), code));
                out.push(SyntheticEvent::Key { key: code, pressed: true });
            }
        } else if let Some(i) = self.down.iter().position(|(name, _)| *name == k.key).or_else(|| evdev(&k.key).and_then(|code| self.down.iter().position(|(_, c)| *c == code))) {
            let (_, code) = self.down.remove(i);
            out.push(SyntheticEvent::Key { key: code, pressed: false });
        }
        self.send_input(out);
        true
    }

    fn handle_modifiers(&mut self, modifiers: &Modifiers) -> bool {
        if !self.controlling {
            return false;
        }
        let mut out = Vec::new();
        self.sync_modifiers(modifiers, &mut out);
        self.send_input(out);
        true
    }

    fn handle_pointer_move(&mut self, x: f64, y: f64) {
        if self.controlling {
            self.send_input(vec![SyntheticEvent::Move { x, y }]);
        }
    }

    fn handle_pointer_button(&mut self, x: f64, y: f64, button: u32, pressed: bool) -> bool {
        if !self.controlling {
            return false;
        }
        if pressed {
            self.pressed.insert(button);
            self.send_input(vec![SyntheticEvent::Move { x, y }, SyntheticEvent::Button { button, pressed: true }]);
            true
        } else if self.pressed.remove(&button) {
            self.send_input(vec![SyntheticEvent::Move { x, y }, SyntheticEvent::Button { button, pressed: false }]);
            true
        } else {
            false
        }
    }

    fn handle_scroll(&mut self, x: f64, y: f64, delta: ScrollDelta) -> bool {
        if !self.controlling {
            return false;
        }
        let scroll = match delta {
            ScrollDelta::Pixels(d) => SyntheticEvent::Scroll { dx: -f64::from(f32::from(d.x)), dy: -f64::from(f32::from(d.y)), at_us: None },
            ScrollDelta::Lines(d) => SyntheticEvent::Wheel { dx: -notch(d.x), dy: -notch(d.y) },
        };
        self.send_input(vec![SyntheticEvent::Move { x, y }, scroll]);
        true
    }

    fn normalize_rename(&self, typed: &str) -> Option<String> {
        let name = self.info.as_ref()?.id.name.as_str();
        let trimmed = typed.trim();
        (!trimmed.is_empty() && trimmed != name).then(|| trimmed.to_owned())
    }

    fn header_subtitle_parts(&self, actions: usize, last_action_ms: Option<u64>, replay_started_ms: Option<u64>, now_ms: u64) -> Vec<String> {
        let Some(info) = &self.info else { return Vec::new() };
        let mut parts = Vec::new();
        let location_phrase = match &info.id.location {
            ScreenLocation::Host { host_label, .. } => Some(format!("on {host_label}")),
            ScreenLocation::Local if info.capabilities.show_location => Some("on this computer".to_owned()),
            ScreenLocation::Local => None,
        };
        if info.capabilities.host_disconnected {
            let state_word = match info.state {
                ScreenState::Running => "Last seen running",
                ScreenState::Archived => "Last seen archived",
                ScreenState::Stopped => "Last seen stopped",
            };
            let base = match &location_phrase {
                Some(loc) => format!("{state_word} {loc}"),
                None => state_word.to_owned(),
            };
            let when = info.disconnected_at_ms.or(last_action_ms).or(info.last_used_ms).map(|t| stats::ago(t, now_ms));
            // Single unbroken sentence ("Last seen running on build-farm 26 min ago") — no dot inside it.
            parts.push(match when {
                Some(ago) => format!("{base} {ago}"),
                None => base,
            });
            if let Some((w, h)) = info.size {
                parts.push(format!("{w} × {h}"));
            }
            return parts;
        }
        // Unified order (Opus 2 / Gemini 2 / Opus 12):
        // "Running [on X] · up T · N actions · W × H" so W × H drops first
        // when room runs out, "Running [on X]" never drops, and "last …" is
        // omitted (the Live footer already displays it).
        let action_count = actions.max(info.actions.unwrap_or(0));
        if info.is_running() {
            parts.push(match &location_phrase {
                Some(loc) => format!("Running {loc}"),
                None => "Running".to_owned(),
            });
            if let Some(t) = replay_started_ms.or(info.started_ms)
                && now_ms >= t
            {
                parts.push(olo_kit::ago::age(Duration::from_millis(now_ms - t), |a| format!("up {}", stats::span(a.as_millis() as u64))));
            }
        } else {
            let state_word = match info.state {
                ScreenState::Archived => "Archived",
                ScreenState::Stopped | ScreenState::Running => "Stopped",
            };
            parts.push(match &location_phrase {
                Some(loc) => format!("{state_word} {loc}"),
                None => state_word.to_owned(),
            });
        }
        if action_count > 0 {
            parts.push(format!("{} action{}", style::thousands(action_count), if action_count == 1 { "" } else { "s" }));
        }
        if info.is_running()
            && let Some((w, h)) = info.size
        {
            parts.push(format!("{w} × {h}"));
        }
        parts
    }

    #[cfg(test)]
    fn header_subtitle(&self, actions: usize, last_action_ms: Option<u64>, replay_started_ms: Option<u64>, now_ms: u64) -> String {
        self.header_subtitle_parts(actions, last_action_ms, replay_started_ms, now_ms).join(" · ")
    }

    /// Whether the Live well should show the neutral barber-pole loading view
    /// (`Opening <display_name>…`) instead of an explicit status/error line.
    fn is_live_loading(&self, has_shot: bool) -> bool {
        let Some(info) = &self.info else { return false };
        info.is_running()
            && !info.capabilities.host_disconnected
            && !has_shot
            && self.failure.is_none()
            && matches!(self.live_status, LiveStatus::Connecting | LiveStatus::Streaming | LiveStatus::Reconnecting)
    }

    fn loading_label(&self) -> String {
        let Some(info) = &self.info else { return "Opening…".to_owned() };
        match &self.live_status {
            LiveStatus::Reconnecting => match &info.id.location {
                ScreenLocation::Host { host_label, .. } => format!("Reconnecting to {host_label}…"),
                ScreenLocation::Local => format!("Opening {}…", info.display_name()),
            },
            _ => format!("Opening {}…", info.display_name()),
        }
    }

    /// Whether a cached replay action `a` (from a replay holding `replay_len`
    /// actions) is still the newest known action for this screen's observed
    /// metadata (B.9 / Astra #30: never label a suspended replay's older action
    /// as "Latest" after newer remote activity has arrived).
    fn latest_action_is_current(&self, a: &history::Action, replay_len: usize) -> bool {
        let Some(info) = &self.info else { return true };
        let a_ts = a.at.saturating_add(a.ms);
        if let Some(lu) = info.last_used_ms
            && lu > a_ts
        {
            return false;
        }
        if let Some(n) = info.actions
            && n > replay_len
        {
            return false;
        }
        true
    }

    /// When the Live footer bar should show `Latest: <mono-command><suffix>`,
    /// returns `(command, suffix)` so the command alone renders in the
    /// monospace face while `"Latest: "` and `" · <ago>"` (with both spaces)
    /// render in the proportional UI face.
    ///
    /// Shown on Live, while Loading (as soon as known), and when Disconnected
    /// (last known action in secondary ink — Opus 4).
    fn live_latest_runs(&self, latest_action: Option<&history::Action>, now_ms: u64) -> Option<(String, String)> {
        let disconnected = self.info.as_ref().is_some_and(|i| i.capabilities.host_disconnected);
        if !disconnected {
            if self.controlling || self.control_pending || self.control_note.is_some() || self.failure.is_some() {
                return None;
            }
            if let Some(info) = &self.info
                && !info.capabilities.can_control
                && info.capabilities.control_disabled_reason.is_some()
            {
                return None;
            }
            if matches!(&self.live_status, LiveStatus::Reconnecting | LiveStatus::Warning(_) | LiveStatus::Message(_) | LiveStatus::Disconnected(_))
                && !matches!(&self.live_status, LiveStatus::Warning(s) | LiveStatus::Message(s) | LiveStatus::Disconnected(s) if s.is_empty())
            {
                return None;
            }
        }
        let a = latest_action?;
        let what = replay::command(a);
        let mark = if a.error.is_some() { "  ✗" } else { "" };
        Some((what, format!("{mark} · {}", stats::ago(a.at.saturating_add(a.ms), now_ms))))
    }

    fn live_placeholder(&self) -> String {
        if let Some(err) = &self.failure {
            return err.clone();
        }
        match &self.live_status {
            LiveStatus::Connecting | LiveStatus::Streaming => "Connecting…".to_owned(),
            LiveStatus::Reconnecting => "Reconnecting…".to_owned(),
            LiveStatus::Warning(msg) => msg.clone(),
            LiveStatus::Message(msg) => msg.clone(),
            LiveStatus::Disconnected(msg) => msg.clone(),
        }
    }

    fn live_status_line(&self, latest_action: Option<&history::Action>, now_ms: u64) -> (String, bool) {
        if self.controlling {
            if let LiveStatus::Warning(msg) = &self.live_status
                && !msg.is_empty()
            {
                return (format!("You have control — {msg}"), true);
            }
            return ("You have control: your pointer and keys go to this screen.".to_owned(), true);
        }
        if self.control_pending {
            return ("Taking control…".to_owned(), false);
        }
        if let Some(info) = &self.info
            && !info.capabilities.can_control
            && let Some(reason) = &info.capabilities.control_disabled_reason
        {
            return (reason.clone(), false);
        }
        if let Some(note) = &self.control_note {
            return (note.clone(), false);
        }
        if let Some(err) = &self.failure {
            return (err.clone(), false);
        }
        match &self.live_status {
            LiveStatus::Reconnecting => return ("Reconnecting…".to_owned(), false),
            LiveStatus::Warning(msg) if !msg.is_empty() => return (msg.clone(), false),
            LiveStatus::Message(msg) if !msg.is_empty() => return (msg.clone(), false),
            LiveStatus::Disconnected(msg) if !msg.is_empty() => return (msg.clone(), false),
            _ => {}
        }
        match latest_action {
            Some(a) => {
                let what = replay::command(a);
                let mark = if a.error.is_some() { "  ✗" } else { "" };
                (format!("Latest: {what}{mark} · {}", stats::ago(a.at.saturating_add(a.ms), now_ms)), false)
            }
            None => {
                // One consistent pattern (Opus 8): "No actions yet" when none exist;
                // never "Watching.", and never contradict a remote header that says
                // "N actions" before lazy history has fetched the command text.
                if self.info.as_ref().is_some_and(|i| !i.id.is_local() && (i.actions.unwrap_or(0) > 0 || i.last_used_ms.is_some())) {
                    (String::new(), false)
                } else {
                    ("No actions yet".to_owned(), false)
                }
            }
        }
    }

    fn can_export_selection(&self, selection_has_gaps: bool) -> bool {
        !selection_has_gaps || self.info.as_ref().is_some_and(|i| i.capabilities.can_export_discontiguous)
    }

    fn menu_state(&self, replay_loaded: bool, has_actions: bool, playing: bool, exporting: bool, has_frames: bool, live_has_shot: bool) -> ScreenMenuState {
        let Some(info) = &self.info else { return ScreenMenuState::default() };
        let disconnected = info.capabilities.host_disconnected;
        let running = info.is_running();
        let shown_tab = self.shown_tab();
        let has_history = shown_tab == Tab::History && replay_loaded && has_actions;
        let can_screenshot = !disconnected && if shown_tab == Tab::Live { live_has_shot || (replay_loaded && has_frames) } else { replay_loaded && has_frames };
        // Finding 24: gate Take Control on first live frame ONLY for remote
        // sources so local screens retain their menu/shortcut from History.
        let remote_ready = info.id.is_local() || live_has_shot || self.controlling;
        ScreenMenuState {
            has_screen: true,
            running,
            shown_tab,
            has_history,
            playing,
            can_export: !disconnected && replay_loaded && !exporting && has_actions,
            can_screenshot,
            can_clear: !disconnected && !running,
            can_control: self.visible && !disconnected && running && info.capabilities.can_control && remote_ready,
            controlling: self.controlling,
            can_move_windows: !disconnected && running && info.capabilities.can_move_windows,
            giving: self.giving,
            busy: self.busy,
            can_airplay: !disconnected && running && info.capabilities.can_airplay,
        }
    }
}

impl Drop for PaneState {
    fn drop(&mut self) {
        self.stop_controlling();
        for ex in self.exports.values() {
            ex.cancel.cancel();
        }
        self.cancel_all_shots();
    }
}

/// Internal messages delivered from background workers via `cx.messenger()`
/// (`pub` because `View::Message` on `pub struct ScreenPane` requires it).
pub enum PaneMsg {
    /// Wake the UI to pull the single latest RGBA frame from `pending_live`
    /// (coalesced so a busy UI never accumulates a backlog of pictures).
    LiveFrameReady,
    /// A frame shown live: watch `watch`'s, connection generation `conn_gen`.
    LiveDmabuf {
        watch: u64,
        conn_gen: u64,
        buffer: ExternalBuffer,
    },
    LiveStatus {
        watch: u64,
        conn_gen: u64,
        status: LiveStatus,
    },
    LiveControl {
        watch: u64,
        held: bool,
        reason: Option<String>,
    },
    LiveFailed {
        watch: u64,
        conn_gen: u64,
        message: String,
    },
    Started(ScreenId, Result<ScreenInfo, String>),
    Stopped(ScreenId, Result<(), String>),
    Cleared(ScreenId, Result<(), String>),
    Renamed(ScreenId, Arc<dyn ScreenSource>, u64, Option<String>, Result<(), String>),
    /// The person's display-workspaces (display, index, name), where a
    /// screen's windows can be given.
    Spaces(Vec<(String, u32, String)>),
    /// A screen's windows given to the person: what didn't go, and why.
    Given(Result<Vec<String>, String>),
    /// Where the history's video goes (the file panel's answer), carrying the
    /// screen, source, and selected action indices captured when opening the
    /// panel so later selection or filter changes cannot redirect the export.
    ExportTo {
        id: ScreenId,
        source: Arc<dyn ScreenSource>,
        part: Option<Vec<usize>>,
        path: PathBuf,
        zoom: usize,
    },
    ExportProgress {
        id: ScreenId,
        done: f32,
    },
    Exported {
        id: ScreenId,
        result: Result<PathBuf, String>,
    },
    /// A picture of the screen at the time shown saved (or why not).
    Shot(u64, Result<PathBuf, String>),
}

/// Single-slot mailbox holding only the newest RGBA frame `(watch, conn_gen, shot)` from `watch_live`.
type PendingLiveFrame = Arc<Mutex<Option<(u64, u64, Arc<Shot>)>>>;

/// Tracks displayed vs retired `(watch, conn_gen)` so an asynchronous
/// `Reconnecting` status for a retired generation never wipes a newer
/// connection's first keyframe (Final pass B.1 / Astra #5, #10).
#[derive(Debug, Default)]
struct LiveGenTracker {
    shown: Option<(u64, u64)>,
    retired: Option<(u64, u64)>,
}

impl LiveGenTracker {
    fn reset(&mut self) {
        self.shown = None;
        self.retired = None;
    }

    /// Clear `pending_live` only if it holds a frame from `watch` with
    /// `gen <= retired_gen`. Returns `(clear_displayed, newer_already_shown)`.
    fn retire(&mut self, pending_live: &PendingLiveFrame, watch: u64, retired_gen: u64) -> (bool, bool) {
        {
            let mut slot = pending_live.lock().unwrap_or_else(|e| e.into_inner());
            if slot.as_ref().is_some_and(|(w, g, _)| *w == watch && *g <= retired_gen) {
                *slot = None;
            }
        }
        let prev = self.retired.filter(|(w, _)| *w == watch).map_or(0, |(_, g)| g);
        self.retired = Some((watch, prev.max(retired_gen)));
        match self.shown {
            Some((w, g)) if w == watch && g > retired_gen => (false, true),
            _ => {
                self.shown = None;
                (true, false)
            }
        }
    }

    /// Accept an incoming frame for `(watch, conn_gen)` iff its generation has
    /// not already been retired.
    fn accept_frame(&mut self, watch: u64, conn_gen: u64) -> bool {
        if self.retired.is_some_and(|(w, rg)| w == watch && conn_gen <= rg) {
            return false;
        }
        self.shown = Some((watch, conn_gen));
        true
    }
}

/// Reusable single-screen view embedded by `apps/screen` (and designed for
/// `apps/rex`).
///
/// Hosts that mount and unmount [`ScreenPane`] inside a long-lived window
/// should call [`ScreenPane::stop_all`] before dropping the child so any
/// compositor-side GPU external buffer key is cleared immediately.
pub struct ScreenPane {
    state: PaneState,
    me: Messenger<PaneMsg>,
    /// Width of the pane's outer box, set by the host via [`ScreenPane::set_width`]
    /// (falls back to viewport width if unset).
    width: Option<f32>,
    /// Whether Start/Stop header buttons delegate to the host via
    /// [`ScreenPaneEvent::StartRequested`] / [`ScreenPaneEvent::StopRequested`]
    /// (used by Agent Screens to honour sidebar multi-selection and its shared
    /// busy guard).
    host_lifecycle: bool,
    /// Whether this pane is hosted inside a terminal-style window whose command
    /// shortcuts arrive with an added Shift (`MenuBar::raw`).
    raw_shortcuts: bool,
    /// The screen the Live tab shows, being watched `(id, watch_seq, paused, handle)`.
    watching: Option<(ScreenId, u64, bool, Box<dyn LiveHandle>)>,
    /// Single-slot mailbox holding only the newest RGBA frame from `watch_live`
    /// so a behind UI never queues older frames.
    pending_live: PendingLiveFrame,
    live_frame_queued: Arc<AtomicBool>,
    live_gen: LiveGenTracker,
    /// The modifiers the window's compositor takes BGRx dmabufs with
    /// (empty: frames can't be shown live; `OLO_SCREEN_PICTURES` set:
    /// pictures taken anyway, to compare).
    modifiers: Vec<u64>,
    live: Child<PictureWell>,
    /// The Live picture's place, as its well last drew it.
    live_at: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    live_focus: FocusHandle,
    /// The pointer is over the picture's place.
    live_hovered: bool,
    /// The chosen screen's history, followed as it grows.
    replay: Option<Child<Replay>>,
    stats_scroll: ScrollHandle,
    /// The chosen screen's Passwords tab (made when first shown).
    shares: Option<(String, Child<Shares>)>,
    /// The chosen screen's name being changed: its field in the header.
    renaming: Option<Child<TextField>>,
    /// Where the header's title was drawn (a click there renames).
    title_at: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
}

impl ScreenPane {
    pub fn new(initial_tab: Tab, cx: &mut Cx<Self>) -> ScreenPane {
        let me = cx.messenger();
        let live = cx.new_view(|cx| PictureWell::new("Screen", cx));
        let live_at = live.read(cx).shown_at();
        let modifiers = if std::env::var_os("OLO_SCREEN_PICTURES").is_some() { Vec::new() } else { olo_kit::external_modifiers(XRGB8888) };
        let pane = ScreenPane {
            state: PaneState::new(initial_tab),
            me,
            width: None,
            host_lifecycle: false,
            raw_shortcuts: false,
            watching: None,
            pending_live: Arc::new(Mutex::new(None)),
            live_frame_queued: Arc::new(AtomicBool::new(false)),
            live_gen: LiveGenTracker::default(),
            modifiers,
            live,
            live_at,
            live_focus: FocusHandle::new(),
            live_hovered: false,
            replay: None,
            stats_scroll: ScrollHandle::new(),
            shares: None,
            renaming: None,
            title_at: Default::default(),
        };
        pane.learn_spaces();
        pane
    }

    /// Delegate header Start Screen / Stop Screen… clicks to the host via
    /// [`ScreenPaneEvent::StartRequested`] / [`ScreenPaneEvent::StopRequested`].
    pub fn set_host_lifecycle(&mut self, enabled: bool) {
        self.host_lifecycle = enabled;
    }

    /// Match terminal-style shifted command shortcuts (when embedded inside a
    /// terminal app such as Rex).
    pub fn set_raw_shortcuts(&mut self, raw: bool) {
        self.raw_shortcuts = raw;
    }

    /// Set the screen source shown by this pane (`None` shows nothing).
    pub fn set_source(&mut self, source: Option<Arc<dyn ScreenSource>>, cx: &mut Cx<Self>) {
        let was_controlling = self.state.controlling();
        let prev_id = self.state.screen_id().cloned();
        let next_id = source.as_ref().map(|s| s.id().clone());
        let next_source = source.clone();
        let changed = self.state.set_source(source);
        if was_controlling
            && !self.state.controlling()
            && let Some(id) = prev_id.clone()
        {
            cx.emit(ScreenPaneEvent::Controlling(id, false));
        }
        if changed {
            // (Another screen or source instance chosen: abandon rename and live
            // stream before binding the new source. Preserve cached `Replay`
            // read-only when rebinding the SAME screen across disconnect/reconnect
            // — Final pass B.6 / Astra #10, #13, #28.)
            self.renaming = None;
            self.watching = None;
            self.forget_live(cx);
            if prev_id.is_some()
                && prev_id == next_id
                && let (Some(r), Some(src)) = (&self.replay, next_source)
            {
                r.update(cx, |r, cx| r.rebind_source(src, cx));
            } else {
                self.replay = None;
            }
            self.learn_spaces();
        }
        self.sync_watch(cx);
        self.sync_history(cx);
        cx.invalidate();
    }

    /// Update metadata for the currently shown screen (state, size, title,
    /// capabilities) without rebuilding the pane or history follower.
    pub fn update_info(&mut self, info: ScreenInfo, cx: &mut Cx<Self>) {
        let was_controlling = self.state.controlling();
        let id = info.id.clone();
        if !self.state.update_info(info) {
            return;
        }
        if was_controlling && !self.state.controlling() {
            cx.emit(ScreenPaneEvent::Controlling(id, false));
        }
        self.sync_watch(cx);
        self.sync_history(cx);
        cx.invalidate();
    }

    /// Set the outer width allocated to this pane by its host layout.
    pub fn set_width(&mut self, width: f32, cx: &mut Cx<Self>) {
        if self.width != Some(width) {
            self.width = Some(width);
            cx.invalidate();
        }
    }

    /// Mark whether the host is performing a screen start/stop operation so the
    /// header's Start Screen button stays disabled.
    pub fn set_busy(&mut self, busy: bool, cx: &mut Cx<Self>) {
        if self.state.busy() != busy {
            self.state.set_busy(busy);
            cx.invalidate();
        }
    }

    pub fn screen_id(&self) -> Option<&ScreenId> {
        self.state.screen_id()
    }

    pub fn info(&self) -> Option<&ScreenInfo> {
        self.state.info()
    }

    pub fn tab(&self) -> Tab {
        self.state.tab()
    }

    pub fn shown_tab(&self) -> Tab {
        self.state.shown_tab()
    }

    pub fn set_tab(&mut self, tab: Tab, cx: &mut Cx<Self>) {
        let id = self.state.screen_id().cloned();
        let released = self.state.set_tab(tab);
        if released && let Some(id) = id {
            cx.emit(ScreenPaneEvent::Controlling(id, false));
        }
        self.sync_watch(cx);
        self.sync_history(cx);
        cx.emit(ScreenPaneEvent::TabChanged(tab));
        cx.invalidate();
    }

    /// Tell the pane whether it is currently visible (e.g. its host window or
    /// sidebar panel is shown). When `false`, the live stream is paused, any
    /// lazy remote history stream is closed (unless exporting), and Take
    /// Control is released until visible again.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Cx<Self>) {
        let id = self.state.screen_id().cloned();
        let released = self.state.set_visible(visible);
        if released && let Some(id) = id {
            cx.emit(ScreenPaneEvent::Controlling(id, false));
        }
        self.sync_watch(cx);
        self.sync_history(cx);
        cx.invalidate();
    }

    pub fn controlling(&self) -> bool {
        self.state.controlling()
    }

    pub fn stop_controlling(&mut self, cx: &mut Cx<Self>) {
        let id = self.state.screen_id().cloned();
        if self.state.stop_controlling() {
            if let Some(id) = id {
                cx.emit(ScreenPaneEvent::Controlling(id, false));
            }
            cx.invalidate();
        }
    }

    /// Release control, stop any active live/history streams, and unconditionally
    /// clear the compositor's external GPU buffer key (call before closing the
    /// host window, quitting, or unmounting this pane from a long-lived window).
    pub fn stop_all(&mut self, cx: &mut Cx<Self>) {
        self.stop_controlling(cx);
        self.state.cancel_all_shots();
        self.watching = None;
        self.forget_live(cx);
        self.replay = None;
    }

    /// Latest action timestamp currently known to `Replay`, for updating a
    /// host sidebar's "last used" column live.
    pub fn latest_used(&self, cx: &WindowCx) -> Option<(ScreenId, u64)> {
        let r = self.replay.as_ref()?.read(cx);
        let at = r.actions().iter().rev().find(|a| !a.is_oversized()).map(|a| a.at.saturating_add(a.ms))?;
        Some((r.id().clone(), at))
    }

    /// Snapshot of menu-bar state for the host window.
    pub fn menu_state(&self, cx: &WindowCx) -> ScreenMenuState {
        let (loaded, has_actions, playing, has_frames, has_gaps, discontiguous_blocked) = match &self.replay {
            Some(r) => {
                let r = r.read(cx);
                let has_gaps = r.export_part().is_some_and(|p| export::has_gaps(&p));
                (r.loaded(), !r.actions().is_empty(), r.playing(), r.has_frames(), has_gaps, r.export_discontiguous_blocked())
            }
            None => (false, false, false, false, false, false),
        };
        let exporting = self.state.is_exporting_current();
        let live_has_shot = self.live.read(cx).has_shot();
        let mut ms = self.state.menu_state(loaded, has_actions, playing, exporting, has_frames, live_has_shot);
        if discontiguous_blocked || !self.state.can_export_selection(has_gaps) {
            ms.can_export = false;
        }
        ms
    }

    /// Move Windows To ▸ each of the person's display-workspaces: every
    /// window on the screen there, programs running on (`moving::give`).
    pub fn give_menu(&self) -> Menu {
        let running = self.state.is_running() && self.state.info().is_some_and(|i| i.capabilities.can_move_windows);
        let mut m = Menu::new("Move Windows To");
        let mut last: Option<&str> = None;
        for (output, index, name) in &self.state.spaces {
            if last.is_some_and(|l| l != output) {
                m = m.separator();
            }
            last = Some(output);
            m = m.item(name.clone(), format!("give:{index}:{output}")).enabled(running && !self.state.giving);
        }
        if self.state.spaces.is_empty() {
            m = m.item("Your Display", "give-here").enabled(running && !self.state.giving);
        }
        m
    }

    /// Dispatch a pane-scoped menu or shortcut command. Returns `true` if
    /// recognized and handled.
    pub fn run_command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        match command {
            "rename" => self.start_rename(cx),
            "start-screen" => self.start_screen(cx),
            "stop-screen" => self.ask_stop(cx),
            "clear" => self.ask_clear(cx),
            "export" => self.ask_export(cx),
            "screenshot" => self.ask_screenshot(cx),
            "tab-live" => {
                if self.state.is_running() {
                    self.set_tab(Tab::Live, cx);
                }
            }
            "tab-history" => self.set_tab(Tab::History, cx),
            "tab-stats" => self.set_tab(Tab::Stats, cx),
            "tab-passwords" => self.set_tab(Tab::Passwords, cx),
            "find" => {
                self.set_tab(Tab::History, cx);
                if let Some(r) = &self.replay {
                    // Once the tab has drawn its search.
                    let r = r.downgrade();
                    cx.on_next_frame(move |cx| r.update(cx, |r, cx| r.focus_search(cx)));
                }
            }
            "play" | "previous" | "next" | "latest" => {
                if let Some(r) = &self.replay {
                    r.update(cx, |r, cx| match command {
                        "play" => r.play_pause(cx),
                        "previous" => r.step(-1, cx),
                        "next" => r.step(1, cx),
                        _ => r.latest(cx),
                    });
                }
            }
            "control" => {
                if self.state.controlling() || self.state.control_pending {
                    self.stop_controlling(cx);
                } else {
                    self.take_control(cx);
                }
            }
            "give-here" => self.give(None, cx),
            c if c.starts_with("give:") => {
                let mut parts = c["give:".len()..].splitn(2, ':');
                if let (Some(Ok(index)), Some(output)) = (parts.next().map(str::parse::<u32>), parts.next()) {
                    self.give(Some((index, output.to_owned())), cx);
                }
            }
            _ => return false,
        }
        cx.invalidate();
        true
    }

    fn ensure_live_watch_open(&mut self) {
        let Some(id) = self.state.running_screen_id() else { return };
        if self.watching.as_ref().is_some_and(|(cur, _, _, _)| cur == &id) {
            return;
        }
        let Some(source) = self.state.source().cloned() else { return };
        let watch = WATCHES.fetch_add(1, Ordering::Relaxed);
        let me = self.me.clone();
        let pending_live = self.pending_live.clone();
        let live_frame_queued = self.live_frame_queued.clone();
        // B.1 (#5, #10): monotonic per-watch connection generation stamped on
        // the sequential delivery thread so a retired generation's `Reconnecting`
        // status can never wipe a newer connection's first keyframe.
        let conn_gen = Arc::new(AtomicU64::new(1));
        let handle = source.watch_live(
            self.modifiers.clone(),
            Box::new(move |ev| match ev {
                LiveEvent::Dmabuf(buffer) => {
                    let cg = conn_gen.load(Ordering::Acquire);
                    me.send(PaneMsg::LiveDmabuf { watch, conn_gen: cg, buffer });
                }
                LiveEvent::Rgba(shot) => {
                    let cg = conn_gen.load(Ordering::Acquire);
                    *pending_live.lock().unwrap_or_else(|e| e.into_inner()) = Some((watch, cg, shot));
                    if !live_frame_queued.swap(true, Ordering::AcqRel) {
                        me.send(PaneMsg::LiveFrameReady);
                    }
                }
                LiveEvent::Status(status) => {
                    let cg = if matches!(status, LiveStatus::Reconnecting | LiveStatus::Disconnected(_)) { conn_gen.fetch_add(1, Ordering::AcqRel) } else { conn_gen.load(Ordering::Acquire) };
                    me.send(PaneMsg::LiveStatus { watch, conn_gen: cg, status });
                }
                LiveEvent::Control { held, reason } => {
                    me.send(PaneMsg::LiveControl { watch, held, reason });
                }
                LiveEvent::Failed(message) => {
                    let cg = conn_gen.fetch_add(1, Ordering::AcqRel);
                    me.send(PaneMsg::LiveFailed { watch, conn_gen: cg, message });
                }
            }),
        );
        self.watching = Some((id, watch, false, handle));
    }

    /// Watch the chosen running screen when its Live tab is shown; keep the
    /// handle paused while on another tab or hidden so a remote host stops
    /// capturing without tearing down and re-keyframing the stream on every
    /// tab switch. Drop the handle completely whenever switching screens,
    /// stopping, or when the remote host disconnects so a longer outage never
    /// leaves a dead/exhausted stream handle stuck after reconnect (Finding 10).
    fn sync_watch(&mut self, cx: &mut Cx<Self>) {
        let host_disconnected = self.state.info().is_some_and(|i| i.capabilities.host_disconnected);
        let running_id = if host_disconnected { None } else { self.state.running_screen_id() };
        let want_live = self.state.should_watch_live();
        if self.watching.as_ref().map(|(id, _, _, _)| id) != running_id.as_ref() && self.watching.take().is_some() {
            self.forget_live(cx);
        }
        if self.watching.is_none() && want_live.is_some() {
            self.ensure_live_watch_open();
        }
        if let Some((_, _, paused, handle)) = &mut self.watching {
            let should_pause = want_live.is_none();
            if *paused != should_pause {
                *paused = should_pause;
                handle.set_paused(should_pause);
            }
        }
        let shape = self.state.info().and_then(|i| i.size).map(|(w, h)| (w as f32, h as f32));
        self.live.update(cx, |w, _| w.set_shape(shape));
    }

    /// Let go of the frames shown live (the compositor's buffers of them
    /// too), once the window is free to be told.
    fn forget_live(&mut self, cx: &mut Cx<Self>) {
        *self.pending_live.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.live_gen.reset();
        self.state.live_pixel_size = None;
        let key = self.live.read(cx).external_key();
        self.live.update(cx, |w, cx| w.clear(None, cx));
        cx.set_external_buffer(key, None);
    }

    /// Retire ONLY frames belonging to `(watch, <= retired_gen)` so a queued
    /// `Reconnecting` status never erases a newer connection's first keyframe
    /// (Final pass B.1 / Astra #5, #10). Returns `true` if a newer generation
    /// is already displayed.
    fn forget_live_retired(&mut self, watch: u64, retired_gen: u64, cx: &mut Cx<Self>) -> bool {
        let (clear_displayed, newer_shown) = self.live_gen.retire(&self.pending_live, watch, retired_gen);
        if clear_displayed {
            self.state.live_pixel_size = None;
            let key = self.live.read(cx).external_key();
            self.live.update(cx, |w, cx| w.clear(None, cx));
            cx.set_external_buffer(key, None);
        }
        newer_shown
    }

    pub fn take_control(&mut self, cx: &mut Cx<Self>) {
        if !self.state.visible {
            return;
        }
        let Some(info) = self.state.info() else { return };
        if !info.is_running() || !info.capabilities.can_control || info.capabilities.host_disconnected {
            return;
        }
        // Finding 5 & 24: enforce first-frame picture readiness inside
        // `take_control` for remote screens (even if invoked directly via the
        // agent command dispatcher), while letting local screens take control
        // from History/shortcut before Live opens.
        if !info.id.is_local() && !self.live.read(cx).has_shot() && !self.state.controlling() {
            return;
        }
        let id = info.id.clone();
        let prev_tab = self.state.tab();
        // Ensure the live stream exists before asking a remote source for
        // control (remote `set_controlled(true)` writes `Control` over it),
        // without committing the tab switch until acquisition succeeds.
        self.ensure_live_watch_open();
        match self.state.take_control() {
            Ok(ControlAcquire::Acquired) => {
                if prev_tab != Tab::Live {
                    cx.emit(ScreenPaneEvent::TabChanged(Tab::Live));
                }
                self.sync_watch(cx);
                self.sync_history(cx);
                self.live_focus.focus(cx);
                cx.emit(ScreenPaneEvent::Controlling(id, true));
                cx.invalidate();
            }
            Ok(ControlAcquire::Pending) => {
                self.sync_watch(cx);
                cx.invalidate();
            }
            Ok(ControlAcquire::Noop) => {
                self.sync_watch(cx);
            }
            Err(e) => {
                self.sync_watch(cx);
                self.alert(&format!("Couldn’t take control: {e}"), cx);
            }
        }
    }

    fn alert(&self, message: &str, cx: &mut Cx<Self>) {
        open_alert(cx, "Agent Screens", "stop", message, &["OK"], |_, _| {});
    }

    fn start_screen(&mut self, cx: &mut Cx<Self>) {
        if self.state.busy() || self.state.is_running() {
            return;
        }
        let Some(source) = self.state.source().cloned() else { return };
        let orig_id = source.id().clone();
        self.state.set_busy(true);
        let me = self.me.clone();
        std::thread::spawn(move || me.send(PaneMsg::Started(orig_id, source.start())));
        cx.invalidate();
    }

    fn ask_stop(&mut self, cx: &mut Cx<Self>) {
        let Some(info) = self.state.info().filter(|i| i.is_running()).cloned() else { return };
        let Some(source) = self.state.source().cloned() else { return };
        // Capture BOTH `started` and `last_used` BY VALUE via the shared helper
        // (sends `Some(0)` for known-empty screens and fails closed on remote
        // screens whose `started` is unknown — Final pass B.3 / Astra #4).
        let started = source.session_started().or_else(|| info.started_ms.map(|ms| ms / 1000));
        let replay_lu = self.replay.as_ref().and_then(|r| r.read(cx).actions().iter().rev().find(|a| !a.is_oversized()).map(|a| a.at.saturating_add(a.ms)));
        let (expected_started, expected_last_used) = match capture_stop_expectations(info.id.is_local(), started, replay_lu, info.last_used_ms, info.actions) {
            Ok(v) => v,
            Err(msg) => {
                self.alert(&msg, cx);
                return;
            }
        };
        let where_str = match &info.id.location {
            ScreenLocation::Host { host_label, .. } => format!(" on {host_label}"),
            ScreenLocation::Local if info.capabilities.show_location => " on this computer".to_owned(),
            ScreenLocation::Local => String::new(),
        };
        let message = format!(
            "Stop the screen “{}”{where_str}? The agent’s programs on it are closed, and anything it hasn’t saved is lost. The screen is kept, with its history: Start Screen starts it again.",
            info.display_name()
        );
        let me = cx.view_ref();
        let id = info.id.clone();
        open_alert(cx, "Stop Screen", "warning", &message, &["Cancel", "Stop Screen"], move |choice, cx| {
            if *choice != Some(1) {
                return;
            }
            let (source, id) = (source.clone(), id.clone());
            me.update(cx, move |p, cx| {
                // B.2 (#5/#24): only release the pane's active control if it is
                // still bound to the EXACT screen source and session generation
                // captured when the dialog opened (never a replacement session!).
                let release_first = if p.state.matches_captured_session(&id, &source, expected_started) {
                    let (was_controlling, pending) = p.state.detach_control_for_worker();
                    if was_controlling {
                        cx.emit(ScreenPaneEvent::Controlling(id.clone(), false));
                    }
                    pending
                } else {
                    None
                };
                let messenger = p.me.clone();
                std::thread::spawn(move || {
                    if let Some((ctrl_src, up)) = release_first {
                        let _ = ctrl_src.release_control_and_wait(up);
                    }
                    messenger.send(PaneMsg::Stopped(id, source.stop_if_expected(expected_started, expected_last_used)));
                });
                cx.invalidate();
            });
        });
    }

    fn ask_clear(&mut self, cx: &mut Cx<Self>) {
        let Some(info) = self.state.info().filter(|i| !i.is_running()).cloned() else { return };
        let Some(source) = self.state.source().cloned() else { return };
        // Capture `expected_started = Some(0)` (stopped) and `expected_last_used`
        // BY VALUE at dialog-open time (`Some(0)` = observed empty; Finding 3).
        let replay_lu = self.replay.as_ref().and_then(|r| r.read(cx).actions().iter().rev().find(|a| !a.is_oversized()).map(|a| a.at.saturating_add(a.ms)));
        let observed_lu = match (replay_lu, info.last_used_ms) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        let expected_started = Some(0u64);
        let expected_last_used = Some(observed_lu.unwrap_or(0));
        let where_str = match &info.id.location {
            ScreenLocation::Host { host_label, .. } => format!(" on {host_label}"),
            ScreenLocation::Local if info.capabilities.show_location => " on this computer".to_owned(),
            ScreenLocation::Local => String::new(),
        };
        let message = format!("Clear the history of “{}”{where_str}? Its actions and the frames kept with them are deleted. This can’t be undone.", info.display_name());
        let messenger = self.me.clone();
        let id = info.id.clone();
        open_alert(cx, "Clear History", "warning", &message, &["Cancel", "Clear History"], move |choice, _| {
            if *choice != Some(1) {
                return;
            }
            let (messenger, source, id) = (messenger.clone(), source.clone(), id.clone());
            std::thread::spawn(move || messenger.send(PaneMsg::Cleared(id, source.clear_history_expected(expected_started, expected_last_used))));
        });
    }

    fn can_export(&self, cx: &WindowCx) -> bool {
        !self.state.info().is_some_and(|i| i.capabilities.host_disconnected)
            && !self.state.is_exporting_current()
            && self.replay.as_ref().is_some_and(|r| {
                let r = r.read(cx);
                r.loaded() && !r.actions().is_empty() && !r.export_discontiguous_blocked()
            })
    }

    /// Whether there's a picture to take: on Live, a live picture (or recorded
    /// history); on History/Stats, a history with recorded frames.
    fn can_shoot(&self, cx: &WindowCx) -> bool {
        if self.state.info().is_some_and(|i| i.capabilities.host_disconnected) {
            return false;
        }
        if self.state.shown_tab() == Tab::Live && self.live.read(cx).has_shot() {
            return true;
        }
        self.replay.as_ref().is_some_and(|r| {
            let r = r.read(cx);
            r.loaded() && r.has_frames()
        })
    }

    fn ask_export(&mut self, cx: &mut Cx<Self>) {
        if !self.can_export(cx) {
            return;
        }
        let Some(replay) = self.replay.as_ref() else { return };
        // Capture screen identity, source, and action selection at Save-panel
        // open time so switching screens or changing filters while the panel is
        // open cannot export the wrong screen into the proposed filename.
        let (id, name, source, part, size) = {
            let r = replay.read(cx);
            (r.id().clone(), r.name.clone(), r.source().clone(), r.export_part(), r.screen_size())
        };
        if let Some(ref p) = part
            && export::has_gaps(p)
            && !source.info().capabilities.can_export_discontiguous
        {
            self.alert("This host only supports exporting a contiguous range of actions. Select adjacent actions or update Olo on the host to export a filtered subset.", cx);
            return;
        }
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let dir = [home.join("Videos"), home.clone()].into_iter().find(|d| d.is_dir()).unwrap_or(home);
        // (A part of it, the rows chosen or those a search shows: said in
        // its name.)
        let file = match part.as_ref().map(Vec::len) {
            Some(1) => format!("{name} (1 action).mp4"),
            Some(n) => format!("{name} ({n} actions).mp4"),
            None => format!("{name}.mp4"),
        };
        // Its size: as the screen is, or zoomed (each pixel two or three
        // times over: a small screen easier to read), as far as H.264
        // goes; the last chosen, again. Omit the picker if `last_size` is
        // unknown (`None`) rather than displaying "(0 × 0)".
        let options: Vec<(String, String)> = match size {
            Some((w, h)) if w > 0 && h > 0 => export::zooms(w, h)
                .into_iter()
                .map(|z| {
                    let (vw, vh) = ((w & !1) * z, (h & !1) * z);
                    let label = if z == 1 { format!("Actual Size ({vw} × {vh})") } else { format!("{z}× ({vw} × {vh})") };
                    (z.to_string(), label)
                })
                .collect(),
            _ => Vec::new(),
        };
        let last = LAST_ZOOM.load(Ordering::Relaxed).to_string();
        let value = options.iter().find(|o| o.0 == last).map_or_else(|| "1".to_owned(), |o| o.0.clone());
        let mut panel = file_panel::FilePanel::save(dir, file);
        if options.len() > 1 {
            panel = panel.choices(vec![file_panel::Choice { id: "zoom".into(), label: "Size:".into(), options, value }]);
        }
        let me = self.me.clone();
        file_panel::show("Export Video", panel, move |a| {
            if let file_panel::PanelResult::Chosen(path) = a.result {
                let zoom = a.choices.iter().find(|c| c.id == "zoom").and_then(|c| c.value.parse().ok()).unwrap_or(1);
                LAST_ZOOM.store(zoom, Ordering::Relaxed);
                me.send(PaneMsg::ExportTo { id: id.clone(), source: source.clone(), part: part.clone(), path, zoom });
            }
        });
    }

    /// Take Screenshot…: the screen as it was at the time shown, saved
    /// as a PNG where the person says, at a size chosen as a video's is
    /// (each pixel two or three times over: a small screen easier to
    /// read), the last chosen again (Ben, 2026-11-17: "agent screens
    /// needs a "Take Screenshot..." button that takes a screenshot at the
    /// current time point and saves it and the user should be able to
    /// choose the size like they can for the video").
    fn ask_screenshot(&mut self, cx: &mut Cx<Self>) {
        if !self.can_shoot(cx) {
            return;
        }
        let on_live = self.state.shown_tab() == Tab::Live;
        let live_shot = on_live.then(|| self.live.read(cx).current_shot()).flatten();
        let live_has_dmabuf = on_live && live_shot.is_none() && self.live.read(cx).has_shot();
        let (_name, source, at, fallback_at, file, size) = if let Some(ref shot) = live_shot {
            let Some(info) = self.state.info() else { return };
            let Some(source) = self.state.source().cloned() else { return };
            let name = info.id.name.clone();
            let clock = history::clock(history::now_ms());
            let stamp = clock.get(11..19).unwrap_or("live").replace(':', ".");
            (name.clone(), source, 0u64, None, format!("{name} live {stamp}.png"), Some((shot.size[0], shot.size[1])))
        } else if live_has_dmabuf {
            let Some(info) = self.state.info() else { return };
            let Some(source) = self.state.source().cloned() else { return };
            let name = info.id.name.clone();
            let now = history::now_ms();
            let stamp = history::clock(now).get(11..19).unwrap_or("live").replace(':', ".");
            let fallback = self.replay.as_ref().map(|r| r.read(cx).playhead());
            let sz = self.state.live_pixel_size.or_else(|| self.replay.as_ref().and_then(|r| r.read(cx).screen_size()));
            (name.clone(), source, now, fallback, format!("{name} live {stamp}.png"), sz)
        } else {
            let Some(replay) = self.replay.as_ref() else { return };
            let r = replay.read(cx);
            let time = r.time_shown();
            let sz = r.screen_size().or_else(|| r.current_shot_size(cx));
            (r.name.clone(), r.source().clone(), r.playhead(), None, format!("{} at {}.png", r.name, time.replace(':', ".")), sz)
        };
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let dir = [home.join("Pictures"), home.clone()].into_iter().find(|d| d.is_dir()).unwrap_or(home);
        let options: Vec<(String, String)> = match size {
            Some((w, h)) if w > 0 && h > 0 => {
                export::zooms(w, h).into_iter().map(|z| (z.to_string(), if z == 1 { format!("Actual Size ({w} × {h})") } else { format!("{z}× ({} × {})", w * z, h * z) })).collect()
            }
            _ => Vec::new(),
        };
        let last = LAST_SHOT_ZOOM.load(Ordering::Relaxed).to_string();
        let value = options.iter().find(|o| o.0 == last).map_or_else(|| "1".to_owned(), |o| o.0.clone());
        let mut panel = file_panel::FilePanel::save(dir, file).types(&["png"]);
        if options.len() > 1 {
            panel = panel.choices(vec![file_panel::Choice { id: "zoom".into(), label: "Size:".into(), options, value }]);
        }
        let me = self.me.clone();
        let (shot_id, cancel) = self.state.register_shot();
        file_panel::show("Take Screenshot", panel, move |a| {
            let file_panel::PanelResult::Chosen(path) = a.result else {
                me.send(PaneMsg::Shot(shot_id, Err("cancelled".into())));
                return;
            };
            let zoom = a.choices.iter().find(|c| c.id == "zoom").and_then(|c| c.value.parse().ok()).unwrap_or(1usize);
            LAST_SHOT_ZOOM.store(zoom, Ordering::Relaxed);
            let source = source.clone();
            let live_shot = live_shot.clone();
            let cancel = cancel.clone();
            let me = me.clone();
            // (Decoded from its keyframe and written on a worker: a 5K
            // screen's picture is tens of megabytes.)
            let _ = std::thread::Builder::new().name("screenshot".into()).spawn(move || {
                let res = if cancel.is_cancelled() {
                    Err("cancelled".into())
                } else if let Some(shot) = live_shot {
                    let frame = olo_screen_codec::Frame { width: shot.size[0], height: shot.size[1], rgba: shot.rgba.clone() };
                    export::save_shot(&frame, zoom, &path).map(|()| path)
                } else {
                    let mut fetched = source.frame_at_pinned(at, &cancel);
                    if matches!(fetched, Ok(None))
                        && let Some(fb) = fallback_at
                    {
                        fetched = source.frame_at_pinned(fb, &cancel);
                    }
                    fetched.and_then(|opt| {
                        let (_, frame) = opt.ok_or_else(|| "no picture was kept from then".to_owned())?;
                        if cancel.is_cancelled() {
                            return Err("cancelled".into());
                        }
                        export::save_shot(&frame, zoom, &path)?;
                        Ok(path)
                    })
                };
                me.send(PaneMsg::Shot(shot_id, res));
            });
        });
    }

    /// An export ended: where the video is, or why not.
    fn exported(&mut self, result: &Result<PathBuf, String>, cx: &mut Cx<Self>) {
        match result {
            Ok(path) => {
                let message = format!("The video “{}” is saved.", path.file_name().and_then(|n| n.to_str()).unwrap_or(""));
                let path = path.clone();
                let me = cx.view_ref();
                open_alert(cx, "Export Video", "video", &message, &[olo_kit::tracker::LABEL, "OK"], move |choice, cx| {
                    if *choice == Some(0)
                        && let Err(e) = olo_kit::tracker::open(&path)
                    {
                        me.update(cx, move |p, cx| p.alert(&format!("Tracker couldn’t show it: {e}"), cx));
                    }
                });
            }
            Err(e) if e == "cancelled" => {}
            Err(e) => self.alert(&format!("The video couldn’t be made: {e}"), cx),
        }
    }

    /// The person's display-workspaces, asked on a worker.
    fn learn_spaces(&self) {
        let Some(source) = self.state.source().cloned() else { return };
        if !source.info().capabilities.can_move_windows {
            return;
        }
        let me = self.me.clone();
        std::thread::spawn(move || {
            if let Ok(spaces) = source.person_workspaces() {
                me.send(PaneMsg::Spaces(spaces));
            }
        });
    }

    /// Every window on the chosen screen to the person's workspace
    /// `index` of `output` (None: wherever they come), on a worker.
    fn give(&mut self, to: Option<(u32, String)>, cx: &mut Cx<Self>) {
        if self.state.giving || !self.state.is_running() {
            return;
        }
        let Some(source) = self.state.source().cloned() else { return };
        self.state.giving = true;
        let me = self.me.clone();
        std::thread::spawn(move || me.send(PaneMsg::Given(source.give_windows(to))));
        cx.invalidate();
    }

    /// `menu` at `at`: what's chosen from it is done here.
    fn menu_at(&mut self, menu: Menu, at: Point<Pixels>, cx: &mut Cx<Self>) {
        let me = cx.view_ref();
        olo_kit::menu::context_menu(cx, menu, at, move |cmd, cx| {
            let cmd = cmd.to_owned();
            me.update(cx, move |p, cx| {
                p.run_command(&cmd, cx);
            });
        });
    }

    /// The chosen screen's name, made a field in the header (Rename…, or
    /// a click on it): Return keeps what's typed, Escape or an empty
    /// field puts its own back.
    fn start_rename(&mut self, cx: &mut Cx<Self>) {
        let Some(info) = self.state.info() else { return };
        let shown = info.display_name().to_owned();
        let field = olo_kit::widgets::text_field(cx, |f| f.name("Name"));
        field.update(cx, |f, cx| {
            f.set_text(shown, cx);
            f.select_all_text(cx);
        });
        field.focus(cx);
        cx.subscribe(&field, |p: &mut ScreenPane, e: &TextFieldEvent, cx| {
            if matches!(e, TextFieldEvent::Entered) {
                p.finish_rename(true, cx);
            }
        });
        self.renaming = Some(field);
        cx.invalidate();
    }

    /// The rename done (`keep`: as typed) or given up.
    fn finish_rename(&mut self, keep: bool, cx: &mut Cx<Self>) {
        let Some(field) = self.renaming.take() else { return };
        let Some(source) = self.state.source().cloned() else { return };
        let Some(id) = self.state.screen_id().cloned() else { return };
        if keep {
            let typed = field.read(cx).text().to_owned();
            let title = self.state.normalize_rename(&typed);
            // Finding 33: reject out-of-policy titles at edit time before
            // mutating displayed state or sending a request that fails locally.
            if let Some(ref t) = title
                && t.len() > crate::rex_sock::MAX_LABEL_LEN
            {
                self.alert(&format!("Screen name is too long (at most {} bytes).", crate::rex_sock::MAX_LABEL_LEN), cx);
                cx.invalidate();
                return;
            }
            // Finding 26 & Final pass B.8: do not invent/commit the renamed title
            // optimistically before the RPC succeeds, and capture `(source, info_rev)`
            // so a delayed Rename reply never overwrites newer Follow metadata.
            let me = self.me.clone();
            let info_rev = self.state.info_rev;
            std::thread::spawn(move || {
                let res = source.rename(title.as_deref());
                me.send(PaneMsg::Renamed(id, source, info_rev, title, res));
            });
        }
        cx.invalidate();
    }

    /// The history of the chosen screen, made when it's first wanted.
    fn ensure_replay(&mut self, cx: &mut Cx<Self>) {
        let Some(source) = self.state.source().cloned() else { return };
        let id = source.id().clone();
        if self.replay.as_ref().is_some_and(|r| r.read(cx).id() == &id) {
            return;
        }
        let replay = cx.new_view(|cx| Replay::new(source, cx));
        // (The header and sidebar say the newest action.)
        cx.subscribe(&replay, |p: &mut ScreenPane, _: &ReplayUpdated, cx| {
            if let Some((id, at)) = p.latest_used(cx) {
                cx.emit(ScreenPaneEvent::Used(id, at));
            }
            cx.invalidate();
        });
        let export_ui = self.state.export_for_current();
        replay.update(cx, |r, cx| r.set_exporting(export_ui, cx));
        self.replay = Some(replay);
    }

    /// Keep the history follower open only when needed: eagerly for local
    /// screens (zero-cost `inotify`, feeding the Live header/status bar), and
    /// lazily for remote host screens (opened only while History or Stats is
    /// shown or an export runs, closed otherwise while retaining cached UI).
    fn sync_history(&mut self, cx: &mut Cx<Self>) {
        if self.state.should_follow_history() {
            self.ensure_replay(cx);
            let export_ui = self.state.export_for_current();
            if let Some(r) = &self.replay {
                r.update(cx, |r, cx| {
                    r.set_history_active(true);
                    r.set_exporting(export_ui, cx);
                });
            }
        } else if let Some(r) = &self.replay {
            let export_ui = self.state.export_for_current();
            r.update(cx, |r, cx| {
                r.set_history_active(false);
                r.set_exporting(export_ui, cx);
            });
        }
    }

    /// The chosen screen's name, how it's doing, and what can be done
    /// with it.
    fn header(&mut self, cx: &mut Cx<Self>) -> Element {
        let Some(info) = self.state.info().cloned() else { return div().into_any_element() };
        let now = history::now_ms();
        let (actions, last, started) = match &self.replay {
            Some(r) => {
                let r = r.read(cx);
                (r.actions().len(), r.actions().iter().rev().find(|a| !a.is_oversized()).map(|a| a.at.saturating_add(a.ms)), r.started())
            }
            None => (0, None, None),
        };
        let running = info.is_running();
        let disconnected = info.capabilities.host_disconnected;
        let line_parts = self.state.header_subtitle_parts(actions, last, started, now);
        let title = SharedString::from(info.display_name().to_owned());
        let renaming_now = self.renaming.is_some();
        let title_at = self.title_at.clone();
        let face = canvas(move |g, r| {
            olo_kit::review::own_grid(g, r);
            let p = theme::palette();
            let icon = style::snap(g, Bounds { origin: point(r.left(), r.center().y - px(16.0)), size: size(px(32.0), px(32.0)) });
            icons::paint(g, icon, "agent-screens");
            let x = icon.right() + px(m::SPACING);
            let w = (r.right() - x).max(px(0.0));
            // (No room for even a few letters, in a window made narrow:
            // none, rather than an "…" over the buttons beside it.)
            if w < px(24.0) {
                return;
            }
            // (Its field over it while it's renamed.)
            if !renaming_now {
                let t = style::label_fit(g, x, r.top() + px(11.0), style::Align::Left, title.clone(), 15.0, style::ui_font(FontWeight::BOLD), c(p.panel_text), w);
                agent::record(g, agent::Node { role: "Heading", name: Some(title.clone()), bounds: t, enabled: true, ..Default::default() });
                title_at.set(t);
            }
            // (Left out while a name is typed: the field has the room.)
            if renaming_now {
                return;
            }
            let line_y = r.top() + px(29.0);
            // Raise dot slightly to align optically with the x-height and `·` dividers.
            let dot_center = point(x + px(4.0), line_y - px(0.5));
            if disconnected {
                paint_status_ring(g, dot_center, 8.0, tertiary_ink(&p, p.panel_background));
            } else {
                let dot = if running { c(p.success) } else { style::disabled_text(&p) };
                paint_status_dot(g, dot_center, 8.0, dot);
            }
            let sub_w = (w - px(14.0)).max(px(0.0));
            let plain = style::ui_font(FontWeight::NORMAL);
            let mut shown = line_parts.clone();
            let mut text = shown.join(" · ");
            // Drop whole trailing segments rather than cutting mid-word.
            while shown.len() > 1 && style::text_width(text.clone(), style::PLAIN_SIZE, plain.clone()) > sub_w {
                shown.pop();
                text = shown.join(" · ");
            }
            let sub_ink = if disconnected { tertiary_ink(&p, p.panel_background) } else { style::secondary_text(&p) };
            let t = style::label_fit(g, x + px(14.0), line_y, style::Align::Left, text.clone(), style::PLAIN_SIZE, plain, sub_ink, sub_w);
            agent::record(g, agent::Node::label(text, t));
        })
        .absolute()
        .inset_0();
        // A click on the title renames it (as Tracker's names); the field,
        // over where it was, keeps it with Return and gives up with Escape.
        let face = div().flex_1().min_w_0().h_full().relative().child(face).on_mouse_down(
            MouseButton::Left,
            cx.listener(|p: &mut ScreenPane, e: &MouseDownEvent, cx| {
                if p.renaming.is_none() && !p.state.info().is_some_and(|i| i.capabilities.host_disconnected) && p.title_at.get().contains(&e.position) {
                    // (The press goes no further: the field has the
                    // keyboard, not what's under.)
                    cx.prevent_default();
                    cx.stop_propagation();
                    p.start_rename(cx);
                }
            }),
        );
        let face = match &self.renaming {
            Some(field) => face.child(
                div()
                    .absolute()
                    .left(px(32.0 + m::SPACING))
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .on_key_down(cx.listener(|p: &mut ScreenPane, e: &KeyDownEvent, cx| {
                        if e.keystroke.key == "escape" {
                            p.finish_rename(false, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(TextControl::new(cx, field).width(RENAME_W)),
            ),
            None => face,
        };
        let first = if running {
            Button::new("Stop Screen…").min_width(96.0).enabled(!disconnected).on_click(cx.listener(|p: &mut ScreenPane, _, cx| {
                if p.host_lifecycle {
                    if let Some(id) = p.state.screen_id().cloned() {
                        cx.emit(ScreenPaneEvent::StopRequested(id));
                    }
                } else {
                    p.ask_stop(cx);
                }
            }))
        } else {
            Button::new("Clear History…").min_width(96.0).enabled(!disconnected).on_click(cx.listener(|p: &mut ScreenPane, _, cx| p.ask_clear(cx)))
        };
        // Exporting, it shows how far (an export takes seconds to minutes),
        // History's bar too.
        let exporting = self.state.export_done_current();
        let export = match exporting {
            Some(done) => Button::new("Export Video…").min_width(96.0).working(Some(done)),
            None => Button::new("Export Video…").min_width(96.0).enabled(self.can_export(cx)).on_click(cx.listener(|p: &mut ScreenPane, _, cx| p.ask_export(cx))),
        };
        let shot = Button::new("Take Screenshot…").min_width(96.0).enabled(self.can_shoot(cx)).on_click(cx.listener(|p: &mut ScreenPane, _, cx| p.ask_screenshot(cx)));
        // Finding 31: on narrow panes (380 pt min window / 420 pt vertical third),
        // fold secondary menu-accessible actions (Export Video…, Take Screenshot…)
        // so the header never overflows its width.
        let pane_w = self.width.unwrap_or_else(|| f32::from(cx.viewport_size().width));
        let show_export = if running { pane_w >= 460.0 } else { pane_w >= 540.0 };
        let show_shot = running || pane_w >= 540.0;
        let mut buttons: Vec<Element> = Vec::new();
        if show_export {
            buttons.push(export.into_any_element());
        }
        if show_shot {
            buttons.push(shot.into_any_element());
        }
        buttons.push(first.into_any_element());
        // Stopped: started again as it was.
        if !running {
            let busy = self.state.busy();
            buttons.push(
                Button::new(if busy { "Starting…" } else { "Start Screen" })
                    .id("start-screen")
                    .min_width(96.0)
                    .enabled(!busy && !disconnected)
                    .on_click(cx.listener(|p: &mut ScreenPane, _, cx| {
                        if p.host_lifecycle {
                            if let Some(id) = p.state.screen_id().cloned() {
                                cx.emit(ScreenPaneEvent::StartRequested(id));
                            }
                        } else {
                            p.start_screen(cx);
                        }
                    }))
                    .into_any_element(),
            );
        }
        header_bar(face.into_any_element(), buttons)
    }

    /// EmptyState-style disconnected host explanation (shared between the Live
    /// well and offline History/Stats tabs when no cached replay is loaded).
    fn disconnected_panel(&self, info: &ScreenInfo, on_well: bool, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let (host_id, host_label) = match &info.id.location {
            ScreenLocation::Host { host_id, host_label } => (host_id.clone(), host_label.clone()),
            ScreenLocation::Local => (String::new(), "this computer".to_owned()),
        };
        let bg = if on_well { p.shade(p.panel_background, 1.18) } else { p.panel_background };
        let title_ink = c(olo_theme::readable(p.panel_text, bg, 7.0));
        // On the medium-grey Live viewport fill (`on_well`), render the second
        // line in primary ink too so it reads crisply in light & dark (Gemini 4).
        let body_ink = if on_well { title_ink } else { c(olo_theme::readable(olo_theme::mix(p.panel_text, bg, 0.30), bg, 4.5)) };
        let title_str = SharedString::from(format!("{host_label} is disconnected."));
        let detail_str = SharedString::from("Reconnect to it in Rex to see this screen again.");
        let icon = canvas(|g, b| {
            icons::paint_faded(g, b, "rex", DISABLED_OPACITY);
        })
        .w(px(32.0))
        .h(px(32.0))
        .flex_shrink_0();
        let title_el = {
            let s = title_str.clone();
            canvas(move |g, b| {
                let t = style::label_fit(g, b.center().x, b.center().y, style::Align::Center, s.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), title_ink, b.size.width);
                agent::record(g, agent::Node::label(s.clone(), t));
            })
            .w_full()
            .h(px(m::LABEL_ROW))
        };
        let detail_el = {
            let s = detail_str.clone();
            canvas(move |g, b| {
                let t = style::label_fit(g, b.center().x, b.center().y, style::Align::Center, s.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), body_ink, b.size.width);
                agent::record(g, agent::Node::label(s.clone(), t));
            })
            .w_full()
            .h(px(m::LABEL_ROW))
        };
        let show_rex = (!host_id.is_empty()).then(|| {
            let hid = host_id.clone();
            Button::new("Show in Rex").id("show-in-rex").on_click(cx.listener(move |_: &mut ScreenPane, _, cx| {
                cx.emit(ScreenPaneEvent::OpenHost(hid.clone()));
            }))
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px(px(m::WINDOW_INSET * 2.0))
            .child(
                div()
                    .w_full()
                    .max_w(px(360.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(icon)
                    .child(div().h(px(16.0)))
                    .child(title_el)
                    .child(div().h(px(4.0)))
                    .child(detail_el)
                    .children(show_rex.map(|btn| div().mt(px(20.0)).child(btn))),
            )
            .into_any_element()
    }

    /// The screen as it is now, and Take Control.
    fn live_tab(&mut self, cx: &mut Cx<Self>) -> Element {
        let Some(info) = self.state.info().filter(|i| i.is_running()).cloned() else { return div().into_any_element() };
        let p = theme::palette();
        let has_shot = self.live.read(cx).has_shot();
        let disconnected = info.capabilities.host_disconnected;
        let is_loading = self.state.is_live_loading(has_shot);
        if disconnected || is_loading {
            self.live.update(cx, |w, cx| w.clear(None, cx));
        } else if !has_shot {
            let placeholder = SharedString::from(self.state.live_placeholder());
            self.live.update(cx, |w, cx| w.clear(Some(placeholder), cx));
        }
        let controlling = self.state.controlling();
        // Over the picture: the ring saying the keys go there, and the
        // person's pointer and keys sent on to the screen.
        let at = self.live_at.clone();
        let ring = canvas(move |g, _| {
            let Some(pic) = at.get() else { return };
            if !controlling {
                return;
            }
            let p = theme::palette();
            let ring = pic.dilate(px(3.0));
            g.paint_quad(olo_kit::outline(ring, c(p.keyboard_navigation), olo_kit::BorderStyle::Solid).border_widths(px(2.0)));
        })
        .absolute()
        .inset_0();
        let well_overlay: Option<Element> = if disconnected {
            Some(div().absolute().inset_0().child(self.disconnected_panel(&info, true, cx)).into_any_element())
        } else if is_loading {
            let well_bg = p.shade(p.panel_background, 1.18);
            // Primary ink on the medium-grey Live viewport fill so the loading
            // label meets >= 4.5:1 crisply in light & dark looks (Gemini 4).
            let body_ink = c(olo_theme::readable(p.panel_text, well_bg, 7.0));
            let label_str = SharedString::from(self.state.loading_label());
            let label_el = {
                let s = label_str.clone();
                canvas(move |g, b| {
                    let t = style::label_fit(g, b.center().x, b.center().y, style::Align::Center, s.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), body_ink, b.size.width);
                    agent::record(g, agent::Node::label(s.clone(), t));
                })
                .w_full()
                .h(px(m::LABEL_ROW))
            };
            Some(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .px(px(m::WINDOW_INSET * 2.0))
                    .child(div().w_full().max_w(px(320.0)).flex().flex_col().items_center().child(label_el).child(div().h(px(10.0))).child(barber_bar(132.0, 11.0, label_str)))
                    .into_any_element(),
            )
        } else {
            None
        };
        let mut input = div().absolute().inset_0().child(ring);
        if controlling {
            let (sw, sh) = self.state.live_logical_size();
            let to_screen = move |pic: Bounds<Pixels>, pos: Point<Pixels>| -> (f64, f64) {
                let x = (f32::from(pos.x - pic.left()) / f32::from(pic.size.width)).clamp(0.0, 1.0) as f64 * sw;
                let y = (f32::from(pos.y - pic.top()) / f32::from(pic.size.height)).clamp(0.0, 1.0) as f64 * sh;
                (x, y)
            };
            let button = |b: MouseButton| match b {
                MouseButton::Left => Some(screens::LEFT),
                MouseButton::Right => Some(screens::RIGHT),
                MouseButton::Middle => Some(screens::MIDDLE),
                _ => None,
            };
            input = input
                // (Every move over the picture; anywhere in the window
                // while a button is held: a drag goes on outside it.)
                .hears_motion()
                .on_drag_move(cx.listener(move |p: &mut ScreenPane, e: &MouseMoveEvent, _| {
                    let Some(pic) = p.live_at.get() else { return };
                    let over = pic.contains(&e.position) && p.live_hovered;
                    if over || !p.state.pressed.is_empty() {
                        let (x, y) = to_screen(pic, e.position);
                        p.state.handle_pointer_move(x, y);
                    }
                }))
                .on_hover(cx.listener(|p: &mut ScreenPane, over: &bool, _| p.live_hovered = *over))
                .on_any_mouse_down(cx.listener(move |p: &mut ScreenPane, e: &MouseDownEvent, cx| {
                    let Some(pic) = p.live_at.get().filter(|pic| pic.contains(&e.position)) else { return };
                    let Some(button) = button(e.button) else { return };
                    let (x, y) = to_screen(pic, e.position);
                    p.live_focus.focus(cx);
                    p.state.handle_pointer_button(x, y, button, true);
                    cx.prevent_default();
                    cx.stop_propagation();
                }))
                .on_window_mouse_up(cx.listener(move |p: &mut ScreenPane, e: &MouseUpEvent, _| {
                    let Some(button) = button(e.button) else { return };
                    let Some(pic) = p.live_at.get() else { return };
                    let (x, y) = to_screen(pic, e.position);
                    p.state.handle_pointer_button(x, y, button, false);
                }))
                .on_scroll_wheel(cx.listener(move |p: &mut ScreenPane, e: &ScrollWheelEvent, cx| {
                    let Some(pic) = p.live_at.get() else { return };
                    let (x, y) = to_screen(pic, e.position);
                    if p.state.handle_scroll(x, y, e.delta) {
                        cx.stop_propagation();
                    }
                }));
        }
        // (Tab and ⇧Tab on a screen being controlled go to it, as every
        // key: `key` stops them before the window moves the keyboard on.)
        let picture = div()
            .id("live-picture")
            .track_focus(&self.live_focus)
            .flex_1()
            .min_h_0()
            .relative()
            .on_key_down(cx.listener(|p: &mut ScreenPane, e: &KeyDownEvent, cx| p.key(e.keystroke.clone(), true, e.is_held, cx)))
            .on_key_up(cx.listener(|p: &mut ScreenPane, e: &KeyUpEvent, cx| p.key(e.keystroke.clone(), false, false, cx)))
            .on_modifiers_changed(cx.listener(|p: &mut ScreenPane, e: &ModifiersChangedEvent, cx| {
                if p.state.handle_modifiers(&e.modifiers) {
                    cx.stop_propagation();
                }
            }))
            .child(&self.live)
            .children(well_overlay)
            .child(input);

        // What is going on, and the control button.
        let now = history::now_ms();
        let latest = self.replay.as_ref().and_then(|r| {
            let r = r.read(cx);
            let acts = r.actions();
            let a = acts.iter().rev().find(|a| !a.is_oversized())?;
            self.state.latest_action_is_current(a, acts.len()).then(|| a.clone())
        });
        let latest_runs = self.state.live_latest_runs(latest.as_ref(), now);
        let (mut status_text, primary_ink) = self.state.live_status_line(latest.as_ref(), now);
        // State each status once: when the Live well has no picture yet and
        // already displays the disconnected or loading overlay (or identical
        // placeholder sentence), leave the footer line quiet unless there is
        // an explicit version-skew/control note or a known `Latest:` action.
        let has_skew_note = !info.capabilities.can_control && info.capabilities.control_disabled_reason.is_some();
        if !has_shot && latest_runs.is_none() && !has_skew_note && (disconnected || is_loading || status_text == self.state.live_placeholder()) {
            status_text.clear();
        }
        let color = if primary_ink && !disconnected { c(p.panel_text) } else { style::secondary_text(&p) };
        let status = canvas(move |g, b| {
            if let Some((cmd, suffix)) = &latest_runs {
                let prefix = "Latest: ";
                let full: SharedString = format!("{prefix}{cmd}{suffix}").into();
                let ui = style::ui_font(FontWeight::NORMAL);
                let mono = style::mono_font(FontWeight::NORMAL);
                let runs = [
                    TextRun { len: prefix.len(), font: ui.clone(), color, background_color: None, underline: None, strikethrough: None, size: None },
                    TextRun { len: cmd.len(), font: mono, color, background_color: None, underline: None, strikethrough: None, size: None },
                    TextRun { len: suffix.len(), font: ui, color, background_color: None, underline: None, strikethrough: None, size: None },
                ];
                let t = style::label_runs_fit(g, b.left(), b.center().y, style::Align::Left, full.clone(), style::PLAIN_SIZE, &runs, b.size.width);
                agent::record(g, agent::Node::label(full, t));
                return;
            }
            if status_text.is_empty() {
                return;
            }
            let t = style::label_fit(g, b.left(), b.center().y, style::Align::Left, status_text.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), color, b.size.width);
            agent::record(g, agent::Node::label(status_text.clone(), t));
        })
        .flex_1()
        .min_w_0()
        .h_full();
        let can_control = !disconnected && info.capabilities.can_control && (info.id.is_local() || has_shot || controlling);
        let pending = self.state.control_pending;
        let control_label = if controlling {
            "Stop Controlling"
        } else if pending {
            "Taking Control…"
        } else {
            "Take Control"
        };
        let control = Button::new(control_label).id("take-control").min_width(140.0).pushed(controlling).enabled(can_control).on_click(cx.listener(|p: &mut ScreenPane, _, cx| {
            if p.state.controlling() || p.state.control_pending {
                p.stop_controlling(cx);
            } else {
                p.take_control(cx);
            }
        }));
        // Beside it, every window on the screen to one of the person's
        // display-workspaces.
        let offer_move = info.capabilities.can_move_windows && olo_kit::moving::menus_offer();
        let give = Button::new("Move Windows To…").id("give-windows").enabled(!self.state.giving).on_click(cx.listener(|p: &mut ScreenPane, _, cx| {
            p.learn_spaces();
            let menu = p.give_menu();
            let at = cx.mouse_position();
            p.menu_at(menu, at, cx);
        }));
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(picture)
            .child(div().flex().flex_row().items_center().gap(px(m::SPACING)).h(px(m::CONTROL_HEIGHT)).flex_shrink_0().child(status).children(offer_move.then_some(give)).child(control))
            .into_any_element()
    }

    /// A key on the picture being controlled: to the screen, but for the
    /// one that stops controlling (⌘K, respecting keyboard-profile overrides).
    fn key(&mut self, k: Keystroke, pressed: bool, held: bool, cx: &mut Cx<Self>) {
        if !self.state.controlling() {
            return;
        }
        if pressed && is_control_shortcut(&k, self.raw_shortcuts) {
            return;
        }
        cx.stop_propagation();
        self.state.handle_key(&k, pressed, held);
    }

    fn told(&mut self, msg: PaneMsg, cx: &mut Cx<Self>) {
        match msg {
            PaneMsg::LiveFrameReady => {
                self.live_frame_queued.store(false, Ordering::Release);
                if let Some((watch, conn_gen, shot)) = self.pending_live.lock().unwrap_or_else(|e| e.into_inner()).take()
                    && self.watching.as_ref().is_some_and(|(_, id, _, _)| *id == watch)
                    && self.live_gen.accept_frame(watch, conn_gen)
                {
                    let had_shot = self.live.read(cx).has_shot();
                    let prev_size = self.state.live_pixel_size;
                    self.state.failure = None;
                    self.state.live_status = LiveStatus::Streaming;
                    self.state.on_live_frame_size(shot.size[0], shot.size[1]);
                    self.live.update(cx, |w, cx| w.set_shot(shot, cx));
                    // B.1 (#5): tell the backend that the pane has accepted and
                    // displayed a live picture from this connection so remote
                    // Take Control can now be granted.
                    if let Some(src) = self.state.source() {
                        src.note_live_frame_accepted();
                    }
                    if !had_shot || prev_size != self.state.live_pixel_size {
                        let shape = self.state.info().and_then(|i| i.size).map(|(w, h)| (w as f32, h as f32));
                        self.live.update(cx, |w, _| w.set_shape(shape));
                        cx.invalidate();
                    }
                }
                return;
            }
            PaneMsg::LiveDmabuf { watch, conn_gen, buffer } => {
                if !self.watching.as_ref().is_some_and(|(_, id, _, _)| *id == watch) || !self.live_gen.accept_frame(watch, conn_gen) {
                    return (buffer.released)(buffer.id);
                }
                let had_shot = self.live.read(cx).has_shot();
                let prev_size = self.state.live_pixel_size;
                self.state.failure = None;
                self.state.live_status = LiveStatus::Streaming;
                self.state.on_live_frame_size(buffer.width as usize, buffer.height as usize);
                let size = (buffer.width, buffer.height);
                let key = self.live.update(cx, |w, cx| {
                    w.set_live(size, cx);
                    w.external_key()
                });
                cx.set_external_buffer(key, Some(buffer));
                if let Some(src) = self.state.source() {
                    src.note_live_frame_accepted();
                }
                if !had_shot || prev_size != self.state.live_pixel_size {
                    cx.invalidate();
                }
                return;
            }
            PaneMsg::LiveStatus { watch, conn_gen, status } => {
                if self.watching.as_ref().is_some_and(|(_, id, _, _)| *id == watch) {
                    let newer_shown = match &status {
                        LiveStatus::Disconnected(msg) => {
                            let stale = self.forget_live_retired(watch, conn_gen, cx);
                            if !stale {
                                self.live.update(cx, |w, cx| w.clear(Some(SharedString::from(msg.clone())), cx));
                            }
                            stale
                        }
                        // B.1 (#5, #10): clear ONLY the retired generation's
                        // pixels so a newer connection's already-queued first
                        // keyframe is never erased.
                        LiveStatus::Reconnecting => self.forget_live_retired(watch, conn_gen, cx),
                        _ => false,
                    };
                    if !newer_shown {
                        let id = self.state.screen_id().cloned();
                        if self.state.on_live_status(status)
                            && let Some(id) = id
                        {
                            cx.emit(ScreenPaneEvent::Controlling(id, false));
                        }
                    }
                }
            }
            PaneMsg::LiveControl { watch, held, reason } => {
                if self.watching.as_ref().is_some_and(|(_, id, _, _)| *id == watch) {
                    let prev_tab = self.state.tab();
                    let shown_tab = self.state.shown_tab();
                    let was_pending = self.state.control_pending;
                    let refusal = (!held && was_pending).then(|| reason.clone()).flatten();
                    let id = self.state.screen_id().cloned();
                    match self.state.on_remote_control(held, reason) {
                        Some(true) => {
                            if prev_tab != Tab::Live {
                                cx.emit(ScreenPaneEvent::TabChanged(Tab::Live));
                            }
                            self.sync_watch(cx);
                            self.sync_history(cx);
                            self.live_focus.focus(cx);
                            if let Some(id) = id {
                                cx.emit(ScreenPaneEvent::Controlling(id, true));
                            }
                        }
                        Some(false) => {
                            if let Some(id) = id {
                                cx.emit(ScreenPaneEvent::Controlling(id, false));
                            }
                        }
                        None => {
                            if let Some(msg) = refusal
                                && shown_tab != Tab::Live
                            {
                                self.alert(&format!("Couldn’t take control: {msg}"), cx);
                            }
                        }
                    }
                }
            }
            PaneMsg::LiveFailed { watch, conn_gen, message } => {
                if self.watching.as_ref().is_some_and(|(_, id, _, _)| *id == watch) {
                    let stale = self.forget_live_retired(watch, conn_gen, cx);
                    if !stale {
                        self.live.update(cx, |w, cx| w.clear(Some(SharedString::from(message.clone())), cx));
                        let id = self.state.screen_id().cloned();
                        if self.state.on_live_failed(message)
                            && let Some(id) = id
                        {
                            cx.emit(ScreenPaneEvent::Controlling(id, false));
                        }
                    }
                }
            }
            PaneMsg::Spaces(spaces) => self.state.spaces = spaces,
            PaneMsg::Shot(shot_id, Ok(path)) => {
                self.state.finish_shot(shot_id);
                let message = format!("The screenshot “{}” is saved.", path.file_name().and_then(|n| n.to_str()).unwrap_or(""));
                let me = cx.view_ref();
                open_alert(cx, "Take Screenshot", "screenshot", &message, &[olo_kit::tracker::LABEL, "OK"], move |choice, cx| {
                    if *choice == Some(0)
                        && let Err(e) = olo_kit::tracker::open(&path)
                    {
                        me.update(cx, move |p, cx| p.alert(&format!("Tracker couldn’t show it: {e}"), cx));
                    }
                });
            }
            PaneMsg::Shot(shot_id, Err(e)) => {
                self.state.finish_shot(shot_id);
                if e != "cancelled" {
                    self.alert(&format!("The screenshot couldn’t be made: {e}"), cx);
                }
            }
            PaneMsg::ExportTo { id, source, part, path, zoom } => {
                let Some(cancel) = self.state.begin_export(id.clone(), path.clone()) else {
                    return;
                };
                self.sync_history(cx);
                let me = self.me.clone();
                // (A name of its own: one started here unnamed takes the looper's,
                // `w>…`, and app_server makes a focused window's loopers realtime
                // by name: video encoding at realtime, Ben's export of 2026-11-15
                // ended the app.)
                let _ = std::thread::Builder::new().name("export".into()).spawn(move || {
                    let mut told = -1;
                    let result = source.export_video_cancellable(part.as_deref(), zoom, &path, &cancel, &mut |done| {
                        let pct = (done * 100.0) as i32;
                        if pct != told {
                            told = pct;
                            me.send(PaneMsg::ExportProgress { id: id.clone(), done });
                        }
                        !cancel.is_cancelled()
                    });
                    // Cleanup of temporary `.part` files belongs solely to their
                    // exclusive creator (`ExclusivePartFile`) — never unlink a
                    // guessed sibling filename here.
                    me.send(PaneMsg::Exported { id, result: result.map(|_| path) });
                });
            }
            PaneMsg::ExportProgress { id, done } => {
                if self.state.update_export_progress(&id, done)
                    && let Some(r) = &self.replay
                {
                    let ui = self.state.export_for_current();
                    r.update(cx, |r, cx| r.set_exporting(ui, cx));
                }
            }
            PaneMsg::Exported { id, result } => {
                let is_current = self.state.finish_export(&id);
                if is_current && let Some(r) = &self.replay {
                    r.update(cx, |r, cx| r.set_exporting(None, cx));
                }
                self.sync_history(cx);
                self.exported(&result, cx);
            }
            PaneMsg::Given(r) => {
                self.state.giving = false;
                match r {
                    Ok(stayed) if stayed.is_empty() => {}
                    Ok(stayed) => self.alert(&format!("Some windows stayed on the screen:\n{}", stayed.join("\n")), cx),
                    Err(e) => self.alert(&format!("The windows couldn't be moved: {e}"), cx),
                }
            }
            PaneMsg::Started(orig_id, r) => match r {
                Ok(info) => {
                    self.state.on_started_reply(&orig_id, Some(info.clone()));
                    cx.emit(ScreenPaneEvent::Started(info));
                }
                Err(e) => {
                    self.state.on_started_reply(&orig_id, None);
                    self.alert(&format!("The screen couldn't be started: {e}"), cx);
                }
            },
            PaneMsg::Stopped(id, r) => {
                if let Err(e) = r {
                    self.alert(&e, cx);
                } else {
                    if self.state.on_stopped_reply(&id) {
                        cx.emit(ScreenPaneEvent::Controlling(id.clone(), false));
                    }
                    cx.emit(ScreenPaneEvent::Stopped(id));
                }
            }
            PaneMsg::Renamed(id, source, captured_info_rev, title, r) => match r {
                Ok(()) => {
                    // B.8 (#26/#27): never let a delayed Rename reply overwrite
                    // newer Follow metadata or a replacement source under the same ID.
                    if self.state.matches_captured_session(&id, &source, None)
                        && (id.is_local() || self.state.info_rev == captured_info_rev)
                        && let Some(info) = self.state.info.as_mut()
                    {
                        info.title = title.clone();
                    }
                    cx.emit(ScreenPaneEvent::Renamed(id, title));
                }
                Err(e) => {
                    self.alert(&e, cx);
                    cx.emit(ScreenPaneEvent::RenameFailed(id));
                }
            },
            PaneMsg::Cleared(id, r) => {
                if let Err(e) = r {
                    self.alert(&e, cx);
                } else {
                    if self.state.screen_id() == Some(&id) {
                        self.replay = None;
                    }
                    cx.emit(ScreenPaneEvent::Cleared(id));
                }
            }
        }
        // A screen come or gone changes what Live and History show.
        self.sync_watch(cx);
        self.sync_history(cx);
        cx.invalidate();
    }
}

impl View for ScreenPane {
    type Message = PaneMsg;

    fn message(&mut self, msg: PaneMsg, cx: &mut Cx<Self>) {
        self.told(msg, cx);
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let Some(info) = self.state.info().cloned() else {
            return div().size_full().into_any_element();
        };
        self.sync_history(cx);
        let running = info.is_running();
        let pane_w = self.width.unwrap_or_else(|| f32::from(cx.viewport_size().width));
        // The tab's area: inside the pane's inset, then the tab view's.
        let area_w = (pane_w - 4.0 * m::WINDOW_INSET).max(120.0);
        if let Some(replay) = &self.replay {
            replay.update(cx, |r, cx| {
                if r.width != area_w || r.running != running {
                    r.width = area_w;
                    r.running = running;
                    cx.invalidate();
                }
            });
        }
        let header = self.header(cx);
        let tabs = self.state.tabs();
        let labels: Vec<SharedString> = tabs.iter().map(|t| SharedString::from(t.label())).collect();
        let shown = self.state.shown_tab();
        let i = tabs.iter().position(|t| *t == shown).unwrap_or(0);
        let show_disconnected_history = matches!(shown, Tab::History | Tab::Stats)
            && should_show_disconnected_for_history(
                info.capabilities.host_disconnected,
                self.replay.as_ref().map(|r| {
                    let r = r.read(cx);
                    (r.loaded(), r.actions().is_empty(), r.unavailable().is_some())
                }),
            );
        let content: Element = if show_disconnected_history {
            self.disconnected_panel(&info, false, cx)
        } else {
            match (shown, &self.replay) {
                (Tab::Live, _) => self.live_tab(cx),
                (Tab::History, Some(replay)) => replay.element(),
                (Tab::Stats, Some(replay)) => stats::view(replay, running, area_w, &self.stats_scroll, cx),
                (Tab::Passwords, _) => match &info.capabilities.passwords {
                    PasswordsMode::Local(screen_name) => {
                        if self.shares.as_ref().is_none_or(|(n, _)| n != screen_name) {
                            let for_screen = screen_name.clone();
                            self.shares = Some((screen_name.clone(), cx.new_view(|cx| Shares::new(Some(for_screen), cx))));
                        }
                        let (_, shares) = self.shares.as_ref().expect("made");
                        shares.update(cx, |s, cx| {
                            if s.width != area_w {
                                s.width = area_w;
                                cx.invalidate();
                            }
                        });
                        shares.element()
                    }
                    PasswordsMode::Unavailable(reason) => shares::unavailable_view(reason.clone()),
                },
                (_, None) => div().into_any_element(),
            }
        };
        let tab_view = TabView::new("screen-tabs", labels, i).on_select(cx.listener(move |p: &mut ScreenPane, i: &usize, cx| {
            if let Some(tab) = tabs.get(*i) {
                p.set_tab(*tab, cx);
            }
        }));
        div().size_full().p(px(m::WINDOW_INSET)).flex().flex_col().gap(px(m::SPACING)).child(header).child(div().flex_1().min_h_0().child(tab_view.child(content))).into_any_element()
    }
}

/// Whether `Tab::History` / `Tab::Stats` should render the disconnected panel
/// instead of `Replay` / `stats::view` (Agy-review R2 #3): when disconnected,
/// show `disconnected_panel` if no replay exists, if initial history never
/// finished loading (`!loaded`), or if it has no cached actions and holds a
/// disconnect/unavailable error — while preserving already-loaded cached
/// read-only History & Stats.
fn should_show_disconnected_for_history(host_disconnected: bool, replay_state: Option<(bool, bool, bool)>) -> bool {
    if !host_disconnected {
        return false;
    }
    match replay_state {
        None => true,
        Some((loaded, actions_empty, has_unavailable)) => !loaded || (actions_empty && has_unavailable),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Mutex;

    use olo_kit::Point;
    use olo_screen_codec::Frame;

    use super::*;
    use crate::source::{HistoryEvent, HistoryHandle, NoopHandle, PaneCapabilities};

    #[derive(Default)]
    struct MockLog {
        controlled_calls: Vec<bool>,
        inputs: Vec<Vec<SyntheticEvent>>,
        clipboard_joins: usize,
        clipboard_drops: usize,
        renames: Vec<Option<String>>,
    }

    struct ClipboardGuard(Arc<Mutex<MockLog>>);
    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            self.0.lock().unwrap().clipboard_drops += 1;
        }
    }

    struct MockSource {
        id: ScreenId,
        info: Mutex<ScreenInfo>,
        log: Arc<Mutex<MockLog>>,
        allow_clipboard: bool,
    }

    impl MockSource {
        fn local_running(name: &str) -> Arc<MockSource> {
            let id = ScreenId::local(name);
            let info = ScreenInfo {
                id: id.clone(),
                title: None,
                state: ScreenState::Running,
                size: Some((1440, 900)),
                scale: 1.0,
                sound: false,
                started_ms: None,
                last_used_ms: None,
                disconnected_at_ms: None,
                actions: None,
                controlled: false,
                capabilities: PaneCapabilities::local(name),
            };
            Arc::new(MockSource { id, info: Mutex::new(info), log: Arc::default(), allow_clipboard: true })
        }

        fn remote_running(host_id: &str, host_label: &str, name: &str, can_control: bool, reason: Option<&str>) -> Arc<MockSource> {
            let id = ScreenId::host(host_id, host_label, name);
            let mut caps = PaneCapabilities::remote_default();
            caps.can_control = can_control;
            caps.control_disabled_reason = reason.map(str::to_owned);
            let info = ScreenInfo {
                id: id.clone(),
                title: None,
                state: ScreenState::Running,
                size: Some((1280, 800)),
                scale: 1.0,
                sound: false,
                started_ms: Some(1_000),
                last_used_ms: None,
                disconnected_at_ms: None,
                actions: None,
                controlled: false,
                capabilities: caps,
            };
            Arc::new(MockSource { id, info: Mutex::new(info), log: Arc::default(), allow_clipboard: false })
        }
    }

    impl ScreenSource for MockSource {
        fn id(&self) -> &ScreenId {
            &self.id
        }
        fn info(&self) -> ScreenInfo {
            self.info.lock().unwrap().clone()
        }
        fn watch_live(&self, _: Vec<u64>, _: Box<dyn Fn(LiveEvent) + Send + Sync>) -> Box<dyn LiveHandle> {
            Box::new(NoopHandle)
        }
        fn set_controlled(&self, on: bool) -> Result<(), String> {
            self.log.lock().unwrap().controlled_calls.push(on);
            self.info.lock().unwrap().controlled = on;
            Ok(())
        }
        fn send_input(&self, events: Vec<SyntheticEvent>) {
            self.log.lock().unwrap().inputs.push(events);
        }
        fn join_clipboard(&self) -> Option<Box<dyn Send>> {
            if self.allow_clipboard {
                self.log.lock().unwrap().clipboard_joins += 1;
                Some(Box::new(ClipboardGuard(self.log.clone())))
            } else {
                None
            }
        }
        fn follow_history(&self, _: Box<dyn Fn(HistoryEvent) + Send + Sync>) -> Box<dyn HistoryHandle> {
            Box::new(NoopHandle)
        }
        fn frame_at(&self, _: u64) -> Result<Option<(u64, Frame)>, String> {
            Ok(None)
        }
        fn export_video(&self, _: Option<&[usize]>, _: usize, _: &Path, _: &mut dyn FnMut(f32) -> bool) -> Result<export::Made, String> {
            Err("mock".into())
        }
        fn start(&self) -> Result<ScreenInfo, String> {
            let mut info = self.info.lock().unwrap();
            info.state = ScreenState::Running;
            Ok(info.clone())
        }
        fn stop(&self) -> Result<(), String> {
            self.info.lock().unwrap().state = ScreenState::Stopped;
            Ok(())
        }
        fn clear_history(&self) -> Result<(), String> {
            Ok(())
        }
        fn rename(&self, title: Option<&str>) -> Result<(), String> {
            self.log.lock().unwrap().renames.push(title.map(str::to_owned));
            self.info.lock().unwrap().title = title.map(str::to_owned);
            Ok(())
        }
    }

    fn keystroke(key: &str, mods: Modifiers) -> Keystroke {
        Keystroke { key: key.into(), key_char: None, modifiers: mods }
    }

    #[test]
    fn keys_and_notches() {
        assert_eq!(evdev("a"), Some(30));
        assert_eq!(evdev("!"), Some(2));
        assert_eq!(evdev("?"), Some(53));
        assert_eq!(evdev("enter"), Some(28));
        assert_eq!(evdev("space"), Some(57));
        assert_eq!(evdev("f12"), Some(88));
        assert_eq!(evdev("insert"), Some(110));
        assert_eq!(evdev("é"), None);
        assert_eq!((notch(0.0), notch(-3.0), notch(0.5)), (0, -1, 1));
    }

    #[test]
    fn mock_source_drives_tabs_control_and_input_release() {
        let src = MockSource::local_running("agent");
        let mut pane = PaneState::new(Tab::Live);
        assert!(pane.set_source(Some(src.clone())));
        assert_eq!(pane.shown_tab(), Tab::Live);
        assert_eq!(pane.tabs(), vec![Tab::Live, Tab::History, Tab::Stats, Tab::Passwords]);
        assert_eq!(pane.should_watch_live(), Some(ScreenId::local("agent")));

        // Input is ignored while not controlling.
        assert!(!pane.handle_key(&keystroke("a", Modifiers::default()), true, false));
        assert!(!pane.handle_pointer_button(10.0, 20.0, screens::LEFT, true));
        assert!(src.log.lock().unwrap().inputs.is_empty());

        // Take Control locks screen, joins local clipboard, and forwards input.
        assert_eq!(pane.take_control(), Ok(ControlAcquire::Acquired));
        assert!(pane.controlling());
        assert_eq!(src.log.lock().unwrap().controlled_calls, vec![true]);
        assert_eq!(src.log.lock().unwrap().clipboard_joins, 1);

        // Press Ctrl+A and mouse button down, plus a wheel scroll.
        let ctrl = Modifiers { control: true, ..Default::default() };
        assert!(pane.handle_key(&keystroke("a", ctrl), true, false));
        // Held repeat is suppressed (screen repeats keys itself).
        assert!(pane.handle_key(&keystroke("a", ctrl), true, true));
        assert!(pane.handle_pointer_button(50.0, 60.0, screens::LEFT, true));
        assert!(pane.handle_scroll(50.0, 60.0, ScrollDelta::Lines(Point { x: 0.0, y: -2.0 })));

        // Virtual-keyboard shifted character without Shift modifier presses & releases Shift around key.
        assert!(pane.handle_key(&keystroke("!", Modifiers::default()), true, false));

        // Switching tab away from Live releases control, drops clipboard, and sends up events for held key/button.
        assert!(pane.set_tab(Tab::History));
        assert!(!pane.controlling());
        assert_eq!(pane.should_watch_live(), None);
        let log = src.log.lock().unwrap();
        assert_eq!(log.controlled_calls, vec![true, false]);
        assert_eq!(log.clipboard_drops, 1);
        let last = log.inputs.last().unwrap();
        assert!(last.contains(&SyntheticEvent::Key { key: 30, pressed: false }));
        assert!(last.contains(&SyntheticEvent::Button { button: screens::LEFT, pressed: false }));
    }

    #[test]
    fn stopped_screen_falls_back_to_history_preserving_tab_preference() {
        let src = MockSource::local_running("agent");
        let mut pane = PaneState::new(Tab::Live);
        pane.set_source(Some(src.clone()));
        assert_eq!(pane.take_control(), Ok(ControlAcquire::Acquired));

        let mut stopped = src.info();
        stopped.state = ScreenState::Stopped;
        assert!(pane.update_info(stopped));
        assert!(!pane.controlling());
        assert_eq!(pane.tab(), Tab::Live);
        assert_eq!(pane.shown_tab(), Tab::History);
        assert_eq!(pane.tabs(), vec![Tab::History, Tab::Stats, Tab::Passwords]);
        assert_eq!(pane.should_watch_live(), None);
        let ms = pane.menu_state(true, true, false, false, true, false);
        assert!(!ms.running && ms.can_clear && !ms.can_control && ms.can_export && ms.can_screenshot);
    }

    #[test]
    fn remote_screen_version_skew_disables_control_and_never_joins_clipboard() {
        let skewed = MockSource::remote_running("h1", "build-box", "qa", false, Some("Update Olo on this host to take control."));
        let mut pane = PaneState::new(Tab::Live);
        pane.set_source(Some(skewed.clone()));

        assert_eq!(pane.take_control(), Ok(ControlAcquire::Noop));
        assert!(!pane.controlling());
        assert!(skewed.log.lock().unwrap().controlled_calls.is_empty());
        assert_eq!(skewed.log.lock().unwrap().clipboard_joins, 0);

        let (status, primary) = pane.live_status_line(None, 10_000);
        assert_eq!(status, "Update Olo on this host to take control.");
        assert!(!primary);
        let ms = pane.menu_state(true, false, false, false, false, false);
        assert!(ms.running && !ms.can_control && !ms.can_move_windows && !ms.can_airplay);

        let remote_sub = pane.header_subtitle(3, None, None, 10_000);
        assert_eq!(remote_sub, "Running on build-box · up 9 s · 3 actions · 1280 × 800");

        // Remote screen with known action count (lazy history unopened) leaves footer quiet
        // instead of showing "Watching." or contradicting "3 actions" with "No actions yet".
        let mut with_acts = MockSource::remote_running("h1", "build-box", "qa-acts", true, None).info();
        with_acts.actions = Some(3);
        let mut pane_acts = PaneState::new(Tab::Live);
        pane_acts.set_source(Some(MockSource::remote_running("h1", "build-box", "qa-acts", true, None)));
        pane_acts.update_info(with_acts);
        assert_eq!(pane_acts.live_status_line(None, 10_000).0, "");

        // Zero-action screen shows "No actions yet" uniformly on local and remote.
        let mut pane_empty = PaneState::new(Tab::Live);
        pane_empty.set_source(Some(MockSource::remote_running("h1", "build-box", "qa-empty", true, None)));
        assert_eq!(pane_empty.live_status_line(None, 10_000).0, "No actions yet");

        // Finding 24: local screen allows Take Control from History before Live opens,
        // while remote screen still gates on first live frame.
        let mut local_hist = PaneState::new(Tab::History);
        local_hist.set_source(Some(MockSource::local_running("agent")));
        assert!(local_hist.menu_state(true, false, false, false, false, false).can_control, "local Take Control must stay enabled on History");

        // Take Control stays disabled in menu_state until the first live frame arrives on remote.
        let ok_remote = MockSource::remote_running("h1", "build-box", "qa-2", true, None);
        pane.set_source(Some(ok_remote.clone()));
        assert!(!pane.menu_state(false, false, false, false, false, false).can_control, "Take Control disabled while loading");
        assert!(pane.menu_state(false, false, false, false, false, true).can_control, "Take Control enabled once picture arrives");

        // Finding 30: explicit non-empty LiveStatus::Message is NOT swallowed into generic loading overlay.
        pane.on_live_status(LiveStatus::Message("Waiting for host display".into()));
        assert!(!pane.is_live_loading(false));

        // Disconnected host screen reports "Last seen running on … <ago>" with NO dot inside the sentence.
        let mut disc = ok_remote.info();
        disc.capabilities.host_disconnected = true;
        let now = history::now_ms();
        disc.disconnected_at_ms = Some(now.saturating_sub(25 * 60_000));
        pane.update_info(disc);
        let disc_sub = pane.header_subtitle(0, None, None, now);
        assert_eq!(disc_sub, "Last seen running on build-box 25 min ago · 1280 × 800");
        assert!(!pane.menu_state(false, false, false, false, false, true).can_control);

        // Reconnect and verify Take Control acquires and releases cleanly without touching clipboard.
        let ok_remote2 = MockSource::remote_running("h1", "build-box", "qa-2", true, None);
        pane.set_source(Some(ok_remote2.clone()));
        assert_eq!(pane.take_control(), Ok(ControlAcquire::Acquired));
        assert_eq!(ok_remote2.log.lock().unwrap().clipboard_joins, 0);
        drop(pane);
        assert_eq!(ok_remote2.log.lock().unwrap().controlled_calls, vec![true, false]);
    }

    #[test]
    fn local_header_omits_host_and_rename_normalizes() {
        let src = MockSource::local_running("agent");
        let mut pane = PaneState::new(Tab::Live);
        pane.set_source(Some(src.clone()));
        // Final pass A.2: no-hosts local screens share the unified order
        // ("Running · up T · N actions · W × H"), dropping resolution first and
        // omitting "last …" (which lives in the Live footer).
        assert_eq!(pane.header_subtitle(0, None, None, 10_000), "Running · 1440 × 900");
        assert_eq!(pane.header_subtitle(1, None, Some(1_000), 10_000), "Running · up 9 s · 1 action · 1440 × 900");
        // Once any remote host is listed (`show_location = true`), adds "on this computer".
        let mut with_loc = src.info();
        with_loc.capabilities.show_location = true;
        pane.update_info(with_loc);
        assert_eq!(pane.header_subtitle(2, None, Some(1_000), 10_000), "Running on this computer · up 9 s · 2 actions · 1440 × 900");
        assert_eq!(pane.normalize_rename("  Browser QA  "), Some("Browser QA".into()));
        assert_eq!(pane.normalize_rename("   "), None);
        assert_eq!(pane.normalize_rename("agent"), None);
    }

    #[test]
    fn remote_control_refusal_shows_host_words_and_visibility_pauses_live() {
        let remote = MockSource::remote_running("h1", "build-box", "qa", true, None);
        let mut pane = PaneState::new(Tab::Live);
        pane.set_source(Some(remote.clone()));
        assert_eq!(pane.should_watch_live(), Some(remote.id().clone()));
        assert_eq!(pane.running_screen_id(), Some(remote.id().clone()));

        // Hiding the pane pauses live without dropping the running screen identity.
        assert!(!pane.set_visible(false));
        assert_eq!(pane.should_watch_live(), None);
        assert_eq!(pane.running_screen_id(), Some(remote.id().clone()));
        assert!(!pane.set_visible(true));
        assert_eq!(pane.should_watch_live(), Some(remote.id().clone()));

        // Take Control, then host refuses with a Status message: pane drops out
        // of controlling without sending redundant `set_controlled(false)` and
        // shows the host's words where control unavailability is explained.
        assert_eq!(pane.take_control(), Ok(ControlAcquire::Acquired));
        assert!(pane.controlling());
        assert_eq!(pane.on_remote_control(false, Some("Someone else is controlling this screen.".into())), Some(false));
        assert!(!pane.controlling());
        assert_eq!(remote.log.lock().unwrap().controlled_calls, vec![true]);
        let (status, primary) = pane.live_status_line(None, 10_000);
        assert_eq!(status, "Someone else is controlling this screen.");
        assert!(!primary);
    }

    #[test]
    fn astra_f13_screen_id_ignores_host_label_and_replaced_source_releases_old() {
        // Host label is display metadata, not identity.
        let id_a = ScreenId::host("h1", "Workstation A", "qa");
        let id_b = ScreenId::host("h1", "Workstation B", "qa");
        assert_eq!(id_a, id_b, "ScreenId equality must depend on host_id + name, not host_label");

        // Replacing source with a new Arc of the same ScreenId while controlling
        // must tear down control on the old source and report `changed == true`.
        let s1 = MockSource::local_running("agent");
        let s2 = MockSource::local_running("agent");
        let mut pane = PaneState::new(Tab::Live);
        assert!(pane.set_source(Some(s1.clone())));
        assert_eq!(pane.take_control(), Ok(ControlAcquire::Acquired));
        assert!(pane.controlling());

        assert!(pane.set_source(Some(s2.clone())), "new source instance with same ID must count as changed");
        assert!(!pane.controlling(), "replacing source must release old control binding");
        assert_eq!(s1.log.lock().unwrap().controlled_calls, vec![true, false]);
        assert!(s2.log.lock().unwrap().controlled_calls.is_empty());
    }

    #[test]
    fn astra_f14_remote_default_disables_control_until_host_verified() {
        let caps = PaneCapabilities::remote_default();
        assert!(!caps.can_control, "unknown host version must default to can_control = false");
        assert!(caps.control_disabled_reason.is_some());
    }

    #[test]
    fn astra_f15_failed_take_control_keeps_history_tab() {
        struct RefusingSource(MockSource);
        impl ScreenSource for RefusingSource {
            fn id(&self) -> &ScreenId {
                self.0.id()
            }
            fn info(&self) -> ScreenInfo {
                self.0.info()
            }
            fn watch_live(&self, m: Vec<u64>, cb: Box<dyn Fn(LiveEvent) + Send + Sync>) -> Box<dyn LiveHandle> {
                self.0.watch_live(m, cb)
            }
            fn set_controlled(&self, on: bool) -> Result<(), String> {
                if on { Err("Someone else is controlling this screen.".into()) } else { Ok(()) }
            }
            fn send_input(&self, e: Vec<SyntheticEvent>) {
                self.0.send_input(e)
            }
            fn join_clipboard(&self) -> Option<Box<dyn Send>> {
                None
            }
            fn follow_history(&self, cb: Box<dyn Fn(HistoryEvent) + Send + Sync>) -> Box<dyn HistoryHandle> {
                self.0.follow_history(cb)
            }
            fn frame_at(&self, at: u64) -> Result<Option<(u64, Frame)>, String> {
                self.0.frame_at(at)
            }
            fn export_video(&self, a: Option<&[usize]>, z: usize, p: &Path, cb: &mut dyn FnMut(f32) -> bool) -> Result<export::Made, String> {
                self.0.export_video(a, z, p, cb)
            }
            fn start(&self) -> Result<ScreenInfo, String> {
                self.0.start()
            }
            fn stop(&self) -> Result<(), String> {
                self.0.stop()
            }
            fn clear_history(&self) -> Result<(), String> {
                self.0.clear_history()
            }
            fn rename(&self, t: Option<&str>) -> Result<(), String> {
                self.0.rename(t)
            }
        }
        let inner = MockSource::local_running("agent");
        let refusing = Arc::new(RefusingSource(MockSource { id: inner.id.clone(), info: Mutex::new(inner.info()), log: Arc::default(), allow_clipboard: false }));
        let mut pane = PaneState::new(Tab::History);
        pane.set_source(Some(refusing));
        assert_eq!(pane.tab(), Tab::History);
        assert!(pane.take_control().is_err());
        assert_eq!(pane.tab(), Tab::History, "failed take_control from History must not leave Live selected");

        // LiveStatus::Disconnected and LiveFailed must both disarm active control
        // AND explicitly release control on the source.
        let ok_src = MockSource::local_running("agent");
        pane.set_source(Some(ok_src.clone()));
        assert_eq!(pane.take_control(), Ok(ControlAcquire::Acquired));
        assert!(pane.on_live_status(LiveStatus::Disconnected("Connection lost.".into())));
        assert!(!pane.controlling());
        let (status, primary) = pane.live_status_line(None, 10_000);
        assert_eq!(status, "Connection lost.");
        assert!(!primary);

        assert_eq!(pane.take_control(), Ok(ControlAcquire::Acquired));
        assert!(pane.on_live_failed("Stream closed: bad tile".into()));
        assert!(!pane.controlling());
        let (status, _) = pane.live_status_line(None, 10_000);
        assert_eq!(status, "Stream closed: bad tile");
        assert_eq!(ok_src.log.lock().unwrap().controlled_calls, vec![true, false, true, false]);
    }

    #[test]
    fn astra_f1_control_shortcut_matches_directly() {
        let plain_a = keystroke("a", Modifiers::default());
        assert!(!is_control_shortcut(&plain_a, false));
        let cmd_k = keystroke("k", Modifiers { control: true, ..Default::default() });
        assert!(is_control_shortcut(&cmd_k, false));
        let shift_cmd_k = keystroke("k", Modifiers { control: true, shift: true, ..Default::default() });
        assert!(!is_control_shortcut(&shift_cmd_k, false));
    }

    #[test]
    fn astra_f10_and_f17_exports_survive_switch_and_late_lifecycle_replies_are_guarded() {
        let a = MockSource::local_running("screen-a");
        let b = MockSource::local_running("screen-b");
        let mut pane = PaneState::new(Tab::History);
        pane.set_source(Some(a.clone()));

        // Start export on A -> duplicate export blocked -> switch to B -> switch back to A -> progress & cancel intact.
        let cancel_a = pane.begin_export(a.id().clone(), PathBuf::from("/var/tmp/a.mp4")).expect("first export");
        assert!(pane.begin_export(a.id().clone(), PathBuf::from("/var/tmp/a.mp4")).is_none(), "duplicate export on A blocked");
        assert!(pane.update_export_progress(a.id(), 0.45));
        pane.set_source(Some(b.clone()));
        assert!(!pane.is_exporting_current());
        assert!(!pane.update_export_progress(a.id(), 0.70));
        pane.set_source(Some(a.clone()));
        let (done, reconnected_cancel) = pane.export_for_current().expect("export reconnected on return to A");
        assert!((done - 0.70).abs() < 1e-6);
        assert!(Arc::ptr_eq(&cancel_a.flag(), &reconnected_cancel.flag()));
        assert!(pane.finish_export(a.id()));
        assert!(!pane.is_exporting_current());

        // Late Started(A) arriving while viewing B's History must NOT switch B to Live.
        pane.set_source(Some(b.clone()));
        pane.set_tab(Tab::History);
        assert!(!pane.on_started_reply(a.id(), Some(a.info())));
        assert_eq!(pane.tab(), Tab::History);

        // Late Stopped(B) arriving while controlling B releases B's control cleanly.
        assert_eq!(pane.take_control(), Ok(ControlAcquire::Acquired));
        assert!(pane.controlling());
        assert!(pane.on_stopped_reply(b.id()));
        assert!(!pane.controlling());
        assert_eq!(pane.info().unwrap().state, ScreenState::Stopped);
    }

    #[test]
    fn review2_f7_f37_f47_f56_async_control_reconnect_release_geometry_and_live_shot() {
        struct AsyncRemote(MockSource);
        impl ScreenSource for AsyncRemote {
            fn id(&self) -> &ScreenId {
                self.0.id()
            }
            fn info(&self) -> ScreenInfo {
                self.0.info()
            }
            fn watch_live(&self, m: Vec<u64>, cb: Box<dyn Fn(LiveEvent) + Send + Sync>) -> Box<dyn LiveHandle> {
                self.0.watch_live(m, cb)
            }
            fn async_control(&self) -> bool {
                true
            }
            fn set_controlled(&self, on: bool) -> Result<(), String> {
                self.0.set_controlled(on)
            }
            fn send_input(&self, e: Vec<SyntheticEvent>) {
                self.0.send_input(e)
            }
            fn join_clipboard(&self) -> Option<Box<dyn Send>> {
                None
            }
            fn follow_history(&self, cb: Box<dyn Fn(HistoryEvent) + Send + Sync>) -> Box<dyn HistoryHandle> {
                self.0.follow_history(cb)
            }
            fn frame_at(&self, at: u64) -> Result<Option<(u64, Frame)>, String> {
                self.0.frame_at(at)
            }
            fn export_video(&self, a: Option<&[usize]>, z: usize, p: &Path, cb: &mut dyn FnMut(f32) -> bool) -> Result<export::Made, String> {
                self.0.export_video(a, z, p, cb)
            }
            fn start(&self) -> Result<ScreenInfo, String> {
                self.0.start()
            }
            fn stop(&self) -> Result<(), String> {
                self.0.stop()
            }
            fn clear_history(&self) -> Result<(), String> {
                self.0.clear_history()
            }
            fn rename(&self, t: Option<&str>) -> Result<(), String> {
                self.0.rename(t)
            }
        }
        let inner = MockSource::remote_running("h1", "box", "qa", true, None);
        let log = inner.log.clone();
        let remote = Arc::new(AsyncRemote(MockSource { id: inner.id.clone(), info: Mutex::new(inner.info()), log: log.clone(), allow_clipboard: false }));
        let mut pane = PaneState::new(Tab::History);
        pane.set_source(Some(remote));

        // Finding #37: async Take Control stays pending on History and ignores input until host grants.
        assert_eq!(pane.take_control(), Ok(ControlAcquire::Pending));
        assert!(!pane.controlling());
        assert_eq!(pane.tab(), Tab::History);
        assert!(!pane.handle_key(&keystroke("a", Modifiers::default()), true, false));
        assert!(log.lock().unwrap().inputs.is_empty());

        // Host grants control -> pane commits Tab::Live & controlling.
        assert_eq!(pane.on_remote_control(true, None), Some(true));
        assert!(pane.controlling());
        assert_eq!(pane.tab(), Tab::Live);

        // Finding #7: Reconnecting status MUST explicitly release control on source!
        assert!(pane.on_live_status(LiveStatus::Reconnecting));
        assert!(!pane.controlling());
        assert_eq!(log.lock().unwrap().controlled_calls, vec![true, false]);

        // Finding #47: live frame resize updates logical pointer coordinates immediately.
        assert_eq!(pane.live_logical_size(), (1280.0, 800.0));
        pane.on_live_frame_size(1920, 1080);
        assert_eq!(pane.live_logical_size(), (1920.0, 1080.0));
        assert_eq!(pane.info().unwrap().size, Some((1920, 1080)));

        // Finding #56: on Tab::Live, Take Screenshot is available as soon as a live shot arrives
        // even when Replay has not loaded any history yet.
        let ms_before = pane.menu_state(false, false, false, false, false, false);
        assert!(!ms_before.can_screenshot);
        let ms_live = pane.menu_state(false, false, false, false, false, true);
        assert!(ms_live.can_screenshot, "Live screenshot must be enabled once live picture arrives");
        pane.set_tab(Tab::History);
        let ms_hist = pane.menu_state(false, false, false, false, false, true);
        assert!(!ms_hist.can_screenshot, "History screenshot must require loaded history frames");

        // Finding #17 & #32: dropping PaneState fires cancel hooks for exports and screenshots.
        let exp_cancel = pane.begin_export(ScreenId::host("h1", "box", "qa"), PathBuf::from("/var/tmp/x.mp4")).unwrap();
        let (_, shot_cancel) = pane.register_shot();
        let exp_hook = Arc::new(AtomicBool::new(false));
        let shot_hook = Arc::new(AtomicBool::new(false));
        let eh = exp_hook.clone();
        let sh = shot_hook.clone();
        exp_cancel.register_hook(move || eh.store(true, Ordering::SeqCst));
        shot_cancel.register_hook(move || sh.store(true, Ordering::SeqCst));
        drop(pane);
        assert!(exp_hook.load(Ordering::Acquire), "dropping PaneState must fire export cancel hook");
        assert!(shot_hook.load(Ordering::Acquire), "dropping PaneState must fire screenshot cancel hook");
    }

    #[test]
    fn lazy_remote_history_only_follows_on_history_stats_or_export() {
        struct LazyRemote(MockSource);
        impl ScreenSource for LazyRemote {
            fn id(&self) -> &ScreenId {
                self.0.id()
            }
            fn info(&self) -> ScreenInfo {
                self.0.info()
            }
            fn watch_live(&self, m: Vec<u64>, cb: Box<dyn Fn(LiveEvent) + Send + Sync>) -> Box<dyn LiveHandle> {
                self.0.watch_live(m, cb)
            }
            fn set_controlled(&self, on: bool) -> Result<(), String> {
                self.0.set_controlled(on)
            }
            fn send_input(&self, e: Vec<SyntheticEvent>) {
                self.0.send_input(e)
            }
            fn join_clipboard(&self) -> Option<Box<dyn Send>> {
                None
            }
            fn lazy_history(&self) -> bool {
                true
            }
            fn follow_history(&self, cb: Box<dyn Fn(HistoryEvent) + Send + Sync>) -> Box<dyn HistoryHandle> {
                self.0.follow_history(cb)
            }
            fn frame_at(&self, at: u64) -> Result<Option<(u64, Frame)>, String> {
                self.0.frame_at(at)
            }
            fn export_video(&self, a: Option<&[usize]>, z: usize, p: &Path, cb: &mut dyn FnMut(f32) -> bool) -> Result<export::Made, String> {
                self.0.export_video(a, z, p, cb)
            }
            fn start(&self) -> Result<ScreenInfo, String> {
                self.0.start()
            }
            fn stop(&self) -> Result<(), String> {
                self.0.stop()
            }
            fn clear_history(&self) -> Result<(), String> {
                self.0.clear_history()
            }
            fn rename(&self, t: Option<&str>) -> Result<(), String> {
                self.0.rename(t)
            }
        }
        let inner = MockSource::remote_running("h1", "box", "qa", true, None);
        let remote = Arc::new(LazyRemote(MockSource { id: inner.id.clone(), info: Mutex::new(inner.info()), log: Arc::default(), allow_clipboard: false }));
        let mut pane = PaneState::new(Tab::Live);
        pane.set_source(Some(remote.clone()));
        assert!(!pane.should_follow_history(), "Live tab must not open remote history");
        pane.set_tab(Tab::Passwords);
        assert!(!pane.should_follow_history(), "Passwords tab must not open remote history");
        pane.set_tab(Tab::History);
        assert!(pane.should_follow_history(), "History tab must open remote history");
        pane.set_tab(Tab::Stats);
        assert!(pane.should_follow_history(), "Stats tab must open remote history");
        pane.set_visible(false);
        assert!(!pane.should_follow_history(), "hidden pane must close remote history when not exporting");
        pane.begin_export(remote.id().clone(), PathBuf::from("/var/tmp/qa.mp4")).unwrap();
        assert!(pane.should_follow_history(), "active export keeps remote history active");
        pane.finish_export(remote.id());
        assert!(!pane.should_follow_history());
    }

    #[test]
    fn warning_status_shows_without_dropping_control_and_discontiguous_export_gated() {
        let remote = MockSource::remote_running("h1", "box", "qa", true, None);
        let mut pane = PaneState::new(Tab::Live);
        pane.set_source(Some(remote.clone()));

        // Warning while watching shows message and never revokes control.
        assert!(!pane.on_live_status(LiveStatus::Warning("Couldn't capture the screen: busy".into())));
        let (status, primary) = pane.live_status_line(None, 10_000);
        assert_eq!(status, "Couldn't capture the screen: busy");
        assert!(!primary);
        assert_eq!(pane.live_placeholder(), "Couldn't capture the screen: busy");

        // Warning while controlling stays in control AND keeps warning visible.
        assert_eq!(pane.take_control(), Ok(ControlAcquire::Acquired));
        assert!(pane.controlling());
        assert!(!pane.on_live_status(LiveStatus::Warning("Screen picture exceeds limit.".into())));
        assert!(pane.controlling(), "Warning must never drop control");
        let (status, primary) = pane.live_status_line(None, 10_000);
        assert_eq!(status, "You have control — Screen picture exceeds limit.");
        assert!(primary);

        // Wheel notches forward as SyntheticEvent::Wheel { dx, dy }.
        assert!(pane.handle_scroll(100.0, 200.0, ScrollDelta::Lines(Point { x: 1.0, y: -3.0 })));
        let last_in = remote.log.lock().unwrap().inputs.last().cloned().unwrap();
        assert_eq!(last_in, vec![SyntheticEvent::Move { x: 100.0, y: 200.0 }, SyntheticEvent::Wheel { dx: -1, dy: 1 }]);

        // Older host lacking `can_export_discontiguous`: contiguous export offered, discontiguous NOT offered.
        assert!(pane.can_export_selection(false));
        assert!(!pane.can_export_selection(true), "discontiguous export must not be offered when host lacks capability");

        // Modern host with `can_export_discontiguous`: discontiguous export IS offered.
        let mut modern_info = remote.info();
        modern_info.capabilities = PaneCapabilities::remote_with_control();
        assert!(pane.update_info(modern_info));
        assert!(pane.can_export_selection(true));
    }

    #[test]
    fn round3_f5_f6_f10_f24_f28_disconnect_visibility_and_frame_gate() {
        // Finding 6: set_visible(false) releases control, drops clipboard, and pauses live.
        let local = MockSource::local_running("local-a");
        let mut pane = PaneState::new(Tab::Live);
        pane.set_source(Some(local.clone()));
        assert_eq!(pane.take_control(), Ok(ControlAcquire::Acquired));
        assert!(pane.controlling());
        assert!(pane.set_visible(false), "hiding while controlling must report change");
        assert!(!pane.controlling(), "hiding pane must release control");
        assert_eq!(pane.should_watch_live(), None, "hiding pane must stop live watching");
        assert_eq!(local.log.lock().unwrap().controlled_calls, vec![true, false]);

        // Finding 10 & 28: host_disconnected stops live watching and prevents history follow;
        // reconnecting restores should_watch_live().
        let remote = MockSource::remote_running("h1", "build-farm", "qa", true, None);
        let mut rpane = PaneState::new(Tab::Live);
        rpane.set_source(Some(remote.clone()));
        assert_eq!(rpane.should_watch_live(), Some(remote.id().clone()));
        let mut disc = remote.info();
        disc.capabilities.host_disconnected = true;
        rpane.update_info(disc);
        assert_eq!(rpane.should_watch_live(), None, "disconnected host must not watch live");
        assert!(!rpane.should_follow_history(), "disconnected host must not follow history");
        // Reconnect resumes watch_live cleanly.
        rpane.update_info(remote.info());
        assert_eq!(rpane.should_watch_live(), Some(remote.id().clone()));

        // Finding 28: stopped remote screen while offline defaults to History and reports host_disconnected.
        let mut stopped_disc = remote.info();
        stopped_disc.state = ScreenState::Stopped;
        stopped_disc.capabilities.host_disconnected = true;
        rpane.update_info(stopped_disc);
        assert_eq!(rpane.shown_tab(), Tab::History);
        assert!(rpane.info().unwrap().capabilities.host_disconnected);
        assert!(!rpane.should_follow_history());
    }

    #[test]
    fn final_pass_a4_b9_loading_disconnected_footer_and_stale_replay_latest() {
        let now = history::now_ms();
        let act_at = now.saturating_sub(60_050);
        let act_end = act_at + 50;
        let remote = MockSource::remote_running("h1", "dev-box", "qa", true, None);
        let mut info = remote.info();
        info.actions = Some(3);
        info.last_used_ms = Some(act_end);
        let mut pane = PaneState::new(Tab::Live);
        pane.set_source(Some(remote.clone()));
        pane.update_info(info.clone());

        let act3 = history::Action { at: act_at, ms: 50, kind: "click".into(), args: vec!["810".into(), "420".into()], result: None, error: None };

        // A.4: While loading (`LiveStatus::Connecting`), shows `Latest: ACTION · TIME` as soon as known.
        assert_eq!(pane.live_status, LiveStatus::Connecting);
        assert!(pane.latest_action_is_current(&act3, 3));
        let (cmd, suf) = pane.live_latest_runs(Some(&act3), now).expect("loading footer must show known latest");
        assert_eq!(cmd, "click 810 420");
        assert_eq!(suf, " · 1 min ago");

        // Agy-review R1 #7: `header_subtitle` uptime & `live_latest_runs` / `"Last seen"` relative times
        // format through `olo_kit::ago::age` (scheduling `holds_until` wakeups while keeping `now_ms` deterministic).
        let syn_act = history::Action { at: 4_000, ms: 50, kind: "click".into(), args: vec!["10".into(), "20".into()], result: None, error: None };
        assert_eq!(pane.live_latest_runs(Some(&syn_act), 10_000).unwrap().1, " · just now");
        assert_eq!(pane.live_latest_runs(Some(&syn_act), 70_050).unwrap().1, " · 1 min ago");
        let up_fmt = |a: Duration| format!("up {}", stats::span(a.as_millis() as u64));
        assert_eq!(olo_kit::ago::until_change(Duration::from_secs(9), &up_fmt, &up_fmt(Duration::from_secs(9))), Duration::from_secs(1));
        assert_eq!(olo_kit::ago::until_change(Duration::from_secs(61), &up_fmt, &up_fmt(Duration::from_secs(61))), Duration::from_secs(59));
        assert_eq!(olo_kit::ago::until_change(Duration::from_secs(3), &stats::ago_words, &stats::ago_words(Duration::from_secs(3))), Duration::from_secs(7));

        // A.4: While disconnected (`host_disconnected = true`), still shows last known `Latest:`
        // (painted in secondary ink by `live_tab`), and returns None only when nothing is known.
        let mut disc = info.clone();
        disc.capabilities.host_disconnected = true;
        pane.update_info(disc);
        let (cmd_d, suf_d) = pane.live_latest_runs(Some(&act3), now).expect("disconnected footer must show last known action");
        assert_eq!(cmd_d, "click 810 420");
        assert_eq!(suf_d, " · 1 min ago");
        assert_eq!(pane.live_latest_runs(None, now), None);

        // B.9 (#30): once host metadata advances (`last_used_ms` OR `actions` count) beyond the
        // suspended replay's action 3, `latest_action_is_current` returns false so action 3 is
        // NEVER labelled "Latest", and header subtitle uses the newer count `4 actions`.
        let mut newer_ts = info.clone();
        newer_ts.last_used_ms = Some(act_end + 10_000);
        pane.update_info(newer_ts);
        assert!(!pane.latest_action_is_current(&act3, 3), "stale replay timestamp must not be labelled Latest");

        let mut newer_count = info.clone();
        newer_count.actions = Some(4);
        pane.update_info(newer_count);
        assert!(!pane.latest_action_is_current(&act3, 3), "stale replay count must not be labelled Latest");
        let sub = pane.header_subtitle(3, Some(act_end), None, now);
        assert!(sub.contains("4 actions"), "header must show newest action count, got: {sub}");
    }

    #[test]
    fn final_pass_b1_b2_b3_live_gen_race_hidden_control_stop_binding_and_capture() {
        // B.1 (#5, #10): `Reconnecting` for retired gen 1 arriving AFTER `pending_live`
        // already holds gen 2's first keyframe MUST preserve gen 2 and reject stale gen 1.
        let pending: PendingLiveFrame = Arc::new(olo_sync::Mutex::new(None));
        let mut tracker = LiveGenTracker::default();
        let shot2 = Arc::new(Shot { size: [16, 16], rgba: vec![2; 16 * 16 * 4] });

        assert!(tracker.accept_frame(7, 1));
        // Delivery worker queued Reconnecting(gen=1), then new conn put (watch=7, gen=2, shot2) in pending_live.
        *pending.lock().unwrap() = Some((7, 2, shot2.clone()));
        let (clear_displayed, newer_shown) = tracker.retire(&pending, 7, 1);
        assert!(clear_displayed, "old gen-1 displayed picture must be cleared");
        assert!(!newer_shown);
        // Gen-2 keyframe in pending_live MUST still be present!
        let taken = pending.lock().unwrap().take().expect("gen-2 keyframe must survive gen-1 Reconnecting");
        assert_eq!((taken.0, taken.1), (7, 2));
        assert!(tracker.accept_frame(taken.0, taken.1), "gen-2 keyframe must be accepted");
        assert!(!tracker.accept_frame(7, 1), "stale gen-1 frame must be rejected after gen-1 retired");

        // If gen-2 was ALREADY shown before a late gen-1 status arrives, do not clear displayed or revert status!
        let (clear_again, newer_already) = tracker.retire(&pending, 7, 1);
        assert!(!clear_again);
        assert!(newer_already);

        // B.2 (#6): hidden local pane disables Control menu item AND refuses `take_control()`
        // without calling source.set_controlled(true); unhiding restores immediate local control from History.
        let local = MockSource::local_running("local-hidden");
        let mut pane = PaneState::new(Tab::History);
        pane.set_source(Some(local.clone()));
        pane.set_visible(false);
        assert!(!pane.menu_state(false, false, false, false, false, false).can_control, "hidden pane must disable Control menu");
        assert_eq!(pane.take_control(), Ok(ControlAcquire::Noop));
        assert!(!pane.controlling());
        assert!(local.log.lock().unwrap().controlled_calls.is_empty(), "hidden pane must not acquire local control");

        pane.set_visible(true);
        assert!(pane.menu_state(false, false, false, false, false, false).can_control, "visible local History keeps Control enabled");
        assert_eq!(pane.take_control(), Ok(ControlAcquire::Acquired));
        assert!(pane.controlling());

        // B.2 (#5/#24): Stop confirmation verifies exact captured source + generation before releasing control.
        let captured: Arc<dyn ScreenSource> = local.clone();
        assert!(pane.matches_captured_session(local.id(), &captured, local.session_started()));
        let replacement = MockSource::local_running("local-hidden");
        pane.set_source(Some(replacement.clone()));
        assert!(!pane.matches_captured_session(local.id(), &captured, local.session_started()), "stale Stop dialog must not match replacement source under same ScreenId");

        // B.3 (#4): shared `capture_stop_expectations` sends Some(0) for known-empty screens
        // and fails closed on remote running screens without `started`.
        assert_eq!(capture_stop_expectations(false, Some(1000), None, None, Some(0)).unwrap(), (Some(1000), Some(0)));
        assert_eq!(capture_stop_expectations(true, Some(1000), None, None, None).unwrap(), (Some(1000), Some(0)));
        assert_eq!(capture_stop_expectations(false, Some(1000), Some(5000), Some(9000), Some(3)).unwrap(), (Some(1000), Some(9000)));
        assert!(capture_stop_expectations(false, None, None, None, Some(0)).is_err(), "remote Stop without started must fail closed");

        // B.8 (#26): `PaneState::info_rev` advances on every `update_info` / `set_source`
        // so a delayed `PaneMsg::Renamed` reply carrying an older revision cannot overwrite
        // newer Follow metadata or a replacement source.
        let rev_before = pane.info_rev;
        let mut updated = replacement.info();
        updated.title = Some("Follow Title".into());
        pane.update_info(updated);
        assert!(pane.info_rev > rev_before, "update_info must advance info_rev to guard delayed Rename replies");
        let rev_after_info = pane.info_rev;
        let replacement2 = MockSource::local_running("local-hidden");
        pane.set_source(Some(replacement2));
        assert!(pane.info_rev > rev_after_info, "set_source must advance info_rev to guard delayed Rename replies");
    }

    #[test]
    fn agy_r2_f3_disconnected_history_and_stats_show_disconnected_panel_until_loaded_with_cache() {
        // Online never shows disconnected_panel.
        assert!(!should_show_disconnected_for_history(false, None));
        assert!(!should_show_disconnected_for_history(false, Some((false, true, false))));
        assert!(!should_show_disconnected_for_history(false, Some((true, true, true))));

        // Offline with no replay, unloaded replay, or empty+unavailable replay shows disconnected_panel.
        assert!(should_show_disconnected_for_history(true, None));
        assert!(should_show_disconnected_for_history(true, Some((false, true, false))));
        assert!(should_show_disconnected_for_history(true, Some((true, true, true))));

        // Offline with already-loaded cached actions (or genuinely empty loaded history) stays readable!
        assert!(!should_show_disconnected_for_history(true, Some((true, false, false))));
        assert!(!should_show_disconnected_for_history(true, Some((true, true, false))));
    }
}
