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
};

use olo_capture::live::XRGB8888;
use olo_kit::{
    Bounds, Child, Cx, Element, ExternalBuffer, FocusHandle, FontWeight, InteractiveElement as _, IntoElement as _, KeyDownEvent, KeyUpEvent, Keystroke, Menu, Messenger, Modifiers,
    ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, ScrollDelta, ScrollHandle, ScrollWheelEvent, SharedString, Styled as _, View,
    WindowCx, agent, canvas, div, file_panel, icons, keys, metrics as m, point, px, size,
    style::{self, c},
    theme,
    widgets::{Button, TabView, TextControl, TextField, TextFieldEvent, open_alert},
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

const HEADER_H: f32 = 40.0;
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
        let stopped = !info.is_running();
        let lost_control_cap = !info.capabilities.can_control;
        self.info = Some(info);
        if stopped || lost_control_cap {
            self.stop_controlling();
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
        (self.visible && self.shown_tab() == Tab::Live && self.is_running()).then(|| self.info.as_ref().map(|i| i.id.clone())).flatten()
    }

    /// Whether the current screen's history follower should be open/active.
    /// Local screens follow eagerly (zero-cost `inotify`, feeding the Live
    /// header and status bar); lazy remote screens open `screen history` only
    /// while History or Stats is shown or an export runs for that screen.
    fn should_follow_history(&self) -> bool {
        let Some(src) = &self.source else { return false };
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
        if self.controlling || self.control_pending {
            return Ok(ControlAcquire::Noop);
        }
        let Some(info) = &self.info else { return Ok(ControlAcquire::Noop) };
        if !info.is_running() || !info.capabilities.can_control {
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

    fn header_subtitle(&self, actions: usize, last_action_ms: Option<u64>, replay_started_ms: Option<u64>, now_ms: u64) -> String {
        let Some(info) = &self.info else { return String::new() };
        let mut parts = Vec::new();
        if info.is_running() {
            parts.push("Running".to_owned());
            if let ScreenLocation::Host { host_label, .. } = &info.id.location {
                parts.push(host_label.clone());
            }
            if let Some((w, h)) = info.size {
                parts.push(format!("{w} × {h}"));
            }
            if let Some(t) = replay_started_ms.or(info.started_ms) {
                parts.push(olo_kit::ago::ago_ms(t, |a| format!("up {}", stats::span(a.as_millis() as u64))));
            }
        } else {
            parts.push(match info.state {
                ScreenState::Archived => "Archived".to_owned(),
                ScreenState::Stopped | ScreenState::Running => "Stopped".to_owned(),
            });
            if let ScreenLocation::Host { host_label, .. } = &info.id.location {
                parts.push(host_label.clone());
            }
        }
        if actions > 0 {
            parts.push(format!("{} action{}", style::thousands(actions), if actions == 1 { "" } else { "s" }));
        }
        if let Some(t) = last_action_ms {
            parts.push(format!("last {}", stats::ago(t, now_ms)));
        }
        parts.join("  ·  ")
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
                (format!("Latest: {what}{mark}  ·  {}", stats::ago(a.at.saturating_add(a.ms), now_ms)), false)
            }
            None => ("Watching. Nothing has been done on it yet.".to_owned(), false),
        }
    }

    fn can_export_selection(&self, selection_has_gaps: bool) -> bool {
        !selection_has_gaps || self.info.as_ref().is_some_and(|i| i.capabilities.can_export_discontiguous)
    }

    fn menu_state(&self, replay_loaded: bool, has_actions: bool, playing: bool, exporting: bool, has_frames: bool, live_has_shot: bool) -> ScreenMenuState {
        let Some(info) = &self.info else { return ScreenMenuState::default() };
        let running = info.is_running();
        let shown_tab = self.shown_tab();
        let has_history = shown_tab == Tab::History && replay_loaded && has_actions;
        let can_screenshot = if shown_tab == Tab::Live { live_has_shot || (replay_loaded && has_frames) } else { replay_loaded && has_frames };
        ScreenMenuState {
            has_screen: true,
            running,
            shown_tab,
            has_history,
            playing,
            can_export: replay_loaded && !exporting && has_actions,
            can_screenshot,
            can_clear: !running,
            can_control: running && info.capabilities.can_control,
            controlling: self.controlling,
            can_move_windows: running && info.capabilities.can_move_windows,
            giving: self.giving,
            busy: self.busy,
            can_airplay: running && info.capabilities.can_airplay,
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
    /// A frame shown live: watch `watch`'s.
    LiveDmabuf {
        watch: u64,
        buffer: ExternalBuffer,
    },
    LiveStatus {
        watch: u64,
        status: LiveStatus,
    },
    LiveControl {
        watch: u64,
        held: bool,
        reason: Option<String>,
    },
    LiveFailed {
        watch: u64,
        message: String,
    },
    Started(ScreenId, Result<ScreenInfo, String>),
    Stopped(ScreenId, Result<(), String>),
    Cleared(ScreenId, Result<(), String>),
    Renamed(ScreenId, Result<(), String>),
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

/// Single-slot mailbox holding only the newest RGBA frame from `watch_live`.
type PendingLiveFrame = Arc<Mutex<Option<(u64, Arc<Shot>)>>>;

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
        let changed = self.state.set_source(source);
        if was_controlling
            && !self.state.controlling()
            && let Some(id) = prev_id
        {
            cx.emit(ScreenPaneEvent::Controlling(id, false));
        }
        if changed {
            // (Another screen or source instance chosen: abandon rename, live
            // stream, and previous replay before binding the new source.)
            self.renaming = None;
            self.watching = None;
            self.forget_live(cx);
            self.replay = None;
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
        let handle = source.watch_live(
            self.modifiers.clone(),
            Box::new(move |ev| match ev {
                LiveEvent::Dmabuf(buffer) => {
                    me.send(PaneMsg::LiveDmabuf { watch, buffer });
                }
                LiveEvent::Rgba(shot) => {
                    *pending_live.lock().unwrap_or_else(|e| e.into_inner()) = Some((watch, shot));
                    if !live_frame_queued.swap(true, Ordering::AcqRel) {
                        me.send(PaneMsg::LiveFrameReady);
                    }
                }
                LiveEvent::Status(status) => {
                    me.send(PaneMsg::LiveStatus { watch, status });
                }
                LiveEvent::Control { held, reason } => {
                    me.send(PaneMsg::LiveControl { watch, held, reason });
                }
                LiveEvent::Failed(message) => {
                    me.send(PaneMsg::LiveFailed { watch, message });
                }
            }),
        );
        self.watching = Some((id, watch, false, handle));
    }

    /// Watch the chosen running screen when its Live tab is shown; keep the
    /// handle paused while on another tab or hidden so a remote host stops
    /// capturing without tearing down and re-keyframing the stream on every
    /// tab switch, and close it immediately when switching screens or stopping.
    fn sync_watch(&mut self, cx: &mut Cx<Self>) {
        let running_id = self.state.running_screen_id();
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
        self.state.live_pixel_size = None;
        let key = self.live.read(cx).external_key();
        self.live.update(cx, |w, cx| w.clear(None, cx));
        cx.set_external_buffer(key, None);
    }

    pub fn take_control(&mut self, cx: &mut Cx<Self>) {
        let Some(info) = self.state.info() else { return };
        if !info.is_running() || !info.capabilities.can_control {
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
        // Capture the screen's start generation (seconds since epoch) when
        // opening the confirmation dialog so stopping never kills a restarted
        // session that replaced this one while the dialog was open.
        let expected_started = source.session_started();
        let message = format!(
            "Stop the screen “{}”? The agent’s programs on it are closed, and anything it hasn’t saved is lost. The screen is kept, with its history: Start Screen starts it again.",
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
                // Only release the pane's active control if it is still
                // displaying the screen being stopped (never screen B), and
                // wait on the stop worker for the release acknowledgement
                // BEFORE issuing `source.stop_if_generation()` so remote stop
                // never races the `.control` lock release.
                let release_first = if p.state.screen_id() == Some(&id) {
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
                    messenger.send(PaneMsg::Stopped(id, source.stop_if_generation(expected_started)));
                });
                cx.invalidate();
            });
        });
    }

    fn ask_clear(&mut self, cx: &mut Cx<Self>) {
        let Some(info) = self.state.info().filter(|i| !i.is_running()).cloned() else { return };
        let Some(source) = self.state.source().cloned() else { return };
        let message = format!("Clear the history of “{}”? Its actions and the frames kept with them are deleted. This can’t be undone.", info.id.name);
        let messenger = self.me.clone();
        let id = info.id.clone();
        open_alert(cx, "Clear History", "warning", &message, &["Cancel", "Clear History"], move |choice, _| {
            if *choice != Some(1) {
                return;
            }
            let (messenger, source, id) = (messenger.clone(), source.clone(), id.clone());
            std::thread::spawn(move || messenger.send(PaneMsg::Cleared(id, source.clear_history())));
        });
    }

    fn can_export(&self, cx: &WindowCx) -> bool {
        !self.state.is_exporting_current()
            && self.replay.as_ref().is_some_and(|r| {
                let r = r.read(cx);
                r.loaded() && !r.actions().is_empty() && !r.export_discontiguous_blocked()
            })
    }

    /// Whether there's a picture to take: on Live, a live picture (or recorded
    /// history); on History/Stats, a history with recorded frames.
    fn can_shoot(&self, cx: &WindowCx) -> bool {
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
            if let Some(info) = self.state.info.as_mut() {
                info.title = title.clone();
            }
            cx.emit(ScreenPaneEvent::Renamed(id.clone(), title.clone()));
            let me = self.me.clone();
            std::thread::spawn(move || me.send(PaneMsg::Renamed(id, source.rename(title.as_deref()))));
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
        let line = self.state.header_subtitle(actions, last, started, now);
        let title = SharedString::from(info.display_name().to_owned());
        let renaming_now = self.renaming.is_some();
        let title_at = self.title_at.clone();
        let face = canvas(move |g, r| {
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
            let dot = if running { c(p.success) } else { style::disabled_text(&p) };
            let d = style::snap_centred(g, point(x + px(4.0), line_y), 8.0, 8.0);
            g.paint_quad(olo_kit::fill(d, dot).corner_radii(d.size.width / 2.0));
            let t = style::label_fit(
                g,
                x + px(14.0),
                line_y,
                style::Align::Left,
                line.clone(),
                style::PLAIN_SIZE,
                style::ui_font(FontWeight::NORMAL),
                style::disabled_text(&p),
                (w - px(14.0)).max(px(0.0)),
            );
            agent::record(g, agent::Node::label(line.clone(), t));
        })
        .absolute()
        .inset_0();
        // A click on the title renames it (as Tracker's names); the field,
        // over where it was, keeps it with Return and gives up with Escape.
        let face = div().flex_1().min_w_0().h_full().relative().child(face).on_mouse_down(
            MouseButton::Left,
            cx.listener(|p: &mut ScreenPane, e: &MouseDownEvent, cx| {
                if p.renaming.is_none() && p.title_at.get().contains(&e.position) {
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
            Button::new("Stop Screen…").min_width(96.0).on_click(cx.listener(|p: &mut ScreenPane, _, cx| {
                if p.host_lifecycle {
                    if let Some(id) = p.state.screen_id().cloned() {
                        cx.emit(ScreenPaneEvent::StopRequested(id));
                    }
                } else {
                    p.ask_stop(cx);
                }
            }))
        } else {
            Button::new("Clear History…").min_width(96.0).on_click(cx.listener(|p: &mut ScreenPane, _, cx| p.ask_clear(cx)))
        };
        // Exporting, it shows how far (an export takes seconds to minutes),
        // History's bar too.
        let exporting = self.state.export_done_current();
        let export = match exporting {
            Some(done) => Button::new("Export Video…").min_width(96.0).working(Some(done)),
            None => Button::new("Export Video…").min_width(96.0).enabled(self.can_export(cx)).on_click(cx.listener(|p: &mut ScreenPane, _, cx| p.ask_export(cx))),
        };
        let shot = Button::new("Take Screenshot…").min_width(96.0).enabled(self.can_shoot(cx)).on_click(cx.listener(|p: &mut ScreenPane, _, cx| p.ask_screenshot(cx)));
        let row = div().flex().flex_row().items_center().gap(px(m::SPACING)).h(px(HEADER_H)).flex_shrink_0().child(face).child(export).child(shot).child(first);
        // Stopped: started again as it was.
        if running {
            row.into_any_element()
        } else {
            let busy = self.state.busy();
            row.child(Button::new(if busy { "Starting…" } else { "Start Screen" }).id("start-screen").min_width(96.0).enabled(!busy).on_click(cx.listener(|p: &mut ScreenPane, _, cx| {
                if p.host_lifecycle {
                    if let Some(id) = p.state.screen_id().cloned() {
                        cx.emit(ScreenPaneEvent::StartRequested(id));
                    }
                } else {
                    p.start_screen(cx);
                }
            })))
            .into_any_element()
        }
    }

    /// The screen as it is now, and Take Control.
    fn live_tab(&mut self, cx: &mut Cx<Self>) -> Element {
        let Some(info) = self.state.info().filter(|i| i.is_running()).cloned() else { return div().into_any_element() };
        let p = theme::palette();
        if !self.live.read(cx).has_shot() {
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
            .child(input);

        // What is going on, and the control button.
        let now = history::now_ms();
        let latest = self.replay.as_ref().and_then(|r| r.read(cx).actions().iter().rev().find(|a| !a.is_oversized()).cloned());
        let (status_text, primary_ink) = self.state.live_status_line(latest.as_ref(), now);
        let color = if primary_ink { c(p.panel_text) } else { style::disabled_text(&p) };
        let status = canvas(move |g, b| {
            let t = style::label_fit(g, b.left(), b.center().y, style::Align::Left, status_text.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), color, b.size.width);
            agent::record(g, agent::Node::label(status_text.clone(), t));
        })
        .flex_1()
        .min_w_0()
        .h_full();
        let can_control = info.capabilities.can_control;
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
                if let Some((watch, shot)) = self.pending_live.lock().unwrap_or_else(|e| e.into_inner()).take()
                    && self.watching.as_ref().is_some_and(|(_, id, _, _)| *id == watch)
                {
                    let prev_size = self.state.live_pixel_size;
                    self.state.failure = None;
                    self.state.live_status = LiveStatus::Streaming;
                    self.state.on_live_frame_size(shot.size[0], shot.size[1]);
                    if prev_size != self.state.live_pixel_size {
                        let shape = self.state.info().and_then(|i| i.size).map(|(w, h)| (w as f32, h as f32));
                        self.live.update(cx, |w, _| w.set_shape(shape));
                        cx.invalidate();
                    }
                    self.live.update(cx, |w, cx| w.set_shot(shot, cx));
                }
                return;
            }
            PaneMsg::LiveDmabuf { watch, buffer } => {
                if !self.watching.as_ref().is_some_and(|(_, id, _, _)| *id == watch) {
                    return (buffer.released)(buffer.id);
                }
                let prev_size = self.state.live_pixel_size;
                self.state.failure = None;
                self.state.live_status = LiveStatus::Streaming;
                self.state.on_live_frame_size(buffer.width as usize, buffer.height as usize);
                if prev_size != self.state.live_pixel_size {
                    cx.invalidate();
                }
                let size = (buffer.width, buffer.height);
                let key = self.live.update(cx, |w, cx| {
                    w.set_live(size, cx);
                    w.external_key()
                });
                cx.set_external_buffer(key, Some(buffer));
                return;
            }
            PaneMsg::LiveStatus { watch, status } => {
                if self.watching.as_ref().is_some_and(|(_, id, _, _)| *id == watch) {
                    if let LiveStatus::Disconnected(ref msg) = status {
                        self.forget_live(cx);
                        self.live.update(cx, |w, cx| w.clear(Some(SharedString::from(msg.clone())), cx));
                    }
                    let id = self.state.screen_id().cloned();
                    if self.state.on_live_status(status)
                        && let Some(id) = id
                    {
                        cx.emit(ScreenPaneEvent::Controlling(id, false));
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
            PaneMsg::LiveFailed { watch, message } => {
                if self.watching.as_ref().is_some_and(|(_, id, _, _)| *id == watch) {
                    self.forget_live(cx);
                    self.live.update(cx, |w, cx| w.clear(Some(SharedString::from(message.clone())), cx));
                    let id = self.state.screen_id().cloned();
                    if self.state.on_live_failed(message)
                        && let Some(id) = id
                    {
                        cx.emit(ScreenPaneEvent::Controlling(id, false));
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
            PaneMsg::Renamed(id, r) => {
                // (Shown at once; the list read again if it couldn't be kept.)
                if let Err(e) = r {
                    self.alert(&e, cx);
                    cx.emit(ScreenPaneEvent::RenameFailed(id));
                }
            }
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
        let content: Element = match (shown, &self.replay) {
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
        };
        let tab_view = TabView::new("screen-tabs", labels, i).on_select(cx.listener(move |p: &mut ScreenPane, i: &usize, cx| {
            if let Some(tab) = tabs.get(*i) {
                p.set_tab(*tab, cx);
            }
        }));
        div().size_full().p(px(m::WINDOW_INSET)).flex().flex_col().gap(px(m::SPACING)).child(header).child(div().flex_1().min_h_0().child(tab_view.child(content))).into_any_element()
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
        assert!(remote_sub.starts_with("Running  ·  build-box  ·  1280 × 800"), "{remote_sub}");

        let ok_remote = MockSource::remote_running("h1", "build-box", "qa-2", true, None);
        pane.set_source(Some(ok_remote.clone()));
        assert_eq!(pane.take_control(), Ok(ControlAcquire::Acquired));
        assert_eq!(ok_remote.log.lock().unwrap().clipboard_joins, 0);
        drop(pane);
        assert_eq!(ok_remote.log.lock().unwrap().controlled_calls, vec![true, false]);
    }

    #[test]
    fn local_header_omits_host_and_rename_normalizes() {
        let src = MockSource::local_running("agent");
        let mut pane = PaneState::new(Tab::Live);
        pane.set_source(Some(src));
        assert_eq!(pane.header_subtitle(0, None, None, 10_000), "Running  ·  1440 × 900");
        assert_eq!(pane.header_subtitle(1, None, None, 10_000), "Running  ·  1440 × 900  ·  1 action");
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
}
