//! [`LocalScreenSource`]: [`ScreenSource`] backed by local headless Olo
//! screens (`olo-session`).
//!
//! Live shows the screen's own frames as it draws them, never copied:
//! its app_server captures each into one of a few dmabufs on the GPU
//! (`olo_capture::live`), and the person's compositor shows that over
//! the pane (`Painter::place_external`), at the screen's frame rate
//! with nothing read back. Where the two can't share dmabufs it takes a
//! picture instead when the screen has drawn something new (its
//! app_server says so, `EventKind::Painted`), at most eight a second.
//! Either way a screen at rest costs nothing to watch. Frames come
//! through a worker, input goes out through another (moves run
//! together, over one connection) and the history is followed by a
//! third, so the pane never waits on a screen or a disk.

use std::{
    os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{RecvTimeoutError, Sender, channel},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use olo_capture::live::Live;
use olo_kit::{ExternalBuffer, ExternalPlane};
use olo_protocol::{Client, Event, EventKind, ImageFormat, Reply, Request, SyntheticEvent};
use olo_screen_codec::{Frame, FrameIndex, OwnedPlayer, olof::SegmentCache};
use olo_sync::{Condvar, Mutex};

use crate::{
    clip, export, history, moving,
    picture::Shot,
    record, replay,
    screens::{self, ControlLock, Screen},
    source::{HistoryEvent, HistoryHandle, LiveEvent, LiveHandle, PaneCapabilities, ScreenId, ScreenInfo, ScreenLocation, ScreenSource, ScreenState},
};

/// Pictures a second, at most, where frames can't be shown live.
const FPS: f64 = 8.0;
/// A live view that ended (the screen's app_server restarting) is tried
/// again this often.
const LIVE_RETRY: Duration = Duration::from_millis(500);
/// After a change on disk, how long to gather more before reading; and
/// how often the frames alone are read again (a running screen adds them
/// every second or faster).
const SETTLE: Duration = Duration::from_millis(200);
const FRAMES_EVERY: Duration = Duration::from_secs(1);
/// Maximum time a synthetic input or screenshot IPC request waits on a stalled
/// screen compositor before timing out cleanly.
const INPUT_IPC_TIMEOUT: Duration = Duration::from_secs(2);
const SHOT_IPC_TIMEOUT: Duration = Duration::from_secs(5);
/// Bounded wait on `release_control` so final key/button releases land on the
/// screen before `.control` is unlinked without ever wedging teardown.
const RELEASE_WAIT: Duration = Duration::from_millis(500);
/// Per-process sequence giving every local fallback watch its own capture file.
static CAPTURE_SEQ: AtomicU64 = AtomicU64::new(1);

fn next_capture_path(views: &Path) -> PathBuf {
    let seq = CAPTURE_SEQ.fetch_add(1, Ordering::Relaxed);
    views.join(format!("view-{}-{seq}.rgba", std::process::id()))
}

/// Moves one after another: only the last counts.
pub(crate) fn run_together(events: &mut Vec<SyntheticEvent>) {
    events.dedup_by(|b, a| {
        let moves = matches!((&*a, &*b), (SyntheticEvent::Move { .. }, SyntheticEvent::Move { .. }));
        if moves {
            *a = b.clone();
        }
        moves
    });
}

/// Width, height (little-endian u32), then straight RGBA rows (opaque).
#[cfg(test)]
pub(crate) fn decode_rgba_shot(mut bytes: Vec<u8>) -> Option<Shot> {
    let w = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?) as usize;
    let h = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
    let pixel_bytes = w.checked_mul(h)?.checked_mul(4)?;
    let end = 8usize.checked_add(pixel_bytes)?;
    if bytes.len() < end {
        return None;
    }
    bytes.drain(..8);
    bytes.truncate(pixel_bytes);
    Some(Shot { size: [w, h], rgba: bytes })
}

/// Build a [`ScreenInfo`] snapshot for a local screen.
pub fn screen_info_for(name: &str, running: Option<&Screen>, title: Option<String>, archived: bool, last_used_ms: Option<u64>) -> ScreenInfo {
    let (state, size, scale, sound) = match running {
        Some(s) => (ScreenState::Running, Some((s.width, s.height)), s.scale, s.sound),
        None => (if archived { ScreenState::Archived } else { ScreenState::Stopped }, None, 1.0, false),
    };
    ScreenInfo {
        id: ScreenId::local(name),
        title,
        state,
        size,
        scale,
        sound,
        // Local header derives "up …" from history's own start action, matching
        // `apps/screen/src/viewer.rs`.
        started_ms: None,
        last_used_ms,
        disconnected_at_ms: None,
        actions: None,
        controlled: false,
        capabilities: PaneCapabilities::local(name),
    }
}

enum LocalInputCmd {
    Events(Vec<SyntheticEvent>),
    Release { final_events: Vec<SyntheticEvent>, lock: Option<ControlLock>, done: Sender<()> },
}

/// Pinned identity of the exact local headless session that granted Take
/// Control, preventing a late key-up or reconnect from reaching a replacement
/// session that reused the same `wayland-N` socket path.
struct PinnedSession {
    pid: i32,
    start_ticks: Option<u64>,
    pidfd: Option<OwnedFd>,
    olo_socket: PathBuf,
    uid: u32,
}

impl PinnedSession {
    fn pin(pid: i32, olo_socket: PathBuf) -> PinnedSession {
        // SAFETY: `geteuid()` has no preconditions.
        let uid = unsafe { libc::geteuid() };
        let start_ticks = screens::proc_start_ticks(pid);
        let pidfd = open_pidfd(pid);
        PinnedSession { pid, start_ticks, pidfd, olo_socket, uid }
    }

    /// True only while the exact originally pinned session process is alive.
    fn is_alive(&self) -> bool {
        if self.pid <= 0 {
            return false;
        }
        if let Some(fd) = &self.pidfd {
            let mut pfd = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            // SAFETY: valid owned pidfd; non-blocking poll. Readable pidfd means process exited.
            if unsafe { libc::poll(&mut pfd, 1, 0) } != 0 {
                return false;
            }
        }
        if let Some(exp) = self.start_ticks
            && screens::proc_start_ticks(self.pid) != Some(exp)
        {
            return false;
        }
        true
    }

    /// Connect to `olo_socket` and verify via `SO_PEERCRED` and `pidfd` that
    /// the peer belongs to this exact pinned session (same UID and in the
    /// pinned session's process tree/group, never a replacement session).
    fn connect_verified(&self, timeout: Duration) -> Option<Client> {
        if !self.is_alive() {
            return None;
        }
        let client = Client::connect_to(&self.olo_socket).ok()?.answers_within(timeout);
        let sock = client.closer().ok()?;
        let cred = peer_cred(&sock)?;
        if cred.uid != self.uid || !peer_belongs_to_session(cred.pid, self.pid) || !self.is_alive() {
            return None;
        }
        Some(client)
    }
}

fn open_pidfd(pid: i32) -> Option<OwnedFd> {
    if pid <= 0 {
        return None;
    }
    // SAFETY: Linux `pidfd_open(2)` syscall with valid flags; fd wrapped immediately.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::c_int, libc::PIDFD_NONBLOCK as libc::c_uint) as libc::c_int };
    if fd < 0 { None } else { Some(unsafe { OwnedFd::from_raw_fd(fd) }) }
}

fn peer_cred(sock: &UnixStream) -> Option<libc::ucred> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: valid unix socket fd and properly sized `ucred` out-param.
    let rc = unsafe { libc::getsockopt(sock.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) };
    (rc == 0 && len as usize == std::mem::size_of::<libc::ucred>()).then_some(cred)
}

/// True if `peer_pid` is `session_pid`, shares its process group (`pgrp ==
/// session_pid`), or descends from `session_pid` via `/proc/*/stat` `ppid`.
fn peer_belongs_to_session(peer_pid: i32, session_pid: i32) -> bool {
    if peer_pid <= 0 || session_pid <= 0 {
        return false;
    }
    if peer_pid == session_pid {
        return true;
    }
    let mut cur = peer_pid;
    for _ in 0..8 {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{cur}/stat")) else { return false };
        let Some((_, rest)) = stat.rsplit_once(')') else { return false };
        let mut fields = rest.split_whitespace();
        let _state = fields.next();
        let ppid = fields.next().and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
        let pgrp = fields.next().and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
        if pgrp == session_pid || ppid == session_pid {
            return true;
        }
        if ppid <= 1 || ppid == cur {
            return false;
        }
        cur = ppid;
    }
    false
}

/// Only pointer moves and key/button releases are safe to retry if a reply is
/// lost after writing; duplicating a click or key press can trigger unintended
/// double clicks or double typing.
fn is_retry_safe(events: &[SyntheticEvent]) -> bool {
    events.iter().all(|e| matches!(e, SyntheticEvent::Move { .. } | SyntheticEvent::Key { pressed: false, .. } | SyntheticEvent::Button { pressed: false, .. }))
}

struct ControlSession {
    lock: Option<ControlLock>,
    wayland_display: String,
    input_tx: Sender<LocalInputCmd>,
    _input_join: JoinHandle<()>,
}

/// Spawn one input worker bound exclusively to `pinned` (verifying `pidfd` and
/// `SO_PEERCRED` on every connection so a replacement session can never receive
/// late events), with bounded socket timeouts, serializing final release events
/// strictly before dropping `ControlLock`.
fn spawn_bound_input_worker(name: String, pinned: PinnedSession, timeout: Duration) -> (Sender<LocalInputCmd>, JoinHandle<()>) {
    let (tx, rx) = channel::<LocalInputCmd>();
    let join = std::thread::Builder::new()
        .name("screen-input".into())
        .spawn(move || {
            let mut client: Option<Client> = None;
            let mut pending: Option<LocalInputCmd> = None;
            loop {
                let cmd = match pending.take() {
                    Some(c) => c,
                    None => match rx.recv() {
                        Ok(c) => c,
                        Err(_) => break,
                    },
                };
                match cmd {
                    LocalInputCmd::Events(mut events) => {
                        while let Ok(next) = rx.try_recv() {
                            match next {
                                LocalInputCmd::Events(more) => events.extend(more),
                                release @ LocalInputCmd::Release { .. } => {
                                    pending = Some(release);
                                    break;
                                }
                            }
                        }
                        send_input_batch_to(&mut client, &pinned, &name, events, timeout);
                    }
                    LocalInputCmd::Release { final_events, mut lock, done } => {
                        if !final_events.is_empty() {
                            send_input_batch_to(&mut client, &pinned, &name, final_events, timeout);
                        }
                        // Drop the exclusive flock only AFTER final release events
                        // have been delivered to the bound screen session.
                        if let Some(mut l) = lock.take() {
                            l.release();
                        }
                        let _ = done.send(());
                        break;
                    }
                }
            }
        })
        .expect("spawn screen-input");
    (tx, join)
}

fn send_input_batch_to(client: &mut Option<Client>, pinned: &PinnedSession, name: &str, mut events: Vec<SyntheticEvent>, timeout: Duration) -> bool {
    run_together(&mut events);
    if events.is_empty() {
        return true;
    }
    let max_attempts = if is_retry_safe(&events) { 2 } else { 1 };
    for _ in 0..max_attempts {
        if !pinned.is_alive() {
            *client = None;
            return false;
        }
        if client.is_none() {
            *client = pinned.connect_verified(timeout);
            // What this connection presses is its own: the screen's
            // compositor lets go of whatever is still down when the
            // connection ends, however it ends (`Request::OwnInput`: a
            // viewer killed with a key down left it down). One that
            // doesn't know the request says so, and things are as they
            // were.
            // (A compositor from before it answers "unknown request":
            // that is an answer, and the connection is good. Taken for
            // a failure, a screen started before an upgrade took no
            // input at all. A code review, 2026-11-27.)
            if let Some(fresh) = client.as_mut()
                && matches!(fresh.request(&Request::OwnInput), Err(e) if !matches!(e, olo_protocol::ClientError::Server(_)))
            {
                *client = None;
            }
        }
        let Some(c) = client.as_mut() else { return false };
        match c.request(&Request::Synthetic { events: events.clone() }) {
            Ok(Reply::Ok) => return true,
            Ok(Reply::Error { message }) => {
                // Explicit server refusal: never retry.
                tracing::warn!("input to screen {name} refused: {message}");
                return false;
            }
            Ok(_) => return true,
            Err(e) => {
                tracing::warn!("input to screen {name}: {e}");
                *client = None;
            }
        }
    }
    false
}

/// Local desktop implementation of [`ScreenSource`].
pub struct LocalScreenSource {
    id: ScreenId,
    info: Mutex<ScreenInfo>,
    /// Running session's `started` timestamp (seconds since epoch) kept separate
    /// from `info.started_ms` (which stays `None` on local screens so the header
    /// derives "up …" from history's start action) so Stop confirms generation.
    expected_started: Mutex<Option<u64>>,
    control: Mutex<Option<ControlSession>>,
    player: Arc<Mutex<OwnedPlayer>>,
    seg_cache: Arc<Mutex<SegmentCache>>,
    history_following: Arc<AtomicBool>,
}

impl LocalScreenSource {
    /// Create a source from a prepared [`ScreenInfo`].
    pub fn new(info: ScreenInfo) -> LocalScreenSource {
        debug_assert_eq!(info.id.location, ScreenLocation::Local);
        LocalScreenSource {
            id: info.id.clone(),
            info: Mutex::new(info),
            expected_started: Mutex::new(None),
            control: Mutex::new(None),
            player: Arc::new(Mutex::new(OwnedPlayer::new(Vec::new()))),
            seg_cache: Arc::new(Mutex::new(SegmentCache::new())),
            history_following: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create a source for a running local screen.
    pub fn from_running(screen: &Screen, title: Option<String>, last_used_ms: Option<u64>) -> LocalScreenSource {
        let src = Self::new(screen_info_for(&screen.name, Some(screen), title, false, last_used_ms));
        *src.expected_started.lock().unwrap_or_else(|e| e.into_inner()) = Some(screen.started);
        src
    }

    /// Create a source for a stopped or archived local screen.
    pub fn from_stopped(name: impl Into<String>, title: Option<String>, archived: bool, last_used_ms: Option<u64>) -> LocalScreenSource {
        let name = name.into();
        Self::new(screen_info_for(&name, None, title, archived, last_used_ms))
    }

    /// Update the expected running session's `started` generation (seconds).
    pub fn set_session_started(&self, started: Option<u64>) {
        *self.expected_started.lock().unwrap_or_else(|e| e.into_inner()) = started;
    }

    /// Replace the cached metadata snapshot (called by the host when its
    /// background screen watcher observes changes).
    pub fn set_info(&self, info: ScreenInfo) {
        *self.info.lock().unwrap_or_else(|e| e.into_inner()) = info;
    }

    fn begin_control_session(&self, pinned: PinnedSession, wayland_display: String, lock: ControlLock, timeout: Duration) {
        let (input_tx, input_join) = spawn_bound_input_worker(self.id.name.clone(), pinned, timeout);
        *self.control.lock().unwrap_or_else(|e| e.into_inner()) = Some(ControlSession { lock: Some(lock), wayland_display, input_tx, _input_join: input_join });
        self.info.lock().unwrap_or_else(|e| e.into_inner()).controlled = true;
    }

    /// Queue final input releases + `ControlLock` drop onto the `screen-input`
    /// worker. When `wait == false` (normal UI path: tab switch / Stop
    /// Controlling / pane close), returns immediately without blocking the
    /// window thread while the worker holds `ControlLock` until final releases
    /// complete. When `wait == true` (worker-thread Stop / tests), waits up to
    /// [`RELEASE_WAIT`] for the worker to finish unlocking.
    fn finish_control_session(&self, final_events: Vec<SyntheticEvent>, wait: bool) {
        let session = self.control.lock().unwrap_or_else(|e| e.into_inner()).take();
        self.info.lock().unwrap_or_else(|e| e.into_inner()).controlled = false;
        let Some(mut session) = session else { return };
        let (done_tx, done_rx) = channel::<()>();
        let lock = session.lock.take();
        if session.input_tx.send(LocalInputCmd::Release { final_events, lock, done: done_tx }).is_ok() && wait {
            let _ = done_rx.recv_timeout(RELEASE_WAIT);
        }
    }
}

impl Drop for LocalScreenSource {
    fn drop(&mut self) {
        self.finish_control_session(Vec::new(), false);
    }
}

/// A screen being watched for the Live tab, until it's dropped: its
/// frames shown live, or a picture taken each time it draws something
/// new, at most [`FPS`] a second.
struct LocalLiveHandle {
    stop: Arc<AtomicBool>,
    /// True while the Live tab is hidden: dmabuf capture stops and picture
    /// fallback sleeps without capturing until resumed.
    paused: Arc<AtomicBool>,
    /// What wakes the live view's worker to end.
    live: Arc<Mutex<Option<olo_capture::live::Waker>>>,
    /// The connection the screen's events come on (shut to stop).
    events: Arc<Mutex<Option<UnixStream>>>,
    /// Closer for an in-flight fallback `Request::Screenshot` IPC socket.
    shot_closer: Arc<Mutex<Option<UnixStream>>>,
    /// Something new to take a picture of.
    dirty: Arc<(Mutex<bool>, Condvar)>,
}

impl LocalLiveHandle {
    fn shutdown(&mut self) {
        // Update `stop` and notify under `dirty.0` so the worker can never miss
        // a wakeup between checking the predicate and entering `wait`.
        {
            let _g = self.dirty.0.lock().unwrap_or_else(|e| e.into_inner());
            self.stop.store(true, Ordering::Release);
            self.dirty.1.notify_all();
        }
        if let Some(w) = self.live.lock().unwrap_or_else(|e| e.into_inner()).take() {
            w.stop();
        }
        if let Some(s) = self.events.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        if let Some(s) = self.shot_closer.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }
}

impl LiveHandle for LocalLiveHandle {
    fn stop(&mut self) {
        self.shutdown();
    }

    fn set_paused(&mut self, paused: bool) {
        let was = self.paused.swap(paused, Ordering::AcqRel);
        if was == paused {
            return;
        }
        if paused {
            // Drop any active GPU dmabuf screencopy session while hidden so
            // the compositor does zero capture work until Live is shown again.
            if let Some(w) = self.live.lock().unwrap_or_else(|e| e.into_inner()).take() {
                w.stop();
            }
            let _g = self.dirty.0.lock().unwrap_or_else(|e| e.into_inner());
            self.dirty.1.notify_all();
        } else {
            // Capture a fresh frame immediately on resume.
            mark(&self.dirty);
        }
    }
}

impl Drop for LocalLiveHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn mark(dirty: &(Mutex<bool>, Condvar)) {
    *dirty.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
    dirty.1.notify_all();
}

/// Show screen `name`'s frames live until the watch stops (true), or
/// false at once if they can't come that way (pictures are taken then).
/// A view that ends (the screen's app_server restarting) starts again.
fn stream_live(
    name: &str,
    on_event: &(dyn Fn(LiveEvent) + Send + Sync),
    modifiers: &[u64],
    stop: &AtomicBool,
    paused: &AtomicBool,
    dirty: &(Mutex<bool>, Condvar),
    slot: &Mutex<Option<olo_capture::live::Waker>>,
) -> bool {
    let mut streamed = false;
    loop {
        if stop.load(Ordering::Acquire) {
            return true;
        }
        if paused.load(Ordering::Acquire) {
            let mut d = dirty.0.lock().unwrap_or_else(|e| e.into_inner());
            while paused.load(Ordering::Acquire) && !stop.load(Ordering::Acquire) {
                d = dirty.1.wait(d).unwrap_or_else(|e| e.into_inner());
            }
            if stop.load(Ordering::Acquire) {
                return true;
            }
        }
        let started = screens::get(name).and_then(|s| Live::start(Some(&s.wayland_display), &olo_capture::Source::Output(None), true, modifiers));
        let mut live = match started {
            Ok(live) => live,
            Err(e) if !streamed => {
                tracing::info!("screen {name}: no live view ({e}); taking pictures");
                return false;
            }
            Err(_) => {
                std::thread::sleep(LIVE_RETRY);
                continue;
            }
        };
        let waker = live.waker();
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(waker.clone());
        // (Stopped or paused as it started: the watch found no waker to wake.)
        if stop.load(Ordering::Acquire) {
            return true;
        }
        if paused.load(Ordering::Acquire) {
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
            continue;
        }
        let released: Arc<dyn Fn(u64) + Send + Sync> = Arc::new(move |id| waker.released(id));
        let mut retry_sleep = false;
        loop {
            match live.next() {
                Ok(Some(f)) => {
                    streamed = true;
                    let planes = f.planes.into_iter().map(|(fd, offset, stride)| ExternalPlane { fd, offset, stride }).collect();
                    let buffer = ExternalBuffer { id: f.id, width: f.width, height: f.height, fourcc: f.fourcc, modifier: f.modifier, planes, released: released.clone() };
                    on_event(LiveEvent::Dmabuf(buffer));
                }
                Ok(None) => {
                    // Treat a Waker stop as termination ONLY when `stop` is set:
                    // a quick History -> Live toggle can clear `paused` before
                    // this thread consumes the wake, so loop and reopen cleanly.
                    if stop.load(Ordering::Acquire) {
                        return true;
                    }
                    break;
                }
                Err(e) => {
                    tracing::info!("screen {name}: the live view ended ({e}); starting it again");
                    retry_sleep = true;
                    break;
                }
            }
        }
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
        if retry_sleep && !paused.load(Ordering::Acquire) && !stop.load(Ordering::Acquire) {
            std::thread::sleep(LIVE_RETRY);
        }
    }
}

/// Mark `dirty` each time screen `name` draws something new, until
/// `stop` (sleeping silently while `paused` even on older compositors without
/// `EventKind::Painted`).
fn follow_paints(name: &str, stop: Arc<AtomicBool>, paused: Arc<AtomicBool>, events: Arc<Mutex<Option<UnixStream>>>, dirty: Arc<(Mutex<bool>, Condvar)>, period: Duration) {
    let name = name.to_owned();
    let _ = std::thread::Builder::new().name("screen-events".into()).spawn(move || {
        let client = screens::get(&name).and_then(|s| Client::connect_to(&s.olo_socket).map_err(|e| e.to_string()));
        let Ok(mut client) = client else { return };
        *events.lock().unwrap_or_else(|e| e.into_inner()) = client.closer().ok();
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match client.request(&Request::SubscribeTo { events: vec![EventKind::Painted] }) {
            Ok(Reply::Ok) => {
                while let Ok(reply) = client.read() {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    if let Reply::Event { event: Event::Painted { .. } } = reply {
                        mark(&dirty);
                    }
                }
            }
            // An app_server from before it said when it drew: a picture
            // each frame's time, sleeping with zero wakeups whenever hidden.
            _ => {
                while !stop.load(Ordering::Acquire) {
                    if paused.load(Ordering::Acquire) {
                        let mut g = dirty.0.lock().unwrap_or_else(|e| e.into_inner());
                        while paused.load(Ordering::Acquire) && !stop.load(Ordering::Acquire) {
                            g = dirty.1.wait(g).unwrap_or_else(|e| e.into_inner());
                        }
                        if stop.load(Ordering::Acquire) {
                            return;
                        }
                    }
                    mark(&dirty);
                    std::thread::sleep(period);
                }
            }
        }
    });
}

struct LocalHistoryHandle {
    watch: Option<olo_kit::watch::Watch>,
    stop: Arc<AtomicBool>,
    following: Arc<AtomicBool>,
}

impl HistoryHandle for LocalHistoryHandle {
    fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.following.store(false, Ordering::Release);
        self.watch = None;
    }
}

impl Drop for LocalHistoryHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

fn take_picture_bounded(screen: &Screen, path: &Path, closer: &Mutex<Option<UnixStream>>) -> Result<Shot, String> {
    let mut client = Client::connect_to(&screen.olo_socket).map_err(|e| format!("can't reach screen {}: {e}", screen.name))?.answers_within(SHOT_IPC_TIMEOUT);
    *closer.lock().unwrap_or_else(|e| e.into_inner()) = client.closer().ok();
    let res = client.request(&Request::Screenshot { path: path.display().to_string(), output: None, window: None, border: true, pointer: true, format: ImageFormat::Rgba, as_shown: false });
    *closer.lock().unwrap_or_else(|e| e.into_inner()) = None;
    match res {
        Ok(Reply::Ok) => {
            // Validate 8-byte header & exact file size before allocating/reading pixels.
            let frame = record::read_rgba_frame_bounded(path)?;
            Ok(Shot { size: [frame.width, frame.height], rgba: frame.rgba })
        }
        Ok(Reply::Error { message }) => Err(message),
        Ok(_) => Err(format!("unexpected answer from screen {}", screen.name)),
        Err(e) => Err(format!("screen {}: {e}", screen.name)),
    }
}

fn frame_at_in(dir: &Path, player: &Mutex<OwnedPlayer>, seg_cache: &Mutex<SegmentCache>, following: bool, at_ms: u64) -> Result<Option<(u64, Frame)>, String> {
    let mut p = player.lock().unwrap_or_else(|e| e.into_inner());
    if !following || p.segments().is_empty() {
        // Incremental stat-keyed scan: 0 record reads when unchanged, detects
        // appends, equal-size inode replacements, and full clears even within
        // the previously cached time range.
        let scan = seg_cache.lock().unwrap_or_else(|e| e.into_inner()).scan(dir);
        if scan.changed || scan.replaced || scan.segments.is_empty() != p.segments().is_empty() {
            p.update(scan.segments);
        }
    }
    Ok(p.at(at_ms).map(|(when, frame)| (when, frame.clone())))
}

fn rename_in(state_root: &Path, runtime_dir: &Path, name: &str, title: Option<&str>) -> Result<(), String> {
    screens::require_valid_name(name)?;
    let sdir = history::screen_dir_in(state_root, name)?;
    let exists = runtime_dir.join(format!("{name}.json")).is_file() || sdir.is_dir();
    if !exists {
        return Err(format!("there's no screen {name:?}"));
    }
    history::set_title_in(state_root, name, title)
}

impl LocalScreenSource {
    pub(crate) fn set_controlled_in(&self, rt_dir: &Path, on: bool) -> Result<(), String> {
        if on {
            if self.control.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
                return Ok(());
            }
            let screen = screens::get_in(rt_dir, &self.id.name)?;
            // Final pass B.2 (#5/#24): verify the session we are about to
            // control still matches the start token this source was created for
            // so a replacement session under the same name is never controlled.
            if let Some(expected) = *self.expected_started.lock().unwrap_or_else(|e| e.into_inner())
                && screen.started != expected
            {
                return Err(format!("screen {} was replaced by a newer session; refusing to take control", self.id.name));
            }
            let lock = screens::try_acquire_control_in(rt_dir, &self.id.name)?;
            let pinned = PinnedSession::pin(screen.pid, screen.olo_socket);
            self.begin_control_session(pinned, screen.wayland_display, lock, INPUT_IPC_TIMEOUT);
            Ok(())
        } else {
            self.finish_control_session(Vec::new(), false);
            Ok(())
        }
    }
}

impl ScreenSource for LocalScreenSource {
    fn id(&self) -> &ScreenId {
        &self.id
    }

    fn info(&self) -> ScreenInfo {
        self.info.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Watch screen `name`: its frames shown live where the window's
    /// compositor takes dmabufs with `modifiers` (`stream_live`); else one
    /// worker hears when it draws something new (`Painted`; from an
    /// app_server before those, every frame's time), the other takes the
    /// picture then.
    fn watch_live(&self, modifiers: Vec<u64>, on_event: Box<dyn Fn(LiveEvent) + Send + Sync>) -> Box<dyn LiveHandle> {
        let handle = LocalLiveHandle {
            stop: Arc::default(),
            paused: Arc::default(),
            live: Arc::default(),
            events: Arc::default(),
            shot_closer: Arc::default(),
            dirty: Arc::new((Mutex::new(true), Condvar::new())),
        };
        let period = Duration::from_secs_f64(1.0 / FPS);
        let (name, stop, paused, live, events, shot_closer, dirty) =
            (self.id.name.clone(), handle.stop.clone(), handle.paused.clone(), handle.live.clone(), handle.events.clone(), handle.shot_closer.clone(), handle.dirty.clone());
        let _ = std::thread::Builder::new().name("screen-picture".into()).spawn(move || {
            if !modifiers.is_empty() && stream_live(&name, on_event.as_ref(), &modifiers, &stop, &paused, &dirty, &live) {
                return;
            }
            follow_paints(&name, stop.clone(), paused.clone(), events, dirty.clone(), period);
            // In a folder of their own: the screens' folder is followed for
            // screens coming and going, not for frames. Each watch uses its
            // own file so concurrent panes or overlapping teardowns never clash.
            let views = screens::dir().join("views");
            let path = next_capture_path(&views);
            let _ = std::fs::create_dir_all(&views);
            loop {
                {
                    let mut d = dirty.0.lock().unwrap_or_else(|e| e.into_inner());
                    while (!*d || paused.load(Ordering::Acquire)) && !stop.load(Ordering::Acquire) {
                        d = dirty.1.wait(d).unwrap_or_else(|e| e.into_inner());
                    }
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    *d = false;
                }
                let began = Instant::now();
                let shot = screens::get(&name).and_then(|s| take_picture_bounded(&s, &path, &shot_closer));
                if stop.load(Ordering::Acquire) {
                    break;
                }
                match shot {
                    Ok(shot) => on_event(LiveEvent::Rgba(Arc::new(shot))),
                    Err(message) => on_event(LiveEvent::Failed(message)),
                }
                std::thread::sleep(period.saturating_sub(began.elapsed()));
            }
            let _ = std::fs::remove_file(&path);
        });
        Box::new(handle)
    }

    fn set_controlled(&self, on: bool) -> Result<(), String> {
        self.set_controlled_in(&screens::dir(), on)
    }

    fn release_control(&self, final_events: Vec<SyntheticEvent>) -> Result<(), String> {
        // Non-blocking on the UI thread: `screen-input` retains `ControlLock`
        // until final releases finish on the pinned session, then unlocks.
        self.finish_control_session(final_events, false);
        Ok(())
    }

    fn release_control_and_wait(&self, final_events: Vec<SyntheticEvent>) -> Result<(), String> {
        // Worker-thread path (e.g. before `stop()`): wait for `screen-input`
        // to finish releasing held keys and drop `ControlLock`.
        self.finish_control_session(final_events, true);
        Ok(())
    }

    fn send_input(&self, events: Vec<SyntheticEvent>) {
        if events.is_empty() {
            return;
        }
        if let Some(session) = self.control.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = session.input_tx.send(LocalInputCmd::Events(events));
        }
    }

    fn join_clipboard(&self) -> Option<Box<dyn Send>> {
        let display = self.control.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.wayland_display.clone())?;
        Some(Box::new(clip::join_display(display)))
    }

    /// Read screen `name`'s history and frames, and again whenever its folder
    /// changes: only what was added, on a worker, for as long as the returned
    /// handle is kept.
    fn follow_history(&self, on_event: Box<dyn Fn(HistoryEvent) + Send + Sync>) -> Box<dyn HistoryHandle> {
        let (tx, rx) = channel::<()>();
        let stop = Arc::new(AtomicBool::new(false));
        // Install the folder watch BEFORE spawning the worker so no write in
        // the gap between the initial scan and watch setup can be missed.
        let watch = olo_kit::watch::watch([history::dir(&self.id.name)], move |_| {
            let _ = tx.send(());
        });
        self.history_following.store(true, Ordering::Release);
        let stop_bg = stop.clone();
        let following_bg = self.history_following.clone();
        let name = self.id.name.clone();
        let player = self.player.clone();
        let seg_cache = self.seg_cache.clone();
        let spawned = std::thread::Builder::new().name("screen-history".into()).spawn(move || {
            let mut reader = history::Reader::default();
            let mut first = true;
            let mut frames_read: Option<Instant> = None;
            let mut last_index = Arc::new(FrameIndex::default());
            loop {
                if stop_bg.load(Ordering::Relaxed) {
                    following_bg.store(false, Ordering::Release);
                    return;
                }
                let (actions, reset) = reader.more(&name);
                // The frames: with new actions, or now and then as they come.
                let due = frames_read.is_none_or(|t| t.elapsed() >= FRAMES_EVERY);
                let read_frames = first || reset || !actions.is_empty() || due;
                let mut frames_changed = false;
                let mut index_replaced = false;
                if read_frames {
                    frames_read = Some(Instant::now());
                    if let Ok(dir) = history::dir_checked(&name) {
                        let scan = seg_cache.lock().unwrap_or_else(|e| e.into_inner()).scan(&dir);
                        if first || scan.changed || scan.replaced {
                            let frames = scan.segments.iter().flat_map(|s| s.frames.iter().map(|e| olo_screen_codec::FrameMark { at: e.at, key: e.key })).collect();
                            last_index = Arc::new(FrameIndex { frames, last_size: scan.last_size });
                            player.lock().unwrap_or_else(|e| e.into_inner()).update(scan.segments);
                            frames_changed = scan.changed || scan.replaced;
                            index_replaced = scan.replaced;
                        }
                    }
                }
                if first || reset || !actions.is_empty() || frames_changed {
                    let texts = actions.iter().map(replay::search_text).collect();
                    on_event(HistoryEvent::Loaded { actions, texts, reset: first || reset, index_reset: first || reset || index_replaced, frames: last_index.clone() });
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
                    Err(RecvTimeoutError::Disconnected) => {
                        following_bg.store(false, Ordering::Release);
                        return;
                    }
                }
            }
        });
        let watch = if spawned.is_ok() {
            watch
        } else {
            self.history_following.store(false, Ordering::Release);
            None
        };
        Box::new(LocalHistoryHandle { watch, stop, following: self.history_following.clone() })
    }

    fn frame_at(&self, at_ms: u64) -> Result<Option<(u64, Frame)>, String> {
        let dir = history::screen_dir_in(&history::root_dir(), &self.id.name)?;
        frame_at_in(&dir, &self.player, &self.seg_cache, self.history_following.load(Ordering::Acquire), at_ms)
    }

    fn export_video(&self, actions: Option<&[usize]>, zoom: usize, out: &Path, progress: &mut dyn FnMut(f32) -> bool) -> Result<export::Made, String> {
        export::export(&self.id.name, actions, zoom, olo_video::Choice::Auto, out, progress)
    }

    fn session_started(&self) -> Option<u64> {
        *self.expected_started.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn start(&self) -> Result<ScreenInfo, String> {
        let opts = screens::kept(&self.id.name).unwrap_or_default();
        let screen = screens::start(&self.id.name, &opts)?;
        *self.expected_started.lock().unwrap_or_else(|e| e.into_inner()) = Some(screen.started);
        let mut info = self.info.lock().unwrap_or_else(|e| e.into_inner());
        info.state = ScreenState::Running;
        info.size = Some((screen.width, screen.height));
        info.scale = screen.scale;
        info.sound = screen.sound;
        Ok(info.clone())
    }

    fn stop(&self) -> Result<(), String> {
        let expected_started = *self.expected_started.lock().unwrap_or_else(|e| e.into_inner());
        self.stop_if_expected(expected_started, None)
    }

    fn stop_if_generation(&self, expected_started: Option<u64>) -> Result<(), String> {
        self.stop_if_expected(expected_started, None)
    }

    fn stop_if_expected(&self, expected_started: Option<u64>, expected_last_used: Option<u64>) -> Result<(), String> {
        // Ensure any in-flight local release completes before stopping.
        self.finish_control_session(Vec::new(), true);
        screens::stop_if_expected(&self.id.name, expected_started, expected_last_used)?;
        *self.expected_started.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let mut info = self.info.lock().unwrap_or_else(|e| e.into_inner());
        info.state = ScreenState::Stopped;
        info.controlled = false;
        Ok(())
    }

    fn clear_history(&self) -> Result<(), String> {
        self.clear_history_expected(None, None)
    }

    fn clear_history_expected(&self, expected_started: Option<u64>, expected_last_used: Option<u64>) -> Result<(), String> {
        screens::clear_history_if_stopped_expected(&self.id.name, expected_started, expected_last_used)?;
        self.player.lock().unwrap_or_else(|e| e.into_inner()).reset();
        *self.seg_cache.lock().unwrap_or_else(|e| e.into_inner()) = SegmentCache::new();
        self.info.lock().unwrap_or_else(|e| e.into_inner()).last_used_ms = None;
        Ok(())
    }

    fn rename(&self, title: Option<&str>) -> Result<(), String> {
        rename_in(&history::root_dir(), &screens::dir(), &self.id.name, title)?;
        let clean = title.map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned);
        self.info.lock().unwrap_or_else(|e| e.into_inner()).title = clean;
        Ok(())
    }

    fn archive(&self, on: bool) -> Result<(), String> {
        screens::archive(&self.id.name, on)?;
        let mut info = self.info.lock().unwrap_or_else(|e| e.into_inner());
        info.state = if on { ScreenState::Archived } else { ScreenState::Stopped };
        Ok(())
    }

    fn delete(&self) -> Result<(), String> {
        screens::delete(&self.id.name)
    }

    fn delete_expected(&self, expected_started: Option<u64>, expected_last_used: Option<u64>) -> Result<(), String> {
        screens::delete_if_stopped_expected(&self.id.name, expected_started, expected_last_used)
    }

    fn give_windows(&self, to: Option<(u32, String)>) -> Result<Vec<String>, String> {
        let screen = screens::get(&self.id.name)?;
        let (index, output) = to.map_or((None, None), |(i, o)| (Some(i), Some(o)));
        moving::give(&screen, None, index, output).map(|o| o.stayed.into_iter().map(|s| format!("{}: {}", s.app_id, s.why)).collect())
    }

    fn person_workspaces(&self) -> Result<Vec<(String, u32, String)>, String> {
        moving::person_workspaces()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead as _, BufReader, Write as _},
        os::unix::net::UnixListener,
    };

    use super::*;

    fn scratch_dir(tag: &str) -> PathBuf {
        let d = PathBuf::from("/var/tmp").join(format!("olo-screen-pane-local-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn moves_run_together() {
        let mut events = vec![
            SyntheticEvent::Move { x: 1.0, y: 1.0 },
            SyntheticEvent::Move { x: 2.0, y: 2.0 },
            SyntheticEvent::Button { button: screens::LEFT, pressed: true },
            SyntheticEvent::Move { x: 3.0, y: 3.0 },
            SyntheticEvent::Move { x: 4.0, y: 4.0 },
        ];
        run_together(&mut events);
        assert_eq!(events, vec![SyntheticEvent::Move { x: 2.0, y: 2.0 }, SyntheticEvent::Button { button: screens::LEFT, pressed: true }, SyntheticEvent::Move { x: 4.0, y: 4.0 }]);
    }

    #[test]
    fn decode_rgba_shot_checks_bounds() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&2u32.to_le_bytes());
        raw.extend_from_slice(&1u32.to_le_bytes());
        raw.extend_from_slice(&[10, 20, 30, 255, 40, 50, 60, 255]);
        let shot = decode_rgba_shot(raw).unwrap();
        assert_eq!(shot.size, [2, 1]);
        assert_eq!(shot.rgba, vec![10, 20, 30, 255, 40, 50, 60, 255]);
        assert!(decode_rgba_shot(vec![2, 0, 0, 0, 1, 0, 0, 0, 1, 2]).is_none());
    }

    #[test]
    fn astra_f3_release_control_delivers_final_input_before_unlocking() {
        let dir = scratch_dir("f3");
        let rt = dir.join("rt");
        let sock_path = dir.join("screen.sock");
        let ctrl_path = rt.join("qa.control");
        let listener = UnixListener::bind(&sock_path).unwrap();

        let saw_lock_while_receiving = Arc::new(AtomicBool::new(false));
        let saw_clone = saw_lock_while_receiving.clone();
        let ctrl_check = ctrl_path.clone();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut writer = stream.try_clone().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            // (A new input connection first says its input is its own: `Request::OwnInput`.)
            let n = reader.read_line(&mut line).unwrap();
            assert!(n > 0 && line.contains("\"own_input\""), "got: {line}");
            writeln!(writer, "{{\"type\":\"ok\"}}").unwrap();
            writer.flush().unwrap();
            line.clear();
            // Simulate compositor taking 60ms before reading final key-up.
            std::thread::sleep(Duration::from_millis(60));
            let n = reader.read_line(&mut line).unwrap();
            assert!(n > 0 && line.contains("\"synthetic\""), "got: {line}");
            // Control file MUST STILL EXIST when the final release arrives!
            saw_clone.store(ctrl_check.is_file(), Ordering::Release);
            writeln!(writer, "{{\"type\":\"ok\"}}").unwrap();
            writer.flush().unwrap();
        });

        let src = LocalScreenSource::from_stopped("qa", None, false, None);
        let lock = screens::try_acquire_control_in(&rt, "qa").unwrap();
        assert!(ctrl_path.starts_with(&dir));
        assert!(ctrl_path.is_file());
        let pinned = PinnedSession::pin(std::process::id() as i32, sock_path);
        src.begin_control_session(pinned, "wayland-test".into(), lock, Duration::from_secs(2));
        src.finish_control_session(vec![SyntheticEvent::Key { key: 30, pressed: false }], true);
        server.join().unwrap();

        assert!(saw_lock_while_receiving.load(Ordering::Acquire), ".control must remain locked until final release events are delivered");
        assert!(!ctrl_path.exists(), ".control must be unlinked once final releases finish");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn astra_f11_fallback_capture_paths_are_unique() {
        let dir = scratch_dir("f11");
        let a = next_capture_path(&dir);
        let b = next_capture_path(&dir);
        assert!(a.starts_with(&dir) && b.starts_with(&dir));
        assert_ne!(a, b, "each fallback watch must use its own capture file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn astra_f12_stalled_socket_times_out_cleanly() {
        let dir = scratch_dir("f12");
        let sock_path = dir.join("stalled.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();
        // Accept and hold open without replying to simulate a wedged compositor.
        let holder = std::thread::spawn(move || {
            let (_s1, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(250));
        });

        let pinned = PinnedSession::pin(std::process::id() as i32, sock_path);
        let mut client = None;
        let t0 = Instant::now();
        send_input_batch_to(&mut client, &pinned, "qa", vec![SyntheticEvent::Key { key: 30, pressed: true }], Duration::from_millis(80));
        assert!(t0.elapsed() < Duration::from_secs(1), "stalled input IPC must time out promptly, took {:?}", t0.elapsed());
        let _ = holder.join();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn astra_f22_frame_at_without_follower_refreshes_on_new_frames() {
        let dir = scratch_dir("f22");
        let f1 = Frame { width: 64, height: 64, rgba: vec![10; 64 * 64 * 4] };
        let f2 = Frame { width: 64, height: 64, rgba: vec![20; 64 * 64 * 4] };
        let mut w = olo_screen_codec::Writer::new(dir.clone(), 1 << 30);
        w.add(f1.clone(), 1_000).unwrap();

        let player = Mutex::new(OwnedPlayer::new(Vec::new()));
        let cache = Mutex::new(SegmentCache::new());
        assert_eq!(frame_at_in(&dir, &player, &cache, false, 1_500).unwrap(), Some((1_000, f1)));

        // Append a second frame while no follower is running; asking past the
        // old tail must refresh the index and return the new frame.
        w.add(f2.clone(), 2_000).unwrap();
        assert_eq!(frame_at_in(&dir, &player, &cache, false, 2_500).unwrap(), Some((2_000, f2)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn review2_f4_pinned_session_rejects_replacement_and_never_retries_click_or_refusal() {
        let dir = scratch_dir("r2-f4");
        let sock_path = dir.join("screen.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        // 1. Spawn a short-lived child process, pin its PID, let it exit ->
        //    even though `sock_path` is listening (our process = replacement),
        //    `connect_verified` MUST refuse!
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let child_pid = child.id() as i32;
        let pinned_dead = PinnedSession::pin(child_pid, sock_path.clone());
        let _ = child.wait();
        assert!(pinned_dead.connect_verified(Duration::from_millis(200)).is_none(), "exited pinned session must never connect to replacement socket");

        // 2. Explicit `Reply::Error` and lost reply on a click press must NOT retry
        //    (each sends exactly 1 request).
        let seen = Arc::new(AtomicU64::new(0));
        let seen_w = seen.clone();
        let server = std::thread::spawn(move || {
            // (Counted: the clicks alone. Each connection's first word is
            // `own_input`; counted with them, two handshakes passed for two
            // clicks, and a compositor from before the request could be
            // sent no input at all without this noticing. The confirming
            // review.)
            let line_of = |reader: &mut BufReader<UnixStream>| {
                let mut line = String::new();
                (reader.read_line(&mut line).unwrap_or(0) > 0).then_some(line)
            };
            // Connection 1, a compositor from before `own_input`: it answers
            // "unknown request", is sent the click all the same, and refuses it.
            let (stream, _) = listener.accept().unwrap();
            let mut writer = stream.try_clone().unwrap();
            let mut reader = BufReader::new(stream);
            assert!(line_of(&mut reader).is_some_and(|l| l.contains("\"own_input\"")));
            writeln!(writer, "{{\"type\":\"error\",\"message\":\"unknown request\"}}").unwrap();
            writer.flush().unwrap();
            if line_of(&mut reader).is_some_and(|l| l.contains("\"synthetic\"")) {
                seen_w.fetch_add(1, Ordering::Relaxed);
                writeln!(writer, "{{\"type\":\"error\",\"message\":\"refused\"}}").unwrap();
                writer.flush().unwrap();
            }
            // Connection 2: `own_input` granted, the click read and the
            // connection dropped with no answer.
            let (stream2, _) = listener.accept().unwrap();
            let mut writer2 = stream2.try_clone().unwrap();
            let mut reader2 = BufReader::new(stream2);
            assert!(line_of(&mut reader2).is_some_and(|l| l.contains("\"own_input\"")));
            writeln!(writer2, "{{\"type\":\"ok\"}}").unwrap();
            writer2.flush().unwrap();
            if line_of(&mut reader2).is_some_and(|l| l.contains("\"synthetic\"")) {
                seen_w.fetch_add(1, Ordering::Relaxed);
            }
            drop(reader2);
        });

        let pinned = PinnedSession::pin(std::process::id() as i32, sock_path);
        let mut client = None;
        assert!(!send_input_batch_to(&mut client, &pinned, "qa", vec![SyntheticEvent::Button { button: screens::LEFT, pressed: true }], Duration::from_millis(200)));
        // Drop client so next batch opens Connection 2.
        client = None;
        assert!(!send_input_batch_to(&mut client, &pinned, "qa", vec![SyntheticEvent::Button { button: screens::LEFT, pressed: true }], Duration::from_millis(200)));
        server.join().unwrap();
        assert_eq!(seen.load(Ordering::Relaxed), 2, "each click reaches its compositor once (one from before `own_input` too), and neither a refusal nor a lost reply is tried again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn review2_f29_release_control_is_nonblocking_and_bounded_rgba_rejects_mismatch() {
        let dir = scratch_dir("r2-f29");
        let rt = dir.join("rt");
        let sock_path = dir.join("stalled.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();
        let holder = std::thread::spawn(move || {
            let (_s, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(350));
        });

        let src = LocalScreenSource::from_stopped("qa", None, false, None);
        let lock = screens::try_acquire_control_in(&rt, "qa").unwrap();
        let pinned = PinnedSession::pin(std::process::id() as i32, sock_path);
        src.begin_control_session(pinned, "wayland-test".into(), lock, Duration::from_millis(250));
        // `release_control` (UI path) must return immediately without blocking on the stalled socket.
        let t0 = Instant::now();
        src.release_control(vec![SyntheticEvent::Key { key: 30, pressed: false }]).unwrap();
        assert!(t0.elapsed() < Duration::from_millis(80), "release_control blocked UI thread for {:?}", t0.elapsed());
        holder.join().unwrap();

        // Bounded RGBA reader rejects header claiming 100x100 on a 16-byte file.
        let bad_rgba = dir.join("bad.rgba");
        let mut raw = Vec::new();
        raw.extend_from_slice(&100u32.to_le_bytes());
        raw.extend_from_slice(&100u32.to_le_bytes());
        raw.extend_from_slice(&[0u8; 8]);
        std::fs::write(&bad_rgba, raw).unwrap();
        assert!(record::read_rgba_frame_bounded(&bad_rgba).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn review2_f41_unfollowed_frame_at_sees_cleared_and_replaced_recording() {
        let dir = scratch_dir("r2-f41");
        let f1 = Frame { width: 64, height: 64, rgba: vec![11; 64 * 64 * 4] };
        let f2 = Frame { width: 64, height: 64, rgba: vec![99; 64 * 64 * 4] };
        olo_screen_codec::Writer::write_frames(&dir, vec![(f1.clone(), 1_000, true)]).unwrap();

        let player = Mutex::new(OwnedPlayer::new(Vec::new()));
        let cache = Mutex::new(SegmentCache::new());
        assert_eq!(frame_at_in(&dir, &player, &cache, false, 1_000).unwrap(), Some((1_000, f1)));

        // Replace recording at the EXACT same timestamp and size with different pixels.
        let tmp = dir.join("tmp");
        olo_screen_codec::Writer::write_frames(&tmp, vec![(f2.clone(), 1_000, true)]).unwrap();
        let seg_name = std::fs::read_dir(&dir).unwrap().flatten().find(|e| e.path().extension().is_some_and(|x| x == "olof")).unwrap().file_name();
        let repl = std::fs::read_dir(&tmp).unwrap().flatten().find(|e| e.path().extension().is_some_and(|x| x == "olof")).unwrap().path();
        std::fs::rename(&repl, dir.join(&seg_name)).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(frame_at_in(&dir, &player, &cache, false, 1_000).unwrap(), Some((1_000, f2)), "replaced equal-size segment must return new pixels");

        // Clear the recording on disk: unfollowed `frame_at_in` must return None (never stale cache).
        std::fs::remove_file(dir.join(&seg_name)).unwrap();
        assert_eq!(frame_at_in(&dir, &player, &cache, false, 1_000).unwrap(), None, "cleared recording must not serve stale cached pixels");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn astra_f24_rename_rejects_invalid_and_unlisted_names_in_scratch() {
        let dir = scratch_dir("f24");
        let state_root = dir.join("state/screens");
        let rt_dir = dir.join("rt/olo-screens");
        std::fs::create_dir_all(&state_root).unwrap();
        std::fs::create_dir_all(&rt_dir).unwrap();

        assert!(rename_in(&state_root, &rt_dir, "../astra-f24-escape", Some("oops")).is_err(), "path-shaped screen name must be rejected");
        assert!(!dir.join("state/astra-f24-escape").exists(), "rejected path-shaped name must write nothing outside screens/");

        assert!(rename_in(&state_root, &rt_dir, "unlisted-screen", Some("Ghost")).is_err(), "unlisted screen must be rejected");
        assert!(!state_root.join("unlisted-screen").exists());

        // Existing kept screen succeeds inside scratch only.
        std::fs::create_dir_all(state_root.join("real-screen")).unwrap();
        rename_in(&state_root, &rt_dir, "real-screen", Some("Renamed")).unwrap();
        let title_file = state_root.join("real-screen/title");
        assert!(title_file.starts_with(&dir));
        assert_eq!(std::fs::read_to_string(&title_file).unwrap(), "Renamed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn final_pass_b2_f5_f24_local_set_controlled_refuses_replaced_session() {
        use std::os::unix::process::CommandExt as _;
        let dir = scratch_dir("b2-f5-f24");
        let rt = dir.join("rt");
        std::fs::create_dir_all(&rt).unwrap();

        let mut fake_session = std::process::Command::new("/usr/bin/sleep").arg0("olo-session").arg("30").env("OLO_SCREEN", "qa").spawn().expect("spawn dummy session");
        let sock = rt.join("olo-test.sock");
        let _listener = UnixListener::bind(&sock).unwrap();
        let scr = Screen {
            name: "qa".into(),
            wayland_display: "wayland-b2".into(),
            olo_socket: sock,
            pid: fake_session.id() as i32,
            width: 800,
            height: 600,
            scale: 1.0,
            sound: false,
            started: 2000,
            bus: None,
            a11y: None,
        };
        std::fs::write(rt.join("qa.json"), serde_json::to_vec(&scr).unwrap()).unwrap();

        // Source was bound for older generation 1000; replacement 2000 is now running.
        let mut info = screen_info_for("qa", Some(&scr), None, false, None);
        info.started_ms = Some(1000 * 1000);
        let src = LocalScreenSource::new(info);
        src.set_session_started(Some(1000));

        let err = src.set_controlled_in(&rt, true).unwrap_err();
        assert!(err.contains("replaced"), "expected replacement refusal, got: {err}");
        assert!(!rt.join("qa.control").exists(), "refused control must not create .control lock");

        // Matching generation 2000 acquires and releases cleanly.
        src.set_session_started(Some(2000));
        src.set_controlled_in(&rt, true).expect("matching session must take control");
        assert!(rt.join("qa.control").exists());
        src.release_control_and_wait(Vec::new()).expect("release control");
        assert!(!rt.join("qa.control").exists());

        let _ = fake_session.kill();
        let _ = fake_session.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agy_r1_f2_local_stop_if_generation_routes_to_stop_if_expected() {
        // An invalid screen name is rejected inside `screens::stop_if_expected`
        // before touching any runtime directory; if `stop_if_generation` fell
        // through to the trait default, it would instead return
        // "this screen backend doesn't support guarded stop".
        let mut info = screen_info_for("bad/name", None, None, false, None);
        info.id = ScreenId::local("bad/name");
        let src = LocalScreenSource::new(info);
        let dyn_src: &dyn ScreenSource = &src;
        let err = dyn_src.stop_if_generation(Some(1_234)).unwrap_err();
        assert!(!err.contains("doesn't support guarded stop"), "must override stop_if_generation, got: {err}");
        assert!(err.contains("can't name a screen"), "expected screens::stop_if_expected name validation, got: {err}");
    }
}
