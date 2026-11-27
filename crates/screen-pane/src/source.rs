//! Source-neutral screen identity, metadata, capabilities, and the
//! [`ScreenSource`] abstraction shared by local screens and remote Rex host
//! screens.

use std::{
    cmp::Ordering,
    hash::{Hash, Hasher},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    },
};

use olo_kit::ExternalBuffer;
use olo_protocol::SyntheticEvent;
use olo_screen_codec::{Frame, FrameIndex};
use olo_sync::Mutex;
use serde::{Deserialize, Serialize};

use crate::{export, history::Action, picture::Shot};

/// Where an agent screen runs.
///
/// Identity (`Eq`, `Ord`, `Hash`) compares only `host_id` for remote hosts;
/// `host_label` is display metadata so renaming a host never changes screen
/// identity or diverges from [`ScreenId::key`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ScreenLocation {
    /// This desktop (the person's machine).
    Local,
    /// A connected (or previously seen) Rex host.
    Host { host_id: String, host_label: String },
}

impl PartialEq for ScreenLocation {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ScreenLocation::Local, ScreenLocation::Local) => true,
            (ScreenLocation::Host { host_id: a, .. }, ScreenLocation::Host { host_id: b, .. }) => a == b,
            _ => false,
        }
    }
}

impl Eq for ScreenLocation {}

impl PartialOrd for ScreenLocation {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScreenLocation {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (ScreenLocation::Local, ScreenLocation::Local) => Ordering::Equal,
            (ScreenLocation::Local, ScreenLocation::Host { .. }) => Ordering::Less,
            (ScreenLocation::Host { .. }, ScreenLocation::Local) => Ordering::Greater,
            (ScreenLocation::Host { host_id: a, .. }, ScreenLocation::Host { host_id: b, .. }) => a.cmp(b),
        }
    }
}

impl Hash for ScreenLocation {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            ScreenLocation::Local => 0u8.hash(state),
            ScreenLocation::Host { host_id, .. } => {
                1u8.hash(state);
                host_id.hash(state);
            }
        }
    }
}

impl ScreenLocation {
    pub fn is_local(&self) -> bool {
        matches!(self, ScreenLocation::Local)
    }

    pub fn host_id(&self) -> Option<&str> {
        match self {
            ScreenLocation::Local => None,
            ScreenLocation::Host { host_id, .. } => Some(host_id.as_str()),
        }
    }

    pub fn host_label(&self) -> Option<&str> {
        match self {
            ScreenLocation::Local => None,
            ScreenLocation::Host { host_label, .. } => Some(host_label.as_str()),
        }
    }
}

/// Unique identity of an agent screen across this desktop and all Rex hosts.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ScreenId {
    pub location: ScreenLocation,
    pub name: String,
}

impl ScreenId {
    pub fn local(name: impl Into<String>) -> ScreenId {
        ScreenId { location: ScreenLocation::Local, name: name.into() }
    }

    pub fn host(host_id: impl Into<String>, host_label: impl Into<String>, name: impl Into<String>) -> ScreenId {
        ScreenId { location: ScreenLocation::Host { host_id: host_id.into(), host_label: host_label.into() }, name: name.into() }
    }

    pub fn is_local(&self) -> bool {
        self.location.is_local()
    }

    /// Stable key for widget IDs (`ColumnListView`, `Slider`, `Card`, `Graph`).
    /// For local screens this is the bare screen name so existing IDs remain
    /// unchanged.
    pub fn key(&self) -> String {
        match &self.location {
            ScreenLocation::Local => self.name.clone(),
            ScreenLocation::Host { host_id, .. } => format!("host:{host_id}:{}", self.name),
        }
    }
}

/// Lifecycle state of an agent screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScreenState {
    Running,
    Stopped,
    Archived,
}

impl ScreenState {
    pub fn is_running(self) -> bool {
        self == ScreenState::Running
    }
}

impl From<crate::screens::State> for ScreenState {
    fn from(s: crate::screens::State) -> ScreenState {
        match s {
            crate::screens::State::Running => ScreenState::Running,
            crate::screens::State::Stopped => ScreenState::Stopped,
            crate::screens::State::Archived => ScreenState::Archived,
        }
    }
}

/// How the Passwords tab behaves for a screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordsMode {
    /// Local screen: full Passwords sharing UI backed by `olo_keychain`.
    Local(String),
    /// Remote screen where password sharing is not yet available (until Step 8)
    /// or disabled by host policy: renders a plain explanation.
    Unavailable(String),
}

/// Capabilities of a screen's source/host, driving which controls are offered
/// or disabled with an explicit explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneCapabilities {
    /// Whether `Move Windows To…` can move windows from this screen onto the
    /// person's desktop (local screens only).
    pub can_move_windows: bool,
    /// Whether this screen can be cast via AirPlay (local screens only).
    pub can_airplay: bool,
    /// Whether Take Control is supported and honoured by the screen's Olo
    /// installation (`false` on version skew where host agents would ignore
    /// control locks).
    pub can_control: bool,
    /// Plain explanation shown when `can_control` is `false` (e.g. "Update Olo
    /// on this host to take control.").
    pub control_disabled_reason: Option<String>,
    /// Whether this host's exporter accepts discontiguous `--part` selections
    /// (e.g. `2-4,8` from a filtered or multi-selected action list), advertised
    /// by `"discontiguous_part"` in `olo-rex-link screen --protocol`.
    pub can_export_discontiguous: bool,
    /// Whether the host's live stream speaks `ClientMsg::ControlSeq { on, seq }`
    /// (`0x14`) and `ServerMsg::Control { held, seq, reason }` (`0x04`),
    /// advertised by `"control-ack"` in `olo-rex-link screen --protocol`.
    /// `false` on older helpers so the client sends legacy `ClientMsg::Control`.
    pub explicit_control_ack: bool,
    /// Whether the host emits non-fatal `ServerMsg::Warning` (`0x05`),
    /// advertised by `"warning-status"`.
    pub warning_status: bool,
    /// Whether the host emits per-request `HistoryMsg::FrameError` (`0x25`),
    /// advertised by `"history-frame-error"`.
    pub history_frame_error: bool,
    /// Whether the host closes every initial and reset history snapshot with
    /// `HistoryMsg::SnapshotDone { actions_gen, index_gen }` (`0x26`),
    /// advertised by `"history-snapshot-done"`.
    pub history_snapshot_done: bool,
    /// Whether `olo-rex-link screen stop NAME --expected-started <u64>` is
    /// advertised (older helper that checked before dropping its lock).
    pub stop_expected_started: bool,
    /// Whether the host helper advertises `"stop-expected"` (atomic locked Stop
    /// checking both `--expected-started` and `--expected-last-used` under
    /// `NAME.lifecycle.lock`).
    pub stop_expected: bool,
    /// Whether the host helper advertises `"clear-expected"` (atomic locked
    /// `history --clear` checking `--expected-started` / `--expected-last-used`).
    pub clear_expected: bool,
    /// Whether the host helper advertises `"delete-expected"` (atomic locked
    /// `delete` checking `--expected-started` / `--expected-last-used`).
    pub delete_expected: bool,
    /// Whether the remote host carrying this screen is currently disconnected
    /// in Rex (renders header status & dot as disconnected and disables live
    /// lifecycle controls while preserving selection until reconnect).
    pub host_disconnected: bool,
    /// Whether the header subtitle should always name where the screen runs
    /// (`Running on this computer` when any remote host is listed, or
    /// `Running on <host>` for remote screens). `false` for local-only mode.
    pub show_location: bool,
    /// How the Passwords tab renders for this screen.
    pub passwords: PasswordsMode,
}

impl PaneCapabilities {
    pub fn local(name: impl Into<String>) -> PaneCapabilities {
        PaneCapabilities {
            can_move_windows: true,
            can_airplay: true,
            can_control: true,
            control_disabled_reason: None,
            can_export_discontiguous: true,
            explicit_control_ack: true,
            warning_status: true,
            history_frame_error: true,
            history_snapshot_done: true,
            stop_expected_started: true,
            stop_expected: true,
            clear_expected: true,
            delete_expected: true,
            host_disconnected: false,
            show_location: false,
            passwords: PasswordsMode::Local(name.into()),
        }
    }

    /// Safe default for an unprobed or older remote host helper (no optional
    /// capabilities advertised): Take Control stays disabled until verified,
    /// discontiguous exports are not offered, and legacy wire/CLI forms are used.
    pub fn remote_default() -> PaneCapabilities {
        PaneCapabilities {
            can_move_windows: false,
            can_airplay: false,
            can_control: false,
            control_disabled_reason: Some("Update Olo on this host to take control.".into()),
            can_export_discontiguous: false,
            explicit_control_ack: false,
            warning_status: false,
            history_frame_error: false,
            history_snapshot_done: false,
            stop_expected_started: false,
            stop_expected: false,
            clear_expected: false,
            delete_expected: false,
            host_disconnected: false,
            show_location: true,
            passwords: PasswordsMode::Unavailable("Password sharing for remote host screens isn’t available yet.".into()),
        }
    }

    /// Capabilities for a current remote host helper whose installed
    /// `olo-screen` honours the helper's control lock and advertises all
    /// current wire capabilities.
    pub fn remote_with_control() -> PaneCapabilities {
        PaneCapabilities {
            can_control: true,
            control_disabled_reason: None,
            can_export_discontiguous: true,
            explicit_control_ack: true,
            warning_status: true,
            history_frame_error: true,
            history_snapshot_done: true,
            stop_expected_started: true,
            stop_expected: true,
            clear_expected: true,
            delete_expected: true,
            ..Self::remote_default()
        }
    }

    /// Capabilities for an older remote helper (none of the new `--protocol`
    /// capabilities advertised) where Take Control is still allowed via the
    /// legacy `ClientMsg::Control` tag.
    pub fn remote_legacy_with_control() -> PaneCapabilities {
        PaneCapabilities { can_control: true, control_disabled_reason: None, ..Self::remote_default() }
    }
}

/// Snapshot of a screen's metadata and capabilities for UI rendering.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenInfo {
    pub id: ScreenId,
    /// Optional user-chosen display title (from Rename…); `None` shows `id.name`.
    pub title: Option<String>,
    pub state: ScreenState,
    /// Logical width × height when known (typically while running).
    pub size: Option<(i32, i32)>,
    pub scale: f64,
    pub sound: bool,
    /// Optional start timestamp (ms since epoch) used in the header when
    /// history has no recorded `start` action (left `None` for local screens so
    /// history's own `start` action stays authoritative).
    pub started_ms: Option<u64>,
    /// Last action timestamp (ms since epoch), if known.
    pub last_used_ms: Option<u64>,
    /// Timestamp (ms since epoch) when contact with the remote host was lost,
    /// used in the header (`Last seen running on host · 25 min ago · W × H`).
    pub disconnected_at_ms: Option<u64>,
    /// Total recorded actions if known before lazy history loads.
    pub actions: Option<usize>,
    /// Whether a viewer currently holds Take Control on this screen.
    pub controlled: bool,
    /// Capabilities of this screen's source/host.
    pub capabilities: PaneCapabilities,
}

impl ScreenInfo {
    pub fn display_name(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.id.name)
    }

    pub fn is_running(&self) -> bool {
        self.state.is_running()
    }
}

/// Connection / streaming status of the Live tab.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LiveStatus {
    #[default]
    Connecting,
    Streaming,
    Reconnecting,
    /// Non-fatal informational status line from the host.
    Message(String),
    /// Non-fatal capture or stream warning (`ServerMsg::Warning`). Shown in
    /// the Live tab without revoking control or ending the stream.
    Warning(String),
    Disconnected(String),
}

/// Events emitted from a live screen stream worker to [`crate::ScreenPane`].
pub enum LiveEvent {
    /// Zero-copy GPU dmabuf frame (local screens when compositor supports it).
    Dmabuf(ExternalBuffer),
    /// Decoded RGBA frame (local fallback or remote tile stream).
    Rgba(Arc<Shot>),
    /// Connection state or host status line change.
    Status(LiveStatus),
    /// Asynchronous control state update from the host (e.g. control refused
    /// because another viewer holds it, or released on stream disconnect).
    Control { held: bool, reason: Option<String> },
    /// Stream failure message.
    Failed(String),
}

/// Handle to an active live stream; dropping or calling [`LiveHandle::stop`]
/// ends the stream and releases its worker resources.
pub trait LiveHandle: Send {
    fn stop(&mut self) {}
    /// Pause or resume live frame capture/delivery without closing the stream
    /// (e.g. while another tab is shown or the window is hidden).
    fn set_paused(&mut self, _paused: bool) {}
}

/// Events emitted from a history follower worker to the History tab.
pub enum HistoryEvent {
    /// Initial snapshot (`reset = true`), incremental append (`reset = false`),
    /// or full replacement after truncation/clear (`reset = true`).
    Loaded {
        actions: Vec<Action>,
        /// Searchable text per action, computed off the UI thread.
        texts: Vec<String>,
        reset: bool,
        /// `true` whenever the underlying frame recording was replaced or reset
        /// (even if action rows did not reset or timestamps match), forcing
        /// Replay to discard cached pixels and re-fetch at the playhead.
        index_reset: bool,
        /// Source-neutral frame timestamp index + last recorded dimensions.
        frames: Arc<FrameIndex>,
    },
    /// Non-fatal error while following history.
    Failed(String),
}

impl HistoryEvent {
    /// Construct a [`HistoryEvent::Loaded`] batch whose per-action search
    /// texts are computed together with `actions` off the UI thread so their
    /// lengths always match.
    pub fn loaded(actions: Vec<Action>, reset: bool, frames: Arc<FrameIndex>) -> HistoryEvent {
        Self::loaded_with_index(actions, reset, reset, frames)
    }

    /// Construct a [`HistoryEvent::Loaded`] batch with independent action-list
    /// `reset` and frame-recording `index_reset` flags.
    pub fn loaded_with_index(actions: Vec<Action>, reset: bool, index_reset: bool, frames: Arc<FrameIndex>) -> HistoryEvent {
        let texts = actions.iter().map(crate::replay::search_text).collect();
        HistoryEvent::Loaded { actions, texts, reset, index_reset, frames }
    }
}

/// Externally triggerable cancellation handle shared between the UI (Cancel
/// button, pane teardown) and a blocking export or screenshot worker.
///
/// Transport cancel callbacks registered via [`ExportCancel::register_hook`]
/// fire immediately when [`ExportCancel::cancel`] is called (or immediately
/// upon registration if already cancelled), interrupting blocked opens or
/// reads without waiting for the next progress callback.
type CancelHook = Box<dyn FnOnce() + Send>;

#[derive(Clone, Default)]
pub struct ExportCancel {
    cancelled: Arc<AtomicBool>,
    hooks: Arc<Mutex<Vec<CancelHook>>>,
}

impl ExportCancel {
    pub fn new() -> ExportCancel {
        Self::default()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(AtomicOrdering::Acquire)
    }

    /// Underlying atomic flag suitable for passing into `StreamOpener::open_*`.
    pub fn flag(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }

    /// Register a callback invoked immediately upon cancellation (or right away
    /// if already cancelled).
    pub fn register_hook(&self, hook: impl FnOnce() + Send + 'static) {
        let mut hooks = self.hooks.lock().unwrap_or_else(|e| e.into_inner());
        if self.cancelled.load(AtomicOrdering::Acquire) {
            drop(hooks);
            hook();
            return;
        }
        hooks.push(Box::new(hook));
    }

    /// Trigger cancellation and run every registered transport hook immediately.
    pub fn cancel(&self) {
        self.cancelled.store(true, AtomicOrdering::SeqCst);
        let hooks: Vec<_> = self.hooks.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
        for hook in hooks {
            hook();
        }
    }
}

impl std::fmt::Debug for ExportCancel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportCancel").field("cancelled", &self.is_cancelled()).finish()
    }
}

/// Handle to an active history follower; dropping or calling
/// [`HistoryHandle::stop`] ends the follower cleanly.
pub trait HistoryHandle: Send {
    fn stop(&mut self) {}
    /// Tell the follower whether History/Stats (or an export) currently needs
    /// live history updates. Remote sources keep their host `screen history`
    /// stream open only while `active` is `true` and close it when `false`.
    fn set_active(&mut self, _active: bool) {}
}

/// No-op handle for tests or inactive sources.
pub struct NoopHandle;

impl LiveHandle for NoopHandle {}
impl HistoryHandle for NoopHandle {}

/// Backend interface that drives [`crate::ScreenPane`] and its History tab.
///
/// Implemented by [`crate::LocalScreenSource`] for screens on this desktop,
/// and by [`crate::StreamScreenSource`] for screens on Rex hosts.
pub trait ScreenSource: Send + Sync {
    /// Unique screen identity.
    fn id(&self) -> &ScreenId;

    /// Snapshot of current metadata and capabilities (must not block on I/O).
    fn info(&self) -> ScreenInfo;

    /// Begin streaming live frames for the Live tab.
    ///
    /// `modifiers` lists the BGRx dmabuf modifiers accepted by the window's
    /// compositor (empty forces RGBA frames). `on_event` is invoked from a
    /// worker thread.
    fn watch_live(&self, modifiers: Vec<u64>, on_event: Box<dyn Fn(LiveEvent) + Send + Sync>) -> Box<dyn LiveHandle>;

    /// Whether Take Control acquisition is asynchronous (confirmed later via
    /// [`LiveEvent::Control { held: true, .. }`]) rather than synchronous inside
    /// [`ScreenSource::set_controlled`]. Remote sources return `true`.
    fn async_control(&self) -> bool {
        false
    }

    /// Inform the screen's host whether the person currently holds Take Control
    /// (agents pause their own actions while `true`).
    fn set_controlled(&self, on: bool) -> Result<(), String>;

    /// Deliver any final held-key/button release events and hand control back
    /// in order so agents never act before the releases arrive. Must not block
    /// the UI thread.
    fn release_control(&self, final_events: Vec<SyntheticEvent>) -> Result<(), String> {
        if !final_events.is_empty() {
            self.send_input(final_events);
        }
        self.set_controlled(false)
    }

    /// Deliver final releases, release Take Control, and wait (bounded, called
    /// ONLY on a worker thread, e.g. before `stop()`) until the host or local
    /// worker has dropped its `.control` lease.
    fn release_control_and_wait(&self, final_events: Vec<SyntheticEvent>) -> Result<(), String> {
        self.release_control(final_events)
    }

    /// Enqueue synthetic pointer/keyboard/scroll events for delivery in order.
    /// Must never block the UI thread.
    fn send_input(&self, events: Vec<SyntheticEvent>);

    /// Join the screen's clipboard with the person's while controlling.
    /// Returns `None` for remote hosts (never shares the person's clipboard
    /// over Rex).
    fn join_clipboard(&self) -> Option<Box<dyn Send>>;

    /// Whether history should be followed lazily (opened only while History or
    /// Stats is shown or an export runs) instead of eagerly while on Live.
    fn lazy_history(&self) -> bool {
        false
    }

    /// Follow this screen's action history and frame index incrementally.
    fn follow_history(&self, on_event: Box<dyn Fn(HistoryEvent) + Send + Sync>) -> Box<dyn HistoryHandle>;

    /// Follow this screen's action history starting active only when `active` is
    /// `true`, preventing an eager connection race before `set_active(false)`.
    fn follow_history_with_active(&self, active: bool, on_event: Box<dyn Fn(HistoryEvent) + Send + Sync>) -> Box<dyn HistoryHandle> {
        let mut handle = self.follow_history(on_event);
        if !active {
            handle.set_active(false);
        }
        handle
    }

    /// Decode/fetch the recorded frame at or before `at_ms` (called on a worker
    /// thread, latest-wins for scrubbing).
    fn frame_at(&self, at_ms: u64) -> Result<Option<(u64, Frame)>, String>;

    /// Fetch a specific recorded frame for a user-initiated action (such as
    /// `Take Screenshot…`) with its own per-request cancellation handle so
    /// ongoing playback/scrubbing cannot cancel it as a miss.
    fn frame_at_pinned(&self, at_ms: u64, cancel: &ExportCancel) -> Result<Option<(u64, Frame)>, String> {
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        self.frame_at(at_ms)
    }

    /// Unblock any worker currently waiting in [`ScreenSource::frame_at`]
    /// because the viewer's playhead moved to a newer timestamp.
    fn cancel_frame_at(&self) {}

    /// Export an MP4 video of this screen's history (or a subset of action
    /// indices) to `out` (called on a worker thread). Returning `false` from
    /// `progress` cancels the export.
    fn export_video(&self, actions: Option<&[usize]>, zoom: usize, out: &Path, progress: &mut dyn FnMut(f32) -> bool) -> Result<export::Made, String>;

    /// Export an MP4 video with an externally triggerable [`ExportCancel`]
    /// handle that interrupts blocked opens and transfers immediately.
    fn export_video_cancellable(&self, actions: Option<&[usize]>, zoom: usize, out: &Path, cancel: &ExportCancel, progress: &mut dyn FnMut(f32) -> bool) -> Result<export::Made, String> {
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        self.export_video(actions, zoom, out, &mut |done| !cancel.is_cancelled() && progress(done))
    }

    /// Running session's start timestamp in seconds since the Unix epoch, used
    /// to bind a Stop confirmation dialog to one generation (`--expected-started`).
    fn session_started(&self) -> Option<u64> {
        self.info().started_ms.map(|ms| ms / 1000)
    }

    /// Start this screen if stopped, returning its updated [`ScreenInfo`].
    fn start(&self) -> Result<ScreenInfo, String>;

    /// Stop this screen if running.
    fn stop(&self) -> Result<(), String>;

    /// Notify the backend that the pane has accepted and displayed a live
    /// picture from the current connection (gates remote Take Control — Astra #5).
    fn note_live_frame_accepted(&self) {}

    /// Stop this screen only if its running generation still matches
    /// `expected_started` (seconds since Unix epoch), when supported by the
    /// backend/host (`--expected-started`). Fails closed when a guard is
    /// supplied on a backend that has not overridden it (Astra #2–#4).
    fn stop_if_generation(&self, expected_started: Option<u64>) -> Result<(), String> {
        if expected_started.is_some() {
            return Err("this screen backend doesn't support guarded stop".into());
        }
        self.stop()
    }

    /// Stop this screen only if its running generation (`expected_started`)
    /// and/or `expected_last_used` still match what the person confirmed.
    /// Preserves a backend's earlier `stop_if_generation` override when only
    /// `expected_started` is supplied, and fails closed if `expected_last_used`
    /// is supplied without an override (Astra #2–#4).
    fn stop_if_expected(&self, expected_started: Option<u64>, expected_last_used: Option<u64>) -> Result<(), String> {
        if expected_last_used.is_some() {
            return Err("this screen backend doesn't support guarded stop with last-used check".into());
        }
        self.stop_if_generation(expected_started)
    }

    /// Clear this screen's recorded actions and frames.
    fn clear_history(&self) -> Result<(), String>;

    /// Clear this screen's history only if it is still stopped and matches
    /// `expected_started` / `expected_last_used` (`Some(0)` = observed empty).
    /// Fails closed when guards are supplied on an overriding-free backend.
    fn clear_history_expected(&self, expected_started: Option<u64>, expected_last_used: Option<u64>) -> Result<(), String> {
        if expected_started.is_some() || expected_last_used.is_some() {
            return Err("this screen backend doesn't support guarded clear history".into());
        }
        self.clear_history()
    }

    /// Rename this screen (`None` or blank clears the custom title).
    fn rename(&self, title: Option<&str>) -> Result<(), String>;

    /// Archive (`on = true`) or unarchive (`on = false`) this stopped screen.
    fn archive(&self, on: bool) -> Result<(), String> {
        let _ = on;
        Err("archiving isn't supported for this screen".into())
    }

    /// Move this screen and everything it kept to the Trash.
    fn delete(&self) -> Result<(), String> {
        Err("deleting isn't supported for this screen".into())
    }

    /// Move this stopped screen to the Trash only if it is still stopped and
    /// matches `expected_started` / `expected_last_used` (`Some(0)` = observed empty).
    /// Fails closed when guards are supplied on an overriding-free backend.
    fn delete_expected(&self, expected_started: Option<u64>, expected_last_used: Option<u64>) -> Result<(), String> {
        if expected_started.is_some() || expected_last_used.is_some() {
            return Err("this screen backend doesn't support guarded delete".into());
        }
        self.delete()
    }

    /// Move all movable windows on this screen to the person's display
    /// (`to`: `Some((workspace_index, output_name))`, or `None` for active
    /// workspace). Returns descriptions of windows that stayed.
    fn give_windows(&self, to: Option<(u32, String)>) -> Result<Vec<String>, String> {
        let _ = to;
        Err("moving windows isn't supported for this screen".into())
    }

    /// List the person's display-workspaces `(output, index, label)` for
    /// `Move Windows To…`.
    fn person_workspaces(&self) -> Result<Vec<(String, u32, String)>, String> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct BareSource {
        id: ScreenId,
        calls: Mutex<Vec<&'static str>>,
    }

    impl ScreenSource for BareSource {
        fn id(&self) -> &ScreenId {
            &self.id
        }
        fn info(&self) -> ScreenInfo {
            ScreenInfo {
                id: self.id.clone(),
                title: None,
                state: ScreenState::Stopped,
                size: None,
                scale: 1.0,
                sound: false,
                started_ms: None,
                last_used_ms: None,
                disconnected_at_ms: None,
                actions: None,
                controlled: false,
                capabilities: PaneCapabilities::local(&self.id.name),
            }
        }
        fn watch_live(&self, _: Vec<u64>, _: Box<dyn Fn(LiveEvent) + Send + Sync>) -> Box<dyn LiveHandle> {
            Box::new(NoopHandle)
        }
        fn set_controlled(&self, _: bool) -> Result<(), String> {
            Ok(())
        }
        fn send_input(&self, _: Vec<SyntheticEvent>) {}
        fn join_clipboard(&self) -> Option<Box<dyn Send>> {
            None
        }
        fn follow_history(&self, _: Box<dyn Fn(HistoryEvent) + Send + Sync>) -> Box<dyn HistoryHandle> {
            Box::new(NoopHandle)
        }
        fn frame_at(&self, _: u64) -> Result<Option<(u64, Frame)>, String> {
            Ok(None)
        }
        fn export_video(&self, _: Option<&[usize]>, _: usize, _: &Path, _: &mut dyn FnMut(f32) -> bool) -> Result<export::Made, String> {
            Err("unsupported".into())
        }
        fn start(&self) -> Result<ScreenInfo, String> {
            Ok(self.info())
        }
        fn stop(&self) -> Result<(), String> {
            self.calls.lock().unwrap().push("stop");
            Ok(())
        }
        fn clear_history(&self) -> Result<(), String> {
            self.calls.lock().unwrap().push("clear");
            Ok(())
        }
        fn rename(&self, _: Option<&str>) -> Result<(), String> {
            Ok(())
        }
        fn delete(&self) -> Result<(), String> {
            self.calls.lock().unwrap().push("delete");
            Ok(())
        }
    }

    struct GenOnlySource {
        inner: BareSource,
        gen_seen: Mutex<Option<u64>>,
    }

    impl ScreenSource for GenOnlySource {
        fn id(&self) -> &ScreenId {
            self.inner.id()
        }
        fn info(&self) -> ScreenInfo {
            self.inner.info()
        }
        fn watch_live(&self, m: Vec<u64>, cb: Box<dyn Fn(LiveEvent) + Send + Sync>) -> Box<dyn LiveHandle> {
            self.inner.watch_live(m, cb)
        }
        fn set_controlled(&self, on: bool) -> Result<(), String> {
            self.inner.set_controlled(on)
        }
        fn send_input(&self, ev: Vec<SyntheticEvent>) {
            self.inner.send_input(ev)
        }
        fn join_clipboard(&self) -> Option<Box<dyn Send>> {
            None
        }
        fn follow_history(&self, cb: Box<dyn Fn(HistoryEvent) + Send + Sync>) -> Box<dyn HistoryHandle> {
            self.inner.follow_history(cb)
        }
        fn frame_at(&self, at: u64) -> Result<Option<(u64, Frame)>, String> {
            self.inner.frame_at(at)
        }
        fn export_video(&self, a: Option<&[usize]>, z: usize, p: &Path, cb: &mut dyn FnMut(f32) -> bool) -> Result<export::Made, String> {
            self.inner.export_video(a, z, p, cb)
        }
        fn start(&self) -> Result<ScreenInfo, String> {
            self.inner.start()
        }
        fn stop(&self) -> Result<(), String> {
            self.inner.stop()
        }
        fn stop_if_generation(&self, expected_started: Option<u64>) -> Result<(), String> {
            *self.gen_seen.lock().unwrap() = expected_started;
            Ok(())
        }
        fn clear_history(&self) -> Result<(), String> {
            self.inner.clear_history()
        }
        fn rename(&self, t: Option<&str>) -> Result<(), String> {
            self.inner.rename(t)
        }
    }

    #[test]
    fn final_pass_b3_screen_source_defaults_fail_closed_and_preserve_generation_override() {
        let bare = BareSource { id: ScreenId::local("qa"), calls: Mutex::new(Vec::new()) };
        assert!(bare.stop_if_generation(Some(100)).is_err());
        assert!(bare.stop_if_expected(Some(100), None).is_err());
        assert!(bare.stop_if_expected(None, Some(500)).is_err());
        assert!(bare.clear_history_expected(Some(0), None).is_err());
        assert!(bare.clear_history_expected(None, Some(0)).is_err());
        assert!(bare.delete_expected(Some(0), None).is_err());
        assert!(bare.delete_expected(None, Some(0)).is_err());
        assert!(bare.calls.lock().unwrap().is_empty(), "fail-closed defaults must never dispatch unguarded destructive verbs");

        // Unguarded calls still reach the base verbs cleanly.
        bare.stop_if_expected(None, None).unwrap();
        bare.clear_history_expected(None, None).unwrap();
        bare.delete_expected(None, None).unwrap();
        assert_eq!(*bare.calls.lock().unwrap(), vec!["stop", "clear", "delete"]);

        // An embedder overriding ONLY the earlier `stop_if_generation` still has
        // `stop_if_expected(Some(ts), None)` reach its override, while supplying
        // `expected_last_used` still fails closed.
        let gen_only = GenOnlySource { inner: BareSource { id: ScreenId::local("qa"), calls: Mutex::new(Vec::new()) }, gen_seen: Mutex::new(None) };
        gen_only.stop_if_expected(Some(1234), None).unwrap();
        assert_eq!(*gen_only.gen_seen.lock().unwrap(), Some(1234));
        assert!(gen_only.stop_if_expected(Some(1234), Some(99)).is_err());
    }
}
