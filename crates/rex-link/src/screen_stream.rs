//! `olo-rex-link screen stream NAME`: live tile-delta streaming and input/control
//! forwarding for one Agent Screen on this host (§3.1).
//!
//! Architecture (zero polling at rest, input decoupled from capture):
//! - **Paint follower thread:** connects to `olo_socket` (verifying `SO_PEERCRED`
//!   belongs to the pinned screen session), subscribes to `EventKind::Painted`
//!   with a bounded handshake deadline, and wakes capture only when the screen
//!   paints something new. At rest: 0 captures, 0 CPU, 0 bytes/s. If the host
//!   `app_server` lacks `subscribe_to painted`, emits a clear `Status` asking
//!   to update Olo on the host and stops (never polls).
//! - **Capture & encoder thread:** captures RGBA via `Request::Screenshot` into
//!   a private `0700` per-stream directory (removed wholesale on exit, including
//!   any `.rgba.part`), validates the 8-byte header and exact file size before
//!   reading pixels, diffs 64×64 tiles via `olo-screen-codec`
//!   (`TileEncoding::PackedStream`, zstd level 1), and paces with
//!   `MAX_IN_FLIGHT = 2` (accepting ACKs only for `seq_acked < seq <= seq_sent`)
//!   and 50 ms controlling / 100 ms watching intervals across both empty-delta
//!   captures and completed writes (keeping the first paint after any quiet
//!   stretch immediate). Re-arms across in-place `app_server` reloads of the
//!   SAME pinned `olo-session` PID up to 5 s; ends immediately with `ScreenGone`
//!   when the original session exits.
//! - **Client stdin reader thread:** polls `[stdin, stop_rd]` independently of
//!   capture, enforcing per-chunk header/size bounds and passing every complete
//!   buffered message (including right before EOF) through one ordered
//!   dispatcher into the bounded input queue.
//! - **Input writer thread:** owns `NAME.control` (`ControlGuard`), so taking
//!   and releasing control are strictly serialized with all queued input events
//!   and held-input releases (`key`, `button`, `keysym`, `touch`). Only accepts
//!   explicit `{"type":"ok"}` replies for committing releases, never retries a
//!   non-idempotent batch after write, coalesces consecutive `move` events, and
//!   marks capture dirty on applied pointer moves so cursor-only motion updates
//!   even over static surfaces.
//! - **Signal watcher thread:** blocks `SIGTERM`/`SIGINT`/`SIGHUP` into a
//!   `signalfd`; `trigger_stop()` writes a broadcast byte to `stop_wr` (never
//!   drained) so blocked non-blocking stdout writes abort immediately and full
//!   teardown (input releases, `NAME.control` removal, temp dir cleanup) runs.

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read as _, Seek as _, SeekFrom, Write as _};
use std::net::Shutdown;
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use olo_rex_link::screen_wire::{ClientMsg, MAX_CLIENT_PAYLOAD, MAX_DIM, MAX_FRAME_PIXELS, ServerMsg, StreamStatus, decode_client_msg, encode_server_msg, validate_client_header};
use olo_screen_codec::{Frame, TileEncoding, tiles::encode_delta};
use serde_json::{Value, json};

use crate::screens::{
    CancelFd, OwnedFd, RuntimeScreen, TrustedTempDir, connect_unix_nonblocking, disk_scratch_base, find_running_screen_pinned_checked, host_cli_honors_helper_control,
    is_controller_pid_alive_with_ticks, open_regular_nonblock, open_stable_control_lock, parse_control_lock_meta, pidfd_exited, proc_start_ticks, read_bounded_regular, read_runtime_screen,
    screens_runtime_dir_candidates_checked, set_nonblocking, valid_name, verify_owned_dir, write_all_cancellable,
};

/// Maximum unacknowledged frames in flight before capture waits for an `Ack`.
const MAX_IN_FLIGHT: u64 = 2;
/// Minimum interval between successive captures/sends while passively watching (10 fps).
const INTERVAL_WATCHING: Duration = Duration::from_millis(100);
/// Minimum interval between successive captures/sends while controlling (20 fps).
const INTERVAL_CONTROLLING: Duration = Duration::from_millis(50);
/// How long to wait for `olo_socket` to return during an in-place reload of the same session.
const RECONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const RECONNECT_STEP: Duration = Duration::from_millis(50);
/// Short retry backoff when reading/decoding the screenshot file fails transiently.
const READ_RETRY_DELAY: Duration = Duration::from_millis(25);
const MAX_READ_RETRIES: u32 = 3;
/// Maximum line length accepted from `app_server` IPC replies or events.
const MAX_IPC_LINE: usize = 64 * 1024;
/// Maximum events retained in the bounded input queue (including releases, Finding 13).
const MAX_QUEUED_INPUT_EVENTS: usize = 512;
/// Maximum serialized event bytes retained in the bounded input queue (Finding 13).
const MAX_QUEUED_INPUT_BYTES: usize = 512 * 1024;
/// Maximum pending control commands queued before overflow revocation (Finding 13).
const MAX_QUEUED_CONTROL_CMDS: usize = 32;
/// Maximum synthetic events sent to `app_server` in a single IPC request.
const MAX_IPC_BATCH_EVENTS: usize = 64;
/// Maximum simultaneously held inputs tracked per category (Finding 13).
const MAX_HELD_KEYS: usize = 256;
const MAX_HELD_BUTTONS: usize = 64;
const MAX_HELD_KEYSYMS: usize = 256;
const MAX_HELD_TOUCHES: usize = 64;

fn handshake_timeout() -> Duration {
    std::env::var("OLO_SCREEN_HANDSHAKE_MS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|&ms| ms > 0).map_or(RECONNECT_TIMEOUT, Duration::from_millis)
}

/// Exclusive control lock holding `LOCK_EX` on both the stable
/// `NAME.control.lock` (serializing against other viewers, in-flight agent CLI
/// `ActionLease` `LOCK_SH`, and `screens::stop`) and `NAME.control` (compatible
/// with legacy viewers), owned by the input writer thread (Findings 5, 8, 10, 36).
struct ControlGuard {
    marker_path: PathBuf,
    stable_path: PathBuf,
    held: Option<(std::fs::File, std::fs::File, (u64, u64))>,
}

impl ControlGuard {
    fn new(rt_dir: &Path, name: &str) -> Self {
        Self { marker_path: rt_dir.join(format!("{name}.control")), stable_path: rt_dir.join(format!("{name}.control.lock")), held: None }
    }

    /// Take control ONLY if host `olo-screen` honors helper control leases and
    /// `NAME.control.lock` + `NAME.control` are free (unlocked and not held by
    /// a live legacy non-flock viewer).
    fn take(&mut self) -> Result<(), String> {
        if self.held.is_some() {
            return Ok(());
        }
        host_cli_honors_helper_control()?;
        let Some(parent) = self.marker_path.parent() else {
            return Err("Invalid screen runtime directory.".into());
        };
        if std::fs::symlink_metadata(parent).is_err() {
            let _ = std::fs::create_dir_all(parent);
        }
        if !verify_owned_dir(parent) {
            return Err("Refusing unsafe screen runtime directory.".into());
        }
        // SAFETY: `geteuid()` has no preconditions.
        let uid = unsafe { libc::geteuid() };
        let my_pid = std::process::id() as i32;
        let my_ticks = proc_start_ticks(my_pid);

        let mut stable = open_stable_control_lock(&self.stable_path).map_err(|e| format!("couldn't open control lock: {e}"))?;
        // SAFETY: `stable` is a valid owned file descriptor.
        if unsafe { libc::flock(stable.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("Someone else is controlling or acting on this screen.".into());
        }

        let stable_existing = read_bounded_regular(&self.stable_path, 64).and_then(|b| parse_control_lock_meta(&b));

        for _ in 0..4 {
            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
                .open(&self.marker_path)
                .map_err(|e| format!("couldn't open control marker: {e}"))?;
            let fm = file.metadata().map_err(|e| format!("stat control marker: {e}"))?;
            if !fm.file_type().is_file() || fm.uid() != uid {
                unsafe { libc::flock(stable.as_raw_fd(), libc::LOCK_UN) };
                return Err("Refusing unsafe control marker.".into());
            }
            // SAFETY: `file` is a valid owned file descriptor.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                unsafe { libc::flock(stable.as_raw_fd(), libc::LOCK_UN) };
                return Err("Someone else is controlling this screen.".into());
            }
            let same_inode = std::fs::symlink_metadata(&self.marker_path).is_ok_and(|pm| pm.file_type().is_file() && fm.dev() == pm.dev() && fm.ino() == pm.ino());
            if !same_inode {
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
                continue;
            }
            // Guard against a live legacy non-flocking viewer BEFORE truncating,
            // verifying start ticks from `.control.lock` when available (Finding 8).
            if fm.len() > 0 && fm.len() <= 64 {
                let mut existing = Vec::new();
                if file.seek(SeekFrom::Start(0)).is_ok()
                    && std::io::Read::by_ref(&mut file).take(65).read_to_end(&mut existing).is_ok()
                    && let Some((other_pid, inline_ticks)) = parse_control_lock_meta(&existing)
                    && other_pid != my_pid
                {
                    let expected_ticks = inline_ticks.or_else(|| stable_existing.filter(|(p, _)| *p == other_pid).and_then(|(_, t)| t));
                    if is_controller_pid_alive_with_ticks(other_pid, expected_ticks) {
                        unsafe {
                            libc::flock(file.as_raw_fd(), libc::LOCK_UN);
                            libc::flock(stable.as_raw_fd(), libc::LOCK_UN);
                        }
                        return Err("Someone else is controlling this screen.".into());
                    }
                }
            }
            let _ = stable.set_len(0);
            let _ = stable.seek(SeekFrom::Start(0));
            if let Some(t) = my_ticks {
                let _ = write!(stable, "{my_pid} {t}");
            } else {
                let _ = write!(stable, "{my_pid}");
            }
            let _ = stable.flush();

            if file.set_len(0).is_err() || file.seek(SeekFrom::Start(0)).is_err() || write!(file, "{my_pid}").is_err() || file.flush().is_err() {
                if std::fs::symlink_metadata(&self.marker_path).is_ok_and(|pm| fm.dev() == pm.dev() && fm.ino() == pm.ino()) {
                    let _ = std::fs::remove_file(&self.marker_path);
                }
                let _ = stable.set_len(0);
                unsafe {
                    libc::flock(file.as_raw_fd(), libc::LOCK_UN);
                    libc::flock(stable.as_raw_fd(), libc::LOCK_UN);
                }
                return Err("Couldn't publish control lock.".into());
            }
            self.held = Some((stable, file, (fm.dev(), fm.ino())));
            return Ok(());
        }
        unsafe { libc::flock(stable.as_raw_fd(), libc::LOCK_UN) };
        Err("Someone else is controlling this screen.".into())
    }

    fn release(&mut self) {
        if let Some((stable, marker, marker_id)) = self.held.take() {
            // Unlink `NAME.control` ONLY while holding BOTH exclusive locks AND
            // verifying `(dev, ino)` still matches our marker inode (Finding 5).
            if let Ok(pm) = std::fs::symlink_metadata(&self.marker_path)
                && pm.file_type().is_file()
                && (pm.dev(), pm.ino()) == marker_id
            {
                let _ = std::fs::remove_file(&self.marker_path);
            }
            let _ = stable.set_len(0);
            // SAFETY: explicitly unlock before dropping so a concurrent fork
            // cannot briefly keep either lock held.
            unsafe {
                libc::flock(marker.as_raw_fd(), libc::LOCK_UN);
            }
            drop(marker);
            unsafe {
                libc::flock(stable.as_raw_fd(), libc::LOCK_UN);
            }
            drop(stable);
        }
    }
}

impl Drop for ControlGuard {
    fn drop(&mut self) {
        self.release();
    }
}

/// Optional microsecond-precision waterfall timings (recorded only when
/// `OLO_SCREEN_STREAM_STATS=1`, zero cost in production).
#[derive(Default, Clone, Copy)]
struct StatsTrace {
    input_stdin_us: u64,
    input_req_start_us: u64,
    input_req_done_us: u64,
    first_painted_us: u64,
}

enum InputCmd {
    TakeControl { seq: u64 },
    ReleaseControl { seq: u64 },
    RevokeControl { reason: String, ack: Option<Arc<(Mutex<bool>, Condvar)>> },
    Events { events: Vec<Value>, bytes: usize },
}

/// Bounded input command queue: enforces strict event-count (including
/// releases), byte-size, and control-command limits (Finding 13). On overflow,
/// drops pending input and enqueues an explicit control revocation so the
/// worker unwinds its bounded held set and drops `.control`.
struct InputQueue {
    cmds: VecDeque<InputCmd>,
    queued_events: usize,
    queued_bytes: usize,
    queued_controls: usize,
    closed: bool,
}

impl InputQueue {
    fn new() -> Self {
        Self { cmds: VecDeque::new(), queued_events: 0, queued_bytes: 0, queued_controls: 0, closed: false }
    }

    fn trigger_overflow_revoke(&mut self, reason: &str) {
        // Preserve any pending synchronous RevokeControl acks while discarding
        // backlogged input batches and TakeControl requests.
        let mut kept = VecDeque::new();
        for cmd in self.cmds.drain(..) {
            if let InputCmd::RevokeControl { ack: Some(_), .. } = cmd {
                kept.push_back(cmd);
            }
        }
        self.cmds = kept;
        self.queued_events = 0;
        self.queued_bytes = 0;
        self.queued_controls = self.cmds.len();
        self.cmds.push_back(InputCmd::RevokeControl { reason: reason.to_owned(), ack: None });
        self.queued_controls += 1;
    }

    fn push_control(&mut self, cmd: InputCmd) {
        if self.closed {
            if let InputCmd::RevokeControl { ack: Some(ref a), .. } = cmd {
                *a.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
                a.1.notify_all();
            }
            return;
        }
        if matches!(cmd, InputCmd::RevokeControl { .. }) {
            // Host revocations (e.g. compositor reconnect) always take effect;
            // drop any pending TakeControl / Events ahead of them if saturated.
            self.cmds.push_back(cmd);
            self.queued_controls += 1;
            return;
        }
        if self.queued_controls >= MAX_QUEUED_CONTROL_CMDS {
            self.trigger_overflow_revoke("Too many pending control commands; control released.");
            return;
        }
        self.queued_controls += 1;
        self.cmds.push_back(cmd);
    }

    fn push_events(&mut self, mut events: Vec<Value>, raw_bytes: usize) {
        if self.closed || events.is_empty() {
            return;
        }
        coalesce_moves(&mut events);
        let bytes = raw_bytes.max(events.len() * 16);
        // Finding 13: ALL events (including releases!) count toward both event
        // and byte budgets; overflow terminates the lease and unwinds held state.
        if self.queued_events.saturating_add(events.len()) > MAX_QUEUED_INPUT_EVENTS || self.queued_bytes.saturating_add(bytes) > MAX_QUEUED_INPUT_BYTES {
            self.trigger_overflow_revoke("Input queue overflow; control released.");
            return;
        }
        self.queued_events += events.len();
        self.queued_bytes += bytes;
        self.cmds.push_back(InputCmd::Events { events, bytes });
    }
}

struct CaptureState {
    dirty: bool,
    need_keyframe: bool,
    paint_ready: bool,
    paused: bool,
    controlling: bool,
    stop: bool,
    seq_sent: u64,
    seq_acked: u64,
    paint_gen: u64,
    paint_broken_gen: u64,
}

struct Shared {
    state: Mutex<CaptureState>,
    cvar: Condvar,
    stop_flag: AtomicBool,
    stdout_lock: Mutex<()>,
    stop_rd: OwnedFd,
    stop_pipe_wr: Mutex<Option<OwnedFd>>,
    socket_path: Mutex<PathBuf>,
    paint_closer: Mutex<Option<UnixStream>>,
    input_q: Mutex<InputQueue>,
    input_cvar: Condvar,
    session_pid: i32,
    screen_name: String,
    stats_on: bool,
    t0: Instant,
    trace: Mutex<StatsTrace>,
}

impl Shared {
    fn is_stopped(&self) -> bool {
        self.stop_flag.load(Ordering::Relaxed)
    }

    fn us_now(&self) -> u64 {
        self.t0.elapsed().as_micros() as u64
    }

    fn push_control_cmd(&self, cmd: InputCmd) {
        let mut q = self.input_q.lock().unwrap_or_else(|e| e.into_inner());
        q.push_control(cmd);
        self.input_cvar.notify_one();
    }

    fn push_input_events(&self, events: Vec<Value>, raw_bytes: usize) {
        let mut q = self.input_q.lock().unwrap_or_else(|e| e.into_inner());
        q.push_events(events, raw_bytes);
        self.input_cvar.notify_one();
    }

    /// Ask `screen-input` to revoke control, drop `.control`, and emit
    /// `ServerMsg::Control { held: false, .. }` synchronously before continuing
    /// (used on compositor reconnect so `.control` is never left held while the
    /// pane clears controlling — Finding 7).
    fn revoke_control_sync(&self, reason: &str) {
        let ack = Arc::new((Mutex::new(false), Condvar::new()));
        {
            let mut q = self.input_q.lock().unwrap_or_else(|e| e.into_inner());
            if q.closed {
                return;
            }
            q.push_control(InputCmd::RevokeControl { reason: reason.to_owned(), ack: Some(ack.clone()) });
            self.input_cvar.notify_one();
        }
        let mut done = ack.0.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = Instant::now() + Duration::from_secs(3);
        while !*done && !self.is_stopped() {
            let Some(rem) = deadline.checked_duration_since(Instant::now()) else { break };
            let (next, _) = ack.1.wait_timeout(done, rem).unwrap_or_else(|e| e.into_inner());
            done = next;
        }
    }

    fn close_input(&self) {
        let mut q = self.input_q.lock().unwrap_or_else(|e| e.into_inner());
        q.closed = true;
        self.input_cvar.notify_all();
    }

    /// Broadcast stop to all threads. Writing one byte to `stop_pipe_wr`
    /// without ever draining `stop_rd` wakes `screen-stdin` and any blocked
    /// `write_all_cancellable` on stdout immediately and permanently.
    fn trigger_stop(&self) {
        let already = self.stop_flag.swap(true, Ordering::SeqCst);
        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.stop = true;
            s.controlling = false;
        }
        self.cvar.notify_all();
        if let Some(sock) = self.paint_closer.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = sock.shutdown(Shutdown::Both);
        }
        if !already && let Some(wr) = self.stop_pipe_wr.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let b = [1u8];
            // SAFETY: writing one broadcast byte to our owned non-blocking wake pipe.
            unsafe {
                libc::write(wr.raw(), b.as_ptr().cast(), 1);
            }
        }
    }

    /// Encode and write one binary [`ServerMsg`] to non-blocking stdout,
    /// aborting immediately on `trigger_stop()`. Returns `InvalidData` if the
    /// message exceeds wire limits so callers can report a typed size error.
    fn send_server_res(&self, msg: &ServerMsg) -> io::Result<()> {
        let (tag, payload) = encode_server_msg(msg)?;
        let mut frame = Vec::with_capacity(5 + payload.len());
        frame.push(tag);
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(&payload);
        let _guard = self.stdout_lock.lock().unwrap_or_else(|e| e.into_inner());
        write_all_cancellable(1, &frame, &[CancelFd::Readable(self.stop_rd.raw())])
    }

    fn send_server(&self, msg: &ServerMsg) -> bool {
        self.send_server_res(msg).is_ok()
    }
}

/// Read one `\n`-terminated line up to `max_bytes`, failing with `InvalidData`
/// without allocating unbounded memory if the peer sends an oversized line.
fn read_bounded_line<R: BufRead>(r: &mut R, out: &mut String, max_bytes: usize) -> io::Result<usize> {
    out.clear();
    let mut raw = Vec::new();
    loop {
        let avail = r.fill_buf()?;
        if avail.is_empty() {
            if raw.is_empty() {
                return Ok(0);
            }
            break;
        }
        if let Some(pos) = memchr::memchr(b'\n', avail) {
            if raw.len() + pos + 1 > max_bytes {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("IPC line exceeds {max_bytes} bytes")));
            }
            raw.extend_from_slice(&avail[..=pos]);
            r.consume(pos + 1);
            break;
        }
        let n = avail.len();
        if raw.len() + n > max_bytes {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("IPC line exceeds {max_bytes} bytes")));
        }
        raw.extend_from_slice(avail);
        r.consume(n);
    }
    let text = String::from_utf8(raw).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let len = text.len();
    *out = text;
    Ok(len)
}

/// Parent PID of `pid` from `/proc/{pid}/stat`.
fn ppid_of(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
}

/// Verify via `SO_PEERCRED` that the connected Unix socket belongs to our user
/// AND to `session_pid`'s process tree (and matching `OLO_SCREEN` if set), so a
/// screen descriptor pointing at another screen's socket can never steal input
/// or capture.
fn verify_socket_peer(stream: &UnixStream, session_pid: i32, screen_name: &str) -> io::Result<()> {
    let mut cred = libc::ucred { pid: 0, uid: u32::MAX, gid: u32::MAX };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: valid owned UnixStream fd and `ucred` output buffer.
    let rc = unsafe { libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&raw mut cred).cast(), &raw mut len) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `geteuid()` has no preconditions.
    let my_uid = unsafe { libc::geteuid() };
    if cred.uid != my_uid || cred.pid <= 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "socket peer belongs to another user"));
    }
    if let Ok(environ) = std::fs::read(format!("/proc/{}/environ", cred.pid))
        && let Some(val) = environ.split(|b| *b == 0).find_map(|v| v.strip_prefix(b"OLO_SCREEN="))
        && val != screen_name.as_bytes()
    {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "socket peer belongs to a different screen"));
    }
    let mut cur = cred.pid;
    for _ in 0..32 {
        if cur == session_pid {
            return Ok(());
        }
        if cur <= 1 {
            break;
        }
        let Some(p) = ppid_of(cur) else { break };
        if p == cur {
            break;
        }
        cur = p;
    }
    Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("socket peer pid {} does not belong to screen session {}", cred.pid, session_pid)))
}

/// Connect to a screen's `app_server` Unix socket with a non-blocking connect
/// deadline, optional `stop_rd` interruption, and `SO_PEERCRED` verification (Finding 25).
fn connect_verified_socket_cancellable(path: &Path, session_pid: i32, screen_name: &str, timeout: Duration, stop_rd: Option<i32>) -> io::Result<UnixStream> {
    let stream = connect_unix_nonblocking(path, timeout, stop_rd)?;
    verify_socket_peer(&stream, session_pid, screen_name)?;
    Ok(stream)
}

#[cfg(test)]
fn connect_verified_socket(path: &Path, session_pid: i32, screen_name: &str, timeout: Duration) -> io::Result<UnixStream> {
    connect_verified_socket_cancellable(path, session_pid, screen_name, timeout, None)
}

/// Persistent line-oriented JSON connection to a screen's `app_server` Unix socket.
struct IpcConn {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    line: String,
}

enum RequestErr {
    /// Failed before the server received our request (safe to reconnect once).
    WriteFailed,
    /// Request was written, but reading/parsing the reply failed (do NOT retry
    /// non-idempotent input!).
    ReplyFailed,
}

impl IpcConn {
    fn connect_verified_cancellable(path: &Path, session_pid: i32, screen_name: &str, timeout: Duration, stop_rd: Option<i32>) -> io::Result<Self> {
        let stream = connect_verified_socket_cancellable(path, session_pid, screen_name, timeout, stop_rd)?;
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Self { writer: stream, reader, line: String::new() })
    }

    fn set_timeout(&mut self, timeout: Duration) {
        let _ = self.writer.set_read_timeout(Some(timeout));
        let _ = self.writer.set_write_timeout(Some(timeout));
        let _ = self.reader.get_ref().set_read_timeout(Some(timeout));
    }

    fn request(&mut self, payload: &[u8]) -> Result<Value, RequestErr> {
        self.writer.write_all(payload).and_then(|()| self.writer.write_all(b"\n")).map_err(|_| RequestErr::WriteFailed)?;
        loop {
            let n = read_bounded_line(&mut self.reader, &mut self.line, MAX_IPC_LINE).map_err(|_| RequestErr::ReplyFailed)?;
            if n == 0 {
                return Err(RequestErr::ReplyFailed);
            }
            let val: Value = serde_json::from_str(self.line.trim()).map_err(|_| RequestErr::ReplyFailed)?;
            if val["type"].as_str() == Some("event") {
                continue;
            }
            return Ok(val);
        }
    }
}

/// Spawn the paint follower thread for `socket_path` at generation `epoch`.
fn spawn_paint_follower(shared: Arc<Shared>, socket_path: PathBuf, epoch: u64) {
    let shared_err = shared.clone();
    let res = std::thread::Builder::new().name("screen-paints".into()).spawn(move || {
        let hs_timeout = handshake_timeout();
        let stream = match connect_verified_socket_cancellable(&socket_path, shared.session_pid, &shared.screen_name, hs_timeout, Some(shared.stop_rd.raw())) {
            Ok(s) => s,
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                let _ = shared.send_server(&ServerMsg::Status(StreamStatus { message: format!("Refusing untrusted screen socket: {e}"), reconnecting: false, error: true }));
                shared.trigger_stop();
                return;
            }
            Err(_) => {
                mark_paint_broken(&shared, epoch);
                return;
            }
        };
        let Ok(closer) = stream.try_clone() else {
            mark_paint_broken(&shared, epoch);
            return;
        };
        {
            let mut slot = shared.paint_closer.lock().unwrap_or_else(|e| e.into_inner());
            if shared.is_stopped() {
                let _ = closer.shutdown(Shutdown::Both);
                return;
            }
            if let Some(old) = slot.replace(closer) {
                let _ = old.shutdown(Shutdown::Both);
            }
        }
        let mut writer = stream;
        let Ok(reader_stream) = writer.try_clone() else {
            mark_paint_broken(&shared, epoch);
            return;
        };
        if writer.write_all(b"{\"type\":\"subscribe_to\",\"events\":[\"painted\"]}\n").is_err() {
            mark_paint_broken(&shared, epoch);
            return;
        }
        let mut reader = BufReader::new(reader_stream);
        let mut line = String::new();
        match read_bounded_line(&mut reader, &mut line, MAX_IPC_LINE) {
            Ok(0) | Err(_) => {
                mark_paint_broken(&shared, epoch);
                return;
            }
            Ok(_) => {}
        }
        let subscribed = serde_json::from_str::<Value>(line.trim()).ok().is_some_and(|v| v["type"].as_str() == Some("ok"));
        if !subscribed {
            // Never poll an app_server that lacks `subscribe_to painted`:
            // tell the desktop pane cleanly and end the stream.
            let _ = shared.send_server(&ServerMsg::Status(StreamStatus { message: "Update Olo on this host to watch its screens live.".into(), reconnecting: false, error: true }));
            shared.trigger_stop();
            return;
        }
        // Handshake succeeded: clear read timeout so the idle paint follower
        // sleeps indefinitely with zero wakeups.
        let _ = reader.get_ref().set_read_timeout(None);

        // Subscription confirmed: trigger the initial keyframe capture.
        {
            let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.paint_gen != epoch {
                return;
            }
            s.paint_ready = true;
            s.dirty = true;
            s.need_keyframe = true;
            shared.cvar.notify_all();
        }

        loop {
            match read_bounded_line(&mut reader, &mut line, MAX_IPC_LINE) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if shared.is_stopped() {
                        return;
                    }
                    if is_painted_event(line.as_bytes()) {
                        let painted_us = if shared.stats_on { shared.us_now() } else { 0 };
                        let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                        if s.paint_gen != epoch {
                            return;
                        }
                        if shared.stats_on && !s.dirty {
                            shared.trace.lock().unwrap_or_else(|e| e.into_inner()).first_painted_us = painted_us;
                        }
                        s.dirty = true;
                        shared.cvar.notify_all();
                    }
                }
            }
        }
        mark_paint_broken(&shared, epoch);
    });
    if res.is_err() {
        mark_paint_broken(&shared_err, epoch);
    }
}

fn mark_paint_broken(shared: &Shared, epoch: u64) {
    if shared.is_stopped() {
        return;
    }
    let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    if s.paint_gen == epoch {
        s.paint_broken_gen = epoch;
        shared.cvar.notify_all();
    }
}

fn is_painted_event(line: &[u8]) -> bool {
    if memchr::memmem::find(line, b"\"painted\"").is_none() {
        return false;
    }
    serde_json::from_slice::<Value>(line).ok().is_some_and(|v| v["type"].as_str() == Some("event") && v["event"]["type"].as_str() == Some("painted"))
}

/// Validate a synthetic input event against the exact set of `SyntheticEvent`
/// variants that this helper supports and can unwind cleanly on disconnect.
fn is_valid_synthetic_event(ev: &Value) -> bool {
    let finite_f64 = |k: &str| ev.get(k).and_then(Value::as_f64).is_some_and(f64::is_finite);
    let u32_field = |k: &str| ev.get(k).and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()).is_some();
    let i32_field = |k: &str| ev.get(k).and_then(Value::as_i64).and_then(|v| i32::try_from(v).ok()).is_some();
    match ev.get("type").and_then(Value::as_str) {
        Some("move") => finite_f64("x") && finite_f64("y"),
        Some("move_by") | Some("scroll") => finite_f64("dx") && finite_f64("dy"),
        // Finding 48: accept discrete mouse-wheel notches (`SyntheticEvent::Wheel`).
        Some("wheel") => i32_field("dx") && i32_field("dy"),
        Some("key") => u32_field("key") && ev.get("pressed").and_then(Value::as_bool).is_some(),
        Some("button") => u32_field("button") && ev.get("pressed").and_then(Value::as_bool).is_some(),
        Some("keysym") => u32_field("keysym") && ev.get("pressed").and_then(Value::as_bool).is_some(),
        Some("touch") => {
            let phase_ok = matches!(ev.get("phase").and_then(Value::as_str), Some("down" | "motion" | "up"));
            let slot_ok = ev.get("slot").is_none_or(|v| v.as_u64().and_then(|n| u32::try_from(n).ok()).is_some());
            let xy_ok = ev.get("x").is_none_or(|v| v.as_f64().is_some_and(f64::is_finite)) && ev.get("y").is_none_or(|v| v.as_f64().is_some_and(f64::is_finite));
            phase_ok && slot_ok && xy_ok
        }
        _ => false,
    }
}

fn has_pointer_motion(events: &[Value]) -> bool {
    events.iter().any(|ev| matches!(ev.get("type").and_then(Value::as_str), Some("move" | "move_by" | "touch")))
}

/// Coalesce consecutive `{"type":"move", ...}` synthetic events to the last one
/// in each run (matching `crates/screen-pane/src/local.rs::run_together`).
fn coalesce_moves(events: &mut Vec<Value>) {
    let mut out: Vec<Value> = Vec::with_capacity(events.len());
    for ev in events.drain(..) {
        let is_move = ev.get("type").and_then(Value::as_str) == Some("move");
        if is_move && out.last().and_then(|prev| prev.get("type")).and_then(Value::as_str) == Some("move") {
            *out.last_mut().unwrap() = ev;
        } else {
            out.push(ev);
        }
    }
    *events = out;
}

/// Tracks every key, mouse button, keysym, and touch slot currently held down
/// through this stream. Presses are recorded pessimistically before sending;
/// releases are committed ONLY after `app_server` confirms `{"type":"ok"}`.
#[derive(Clone, Default)]
struct HeldInputs {
    keys: Vec<u32>,
    buttons: Vec<u32>,
    keysyms: Vec<u32>,
    touches: Vec<u32>,
}

impl HeldInputs {
    fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.buttons.is_empty() && self.keysyms.is_empty() && self.touches.is_empty()
    }

    fn clear(&mut self) {
        self.keys.clear();
        self.buttons.clear();
        self.keysyms.clear();
        self.touches.clear();
    }

    /// True if applying `events` in order would exceed any per-category cap
    /// (`MAX_HELD_KEYS`, `MAX_HELD_BUTTONS`, `MAX_HELD_KEYSYMS`, `MAX_HELD_TOUCHES`).
    fn would_exceed_cap(&self, events: &[Value]) -> bool {
        let mut sim = self.clone();
        for ev in events {
            match ev.get("type").and_then(Value::as_str) {
                Some("key") => {
                    if let (Some(k), Some(p)) = (ev.get("key").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()), ev.get("pressed").and_then(Value::as_bool)) {
                        if p {
                            if !sim.keys.contains(&k) {
                                if sim.keys.len() >= MAX_HELD_KEYS {
                                    return true;
                                }
                                sim.keys.push(k);
                            }
                        } else {
                            sim.keys.retain(|&x| x != k);
                        }
                    }
                }
                Some("button") => {
                    if let (Some(b), Some(p)) = (ev.get("button").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()), ev.get("pressed").and_then(Value::as_bool)) {
                        if p {
                            if !sim.buttons.contains(&b) {
                                if sim.buttons.len() >= MAX_HELD_BUTTONS {
                                    return true;
                                }
                                sim.buttons.push(b);
                            }
                        } else {
                            sim.buttons.retain(|&x| x != b);
                        }
                    }
                }
                Some("keysym") => {
                    if let (Some(ks), Some(p)) = (ev.get("keysym").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()), ev.get("pressed").and_then(Value::as_bool)) {
                        if p {
                            if !sim.keysyms.contains(&ks) {
                                if sim.keysyms.len() >= MAX_HELD_KEYSYMS {
                                    return true;
                                }
                                sim.keysyms.push(ks);
                            }
                        } else {
                            sim.keysyms.retain(|&x| x != ks);
                        }
                    }
                }
                Some("touch") => {
                    let slot = ev.get("slot").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()).unwrap_or(0);
                    match ev.get("phase").and_then(Value::as_str) {
                        Some("down") => {
                            if !sim.touches.contains(&slot) {
                                if sim.touches.len() >= MAX_HELD_TOUCHES {
                                    return true;
                                }
                                sim.touches.push(slot);
                            }
                        }
                        Some("up") => sim.touches.retain(|&x| x != slot),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        false
    }

    /// Commit every transition in `events` strictly in wire order (Finding 9):
    /// `[release(K), press(K)]` ends with `K` tracked as held.
    fn apply_batch_in_order(&mut self, events: &[Value]) {
        for ev in events {
            match ev.get("type").and_then(Value::as_str) {
                Some("key") => {
                    if let (Some(k), Some(p)) = (ev.get("key").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()), ev.get("pressed").and_then(Value::as_bool)) {
                        if p {
                            if !self.keys.contains(&k) && self.keys.len() < MAX_HELD_KEYS {
                                self.keys.push(k);
                            }
                        } else {
                            self.keys.retain(|&x| x != k);
                        }
                    }
                }
                Some("button") => {
                    if let (Some(b), Some(p)) = (ev.get("button").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()), ev.get("pressed").and_then(Value::as_bool)) {
                        if p {
                            if !self.buttons.contains(&b) && self.buttons.len() < MAX_HELD_BUTTONS {
                                self.buttons.push(b);
                            }
                        } else {
                            self.buttons.retain(|&x| x != b);
                        }
                    }
                }
                Some("keysym") => {
                    if let (Some(ks), Some(p)) = (ev.get("keysym").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()), ev.get("pressed").and_then(Value::as_bool)) {
                        if p {
                            if !self.keysyms.contains(&ks) && self.keysyms.len() < MAX_HELD_KEYSYMS {
                                self.keysyms.push(ks);
                            }
                        } else {
                            self.keysyms.retain(|&x| x != ks);
                        }
                    }
                }
                Some("touch") => {
                    let slot = ev.get("slot").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()).unwrap_or(0);
                    match ev.get("phase").and_then(Value::as_str) {
                        Some("down") => {
                            if !self.touches.contains(&slot) && self.touches.len() < MAX_HELD_TOUCHES {
                                self.touches.push(slot);
                            }
                        }
                        Some("up") => self.touches.retain(|&x| x != slot),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// On an ambiguous lost reply, retain pre-state union every press in
    /// `events` so no possibly-applied press is forgotten before release.
    fn record_presses_conservative(&mut self, events: &[Value]) {
        for ev in events {
            match ev.get("type").and_then(Value::as_str) {
                Some("key") => {
                    if let (Some(k), Some(true)) = (ev.get("key").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()), ev.get("pressed").and_then(Value::as_bool))
                        && !self.keys.contains(&k)
                        && self.keys.len() < MAX_HELD_KEYS
                    {
                        self.keys.push(k);
                    }
                }
                Some("button") => {
                    if let (Some(b), Some(true)) = (ev.get("button").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()), ev.get("pressed").and_then(Value::as_bool))
                        && !self.buttons.contains(&b)
                        && self.buttons.len() < MAX_HELD_BUTTONS
                    {
                        self.buttons.push(b);
                    }
                }
                Some("keysym") => {
                    if let (Some(ks), Some(true)) = (ev.get("keysym").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()), ev.get("pressed").and_then(Value::as_bool))
                        && !self.keysyms.contains(&ks)
                        && self.keysyms.len() < MAX_HELD_KEYSYMS
                    {
                        self.keysyms.push(ks);
                    }
                }
                Some("touch") if ev.get("phase").and_then(Value::as_str) == Some("down") => {
                    let slot = ev.get("slot").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()).unwrap_or(0);
                    if !self.touches.contains(&slot) && self.touches.len() < MAX_HELD_TOUCHES {
                        self.touches.push(slot);
                    }
                }
                _ => {}
            }
        }
    }

    fn build_releases(&self) -> Vec<Value> {
        let mut releases = Vec::with_capacity(self.keys.len() + self.buttons.len() + self.keysyms.len() + self.touches.len());
        for &k in self.keys.iter().rev() {
            releases.push(json!({ "type": "key", "key": k, "pressed": false }));
        }
        for &b in self.buttons.iter().rev() {
            releases.push(json!({ "type": "button", "button": b, "pressed": false }));
        }
        for &ks in self.keysyms.iter().rev() {
            releases.push(json!({ "type": "keysym", "keysym": ks, "pressed": false }));
        }
        for &slot in self.touches.iter().rev() {
            releases.push(json!({ "type": "touch", "slot": slot, "phase": "up", "x": 0.0, "y": 0.0 }));
        }
        releases
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SyntheticSendOutcome {
    Accepted,
    Rejected,
    WriteFailed,
    ReplyLost,
}

/// Send one synthetic event batch, distinguishing confirmed acceptance,
/// explicit rejection, pre-delivery write error, and ambiguous lost reply.
fn send_synthetic_batch(conn: &mut Option<(PathBuf, IpcConn)>, shared: &Shared, batch: &[Value], timeout: Duration, retry_after_reply_err: bool) -> SyntheticSendOutcome {
    if batch.is_empty() {
        return SyntheticSendOutcome::Accepted;
    }
    let Ok(req_bytes) = serde_json::to_vec(&json!({ "type": "synthetic", "events": batch })) else {
        return SyntheticSendOutcome::Rejected;
    };
    let target = shared.socket_path.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for attempt in 0..2 {
        let had_existing = conn.as_ref().is_some_and(|(p, _)| *p == target);
        if !had_existing {
            // During shutdown unwind (`retry_after_reply_err == true`), pass `None`
            // so final held-input release batches still get their bounded timeout even
            // after `stop_rd` fired.
            let cancel_fd = (!retry_after_reply_err).then_some(shared.stop_rd.raw());
            *conn = IpcConn::connect_verified_cancellable(&target, shared.session_pid, &shared.screen_name, timeout, cancel_fd).ok().map(|c| (target.clone(), c));
            // What this connection presses is its own: the screen's
            // compositor lets go of whatever is still down when the
            // connection ends, however it ends (`Request::OwnInput`).
            // This helper lets go itself at an orderly end, but killed
            // outright it can't, and a key stayed down in a host's
            // screen with nobody to let it go (a code review,
            // 2026-11-27). A compositor that doesn't know the request
            // answers with an error: as it was.
            if let Some((_, fresh)) = conn.as_mut() {
                fresh.set_timeout(timeout);
                if matches!(fresh.request(br#"{"type":"own_input"}"#), Err(RequestErr::WriteFailed | RequestErr::ReplyFailed)) {
                    *conn = None;
                }
            }
        }
        let Some((_, c)) = conn.as_mut() else {
            return SyntheticSendOutcome::WriteFailed;
        };
        c.set_timeout(timeout);
        if shared.stats_on {
            shared.trace.lock().unwrap_or_else(|e| e.into_inner()).input_req_start_us = shared.us_now();
        }
        match c.request(&req_bytes) {
            Ok(v) if v["type"].as_str() == Some("ok") => {
                if shared.stats_on {
                    shared.trace.lock().unwrap_or_else(|e| e.into_inner()).input_req_done_us = shared.us_now();
                }
                return SyntheticSendOutcome::Accepted;
            }
            Ok(_) => {
                return SyntheticSendOutcome::Rejected;
            }
            Err(RequestErr::WriteFailed) => {
                *conn = None;
                if attempt == 0 && had_existing {
                    continue;
                }
                return SyntheticSendOutcome::WriteFailed;
            }
            Err(RequestErr::ReplyFailed) => {
                *conn = None;
                if attempt == 0 && retry_after_reply_err {
                    continue;
                }
                return SyntheticSendOutcome::ReplyLost;
            }
        }
    }
    SyntheticSendOutcome::WriteFailed
}

/// Send release events for all held inputs in bounded chunks, returning `true`
/// if every held input was explicitly confirmed released.
fn release_held_inputs(conn: &mut Option<(PathBuf, IpcConn)>, shared: &Shared, held: &mut HeldInputs, timeout: Duration) -> bool {
    if held.is_empty() {
        return true;
    }
    let releases = held.build_releases();
    let mut all_ok = true;
    for chunk in releases.chunks(MAX_IPC_BATCH_EVENTS) {
        if send_synthetic_batch(conn, shared, chunk, timeout, true) == SyntheticSendOutcome::Accepted {
            held.apply_batch_in_order(chunk);
        } else {
            for ev in chunk {
                if send_synthetic_batch(conn, shared, std::slice::from_ref(ev), timeout, false) == SyntheticSendOutcome::Accepted {
                    held.apply_batch_in_order(std::slice::from_ref(ev));
                } else {
                    all_ok = false;
                }
            }
        }
    }
    // Never carry stale entries across independent control acquisitions (Finding 9).
    if !held.is_empty() {
        held.clear();
        all_ok = false;
    }
    all_ok
}

/// Dedicated input-forwarding thread owning `ControlGuard`: guarantees every
/// held-input release finishes BEFORE `NAME.control` is dropped, splits every
/// batch at `MAX_IPC_BATCH_EVENTS`, and emits authoritative `ServerMsg::Control`
/// replies on every state transition.
fn spawn_input_writer(shared: Arc<Shared>, mut control: ControlGuard) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("screen-input".into())
        .spawn(move || {
            let mut conn: Option<(PathBuf, IpcConn)> = None;
            let mut held = HeldInputs::default();
            let mut controlling = false;

            let do_release = |controlling: &mut bool, conn: &mut Option<(PathBuf, IpcConn)>, held: &mut HeldInputs, control: &mut ControlGuard, seq: u64, reason: Option<String>, notify: bool| {
                let was = *controlling;
                *controlling = false;
                {
                    let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    s.controlling = false;
                    shared.cvar.notify_all();
                }
                let mut release_note = reason;
                if was {
                    let clean = release_held_inputs(conn, &shared, held, Duration::from_secs(2));
                    *conn = None;
                    control.release();
                    if !clean && release_note.is_none() {
                        release_note = Some("Control released; compositor did not confirm all key releases.".into());
                    }
                } else {
                    held.clear();
                    *conn = None;
                }
                if notify {
                    let _ = shared.send_server(&ServerMsg::Control { held: false, seq, reason: release_note });
                }
            };

            loop {
                let cmd = {
                    let mut q = shared.input_q.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        if let Some(c) = q.cmds.pop_front() {
                            match &c {
                                InputCmd::Events { events, bytes } => {
                                    q.queued_events = q.queued_events.saturating_sub(events.len());
                                    q.queued_bytes = q.queued_bytes.saturating_sub(*bytes);
                                    // Finding 13: once EOF/shutdown arrives, do NOT drain
                                    // backlogged input events (unwind held set immediately).
                                    if q.closed {
                                        continue;
                                    }
                                }
                                InputCmd::TakeControl { .. } => {
                                    q.queued_controls = q.queued_controls.saturating_sub(1);
                                    if q.closed {
                                        continue;
                                    }
                                }
                                InputCmd::ReleaseControl { .. } | InputCmd::RevokeControl { .. } => {
                                    q.queued_controls = q.queued_controls.saturating_sub(1);
                                }
                            }
                            break Some(c);
                        }
                        if q.closed {
                            break None;
                        }
                        q = shared.input_cvar.wait(q).unwrap_or_else(|e| e.into_inner());
                    }
                };
                let Some(cmd) = cmd else { break };

                match cmd {
                    InputCmd::TakeControl { seq } => {
                        if controlling {
                            let _ = shared.send_server(&ServerMsg::Control { held: true, seq, reason: None });
                        } else {
                            held.clear();
                            match control.take() {
                                Ok(()) => {
                                    controlling = true;
                                    {
                                        let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                                        s.controlling = true;
                                        shared.cvar.notify_all();
                                    }
                                    let _ = shared.send_server(&ServerMsg::Control { held: true, seq, reason: None });
                                }
                                Err(msg) => {
                                    let _ = shared.send_server(&ServerMsg::Control { held: false, seq, reason: Some(msg) });
                                }
                            }
                        }
                    }
                    InputCmd::ReleaseControl { seq } => {
                        do_release(&mut controlling, &mut conn, &mut held, &mut control, seq, None, true);
                    }
                    InputCmd::RevokeControl { reason, ack } => {
                        let notify = controlling;
                        do_release(&mut controlling, &mut conn, &mut held, &mut control, 0, Some(reason), notify);
                        if let Some(a) = ack {
                            *a.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
                            a.1.notify_all();
                        }
                    }
                    InputCmd::Events { mut events, bytes: _ } => {
                        if !controlling {
                            continue;
                        }
                        // Finding 13: split oversized FIRST batches at MAX_IPC_BATCH_EVENTS too,
                        // and merge consecutive queued batches strictly up to MAX_IPC_BATCH_EVENTS.
                        let mut batch = if events.len() > MAX_IPC_BATCH_EVENTS {
                            let rest = events.split_off(MAX_IPC_BATCH_EVENTS);
                            let rest_bytes = rest.len() * 16;
                            let mut q = shared.input_q.lock().unwrap_or_else(|e| e.into_inner());
                            if !q.closed {
                                q.queued_events += rest.len();
                                q.queued_bytes += rest_bytes;
                                q.cmds.push_front(InputCmd::Events { events: rest, bytes: rest_bytes });
                            }
                            events
                        } else {
                            let mut q = shared.input_q.lock().unwrap_or_else(|e| e.into_inner());
                            while !q.closed && events.len() < MAX_IPC_BATCH_EVENTS && matches!(q.cmds.front(), Some(InputCmd::Events { .. })) {
                                if let Some(InputCmd::Events { events: mut more, bytes: more_bytes }) = q.cmds.pop_front() {
                                    q.queued_events = q.queued_events.saturating_sub(more.len());
                                    q.queued_bytes = q.queued_bytes.saturating_sub(more_bytes);
                                    if events.len() + more.len() <= MAX_IPC_BATCH_EVENTS {
                                        events.extend(more);
                                    } else {
                                        let take = MAX_IPC_BATCH_EVENTS - events.len();
                                        let rest = more.split_off(take);
                                        let rest_bytes = rest.len() * 16;
                                        events.extend(more);
                                        q.queued_events += rest.len();
                                        q.queued_bytes += rest_bytes;
                                        q.cmds.push_front(InputCmd::Events { events: rest, bytes: rest_bytes });
                                        break;
                                    }
                                }
                            }
                            events
                        };
                        coalesce_moves(&mut batch);
                        if held.would_exceed_cap(&batch) {
                            do_release(&mut controlling, &mut conn, &mut held, &mut control, 0, Some("Too many simultaneously held inputs; control released.".into()), true);
                            continue;
                        }
                        let moved_pointer = has_pointer_motion(&batch);
                        match send_synthetic_batch(&mut conn, &shared, &batch, Duration::from_secs(5), false) {
                            SyntheticSendOutcome::Accepted => {
                                held.apply_batch_in_order(&batch);
                                if moved_pointer {
                                    let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                                    s.dirty = true;
                                    shared.cvar.notify_all();
                                }
                            }
                            SyntheticSendOutcome::ReplyLost => {
                                held.record_presses_conservative(&batch);
                            }
                            SyntheticSendOutcome::Rejected | SyntheticSendOutcome::WriteFailed => {}
                        }
                    }
                }
            }

            // Stream ended (stdin EOF or signal): release anything still held
            // down BEFORE dropping `NAME.control` + `NAME.control.lock`.
            if controlling {
                release_held_inputs(&mut conn, &shared, &mut held, Duration::from_secs(1));
                control.release();
            }
        })
        .expect("spawn screen-input")
}

/// Dispatch one decoded client message strictly in arrival order.
fn dispatch_client_msg(shared: &Shared, msg: ClientMsg) {
    match msg {
        ClientMsg::Ack { seq } => {
            let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            // Finding 13: accept credit strictly for `seq_acked < seq <= seq_sent`.
            if seq > s.seq_acked && seq <= s.seq_sent {
                s.seq_acked = seq;
                shared.cvar.notify_all();
            }
        }
        ClientMsg::Pause { paused } => {
            let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.paused != paused {
                s.paused = paused;
                shared.cvar.notify_all();
            }
        }
        ClientMsg::Control { on } => {
            shared.push_control_cmd(if on { InputCmd::TakeControl { seq: 0 } } else { InputCmd::ReleaseControl { seq: 0 } });
        }
        ClientMsg::ControlSeq { on, seq } => {
            shared.push_control_cmd(if on { InputCmd::TakeControl { seq } } else { InputCmd::ReleaseControl { seq } });
        }
        ClientMsg::Input { json_events } => {
            if shared.stats_on {
                shared.trace.lock().unwrap_or_else(|e| e.into_inner()).input_stdin_us = shared.us_now();
            }
            let raw_bytes = json_events.len();
            if let Ok(Value::Array(events)) = serde_json::from_str::<Value>(&json_events) {
                let valid: Vec<Value> = events.into_iter().filter(is_valid_synthetic_event).collect();
                if !valid.is_empty() {
                    shared.push_input_events(valid, raw_bytes);
                }
            }
        }
    }
}

/// Drain all complete messages currently buffered in `buf[pos..]`, enforcing
/// header validation before buffering past a single message limit. Returns
/// `false` on protocol error.
fn drain_buffered_client_msgs(shared: &Shared, buf: &mut Vec<u8>, pos: &mut usize) -> bool {
    while buf.len() - *pos >= 5 {
        let head = &buf[*pos..*pos + 5];
        let tag = head[0];
        let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]);
        if validate_client_header(tag, len).is_err() {
            return false;
        }
        let total = 5 + len as usize;
        if buf.len() - *pos < total {
            break;
        }
        let payload = &buf[*pos + 5..*pos + total];
        let Ok(msg) = decode_client_msg(tag, payload) else {
            return false;
        };
        *pos += total;
        dispatch_client_msg(shared, msg);
    }
    if *pos == buf.len() {
        buf.clear();
        *pos = 0;
    } else if *pos >= 64 * 1024 {
        let rem = buf.len() - *pos;
        buf.copy_within(*pos.., 0);
        buf.truncate(rem);
        *pos = 0;
    }
    buf.len() - *pos <= (MAX_CLIENT_PAYLOAD as usize) + 5
}

/// Non-blocking stdin reader thread multiplexed with `stop_rd`, stdout
/// hangup/error (`fd 1`, Finding 35), and the pinned screen session `pidfd`
/// (Finding 26) in a single `poll`.
fn spawn_stdin_reader(shared: Arc<Shared>, mut session_pidfd: Option<OwnedFd>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("screen-stdin".into())
        .spawn(move || {
            set_nonblocking(0);
            let mut buf: Vec<u8> = Vec::with_capacity(8192);
            let mut pos: usize = 0;
            let mut chunk = [0u8; 8192];

            'outer: loop {
                if !drain_buffered_client_msgs(&shared, &mut buf, &mut pos) {
                    break 'outer;
                }
                if shared.is_stopped() {
                    break;
                }

                let mut pfds = [
                    libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 },
                    libc::pollfd { fd: shared.stop_rd.raw(), events: libc::POLLIN, revents: 0 },
                    // Finding 35: wake immediately if our stdout pipe reader closes while idle.
                    libc::pollfd { fd: 1, events: 0, revents: 0 },
                    // Finding 26: wake immediately when the pinned screen session exits even while paused/idle.
                    libc::pollfd { fd: session_pidfd.as_ref().map_or(-1, OwnedFd::raw), events: libc::POLLIN, revents: 0 },
                ];
                let nfds: libc::nfds_t = if session_pidfd.is_some() { 4 } else { 3 };
                // SAFETY: `pfds[..nfds]` holds valid open file descriptors.
                let n = unsafe { libc::poll(pfds.as_mut_ptr(), nfds, -1) };
                if n < 0 {
                    if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }
                if pfds[1].revents != 0 {
                    break;
                }
                if pfds[2].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                    break;
                }
                if session_pidfd.is_some() && pfds[3].revents != 0 {
                    // Stop polling the exited pidfd and wake `stream_loop` so it
                    // emits `ServerMsg::ScreenGone` and runs clean teardown.
                    session_pidfd = None;
                    let _g = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    shared.cvar.notify_all();
                }
                if pfds[0].revents != 0 {
                    // Read bounded chunks and parse after each chunk so continuous
                    // stdin input can never grow `buf` unboundedly or starve stop.
                    let mut saw_eof = false;
                    for _ in 0..16 {
                        if shared.is_stopped() {
                            break 'outer;
                        }
                        // SAFETY: non-blocking read from stdin into `chunk`.
                        let got = unsafe { libc::read(0, chunk.as_mut_ptr().cast(), chunk.len()) };
                        if got > 0 {
                            buf.extend_from_slice(&chunk[..got as usize]);
                            if !drain_buffered_client_msgs(&shared, &mut buf, &mut pos) {
                                break 'outer;
                            }
                        } else if got == 0 {
                            saw_eof = true;
                            break;
                        } else {
                            let err = io::Error::last_os_error();
                            if err.kind() == io::ErrorKind::WouldBlock || err.kind() == io::ErrorKind::Interrupted {
                                break;
                            }
                            saw_eof = true;
                            break;
                        }
                    }
                    if saw_eof {
                        let _ = drain_buffered_client_msgs(&shared, &mut buf, &mut pos);
                        break;
                    }
                    if pfds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 && buf.len() - pos < 5 {
                        break;
                    }
                }
            }
            // Close input queue so `screen-input` finishes all queued commands
            // (including final Control/Input), releases held inputs, and drops
            // `NAME.control` before exit.
            shared.close_input();
            shared.trigger_stop();
        })
        .expect("spawn screen-stdin")
}

/// Block SIGTERM/SIGINT/SIGHUP on the calling thread (inherited by all spawned
/// threads) and return a blocking `signalfd` to monitor on a dedicated thread.
fn block_signals() -> Option<OwnedFd> {
    // SAFETY: called before any threads are spawned.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGHUP);
        if libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) != 0 {
            return None;
        }
        let fd = libc::signalfd(-1, &set, libc::SFD_CLOEXEC);
        (fd >= 0).then(|| OwnedFd::new(fd))
    }
}

fn spawn_signal_watcher(shared: Arc<Shared>, sfd: OwnedFd) {
    let _ = std::thread::Builder::new().name("screen-signal".into()).spawn(move || {
        let mut info: libc::signalfd_siginfo = unsafe { std::mem::zeroed() };
        loop {
            // SAFETY: blocking read of one `signalfd_siginfo` from our owned `signalfd`.
            let got = unsafe { libc::read(sfd.raw(), (&raw mut info).cast(), std::mem::size_of::<libc::signalfd_siginfo>()) };
            if got > 0 {
                shared.trigger_stop();
                break;
            }
            if got < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
    });
}

/// Look up the runtime descriptor of the SAME original screen session
/// (`session_pid`) without ever switching to a replacement session started
/// later under the same name.
fn find_same_session_screen(name: &str, session_pid: i32, session_pidfd: &OwnedFd) -> Option<RuntimeScreen> {
    if pidfd_exited(session_pidfd) {
        return None;
    }
    let Ok(rt_dirs) = screens_runtime_dir_candidates_checked() else {
        return None;
    };
    for rt in rt_dirs {
        let path = rt.join(format!("{name}.json"));
        if let Some(screen) = read_runtime_screen(&path)
            && screen.name == name
            && screen.pid == session_pid
            && !pidfd_exited(session_pidfd)
        {
            return Some(screen);
        }
    }
    None
}

/// Wait up to `deadline` for the SAME original screen session (`session_pid`)
/// to answer again after an in-place `app_server` reload. Ends immediately if
/// the original session's `pidfd` signals exit.
fn wait_for_reconnect(shared: &Shared, name: &str, session_pid: i32, session_pidfd: &OwnedFd, deadline: Instant) -> Option<RuntimeScreen> {
    loop {
        if shared.is_stopped() || pidfd_exited(session_pidfd) {
            return None;
        }
        let now = Instant::now();
        if now >= deadline {
            return None;
        }
        let rem = deadline - now;
        if let Some(screen) = find_same_session_screen(name, session_pid, session_pidfd)
            && IpcConn::connect_verified_cancellable(&screen.olo_socket, session_pid, name, rem.min(Duration::from_secs(1)), Some(shared.stop_rd.raw())).is_ok()
        {
            return Some(screen);
        }
        let now = Instant::now();
        if now >= deadline {
            return None;
        }
        let wait = RECONNECT_STEP.min(deadline - now);
        let s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.stop {
            return None;
        }
        let _ = shared.cvar.wait_timeout(s, wait);
    }
}

enum RgbaReadError {
    IoOrCorrupt,
    TooLarge(u32, u32),
}

/// Validate the 8-byte `(width, height)` header and exact file length BEFORE
/// allocating or reading RGBA pixels (rejecting sparse, truncated, FIFO, or
/// over-limit files cleanly).
fn read_rgba_frame_bounded(path: &Path) -> Result<Frame, RgbaReadError> {
    let (mut file, meta) = open_regular_nonblock(path).map_err(|_| RgbaReadError::IoOrCorrupt)?;
    let mut head = [0u8; 8];
    file.read_exact(&mut head).map_err(|_| RgbaReadError::IoOrCorrupt)?;
    let w = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
    let h = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
    if w == 0 || h == 0 {
        return Err(RgbaReadError::IoOrCorrupt);
    }
    let Some(pixels) = (w as usize).checked_mul(h as usize) else {
        return Err(RgbaReadError::TooLarge(w, h));
    };
    if w > MAX_DIM || h > MAX_DIM || pixels > MAX_FRAME_PIXELS {
        return Err(RgbaReadError::TooLarge(w, h));
    }
    let Some(pixel_bytes) = pixels.checked_mul(4) else {
        return Err(RgbaReadError::TooLarge(w, h));
    };
    let expected_len = 8u64.saturating_add(pixel_bytes as u64);
    if meta.len() != expected_len {
        return Err(RgbaReadError::IoOrCorrupt);
    }
    let mut rgba = vec![0u8; pixel_bytes];
    file.read_exact(&mut rgba).map_err(|_| RgbaReadError::IoOrCorrupt)?;
    Ok(Frame { width: w as usize, height: h as usize, rgba })
}

pub fn run(args: &[String]) -> ! {
    if args.first().is_none_or(|a| a == "--help" || a == "-h") {
        println!("usage: olo-rex-link screen stream NAME");
        std::process::exit(if args.is_empty() { 2 } else { 0 });
    }
    let name = &args[0];
    if !valid_name(name) {
        let _ = olo_rex_link::screen_wire::write_server_msg(&mut io::stdout().lock(), &ServerMsg::ScreenGone { reason: format!("invalid screen name {name:?}") });
        std::process::exit(2);
    }
    std::process::exit(stream_loop(name))
}

fn stream_loop(name: &str) -> i32 {
    // Block termination signals BEFORE spawning any threads so every thread
    // inherits the mask and `signalfd` catches them reliably.
    let sigfd = block_signals();
    set_nonblocking(1);

    let (rt_dir, initial, session_pidfd) = match find_running_screen_pinned_checked(name) {
        Ok(Some(v)) => v,
        Ok(None) => {
            let _ = olo_rex_link::screen_wire::write_server_msg(&mut io::stdout().lock(), &ServerMsg::ScreenGone { reason: format!("screen {name} is not running") });
            return 0;
        }
        Err(e) => {
            let _ = olo_rex_link::screen_wire::write_server_msg(&mut io::stdout().lock(), &ServerMsg::ScreenGone { reason: format!("unsafe runtime directory: {e}") });
            return 1;
        }
    };

    let mut pipe_fds = [-1i32; 2];
    // SAFETY: creates a close-on-exec, non-blocking broadcast self-pipe.
    if unsafe { libc::pipe2(pipe_fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        eprintln!("olo screen stream: pipe2: {}", io::Error::last_os_error());
        return 1;
    }
    let stop_rd = OwnedFd::new(pipe_fds[0]);
    let stop_wr = OwnedFd::new(pipe_fds[1]);

    // Private exclusively allocated `0700` per-stream capture directory pinned
    // by directory descriptors and `.owner.lock` (Findings 14 & 18).
    let views_base = if verify_owned_dir(&rt_dir) { rt_dir.join("views") } else { disk_scratch_base().join("views") };
    let capture_scratch = match TrustedTempDir::create(&views_base, &format!("stream-{name}"), "stream-") {
        Ok(d) => d,
        Err(e) => {
            let _ = olo_rex_link::screen_wire::write_server_msg(&mut io::stdout().lock(), &ServerMsg::ScreenGone { reason: format!("capture scratch unavailable: {e}") });
            return 1;
        }
    };
    let capture_path = capture_scratch.path().join("frame.rgba");
    let stats_on = std::env::var_os("OLO_SCREEN_STREAM_STATS").is_some_and(|v| v != "0" && !v.is_empty());

    let shared = Arc::new(Shared {
        state: Mutex::new(CaptureState {
            dirty: false,
            need_keyframe: true,
            paint_ready: false,
            paused: false,
            controlling: false,
            stop: false,
            seq_sent: 0,
            seq_acked: 0,
            paint_gen: 1,
            paint_broken_gen: 0,
        }),
        cvar: Condvar::new(),
        stop_flag: AtomicBool::new(false),
        stdout_lock: Mutex::new(()),
        stop_rd,
        stop_pipe_wr: Mutex::new(Some(stop_wr)),
        socket_path: Mutex::new(initial.olo_socket.clone()),
        paint_closer: Mutex::new(None),
        input_q: Mutex::new(InputQueue::new()),
        input_cvar: Condvar::new(),
        session_pid: initial.pid,
        screen_name: name.to_owned(),
        stats_on,
        t0: Instant::now(),
        trace: Mutex::new(StatsTrace::default()),
    });

    if let Some(sfd) = sigfd {
        spawn_signal_watcher(shared.clone(), sfd);
    }

    let input_handle = spawn_input_writer(shared.clone(), ControlGuard::new(&rt_dir, name));
    let stdin_handle = spawn_stdin_reader(shared.clone(), session_pidfd.try_dup());

    let hs_timeout = handshake_timeout();
    let mut reconnect_deadline: Option<Instant> = Some(Instant::now() + hs_timeout);
    spawn_paint_follower(shared.clone(), initial.olo_socket.clone(), 1);

    let mut shot_conn: Option<IpcConn> = IpcConn::connect_verified_cancellable(&initial.olo_socket, initial.pid, name, hs_timeout, Some(shared.stop_rd.raw())).ok();
    let mut screen_dims = (initial.width, initial.height, initial.scale);
    let mut oversize_warned = false;
    let shot_req = serde_json::to_vec(&json!({
        "type": "screenshot",
        "path": capture_path.display().to_string(),
        "border": true,
        "pointer": true,
        "format": "rgba",
        "as_shown": false,
    }))
    .unwrap();

    let mut last_capture_at: Option<Instant> = None;
    let mut last_sent_done_at: Option<Instant> = None;
    let mut prev_frame: Option<Frame> = None;
    let mut current_gen: u64 = 1;
    let mut read_failures: u32 = 0;
    let mut shot_app_errors: u32 = 0;

    loop {
        let mut pacing_wait_us = 0u64;
        let mut capture_wake_us = 0u64;
        let (keyframe, need_reconnect, is_controlling) = {
            let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if s.stop {
                    break (false, false, false);
                }
                if pidfd_exited(&session_pidfd) {
                    break (false, true, s.controlling);
                }
                if s.paint_broken_gen == current_gen {
                    break (false, true, s.controlling);
                }
                if !s.paint_ready
                    && let Some(dl) = reconnect_deadline
                {
                    let now = Instant::now();
                    if now >= dl {
                        break (false, true, s.controlling);
                    }
                    let (next, _) = shared.cvar.wait_timeout(s, (dl - now).min(Duration::from_millis(200))).unwrap_or_else(|e| e.into_inner());
                    s = next;
                    continue;
                }
                let in_flight = s.seq_sent.saturating_sub(s.seq_acked);
                if !s.paint_ready || s.paused || !s.dirty || in_flight >= MAX_IN_FLIGHT {
                    s = shared.cvar.wait(s).unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                if stats_on && capture_wake_us == 0 {
                    capture_wake_us = shared.us_now();
                }
                let min_interval = if s.controlling { INTERVAL_CONTROLLING } else { INTERVAL_WATCHING };
                let last_activity = match (last_capture_at, last_sent_done_at) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
                if let Some(last) = last_activity {
                    let elapsed = last.elapsed();
                    if elapsed < min_interval {
                        let tw = Instant::now();
                        let (next, _) = shared.cvar.wait_timeout(s, min_interval - elapsed).unwrap_or_else(|e| e.into_inner());
                        pacing_wait_us += tw.elapsed().as_micros() as u64;
                        s = next;
                        continue;
                    }
                }
                let kf = s.need_keyframe;
                let ctl = s.controlling;
                s.dirty = false;
                s.need_keyframe = false;
                break (kf, false, ctl);
            }
        };

        if shared.is_stopped() {
            break;
        }

        if need_reconnect {
            drop(shot_conn.take());
            if pidfd_exited(&session_pidfd) {
                let _ = shared.send_server(&ServerMsg::ScreenGone { reason: format!("screen {name} stopped") });
                shared.trigger_stop();
                break;
            }
            let dl = *reconnect_deadline.get_or_insert_with(|| Instant::now() + handshake_timeout());
            if Instant::now() >= dl {
                let _ = shared.send_server(&ServerMsg::ScreenGone { reason: format!("screen {name} stopped answering") });
                shared.trigger_stop();
                break;
            }
            // Finding 7: explicitly revoke control and remove `.control` on disk
            // BEFORE emitting `Reconnecting` so host and pane agree control is off.
            shared.revoke_control_sync("Compositor restarted; control released.");
            if !shared.send_server(&ServerMsg::Status(StreamStatus { message: "Reconnecting…".into(), reconnecting: true, error: false })) {
                shared.trigger_stop();
                break;
            }
            match wait_for_reconnect(&shared, name, initial.pid, &session_pidfd, dl) {
                Some(new_screen) => {
                    *shared.socket_path.lock().unwrap_or_else(|e| e.into_inner()) = new_screen.olo_socket.clone();
                    screen_dims = (new_screen.width, new_screen.height, new_screen.scale);
                    oversize_warned = false;
                    let rem = dl.saturating_duration_since(Instant::now()).max(Duration::from_millis(200));
                    shot_conn = IpcConn::connect_verified_cancellable(&new_screen.olo_socket, initial.pid, name, rem, Some(shared.stop_rd.raw())).ok();
                    current_gen += 1;
                    read_failures = 0;
                    shot_app_errors = 0;
                    {
                        let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                        s.paint_gen = current_gen;
                        s.paint_ready = false;
                        s.dirty = false;
                        s.need_keyframe = true;
                    }
                    spawn_paint_follower(shared.clone(), new_screen.olo_socket, current_gen);
                    continue;
                }
                None => {
                    if !shared.is_stopped() {
                        let _ = shared.send_server(&ServerMsg::ScreenGone { reason: format!("screen {name} stopped") });
                    }
                    shared.trigger_stop();
                    break;
                }
            }
        }

        // Finding 15: check physical screen dimensions BEFORE asking app_server
        // to write an uncompressed RGBA capture file (which can exceed 1 GiB
        // for 7680x4320@3x).
        let phys_w = ((screen_dims.0 as f64) * screen_dims.2.max(0.5)).round() as usize;
        let phys_h = ((screen_dims.1 as f64) * screen_dims.2.max(0.5)).round() as usize;
        if phys_w == 0 || phys_h == 0 || phys_w > MAX_DIM as usize || phys_h > MAX_DIM as usize || phys_w.saturating_mul(phys_h) > MAX_FRAME_PIXELS {
            last_capture_at = Some(Instant::now());
            if !oversize_warned {
                oversize_warned = true;
                let _ = shared.send_server(&ServerMsg::Warning { message: format!("Screen picture {phys_w}×{phys_h} exceeds stream size limits ({MAX_DIM} max side, {MAX_FRAME_PIXELS} pixels).") });
            }
            let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            s.need_keyframe |= keyframe;
            continue;
        }
        oversize_warned = false;

        // 1. Ask app_server for an RGBA screenshot into our private 0700 dir.
        let _ = is_controlling;
        let t_total = Instant::now();
        let capture_begin_us = if stats_on { shared.us_now() } else { 0 };
        let t_shot = Instant::now();
        let socket_now = shared.socket_path.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if shot_conn.is_none() {
            match IpcConn::connect_verified_cancellable(&socket_now, initial.pid, name, Duration::from_secs(5), Some(shared.stop_rd.raw())) {
                Ok(c) => shot_conn = Some(c),
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                    let _ = shared.send_server(&ServerMsg::Status(StreamStatus { message: format!("Refusing untrusted screen socket: {e}"), reconnecting: false, error: true }));
                    shared.trigger_stop();
                    break;
                }
                Err(_) => {}
            }
        }
        enum ShotOutcome {
            Ok,
            AppError(String),
            TransportBroken,
        }
        let outcome = match shot_conn.as_mut() {
            Some(c) => match c.request(&shot_req) {
                Ok(v) if v["type"].as_str() == Some("ok") => ShotOutcome::Ok,
                Ok(v) => {
                    let msg = v["message"].as_str().unwrap_or("screenshot failed").to_owned();
                    ShotOutcome::AppError(msg)
                }
                Err(_) => ShotOutcome::TransportBroken,
            },
            None => ShotOutcome::TransportBroken,
        };
        let shot_us = t_shot.elapsed().as_micros() as u64;

        match outcome {
            ShotOutcome::Ok => {
                shot_app_errors = 0;
                reconnect_deadline = None;
            }
            ShotOutcome::AppError(msg) => {
                // Finding 7: capture warnings always use dedicated `ServerMsg::Warning`
                // so they never drop control, reconnect-loop, or depend on sampled state.
                last_capture_at = Some(Instant::now());
                shot_app_errors += 1;
                if shot_app_errors <= MAX_READ_RETRIES {
                    let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    s.need_keyframe |= keyframe;
                    s.dirty = true;
                    if !s.stop {
                        let _ = shared.cvar.wait_timeout(s, READ_RETRY_DELAY);
                    }
                } else {
                    if shot_app_errors == MAX_READ_RETRIES + 1 {
                        let _ = shared.send_server(&ServerMsg::Warning { message: format!("Couldn't capture the screen: {msg}") });
                    }
                    let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    s.need_keyframe |= keyframe;
                }
                continue;
            }
            ShotOutcome::TransportBroken => {
                shot_conn = None;
                let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                s.need_keyframe |= keyframe;
                s.dirty = true;
                s.paint_broken_gen = current_gen;
                continue;
            }
        }

        // 2. Read the RGBA file back with upfront header & size validation.
        let t_read = Instant::now();
        let read_res = read_rgba_frame_bounded(&capture_path);
        let read_us = t_read.elapsed().as_micros() as u64;
        last_capture_at = Some(Instant::now());
        let frame = match read_res {
            Ok(f) => {
                read_failures = 0;
                f
            }
            Err(RgbaReadError::TooLarge(w, h)) => {
                let _ = shared.send_server(&ServerMsg::Warning { message: format!("Screen picture {w}×{h} exceeds stream size limits.") });
                let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                s.need_keyframe |= keyframe;
                continue;
            }
            Err(RgbaReadError::IoOrCorrupt) => {
                read_failures += 1;
                if read_failures <= MAX_READ_RETRIES {
                    let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    s.need_keyframe |= keyframe;
                    s.dirty = true;
                    if !s.stop {
                        let _ = shared.cvar.wait_timeout(s, READ_RETRY_DELAY);
                    }
                } else {
                    if read_failures == MAX_READ_RETRIES + 1 {
                        let _ = shared.send_server(&ServerMsg::Warning { message: "Couldn't read the screen's picture.".into() });
                    }
                    let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    s.need_keyframe |= keyframe;
                }
                continue;
            }
        };

        // 3. Diff + compress via the single shared `olo-screen-codec` (`PackedStream`, zstd level 1).
        let before = (!keyframe).then_some(prev_frame.as_ref()).flatten();
        let diff_us = if stats_on {
            let td = Instant::now();
            let _ = frame.changed(before);
            td.elapsed().as_micros() as u64
        } else {
            0
        };
        let t_enc = Instant::now();
        let delta = match encode_delta(&frame, before, TileEncoding::PackedStream, 1) {
            Ok(d) => d,
            Err(e) => {
                let _ = shared.send_server(&ServerMsg::Warning { message: format!("Couldn't encode the screen's picture: {e}") });
                let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                s.need_keyframe |= keyframe;
                continue;
            }
        };
        let enc_us = t_enc.elapsed().as_micros() as u64;
        if delta.rects.is_empty() {
            // Finding 22: `last_capture_at` was already recorded above so
            // unchanged-pixel repaints obey pacing instead of spinning at 120 Hz.
            continue;
        }
        let tile_count = delta.rects.len();
        let raw_bytes: usize = delta.rects.iter().map(|r| r.byte_len()).sum();
        let force_key = delta.keyframe;
        let width = delta.width;
        let height = delta.height;
        let olot = delta.encode_bytes();
        let whole_wire_bytes = 5 + 8 + olot.len();

        let (pt_wire, pt_enc_us) = if stats_on {
            let tp = Instant::now();
            let pt = encode_delta(&frame, before, TileEncoding::PerTile, 1).ok();
            let us = tp.elapsed().as_micros() as u64;
            (pt.map_or(0, |d| 5 + 8 + d.encode_bytes().len()), us)
        } else {
            (0, 0)
        };

        // 4. Reserve sequence number and write the binary frame.
        let seq = {
            let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.stop {
                break;
            }
            s.seq_sent += 1;
            s.seq_sent
        };
        let t_write = Instant::now();
        match shared.send_server_res(&ServerMsg::Frame { seq, delta: olot }) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                // Undo the unsent seq reservation and report the wire-limit error
                // cleanly instead of terminating with a silent EOF (Finding 17).
                {
                    let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    s.seq_sent = s.seq_sent.saturating_sub(1);
                    s.need_keyframe = true;
                }
                let _ = shared.send_server(&ServerMsg::Status(StreamStatus { message: format!("Screen picture exceeds wire limit: {e}"), reconnecting: false, error: !is_controlling }));
                continue;
            }
            Err(_) => {
                shared.trigger_stop();
                break;
            }
        }
        let write_us = t_write.elapsed().as_micros() as u64;
        last_sent_done_at = Some(Instant::now());

        if stats_on {
            let frame_sent_us = shared.us_now();
            let total_us = t_total.elapsed().as_micros() as u64;
            let tr = *shared.trace.lock().unwrap_or_else(|e| e.into_inner());
            eprintln!(
                "{}",
                json!({
                    "seq": seq,
                    "key": force_key,
                    "width": width,
                    "height": height,
                    "tiles": tile_count,
                    "raw_bytes": raw_bytes,
                    "whole_wire_bytes": whole_wire_bytes,
                    "per_tile_wire_bytes": pt_wire,
                    "input_stdin_us": tr.input_stdin_us,
                    "input_req_start_us": tr.input_req_start_us,
                    "input_req_done_us": tr.input_req_done_us,
                    "first_painted_us": tr.first_painted_us,
                    "capture_wake_us": capture_wake_us,
                    "pacing_wait_us": pacing_wait_us,
                    "capture_begin_us": capture_begin_us,
                    "shot_us": shot_us,
                    "read_us": read_us,
                    "diff_us": diff_us,
                    "whole_zstd_us": enc_us.saturating_sub(diff_us),
                    "per_tile_zstd_us": pt_enc_us.saturating_sub(diff_us),
                    "write_us": write_us,
                    "frame_sent_us": frame_sent_us,
                    "total_us": total_us,
                })
            );
        }

        // Save current frame as reference for the next delta.
        prev_frame = Some(frame);
    }

    // Orderly teardown: stop threads, let `screen-input` flush queued commands,
    // release all held inputs, and drop `NAME.control` (`_capture_guard` removes
    // the private `0700` capture dir on drop).
    shared.trigger_stop();
    let _ = stdin_handle.join();
    shared.close_input();
    let _ = input_handle.join();
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_consecutive_moves_preserving_order() {
        let mut events = vec![
            json!({"type": "move", "x": 1.0, "y": 2.0}),
            json!({"type": "move", "x": 3.0, "y": 4.0}),
            json!({"type": "button", "button": 272, "pressed": true}),
            json!({"type": "move", "x": 5.0, "y": 6.0}),
            json!({"type": "move", "x": 7.0, "y": 8.0}),
            json!({"type": "move", "x": 9.0, "y": 10.0}),
            json!({"type": "button", "button": 272, "pressed": false}),
        ];
        coalesce_moves(&mut events);
        assert_eq!(
            events,
            vec![
                json!({"type": "move", "x": 3.0, "y": 4.0}),
                json!({"type": "button", "button": 272, "pressed": true}),
                json!({"type": "move", "x": 9.0, "y": 10.0}),
                json!({"type": "button", "button": 272, "pressed": false}),
            ]
        );
    }

    #[test]
    fn tracks_held_keys_buttons_keysyms_and_touches_without_duplicates() {
        let mut held = HeldInputs::default();
        let events = [
            json!({"type": "key", "key": 42, "pressed": true}),
            json!({"type": "key", "key": 42, "pressed": true}),
            json!({"type": "key", "key": 30, "pressed": true}),
            json!({"type": "button", "button": 272, "pressed": true}),
            json!({"type": "keysym", "keysym": 65, "pressed": true}),
            json!({"type": "touch", "slot": 1, "phase": "down", "x": 10.0, "y": 20.0}),
            json!({"type": "key", "key": 30, "pressed": false}),
        ];
        held.record_presses_conservative(&events);
        // On ambiguous reply, key 30 is STILL tracked as held!
        assert_eq!(held.keys, vec![42, 30]);
        held.clear();
        held.apply_batch_in_order(&events);
        assert_eq!(held.keys, vec![42]);
        assert_eq!(held.buttons, vec![272]);
        assert_eq!(held.keysyms, vec![65]);
        assert_eq!(held.touches, vec![1]);
        // Finding 9: [release(42), press(42)] in one batch ends with 42 still held!
        let re_press = [json!({"type": "key", "key": 42, "pressed": false}), json!({"type": "key", "key": 42, "pressed": true})];
        held.apply_batch_in_order(&re_press);
        assert_eq!(held.keys, vec![42]);
    }

    #[test]
    fn validates_only_supported_synthetic_events() {
        assert!(is_valid_synthetic_event(&json!({"type": "move", "x": 1.0, "y": 2.0})));
        assert!(is_valid_synthetic_event(&json!({"type": "keysym", "keysym": 65, "pressed": true})));
        assert!(is_valid_synthetic_event(&json!({"type": "touch", "slot": 0, "phase": "down", "x": 1.0, "y": 2.0})));
        assert!(!is_valid_synthetic_event(&json!({"type": "touchpad_gesture", "kind": "pinch"})));
        assert!(!is_valid_synthetic_event(&json!({"type": "key", "key": -1, "pressed": true})));
        assert!(!is_valid_synthetic_event(&json!({"type": "move", "x": "bad", "y": 2.0})));
    }

    /// Finding 48: `SyntheticEvent::Wheel { dx: i32, dy: i32 }` is accepted and
    /// non-integer / missing fields are rejected.
    #[test]
    fn review2_w3_wheel_synthetic_event_valid() {
        assert!(is_valid_synthetic_event(&json!({"type": "wheel", "dx": 0, "dy": -1})));
        assert!(is_valid_synthetic_event(&json!({"type": "wheel", "dx": 2, "dy": 0})));
        assert!(!is_valid_synthetic_event(&json!({"type": "wheel", "dx": "bad", "dy": 1})));
        assert!(!is_valid_synthetic_event(&json!({"type": "wheel", "dx": 0})));
        assert!(!is_valid_synthetic_event(&json!({"type": "wheel", "dx": 1.5, "dy": 0})));
    }

    /// Finding 25: `connect_verified_socket` MUST NOT block indefinitely when
    /// an `AF_UNIX` listener has a full accept backlog (Linux `unix_wait_for_peer`).
    #[test]
    fn review2_w2_connect_verified_socket_respects_deadline_on_full_backlog() {
        use std::os::unix::ffi::OsStrExt as _;
        use std::sync::mpsc;

        let dir = PathBuf::from(format!("/var/tmp/olo-r2-w2-conn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sock_path = dir.join("full.sock");

        // Bind AF_UNIX listener with backlog=1 and NEVER call accept().
        let lfd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
        assert!(lfd >= 0);
        let lguard = OwnedFd::new(lfd);
        let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        let bytes = sock_path.as_os_str().as_bytes();
        assert!(bytes.len() < addr.sun_path.len());
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), addr.sun_path.as_mut_ptr().cast(), bytes.len());
        }
        let addrlen = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1) as libc::socklen_t;
        assert_eq!(unsafe { libc::bind(lguard.raw(), (&addr as *const libc::sockaddr_un).cast(), addrlen) }, 0);
        assert_eq!(unsafe { libc::listen(lguard.raw(), 1) }, 0);

        // Fill kernel AF_UNIX listen backlog until non-blocking connect hits EAGAIN.
        let mut fillers = Vec::new();
        let mut hit_eagain = false;
        for _ in 0..32 {
            let cfd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC, 0) };
            assert!(cfd >= 0);
            let cg = OwnedFd::new(cfd);
            let r = unsafe { libc::connect(cg.raw(), (&addr as *const libc::sockaddr_un).cast(), addrlen) };
            if r == 0 {
                fillers.push(cg);
            } else {
                let err = io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EAGAIN) || err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                    hit_eagain = true;
                    break;
                }
                panic!("unexpected connect error while filling backlog: {err}");
            }
        }
        assert!(hit_eagain, "failed to fill AF_UNIX listen backlog");

        let (tx, rx) = mpsc::channel();
        let sp = sock_path.clone();
        let pid = std::process::id() as i32;
        let t = std::thread::spawn(move || {
            let started = Instant::now();
            let res = connect_verified_socket(&sp, pid, "qa", Duration::from_millis(120));
            let _ = tx.send((res.err().map(|e| e.kind()), started.elapsed()));
        });

        let outcome = rx.recv_timeout(Duration::from_millis(600));
        // Always close listener so a blocked thread (pre-fix) unblocks cleanly!
        drop(lguard);
        drop(fillers);
        let _ = t.join();
        let _ = std::fs::remove_dir_all(&dir);

        let (err_kind, elapsed) = outcome.expect("Finding 25: connect_verified_socket blocked past 600 ms on full AF_UNIX backlog!");
        assert!(err_kind.is_some(), "expected connect failure on full backlog");
        assert!(elapsed < Duration::from_millis(450), "connect exceeded deadline: {elapsed:?}");
    }
}
