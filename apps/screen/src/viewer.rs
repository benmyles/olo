//! Agent Screens, the window: the agents' screens in a sidebar (on this
//! computer and on any connected Rex host), and the one chosen shown in
//! [`olo_screen_pane::ScreenPane`] under a header saying what it is and how
//! it's doing, in four tabs: Live (the screen as it is, with the agent's
//! pointer, and Take Control to step in), History, Stats and Passwords
//! (besides or without what every screen has, Screen ▸ Passwords for Every
//! Screen…).

use olo_sync::Mutex;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    ffi::OsString,
    io,
    os::{
        fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd},
        unix::fs::MetadataExt as _,
        unix::net::UnixStream,
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use olo_kit::{
    Bounds, Child, Cx, Element, FontWeight, InteractiveElement as _, IntoElement as _, Menu, MenuBar, Modifiers, MouseButton, ParentElement as _, Pixels, Point, Proxy, ScrollHandle, SharedString,
    Styled as _, TextRun, View, WindowCx, WindowHandle, agent, canvas, control_canvas, div, icons, metrics as m, point, px, size, style, theme,
    widgets::{Button, DISABLED_OPACITY, Front, Panes, ScrollView, SidePane, open_alert, sidebar},
};
use olo_screen_pane::{
    HostCapabilities, HostProtocolCache, LocalScreenSource, OpenHostError, Padded, PaneCapabilities, PasswordsMode, RexSockClient, RexSockOpener, ScreenId, ScreenInfo, ScreenLocation, ScreenPane,
    ScreenPaneEvent, ScreenSource, ScreenState, StreamScreenSource, Tab, capture_stop_expectations, history, local,
    pane::{paint_status_dot, paint_status_ring, tertiary_ink},
    paths,
    rex_sock::{self, HostScreenState, MAX_HOSTS, MAX_RESPONSE_LINE_BYTES, MAX_TOTAL_SCREENS, RexFollowLine, RexHost, RexHostConn, RexScreen, RexSockRequest},
    screens::{self, Screen},
    shares,
};
use serde::{Deserialize, Serialize};

use crate::sidebar::{DotKind, Entry, LocalSidebar, MultiSidebar};

/// The sidebar's width in local-only mode.
const SIDEBAR_W: f32 = 196.0;
const SIDEBAR_MIN_W: f32 = 160.0;
/// Roomier sidebar width when remote Rex hosts are shown (fits left latch,
/// host icon, middle-truncated names, and one right edge for ages & slots).
const MULTI_SIDEBAR_W: f32 = 228.0;
const MULTI_SIDEBAR_MIN_W: f32 = 184.0;
/// Minimum right-pane width before the sidebar gives way cleanly in narrow
/// tiling layouts (vertical half / quarter of 1280 × 800).
const PANE_MIN_BESIDE: f32 = 460.0;
/// New Screen…'s place under the sidebar.
const FOOTER_H: f32 = m::CONTROL_HEIGHT + 2.0 * m::SPACING;
/// Row height inside the selected host's screen list well.
const HOST_LIST_ROW_H: f32 = 28.0;
/// Deadline for initial Rex `follow` connect + mandatory `Snapshot`.
const REX_FOLLOW_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Stable host key used for the "This Computer" section heading.
const LOCAL_HOST_KEY: &str = "local";

/// What is currently selected in the sidebar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Selection {
    Screen(ScreenId),
    /// A host heading (`"local"` for This Computer, or `RexHost::key`).
    Host(String),
}

/// Requested or restored target held until its authoritative source arrives
/// (Finding 18: remote disconnect can never discard a pending local `watch` or
/// restored local screen, and local discovery can never discard a pending remote
/// target before Rex's initial snapshot).
#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingTarget {
    Watch(String),
    RestoredScreen(ScreenId),
    RestoredHost(String),
}

impl PendingTarget {
    fn is_remote(&self) -> bool {
        match self {
            PendingTarget::Watch(spec) => spec.contains(':'),
            PendingTarget::RestoredScreen(id) => !id.is_local(),
            PendingTarget::RestoredHost(k) => k != LOCAL_HOST_KEY,
        }
    }
}

pub enum Msg {
    /// Both running local screens and rows refreshed together off the UI thread.
    Refreshed(u64, Vec<Screen>, Vec<Row>),
    Started(Result<Screen, String>),
    /// `(host_key, host_label, captured_rex_model_rev, result)` — B.8: Follow wins over stale replies.
    RemoteStarted(String, String, u64, Result<RexScreen, String>),
    Stopped(Result<(), String>),
    /// Screens archived, unarchived, stopped or deleted together: what
    /// went wrong, if anything.
    Changed(Vec<String>),
    /// Coalesced notification that `RexMailbox` holds newer Rex host state.
    RexUpdated,
    /// Plain-words alert posted from a background helper (e.g. Rex refused `open_host`).
    Alert(String),
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    /// Legacy local-only selected screen name (still accepted on upgrade).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selected: Option<String>,
    /// Location-aware selected screen (`Local` or `Host`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selected_id: Option<ScreenId>,
    /// Selected host heading (`"local"` or `RexHost::key`), if a heading was
    /// chosen rather than a screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selected_host: Option<String>,
    /// Unresolved `watch [HOST:]NAME` target preserved across reload until
    /// resolved or explicitly superseded (Final pass B.10 / #18).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_watch: Option<String>,
    /// Host sections folded by the user (`"local"` or `RexHost::key`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    collapsed_hosts: Vec<String>,
    #[serde(default)]
    tab: Tab,
    /// The Archived section hidden (View ▸ Show Archived off): listed unless.
    #[serde(default)]
    hide_archived: bool,
    /// Whether the sidebar was hidden via View ▸ Hide Sidebar.
    #[serde(default)]
    sidebar_collapsed: bool,
    /// Whether narrow-window mode was showing the sidebar alone (`Front::Pane`).
    #[serde(default)]
    narrow_front_pane: bool,
    /// Sidebar vertical scroll offset in logical points (Finding 19).
    #[serde(default)]
    sidebar_scroll_y: f32,
}

/// A local screen in the list.
#[derive(Debug, Clone)]
pub struct Row {
    name: String,
    last_used: Option<u64>,
    /// The name the person gave it (Rename…), shown for its own.
    title: Option<String>,
    /// Archived (stopped, listed only with View ▸ Show Archived).
    archived: bool,
}

impl Row {
    fn shown(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.name)
    }
}

/// Target of a multi-host sidebar entry.
#[derive(Clone, Debug)]
enum SidebarTarget {
    Screen(ScreenId),
    Host(String),
}

/// The screens running (`running`) and those stopped with a history, by
/// last use, most recent first. Uses tail-only `last_used_fast` (<= 64 KiB)
/// off the UI thread (Finding 23).
fn rows(running: &[Screen]) -> Vec<Row> {
    let own = std::env::var("OLO_SCREEN").ok();
    let mut names: Vec<String> = running.iter().map(|s| s.name.clone()).collect();
    for n in history::names() {
        if own.as_deref() != Some(n.as_str()) && !names.contains(&n) {
            names.push(n);
        }
    }
    let mut out: Vec<Row> = names.into_iter().map(|name| Row { last_used: history::last_used_fast(&name), title: history::title(&name), archived: history::archived(&name), name }).collect();
    out.sort_by(|a, b| b.last_used.cmp(&a.last_used).then(a.name.cmp(&b.name)));
    out
}

/// Screens listed for the window: never its own (watching a screen from
/// itself mirrors it into itself).
fn listed() -> Vec<Screen> {
    let own = std::env::var("OLO_SCREEN").ok();
    screens::list().into_iter().filter(|s| own.as_deref() != Some(s.name.as_str())).collect()
}

/// Result of opening a Linux `pidfd` for a running screen compositor.
enum PidfdOpen {
    Opened(OwnedFd),
    /// Process already exited before `pidfd_open` (`ESRCH`).
    Exited,
    /// Non-exit error (e.g. `EMFILE`/`ENOSYS`).
    Unavailable,
}

/// Open a Linux `pidfd` for `pid` so the sidebar watcher wakes immediately when
/// a running screen compositor exits, with zero timer polling.
fn open_pidfd(pid: i32) -> PidfdOpen {
    if pid <= 0 {
        return PidfdOpen::Exited;
    }
    // SAFETY: `SYS_pidfd_open` takes `(pid_t, unsigned int)` and returns a new
    // close-on-exec file descriptor or `-1` on error.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0 as libc::c_uint) as i32 };
    if fd >= 0 {
        PidfdOpen::Opened(unsafe { OwnedFd::from_raw_fd(fd) })
    } else if io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        PidfdOpen::Exited
    } else {
        PidfdOpen::Unavailable
    }
}

/// Open an `eventfd` used to wake a `poll` loop immediately on filesystem
/// events, explicit refreshes, or shutdown.
fn open_eventfd() -> Option<OwnedFd> {
    // SAFETY: `eventfd(0, EFD_NONBLOCK | EFD_CLOEXEC)` creates an owned fd.
    let fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
    if fd < 0 { None } else { Some(unsafe { OwnedFd::from_raw_fd(fd) }) }
}

fn signal_eventfd(fd: &OwnedFd) {
    let one: u64 = 1;
    let _ = unsafe { libc::write(fd.as_raw_fd(), (&raw const one).cast(), std::mem::size_of::<u64>()) };
}

fn drain_eventfd(fd: &OwnedFd) {
    let mut buf: u64 = 0;
    let _ = unsafe { libc::read(fd.as_raw_fd(), (&raw mut buf).cast(), std::mem::size_of::<u64>()) };
}

/// Read `/proc/<pid>/stat` field 22 (`starttime` in clock ticks) so pidfd
/// retention keys on `(pid, start_ticks)` rather than numeric PID alone
/// (B.11 / Astra #21: prevents PID reuse from retaining an exited pidfd).
fn proc_start_ticks(pid: i32) -> Option<u64> {
    if pid <= 0 {
        return None;
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(')')?.1;
    // Field 3 (`state`) is index 0 after `") "`; field 22 (`starttime`) is index 19.
    after_comm.split_whitespace().nth(19)?.parse::<u64>().ok()
}

/// Files inside a screen's state folder that affect the sidebar list or its
/// `last_used` / `title` / `archived` display (explicitly excluding recorder
/// frame segments `frames-*.olof`, `recorder.lock`, `pictures/`, `logs/`, and
/// Chrome profile data — B.11 / Astra #23).
fn is_screen_state_file(name: &std::ffi::OsStr) -> bool {
    matches!(name.to_str(), Some("actions.jsonl" | "title" | "archived" | "screen.json"))
}

/// Whether a filesystem event inside `runtime_dir` or `state_dir` could affect
/// the local screen list (B.11 / Astra #23):
/// - `olo_kit::watch` delivers `[watched_dir, watched_dir/child, ...]` on normal
///   child events and `[watched_dir]` alone only on overflow/resync.
/// - Ignore watched-directory self-entries whenever child paths are present;
///   filter out locks, sockets, `frames-*.olof`, `pictures/`, `recorder.lock`,
///   and `logs/`.
fn runtime_or_state_event_relevant(paths: &[PathBuf], runtime_dir: &Path, state_dir: &Path) -> bool {
    let mut saw_child = false;
    let mut saw_relevant = false;
    for p in paths {
        if p == runtime_dir || p == state_dir {
            continue;
        }
        saw_child = true;
        if let Ok(rel) = p.strip_prefix(runtime_dir) {
            // Only direct `NAME.json` descriptors under `runtime_dir` change running screens.
            if rel.components().count() == 1 && p.extension().is_some_and(|e| e == "json") {
                saw_relevant = true;
            }
        } else if let Ok(rel) = p.strip_prefix(state_dir) {
            let depth = rel.components().count();
            if depth == 1 {
                // Direct screen-directory add/remove under `state_dir` (skip non-screen noise).
                if let Some(name) = rel.file_name().and_then(|n| n.to_str())
                    && !matches!(name, "logs" | "pictures" | "recorder.lock")
                    && !name.starts_with("frames-")
                    && !name.ends_with(".olof")
                {
                    saw_relevant = true;
                }
            } else if depth == 2 && p.file_name().is_some_and(is_screen_state_file) {
                saw_relevant = true;
            }
        } else {
            saw_relevant = true;
        }
    }
    if saw_child { saw_relevant } else { !paths.is_empty() }
}

/// Filter per-screen `follow_use` callbacks so recorder writes (`frames-*.olof`,
/// `recorder.lock`, `pictures/`, `logs/`) never wake a full list rescan
/// (B.11 / Astra #23).
fn screen_history_event_relevant(paths: &[PathBuf], watched_dirs: &[PathBuf]) -> bool {
    let mut saw_child = false;
    let mut saw_relevant = false;
    for p in paths {
        if watched_dirs.iter().any(|d| d == p) {
            continue;
        }
        saw_child = true;
        if p.file_name().is_some_and(is_screen_state_file) {
            saw_relevant = true;
        }
    }
    if saw_child { saw_relevant } else { !paths.is_empty() }
}

/// Handle keeping the local screen-list watcher alive for the lifetime of
/// `Viewer`, allowing explicit refreshes to serialize through the same worker
/// (Finding 22) and stopping its `poll` thread cleanly on drop.
struct ListWatcher {
    _watch: Option<olo_kit::watch::Watch>,
    stop: Arc<AtomicBool>,
    wake_tx: Option<Arc<OwnedFd>>,
    proxy: Proxy,
    list_seq: Arc<AtomicU64>,
}

impl ListWatcher {
    /// Wake the serialized `screen-list-watch` worker to perform an ordered
    /// local rescan (or run a fallback one-shot scan if `eventfd` was unavailable).
    fn poke(&self) {
        if let Some(fd) = &self.wake_tx {
            signal_eventfd(fd);
        } else {
            let proxy = self.proxy.clone();
            let seq = self.list_seq.fetch_add(1, Ordering::SeqCst) + 1;
            std::thread::spawn(move || {
                let s = listed();
                let r = rows(&s);
                proxy.send(Msg::Refreshed(seq, s, r));
            });
        }
    }
}

impl Drop for ListWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(fd) = &self.wake_tx {
            signal_eventfd(fd);
        }
    }
}

/// Remove any pidfds reported readable/errored by `poll` before reconciling
/// (B.11 / Astra #21: prevents an exited pidfd from spinning `poll` forever if
/// a new session reuses the same numeric PID).
fn drop_signalled_pidfds(pidfds: &mut HashMap<(i32, u64), OwnedFd>, polled_keys: &[(i32, u64)], fds: &[libc::pollfd]) {
    for (idx, key) in polled_keys.iter().enumerate() {
        if let Some(pfd) = fds.get(idx + 1)
            && (pfd.revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP | libc::POLLNVAL)) != 0
        {
            pidfds.remove(key);
        }
    }
}

/// Watch the screens runtime folder (`screens::dir()`) and actual history root
/// (`history::root()`, Finding 20) plus retained `pidfd`s keyed by
/// `(pid, proc_start_ticks)` for every running screen (dropping readable pidfds
/// before reconciling — B.11 / Astra #21, reconciling `ESRCH` before `poll`),
/// stamping `list_seq` BEFORE each scan (Finding 22), with 0 wakeups/s at rest.
fn watch_list(proxy: Proxy, list_seq: Arc<AtomicU64>) -> ListWatcher {
    let runtime_dir = screens::dir();
    let state_dir = history::root();
    let stop = Arc::new(AtomicBool::new(false));

    let Some(wake_fd) = open_eventfd().map(Arc::new) else {
        // Finding 21: even on eventfd-failure fallback, emit the initial local
        // snapshot immediately without waiting for a future directory event.
        let p0 = proxy.clone();
        let s0 = list_seq.clone();
        std::thread::spawn(move || {
            let seq = s0.fetch_add(1, Ordering::SeqCst) + 1;
            let s = listed();
            let r = rows(&s);
            p0.send(Msg::Refreshed(seq, s, r));
        });
        let proxy_w = proxy.clone();
        let seq_w = list_seq.clone();
        let rt_filter = runtime_dir.clone();
        let st_filter = state_dir.clone();
        let watch = olo_kit::watch::watch([runtime_dir, state_dir], move |changed| {
            if !runtime_or_state_event_relevant(changed, &rt_filter, &st_filter) {
                return;
            }
            let p = proxy_w.clone();
            let sq = seq_w.clone();
            std::thread::spawn(move || {
                let seq = sq.fetch_add(1, Ordering::SeqCst) + 1;
                let s = listed();
                let r = rows(&s);
                p.send(Msg::Refreshed(seq, s, r));
            });
        });
        return ListWatcher { _watch: watch, stop, wake_tx: None, proxy, list_seq };
    };

    let wake_watch = wake_fd.clone();
    let rt_filter = runtime_dir.clone();
    let st_filter = state_dir.clone();
    let watch = olo_kit::watch::watch([runtime_dir, state_dir], move |changed| {
        if runtime_or_state_event_relevant(changed, &rt_filter, &st_filter) {
            signal_eventfd(&wake_watch);
        }
    });

    let stop_bg = stop.clone();
    let wake_bg = wake_fd.clone();
    let proxy_bg = proxy.clone();
    let seq_bg = list_seq.clone();
    let _ = std::thread::Builder::new().name("screen-list-watch".into()).spawn(move || {
        // Stamp `seq` BEFORE scanning so scans are strictly ordered (Finding 22).
        let seq0 = seq_bg.fetch_add(1, Ordering::SeqCst) + 1;
        let mut last_screens = listed();
        let mut last_rows = rows(&last_screens);
        proxy_bg.send(Msg::Refreshed(seq0, last_screens.clone(), last_rows.clone()));

        let mut pidfds: HashMap<(i32, u64), OwnedFd> = HashMap::new();
        while !stop_bg.load(Ordering::Acquire) {
            // B.11 (#21): key retained pidfds by `(pid, proc_start_ticks)` so
            // PID reuse never keeps an exited pidfd alive across iterations.
            let mut live_keys: HashSet<(i32, u64)> = HashSet::with_capacity(last_screens.len());
            let mut exited_before_poll = false;
            for s in &last_screens {
                let Some(ticks) = proc_start_ticks(s.pid) else {
                    exited_before_poll = true;
                    continue;
                };
                let key = (s.pid, ticks);
                live_keys.insert(key);
                if let std::collections::hash_map::Entry::Vacant(slot) = pidfds.entry(key) {
                    match open_pidfd(s.pid) {
                        PidfdOpen::Opened(fd) => {
                            // Re-verify start_ticks after opening pidfd in case
                            // the PID recycled between stat and pidfd_open.
                            if proc_start_ticks(s.pid) == Some(ticks) {
                                slot.insert(fd);
                            } else {
                                exited_before_poll = true;
                            }
                        }
                        PidfdOpen::Exited => {
                            exited_before_poll = true;
                        }
                        PidfdOpen::Unavailable => {}
                    }
                }
            }
            pidfds.retain(|key, _| live_keys.contains(key));

            if !exited_before_poll {
                let mut fds = Vec::with_capacity(1 + pidfds.len());
                let mut polled_keys = Vec::with_capacity(pidfds.len());
                fds.push(libc::pollfd { fd: wake_bg.as_raw_fd(), events: libc::POLLIN, revents: 0 });
                for (&key, pfd) in &pidfds {
                    polled_keys.push(key);
                    fds.push(libc::pollfd { fd: pfd.as_raw_fd(), events: libc::POLLIN, revents: 0 });
                }
                let rc = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
                if rc < 0 {
                    let err = io::Error::last_os_error();
                    if err.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }
                if stop_bg.load(Ordering::Acquire) {
                    break;
                }
                // B.11 (#21): remove every readable/exited pidfd BEFORE reconciling
                // so a reused PID can never spin `poll`.
                drop_signalled_pidfds(&mut pidfds, &polled_keys, &fds);
                if (fds[0].revents & libc::POLLIN) != 0 {
                    drain_eventfd(&wake_bg);
                }
            } else {
                // Reconcile immediately after an `ESRCH` during `pidfd_open`
                // without sleeping in `poll` (Finding 21).
                drain_eventfd(&wake_bg);
            }

            let seq = seq_bg.fetch_add(1, Ordering::SeqCst) + 1;
            let now_screens = listed();
            let now_rows = rows(&now_screens);
            let rows_changed =
                now_rows.len() != last_rows.len() || now_rows.iter().zip(&last_rows).any(|(a, b)| a.name != b.name || a.title != b.title || a.archived != b.archived || a.last_used != b.last_used);
            if now_screens != last_screens || rows_changed {
                last_screens = now_screens;
                last_rows = now_rows;
                proxy_bg.send(Msg::Refreshed(seq, last_screens.clone(), last_rows.clone()));
            }
        }
    });

    ListWatcher { _watch: watch, stop, wake_tx: Some(wake_fd), proxy, list_seq }
}

// ---------------------------------------------------------------------------
// Zero-polling, peer-authenticated, bounded Rex Screens watcher
// (Findings 1, 9, 11, 12, 14, 16, 17, 23, 29)
// ---------------------------------------------------------------------------

/// Coalesced state handed from the `rex-follow` worker to the UI thread via
/// `RexMailbox` (at most ONE pending `Msg::RexUpdated` in the looper queue).
///
/// - Structural updates (`Snapshot`, `HostUpdated`, `HostRemoved`, disconnect)
///   populate `hosts = Some(...)` and `host_conn_revs = Some(...)` (preserving
///   offline->online transitions across coalescing — B.6 / #10/#13/#28) and
///   clear superseded `last_used_deltas`.
/// - Activity ticks (`ScreenLastUsed`) populate ONLY `last_used_deltas` without
///   cloning `hosts` at all (B.11 / #29).
struct RexMailboxSnap {
    hosts: Option<Vec<RexHost>>,
    host_conn_revs: Option<HashMap<String, u64>>,
    rex_epoch: u64,
    connected: bool,
    /// Coalesced `(host_key, screen_name) -> (last_used_ms, actions)` deltas
    /// applied in-place when `hosts` is `None` (zero host clones on ticks!).
    last_used_deltas: HashMap<(String, String), (u64, Option<usize>)>,
    /// Bounded protocol error surfaced once per occurrence (Finding 16).
    protocol_error: Option<String>,
}

struct RexMailbox {
    inner: Mutex<Option<RexMailboxSnap>>,
    pending: AtomicBool,
}

impl RexMailbox {
    fn new() -> Self {
        RexMailbox { inner: Mutex::new(None), pending: AtomicBool::new(false) }
    }

    fn wake(&self, proxy: &Proxy) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            proxy.send(Msg::RexUpdated);
        }
    }

    /// Publish a structural snapshot + monotonic per-host connection revisions
    /// (`host_conn_revs`), superseding any older pending `last_used_deltas`.
    fn publish_structural(&self, proxy: &Proxy, hosts: Vec<RexHost>, host_conn_revs: HashMap<String, u64>, rex_epoch: u64, connected: bool) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let prev_err = guard.as_mut().and_then(|prev| prev.protocol_error.take());
        *guard = Some(RexMailboxSnap { hosts: Some(hosts), host_conn_revs: Some(host_conn_revs), rex_epoch, connected, last_used_deltas: HashMap::new(), protocol_error: prev_err });
        drop(guard);
        self.wake(proxy);
    }

    /// Publish a lightweight `ScreenLastUsed` delta WITHOUT cloning `hosts`
    /// (B.11 / #29). If a structural snapshot is already waiting unread, updates
    /// that pending snapshot's matching screen in place instead of growing
    /// `last_used_deltas`.
    #[allow(clippy::too_many_arguments)]
    fn publish_last_used(&self, proxy: &Proxy, rex_epoch: u64, connected: bool, host: String, name: String, last_used: u64, actions: Option<usize>) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(prev) = guard.as_mut() {
            prev.rex_epoch = rex_epoch;
            prev.connected = connected;
            if let Some(hosts) = prev.hosts.as_mut() {
                if let Some(h) = hosts.iter_mut().find(|h| h.key == host)
                    && let Some(s) = h.screens.iter_mut().find(|s| s.name == name)
                {
                    if last_used > 0 {
                        s.last_used = Some(s.last_used.map_or(last_used, |cur| cur.max(last_used)));
                    }
                    if actions.is_some() {
                        s.actions = actions;
                    }
                }
            } else {
                prev.last_used_deltas
                    .entry((host, name))
                    .and_modify(|(lu, act)| {
                        *lu = (*lu).max(last_used);
                        if actions.is_some() {
                            *act = actions;
                        }
                    })
                    .or_insert((last_used, actions));
            }
        } else {
            let mut deltas = HashMap::new();
            deltas.insert((host, name), (last_used, actions));
            *guard = Some(RexMailboxSnap { hosts: None, host_conn_revs: None, rex_epoch, connected, last_used_deltas: deltas, protocol_error: None });
        }
        drop(guard);
        self.wake(proxy);
    }

    /// Publish a bounded protocol diagnostic without cloning `hosts`.
    fn publish_error(&self, proxy: &Proxy, rex_epoch: u64, connected: bool, message: String) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(prev) = guard.as_mut() {
            prev.rex_epoch = rex_epoch;
            prev.connected = connected;
            prev.protocol_error = Some(message);
        } else {
            *guard = Some(RexMailboxSnap { hosts: None, host_conn_revs: None, rex_epoch, connected, last_used_deltas: HashMap::new(), protocol_error: Some(message) });
        }
        drop(guard);
        self.wake(proxy);
    }

    fn take(&self) -> Option<RexMailboxSnap> {
        self.pending.store(false, Ordering::Release);
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

/// Worker-side bounded host model enforcing `MAX_HOSTS`, `MAX_TOTAL_SCREENS`,
/// monotonic `rev` ordering, mandatory initial `Snapshot`, monotonic per-host
/// `host_conn_revs` preserved across coalescing (B.6), and complete offline
/// marking on Rex disconnect (Findings 14 & 16).
struct WorkerRexModel {
    hosts: Vec<RexHost>,
    rex_epoch: u64,
    /// True only until the first `Snapshot` on a new subscription completes;
    /// in-subscription lag-resync `Snapshot`s keep `rex_epoch` unchanged so
    /// Take Control isn't dropped needlessly (B.6 / #10/#13/#28).
    is_new_subscription: bool,
    /// Strictly monotonic counter assigned into `host_conn_revs` whenever any
    /// host transitions offline/online/capabilities/installed or appears new.
    next_host_rev: u64,
    host_conn_revs: HashMap<String, u64>,
    last_rev: u64,
    saw_snapshot: bool,
    connected: bool,
    last_reported_error: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum ApplyOutcome {
    /// Model changed structurally (snapshot, host added/updated/removed).
    Structural,
    /// Only one screen's `last_used` / `actions` advanced (zero-clone fast path — B.11 / #29).
    LastUsed { host: String, name: String, last_used: u64, actions: Option<usize> },
    /// Stale revision or no-op: ignored safely.
    Ignored,
    /// Protocol error line from Rex (bounded, surfaced once).
    ProtocolError(String),
    /// Protocol violation (delta before snapshot, aggregate limit exceeded):
    /// caller must drop connection and resync.
    NeedResync(String),
}

impl WorkerRexModel {
    fn new() -> Self {
        WorkerRexModel {
            hosts: Vec::new(),
            rex_epoch: 0,
            is_new_subscription: true,
            next_host_rev: 1,
            host_conn_revs: HashMap::new(),
            last_rev: 0,
            saw_snapshot: false,
            connected: false,
            last_reported_error: None,
        }
    }

    fn begin_subscription(&mut self) {
        self.saw_snapshot = false;
        self.is_new_subscription = true;
    }

    fn bump_host_rev(&mut self, key: &str) -> u64 {
        let rev = self.next_host_rev;
        self.next_host_rev = self.next_host_rev.wrapping_add(1).max(1);
        self.host_conn_revs.insert(key.to_owned(), rev);
        rev
    }

    fn total_screens(&self) -> usize {
        self.hosts.iter().map(|h| h.screens.len()).sum()
    }

    fn apply_line(&mut self, line: RexFollowLine) -> ApplyOutcome {
        match line {
            RexFollowLine::Snapshot { mut state } => {
                // Across a Rex restart or reconnect, Rex's initial baseline snapshot
                // may list saved hosts as `Connecting` / `Reconnecting` / `Disconnected`
                // with `screens: []` before `screen follow` has synced on each host.
                // Preserve last-known screens (bounded by `MAX_TOTAL_SCREENS`) for
                // hosts that are not yet connected and report empty screens so the
                // selected remote screen stays intact and resumes Live automatically
                // once the host finishes connecting.
                let prev_hosts = std::mem::take(&mut self.hosts);
                let mut total: usize = state.hosts.iter().map(|h| h.screens.len()).sum();
                let mut live_keys = HashSet::with_capacity(state.hosts.len());
                for h in &mut state.hosts {
                    live_keys.insert(h.key.clone());
                    let prev_opt = prev_hosts.iter().find(|p| p.key == h.key);
                    if !h.state.is_connected()
                        && h.screens.is_empty()
                        && let Some(prev) = prev_opt
                        && !prev.screens.is_empty()
                        && total.saturating_add(prev.screens.len()) <= MAX_TOTAL_SCREENS
                    {
                        total += prev.screens.len();
                        h.screens = prev.screens.clone();
                        if h.disconnected_at.is_none() {
                            h.disconnected_at = prev.disconnected_at;
                        }
                        if h.installed.is_none() {
                            h.installed = prev.installed.clone();
                        }
                    }
                    let transitioned = match prev_opt {
                        Some(prev) => prev.state.is_connected() != h.state.is_connected() || prev.capabilities != h.capabilities || prev.installed != h.installed,
                        None => true,
                    };
                    if transitioned || !self.host_conn_revs.contains_key(&h.key) {
                        self.bump_host_rev(&h.key);
                    }
                }
                self.host_conn_revs.retain(|k, _| live_keys.contains(k));
                self.hosts = state.hosts;
                self.last_rev = state.rev;
                self.saw_snapshot = true;
                self.connected = true;
                // B.6 (#10/#13/#28): bump `rex_epoch` ONLY on a new subscription
                // baseline Snapshot, never on an in-subscription lag-resync Snapshot
                // (so an active Take Control lease isn't dropped needlessly).
                if self.is_new_subscription {
                    self.rex_epoch = self.rex_epoch.wrapping_add(1);
                    self.is_new_subscription = false;
                }
                self.last_reported_error = None;
                ApplyOutcome::Structural
            }
            RexFollowLine::HostUpdated { rev, mut host } => {
                if !self.saw_snapshot {
                    return ApplyOutcome::NeedResync("received host_updated before initial snapshot".into());
                }
                if rev <= self.last_rev {
                    return ApplyOutcome::Ignored;
                }
                let cur_total = self.total_screens();
                if let Some(existing) = self.hosts.iter_mut().find(|h| h.key == host.key) {
                    if !host.state.is_connected() && host.screens.is_empty() && !existing.screens.is_empty() {
                        host.screens = existing.screens.clone();
                        if host.disconnected_at.is_none() {
                            host.disconnected_at = existing.disconnected_at;
                        }
                        if host.installed.is_none() {
                            host.installed = existing.installed.clone();
                        }
                    }
                    let new_total = cur_total.saturating_sub(existing.screens.len()).saturating_add(host.screens.len());
                    if new_total > MAX_TOTAL_SCREENS {
                        return ApplyOutcome::NeedResync(format!("total screen count {new_total} exceeds {MAX_TOTAL_SCREENS}"));
                    }
                    let transitioned = existing.state.is_connected() != host.state.is_connected() || existing.capabilities != host.capabilities || existing.installed != host.installed;
                    let key = host.key.clone();
                    *existing = *host;
                    if transitioned {
                        self.bump_host_rev(&key);
                    }
                } else {
                    if self.hosts.len() >= MAX_HOSTS {
                        return ApplyOutcome::NeedResync(format!("host count exceeds {MAX_HOSTS}"));
                    }
                    let new_total = cur_total.saturating_add(host.screens.len());
                    if new_total > MAX_TOTAL_SCREENS {
                        return ApplyOutcome::NeedResync(format!("total screen count {new_total} exceeds {MAX_TOTAL_SCREENS}"));
                    }
                    let key = host.key.clone();
                    self.hosts.push(*host);
                    self.bump_host_rev(&key);
                }
                self.last_rev = rev;
                ApplyOutcome::Structural
            }
            RexFollowLine::HostRemoved { rev, key } => {
                if !self.saw_snapshot {
                    return ApplyOutcome::NeedResync("received host_removed before initial snapshot".into());
                }
                if rev <= self.last_rev {
                    return ApplyOutcome::Ignored;
                }
                self.hosts.retain(|h| h.key != key);
                self.host_conn_revs.remove(&key);
                self.last_rev = rev;
                ApplyOutcome::Structural
            }
            RexFollowLine::ScreenLastUsed { host, name, last_used, actions } => {
                if !self.saw_snapshot {
                    return ApplyOutcome::NeedResync("received screen_last_used before initial snapshot".into());
                }
                if let Some(h) = self.hosts.iter_mut().find(|h| h.key == host)
                    && let Some(s) = h.screens.iter_mut().find(|s| s.name == name)
                {
                    let next_lu = s.last_used.map_or(last_used, |prev| prev.max(last_used));
                    let lu_changed = last_used > 0 && s.last_used != Some(next_lu);
                    if lu_changed {
                        s.last_used = Some(next_lu);
                    }
                    let act_changed = actions.is_some() && s.actions != actions;
                    if act_changed {
                        s.actions = actions;
                    }
                    if lu_changed || act_changed {
                        return ApplyOutcome::LastUsed { host, name, last_used: s.last_used.unwrap_or(0), actions: s.actions };
                    }
                }
                ApplyOutcome::Ignored
            }
            RexFollowLine::Error { message } => {
                let clean = rex_sock::sanitize_display_text(&message);
                if clean.is_empty() || self.last_reported_error.as_deref() == Some(clean.as_str()) {
                    ApplyOutcome::Ignored
                } else {
                    self.last_reported_error = Some(clean.clone());
                    ApplyOutcome::ProtocolError(clean)
                }
            }
        }
    }

    /// Mark every non-disconnected host (`Connected`, `Connecting`,
    /// `Reconnecting`) as `Disconnected` when Rex exits (Finding 16), advancing
    /// `host_conn_revs` so a subsequent fast reconnect still rebinds Live (B.6).
    fn mark_disconnected(&mut self, now_ms: u64) {
        self.connected = false;
        self.saw_snapshot = false;
        self.is_new_subscription = true;
        let mut bumped = Vec::new();
        for h in &mut self.hosts {
            if matches!(h.state, RexHostConn::Connected | RexHostConn::Connecting | RexHostConn::Reconnecting) {
                h.state = RexHostConn::Disconnected;
                h.disconnected_at = Some(h.disconnected_at.unwrap_or(now_ms));
                bumped.push(h.key.clone());
            }
        }
        for key in bumped {
            self.bump_host_rev(&key);
        }
    }
}

/// Unique filesystem identity `(st_dev, st_ino)` of a socket path, used to
/// detect a replacement Rex listener without restarting a healthy follow stream
/// on unrelated file activity in `olo-state/` (Finding 11).
fn socket_identity(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some((meta.dev(), meta.ino()))
}

/// Nonblocking buffered line reader for a follow `UnixStream` that can poll
/// BOTH the socket AND a wake `eventfd` simultaneously (Findings 11 & 12),
/// advancing `scanned_up_to` so large multi-chunk lines never rescan their
/// already-examined prefix (Final pass B.7 / #11).
struct PollLineReader {
    buf: Vec<u8>,
    scanned_up_to: usize,
}

enum FollowRead {
    Line(Vec<u8>),
    Eof,
    Woken,
    TimedOut,
    Err,
}

impl PollLineReader {
    fn new() -> Self {
        PollLineReader { buf: Vec::new(), scanned_up_to: 0 }
    }

    fn take_buffered_line(&mut self) -> Option<Result<Vec<u8>, io::Error>> {
        let start = self.scanned_up_to.min(self.buf.len());
        if let Some(rel) = self.buf[start..].iter().position(|&b| b == b'\n') {
            let pos = start + rel;
            if pos + 1 > MAX_RESPONSE_LINE_BYTES {
                self.buf.clear();
                self.scanned_up_to = 0;
                return Some(Err(io::Error::new(io::ErrorKind::InvalidData, format!("follow line exceeds {MAX_RESPONSE_LINE_BYTES} bytes"))));
            }
            let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
            self.scanned_up_to = 0;
            line.pop(); // '\n'
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Some(Ok(line));
        }
        self.scanned_up_to = self.buf.len();
        None
    }

    fn read_next(&mut self, stream: &UnixStream, wake_fd: RawFd, deadline: Option<Instant>, stop: &AtomicBool) -> FollowRead {
        if let Some(res) = self.take_buffered_line() {
            return match res {
                Ok(l) => FollowRead::Line(l),
                Err(_) => FollowRead::Err,
            };
        }
        let mut chunk = [0u8; 8192];
        loop {
            if stop.load(Ordering::Acquire) {
                return FollowRead::Woken;
            }
            let timeout_ms = match deadline {
                Some(dl) => {
                    let now = Instant::now();
                    if now >= dl {
                        return FollowRead::TimedOut;
                    }
                    dl.saturating_duration_since(now).as_millis().clamp(1, i32::MAX as u128) as i32
                }
                None => -1,
            };
            let mut fds = [libc::pollfd { fd: stream.as_raw_fd(), events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: wake_fd, events: libc::POLLIN, revents: 0 }];
            let rc = unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout_ms) };
            if rc < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return FollowRead::Err;
            }
            if rc == 0 {
                return FollowRead::TimedOut;
            }
            if (fds[1].revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP)) != 0 {
                return FollowRead::Woken;
            }
            if (fds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR)) != 0 {
                // SAFETY: valid nonblocking socket fd and stack buffer.
                let n = unsafe { libc::recv(stream.as_raw_fd(), chunk.as_mut_ptr().cast(), chunk.len(), 0) };
                if n == 0 {
                    return if self.buf.is_empty() { FollowRead::Eof } else { FollowRead::Err };
                }
                if n < 0 {
                    let e = io::Error::last_os_error();
                    if e.kind() == io::ErrorKind::Interrupted || e.kind() == io::ErrorKind::WouldBlock {
                        continue;
                    }
                    return FollowRead::Err;
                }
                let got = &chunk[..n as usize];
                if self.buf.len().saturating_add(got.len()) > MAX_RESPONSE_LINE_BYTES && !got.contains(&b'\n') {
                    self.buf.clear();
                    self.scanned_up_to = 0;
                    return FollowRead::Err;
                }
                self.buf.extend_from_slice(got);
                if let Some(res) = self.take_buffered_line() {
                    return match res {
                        Ok(l) => FollowRead::Line(l),
                        Err(_) => FollowRead::Err,
                    };
                }
            }
        }
    }
}

struct RexWatcher {
    _watch: Option<olo_kit::watch::Watch>,
    stop: Arc<AtomicBool>,
    wake_fd: Option<Arc<OwnedFd>>,
}

impl Drop for RexWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(fd) = &self.wake_fd {
            signal_eventfd(fd);
        }
    }
}

enum FollowPublish {
    Structural { hosts: Vec<RexHost>, host_conn_revs: HashMap<String, u64>, rex_epoch: u64, connected: bool },
    LastUsed { rex_epoch: u64, connected: bool, host: String, name: String, last_used: u64, actions: Option<usize> },
    Error { rex_epoch: u64, connected: bool, message: String },
}

/// Core Follow worker loop separated from `Proxy` so tests can verify B.4 / B.7
/// directly against real Unix sockets.
fn run_rex_follow_loop(sock_path: PathBuf, wake_bg: Arc<OwnedFd>, stop_bg: Arc<AtomicBool>, mut on_publish: impl FnMut(FollowPublish)) {
    let mut model = WorkerRexModel::new();
    let mut published_initial_offline = false;
    let follow_bytes = rex_sock::encode_bounded_json_line(&RexSockRequest::Follow, rex_sock::MAX_REQUEST_LINE_BYTES, "RexSockRequest").unwrap_or_else(|_| b"{\"op\":\"follow\"}\n".to_vec());

    while !stop_bg.load(Ordering::Acquire) {
        // Drain any accumulated discovery notifications before attempting connect.
        drain_eventfd(&wake_bg);
        let mut immediate_retry_left = 1u8;

        loop {
            if stop_bg.load(Ordering::Acquire) {
                return;
            }
            let sock_id_before = socket_identity(&sock_path);
            let hs_deadline = Instant::now() + REX_FOLLOW_HANDSHAKE_TIMEOUT;
            let connected_stream = rex_sock::connect_verified(&sock_path, hs_deadline, Some(wake_bg.as_raw_fd()), Some(&stop_bg)).and_then(|stream| {
                rex_sock::write_all_deadline(&stream, &follow_bytes, hs_deadline, Some(wake_bg.as_raw_fd()), Some(&stop_bg))?;
                Ok(stream)
            });

            let Ok(stream) = connected_stream else {
                if !published_initial_offline || model.connected {
                    model.mark_disconnected(history::now_ms());
                    on_publish(FollowPublish::Structural { hosts: model.hosts.clone(), host_conn_revs: model.host_conn_revs.clone(), rex_epoch: model.rex_epoch, connected: false });
                    published_initial_offline = true;
                }
                break;
            };

            model.begin_subscription();
            let mut reader = PollLineReader::new();
            let mut established = false;
            let mut replaced = false;

            loop {
                let deadline = if established { None } else { Some(hs_deadline) };
                match reader.read_next(&stream, wake_bg.as_raw_fd(), deadline, &stop_bg) {
                    FollowRead::Line(raw) => match serde_json::from_slice::<RexFollowLine>(&raw).map_err(|e| e.to_string()).and_then(|l| {
                        rex_sock::validate_follow_line(&l)?;
                        Ok(l)
                    }) {
                        Ok(line) => match model.apply_line(line) {
                            ApplyOutcome::Structural => {
                                established = true;
                                // B.7 (#11): reset the per-outage retry allowance once
                                // a healthy baseline has been established so a later
                                // outage an hour from now still recovers immediately.
                                immediate_retry_left = 1;
                                published_initial_offline = true;
                                on_publish(FollowPublish::Structural { hosts: model.hosts.clone(), host_conn_revs: model.host_conn_revs.clone(), rex_epoch: model.rex_epoch, connected: true });
                            }
                            ApplyOutcome::LastUsed { host, name, last_used, actions } => {
                                on_publish(FollowPublish::LastUsed { rex_epoch: model.rex_epoch, connected: true, host, name, last_used, actions });
                            }
                            ApplyOutcome::ProtocolError(msg) => {
                                on_publish(FollowPublish::Error { rex_epoch: model.rex_epoch, connected: model.connected, message: msg });
                            }
                            ApplyOutcome::Ignored => {}
                            ApplyOutcome::NeedResync(_) => break,
                        },
                        Err(_) => break,
                    },
                    FollowRead::Woken => {
                        if stop_bg.load(Ordering::Acquire) {
                            return;
                        }
                        drain_eventfd(&wake_bg);
                        // Finding 11 & B.7: keep a healthy follow untouched when
                        // unrelated sibling files change (`now_id == sock_id_before`),
                        // reconnect immediately on replacement (`now_id.is_some()`),
                        // AND treat removal (`now_id.is_none()`) as loss of the old
                        // binding even if its accepted socket remains open.
                        let now_id = socket_identity(&sock_path);
                        if now_id != sock_id_before {
                            replaced = now_id.is_some();
                            break;
                        }
                    }
                    FollowRead::Eof | FollowRead::TimedOut | FollowRead::Err => break,
                }
            }

            drop(stream);
            if stop_bg.load(Ordering::Acquire) {
                return;
            }
            if replaced {
                // Replacement Rex listener detected — reconnect immediately.
                continue;
            }
            if immediate_retry_left > 0 && socket_identity(&sock_path).is_some() {
                // Finding 11 & B.7: after a follow ends (whether established or
                // dropped/timed out before baseline), make one bounded immediate
                // reconnect attempt while the endpoint still exists before parking.
                immediate_retry_left -= 1;
                continue;
            }
            model.mark_disconnected(history::now_ms());
            on_publish(FollowPublish::Structural { hosts: model.hosts.clone(), host_conn_revs: model.host_conn_revs.clone(), rex_epoch: model.rex_epoch, connected: false });
            published_initial_offline = true;
            break;
        }

        // Sleep passively with 0 wakeups/s until `olo_kit::watch` sees Rex's
        // socket appear or change, or until `RexWatcher::drop` signals shutdown.
        let mut pfd = libc::pollfd { fd: wake_bg.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let rc = unsafe { libc::poll(&mut pfd, 1, -1) };
        if rc < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            break;
        }
    }
}

/// Watch Rex's configured socket (`paths::rex_screens_socket()` — NEVER an
/// untrusted path from a disk file) with peer-UID authentication before the
/// first byte, mandatory initial snapshot deadline, endpoint-identity change
/// detection, bounded worker-side coalescing, and 0 wakeups/s while Rex is down.
/// Never creates or chmods Rex's socket directory (Final pass B.4 / #8).
fn watch_rex(proxy: Proxy, mailbox: Arc<RexMailbox>) -> RexWatcher {
    let sock_path = paths::rex_screens_socket();
    let stop = Arc::new(AtomicBool::new(false));
    let Some(wake_fd) = open_eventfd().map(Arc::new) else {
        // Ensure `rex_ready` still settles promptly if eventfd allocation fails (Agy-review R1 #8).
        mailbox.publish_structural(&proxy, Vec::new(), HashMap::new(), 0, false);
        return RexWatcher { _watch: None, stop, wake_fd: None };
    };

    let wake_watch = wake_fd.clone();
    let watch = olo_kit::watch::watch([sock_path.clone()], move |_| {
        signal_eventfd(&wake_watch);
    });

    let stop_bg = stop.clone();
    let wake_bg = wake_fd.clone();
    let _ = std::thread::Builder::new().name("rex-follow".into()).spawn(move || {
        run_rex_follow_loop(sock_path, wake_bg, stop_bg, |ev| match ev {
            FollowPublish::Structural { hosts, host_conn_revs, rex_epoch, connected } => {
                mailbox.publish_structural(&proxy, hosts, host_conn_revs, rex_epoch, connected);
            }
            FollowPublish::LastUsed { rex_epoch, connected, host, name, last_used, actions } => {
                mailbox.publish_last_used(&proxy, rex_epoch, connected, host, name, last_used, actions);
            }
            FollowPublish::Error { rex_epoch, connected, message } => {
                mailbox.publish_error(&proxy, rex_epoch, connected, message);
            }
        });
    });

    RexWatcher { _watch: watch, stop, wake_fd: Some(wake_fd) }
}

// ---------------------------------------------------------------------------
// Remote screen metadata, capabilities, host resolution & formatting
// ---------------------------------------------------------------------------

fn host_caps_from_rex(host: &RexHost) -> Option<HostCapabilities> {
    if host.capabilities.is_empty() {
        return None;
    }
    let has = |names: &[&str]| host.capabilities.iter().any(|c| names.contains(&c.as_str()));
    Some(HostCapabilities {
        proto: 1,
        commands: vec!["follow".into(), "stream".into(), "history-stream".into(), "export".into()],
        control_ack: has(&["control-ack", "control_ack"]),
        warning_status: has(&["warning-status", "warning_status"]),
        history_frame_error: has(&["history-frame-error", "history_frame_error"]),
        history_snapshot_done: has(&["history-snapshot-done", "history_snapshot_done"]),
        export_part_ranges: has(&["export-part-ranges", "export_part_ranges"]),
        discontiguous_part: has(&["discontiguous_part", "discontiguous-part"]),
        stop_expected: has(&["stop-expected", "stop_expected"]),
        stop_expected_started: has(&["stop-expected-started", "stop_expected_started"]),
        clear_expected: has(&["clear-expected", "clear_expected"]),
        delete_expected: has(&["delete-expected", "delete_expected"]),
    })
}

fn remote_screen_info(host: &RexHost, screen: &RexScreen) -> ScreenInfo {
    let disconnected = !host.state.is_connected();
    let state = match screen.state {
        HostScreenState::Running => ScreenState::Running,
        HostScreenState::Stopped => ScreenState::Stopped,
        HostScreenState::Archived => ScreenState::Archived,
    };
    let mut caps = PaneCapabilities::remote_default();
    caps.host_disconnected = disconnected;
    caps.show_location = true;
    if disconnected {
        caps.can_control = false;
        caps.control_disabled_reason = None;
    } else if host.can_control == Some(true) {
        caps.can_control = true;
        caps.control_disabled_reason = None;
    } else if let Some(reason) = &host.control_disabled_reason {
        caps.can_control = false;
        caps.control_disabled_reason = Some(reason.clone());
    }
    if let Some(hc) = host_caps_from_rex(host) {
        hc.apply_to(&mut caps);
    }
    caps.passwords = PasswordsMode::Unavailable("Password sharing for remote host screens isn’t available yet.".into());

    ScreenInfo {
        id: ScreenId::host(&host.key, &host.label, &screen.name),
        title: screen.title.clone(),
        state,
        size: (screen.width > 0 && screen.height > 0).then_some((screen.width as i32, screen.height as i32)),
        scale: screen.scale.max(0.5),
        sound: screen.sound,
        started_ms: screen.started.map(|s| s.saturating_mul(1000)),
        last_used_ms: screen.last_used,
        disconnected_at_ms: disconnected.then_some(host.disconnected_at).flatten(),
        actions: screen.actions,
        controlled: screen.controlled,
        capabilities: caps,
    }
}

/// Concise collapsed host summary for the heading's single state slot (Design B.5 / Opus 5):
/// `"2 running"` when any run, else `"3 stopped"`, else `"1 archived"`, else `"No screens"`.
fn collapsed_summary(running: usize, stopped: usize, archived: usize) -> Option<SharedString> {
    if running > 0 {
        Some(SharedString::from(format!("{running} running")))
    } else if stopped > 0 {
        Some(SharedString::from(format!("{stopped} stopped")))
    } else if archived > 0 {
        Some(SharedString::from(format!("{archived} archived")))
    } else {
        Some(SharedString::from("No screens"))
    }
}

/// Start command shown on empty host panes (Design B.9 & Final pass B.8 / #27):
/// - Local: `olo-screen start checkout-qa`
/// - Remote with `default_on_host == true`: `olo screen start checkout-qa`
/// - Remote with `default_on_host == false`: `olo screen --on host start checkout-qa`
///   (the bridge `olo screen --on` accepts ONLY literal `host` or `desktop`, never a hostname).
fn start_command_for_host(host: Option<&RexHost>) -> String {
    let Some(h) = host else {
        return "olo-screen start checkout-qa".into();
    };
    if h.default_on_host { "olo screen start checkout-qa".into() } else { "olo screen --on host start checkout-qa".into() }
}

/// Choose the first unused name (`"agent"`, `"agent-2"`, …) on a remote host,
/// avoiding ALL known screens on that host (running, stopped, AND archived) so
/// `New Screen` never reopens an archived workspace (Final pass B.8 / #27).
fn new_remote_screen_name(screens: &[RexScreen]) -> String {
    (1..).map(|n| if n == 1 { "agent".to_owned() } else { format!("agent-{n}") }).find(|cand| !screens.iter().any(|s| s.name == *cand)).unwrap_or_default()
}

/// Whether a restored sidebar scroll offset is ready to apply: waits for BOTH
/// local and Rex discovery unconditionally so a fast local refresh can never
/// prematurely clamp the offset against a local-only list before Rex hosts
/// arrive and switch to `MultiSidebar` (Agy-review R1 #8).
fn pending_sidebar_scroll_ready(local_ready: bool, rex_ready: bool) -> bool {
    local_ready && rex_ready
}

/// AirPlay target for the current sidebar selection: enabled ONLY when a single
/// running local screen is chosen (Agy-review R1 #1).
fn airplay_shown_for(chosen: &[ScreenId], is_running: impl Fn(&ScreenId) -> bool) -> Option<olo_protocol::airplay::Shown> {
    match chosen {
        [one] if one.is_local() && is_running(one) => Some(olo_protocol::airplay::Shown::Screen { name: one.name.clone() }),
        _ => None,
    }
}

/// Alert message when ALL selected running screens lack a known `started`
/// timestamp (Agy-review R1 #12).
fn format_stop_missing_gen_alert(missing_gen: &[String]) -> String {
    if missing_gen.len() == 1 {
        format!("Can’t safely stop {} because its session start time isn’t known yet.", missing_gen[0])
    } else {
        format!("Can’t safely stop {} because their session start times aren’t known yet.", missing_gen.join(", "))
    }
}

/// Extra sentence appended to the Stop confirmation dialog when a mixed
/// multi-selection includes one or more legacy screens without a known
/// `started` timestamp (Agy-review R1 #12).
fn format_stop_missing_gen_note(missing_gen: &[String]) -> String {
    if missing_gen.is_empty() {
        String::new()
    } else if missing_gen.len() == 1 {
        format!(" Skipping {} because its session start time isn’t known yet.", missing_gen[0])
    } else {
        format!(" Skipping {} because their session start times aren’t known yet.", missing_gen.join(", "))
    }
}

/// Whether local screen `name` is archived: a running local screen is NEVER
/// considered archived even if a stray `archived` marker exists on disk
/// (Agy-review R1 #12).
fn local_screen_is_archived(name: &str, running: &[Screen], rows: &[Row]) -> bool {
    !running.iter().any(|s| s.name == name) && rows.iter().any(|r| r.name == name && r.archived)
}

/// Convert the kit's signed `ScrollHandle::offset().y` (negative when scrolled
/// down — `crates/interface/src/scroll.rs:149`) into a non-negative persisted
/// scroll distance (Final pass B.10 / #19).
fn encode_sidebar_scroll_y(offset_y: f32) -> f32 {
    (-offset_y).max(0.0)
}

/// Convert a persisted non-negative scroll distance back into the kit's signed
/// `ScrollHandle` Y offset (negative when scrolled down — Final pass B.10 / #19).
fn decode_sidebar_scroll_y(saved_y: f32) -> f32 {
    -saved_y.max(0.0)
}

/// Filter out generic boilerplate ("not installed" / "not set up") from a host's
/// `not_set_up_reason` so the pane appends a second line ONLY when the helper
/// reports specific extra detail (e.g. missing Mesa EGL or older glibc — Design B.10).
fn not_set_up_extra_reason(reason: Option<&str>) -> Option<String> {
    let r = rex_sock::sanitize_display_text(reason?);
    let trimmed = r.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("not installed") || lower.contains("aren't installed") || lower.contains("aren’t installed") || lower == "not set up" {
        return None;
    }
    Some(trimmed.to_owned())
}

/// Resolve a Rex host spec by exact `key`, then unique exact `label`, then
/// unique case-insensitive `label` (Finding 32).
fn find_host_by_spec<'a>(hosts: &'a [RexHost], spec: &str) -> Result<&'a RexHost, String> {
    if let Some(h) = hosts.iter().find(|h| h.key == spec) {
        return Ok(h);
    }
    let exact: Vec<&RexHost> = hosts.iter().filter(|h| h.label == spec).collect();
    if exact.len() == 1 {
        return Ok(exact[0]);
    }
    if exact.len() > 1 {
        return Err(format!("multiple Rex hosts are named {spec:?}"));
    }
    let ci: Vec<&RexHost> = hosts.iter().filter(|h| h.label.eq_ignore_ascii_case(spec)).collect();
    match ci.as_slice() {
        [only] => Ok(only),
        [] => Err(format!("no Rex host named {spec:?}")),
        _ => Err(format!("host name {spec:?} is ambiguous")),
    }
}

/// Open/focus `host_key` in Rex: report explicit service refusals directly via
/// alert and only fall back to launching `olo-rex --focus-host <key>` (detached
/// and reaped via `olo_xdg::open::spawn` — zero zombies) when Rex's socket is
/// unavailable (Finding 36).
fn open_host_in_rex(client: RexSockClient, host_key: String, host_label: String, proxy: Proxy) {
    std::thread::spawn(move || match client.open_host(&host_key) {
        Ok(()) => {}
        Err(OpenHostError::Refused(msg)) => {
            proxy.send(Msg::Alert(format!("Couldn’t show {host_label} in Rex: {msg}")));
        }
        Err(OpenHostError::Unavailable(_)) => {
            if let Some(bin) = olo_xdg::find_program("olo-rex") {
                let argv = [bin.into_os_string(), OsString::from("--focus-host"), OsString::from(&host_key)];
                if let Err(e) = olo_xdg::open::spawn(&argv) {
                    proxy.send(Msg::Alert(format!("Couldn’t open Rex for {host_label}: {e}")));
                }
            } else {
                proxy.send(Msg::Alert(format!("Rex isn’t installed, so {host_label} couldn’t be opened.")));
            }
        }
    });
}

enum BoundSource {
    Local { source: Arc<LocalScreenSource>, started: Option<u64> },
    Remote { source: Arc<StreamScreenSource>, started: Option<u64>, rex_epoch: u64, host_epoch: u64 },
}

impl BoundSource {
    fn as_dyn(&self) -> Arc<dyn ScreenSource> {
        match self {
            BoundSource::Local { source, .. } => source.clone(),
            BoundSource::Remote { source, .. } => source.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutlinePatch {
    Hidden,
    Unchanged,
    UpdatedInPlace,
    NeedsRebuild,
}

/// Cached multi-host outline (`entries`, `targets`, and O(1) per-screen index)
/// so a single `ScreenLastUsed` activity tick updates ONLY that one row in place
/// without re-sorting 200+ screens or rebuilding the whole outline (B.11 / #29).
struct MultiOutlineCache {
    entries: Vec<Entry>,
    entries_rc: Option<std::rc::Rc<[Entry]>>,
    targets: std::rc::Rc<[SidebarTarget]>,
    remote_index: HashMap<(String, String), usize>,
    local_index: HashMap<String, usize>,
    timestamps: Vec<Option<u64>>,
    /// `(at_inst, at_ms, soonest_ts)` for the soonest-changing short age so
    /// refreshes fire on BOTH monotonic deadline and wall-clock advancement
    /// (preventing 1 ms clock-skew or suspend-to-RAM freezes — Agy-review R2 #2).
    next_age_refresh: Option<(Instant, u64, u64)>,
}

impl MultiOutlineCache {
    fn recompute_next_refresh(&mut self, now_ms: u64, now_inst: Instant) {
        let mut best: Option<(Duration, u64)> = None;
        for (i, ts_opt) in self.timestamps.iter().enumerate() {
            let Some(ts) = *ts_opt else { continue };
            let age = Duration::from_millis(now_ms.saturating_sub(ts));
            let words = olo_kit::ago::short(age);
            let until = olo_kit::ago::until_change(age, &olo_kit::ago::short, &words);
            if let Some(Entry::Screen { value, .. }) = self.entries.get_mut(i) {
                let s = SharedString::from(words);
                if *value != Some(s.clone()) {
                    *value = Some(s);
                    self.entries_rc = None;
                }
            }
            if best.is_none_or(|(b, _)| until < b) {
                best = Some((until, ts));
            }
        }
        self.next_age_refresh = best.map(|(until, ts)| (now_inst + until, now_ms.saturating_add(until.as_millis() as u64), ts));
    }

    /// Recompute cached short ages whenever either the monotonic deadline or
    /// wall-clock deadline has been reached, returning the soonest-changing
    /// timestamp for `holds_until` scheduling from the same `now_ms`.
    fn refresh_if_due(&mut self, now_ms: u64, now_inst: Instant) -> Option<u64> {
        if self.next_age_refresh.is_some_and(|(at_inst, at_ms, _)| now_inst >= at_inst || now_ms >= at_ms) {
            self.recompute_next_refresh(now_ms, now_inst);
        }
        self.next_age_refresh.map(|(_, _, soonest_ts)| soonest_ts)
    }

    fn entries_rc(&mut self) -> std::rc::Rc<[Entry]> {
        if let Some(rc) = &self.entries_rc {
            return rc.clone();
        }
        let rc: std::rc::Rc<[Entry]> = self.entries.clone().into();
        self.entries_rc = Some(rc.clone());
        rc
    }

    /// Patch a single screen's `last_used` timestamp in place if its sort order
    /// among same-tier siblings is unchanged; returns `Unchanged` when the
    /// formatted short age (`"now"`, `"5 min"`) did not even change.
    fn patch_at(&mut self, idx: usize, name: &str, new_lu: u64, now_ms: u64, now_inst: Instant) -> OutlinePatch {
        let Some(Entry::Screen { dot, .. }) = self.entries.get(idx) else {
            return OutlinePatch::Hidden;
        };
        let dot = *dot;
        if matches!(dot, DotKind::Disconnected | DotKind::Archived) {
            return OutlinePatch::Unchanged;
        }
        // Increasing `last_used` can only invert order against the immediately
        // preceding sibling in the same host & status tier.
        if idx > 0
            && let Some(Entry::Screen { dot: prev_dot, .. }) = self.entries.get(idx - 1)
            && *prev_dot == dot
        {
            let prev_ts = self.timestamps.get(idx - 1).copied().flatten();
            let prev_name = match self.targets.get(idx - 1) {
                Some(SidebarTarget::Screen(id)) => id.name.as_str(),
                _ => "",
            };
            if (std::cmp::Reverse(prev_ts), prev_name) > (std::cmp::Reverse(Some(new_lu)), name) {
                return OutlinePatch::NeedsRebuild;
            }
        }
        self.timestamps[idx] = Some(new_lu);
        let age = Duration::from_millis(now_ms.saturating_sub(new_lu));
        let words = olo_kit::ago::short(age);
        let until = olo_kit::ago::until_change(age, &olo_kit::ago::short, &words);
        let next_at_inst = now_inst + until;
        let next_at_ms = now_ms.saturating_add(until.as_millis() as u64);
        if self.next_age_refresh.is_none_or(|(cur_inst, cur_ms, _)| next_at_inst < cur_inst || next_at_ms < cur_ms) {
            self.next_age_refresh = Some((next_at_inst, next_at_ms, new_lu));
        }
        let new_val = Some(SharedString::from(words));
        if let Some(Entry::Screen { value, .. }) = self.entries.get_mut(idx)
            && *value != new_val
        {
            *value = new_val;
            self.entries_rc = None;
            return OutlinePatch::UpdatedInPlace;
        }
        OutlinePatch::Unchanged
    }

    fn patch_remote_last_used(&mut self, host_key: &str, name: &str, new_lu: u64, now_ms: u64, now_inst: Instant) -> OutlinePatch {
        let Some(&idx) = self.remote_index.get(&(host_key.to_owned(), name.to_owned())) else {
            return OutlinePatch::Hidden;
        };
        self.patch_at(idx, name, new_lu, now_ms, now_inst)
    }

    fn patch_local_last_used(&mut self, name: &str, new_lu: u64, now_ms: u64, now_inst: Instant) -> OutlinePatch {
        let Some(&idx) = self.local_index.get(name) else {
            return OutlinePatch::Hidden;
        };
        self.patch_at(idx, name, new_lu, now_ms, now_inst)
    }
}

pub struct Viewer {
    screens: Vec<Screen>,
    rows: Vec<Row>,
    /// Remote hosts published by Rex (in Rex's canonical `host_order`), always
    /// populated from the peer-authenticated `follow` socket (never sync disk read).
    rex_hosts: Vec<RexHost>,
    rex_mailbox: Arc<RexMailbox>,
    rex_epoch: u64,
    rex_connected: bool,
    /// Monotonic structural counter incremented on every structural Rex mailbox
    /// drain so late RPC replies (`RemoteStarted`) never overwrite newer Follow state (B.8).
    rex_model_rev: u64,
    /// Per-host connection/capability generation mirrored from `WorkerRexModel::host_conn_revs`
    /// so coalesced offline->online bounces still rebind Live cleanly (B.6 / #10/#13/#28).
    host_epochs: HashMap<String, u64>,
    /// Per-host cached `--protocol` probe results (invalidated on disconnect/change).
    rex_proto_cache: HashMap<String, Arc<HostProtocolCache>>,
    /// Currently selected screen or host heading.
    selected: Option<Selection>,
    /// Requested/restored target kept pending until its own list arrives (Finding 18).
    pending_target: Option<PendingTarget>,
    local_ready: bool,
    rex_ready: bool,
    /// Folded host sections (`"local"` or `RexHost::key`), preserved across reload.
    collapsed_hosts: BTreeSet<String>,
    /// Responsive sidebar layout handle (gives way cleanly in vertical half / quarter tiles).
    side_pane: SidePane,
    /// Shared scroll handle for BOTH `LocalSidebar` and `MultiSidebar` so scroll
    /// offset persists across reload with the kit's signed convention (B.10 / #19).
    sidebar_scroll: ScrollHandle,
    /// Saved non-negative scroll distance applied AFTER rows arrive (B.10 / #19).
    pending_sidebar_scroll_y: Option<f32>,
    /// Suppress auto-reveal once right after restoring a saved scroll offset.
    suppress_sidebar_reveal: bool,
    /// Scroll handle for the selected host's screen list well (Design B.3).
    host_list_scroll: ScrollHandle,
    /// The shared pane showing the chosen screen.
    pane: Child<ScreenPane>,
    /// Last visibility state propagated to `self.pane` (Finding 6).
    pane_visible: bool,
    /// Current source bound to `pane`.
    source: Option<BoundSource>,
    busy: bool,
    /// Shared monotonic sequence number ordering local list & row snapshots.
    list_seq: Arc<AtomicU64>,
    applied_seq: u64,
    /// Retained directory + `pidfd` watcher (dropped cleanly with `Viewer`).
    list_watch: ListWatcher,
    /// Retained zero-polling Rex Screens watcher (dropped cleanly with `Viewer`).
    _rex_watch: RexWatcher,
    /// Per-screen history folder watch waking `list_watch` (Finding 20 & 23).
    used_watch: Option<olo_kit::watch::Watch>,
    used_watched_names: Vec<String>,
    proxy: Proxy,
    /// Screens chosen with the one shown (⌘- and ⇧-clicks, Select All) — O(1) lookup (Finding 29).
    also: HashSet<ScreenId>,
    anchor: Option<ScreenId>,
    /// The archived screens listed too, in a section of their own.
    show_archived: bool,
    /// Cached multi-host outline rebuilt only on structural changes (B.11 / #29).
    outline_cache: Option<MultiOutlineCache>,
    /// The window of what every screen has.
    shares_everywhere: Option<WindowHandle<Padded>>,
}

impl Viewer {
    pub fn new(show: Option<String>, cx: &mut Cx<Viewer>) -> Viewer {
        let restored_raw = olo_kit::restored_state();
        let is_restored = restored_raw.is_some();
        let saved: Saved = restored_raw.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let proxy = olo_kit::proxy();
        let list_seq = Arc::new(AtomicU64::new(0));
        let list_watch = watch_list(proxy.clone(), list_seq.clone());
        let rex_mailbox = Arc::new(RexMailbox::new());
        let rex_watch = watch_rex(proxy.clone(), rex_mailbox.clone());

        // Finding 17: never read/parse the up-to-8 MiB Rex state file on the UI
        // thread; start empty and populate asynchronously from the peer-authenticated
        // follow stream. Local running screens (small runtime dir) are quick.
        let screens = listed();
        let collapsed_hosts: BTreeSet<String> = saved.collapsed_hosts.into_iter().collect();

        // Finding 18 & 19 & B.10: on a re-exec restore, honor the saved view (including
        // any still-pending `Watch` target waiting for its host/list) and ignore
        // original startup `watch` argv; hold requested/restored targets pending
        // until their authoritative list arrives.
        let pending_target = if is_restored {
            if let Some(spec) = saved.pending_watch {
                Some(PendingTarget::Watch(spec))
            } else if let Some(id) = saved.selected_id.or_else(|| saved.selected.map(ScreenId::local)) {
                Some(PendingTarget::RestoredScreen(id))
            } else {
                saved.selected_host.map(PendingTarget::RestoredHost)
            }
        } else {
            show.map(PendingTarget::Watch)
        };

        let initial_local_selected = match &pending_target {
            Some(PendingTarget::RestoredScreen(id)) if id.is_local() && screens::valid_name(&id.name) && screens.iter().any(|x| x.name == id.name) => Some(Selection::Screen(id.clone())),
            Some(PendingTarget::Watch(spec)) if !spec.contains(':') && screens::valid_name(spec) && screens.iter().any(|x| x.name == *spec) => Some(Selection::Screen(ScreenId::local(spec))),
            None => screens.first().map(|s| Selection::Screen(ScreenId::local(&s.name))),
            _ => None,
        };

        let front = if saved.narrow_front_pane { Front::Pane } else { Front::Rest };
        let side_pane = SidePane::new(MULTI_SIDEBAR_W, MULTI_SIDEBAR_MIN_W).first(front).restored(None, saved.sidebar_collapsed);
        let sidebar_scroll = ScrollHandle::new();
        // B.10 (#19): hold saved scroll offset until rows actually arrive so
        // `ScrollView` never clamps it to 0 against an empty initial list.
        let pending_sidebar_scroll_y = (is_restored && saved.sidebar_scroll_y > 0.5).then_some(saved.sidebar_scroll_y);
        let suppress_sidebar_reveal = pending_sidebar_scroll_y.is_some();

        let pane = cx.new_view(|cx| {
            let mut p = ScreenPane::new(saved.tab, cx);
            p.set_host_lifecycle(true);
            p
        });
        cx.subscribe(&pane, |v: &mut Viewer, e: &ScreenPaneEvent, cx| match e {
            ScreenPaneEvent::StartRequested(_) => {
                v.start_chosen(cx);
            }
            ScreenPaneEvent::StopRequested(_) => {
                v.ask_stop(cx);
            }
            ScreenPaneEvent::Started(info) => {
                v.refresh_all();
                v.select(Some(Selection::Screen(info.id.clone())), cx);
                cx.invalidate();
            }
            ScreenPaneEvent::Stopped(_) | ScreenPaneEvent::Cleared(_) | ScreenPaneEvent::RenameFailed(_) => {
                v.refresh_all();
                cx.invalidate();
            }
            ScreenPaneEvent::Renamed(id, title) => {
                // Finding 26 & Final pass B.8: Follow owns remote metadata in
                // `self.rex_hosts`, so only update local rows here (a late Rename
                // reply must never overwrite newer Follow state).
                if id.is_local()
                    && let Some(r) = v.rows.iter_mut().find(|r| r.name == id.name)
                {
                    r.title = title.clone();
                    v.outline_cache = None;
                }
                cx.invalidate();
            }
            ScreenPaneEvent::Used(id, at) => {
                let now_ms = history::now_ms();
                let now_inst = Instant::now();
                let mut touched = false;
                if id.is_local() {
                    if let Some(r) = v.rows.iter_mut().find(|r| r.name == id.name)
                        && r.last_used.is_none_or(|t| t < *at)
                    {
                        r.last_used = Some(*at);
                        touched = true;
                        if let Some(cache) = v.outline_cache.as_mut()
                            && matches!(cache.patch_local_last_used(&id.name, *at, now_ms, now_inst), OutlinePatch::NeedsRebuild)
                        {
                            v.outline_cache = None;
                        }
                    }
                } else if let ScreenLocation::Host { host_id, .. } = &id.location
                    && let Some(h) = v.rex_hosts.iter_mut().find(|h| h.key == *host_id)
                    && let Some(s) = h.screens.iter_mut().find(|s| s.name == id.name)
                    && s.last_used.is_none_or(|t| t < *at)
                {
                    s.last_used = Some(*at);
                    touched = true;
                    if let Some(cache) = v.outline_cache.as_mut()
                        && matches!(cache.patch_remote_last_used(host_id, &id.name, *at, now_ms, now_inst), OutlinePatch::NeedsRebuild)
                    {
                        v.outline_cache = None;
                    }
                }
                if touched {
                    cx.invalidate();
                }
            }
            ScreenPaneEvent::OpenHost(host_key) => {
                let label = v.host_label_for(host_key);
                open_host_in_rex(v.rex_client(), host_key.clone(), label, v.proxy.clone());
            }
            ScreenPaneEvent::Controlling(_, _) | ScreenPaneEvent::TabChanged(_) => {
                cx.invalidate();
            }
        });
        let mut v = Viewer {
            screens,
            rows: Vec::new(),
            rex_hosts: Vec::new(),
            rex_mailbox,
            rex_epoch: 0,
            rex_connected: false,
            rex_model_rev: 0,
            host_epochs: HashMap::new(),
            rex_proto_cache: HashMap::new(),
            selected: initial_local_selected,
            pending_target,
            local_ready: false,
            rex_ready: false,
            collapsed_hosts,
            side_pane,
            sidebar_scroll,
            pending_sidebar_scroll_y,
            suppress_sidebar_reveal,
            host_list_scroll: ScrollHandle::new(),
            pane,
            pane_visible: true,
            source: None,
            busy: false,
            list_seq,
            applied_seq: 0,
            list_watch,
            _rex_watch: rex_watch,
            used_watch: None,
            used_watched_names: Vec::new(),
            proxy,
            also: HashSet::new(),
            anchor: None,
            show_archived: !saved.hide_archived,
            outline_cache: None,
            shares_everywhere: None,
        };
        v.sync_pane(cx);
        v
    }

    fn has_remote_hosts(&self) -> bool {
        !self.rex_hosts.is_empty()
    }

    /// Always bind RPCs to the configured display socket (`paths::rex_screens_socket()`),
    /// authenticated via `SO_PEERCRED` before writing (Findings 1 & 9).
    fn rex_client(&self) -> RexSockClient {
        RexSockClient::new()
    }

    fn host_label_for(&self, host_key: &str) -> String {
        if host_key == LOCAL_HOST_KEY {
            return "This Computer".into();
        }
        self.rex_hosts.iter().find(|h| h.key == host_key).map_or_else(|| host_key.to_owned(), |h| h.label.clone())
    }

    fn selected_screen(&self) -> Option<&ScreenId> {
        match &self.selected {
            Some(Selection::Screen(id)) => Some(id),
            _ => None,
        }
    }

    /// Host key targeted by `Screen ▸ New Screen…` / `⌘N` (Finding 27):
    /// if a startable remote host heading or one of its screens is selected,
    /// target that host; otherwise `LOCAL_HOST_KEY`.
    fn selected_startable_host(&self) -> String {
        let candidate = match &self.selected {
            Some(Selection::Host(k)) if k != LOCAL_HOST_KEY => Some(k.as_str()),
            Some(Selection::Screen(id)) => match &id.location {
                ScreenLocation::Host { host_id, .. } => Some(host_id.as_str()),
                ScreenLocation::Local => None,
            },
            _ => None,
        };
        if let Some(key) = candidate
            && self.rex_hosts.iter().any(|h| h.key == key && h.can_start_screen())
        {
            return key.to_owned();
        }
        LOCAL_HOST_KEY.to_owned()
    }

    /// Apply a saved sidebar scroll offset once BOTH local and Rex discovery
    /// have settled so the kit's `ScrollView` never clamps it against an empty
    /// or premature local-only list before `MultiSidebar` mounts (B.10 / #19;
    /// Agy-review R1 #8).
    fn apply_pending_sidebar_scroll(&mut self, cx: &mut Cx<Self>) {
        let Some(saved_y) = self.pending_sidebar_scroll_y else { return };
        if !pending_sidebar_scroll_ready(self.local_ready, self.rex_ready) {
            return;
        }
        self.pending_sidebar_scroll_y = None;
        self.suppress_sidebar_reveal = true;
        self.sidebar_scroll.set_offset_once_laid_out(cx, point(px(0.0), px(decode_sidebar_scroll_y(saved_y))));
    }

    /// Resolve a CLI/launch target (`NAME` or `HOST:NAME` split on the LAST colon)
    /// using exact host key -> exact label -> unique case-insensitive label
    /// strictly against already-enumerated local/remote lists (never filesystem
    /// probing ahead of `local_ready` — Final pass B.10 / #18, #32).
    fn resolve_target(&self, spec: &str) -> Result<ScreenId, String> {
        if let Some((host_spec, name)) = spec.rsplit_once(':') {
            if !screens::valid_name(name) {
                return Err(format!("invalid screen name {name:?}"));
            }
            let host = find_host_by_spec(&self.rex_hosts, host_spec)?;
            if host.screens.iter().any(|s| s.name == name) {
                return Ok(ScreenId::host(&host.key, &host.label, name));
            }
            return Err(format!("no screen {name:?} on {}", host.label));
        }
        if screens::valid_name(spec) && (self.screens.iter().any(|s| s.name == spec) || self.rows.iter().any(|r| r.name == spec)) {
            return Ok(ScreenId::local(spec));
        }
        Err(format!("no local screen named {spec:?}"))
    }

    fn expand_host_of(&mut self, id: &ScreenId) {
        let removed = match &id.location {
            ScreenLocation::Local => self.collapsed_hosts.remove(LOCAL_HOST_KEY),
            ScreenLocation::Host { host_id, .. } => self.collapsed_hosts.remove(host_id),
        };
        if removed {
            self.outline_cache = None;
        }
    }

    /// Choose `name` (from a forwarded `olo-screen watch [HOST:]NAME` launch),
    /// clearing secondary selections (Finding 7) and holding it pending if its
    /// source list hasn't arrived yet (Finding 18).
    pub fn show(&mut self, name: Option<String>, cx: &mut Cx<Self>) {
        if let Some(spec) = name {
            self.also.clear();
            self.anchor = None;
            self.pending_target = Some(PendingTarget::Watch(spec));
            self.try_resolve_pending(cx);
        }
        cx.activate_window();
    }

    /// Attempt to satisfy `self.pending_target` from whichever authoritative
    /// list has arrived (Final pass B.10 / #18, #32):
    /// - Never resolves a local target before `self.local_ready`.
    /// - Retains a remote target while its host is offline / connecting / reconnecting.
    /// - Surfaces `resolve_target` errors (e.g. ambiguous host labels) once authoritative.
    fn try_resolve_pending(&mut self, cx: &mut Cx<Self>) {
        let Some(target) = self.pending_target.clone() else { return };
        match &target {
            PendingTarget::Watch(spec) => {
                if !target.is_remote() {
                    if !self.local_ready {
                        return;
                    }
                    match self.resolve_target(spec) {
                        Ok(id) => {
                            self.pending_target = None;
                            self.also.clear();
                            self.anchor = Some(id.clone());
                            self.expand_host_of(&id);
                            self.select(Some(Selection::Screen(id)), cx);
                            self.pane.update(cx, |p, cx| p.set_tab(Tab::Live, cx));
                        }
                        Err(err) => {
                            self.pending_target = None;
                            self.alert(&format!("Couldn’t open “{spec}”: {err}"), cx);
                        }
                    }
                    return;
                }
                if !self.rex_ready {
                    return;
                }
                match self.resolve_target(spec) {
                    Ok(id) => {
                        self.pending_target = None;
                        self.also.clear();
                        self.anchor = Some(id.clone());
                        self.expand_host_of(&id);
                        self.select(Some(Selection::Screen(id)), cx);
                        self.pane.update(cx, |p, cx| p.set_tab(Tab::Live, cx));
                    }
                    Err(err) => {
                        if !self.rex_connected {
                            return;
                        }
                        // Retain pending remote target if the named host exists but is not
                        // yet connected (non-authoritative offline / connecting state).
                        if let Some((h_spec, _)) = spec.rsplit_once(':')
                            && let Ok(h) = find_host_by_spec(&self.rex_hosts, h_spec)
                            && !h.state.is_connected()
                        {
                            return;
                        }
                        // Authoritative miss or ambiguous host label: clear and report!
                        self.pending_target = None;
                        self.alert(&format!("Couldn’t open “{spec}”: {err}"), cx);
                    }
                }
            }
            PendingTarget::RestoredScreen(id) => {
                if id.is_local() && !self.local_ready {
                    return;
                }
                if !id.is_local() && !self.rex_ready {
                    return;
                }
                let exists = match &id.location {
                    ScreenLocation::Local => self.screens.iter().any(|s| s.name == id.name) || self.rows.iter().any(|r| r.name == id.name),
                    ScreenLocation::Host { host_id, .. } => self.find_remote(host_id, &id.name).is_some(),
                };
                if exists {
                    let resolved = match &id.location {
                        ScreenLocation::Local => id.clone(),
                        ScreenLocation::Host { host_id, .. } => self.find_remote(host_id, &id.name).map_or_else(|| id.clone(), |(h, _)| ScreenId::host(&h.key, &h.label, &id.name)),
                    };
                    self.pending_target = None;
                    self.select(Some(Selection::Screen(resolved)), cx);
                } else if id.is_local()
                    || (self.rex_connected
                        && !match &id.location {
                            ScreenLocation::Host { host_id, .. } => self.rex_hosts.iter().find(|h| h.key == *host_id).is_some_and(|h| !h.state.is_connected()),
                            ScreenLocation::Local => false,
                        })
                {
                    self.pending_target = None;
                }
            }
            PendingTarget::RestoredHost(k) => {
                if !self.rex_ready {
                    return;
                }
                let exists = self.has_remote_hosts() && (k == LOCAL_HOST_KEY || self.rex_hosts.iter().any(|h| h.key == *k));
                if exists {
                    self.pending_target = None;
                    self.select(Some(Selection::Host(k.clone())), cx);
                } else if self.rex_connected {
                    self.pending_target = None;
                }
            }
        }
    }

    /// The window closed, or Quit: let go of the screen, and end.
    pub fn quit(&mut self, cx: &mut Cx<Self>) {
        self.pane.update(cx, |p, cx| p.stop_all(cx));
        olo_kit::quit();
    }

    /// What to keep over an upgrade (reload in place: screen or host heading,
    /// collapsed hosts, active tab, narrow front pane, sidebar scroll offset —
    /// never Take Control).
    pub fn saved(&self, cx: &WindowCx) -> String {
        let tab = self.pane.read(cx).tab();
        let (selected, selected_id, selected_host) = match &self.selected {
            Some(Selection::Screen(id)) => (id.is_local().then(|| id.name.clone()), Some(id.clone()), None),
            Some(Selection::Host(k)) => (None, None, Some(k.clone())),
            None => match &self.pending_target {
                Some(PendingTarget::RestoredScreen(id)) => (id.is_local().then(|| id.name.clone()), Some(id.clone()), None),
                Some(PendingTarget::RestoredHost(k)) => (None, None, Some(k.clone())),
                _ => (None, None, None),
            },
        };
        let pending_watch = match &self.pending_target {
            Some(PendingTarget::Watch(spec)) => Some(spec.clone()),
            _ => None,
        };
        let scroll_y = self.pending_sidebar_scroll_y.unwrap_or_else(|| encode_sidebar_scroll_y(f32::from(self.sidebar_scroll.offset().y)));
        serde_json::to_string(&Saved {
            selected,
            selected_id,
            selected_host,
            pending_watch,
            collapsed_hosts: self.collapsed_hosts.iter().cloned().collect(),
            tab,
            hide_archived: !self.show_archived,
            sidebar_collapsed: self.side_pane.collapsed(),
            narrow_front_pane: self.side_pane.alone() && self.side_pane.showing(),
            sidebar_scroll_y: scroll_y,
        })
        .unwrap_or_default()
    }

    /// Trigger an ordered local refresh through the serialized `screen-list-watch`
    /// worker (Finding 22).
    fn refresh_all(&self) {
        self.list_watch.poke();
    }

    fn set_busy(&mut self, busy: bool, cx: &mut Cx<Self>) {
        self.busy = busy;
        self.pane.update(cx, |p, cx| p.set_busy(busy, cx));
    }

    fn local_info_for(&self, name: &str) -> ScreenInfo {
        let running = self.screens.iter().find(|s| s.name == name);
        let row = self.rows.iter().find(|r| r.name == name);
        let mut info = local::screen_info_for(name, running, row.and_then(|r| r.title.clone()), row.is_some_and(|r| r.archived), row.and_then(|r| r.last_used));
        info.capabilities.show_location = self.has_remote_hosts();
        info
    }

    fn find_remote(&self, host_id: &str, name: &str) -> Option<(&RexHost, &RexScreen)> {
        let host = self.rex_hosts.iter().find(|h| h.key == host_id)?;
        let screen = host.screens.iter().find(|s| s.name == name)?;
        Some((host, screen))
    }

    fn proto_cache_for(&mut self, host_key: &str, caps: Option<HostCapabilities>) -> Arc<HostProtocolCache> {
        if let Some(existing) = self.rex_proto_cache.get(host_key) {
            return existing.clone();
        }
        let cache = match caps {
            Some(hc) => Arc::new(HostProtocolCache::fixed(hc)),
            None => Arc::new(HostProtocolCache::new()),
        };
        self.rex_proto_cache.insert(host_key.to_owned(), cache.clone());
        cache
    }

    fn sync_pane(&mut self, cx: &mut Cx<Self>) {
        let Some(id) = self.selected_screen().cloned() else {
            if self.source.take().is_some() {
                self.pane.update(cx, |p, cx| p.set_source(None, cx));
            }
            return;
        };
        match &id.location {
            ScreenLocation::Local => {
                let running_started = self.screens.iter().find(|s| s.name == id.name).map(|s| s.started);
                let info = self.local_info_for(&id.name);
                if let Some(BoundSource::Local { source, started }) = &self.source
                    && source.id() == &id
                    && *started == running_started
                {
                    source.set_info(info.clone());
                    source.set_session_started(running_started);
                    self.pane.update(cx, |p, cx| p.update_info(info, cx));
                } else {
                    let src = Arc::new(LocalScreenSource::new(info));
                    src.set_session_started(running_started);
                    let dyn_src: Arc<dyn ScreenSource> = src.clone();
                    self.source = Some(BoundSource::Local { source: src, started: running_started });
                    self.pane.update(cx, |p, cx| p.set_source(Some(dyn_src), cx));
                }
            }
            ScreenLocation::Host { host_id, .. } => {
                let Some((host, screen)) = self.find_remote(host_id, &id.name) else {
                    if self.source.take().is_some() {
                        self.pane.update(cx, |p, cx| p.set_source(None, cx));
                    }
                    return;
                };
                // Avoid cloning the entire host + all its screens (Finding 29).
                let host_key = host.key.clone();
                let started = screen.started;
                let info = remote_screen_info(host, screen);
                let host_caps = host_caps_from_rex(host);
                let cur_rex_epoch = self.rex_epoch;
                let cur_host_epoch = self.host_epochs.get(&host_key).copied().unwrap_or(0);

                // Findings 5 & 13: rebind whenever screen session `started`,
                // Rex socket epoch, or host connection/capability epoch changes.
                if let Some(BoundSource::Remote { source, started: bound_started, rex_epoch, host_epoch }) = &self.source
                    && source.id() == &id
                    && *bound_started == started
                    && *rex_epoch == cur_rex_epoch
                    && *host_epoch == cur_host_epoch
                {
                    source.set_session_started(started);
                    source.set_info(info.clone());
                    self.pane.update(cx, |p, cx| p.update_info(info, cx));
                } else {
                    let cache = self.proto_cache_for(&host_key, host_caps);
                    // B.1 (#5): bind `open_live` to this session's `started` generation so
                    // reconnect retries never attach to a replacement session under the same name.
                    let opener = Arc::new(RexSockOpener::new(self.rex_client(), &host_key).with_expected_started(started));
                    let source = Arc::new(StreamScreenSource::with_host_cache(info, opener, cache));
                    source.set_session_started(started);
                    let dyn_src: Arc<dyn ScreenSource> = source.clone();
                    self.source = Some(BoundSource::Remote { source, started, rex_epoch: cur_rex_epoch, host_epoch: cur_host_epoch });
                    self.pane.update(cx, |p, cx| p.set_source(Some(dyn_src), cx));
                }
            }
        }
    }

    fn select(&mut self, sel: Option<Selection>, cx: &mut Cx<Self>) {
        if self.selected != sel {
            self.selected = sel;
            self.sync_pane(cx);
            cx.invalidate();
        }
    }

    fn belongs_to_host(id: &ScreenId, host_key: &str) -> bool {
        match &id.location {
            ScreenLocation::Local => host_key == LOCAL_HOST_KEY,
            ScreenLocation::Host { host_id, .. } => host_id == host_key,
        }
    }

    fn toggle_host_collapsed(&mut self, host_key: &str, cx: &mut Cx<Self>) {
        if self.collapsed_hosts.contains(host_key) {
            self.collapsed_hosts.remove(host_key);
        } else {
            self.collapsed_hosts.insert(host_key.to_owned());
            // Finding 7: prune ALL hidden children of the collapsed host from
            // secondary selections and anchor so bulk Stop/Delete never reach
            // screens hidden inside a folded host.
            self.also.retain(|id| !Self::belongs_to_host(id, host_key));
            if self.anchor.as_ref().is_some_and(|id| Self::belongs_to_host(id, host_key)) {
                self.anchor = None;
            }
            let was_inside = matches!(&self.selected, Some(Selection::Screen(id)) if Self::belongs_to_host(id, host_key));
            if was_inside {
                self.also.clear();
                self.select(Some(Selection::Host(host_key.to_owned())), cx);
            }
        }
        self.outline_cache = None;
        cx.invalidate();
    }

    fn alert(&self, message: &str, cx: &mut Cx<Self>) {
        open_alert(cx, "Agent Screens", "stop", message, &["OK"], |_, _| {});
    }

    fn new_screen(&mut self, cx: &mut Cx<Self>) {
        if self.busy {
            return;
        }
        self.set_busy(true, cx);
        let proxy = self.proxy.clone();
        std::thread::spawn(move || proxy.send(Msg::Started(screens::start(&screens::new_name(), &screens::Options::default()))));
        cx.invalidate();
    }

    fn new_screen_on(&mut self, host_key: &str, cx: &mut Cx<Self>) {
        if self.busy {
            return;
        }
        if host_key == LOCAL_HOST_KEY {
            self.new_screen(cx);
            return;
        }
        let Some(host) = self.rex_hosts.iter().find(|h| h.key == host_key) else {
            return;
        };
        if !host.can_start_screen() {
            self.alert(&format!("Can’t start a screen on {} right now.", host.label), cx);
            return;
        }
        let key = host.key.clone();
        let label = host.label.clone();
        // B.8 (#27): pick an unused name avoiding running, stopped, AND archived
        // screens on this host so `New Screen` never reopens an archived workspace.
        let new_name = new_remote_screen_name(&host.screens);
        let captured_rev = self.rex_model_rev;
        let client = self.rex_client();
        let proxy = self.proxy.clone();
        self.set_busy(true, cx);
        std::thread::spawn(move || {
            let res = client.start_screen(&key, Some(&new_name), false, None, None, false);
            proxy.send(Msg::RemoteStarted(key, label, captured_rev, res));
        });
        cx.invalidate();
    }

    fn start_chosen(&mut self, cx: &mut Cx<Self>) {
        let targets: Vec<ScreenId> = self.chosen().into_iter().filter(|id| !self.is_id_disconnected(id) && !self.is_id_running(id)).collect();
        if self.busy || targets.is_empty() {
            return;
        }
        self.set_busy(true, cx);
        let shown = self.selected_screen().cloned();
        let mut order = targets;
        if let Some(i) = shown.as_ref().and_then(|s| order.iter().position(|n| n == s)) {
            let s = order.remove(i);
            order.push(s);
        }
        let captured_rev = self.rex_model_rev;
        let client = self.rex_client();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            enum LastStarted {
                Local(Screen),
                Remote(String, String, RexScreen),
            }
            let mut failed = Vec::new();
            // Finding 27: single ordered `last_ok` so the last item (the shown
            // screen) always wins regardless of local vs remote mix.
            let mut last_ok: Option<LastStarted> = None;
            for id in &order {
                match &id.location {
                    ScreenLocation::Local => match screens::start(&id.name, &screens::kept(&id.name).unwrap_or_default()) {
                        Ok(s) => last_ok = Some(LastStarted::Local(s)),
                        Err(e) => failed.push(format!("{}: {e}", id.name)),
                    },
                    // Finding 27: pass `keep: true` on named remote restarts.
                    ScreenLocation::Host { host_id, host_label } => match client.start_screen(host_id, Some(&id.name), true, None, None, false) {
                        Ok(sc) => last_ok = Some(LastStarted::Remote(host_id.clone(), host_label.clone(), sc)),
                        Err(e) => failed.push(format!("{} on {host_label}: {e}", id.name)),
                    },
                }
            }
            if !failed.is_empty() && last_ok.is_some() {
                proxy.send(Msg::Changed(failed.clone()));
            }
            match last_ok {
                Some(LastStarted::Remote(hk, hl, sc)) => proxy.send(Msg::RemoteStarted(hk, hl, captured_rev, Ok(sc))),
                Some(LastStarted::Local(s)) => proxy.send(Msg::Started(Ok(s))),
                None => proxy.send(Msg::Started(Err(failed.join("\n")))),
            }
        });
        cx.invalidate();
    }

    fn single_screen_target_label(&self, id: &ScreenId, shown: &str) -> String {
        if self.has_remote_hosts() {
            match &id.location {
                ScreenLocation::Local => format!("“{shown}” on This Computer"),
                ScreenLocation::Host { host_id, host_label } => {
                    // B.10 (#32): include stable host key when displayed host labels collide.
                    let unique = find_host_by_spec(&self.rex_hosts, host_label).ok().map(|h| h.key.as_str()) == Some(host_id.as_str());
                    if unique { format!("“{shown}” on {host_label}") } else { format!("“{shown}” on {host_label} ({host_id})") }
                }
            }
        } else {
            format!("“{shown}”")
        }
    }

    /// Ask, once for all, before the running screens chosen are stopped.
    /// Captures `(expected_started, expected_last_used)` via the shared
    /// `capture_stop_expectations`, keys remote Stop on `"stop-expected"`, and
    /// verifies bound session generation before releasing Take Control
    /// (Final pass B.2 / B.3 / Astra #2–#5, #7, #24).
    fn ask_stop(&mut self, cx: &mut Cx<Self>) {
        type StopTarget = (ScreenId, Option<u64>, Option<u64>, String, Option<bool>);
        let pane_latest = self.pane.read(cx).latest_used(cx);
        let mut missing_gen = Vec::new();
        let targets: Vec<StopTarget> = self
            .chosen()
            .into_iter()
            .filter(|id| !self.is_id_disconnected(id))
            .filter_map(|id| {
                let replay_lu = pane_latest.as_ref().filter(|(pid, _)| pid == &id).map(|(_, ts)| *ts);
                match &id.location {
                    ScreenLocation::Local => self.screens.iter().find(|s| s.name == id.name).and_then(|s| {
                        let row = self.rows.iter().find(|r| r.name == id.name);
                        let shown = row.map_or(id.name.as_str(), Row::shown).to_owned();
                        let (exp_s, exp_lu) = capture_stop_expectations(true, Some(s.started), replay_lu, row.and_then(|r| r.last_used), None).ok()?;
                        Some((id, exp_s, exp_lu, shown, None))
                    }),
                    ScreenLocation::Host { host_id, .. } => self.find_remote(host_id, &id.name).filter(|(_, s)| s.is_running()).and_then(|(h, s)| {
                        let shown = s.display_name().to_owned();
                        let Ok((exp_s, exp_lu)) = capture_stop_expectations(false, s.started, replay_lu, s.last_used, s.actions) else {
                            missing_gen.push(self.single_screen_target_label(&id, &shown));
                            return None;
                        };
                        let has_stop_expected = if h.capabilities.is_empty() { None } else { Some(h.capabilities.iter().any(|c| c == "stop-expected" || c == "stop_expected")) };
                        Some((id, exp_s, exp_lu, shown, has_stop_expected))
                    }),
                }
            })
            .collect();
        if !missing_gen.is_empty() && targets.is_empty() {
            self.alert(&format_stop_missing_gen_alert(&missing_gen), cx);
            return;
        }
        let skipped_note = format_stop_missing_gen_note(&missing_gen);
        let (message, button) = match targets.as_slice() {
            [] => return,
            [(id, _, _, shown, _)] => {
                let target = self.single_screen_target_label(id, shown);
                (
                    format!(
                        "Stop the screen {target}? The agent’s programs on it are closed, and anything it hasn’t saved is lost. The screen is kept, with its history: Start Screen starts it again.{skipped_note}"
                    ),
                    "Stop Screen".to_owned(),
                )
            }
            many => (
                format!(
                    "Stop {} screens? The agents’ programs on them are closed, and anything they haven’t saved is lost. The screens are kept, with their histories: Start starts them again.{skipped_note}",
                    many.len()
                ),
                format!("Stop {} Screens", many.len()),
            ),
        };
        let me = cx.view_ref();
        open_alert(cx, "Stop Screen", "warning", &message, &["Cancel", &button], move |choice, cx| {
            if *choice != Some(1) {
                return;
            }
            let targets = targets.clone();
            me.update(cx, move |v, cx| {
                // B.2 (#5/#24): only release Take Control if the currently bound
                // source still matches BOTH screen ID AND captured `expected_started`
                // (never drop control on a replacement session under the same name!).
                let bound_started = match &v.source {
                    Some(BoundSource::Local { started, .. }) | Some(BoundSource::Remote { started, .. }) => *started,
                    None => None,
                };
                let matches_bound = v.selected_screen().is_some_and(|sel| targets.iter().any(|(id, exp_s, _, _, _)| id == sel && *exp_s == bound_started));
                let controlled_src = if matches_bound {
                    v.pane.update(cx, |p, cx| p.stop_controlling(cx));
                    v.source.as_ref().map(BoundSource::as_dyn)
                } else {
                    None
                };
                let client = v.rex_client();
                let (proxy, targets) = (v.proxy.clone(), targets.clone());
                std::thread::spawn(move || {
                    if let Some(src) = controlled_src {
                        let _ = src.release_control_and_wait(Vec::new());
                    }
                    let mut failed = Vec::new();
                    for (id, expected_started, expected_last_used, _, has_stop_expected) in &targets {
                        let res = match &id.location {
                            ScreenLocation::Local => screens::stop_if_expected(&id.name, *expected_started, *expected_last_used),
                            ScreenLocation::Host { host_id, host_label } => {
                                // B.3 (#2–#4): guarded Stop requires `"stop-expected"` (not
                                // `"stop-expected-started"` alone).
                                let supported = match has_stop_expected {
                                    Some(v) => *v,
                                    None => client.protocol(host_id).is_ok_and(|(_, _, caps)| caps.iter().any(|c| c == "stop-expected" || c == "stop_expected")),
                                };
                                if !supported {
                                    Err(format!("Update Olo on “{host_label}” to stop a screen from here."))
                                } else {
                                    client.stop_screen(host_id, &id.name, *expected_started, *expected_last_used)
                                }
                            }
                        };
                        if let Err(e) = res {
                            failed.push(e);
                        }
                    }
                    proxy.send(Msg::Stopped(if failed.is_empty() { Ok(()) } else { Err(failed.join("\n")) }));
                });
                cx.invalidate();
            });
        });
    }

    fn apply_rex_mailbox(&mut self, cx: &mut Cx<Self>) {
        let Some(snap) = self.rex_mailbox.take() else { return };
        let prev_epoch = self.rex_epoch;
        self.rex_epoch = snap.rex_epoch;
        self.rex_connected = snap.connected;
        self.rex_ready = true;

        if let Some(err) = snap.protocol_error {
            self.alert(&format!("Rex screens service: {err}"), cx);
        }

        // B.11 (#29): zero-clone fast path when only `ScreenLastUsed` deltas arrived!
        // Mutates ONLY the affected screen(s) in-place, patches the cached outline
        // in O(1) without sorting all hosts, skips `cx.invalidate()` completely when
        // an unselected screen's displayed short age didn't change, and calls
        // `sync_pane` at most once if the currently selected screen was touched.
        let Some(new_hosts) = snap.hosts else {
            let sel = self.selected_screen().cloned();
            let now_ms = history::now_ms();
            let now_inst = Instant::now();
            let mut selected_touched = false;
            let mut visible_touched = false;
            for ((host_key, name), (lu, actions)) in snap.last_used_deltas {
                if let Some(h) = self.rex_hosts.iter_mut().find(|h| h.key == host_key)
                    && let Some(s) = h.screens.iter_mut().find(|s| s.name == name)
                {
                    let prev_lu = s.last_used;
                    if lu > 0 {
                        s.last_used = Some(s.last_used.map_or(lu, |prev| prev.max(lu)));
                    }
                    if actions.is_some() {
                        s.actions = actions;
                    }
                    let effective_lu = s.last_used.unwrap_or(0);
                    let is_sel = sel.as_ref().is_some_and(|id| matches!(&id.location, ScreenLocation::Host { host_id, .. } if host_id == &host_key) && id.name == name);
                    if is_sel {
                        selected_touched = true;
                        visible_touched = true;
                    }
                    if effective_lu > 0 {
                        if let Some(cache) = self.outline_cache.as_mut() {
                            match cache.patch_remote_last_used(&host_key, &name, effective_lu, now_ms, now_inst) {
                                OutlinePatch::Hidden | OutlinePatch::Unchanged => {}
                                OutlinePatch::UpdatedInPlace => visible_touched = true,
                                OutlinePatch::NeedsRebuild => {
                                    self.outline_cache = None;
                                    visible_touched = true;
                                }
                            }
                        } else if !self.collapsed_hosts.contains(&host_key) && (self.show_archived || s.state != HostScreenState::Archived) {
                            visible_touched = true;
                        }
                    }
                    // If the host summary pane (`Selection::Host(host_key)`) is open,
                    // only redraw if its displayed short age text actually changed.
                    if !visible_touched && matches!(&self.selected, Some(Selection::Host(hk)) if hk == &host_key) {
                        let old_word = prev_lu.map(|t| olo_kit::ago::short(Duration::from_millis(now_ms.saturating_sub(t))));
                        let new_word = s.last_used.map(|t| olo_kit::ago::short(Duration::from_millis(now_ms.saturating_sub(t))));
                        if old_word != new_word {
                            visible_touched = true;
                        }
                    }
                }
            }
            if selected_touched {
                self.sync_pane(cx);
            }
            if visible_touched {
                cx.invalidate();
            }
            return;
        };

        self.rex_model_rev = self.rex_model_rev.wrapping_add(1);
        self.rex_hosts = new_hosts;
        self.outline_cache = None;

        // B.6 (#10/#13/#28): compare worker's monotonic `host_conn_revs` against
        // `self.host_epochs` so even an offline->online bounce coalesced inside
        // `RexMailbox` still evicts the protocol cache and rebinds Live!
        if self.rex_epoch != prev_epoch {
            self.rex_proto_cache.clear();
        }
        if let Some(conn_revs) = snap.host_conn_revs {
            for (k, rev) in &conn_revs {
                if self.host_epochs.get(k) != Some(rev) {
                    self.rex_proto_cache.remove(k);
                    self.host_epochs.insert(k.clone(), *rev);
                }
            }
            self.rex_proto_cache.retain(|k, _| conn_revs.contains_key(k));
            self.host_epochs.retain(|k, _| conn_revs.contains_key(k));
        }

        self.try_resolve_pending(cx);
        self.keep_choice_listed(cx);
        self.apply_pending_sidebar_scroll(cx);
    }

    fn told(&mut self, message: Box<dyn std::any::Any + Send>, cx: &mut Cx<Self>) {
        // Another start: `olo-screen [watch [HOST:]NAME]`.
        if let Some(launch) = message.downcast_ref::<olo_kit::instance::Launch>() {
            self.show(launch.args.get(1).cloned(), cx);
            return;
        }
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Refreshed(seq, screens, rows) => {
                if !accept_list_seq(&mut self.applied_seq, seq, true) {
                    return;
                }
                self.screens = screens;
                self.rows = rows;
                self.local_ready = true;
                self.outline_cache = None;
                self.follow_use();
                self.try_resolve_pending(cx);
                self.keep_choice_listed(cx);
                self.apply_pending_sidebar_scroll(cx);
                return;
            }
            Msg::Changed(failed) => {
                if !failed.is_empty() {
                    self.alert(&failed.join("\n"), cx);
                }
                self.refresh_all();
            }
            Msg::Started(r) => {
                self.set_busy(false, cx);
                match r {
                    Ok(screen) => {
                        if !self.screens.contains(&screen) {
                            self.screens.push(screen.clone());
                        }
                        self.pending_target = None;
                        self.collapsed_hosts.remove(LOCAL_HOST_KEY);
                        self.outline_cache = None;
                        self.select(Some(Selection::Screen(ScreenId::local(screen.name))), cx);
                        self.pane.update(cx, |p, cx| p.set_tab(Tab::Live, cx));
                        return;
                    }
                    Err(e) => self.alert(&format!("The screen couldn’t be started: {e}"), cx),
                }
            }
            Msg::RemoteStarted(host_key, host_label, captured_rev, r) => {
                self.set_busy(false, cx);
                match r {
                    Ok(screen) => {
                        let name = screen.name.clone();
                        // B.8 (#26/#27): Follow owns remote metadata! Only insert a
                        // provisional `screen` when NO newer structural Follow update
                        // arrived while Start was in flight; otherwise honor what
                        // Follow currently says (never resurrect a stopped screen).
                        let follow_moved_on = self.rex_model_rev != captured_rev;
                        let is_running_now = if !follow_moved_on {
                            if let Some(h) = self.rex_hosts.iter_mut().find(|h| h.key == host_key) {
                                if let Some(existing) = h.screens.iter_mut().find(|s| s.name == name) {
                                    *existing = screen;
                                } else {
                                    h.screens.push(screen);
                                }
                            }
                            true
                        } else {
                            self.find_remote(&host_key, &name).is_some_and(|(_, s)| s.is_running())
                        };
                        if self.find_remote(&host_key, &name).is_some() {
                            self.pending_target = None;
                            self.collapsed_hosts.remove(&host_key);
                            self.outline_cache = None;
                            self.select(Some(Selection::Screen(ScreenId::host(host_key, host_label, name))), cx);
                            if is_running_now {
                                self.pane.update(cx, |p, cx| p.set_tab(Tab::Live, cx));
                            }
                        }
                        return;
                    }
                    Err(e) => self.alert(&format!("The screen on {host_label} couldn’t be started: {e}"), cx),
                }
            }
            Msg::Stopped(r) => {
                if let Err(e) = r {
                    self.alert(&e, cx);
                }
                self.refresh_all();
            }
            Msg::RexUpdated => {
                self.apply_rex_mailbox(cx);
                return;
            }
            Msg::Alert(msg) => {
                self.alert(&msg, cx);
            }
        }
        self.sync_pane(cx);
        cx.invalidate();
    }

    fn menus(&self, cx: &WindowCx) -> Vec<Menu> {
        let st = self.pane.read(cx).menu_state(cx);
        let running = st.running;
        let chosen = self.selected_screen().is_some();
        let tab = st.shown_tab;
        let history = st.has_history;
        let playing = st.playing;
        let any_listed = self.has_any_visible_screen();
        let startable_remotes: Vec<(String, String)> = self.rex_hosts.iter().filter(|h| h.can_start_screen()).map(|h| (h.key.clone(), h.label.clone())).collect();

        // Finding 27: when remote hosts can start screens, `Screen ▸ New Screen`
        // routes `⌘N` through the currently selected host (or This Computer) AND
        // keeps explicit "New Screen on This Computer…" + destination submenu.
        let mut screen_base = Menu::new("Screen").app_items(&["airplay", "terminal"]);
        if startable_remotes.is_empty() {
            screen_base = screen_base.item("New Screen…", "new").key("n").enabled(!self.busy);
        } else {
            let target_host = self.selected_startable_host();
            let target_label = if target_host == LOCAL_HOST_KEY { "New Screen on This Computer…".to_owned() } else { format!("New Screen on {}…", self.host_label_for(&target_host)) };
            let target_cmd = if target_host == LOCAL_HOST_KEY { "new-local".to_owned() } else { format!("new-host:{target_host}") };
            screen_base = screen_base.item(target_label, target_cmd).key("n").enabled(!self.busy);
            let mut sub = Menu::new("New Screen On").item("This Computer", "new-local").enabled(!self.busy).separator();
            for (key, label) in startable_remotes {
                sub = sub.item(label, format!("new-host:{key}")).enabled(!self.busy);
            }
            screen_base = screen_base.submenu(sub);
        }
        let screen = self.chosen_items(screen_base.item("Passwords for Every Screen…", "shares-everywhere").separator());
        let sidebar_label = if self.side_pane.showing() { "Hide Sidebar" } else { "Show Sidebar" };
        vec![
            screen
                .item("Clear History…", "clear")
                .enabled(st.can_clear)
                .separator()
                .item("Export Video…", "export")
                .key("e")
                .enabled(st.can_export)
                .item("Take Screenshot…", "screenshot")
                .shift_key("e")
                .enabled(st.can_screenshot)
                .close_quit(),
            Menu::new("Edit")
                .item("Select All Screens", "select-all")
                .enabled(any_listed)
                .item("Deselect All Screens", "deselect")
                .enabled(!self.also.is_empty())
                .separator()
                .item("Find…", "find")
                .key("f")
                .enabled(chosen),
            Menu::new("View")
                .item("Live", "tab-live")
                .key("1")
                .enabled(running)
                .checked(tab == Tab::Live)
                .item("History", "tab-history")
                .key("2")
                .enabled(chosen)
                .checked(tab == Tab::History)
                .item("Stats", "tab-stats")
                .key("3")
                .enabled(chosen)
                .checked(tab == Tab::Stats)
                .separator()
                .item(sidebar_label, "sidebar")
                .key("s")
                .item("Show Archived", "show-archived")
                .checked(self.show_archived),
            Menu::new("Playback")
                .item(if playing { "Pause" } else { "Play" }, "play")
                .enabled(history)
                .item("Previous Action", "previous")
                .enabled(history)
                .item("Next Action", "next")
                .enabled(history)
                .separator()
                .item(if running { "Go Live" } else { "Go to Latest" }, "latest")
                .enabled(history),
            {
                let control = Menu::new("Control").item(if st.controlling { "Stop Controlling" } else { "Take Control" }, "control").key("k").enabled(st.can_control);
                // Design B.12 (Opus 13): `Move Windows To…` can never move local
                // Wayland windows to a remote host, so it is hidden on remote
                // screens and shown only where `st.can_move_windows` is true.
                if olo_kit::moving::menus_offer() && st.can_move_windows { control.separator().submenu(self.pane.read(cx).give_menu()) } else { control }
            },
        ]
    }

    fn run(&mut self, command: &str, cx: &mut Cx<Self>) {
        // Agy-review R1 #1: dispatch `Show on AirPlay` menu items for a single
        // running local screen before falling through to `ScreenPane::run_command`.
        if let Some(shown) = airplay_shown_for(&self.chosen(), |id| self.is_id_running(id))
            && olo_kit::airplay::show_from_menu(command, &shown)
        {
            return;
        }
        if let Some(host_key) = command.strip_prefix("new-host:") {
            self.new_screen_on(host_key, cx);
            return;
        }
        if let Some(host_key) = command.strip_prefix("open-host:") {
            let label = self.host_label_for(host_key);
            open_host_in_rex(self.rex_client(), host_key.to_owned(), label, self.proxy.clone());
            return;
        }
        match command {
            "new" => {
                let target = self.selected_startable_host();
                self.new_screen_on(&target, cx);
            }
            "new-local" => self.new_screen(cx),
            "shares-everywhere" => shares::open_everywhere(&mut self.shares_everywhere),
            "archive" => self.archive(true, cx),
            "start-stopped" => self.start_chosen(cx),
            "unarchive" => self.archive(false, cx),
            "delete" => self.ask_delete(cx),
            "select-all" => self.select_all(cx),
            "deselect" => self.also.clear(),
            "sidebar" => self.side_pane.toggle(),
            "show-archived" => {
                self.show_archived = !self.show_archived;
                self.outline_cache = None;
                self.keep_choice_listed(cx);
            }
            "stop" => self.ask_stop(cx),
            "close" => cx.close_window(),
            "quit" => self.quit(cx),
            other => {
                self.pane.update(cx, |p, cx| {
                    p.run_command(other, cx);
                });
            }
        }
        cx.invalidate();
    }

    /// Local-only list sections (`Running`, `Stopped`, `Archived`).
    fn local_groups(&self) -> Vec<(&'static str, Vec<&Row>)> {
        let running = |n: &str| self.screens.iter().any(|s| s.name == n);
        let mut groups = vec![("Running", self.rows.iter().filter(|r| running(&r.name)).collect()), ("Stopped", self.rows.iter().filter(|r| !running(&r.name) && !r.archived).collect())];
        if self.show_archived {
            groups.push(("Archived", self.rows.iter().filter(|r| !running(&r.name) && r.archived).collect()));
        }
        groups
    }

    /// Visible screens of a remote host ordered Running -> Stopped -> Archived,
    /// most recently used first within each tier.
    fn host_visible_screens<'a>(&self, host: &'a RexHost) -> Vec<&'a RexScreen> {
        let mut list: Vec<&RexScreen> = host.screens.iter().filter(|s| self.show_archived || s.state != HostScreenState::Archived).collect();
        list.sort_by_key(|s| {
            let rank = match s.state {
                HostScreenState::Running => 0u8,
                HostScreenState::Stopped => 1u8,
                HostScreenState::Archived => 2u8,
            };
            (rank, std::cmp::Reverse(s.last_used), s.name.as_str())
        });
        list
    }

    /// All screens visible in the current outline (skipping children of folded
    /// hosts in multi-host mode — Finding 7).
    fn visible_listed_ids(&self) -> Vec<ScreenId> {
        let multi = self.has_remote_hosts();
        let mut ids = Vec::new();
        if !multi || !self.collapsed_hosts.contains(LOCAL_HOST_KEY) {
            ids.extend(self.local_groups().into_iter().flat_map(|(_, rs)| rs.into_iter().map(|r| ScreenId::local(&r.name))));
        }
        for h in &self.rex_hosts {
            if self.collapsed_hosts.contains(&h.key) {
                continue;
            }
            for s in self.host_visible_screens(h) {
                ids.push(ScreenId::host(&h.key, &h.label, &s.name));
            }
        }
        ids
    }

    /// Fast O(1)-amortized check whether at least one screen is visible in the
    /// outline without sorting or allocating every host's screens (B.11 / #29).
    fn has_any_visible_screen(&self) -> bool {
        let multi = self.has_remote_hosts();
        if (!multi || !self.collapsed_hosts.contains(LOCAL_HOST_KEY)) && self.rows.iter().any(|r| self.show_archived || !r.archived || self.screens.iter().any(|s| s.name == r.name)) {
            return true;
        }
        self.rex_hosts.iter().filter(|h| !self.collapsed_hosts.contains(&h.key)).any(|h| h.screens.iter().any(|s| self.show_archived || s.state != HostScreenState::Archived))
    }

    /// Whether a single screen `id` is currently visible in the outline.
    fn is_id_visible(&self, id: &ScreenId) -> bool {
        let multi = self.has_remote_hosts();
        match &id.location {
            ScreenLocation::Local => {
                if multi && self.collapsed_hosts.contains(LOCAL_HOST_KEY) {
                    return false;
                }
                self.rows.iter().any(|r| r.name == id.name && (self.show_archived || !r.archived || self.screens.iter().any(|s| s.name == r.name)))
            }
            ScreenLocation::Host { host_id, .. } => {
                !self.collapsed_hosts.contains(host_id) && self.find_remote(host_id, &id.name).is_some_and(|(_, s)| self.show_archived || s.state != HostScreenState::Archived)
            }
        }
    }

    /// The screens chosen: the visible shown screen and visible screens chosen
    /// with it, in outline order (Finding 7). Takes a zero-sort fast path when
    /// no secondary rows are selected (B.11 / #29).
    fn chosen(&self) -> Vec<ScreenId> {
        let sel = self.selected_screen();
        if self.also.is_empty() {
            return sel.filter(|id| self.is_id_visible(id)).cloned().into_iter().collect();
        }
        self.visible_listed_ids().into_iter().filter(|id| sel == Some(id) || self.also.contains(id)).collect()
    }

    fn is_id_running(&self, id: &ScreenId) -> bool {
        match &id.location {
            ScreenLocation::Local => self.screens.iter().any(|s| s.name == id.name),
            ScreenLocation::Host { host_id, .. } => self.find_remote(host_id, &id.name).is_some_and(|(h, s)| h.state.is_connected() && s.is_running()),
        }
    }

    fn is_id_archived(&self, id: &ScreenId) -> bool {
        match &id.location {
            ScreenLocation::Local => local_screen_is_archived(&id.name, &self.screens, &self.rows),
            ScreenLocation::Host { host_id, .. } => self.find_remote(host_id, &id.name).is_some_and(|(_, s)| s.state == HostScreenState::Archived),
        }
    }

    fn is_id_disconnected(&self, id: &ScreenId) -> bool {
        match &id.location {
            ScreenLocation::Local => false,
            ScreenLocation::Host { host_id, .. } => self.rex_hosts.iter().find(|h| h.key == *host_id).is_none_or(|h| !h.state.is_connected()),
        }
    }

    /// A ⌘-click (`run`: a ⇧-click) on `id`: over the visible outline only (Finding 7).
    fn pick(&mut self, id: ScreenId, run: bool, cx: &mut Cx<Self>) {
        let sel = self.selected_screen().cloned();
        if run {
            let order = self.visible_listed_ids();
            let from = self.anchor.clone().or_else(|| sel.clone()).and_then(|f| order.iter().position(|n| *n == f));
            if let Some(to) = order.iter().position(|n| *n == id) {
                let from = from.unwrap_or(to);
                self.also = order[from.min(to)..=from.max(to)].iter().filter(|n| sel.as_ref() != Some(*n)).cloned().collect();
            }
        } else if sel.as_ref() != Some(&id) {
            if !self.also.remove(&id) {
                self.also.insert(id.clone());
            }
            self.anchor = Some(id);
        }
        cx.invalidate();
    }

    fn select_all(&mut self, cx: &mut Cx<Self>) {
        let sel = self.selected_screen();
        self.also = self.visible_listed_ids().into_iter().filter(|n| sel != Some(n)).collect();
        cx.invalidate();
    }

    fn row_menu(&mut self, id: ScreenId, at: Point<Pixels>, cx: &mut Cx<Self>) {
        if !self.chosen().contains(&id) {
            self.also.clear();
            self.anchor = Some(id.clone());
            self.select(Some(Selection::Screen(id)), cx);
        }
        let menu = self.chosen_items(Menu::new(""));
        self.menu_at(menu, at, cx);
        cx.invalidate();
    }

    fn host_menu(&mut self, host_key: &str, at: Point<Pixels>, cx: &mut Cx<Self>) {
        if host_key == LOCAL_HOST_KEY {
            let menu = Menu::new("").item("New Screen on This Computer…", "new-local").enabled(!self.busy);
            self.menu_at(menu, at, cx);
            return;
        }
        let Some(host) = self.rex_hosts.iter().find(|h| h.key == host_key) else { return };
        let mut menu = Menu::new("");
        if host.can_start_screen() {
            menu = menu.item(format!("New Screen on {}…", host.label), format!("new-host:{}", host.key)).enabled(!self.busy).separator();
        }
        menu = menu.item("Show in Rex", format!("open-host:{}", host.key));
        self.menu_at(menu, at, cx);
    }

    fn menu_at(&mut self, menu: Menu, at: Point<Pixels>, cx: &mut Cx<Self>) {
        let me = cx.view_ref();
        olo_kit::menu::context_menu(cx, menu, at, move |cmd, cx| {
            let cmd = cmd.to_owned();
            me.update(cx, move |v, cx| v.run(&cmd, cx));
        });
    }

    fn chosen_items(&self, menu: Menu) -> Menu {
        let chosen = self.chosen();
        let active: Vec<&ScreenId> = chosen.iter().filter(|id| !self.is_id_disconnected(id)).collect();
        let count = |f: &dyn Fn(&ScreenId) -> bool| active.iter().filter(|id| f(id)).count();
        let running = |id: &ScreenId| self.is_id_running(id);
        let archived = |id: &ScreenId| self.is_id_archived(id);
        let (stop, archive, unarchive, delete) = (count(&running), count(&|id| !running(id) && !archived(id)), count(&archived), count(&|id| !running(id)));
        let say = |verb: &str, k: usize, dots: &str| if k > 1 { format!("{verb} {k} Screens{dots}") } else { format!("{verb} Screen{dots}") };
        let menu = match chosen.as_slice() {
            [one] if one.is_local() && running(one) => menu.submenu(Menu::airplay("Show on AirPlay", &olo_kit::airplay::paired(), "")).separator(),
            _ => menu,
        };
        menu.item("Rename…", "rename")
            .enabled(active.len() == 1 && chosen.len() == 1)
            .separator()
            .item(say("Start", delete, ""), "start-stopped")
            .enabled(delete > 0 && !self.busy)
            .item(say("Archive", archive, ""), "archive")
            .enabled(archive > 0)
            .item(say("Unarchive", unarchive, ""), "unarchive")
            .enabled(unarchive > 0)
            .item(say("Stop", stop, "…"), "stop")
            .enabled(stop > 0)
            .separator()
            .item(say("Delete", delete, "…"), "delete")
            .enabled(delete > 0)
    }

    fn archive(&mut self, on: bool, cx: &mut Cx<Self>) {
        let targets: Vec<ScreenId> = self.chosen().into_iter().filter(|id| !self.is_id_disconnected(id) && !self.is_id_running(id) && self.is_id_archived(id) != on).collect();
        if targets.is_empty() {
            return;
        }
        // Finding 26: never mutate `rex_hosts` optimistically before the remote
        // archive RPC succeeds (Rex's `follow` stream updates authoritative state
        // on success, and a refusal leaves zero invented state on screen).
        for id in &targets {
            if id.is_local()
                && let Some(r) = self.rows.iter_mut().find(|r| r.name == id.name)
            {
                r.archived = on;
                self.outline_cache = None;
            }
        }
        let client = self.rex_client();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let mut failed = Vec::new();
            for id in &targets {
                let res = match &id.location {
                    ScreenLocation::Local => history::set_archived(&id.name, on),
                    ScreenLocation::Host { host_id, .. } => client.archive(host_id, &id.name, on),
                };
                if let Err(e) = res {
                    failed.push(e);
                }
            }
            proxy.send(Msg::Changed(failed));
        });
        self.keep_choice_listed(cx);
    }

    /// Ask before deleting stopped screens. Captures `expected_started = Some(0)`
    /// (stopped) and `expected_last_used = Some(last_used.unwrap_or(0))` (`0` =
    /// observed empty) by value at dialog-open time for BOTH local and remote
    /// screens (Findings 2, 3, 7).
    fn ask_delete(&mut self, cx: &mut Cx<Self>) {
        let targets: Vec<(ScreenId, Option<u64>, Option<u64>, String)> = self
            .chosen()
            .into_iter()
            .filter(|id| !self.is_id_disconnected(id) && !self.is_id_running(id))
            .map(|id| {
                let (last_used, shown) = match &id.location {
                    ScreenLocation::Local => {
                        let r = self.rows.iter().find(|r| r.name == id.name);
                        (r.and_then(|r| r.last_used), r.map_or(id.name.as_str(), Row::shown).to_owned())
                    }
                    ScreenLocation::Host { host_id, .. } => self.find_remote(host_id, &id.name).map_or((None, id.name.clone()), |(_, s)| (s.last_used, s.display_name().to_owned())),
                };
                // `Some(0)` = observed stopped / observed empty history.
                (id, Some(0), Some(last_used.unwrap_or(0)), shown)
            })
            .collect();
        let (message, button) = match targets.as_slice() {
            [] => return,
            [(id, _, _, shown)] => {
                let target = self.single_screen_target_label(id, shown);
                (format!("Delete the screen {target}? Its history, recordings and stats go to the Trash."), "Delete Screen".to_owned())
            }
            many => (format!("Delete {} screens? Their histories, recordings and stats go to the Trash.", many.len()), format!("Delete {} Screens", many.len())),
        };
        let me = cx.view_ref();
        open_alert(cx, "Delete", "warning", &message, &["Cancel", &button], move |choice, cx| {
            if *choice != Some(1) {
                return;
            }
            let targets = targets.clone();
            me.update(cx, move |v, cx| {
                let client = v.rex_client();
                let (proxy, targets) = (v.proxy.clone(), targets.clone());
                std::thread::spawn(move || {
                    let mut failed = Vec::new();
                    for (id, expected_started, expected_last_used, _) in &targets {
                        let res = match &id.location {
                            ScreenLocation::Local => screens::delete_if_stopped_expected(&id.name, *expected_started, *expected_last_used),
                            ScreenLocation::Host { host_id, .. } => client.delete(host_id, &id.name, *expected_started, *expected_last_used),
                        };
                        if let Err(e) = res {
                            failed.push(e);
                        }
                    }
                    proxy.send(Msg::Changed(failed));
                });
                cx.invalidate();
            });
        });
    }

    fn keep_choice_listed(&mut self, cx: &mut Cx<Self>) {
        let visible = self.visible_listed_ids();
        let visible_set: HashSet<&ScreenId> = visible.iter().collect();
        self.also.retain(|n| visible_set.contains(n));
        if self.anchor.as_ref().is_some_and(|a| !visible_set.contains(a)) {
            self.anchor = None;
        }
        // Finding 18: do NOT discard a pending selection before its own
        // authoritative list has arrived.
        let keep_pending_local = self.pending_target.as_ref().is_some_and(|p| !p.is_remote()) && !self.local_ready;
        let keep_pending_remote = self.pending_target.as_ref().is_some_and(PendingTarget::is_remote) && !self.rex_ready;
        if keep_pending_local || keep_pending_remote {
            self.sync_pane(cx);
            cx.invalidate();
            return;
        }

        let valid = match &self.selected {
            Some(Selection::Screen(id)) => {
                // Keep a local screen valid while `local_ready` hasn't finished yet
                // if `self.screens` already saw it, or if it's in `visible_set`.
                visible_set.contains(id) || (id.is_local() && !self.local_ready && self.screens.iter().any(|s| s.name == id.name))
            }
            Some(Selection::Host(k)) => self.has_remote_hosts() && (k == LOCAL_HOST_KEY || self.rex_hosts.iter().any(|h| h.key == *k)),
            None => false,
        };
        if !valid {
            let next = visible.first().cloned().map(Selection::Screen).or_else(|| self.has_remote_hosts().then(|| Selection::Host(LOCAL_HOST_KEY.into())));
            self.pane.update(cx, |p, cx| p.stop_controlling(cx));
            self.selected = next;
            self.sync_pane(cx);
        } else {
            self.sync_pane(cx);
        }
        cx.invalidate();
    }

    /// Watch every listed local screen's history directory (`history::dir(&r.name)`)
    /// so external `title`, `archived`, `screen.json`, and `actions.jsonl` writes,
    /// truncations, or removals trigger a tail-only refresh on the worker thread
    /// while ignoring recorder frames (`frames-*.olof`), `recorder.lock`,
    /// `pictures/`, and `logs/` (B.11 / Astra #20 & #23).
    fn follow_use(&mut self) {
        let mut names: Vec<String> = self.rows.iter().map(|r| r.name.clone()).collect();
        names.sort();
        if names == self.used_watched_names && self.used_watch.is_some() {
            return;
        }
        self.used_watched_names = names;
        let paths: Vec<PathBuf> = self.used_watched_names.iter().map(|n| history::dir(n)).collect();
        let watched_for_filter = paths.clone();
        let wake = self.list_watch.wake_tx.clone();
        let proxy = self.proxy.clone();
        let seq = self.list_seq.clone();
        self.used_watch = olo_kit::watch::watch(paths, move |changed| {
            if !screen_history_event_relevant(changed, &watched_for_filter) {
                return;
            }
            if let Some(fd) = &wake {
                signal_eventfd(fd);
            } else {
                let p = proxy.clone();
                let sq = seq.clone();
                std::thread::spawn(move || {
                    let s_num = sq.fetch_add(1, Ordering::SeqCst) + 1;
                    let s = listed();
                    let r = rows(&s);
                    p.send(Msg::Refreshed(s_num, s, r));
                });
            }
        });
    }

    /// Local-only sidebar (no Rex hosts configured: keeps the exact kit `Sidebar`
    /// look without a `This Computer` heading while sharing `self.sidebar_scroll`
    /// for reload restore and allowing narrow click on already-selected rows —
    /// Final pass B.10 / #19, #25).
    fn local_sidebar(&mut self, side_w: f32, alone: bool, cx: &mut Cx<Self>) -> Element {
        let latest = self.pane.read(cx).latest_used(cx).filter(|(id, _)| id.is_local()).map(|(id, t)| (id.name, t));
        let last_used = |r: &Row| match &latest {
            Some((n, t)) if *n == r.name => Some(r.last_used.map_or(*t, |prev| prev.max(*t))),
            _ => r.last_used,
        };
        let groups = self.local_groups();
        let mut items = Vec::new();
        let mut names: Vec<Option<String>> = Vec::new();
        for (heading, rs) in &groups {
            if rs.is_empty() {
                continue;
            }
            items.push(sidebar::Row::heading(*heading));
            names.push(None);
            for r in rs {
                let row = sidebar::Row::new("agent-screens", r.shown().to_owned());
                items.push(match last_used(r) {
                    Some(t) => row.value(olo_kit::ago::ago_ms(t, olo_kit::ago::short)),
                    None => row,
                });
                names.push(Some(r.name.clone()));
            }
        }
        let selected_name = self.selected_screen().filter(|id| id.is_local()).map(|id| id.name.as_str());
        let first = names.iter().position(Option::is_some).unwrap_or(0);
        let chosen = names.iter().position(|n| n.as_deref() == selected_name).unwrap_or(first);
        let also: Vec<usize> = names.iter().enumerate().filter(|(_, n)| n.as_ref().is_some_and(|n| self.also.contains(&ScreenId::local(n)))).map(|(i, _)| i).collect();
        let names = std::rc::Rc::new(names);
        let (n1, n2, n3) = (names.clone(), names.clone(), names.clone());
        let suppress = self.suppress_sidebar_reveal || self.pending_sidebar_scroll_y.is_some();
        if self.pending_sidebar_scroll_y.is_none() {
            self.suppress_sidebar_reveal = false;
        }
        let list = LocalSidebar::new("screens", items, chosen)
            .bare(true)
            .edge(!alone)
            .also(also)
            .scroll(self.sidebar_scroll.clone())
            .suppress_reveal(suppress)
            .on_choose(cx.listener(move |v: &mut Viewer, i: &usize, cx| {
                if let Some(Some(name)) = n1.get(*i) {
                    v.pending_target = None;
                    v.also.clear();
                    let id = ScreenId::local(name);
                    v.anchor = Some(id.clone());
                    v.select(Some(Selection::Screen(id)), cx);
                    if v.side_pane.alone() {
                        v.side_pane.show_rest();
                    }
                    cx.invalidate();
                }
            }))
            .on_pick(cx.listener(move |v: &mut Viewer, (i, mods): &(usize, Modifiers), cx| {
                if let Some(Some(name)) = n2.get(*i) {
                    v.pick(ScreenId::local(name), mods.shift, cx);
                }
            }))
            .on_menu(cx.listener(move |v: &mut Viewer, (i, at): &(usize, Point<Pixels>), cx| {
                if let Some(Some(name)) = n3.get(*i) {
                    v.row_menu(ScreenId::local(name), *at, cx);
                }
            }))
            .on_select_all(cx.listener(|v: &mut Viewer, _: &(), cx| v.select_all(cx)))
            .build(cx);
        self.wrap_sidebar(side_w, alone, list, false, cx)
    }

    /// Build the cached multi-host outline once per structural change (B.11 / #29).
    fn build_multi_outline(&self, latest: Option<&(ScreenId, u64)>, now_ms: u64, now_inst: Instant) -> MultiOutlineCache {
        let local_last_used = |r: &Row| match latest {
            Some((id, t)) if id.is_local() && id.name == r.name => Some(r.last_used.map_or(*t, |prev| prev.max(*t))),
            _ => r.last_used,
        };
        let remote_last_used = |host_key: &str, s: &RexScreen| match latest {
            Some((id, t)) if matches!(&id.location, ScreenLocation::Host { host_id, .. } if host_id == host_key) && id.name == s.name => Some(s.last_used.map_or(*t, |prev| prev.max(*t))),
            _ => s.last_used,
        };

        let mut entries: Vec<Entry> = Vec::new();
        let mut targets: Vec<SidebarTarget> = Vec::new();
        let mut timestamps: Vec<Option<u64>> = Vec::new();
        let mut remote_index: HashMap<(String, String), usize> = HashMap::new();
        let mut local_index: HashMap<String, usize> = HashMap::new();

        // Section 1: This Computer (always first in canonical order).
        let local_groups = self.local_groups();
        let local_running = local_groups.iter().find(|(g, _)| *g == "Running").map_or(0, |(_, rs)| rs.len());
        let local_stopped = local_groups.iter().find(|(g, _)| *g == "Stopped").map_or(0, |(_, rs)| rs.len());
        let local_archived = local_groups.iter().find(|(g, _)| *g == "Archived").map_or(0, |(_, rs)| rs.len());
        let local_total = local_running + local_stopped + local_archived;
        let local_open = !self.collapsed_hosts.contains(LOCAL_HOST_KEY);
        let local_slot = if local_total == 0 {
            Some(SharedString::from("No screens"))
        } else if !local_open {
            collapsed_summary(local_running, local_stopped, local_archived)
        } else {
            None
        };
        entries.push(Entry::Heading {
            key: LOCAL_HOST_KEY.into(),
            label: "This Computer".into(),
            icon: "desktop".into(),
            disconnected: false,
            open: local_open,
            has_children: local_total > 0,
            status: local_slot,
            status_error: false,
            agent_name: "This Computer".into(),
        });
        targets.push(SidebarTarget::Host(LOCAL_HOST_KEY.into()));
        timestamps.push(None);
        if local_open {
            for (group_name, rs) in &local_groups {
                for r in rs {
                    let ts = if *group_name == "Archived" { None } else { local_last_used(r) };
                    let (dot, state_word, val) = match *group_name {
                        "Running" => (DotKind::Running, "running", ts.map(|t| SharedString::from(olo_kit::ago::short(Duration::from_millis(now_ms.saturating_sub(t)))))),
                        "Archived" => (DotKind::Archived, "archived", Some(SharedString::from("Archived"))),
                        _ => (DotKind::Stopped, "stopped", ts.map(|t| SharedString::from(olo_kit::ago::short(Duration::from_millis(now_ms.saturating_sub(t)))))),
                    };
                    let shown = r.shown().to_owned();
                    let idx = entries.len();
                    local_index.insert(r.name.clone(), idx);
                    entries.push(Entry::Screen { label: shown.clone().into(), value: val, dot, agent_name: format!("{shown} · This Computer · {state_word}").into() });
                    targets.push(SidebarTarget::Screen(ScreenId::local(&r.name)));
                    timestamps.push(ts);
                }
            }
        }

        // Sections 2..N: Remote Rex hosts in Rex's canonical `host_order`.
        for host in &self.rex_hosts {
            let visible = self.host_visible_screens(host);
            let open = !self.collapsed_hosts.contains(&host.key);
            let has_children = !visible.is_empty();
            let disconnected = !host.state.is_connected();

            let (slot, status_error, agent_heading) = match host.state {
                RexHostConn::Error => (Some(SharedString::from("Error")), true, format!("{} (error)", host.label)),
                RexHostConn::Disconnected => (Some(SharedString::from("Disconnected")), false, format!("{} (disconnected)", host.label)),
                RexHostConn::Reconnecting => (Some(SharedString::from("Reconnecting…")), false, format!("{} (reconnecting)", host.label)),
                RexHostConn::Connecting => (Some(SharedString::from("Connecting…")), false, format!("{} (connecting)", host.label)),
                RexHostConn::Connected if !host.has_olo_screens() => (Some(SharedString::from("Not set up")), false, format!("{} (not set up)", host.label)),
                RexHostConn::Connected if visible.is_empty() => (Some(SharedString::from("No screens")), false, format!("{} (no screens)", host.label)),
                RexHostConn::Connected if !open => {
                    let r = visible.iter().filter(|s| s.state == HostScreenState::Running).count();
                    let st = visible.iter().filter(|s| s.state == HostScreenState::Stopped).count();
                    let ar = visible.iter().filter(|s| s.state == HostScreenState::Archived).count();
                    (collapsed_summary(r, st, ar), false, host.label.clone())
                }
                RexHostConn::Connected => (None, false, host.label.clone()),
            };

            entries.push(Entry::Heading {
                key: host.key.clone().into(),
                label: host.label.clone().into(),
                icon: "rex".into(),
                disconnected: disconnected || !host.has_olo_screens(),
                open,
                has_children,
                status: slot,
                status_error,
                agent_name: agent_heading.into(),
            });
            targets.push(SidebarTarget::Host(host.key.clone()));
            timestamps.push(None);

            if open && has_children {
                for s in visible {
                    let shown = s.display_name().to_owned();
                    let ts = if disconnected || s.state == HostScreenState::Archived { None } else { remote_last_used(&host.key, s) };
                    let (dot, state_word, val) = if disconnected {
                        (DotKind::Disconnected, "disconnected", None)
                    } else {
                        match s.state {
                            HostScreenState::Running => (DotKind::Running, "running", ts.map(|t| SharedString::from(olo_kit::ago::short(Duration::from_millis(now_ms.saturating_sub(t)))))),
                            HostScreenState::Archived => (DotKind::Archived, "archived", Some(SharedString::from("Archived"))),
                            HostScreenState::Stopped => (DotKind::Stopped, "stopped", ts.map(|t| SharedString::from(olo_kit::ago::short(Duration::from_millis(now_ms.saturating_sub(t)))))),
                        }
                    };
                    let idx = entries.len();
                    remote_index.insert((host.key.clone(), s.name.clone()), idx);
                    entries.push(Entry::Screen { label: shown.clone().into(), value: val, dot, agent_name: format!("{shown} · {} · {state_word}", host.label).into() });
                    targets.push(SidebarTarget::Screen(ScreenId::host(&host.key, &host.label, &s.name)));
                    timestamps.push(ts);
                }
            }
        }

        let mut cache = MultiOutlineCache { entries, entries_rc: None, targets: targets.into(), remote_index, local_index, timestamps, next_age_refresh: None };
        cache.recompute_next_refresh(now_ms, now_inst);
        cache
    }

    /// Multi-host outline sidebar (`This Computer`, then each Rex host in
    /// canonical `host_order`), backed by `self.outline_cache` so activity ticks
    /// touch only the affected row instead of re-sorting and rebuilding every
    /// host's screens (B.11 / #29).
    fn multi_sidebar(&mut self, side_w: f32, alone: bool, cx: &mut Cx<Self>) -> Element {
        let latest = self.pane.read(cx).latest_used(cx);
        let now_ms = history::now_ms();
        let now_inst = Instant::now();
        if let Some((id, t)) = &latest
            && let Some(cache) = self.outline_cache.as_mut()
        {
            let patch = match &id.location {
                ScreenLocation::Local => {
                    let cur = cache.local_index.get(&id.name).and_then(|&i| cache.timestamps[i]);
                    if cur.is_none_or(|c| c < *t) { cache.patch_local_last_used(&id.name, *t, now_ms, now_inst) } else { OutlinePatch::Unchanged }
                }
                ScreenLocation::Host { host_id, .. } => {
                    let cur = cache.remote_index.get(&(host_id.clone(), id.name.clone())).and_then(|&i| cache.timestamps[i]);
                    if cur.is_none_or(|c| c < *t) { cache.patch_remote_last_used(host_id, &id.name, *t, now_ms, now_inst) } else { OutlinePatch::Unchanged }
                }
            };
            if matches!(patch, OutlinePatch::NeedsRebuild) {
                self.outline_cache = None;
            }
        }
        if self.outline_cache.is_none() {
            self.outline_cache = Some(self.build_multi_outline(latest.as_ref(), now_ms, now_inst));
        }
        let (entries, targets) = {
            let cache = self.outline_cache.as_mut().expect("outline_cache populated");
            if let Some(soonest_ts) = cache.refresh_if_due(now_ms, now_inst) {
                // Schedule ONE `holds_until` wake from the SAME `now_ms` so the
                // wakeup cannot precede `at_inst` (Agy-review R2 #2).
                let _ = olo_kit::ago::age(Duration::from_millis(now_ms.saturating_sub(soonest_ts)), olo_kit::ago::short);
            }
            (cache.entries_rc(), cache.targets.clone())
        };

        let chosen = self.selected.as_ref().and_then(|sel| {
            targets.iter().position(|t| match (sel, t) {
                (Selection::Screen(a), SidebarTarget::Screen(b)) => a == b,
                (Selection::Host(a), SidebarTarget::Host(b)) => a == b,
                _ => false,
            })
        });
        let also: Vec<usize> =
            if self.also.is_empty() { Vec::new() } else { targets.iter().enumerate().filter(|(_, t)| matches!(t, SidebarTarget::Screen(id) if self.also.contains(id))).map(|(i, _)| i).collect() };
        let (t1, t2, t3, t4) = (targets.clone(), targets.clone(), targets.clone(), targets);
        let suppress = self.suppress_sidebar_reveal || self.pending_sidebar_scroll_y.is_some();
        if self.pending_sidebar_scroll_y.is_none() {
            self.suppress_sidebar_reveal = false;
        }
        let list = MultiSidebar::new("screens", entries, chosen)
            .bare(true)
            .edge(!alone)
            .also(also)
            .scroll(self.sidebar_scroll.clone())
            .suppress_reveal(suppress)
            .on_choose(cx.listener(move |v: &mut Viewer, i: &usize, cx| match t1.get(*i) {
                Some(SidebarTarget::Screen(id)) => {
                    v.pending_target = None;
                    v.also.clear();
                    v.anchor = Some(id.clone());
                    v.select(Some(Selection::Screen(id.clone())), cx);
                    if v.side_pane.alone() {
                        v.side_pane.show_rest();
                    }
                    cx.invalidate();
                }
                Some(SidebarTarget::Host(key)) => {
                    v.pending_target = None;
                    v.also.clear();
                    v.anchor = None;
                    v.select(Some(Selection::Host(key.clone())), cx);
                    if v.side_pane.alone() {
                        v.side_pane.show_rest();
                    }
                    cx.invalidate();
                }
                None => {}
            }))
            .on_toggle(cx.listener(move |v: &mut Viewer, i: &usize, cx| {
                if let Some(SidebarTarget::Host(key)) = t2.get(*i) {
                    v.toggle_host_collapsed(key, cx);
                }
            }))
            .on_pick(cx.listener(move |v: &mut Viewer, (i, mods): &(usize, Modifiers), cx| {
                if let Some(SidebarTarget::Screen(id)) = t3.get(*i) {
                    v.pick(id.clone(), mods.shift, cx);
                }
            }))
            .on_menu(cx.listener(move |v: &mut Viewer, (i, at): &(usize, Point<Pixels>), cx| match t4.get(*i) {
                Some(SidebarTarget::Screen(id)) => v.row_menu(id.clone(), *at, cx),
                Some(SidebarTarget::Host(host_key)) => v.host_menu(host_key, *at, cx),
                None => {}
            }))
            .on_select_all(cx.listener(|v: &mut Viewer, _: &(), cx| v.select_all(cx)))
            .build(cx);

        let any_remote_startable = self.rex_hosts.iter().any(RexHost::can_start_screen);
        self.wrap_sidebar(side_w, alone, list, any_remote_startable, cx)
    }

    fn wrap_sidebar(&mut self, side_w: f32, alone: bool, list: Element, multi_destination_new: bool, cx: &mut Cx<Self>) -> Element {
        let ground = canvas(move |g, b| {
            let p = theme::palette();
            style::fill_rect(g, b, sidebar::ground(&p));
            if !alone {
                style::vline_left_of(g, b.right(), b.top(), b.bottom(), sidebar::edge(&p));
            }
            let top = b.bottom() - px(FOOTER_H);
            let right = if alone { b.right() } else { b.right() - style::hairline(g) };
            style::hline(g, b.left(), right, top, style::shade(&p, p.panel_background, 1.18));
        })
        .absolute()
        .inset_0();
        let busy = self.busy;
        let label = if busy { "Starting…" } else { "New Screen…" };
        let new = Button::new(label).id("new-screen").enabled(!busy).on_click(cx.listener(move |v: &mut Viewer, _, cx| {
            if multi_destination_new {
                let mut m = Menu::new("").item("This Computer", "new-local");
                let startable: Vec<(String, String)> = v.rex_hosts.iter().filter(|h| h.can_start_screen()).map(|h| (h.key.clone(), h.label.clone())).collect();
                if !startable.is_empty() {
                    m = m.separator();
                    for (key, label) in startable {
                        m = m.item(label, format!("new-host:{key}"));
                    }
                }
                let at = cx.mouse_position();
                v.menu_at(m, at, cx);
            } else {
                v.new_screen(cx);
            }
        }));
        let col = div()
            .h_full()
            .flex()
            .flex_col()
            .relative()
            .child(ground)
            .child(div().flex_1().min_h_0().child(list))
            .child(div().h(px(FOOTER_H)).flex_shrink_0().flex().justify_center().pt(px(m::SPACING)).child(new));
        if alone { col.w_full().into_any_element() } else { col.w(px(side_w)).flex_shrink_0().into_any_element() }
    }

    /// Render a selected host WITH screens using the same header block as
    /// `ScreenPane::header` + a BeOS sunken well listing its visible screens
    /// (Design B.3 / Opus 3).
    #[allow(clippy::too_many_arguments)]
    fn host_screens_pane(
        &self,
        icon_name: &'static str,
        icon_faded: bool,
        title: String,
        subtitle: String,
        show_in_rex_cmd: Option<String>,
        new_screen_action: Option<(String, String, bool)>,
        items: Vec<(ScreenId, SharedString, Option<SharedString>, DotKind, SharedString)>,
        cx: &mut Cx<Self>,
    ) -> Element {
        let title_s = SharedString::from(title);
        let sub_s = SharedString::from(subtitle);
        let face = canvas(move |g, r| {
            olo_kit::review::own_grid(g, r);
            let p = theme::palette();
            let icon = style::snap(g, Bounds { origin: point(r.left(), r.center().y - px(16.0)), size: size(px(32.0), px(32.0)) });
            if icon_faded {
                icons::paint_faded(g, icon, icon_name, DISABLED_OPACITY);
            } else {
                icons::paint(g, icon, icon_name);
            }
            let x = icon.right() + px(m::SPACING);
            let w = (r.right() - x).max(px(0.0));
            if w < px(24.0) {
                return;
            }
            let t = style::label_fit_middle(g, x, r.top() + px(11.0), style::Align::Left, title_s.clone(), 15.0, style::ui_font(FontWeight::BOLD), style::c(p.panel_text), w);
            agent::record(g, agent::Node { role: "Heading", name: Some(title_s.clone()), bounds: t, enabled: true, ..Default::default() });
            let sub_ink = if icon_faded { tertiary_ink(&p, p.panel_background) } else { style::secondary_text(&p) };
            let st = style::label_fit(g, x, r.top() + px(29.0), style::Align::Left, sub_s.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), sub_ink, w);
            agent::record(g, agent::Node::label(sub_s.clone(), st));
        })
        .absolute()
        .inset_0();
        let face = div().flex_1().min_w_0().h_full().relative().child(face);

        let show_btn = show_in_rex_cmd.map(|cmd| {
            Button::new("Show in Rex").id("show-in-rex").min_width(96.0).on_click(cx.listener(move |v: &mut Viewer, _, cx| {
                v.run(&cmd, cx);
            }))
        });
        let new_btn = new_screen_action.map(|(label, cmd, enabled)| {
            Button::new(label).id("host-new-screen").min_width(96.0).enabled(enabled).on_click(cx.listener(move |v: &mut Viewer, _, cx| {
                v.run(&cmd, cx);
            }))
        });
        let mut buttons: Vec<Element> = Vec::new();
        if let Some(btn) = show_btn {
            buttons.push(btn.into_any_element());
        }
        if let Some(btn) = new_btn {
            buttons.push(btn.into_any_element());
        }
        let header = olo_screen_pane::header_bar(face.into_any_element(), buttons);

        let total_rows = items.len();
        let rows_list = div().w_full().flex().flex_col().children(items.into_iter().enumerate().map(|(idx, (id, label, age, dot, agent_name))| {
            let is_last = idx + 1 == total_rows;
            let painted_label = label.clone();
            let painted_age = age.clone();
            let painted_agent = agent_name.clone();
            let id_click = id.clone();
            let id_menu = id.clone();
            control_canvas(move |g, r, cstate| {
                let p = theme::palette();
                let bg = p.document_background;
                let tert = tertiary_ink(&p, bg);
                if cstate.hovered {
                    style::fill_rect(g, r, style::shade(&p, bg, 1.07));
                } else {
                    style::fill_rect(g, r, style::c(bg));
                }
                if !is_last {
                    style::hline_above(g, r.left(), r.right(), r.bottom(), style::shade(&p, bg, 1.08));
                }
                let text_cy = r.center().y - px(0.5);
                let dot_center = point(r.left() + px(16.0), text_cy - px(0.5));
                match dot {
                    DotKind::Running => paint_status_dot(g, dot_center, 8.0, style::c(p.success)),
                    DotKind::Stopped | DotKind::Archived => paint_status_dot(g, dot_center, 8.0, tert),
                    DotKind::Disconnected => paint_status_ring(g, dot_center, 8.0, tert),
                }
                let plain = style::ui_font(FontWeight::NORMAL);
                let text_left = r.left() + px(30.0);
                let right = r.right() - px(12.0);
                let name_right = if let Some(val) = &painted_age {
                    let val_ink = if matches!(dot, DotKind::Archived | DotKind::Disconnected) { tert } else { style::secondary_text(&p) };
                    let vb = style::label(g, right, text_cy, style::Align::Right, val.clone(), style::PLAIN_SIZE, plain.clone(), val_ink);
                    vb.left() - px(m::SPACING)
                } else {
                    right
                };
                let label_ink = match dot {
                    DotKind::Running => style::c(p.document_text),
                    DotKind::Stopped => style::secondary_text(&p),
                    DotKind::Archived | DotKind::Disconnected => tert,
                };
                let name_room = (name_right - text_left).max(px(0.0));
                style::label_fit_middle(g, text_left, text_cy, style::Align::Left, painted_label.clone(), style::PLAIN_SIZE, plain, label_ink, name_room);
                g.agent(agent::Node {
                    role: "ListItem",
                    name: Some(painted_agent.clone()),
                    value: painted_age.clone(),
                    bounds: r,
                    enabled: true,
                    clickable: true,
                    selected: Some(false),
                    ..Default::default()
                });
            })
            .w_full()
            .h(px(HOST_LIST_ROW_H))
            .flex_shrink_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |v: &mut Viewer, _, cx| {
                    v.pending_target = None;
                    v.also.clear();
                    v.anchor = Some(id_click.clone());
                    v.expand_host_of(&id_click);
                    v.select(Some(Selection::Screen(id_click.clone())), cx);
                    cx.invalidate();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |v: &mut Viewer, e: &olo_kit::MouseDownEvent, cx| {
                    v.row_menu(id_menu.clone(), e.position, cx);
                }),
            )
        }));

        let well_bg = canvas(|g, r| {
            let p = theme::palette();
            style::fill_rect(g, r, style::c(p.document_background));
        })
        .paint_key(0)
        .absolute()
        .inset_0();
        let well = div().flex_1().min_h_0().w_full().relative().child(well_bg).child(ScrollView::new(&self.host_list_scroll).border(true).auto_hide(true).child(rows_list).into_element());

        div().size_full().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).child(header).child(well).into_any_element()
    }

    /// Render the empty-state layout for the full-window empty app, an empty
    /// host (`staging`), or a not-set-up host (`docs-box`), supporting an inline
    /// monospace command without quote marks and an optional quiet secondary line.
    #[allow(clippy::too_many_arguments)]
    fn host_or_empty_pane(
        &self,
        icon_name: &'static str,
        icon_faded: bool,
        title: String,
        detail_before: String,
        detail_mono: Option<String>,
        detail_after: String,
        extra_note: Option<String>,
        action: Option<(String, String, bool)>,
        cx: &mut Cx<Self>,
    ) -> Element {
        let icon = canvas(move |g, b| {
            if icon_faded {
                icons::paint_faded(g, b, icon_name, DISABLED_OPACITY);
            } else {
                icons::paint(g, b, icon_name);
            }
        })
        .w(px(32.0))
        .h(px(32.0))
        .flex_shrink_0();

        let title_s = SharedString::from(title);
        let title_el = {
            let s = title_s.clone();
            canvas(move |g, b| {
                let p = theme::palette();
                let t = style::label_fit(g, b.center().x, b.center().y, style::Align::Center, s.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), style::c(p.panel_text), b.size.width);
                agent::record(g, agent::Node::label(s.clone(), t));
            })
            .w_full()
            .h(px(m::LABEL_ROW))
        };

        let detail_el = if detail_before.is_empty() && detail_mono.is_none() && detail_after.is_empty() {
            None
        } else {
            let before = detail_before;
            let mono = detail_mono;
            let after = detail_after;
            Some(
                canvas(move |g, b| {
                    let p = theme::palette();
                    let ink = style::secondary_text(&p);
                    let ui = style::ui_font(FontWeight::NORMAL);
                    if let Some(cmd) = &mono {
                        let mono_font = style::mono_font(FontWeight::NORMAL);
                        let full: SharedString = format!("{before}{cmd}{after}").into();
                        let mut runs = Vec::new();
                        if !before.is_empty() {
                            runs.push(TextRun { len: before.len(), font: ui.clone(), color: ink, background_color: None, underline: None, strikethrough: None, size: None });
                        }
                        runs.push(TextRun { len: cmd.len(), font: mono_font, color: style::c(p.panel_text), background_color: None, underline: None, strikethrough: None, size: None });
                        if !after.is_empty() {
                            runs.push(TextRun { len: after.len(), font: ui.clone(), color: ink, background_color: None, underline: None, strikethrough: None, size: None });
                        }
                        let t = style::label_runs_fit(g, b.center().x, b.center().y, style::Align::Center, full.clone(), style::PLAIN_SIZE, &runs, b.size.width);
                        agent::record(g, agent::Node::label(full, t));
                    } else {
                        let s = SharedString::from(before.clone());
                        let t = style::label_fit(g, b.center().x, b.center().y, style::Align::Center, s.clone(), style::PLAIN_SIZE, ui, ink, b.size.width);
                        agent::record(g, agent::Node::label(s, t));
                    }
                })
                .w_full()
                .h(px(m::LABEL_ROW)),
            )
        };

        let extra_el = extra_note.map(|note| {
            let s = SharedString::from(note);
            canvas(move |g, b| {
                let p = theme::palette();
                let ink = tertiary_ink(&p, p.panel_background);
                let t = style::label_fit(g, b.center().x, b.center().y, style::Align::Center, s.clone(), style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), ink, b.size.width);
                agent::record(g, agent::Node::label(s.clone(), t));
            })
            .w_full()
            .h(px(m::LABEL_ROW))
        });

        let btn = action.map(|(label, cmd, enabled)| {
            let id = if cmd.starts_with("open-host:") { "show-in-rex" } else { "empty-action" };
            Button::new(label).id(id).enabled(enabled).on_click(cx.listener(move |v: &mut Viewer, _, cx| {
                v.run(&cmd, cx);
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
                    .max_w(px(520.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(icon)
                    .child(div().h(px(16.0)))
                    .child(title_el)
                    .children(detail_el.map(|d| div().w_full().mt(px(4.0)).child(d)))
                    .children(extra_el.map(|d| div().w_full().mt(px(4.0)).child(d)))
                    .children(btn.map(|b| div().mt(px(20.0)).child(b))),
            )
            .into_any_element()
    }

    fn host_summary_pane(&self, host_key: &str, cx: &mut Cx<Self>) -> Element {
        let busy = self.busy;
        if host_key == LOCAL_HOST_KEY {
            let local_groups = self.local_groups();
            let running = local_groups.iter().find(|(g, _)| *g == "Running").map_or(0, |(_, rs)| rs.len());
            let stopped = local_groups.iter().find(|(g, _)| *g == "Stopped").map_or(0, |(_, rs)| rs.len());
            let archived = local_groups.iter().find(|(g, _)| *g == "Archived").map_or(0, |(_, rs)| rs.len());
            let new_local_label = if busy { "Starting…".to_owned() } else { "New Screen on This Computer…".to_owned() };
            if running + stopped + archived == 0 {
                return self.host_or_empty_pane(
                    "desktop",
                    false,
                    "No screens on this computer.".into(),
                    "An agent starts one with ".into(),
                    Some(start_command_for_host(None)),
                    ", or start one yourself.".into(),
                    None,
                    Some((new_local_label, "new-local".into(), !busy)),
                    cx,
                );
            }
            // Design B.3: selected This Computer WITH screens gets header + sunken list well.
            let mut items = Vec::new();
            for (group_name, rs) in &local_groups {
                for r in rs {
                    let (dot, state_word, val) = match *group_name {
                        "Running" => (DotKind::Running, "running", r.last_used.map(|t| SharedString::from(olo_kit::ago::ago_ms(t, olo_kit::ago::short)))),
                        "Archived" => (DotKind::Archived, "archived", Some(SharedString::from("Archived"))),
                        _ => (DotKind::Stopped, "stopped", r.last_used.map(|t| SharedString::from(olo_kit::ago::ago_ms(t, olo_kit::ago::short)))),
                    };
                    let shown = r.shown().to_owned();
                    items.push((ScreenId::local(&r.name), SharedString::from(shown.clone()), val, dot, SharedString::from(format!("{shown} · This Computer · {state_word}"))));
                }
            }
            let sub = format!("This computer · {}", format_counts(running, stopped, archived));
            return self.host_screens_pane("desktop", false, "This Computer".into(), sub, None, Some((new_local_label, "new-local".into(), !busy)), items, cx);
        }

        let Some(host) = self.rex_hosts.iter().find(|h| h.key == host_key) else {
            return self.host_or_empty_pane(
                "agent-screens",
                false,
                "No agent screens yet.".into(),
                "An agent starts one with ".into(),
                Some(start_command_for_host(None)),
                ", or start one yourself.".into(),
                None,
                Some((if busy { "Starting…".into() } else { "New Screen…".into() }, "new-local".into(), !busy)),
                cx,
            );
        };

        let visible = self.host_visible_screens(host);
        let open_cmd = format!("open-host:{}", host.key);
        let new_host_label = if busy { "Starting…".to_owned() } else { format!("New Screen on {}…", host.label) };
        let new_host_cmd = format!("new-host:{}", host.key);

        match host.state {
            RexHostConn::Disconnected if visible.is_empty() => self.host_or_empty_pane(
                "rex",
                true,
                format!("{} is disconnected.", host.label),
                "Reconnect to it in Rex to see its screens again.".into(),
                None,
                String::new(),
                None,
                Some(("Show in Rex".into(), open_cmd, true)),
                cx,
            ),
            RexHostConn::Disconnected => {
                let items: Vec<_> = visible
                    .iter()
                    .map(|s| {
                        let shown = s.display_name().to_owned();
                        (
                            ScreenId::host(&host.key, &host.label, &s.name),
                            SharedString::from(shown.clone()),
                            None,
                            DotKind::Disconnected,
                            SharedString::from(format!("{shown} · {} · disconnected", host.label)),
                        )
                    })
                    .collect();
                let n = items.len();
                let sub = format!("Disconnected · {n} screen{} last seen", if n == 1 { "" } else { "s" });
                self.host_screens_pane("rex", true, host.label.clone(), sub, Some(open_cmd), None, items, cx)
            }
            RexHostConn::Reconnecting | RexHostConn::Connecting => {
                let title = if host.state == RexHostConn::Reconnecting { format!("Reconnecting to {}…", host.label) } else { format!("Connecting to {}…", host.label) };
                self.host_or_empty_pane(
                    "rex",
                    true,
                    title,
                    "Screens on this host will appear once the connection is back.".into(),
                    None,
                    String::new(),
                    None,
                    Some(("Show in Rex".into(), open_cmd, true)),
                    cx,
                )
            }
            RexHostConn::Error => {
                let reason = host.error.clone().filter(|e| !e.trim().is_empty()).unwrap_or_else(|| "Couldn’t reach this host in Rex.".into());
                self.host_or_empty_pane("rex", true, format!("{} isn’t reachable.", host.label), reason, None, String::new(), None, Some(("Show in Rex".into(), open_cmd, true)), cx)
            }
            RexHostConn::Connected if !host.has_olo_screens() => {
                // Design B.10 (Opus 11): body leads directly to Show in Rex;
                // helper's own reason shown underneath only if it adds specific detail.
                let extra = not_set_up_extra_reason(host.not_set_up_reason.as_deref());
                self.host_or_empty_pane(
                    "rex",
                    true,
                    format!("Agent screens aren’t set up on {}.", host.label),
                    format!("Set them up from Rex and {}’s screens will show here.", host.label),
                    None,
                    String::new(),
                    extra,
                    Some(("Show in Rex".into(), open_cmd, true)),
                    cx,
                )
            }
            RexHostConn::Connected if visible.is_empty() => self.host_or_empty_pane(
                "rex",
                false,
                format!("No screens on {}.", host.label),
                "An agent there starts one with ".into(),
                Some(start_command_for_host(Some(host))),
                ", or start one yourself.".into(),
                None,
                Some((new_host_label, new_host_cmd, !busy)),
                cx,
            ),
            RexHostConn::Connected => {
                // Design B.3 (Opus 3): selected connected host WITH screens
                // renders the screen-style header block + BeOS sunken well list.
                let r = visible.iter().filter(|s| s.state == HostScreenState::Running).count();
                let st = visible.iter().filter(|s| s.state == HostScreenState::Stopped).count();
                let ar = visible.iter().filter(|s| s.state == HostScreenState::Archived).count();
                let sub = format!("Connected · {}", format_counts(r, st, ar));
                let items: Vec<_> = visible
                    .iter()
                    .map(|s| {
                        let shown = s.display_name().to_owned();
                        let (dot, state_word, val) = match s.state {
                            HostScreenState::Running => (DotKind::Running, "running", s.last_used.map(|t| SharedString::from(olo_kit::ago::ago_ms(t, olo_kit::ago::short)))),
                            HostScreenState::Archived => (DotKind::Archived, "archived", Some(SharedString::from("Archived"))),
                            HostScreenState::Stopped => (DotKind::Stopped, "stopped", s.last_used.map(|t| SharedString::from(olo_kit::ago::ago_ms(t, olo_kit::ago::short)))),
                        };
                        (ScreenId::host(&host.key, &host.label, &s.name), SharedString::from(shown.clone()), val, dot, SharedString::from(format!("{shown} · {} · {state_word}", host.label)))
                    })
                    .collect();
                self.host_screens_pane("rex", false, host.label.clone(), sub, Some(open_cmd), Some((new_host_label, new_host_cmd, !busy)), items, cx)
            }
        }
    }

    fn set_pane_visible(&mut self, visible: bool, cx: &mut Cx<Self>) {
        if self.pane_visible != visible {
            self.pane_visible = visible;
            self.pane.update(cx, |p, cx| p.set_visible(visible, cx));
        }
    }
}

fn format_counts(running: usize, stopped: usize, archived: usize) -> String {
    let mut parts = Vec::new();
    if running > 0 {
        parts.push(format!("{running} running"));
    }
    if stopped > 0 {
        parts.push(format!("{stopped} stopped"));
    }
    if archived > 0 {
        parts.push(format!("{archived} archived"));
    }
    if parts.is_empty() { "No screens".into() } else { parts.join(", ") }
}

impl View for Viewer {
    type Message = Box<dyn std::any::Any + Send>;

    fn message(&mut self, message: Self::Message, cx: &mut Cx<Self>) {
        self.told(message, cx);
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        self.run(command, cx);
        true
    }

    /// Closing it ends the app (and any control of a screen).
    fn close_requested(&mut self, cx: &mut Cx<Self>) -> bool {
        self.quit(cx);
        true
    }

    fn save_state(&mut self, cx: &mut Cx<Self>) -> Option<String> {
        Some(self.saved(cx))
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let full_w = f32::from(cx.viewport_size().width);
        let has_hosts = self.has_remote_hosts();

        // Full-window empty state ONLY when there are no local screens AND no Rex hosts.
        if self.rows.is_empty() && !has_hosts {
            self.set_pane_visible(false, cx);
            let bar = MenuBar::of(self, cx, |v, cx| v.menus(cx));
            let busy = self.busy;
            let empty = self.host_or_empty_pane(
                "agent-screens",
                false,
                "No agent screens yet.".into(),
                "An agent starts one with ".into(),
                Some(start_command_for_host(None)),
                ", or start one yourself.".into(),
                None,
                Some((if busy { "Starting…".into() } else { "New Screen…".into() }, "new-local".into(), !busy)),
                cx,
            );
            return div().size_full().flex().flex_col().bg(style::c(p.panel_background)).child(bar).child(div().flex_1().min_h_0().child(empty)).into_any_element();
        }

        // Responsive SidePane: gives way cleanly in narrow tiling layouts
        // (vertical half / quarter of 1280 × 800), with View ▸ Show Sidebar
        // bringing the list in front alone.
        let layout = self.side_pane.panes(full_w, PANE_MIN_BESIDE);
        let showing_screen_pane = layout != Panes::Pane && matches!(self.selected, Some(Selection::Screen(_)));
        // Finding 6: propagate visibility transitions once per change so hiding
        // `ScreenPane` (narrow Show Sidebar or Host heading view) releases Take
        // Control, pauses live capture, and suspends lazy history.
        self.set_pane_visible(showing_screen_pane, cx);

        let bar = MenuBar::of(self, cx, |v, cx| v.menus(cx));
        let root = div().size_full().flex().flex_col().bg(style::c(p.panel_background)).child(bar);

        if layout == Panes::Pane {
            let sidebar = if has_hosts { self.multi_sidebar(full_w, true, cx) } else { self.local_sidebar(full_w, true, cx) };
            return root.child(div().flex_1().min_h_0().child(sidebar)).into_any_element();
        }

        let (side_el, side_w) = match layout {
            Panes::Both(_) => {
                if has_hosts {
                    let w = (full_w * 0.225).round().clamp(MULTI_SIDEBAR_MIN_W, MULTI_SIDEBAR_W);
                    (Some(self.multi_sidebar(w, false, cx)), w)
                } else {
                    let w = (full_w * 0.2).round().clamp(SIDEBAR_MIN_W, SIDEBAR_W);
                    (Some(self.local_sidebar(w, false, cx)), w)
                }
            }
            Panes::Rest | Panes::Pane => (None, 0.0),
        };

        let right_pane: Element = match &self.selected {
            Some(Selection::Screen(_)) => {
                let pane_w = (full_w - side_w).max(200.0);
                self.pane.update(cx, |p, cx| p.set_width(pane_w, cx));
                div().flex_1().min_w_0().h_full().child(&self.pane).into_any_element()
            }
            Some(Selection::Host(host_key)) => {
                let key = host_key.clone();
                div().flex_1().min_w_0().h_full().child(self.host_summary_pane(&key, cx)).into_any_element()
            }
            None => div().flex_1().min_w_0().h_full().child(self.host_summary_pane(LOCAL_HOST_KEY, cx)).into_any_element(),
        };

        root.child(div().flex_1().min_h_0().flex().flex_row().children(side_el).child(right_pane)).into_any_element()
    }
}

/// Sequence gate for `Msg::Refreshed` so an older worker snapshot (`seq < applied`)
/// can never overwrite a newer list.
fn accept_list_seq(applied: &mut u64, seq: u64, advance: bool) -> bool {
    if seq < *applied {
        return false;
    }
    if advance {
        *applied = seq;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use olo_screen_pane::rex_sock::RexScreensState;

    fn sample_host(key: &str, label: &str, state: RexHostConn, default_on_host: bool) -> RexHost {
        RexHost {
            key: key.into(),
            label: label.into(),
            state,
            error: None,
            disconnected_at: None,
            installed: Some("1.0".into()),
            committed: None,
            default_on_host,
            not_set_up_reason: None,
            can_control: Some(true),
            control_disabled_reason: None,
            capabilities: Vec::new(),
            screens: Vec::new(),
        }
    }

    #[test]
    fn review2_f53_stale_refreshed_cannot_overwrite_newer_screens() {
        let mut applied = 0u64;
        assert!(accept_list_seq(&mut applied, 2, true));
        assert_eq!(applied, 2);
        assert!(!accept_list_seq(&mut applied, 1, true), "older Refreshed(1) must be discarded");
        assert!(!accept_list_seq(&mut applied, 1, false), "older Rows(1) must be discarded");
        assert!(accept_list_seq(&mut applied, 2, false));
        assert!(accept_list_seq(&mut applied, 3, true));
        assert_eq!(applied, 3);
        assert!(!accept_list_seq(&mut applied, 2, false));
    }

    #[test]
    fn review2_f52_pidfd_wakes_immediately_on_process_exit_and_detects_esrch() {
        let mut child = std::process::Command::new("/usr/bin/sleep").arg("60").spawn().expect("spawn sleep");
        let pid = child.id() as i32;
        let PidfdOpen::Opened(pfd) = open_pidfd(pid) else { panic!("expected pidfd_open to succeed") };
        let mut p = libc::pollfd { fd: pfd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        assert_eq!(unsafe { libc::poll(&mut p, 1, 0) }, 0);
        let _ = child.kill();
        let _ = child.wait();
        let rc = unsafe { libc::poll(&mut p, 1, 500) };
        assert!(rc > 0 && (p.revents & libc::POLLIN) != 0, "pidfd must wake immediately on exit: rc={rc} revents={}", p.revents);
        // Finding 21: opening pidfd for an already-reaped PID returns `Exited` (ESRCH).
        assert!(matches!(open_pidfd(pid), PidfdOpen::Exited));
    }

    #[test]
    fn collapsed_summary_uses_single_concise_slot() {
        assert_eq!(collapsed_summary(2, 4, 0), Some("2 running".into()));
        assert_eq!(collapsed_summary(2, 0, 0), Some("2 running".into()));
        assert_eq!(collapsed_summary(0, 3, 1), Some("3 stopped".into()));
        assert_eq!(collapsed_summary(0, 0, 2), Some("2 archived".into()));
        assert_eq!(collapsed_summary(0, 0, 0), Some("No screens".into()));
    }

    #[test]
    fn round3_f14_f16_worker_model_enforces_snapshot_first_rev_order_bounds_and_disconnect() {
        let mut m = WorkerRexModel::new();
        // Finding 16: delta before initial Snapshot demands resync.
        let early = RexFollowLine::HostRemoved { rev: 1, key: "h1".into() };
        assert!(matches!(m.apply_line(early), ApplyOutcome::NeedResync(_)));

        // Valid baseline snapshot at rev 12.
        let snap = RexScreensState {
            version: rex_sock::STATE_VERSION,
            rev: 12,
            pid: 1,
            socket: String::new(),
            hosts: vec![
                sample_host("h1", "dev-box", RexHostConn::Connected, true),
                sample_host("h2", "staging", RexHostConn::Connecting, true),
                sample_host("h3", "build-farm", RexHostConn::Reconnecting, true),
            ],
        };
        assert_eq!(m.apply_line(RexFollowLine::Snapshot { state: snap }), ApplyOutcome::Structural);
        assert_eq!(m.rex_epoch, 1);

        // Remove h1 at rev 14, then delayed update at rev 13 must NOT resurrect h1.
        assert_eq!(m.apply_line(RexFollowLine::HostRemoved { rev: 14, key: "h1".into() }), ApplyOutcome::Structural);
        assert_eq!(m.apply_line(RexFollowLine::HostUpdated { rev: 13, host: Box::new(sample_host("h1", "dev-box", RexHostConn::Connected, true)) }), ApplyOutcome::Ignored);
        assert!(m.hosts.iter().all(|h| h.key != "h1"), "stale rev 13 must never resurrect removed host");

        // Protocol Error surfaced once, deduplicated if repeated.
        assert_eq!(m.apply_line(RexFollowLine::Error { message: "bad\x1b[31m peer".into() }), ApplyOutcome::ProtocolError("bad[31m peer".into()));
        assert_eq!(m.apply_line(RexFollowLine::Error { message: "bad\x1b[31m peer".into() }), ApplyOutcome::Ignored);

        // Finding 14: exceeding MAX_HOSTS via HostUpdated demands resync.
        for i in 0..(MAX_HOSTS - m.hosts.len()) {
            let rev = 15 + i as u64;
            assert_eq!(m.apply_line(RexFollowLine::HostUpdated { rev, host: Box::new(sample_host(&format!("k{i}"), &format!("h{i}"), RexHostConn::Connected, true)) }), ApplyOutcome::Structural);
        }
        assert!(
            matches!(m.apply_line(RexFollowLine::HostUpdated { rev: 9999, host: Box::new(sample_host("overflow", "overflow", RexHostConn::Connected, true)) }), ApplyOutcome::NeedResync(_)),
            "adding beyond MAX_HOSTS must fail closed"
        );

        // Finding 16: Rex disconnect marks Connected, Connecting, AND Reconnecting offline.
        m.mark_disconnected(1_700_000_000_000);
        for k in ["h2", "h3"] {
            let h = m.hosts.iter().find(|h| h.key == k).unwrap();
            assert_eq!(h.state, RexHostConn::Disconnected, "{k} must be marked Disconnected on Rex exit");
        }
    }

    #[test]
    fn round3_f27_f32_b9_b10_host_resolution_start_commands_and_not_set_up_reason() {
        let hosts =
            vec![sample_host("k1", "Build", RexHostConn::Connected, true), sample_host("k2", "build", RexHostConn::Connected, false), sample_host("k3", "lab:west", RexHostConn::Connected, false)];
        // Finding 32: exact label wins over earlier case-insensitive match; ambiguity fails.
        assert_eq!(find_host_by_spec(&hosts, "build").unwrap().key, "k2");
        assert_eq!(find_host_by_spec(&hosts, "k1").unwrap().key, "k1");
        assert!(find_host_by_spec(&hosts, "BUILD").is_err());
        assert_eq!(find_host_by_spec(&hosts, "lab:west").unwrap().key, "k3");

        // Design B.9 & Final pass B.8 (#27): `--on` accepts ONLY literal `host` or `desktop`, never hostnames.
        assert_eq!(start_command_for_host(None), "olo-screen start checkout-qa");
        assert_eq!(start_command_for_host(Some(&hosts[0])), "olo screen start checkout-qa");
        assert_eq!(start_command_for_host(Some(&hosts[1])), "olo screen --on host start checkout-qa");
        assert_eq!(start_command_for_host(Some(&hosts[2])), "olo screen --on host start checkout-qa");

        // Design B.10: generic boilerplate suppressed; specific host diagnostic preserved.
        assert_eq!(not_set_up_extra_reason(Some("Olo’s screen binaries aren’t installed on this host yet.")), None);
        assert_eq!(not_set_up_extra_reason(Some("Not set up")), None);
        assert_eq!(not_set_up_extra_reason(Some("Mesa software EGL driver (llvmpipe) is missing.")), Some("Mesa software EGL driver (llvmpipe) is missing.".into()));
    }

    #[test]
    fn round3_f19_saved_roundtrips_narrow_front_and_sidebar_scroll() {
        // B.10 (#19): kit scroll down 142.5 pt is negative (-142.5); encode/decode negate cleanly at boundaries!
        let encoded = encode_sidebar_scroll_y(-142.5);
        assert!((encoded - 142.5).abs() < 0.01);
        assert!((decode_sidebar_scroll_y(encoded) - (-142.5)).abs() < 0.01);
        assert_eq!(encode_sidebar_scroll_y(0.0), 0.0);
        assert_eq!(encode_sidebar_scroll_y(12.0), 0.0);
        let s = Saved { narrow_front_pane: true, sidebar_scroll_y: encoded, collapsed_hosts: vec!["dev-box".into()], ..Default::default() };
        let j = serde_json::to_string(&s).unwrap();
        let back: Saved = serde_json::from_str(&j).unwrap();
        assert!(back.narrow_front_pane);
        assert!((back.sidebar_scroll_y - 142.5).abs() < 0.01);
        assert!((decode_sidebar_scroll_y(back.sidebar_scroll_y) - (-142.5)).abs() < 0.01);
        assert_eq!(back.collapsed_hosts, vec!["dev-box"]);
    }

    #[test]
    fn round3_f13_f14_f29_worker_host_epochs_and_last_used_coalescing() {
        let mut m = WorkerRexModel::new();
        let mut h1 = sample_host("h1", "dev-box", RexHostConn::Connected, true);
        h1.screens.push(RexScreen {
            name: "qa".into(),
            title: None,
            state: rex_sock::HostScreenState::Running,
            width: 1280,
            height: 800,
            scale: 1.0,
            sound: false,
            pid: Some(10),
            started: Some(1000),
            last_used: Some(10_000),
            actions: Some(1),
            controlled: false,
            controlled_by: None,
            owner: None,
            started_by: None,
        });
        let snap = RexScreensState { version: rex_sock::STATE_VERSION, rev: 1, pid: 1, socket: String::new(), hosts: vec![h1.clone()] };
        assert_eq!(m.apply_line(RexFollowLine::Snapshot { state: snap.clone() }), ApplyOutcome::Structural);
        assert_eq!(m.rex_epoch, 1);
        let rev_after_initial = m.host_conn_revs["h1"];

        // B.6 (#10/#13/#28): an in-subscription lag-resync Snapshot with unchanged host
        // connectivity & capabilities MUST NOT bump `rex_epoch` or `host_conn_revs`
        // (prevents needlessly dropping Take Control!).
        assert_eq!(m.apply_line(RexFollowLine::Snapshot { state: snap }), ApplyOutcome::Structural);
        assert_eq!(m.rex_epoch, 1, "in-subscription lag-resync Snapshot must not bump rex_epoch");
        assert_eq!(m.host_conn_revs["h1"], rev_after_initial, "unchanged lag-resync must not bump host_conn_revs");

        // B.6 (#10/#13/#28): an offline -> online bounce coalesced BEFORE UI drains
        // still advances `host_conn_revs["h1"]` monotonically so Live reopens!
        let mut h1_off = h1.clone();
        h1_off.state = RexHostConn::Disconnected;
        assert_eq!(m.apply_line(RexFollowLine::HostUpdated { rev: 2, host: Box::new(h1_off) }), ApplyOutcome::Structural);
        assert_eq!(m.apply_line(RexFollowLine::HostUpdated { rev: 3, host: Box::new(h1.clone()) }), ApplyOutcome::Structural);
        assert!(m.host_conn_revs["h1"] > rev_after_initial, "coalesced offline->online bounce must advance host_conn_revs");

        // Finding 29 & 30: ScreenLastUsed takes the fast non-structural path without
        // bumping structural state, and applies count-only updates even when `last_used`
        // did not advance.
        assert_eq!(
            m.apply_line(RexFollowLine::ScreenLastUsed { host: "h1".into(), name: "qa".into(), last_used: 20_000, actions: Some(2) }),
            ApplyOutcome::LastUsed { host: "h1".into(), name: "qa".into(), last_used: 20_000, actions: Some(2) }
        );
        assert_eq!(m.hosts[0].screens[0].last_used, Some(20_000));
        assert_eq!(m.hosts[0].screens[0].actions, Some(2));
        // Count-only update with unchanged timestamp still applies (B.9 / hostside Finding 22).
        assert_eq!(
            m.apply_line(RexFollowLine::ScreenLastUsed { host: "h1".into(), name: "qa".into(), last_used: 20_000, actions: Some(3) }),
            ApplyOutcome::LastUsed { host: "h1".into(), name: "qa".into(), last_used: 20_000, actions: Some(3) }
        );
        assert_eq!(m.hosts[0].screens[0].actions, Some(3));
        // Equal timestamp and unchanged count is ignored.
        assert_eq!(m.apply_line(RexFollowLine::ScreenLastUsed { host: "h1".into(), name: "qa".into(), last_used: 20_000, actions: Some(3) }), ApplyOutcome::Ignored);
        assert_eq!(m.hosts[0].screens[0].last_used, Some(20_000));

        // Finding 13 & Part C Rex-restart recovery: on a NEW subscription, Rex's
        // initial baseline Snapshot bumps `rex_epoch` AND preserves `h1`'s last-known
        // screens across transient `Connecting` until `HostUpdated` arrives with `Connected`.
        m.begin_subscription();
        let mut h1_connecting = sample_host("h1", "dev-box", RexHostConn::Connecting, true);
        h1_connecting.screens.clear();
        let snap2 = RexScreensState { version: rex_sock::STATE_VERSION, rev: 1, pid: 2, socket: String::new(), hosts: vec![h1_connecting] };
        assert_eq!(m.apply_line(RexFollowLine::Snapshot { state: snap2 }), ApplyOutcome::Structural);
        assert_eq!(m.rex_epoch, 2);
        assert_eq!(m.hosts[0].screens.len(), 1, "transient Connecting snapshot on Rex restart must preserve last-known screens");
        assert_eq!(m.hosts[0].screens[0].name, "qa");
        // Once `h1` finishes connecting with an authoritative list, it replaces cleanly.
        assert_eq!(m.apply_line(RexFollowLine::HostUpdated { rev: 2, host: Box::new(h1) }), ApplyOutcome::Structural);
        assert_eq!(m.hosts[0].state, RexHostConn::Connected);
        assert_eq!(m.hosts[0].screens[0].name, "qa");
    }

    #[test]
    fn final_pass_b7_b8_b11_incremental_scan_unused_remote_name_pidfd_cleanup_and_watch_filters() {
        // B.7 (#11): PollLineReader advances `scanned_up_to` so large multi-chunk
        // snapshots never re-scan their prefix.
        let mut r = PollLineReader::new();
        r.buf.extend_from_slice(&vec![b'a'; 8192]);
        assert!(r.take_buffered_line().is_none());
        assert_eq!(r.scanned_up_to, 8192, "scanned_up_to must advance past scanned prefix");
        r.buf.extend_from_slice(&vec![b'b'; 8192]);
        assert!(r.take_buffered_line().is_none());
        assert_eq!(r.scanned_up_to, 16_384);
        r.buf.extend_from_slice(b"tail\nnext");
        let line = r.take_buffered_line().unwrap().unwrap();
        assert_eq!(line.len(), 16_384 + 4);
        assert_eq!(r.scanned_up_to, 0);
        assert_eq!(r.buf, b"next");

        // B.8 (#27): `new_remote_screen_name` avoids ALL names on that host
        // (running, stopped, AND archived!).
        let mk_scr = |name: &str, state: HostScreenState| RexScreen {
            name: name.into(),
            title: None,
            state,
            width: 1280,
            height: 800,
            scale: 1.0,
            sound: false,
            pid: None,
            started: None,
            last_used: None,
            actions: None,
            controlled: false,
            controlled_by: None,
            owner: None,
            started_by: None,
        };
        let screens = vec![mk_scr("agent", HostScreenState::Archived), mk_scr("agent-2", HostScreenState::Stopped), mk_scr("agent-3", HostScreenState::Running)];
        assert_eq!(new_remote_screen_name(&screens), "agent-4");

        // B.11 (#21): `drop_signalled_pidfds` removes readable pidfds before reconciling,
        // and `proc_start_ticks` returns a valid nonzero start time for our own PID.
        let my_pid = std::process::id() as i32;
        let my_ticks = proc_start_ticks(my_pid).expect("proc_start_ticks for self");
        assert!(my_ticks > 0);
        let mut child = std::process::Command::new("/usr/bin/sleep").arg("30").spawn().expect("spawn sleep");
        let cpid = child.id() as i32;
        let cticks = proc_start_ticks(cpid).expect("child start ticks");
        let PidfdOpen::Opened(pfd) = open_pidfd(cpid) else { panic!("open_pidfd child") };
        let raw_fd = pfd.as_raw_fd();
        let mut map = HashMap::new();
        map.insert((cpid, cticks), pfd);
        let fds = [libc::pollfd { fd: -1, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: raw_fd, events: libc::POLLIN, revents: libc::POLLIN }];
        drop_signalled_pidfds(&mut map, &[(cpid, cticks)], &fds);
        assert!(map.is_empty(), "readable pidfd must be dropped immediately before reconciling");
        let _ = child.kill();
        let _ = child.wait();

        // B.11 (#23): watcher path filters ignore `frames-*.olof`, `recorder.lock`,
        // `pictures/`, `logs/`, and watched-dir self-entries paired with noisy children.
        let rt = PathBuf::from("/var/tmp/rt");
        let st = PathBuf::from("/var/tmp/state/screens");
        let qa = st.join("qa");
        assert!(!runtime_or_state_event_relevant(&[rt.clone(), rt.join("qa.control.lock")], &rt, &st));
        assert!(!runtime_or_state_event_relevant(&[rt.clone(), rt.join("views/record-qa.rgba")], &rt, &st));
        assert!(runtime_or_state_event_relevant(&[rt.clone(), rt.join("qa.json")], &rt, &st));
        assert!(runtime_or_state_event_relevant(std::slice::from_ref(&rt), &rt, &st), "bare watched dir resync stays relevant");
        assert!(!runtime_or_state_event_relevant(&[st.clone(), qa.join("frames-0001.olof")], &rt, &st));
        assert!(!runtime_or_state_event_relevant(&[st.clone(), qa.join("recorder.lock")], &rt, &st));
        assert!(!runtime_or_state_event_relevant(&[st.clone(), qa.join("pictures/shot.png")], &rt, &st));
        assert!(!runtime_or_state_event_relevant(&[st.clone(), qa.join("logs/out.log")], &rt, &st));
        assert!(runtime_or_state_event_relevant(&[st.clone(), qa.join("actions.jsonl")], &rt, &st));
        assert!(runtime_or_state_event_relevant(&[st.clone(), qa.join("title")], &rt, &st));
        assert!(!screen_history_event_relevant(&[qa.clone(), qa.join("frames-0001.olof")], std::slice::from_ref(&qa)));
        assert!(!screen_history_event_relevant(&[qa.clone(), qa.join("recorder.lock")], std::slice::from_ref(&qa)));
        assert!(screen_history_event_relevant(&[qa.clone(), qa.join("actions.jsonl")], std::slice::from_ref(&qa)));
        assert!(screen_history_event_relevant(std::slice::from_ref(&qa), std::slice::from_ref(&qa)));

        // B.11 (#29): `RexMailbox::publish_last_used` stores ONLY deltas (hosts = None)
        // without cloning host lists, and coalesces repeated ticks per (host, name).
        let mb = RexMailbox::new();
        {
            let mut g = mb.inner.lock().unwrap();
            let mut deltas = HashMap::new();
            deltas.insert(("h1".into(), "qa".into()), (1000, Some(1)));
            *g = Some(RexMailboxSnap { hosts: None, host_conn_revs: None, rex_epoch: 1, connected: true, last_used_deltas: deltas, protocol_error: None });
        }
        let snap = mb.take().unwrap();
        assert!(snap.hosts.is_none(), "fast last_used tick path must not clone hosts");
        assert_eq!(snap.last_used_deltas.get(&("h1".into(), "qa".into())), Some(&(1000, Some(1))));
    }

    #[test]
    fn final_pass_b4_b7_follow_loop_private_dir_retry_reset_eof_and_unlink() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;
        use std::sync::mpsc;

        let dir = PathBuf::from(format!("/var/tmp/olo-follow-b4-b7-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // B.4 (#8): `run_rex_follow_loop` refuses a non-private (0770) parent dir
        // without creating or chmodding it!
        let shared = dir.join("shared-0770");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o770)).unwrap();
        let sock_shared = shared.join("rex.sock");
        let _srv_shared = UnixListener::bind(&sock_shared).unwrap();
        let wake0 = Arc::new(open_eventfd().unwrap());
        let stop0 = Arc::new(AtomicBool::new(false));
        let (tx0, rx0) = mpsc::channel::<bool>();
        let stop0_c = stop0.clone();
        let wake0_c = wake0.clone();
        let h0 = std::thread::spawn(move || {
            run_rex_follow_loop(sock_shared, wake0_c, stop0_c, |ev| {
                if let FollowPublish::Structural { connected, .. } = ev {
                    let _ = tx0.send(connected);
                }
            });
        });
        assert!(!rx0.recv_timeout(Duration::from_secs(3)).unwrap(), "0770 parent must be refused");
        stop0.store(true, Ordering::SeqCst);
        signal_eventfd(&wake0);
        let _ = h0.join();
        let mode_after = std::fs::metadata(&shared).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode_after, 0o770, "viewer must NEVER chmod an existing 0770 directory");

        // B.7 (#11):
        //  1) Conn 1: EOF before first snapshot -> retries immediately (Conn 2) while socket exists!
        //  2) Conn 2: healthy Snapshot then EOF -> resets retry allowance and retries immediately (Conn 3)!
        //  3) Conn 3: healthy Snapshot then EOF -> proves allowance reset on Conn 2 (retries immediately as Conn 4)!
        //  4) Conn 4: healthy Snapshot, server keeps stream open while socket file is UNLINKED & wake_fd signalled
        //     -> worker immediately drops the stale binding and publishes disconnected!
        let priv_dir = dir.join("private-0700");
        std::fs::create_dir(&priv_dir).unwrap();
        std::fs::set_permissions(&priv_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let sock_path = priv_dir.join("rex.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();
        std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let (srv_tx, srv_rx) = mpsc::channel::<u32>();
        let (hold_tx, hold_rx) = mpsc::channel::<()>();
        let srv_thread = std::thread::spawn(move || {
            let snap_json = serde_json::to_string(&RexFollowLine::Snapshot { state: RexScreensState { version: rex_sock::STATE_VERSION, rev: 1, pid: 1, socket: String::new(), hosts: Vec::new() } })
                .unwrap()
                + "\n";
            for conn_num in 1..=4u32 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut rdr = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                let _ = rdr.read_line(&mut line);
                let _ = srv_tx.send(conn_num);
                match conn_num {
                    1 => {
                        // Drop immediately before baseline snapshot.
                        drop(stream);
                    }
                    2 | 3 => {
                        // Healthy baseline snapshot, then drop.
                        stream.write_all(snap_json.as_bytes()).unwrap();
                        stream.flush().unwrap();
                        std::thread::sleep(Duration::from_millis(40));
                        drop(stream);
                    }
                    4 => {
                        // Healthy baseline snapshot, then hold connection open even after unlink!
                        stream.write_all(snap_json.as_bytes()).unwrap();
                        stream.flush().unwrap();
                        let _ = hold_rx.recv_timeout(Duration::from_secs(5));
                        drop(stream);
                    }
                    _ => unreachable!(),
                }
            }
        });

        let wake = Arc::new(open_eventfd().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let (ev_tx, ev_rx) = mpsc::channel::<(u64, bool)>();
        let sock_for_worker = sock_path.clone();
        let wake_c = wake.clone();
        let stop_c = stop.clone();
        let worker = std::thread::spawn(move || {
            run_rex_follow_loop(sock_for_worker, wake_c, stop_c, |ev| {
                if let FollowPublish::Structural { rex_epoch, connected, .. } = ev {
                    let _ = ev_tx.send((rex_epoch, connected));
                }
            });
        });

        // All 4 connections must arrive WITHOUT needing an external discovery wake!
        for want in 1..=4u32 {
            assert_eq!(srv_rx.recv_timeout(Duration::from_secs(4)).expect("follow must retry without parking"), want);
        }
        // Wait until Conn 4's healthy snapshot (`rex_epoch == 3`, `connected == true`) is published.
        let mut saw_epoch3 = false;
        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline {
            if let Ok((ep, conn)) = ev_rx.recv_timeout(Duration::from_millis(200))
                && ep == 3
                && conn
            {
                saw_epoch3 = true;
                break;
            }
        }
        assert!(saw_epoch3, "Conn 4 must establish healthy snapshot at rex_epoch 3");

        // Now unlink the socket while Conn 4 is still held open by the server and wake the worker!
        std::fs::remove_file(&sock_path).unwrap();
        signal_eventfd(&wake);
        let (_, conn_after_unlink) = ev_rx.recv_timeout(Duration::from_secs(3)).expect("unlinking socket must drop open Follow binding");
        assert!(!conn_after_unlink, "unlinking watched socket must publish disconnected");

        let _ = hold_tx.send(());
        stop.store(true, Ordering::SeqCst);
        signal_eventfd(&wake);
        let _ = worker.join();
        let _ = srv_thread.join();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn final_pass_b10_b11_pending_watch_and_200_screen_outline_cache() {
        // B.10 (#18): `Saved` preserves unresolved `pending_watch` across reload.
        let s = Saved { pending_watch: Some("far:qa".into()), ..Default::default() };
        let j = serde_json::to_string(&s).unwrap();
        let back: Saved = serde_json::from_str(&j).unwrap();
        assert_eq!(back.pending_watch.as_deref(), Some("far:qa"));

        // B.11 (#29): 200 screens across 5 hosts -> `MultiOutlineCache::patch_remote_last_used`
        // patches ONLY the single touched screen in place, returns `Unchanged` on repeated
        // `"now"` ticks (avoiding UI redraws), and requests rebuild ONLY when sort order flips.
        let now_ms = history::now_ms();
        let now_inst = Instant::now();
        let mut entries = Vec::new();
        let mut targets = Vec::new();
        let mut timestamps = Vec::new();
        let mut remote_index = HashMap::new();
        for h in 0..5usize {
            let hk = format!("host-{h}");
            entries.push(Entry::Heading {
                key: hk.clone().into(),
                label: hk.clone().into(),
                icon: "rex".into(),
                disconnected: false,
                open: true,
                has_children: true,
                status: None,
                status_error: false,
                agent_name: hk.clone().into(),
            });
            targets.push(SidebarTarget::Host(hk.clone()));
            timestamps.push(None);
            for i in 0..40usize {
                let sname = format!("scr-{i:02}");
                // Sorted descending by `last_used`: scr-00 is 10 min ago, scr-01 is 11 min ago, ...
                let ts = now_ms - ((10 + i as u64) * 60_000);
                let idx = entries.len();
                remote_index.insert((hk.clone(), sname.clone()), idx);
                entries.push(Entry::Screen {
                    label: sname.clone().into(),
                    value: Some(SharedString::from(format!("{} min", 10 + i))),
                    dot: DotKind::Running,
                    agent_name: format!("{sname} · {hk} · running").into(),
                });
                targets.push(SidebarTarget::Screen(ScreenId::host(&hk, &hk, &sname)));
                timestamps.push(Some(ts));
            }
        }
        assert_eq!(remote_index.len(), 200);
        let mut cache = MultiOutlineCache { entries, entries_rc: None, targets: targets.into(), remote_index, local_index: HashMap::new(), timestamps, next_age_refresh: None };
        cache.recompute_next_refresh(now_ms, now_inst);

        // Tick 1 on top-sorted `host-2/scr-00` ("10 min" -> "now"): updates ONLY that 1 entry in place!
        let rc_before = cache.entries_rc();
        let p1 = cache.patch_remote_last_used("host-2", "scr-00", now_ms - 1_000, now_ms, now_inst);
        assert_eq!(p1, OutlinePatch::UpdatedInPlace);
        let idx = cache.remote_index[&("host-2".into(), "scr-00".into())];
        assert!(matches!(&cache.entries[idx], Entry::Screen { value: Some(v), .. } if v.as_ref() == "now"));
        // All other 204 entries remain identical.
        for (i, (a, b)) in rc_before.iter().zip(cache.entries.iter()).enumerate() {
            if i != idx {
                assert_eq!(a, b);
            }
        }

        // Tick 2 on `host-2/scr-00` 1 s later ("now" -> "now"): Unchanged (0 redraws)!
        let p2 = cache.patch_remote_last_used("host-2", "scr-00", now_ms, now_ms + 1_000, now_inst + Duration::from_secs(1));
        assert_eq!(p2, OutlinePatch::Unchanged);

        // Tick on a LOWER screen (`host-2/scr-05`) that leaps ahead of `scr-04`: detects NeedsRebuild so sort order stays exact!
        let p3 = cache.patch_remote_last_used("host-2", "scr-05", now_ms + 2_000, now_ms + 2_000, now_inst + Duration::from_secs(2));
        assert_eq!(p3, OutlinePatch::NeedsRebuild);
    }

    #[test]
    fn agy_r1_f1_f8_f11_f12_airplay_scroll_readiness_caps_stop_skip_and_running_archive() {
        // Finding #1: AirPlay menu dispatch targets ONLY a single running local screen,
        // and recognizes `olo_protocol::airplay::menu_command` items while rejecting
        // stopped, multi-selected, or remote screens.
        let local_run = ScreenId::local("qa");
        let local_stop = ScreenId::local("old");
        let remote_run = ScreenId::host("h1", "dev-box", "qa");
        let is_running = |id: &ScreenId| id == &local_run || id == &remote_run;
        assert_eq!(airplay_shown_for(std::slice::from_ref(&local_run), is_running), Some(olo_protocol::airplay::Shown::Screen { name: "qa".into() }));
        assert_eq!(airplay_shown_for(std::slice::from_ref(&local_stop), is_running), None, "stopped local screen must not dispatch AirPlay");
        assert_eq!(airplay_shown_for(std::slice::from_ref(&remote_run), is_running), None, "remote screen must not dispatch local AirPlay");
        assert_eq!(airplay_shown_for(&[local_run.clone(), local_stop.clone()], is_running), None, "multi-selection must not dispatch AirPlay");
        assert!(!olo_kit::airplay::show_from_menu("tab-live", &olo_protocol::airplay::Shown::Screen { name: "qa".into() }), "non-AirPlay commands return false");

        // Finding #8: restored sidebar scroll waits for BOTH local_ready AND rex_ready
        // unconditionally (never consuming early when local arrives before Rex hosts).
        assert!(!pending_sidebar_scroll_ready(false, false));
        assert!(!pending_sidebar_scroll_ready(true, false));
        assert!(!pending_sidebar_scroll_ready(false, true));
        assert!(pending_sidebar_scroll_ready(true, true));

        // Finding #11: `host_caps_from_rex` propagates `clear-expected` and `delete-expected`.
        let mut h = sample_host("h1", "dev-box", RexHostConn::Connected, true);
        h.capabilities = vec!["stop-expected".into(), "clear-expected".into(), "delete-expected".into()];
        let hc = host_caps_from_rex(&h).expect("host caps parsed");
        assert!(hc.stop_expected && hc.clear_expected && hc.delete_expected);
        let mut h_old = sample_host("h2", "old-box", RexHostConn::Connected, true);
        h_old.capabilities = vec!["stop-expected".into()];
        let hc_old = host_caps_from_rex(&h_old).expect("old host caps parsed");
        assert!(hc_old.stop_expected && !hc_old.clear_expected && !hc_old.delete_expected);

        // Finding #12a: mixed multi-selection with missing `started` reports skipped screens
        // explicitly instead of silently omitting them.
        assert_eq!(format_stop_missing_gen_note(&[]), "");
        let one_note = format_stop_missing_gen_note(&["“old” on legacy-box".to_owned()]);
        assert!(one_note.contains("Skipping “old” on legacy-box") && one_note.contains("its session start time"), "{one_note}");
        let two_note = format_stop_missing_gen_note(&["“a” on h1".to_owned(), "“b” on h2".to_owned()]);
        assert!(two_note.contains("“a” on h1, “b” on h2") && two_note.contains("their session start times"), "{two_note}");
        let one_alert = format_stop_missing_gen_alert(&["“old” on legacy-box".to_owned()]);
        assert!(one_alert.contains("Can’t safely stop “old” on legacy-box"));
        let two_alert = format_stop_missing_gen_alert(&["“a”".to_owned(), "“b”".to_owned()]);
        assert!(two_alert.contains("their session start times"));

        // Finding #12b: a running local screen with a stray `archived` file on disk is NOT archived.
        let running = vec![Screen {
            name: "qa".into(),
            wayland_display: "wayland-qa".into(),
            olo_socket: PathBuf::from("/var/tmp/qa.sock"),
            pid: 99,
            width: 1280,
            height: 800,
            scale: 1.0,
            sound: false,
            started: 1_000,
            bus: None,
            a11y: None,
        }];
        let rows = vec![Row { name: "qa".into(), title: None, archived: true, last_used: None }, Row { name: "old".into(), title: None, archived: true, last_used: None }];
        assert!(!local_screen_is_archived("qa", &running, &rows), "running local screen must never enable Unarchive");
        assert!(local_screen_is_archived("old", &running, &rows), "stopped archived local screen remains archived");
    }

    #[test]
    fn agy_r2_f2_outline_cache_refreshes_on_wall_clock_advance_or_suspend_resume() {
        let base_inst = Instant::now();
        let ts = 1_000u64;
        let now_ms = 59_000u64; // age = 58 s -> "now", changes at +2_000 ms (61_000 ms).
        let mut cache = MultiOutlineCache {
            entries: vec![Entry::Screen { label: "qa".into(), value: Some("now".into()), dot: DotKind::Running, agent_name: "qa".into() }],
            entries_rc: None,
            targets: vec![SidebarTarget::Screen(ScreenId::local("qa"))].into(),
            remote_index: HashMap::new(),
            local_index: HashMap::from([("qa".into(), 0)]),
            timestamps: vec![Some(ts)],
            next_age_refresh: None,
        };
        cache.recompute_next_refresh(now_ms, base_inst);
        let (at_inst, at_ms, soonest) = cache.next_age_refresh.expect("next_age_refresh set");
        assert_eq!(soonest, ts);
        assert_eq!(at_ms, 61_000);
        assert_eq!(at_inst, base_inst + Duration::from_secs(2));

        // 1. 1 ms monotonic wake slightly before `at_inst`, while wall clock already hit `at_ms`:
        //    MUST refresh `"now"` -> `"1 min"` instead of freezing for an extra minute!
        let early_inst = base_inst + Duration::from_millis(1_999);
        assert!(early_inst < at_inst);
        let next_ts = cache.refresh_if_due(61_000, early_inst);
        assert_eq!(next_ts, Some(ts));
        assert!(matches!(&cache.entries[0], Entry::Screen { value: Some(v), .. } if v.as_ref() == "1 min"), "got: {:?}", cache.entries[0]);
        assert_eq!(cache.next_age_refresh.unwrap().1, 121_000);

        // 2. Suspend-to-RAM: monotonic clock stayed at `early_inst` while wall clock jumped +2 h.
        let _ = cache.refresh_if_due(61_000 + 2 * 3_600_000, early_inst);
        assert!(matches!(&cache.entries[0], Entry::Screen { value: Some(v), .. } if v.as_ref() == "2 h"), "got: {:?}", cache.entries[0]);
    }
}
