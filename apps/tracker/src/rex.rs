//! Client for Rex's local files Unix socket (`rex://HOST/PATH`).
//!
//! All calls are synchronous blocking I/O meant to run on Tracker's worker
//! threads (never on the UI looper). When `OLO_TRACKER_DEMO_REX=1` is set
//! (for headless visual QA and `ui-lint`), returns a deterministic fixture
//! host (`dev-box`) without requiring a live Rex instance.

use std::{
    fs::File,
    io::{self, BufRead, BufReader, Write},
    net::Shutdown,
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
        net::UnixStream,
    },
    path::{Path, PathBuf},
    sync::{
        Once,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use olo_kit_base::tracker::RexPath;
use serde_json::{Value, json};

use crate::{
    fs::{Entry, Shown, kind_for},
    ops::{CancelWake, Progress},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostState {
    Connected,
    Connecting,
    Reconnecting,
    Disconnected,
    Error(String),
    RexNotRunning,
    Unknown,
}

impl HostState {
    pub fn from_wire(state: &str, err: Option<String>) -> Self {
        match state {
            "connected" => HostState::Connected,
            "connecting" => HostState::Connecting,
            "reconnecting" => HostState::Reconnecting,
            "disconnected" => HostState::Disconnected,
            "error" => HostState::Error(err.unwrap_or_else(|| "connection error".into())),
            _ => HostState::Unknown,
        }
    }

    pub fn is_connected(&self) -> bool {
        *self == HostState::Connected
    }
}

#[derive(Debug, Clone)]
pub struct HostInfo {
    pub label: String,
    pub state: HostState,
}

#[derive(Debug, Clone)]
pub struct ListResult {
    pub host: String,
    pub resolved: String,
    pub free_bytes: Option<u64>,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone)]
pub struct StatResult {
    pub host: String,
    pub resolved: String,
    pub exists: bool,
    pub entry: Option<Entry>,
    pub endpoint_id: Option<String>,
    pub dev: Option<u64>,
    pub ino: Option<u64>,
    pub size: Option<u64>,
    pub mtime_secs: Option<i64>,
    pub mtime_nanos: Option<u32>,
    pub ctime_secs: Option<i64>,
    pub ctime_nanos: Option<u32>,
    /// `lstat` (non-following) identity fields identifying the directory entry
    /// itself (including valid and dangling symlinks) for verified removal.
    pub lstat_dev: Option<u64>,
    pub lstat_ino: Option<u64>,
    pub lstat_size: Option<u64>,
    pub lstat_mtime_secs: Option<i64>,
    pub lstat_mtime_nanos: Option<u32>,
    pub lstat_ctime_secs: Option<i64>,
    pub lstat_ctime_nanos: Option<u32>,
}

/// Verified endpoint and source `lstat` inode/version snapshot attached to a
/// Move's post-copy source removal so concurrent replacement or host
/// reassignment never deletes newer or unrelated data.
#[derive(Debug, Clone)]
pub struct RemoveExpect {
    pub is_dir: bool,
    pub endpoint_id: String,
    pub dev: u64,
    pub ino: u64,
    pub size: Option<u64>,
    pub mtime_secs: i64,
    pub mtime_nanos: u32,
    pub ctime_secs: i64,
    pub ctime_nanos: u32,
}

impl RemoveExpect {
    pub fn from_stat(st: &StatResult) -> Result<Self, String> {
        if !st.exists {
            return Err("Source no longer exists on the remote host.".into());
        }
        let is_dir = st.entry.as_ref().is_some_and(|e| e.is_dir && !e.is_link);
        let endpoint_id = st
            .endpoint_id
            .as_deref()
            .filter(|ep| !ep.is_empty() && !ep.contains("|unverified"))
            .ok_or_else(|| "Remote host did not provide a verified SSH endpoint identity; leaving original in place.".to_owned())?
            .to_owned();
        let (Some(dev), Some(ino), Some(mtime_secs), Some(mtime_nanos), Some(ctime_secs), Some(ctime_nanos)) =
            (st.lstat_dev, st.lstat_ino, st.lstat_mtime_secs, st.lstat_mtime_nanos, st.lstat_ctime_secs, st.lstat_ctime_nanos)
        else {
            return Err("Remote host helper did not report lstat identity fields (update Olo’s helper); leaving original in place.".into());
        };
        let size = if is_dir { None } else { Some(st.lstat_size.ok_or_else(|| "Remote host helper did not report lstat size; leaving original in place.".to_owned())?) };
        Ok(Self { is_dir, endpoint_id, dev, ino, size, mtime_secs, mtime_nanos, ctime_secs, ctime_nanos })
    }
}

#[derive(Debug, Clone)]
pub struct DownloadResult {
    pub path: PathBuf,
    pub complete: bool,
    pub endpoint_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UploadResult {
    pub path: RexPath,
    pub complete: bool,
}

fn demo_mode() -> bool {
    std::env::var("OLO_TRACKER_DEMO_REX").ok().as_deref() == Some("1")
}

fn demo_entry(host: &str, dir: &str, name: &str, is_dir: bool, size: u64, age_secs: u64, mode: u32) -> Entry {
    let rp = RexPath::new(host, dir).join(name);
    let path = rp.to_path_buf();
    let kind = kind_for(Path::new(name), is_dir, mode);
    let modified = SystemTime::now().checked_sub(Duration::from_secs(age_secs));
    Entry { path, name: name.to_owned(), is_dir, is_link: false, broken: false, size, modified, mode, kind, app_icon: None, downloading: None, shown: Shown::default() }
}

fn demo_list(rex: &RexPath, hidden: bool) -> Result<ListResult, String> {
    if rex.host.eq_ignore_ascii_case("build-gpu") {
        return Err("“build-gpu” isn’t connected in Rex.".into());
    }
    let resolved = if rex.path == "~" || rex.path == "/home/dev" {
        "/home/dev".to_owned()
    } else if let Some(rest) = rex.path.strip_prefix("~/") {
        format!("/home/dev/{rest}")
    } else {
        rex.path.clone()
    };
    let host = if rex.host.eq_ignore_ascii_case("dev-box") { "dev-box".to_owned() } else { rex.host.clone() };
    let mut entries = match resolved.as_str() {
        "/home/dev/src" => vec![
            demo_entry(&host, &resolved, "olo", true, 0, 120, 0o755),
            demo_entry(&host, &resolved, "jato", true, 0, 3600, 0o755),
            demo_entry(&host, &resolved, "Cargo.toml", false, 4_120, 600, 0o644),
            demo_entry(&host, &resolved, "README.md", false, 14_800, 7200, 0o644),
            demo_entry(&host, &resolved, "deploy.sh", false, 1_940, 1800, 0o755),
        ],
        _ => vec![
            demo_entry(&host, &resolved, "src", true, 0, 180, 0o755),
            demo_entry(&host, &resolved, "docs", true, 0, 2400, 0o755),
            demo_entry(&host, &resolved, "notes", true, 0, 900, 0o755),
            demo_entry(&host, &resolved, "README.md", false, 12_480, 420, 0o644),
            demo_entry(&host, &resolved, "Cargo.toml", false, 3_920, 1400, 0o644),
            demo_entry(&host, &resolved, "benchmark.csv", false, 98_432, 3100, 0o644),
            demo_entry(&host, &resolved, "architecture.png", false, 418_200, 5400, 0o644),
            demo_entry(&host, &resolved, "run-tests.sh", false, 2_150, 8600, 0o755),
        ],
    };
    if hidden {
        entries.push(demo_entry(&host, &resolved, ".bashrc", false, 1_120, 86_400, 0o644));
    }
    Ok(ListResult { host, resolved, free_bytes: Some(142 * 1024 * 1024 * 1024), entries })
}

/// Maximum raw bytes sent inline as base64 over Rex's files socket (4 MiB).
/// Larger payloads are staged privately under `/var/tmp` and streamed via
/// `upload` (`olo-rex-sync`), keeping inline memory tiny and enabling live
/// progress and cancellation.
const MAX_INLINE_WRITE_BYTES: usize = 4 * 1024 * 1024;

/// Maximum reply frame accepted from Rex on the local Unix socket (32 MiB).
const MAX_REPLY_LINE_BYTES: usize = 32 * 1024 * 1024;

/// Client-side read timeouts with explicit 15 s slack above Rex's server
/// operation deadlines so Rex reports its own error instead of racing Tracker.
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(75);
const COPY_REMOTE_READ_TIMEOUT: Duration = Duration::from_secs(315);
const READ_CACHE_TIMEOUT: Duration = Duration::from_secs(615);
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(180);
const STALE_STAGE_AGE: Duration = Duration::from_secs(3600);

fn connect_with_read_timeout(read_timeout: Duration) -> Result<UnixStream, String> {
    let sock = olo_kit_base::tracker::rex_files_socket();
    let stream = UnixStream::connect(&sock).map_err(|_| "Rex isn’t running. Open Rex to browse files on remote hosts.".to_owned())?;
    let _ = stream.set_read_timeout(Some(read_timeout));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    Ok(stream)
}

/// Read one newline-delimited JSON reply up to `max_bytes` into caller-owned
/// `buf`, preserving partial bytes across `WouldBlock`/`TimedOut`/`Interrupted`
/// and enforcing the byte cap BEFORE UTF-8 decoding.
fn read_bounded_line_into<R: BufRead>(reader: &mut R, buf: &mut Vec<u8>, max_bytes: usize) -> io::Result<Option<String>> {
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            if buf.is_empty() {
                return Ok(None);
            }
            let raw = std::mem::take(buf);
            return String::from_utf8(raw).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("Unexpected non-UTF-8 response from Rex: {e}")));
        }
        let (take, saw_nl) = match chunk.iter().position(|&b| b == b'\n') {
            Some(pos) => (pos + 1, true),
            None => (chunk.len(), false),
        };
        if buf.len().saturating_add(take) > max_bytes {
            buf.clear();
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("Response from Rex exceeded {} MiB limit.", max_bytes / (1024 * 1024))));
        }
        buf.extend_from_slice(&chunk[..take]);
        reader.consume(take);
        if saw_nl {
            let raw = std::mem::take(buf);
            return String::from_utf8(raw).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("Unexpected non-UTF-8 response from Rex: {e}")));
        }
    }
}

fn read_bounded_line<R: BufRead>(reader: &mut R, max_bytes: usize) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    read_bounded_line_into(reader, &mut buf, max_bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PollOutcome {
    Ready,
    Cancelled,
    TimedOut,
}

#[derive(Clone, Copy)]
enum CancelWatch<'a> {
    Active(&'a AtomicBool, Option<&'a CancelWake>),
    /// Cancel frame was already sent: wait straight to `deadline` on `sock_fd`
    /// with ZERO 200 ms poll slices.
    AlreadySent,
}

/// Wait on `sock_fd` for `events` (`POLLIN` or `POLLOUT`) or `watch`'s
/// `eventfd` until `deadline` with ZERO 200 ms polling when `CancelWake` is
/// supplied or when `CancelWatch::AlreadySent` is active.
fn poll_socket_or_cancel(sock_fd: i32, events: i16, watch: CancelWatch<'_>, deadline: Instant) -> io::Result<PollOutcome> {
    loop {
        if let CancelWatch::Active(cancel, _) = watch
            && cancel.load(Ordering::Acquire)
        {
            return Ok(PollOutcome::Cancelled);
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(PollOutcome::TimedOut);
        }
        let rem = deadline.saturating_duration_since(now);
        let (wake_fd, wait_ms) = match watch {
            CancelWatch::AlreadySent => (-1, rem.as_millis().clamp(1, i32::MAX as u128) as i32),
            CancelWatch::Active(_, cancel_wake) => match cancel_wake.and_then(CancelWake::raw_fd) {
                Some(fd) => (fd, rem.as_millis().clamp(1, i32::MAX as u128) as i32),
                None => (-1, rem.min(Duration::from_millis(200)).as_millis().clamp(1, i32::MAX as u128) as i32),
            },
        };
        let mut pfds = [libc::pollfd { fd: sock_fd, events, revents: 0 }, libc::pollfd { fd: wake_fd, events: libc::POLLIN, revents: 0 }];
        let nfds: libc::nfds_t = if wake_fd >= 0 { 2 } else { 1 };
        // SAFETY: pfds has at least `nfds` valid pollfd entries.
        let rc = unsafe { libc::poll(pfds.as_mut_ptr(), nfds, wait_ms) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if let CancelWatch::Active(cancel, _) = watch
            && (cancel.load(Ordering::Acquire) || (nfds == 2 && (pfds[1].revents & libc::POLLIN) != 0))
        {
            return Ok(PollOutcome::Cancelled);
        }
        if rc == 0 {
            if Instant::now() >= deadline {
                return Ok(PollOutcome::TimedOut);
            }
            continue;
        }
        if pfds[0].revents != 0 {
            return Ok(PollOutcome::Ready);
        }
    }
}

/// Write `data` to a non-blocking `UnixStream`, observing `cancel` and
/// `cancel_wake` before and between chunks so Stop wakes even if the socket
/// buffer is full. Returns `Ok(None)` on full write, `Ok(Some(err))` on
/// BrokenPipe/ConnectionReset (so caller can still read Rex's JSON error), or
/// `Err(Interrupted)` if cancelled before completing the write.
fn write_all_cancellable(stream: &mut UnixStream, data: &[u8], cancel: &AtomicBool, cancel_wake: Option<&CancelWake>, deadline: Instant) -> io::Result<Option<io::Error>> {
    let mut off = 0usize;
    let fd = stream.as_raw_fd();
    while off < data.len() {
        if cancel.load(Ordering::Acquire) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        match stream.write(&data[off..]) {
            Ok(0) => return Ok(Some(io::Error::new(io::ErrorKind::WriteZero, "socket closed while writing"))),
            Ok(n) => off += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => match poll_socket_or_cancel(fd, libc::POLLOUT, CancelWatch::Active(cancel, cancel_wake), deadline)? {
                PollOutcome::Ready => continue,
                PollOutcome::Cancelled => return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled")),
                PollOutcome::TimedOut => return Err(io::Error::new(io::ErrorKind::TimedOut, "Timed out sending request to Rex.")),
            },
            Err(e) if matches!(e.kind(), io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset) => return Ok(Some(e)),
            Err(e) => return Err(io::Error::other(format!("Couldn’t reach Rex: {e}"))),
        }
    }
    Ok(None)
}

/// Send `{"op":"cancel"}\n` over a non-blocking UnixStream using a bounded 2 s
/// `poll(POLLOUT)` loop. NEVER clears `O_NONBLOCK` on the shared file
/// description (which would make concurrent reads on the cloned fd block forever!).
fn send_cancel_best_effort(writer: &mut UnixStream) {
    const FRAME: &[u8] = b"{\"op\":\"cancel\"}\n";
    let deadline = Instant::now() + Duration::from_secs(2);
    let fd = writer.as_raw_fd();
    let mut off = 0usize;
    while off < FRAME.len() {
        match writer.write(&FRAME[off..]) {
            Ok(0) => return,
            Ok(n) => off += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                let now = Instant::now();
                if now >= deadline {
                    return;
                }
                let rem_ms = deadline.saturating_duration_since(now).as_millis().clamp(1, i32::MAX as u128) as i32;
                let mut pfd = libc::pollfd { fd, events: libc::POLLOUT, revents: 0 };
                // SAFETY: `pfd` points to one valid pollfd struct.
                let rc = unsafe { libc::poll(&mut pfd, 1, rem_ms) };
                if rc <= 0 && (rc == 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted) {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

fn call_on_stream(mut stream: UnixStream, req: Value) -> Result<Value, String> {
    let mut line = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
    // Release the JSON Value immediately after serialization so large inline
    // payloads are never kept in memory twice while writing or awaiting reply.
    drop(req);
    line.push(b'\n');
    let write_err = match stream.write_all(&line).and_then(|()| stream.flush()) {
        Ok(()) => None,
        Err(e) if matches!(e.kind(), io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset) => Some(e),
        Err(e) => return Err(format!("Couldn’t reach Rex: {e}")),
    };
    drop(line);

    let mut reader = BufReader::new(stream);
    match read_bounded_line(&mut reader, MAX_REPLY_LINE_BYTES) {
        Ok(None) => {
            if let Some(e) = write_err {
                Err(format!("Couldn’t reach Rex: {e}"))
            } else {
                Err("Rex closed the connection.".into())
            }
        }
        Ok(Some(resp_line)) => {
            let val: Value = serde_json::from_str(resp_line.trim()).map_err(|e| format!("Unexpected response from Rex: {e}"))?;
            if val.get("ok").and_then(Value::as_bool) == Some(true) { Ok(val) } else { Err(val.get("error").and_then(Value::as_str).unwrap_or("Remote operation failed.").to_owned()) }
        }
        Err(e) => {
            if let Some(we) = write_err {
                Err(format!("Couldn’t reach Rex: {we}"))
            } else {
                Err(format!("Rex didn’t answer: {e}"))
            }
        }
    }
}

fn call_on_stream_cancellable(mut stream: UnixStream, req: Value, total_deadline: Duration, cancel: &AtomicBool, cancel_wake: Option<&CancelWake>) -> io::Result<Value> {
    stream.set_nonblocking(true)?;
    let started = Instant::now();
    let deadline = started + total_deadline;
    let write_deadline = (started + Duration::from_secs(30)).min(deadline);

    let mut line = serde_json::to_vec(&req).map_err(io::Error::other)?;
    drop(req);
    line.push(b'\n');
    let write_err = write_all_cancellable(&mut stream, &line, cancel, cancel_wake, write_deadline)?;
    drop(line);

    let mut writer = stream.try_clone()?;
    let sock_fd = stream.as_raw_fd();
    let mut reader = BufReader::new(stream);
    let mut partial_buf = Vec::new();
    let mut sent_cancel = false;
    loop {
        if cancel.load(Ordering::Acquire) && !sent_cancel {
            sent_cancel = true;
            send_cancel_best_effort(&mut writer);
        }
        match read_bounded_line_into(&mut reader, &mut partial_buf, MAX_REPLY_LINE_BYTES) {
            Ok(None) => {
                if cancel.load(Ordering::Acquire) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                }
                return if let Some(we) = write_err { Err(io::Error::other(format!("Couldn’t reach Rex: {we}"))) } else { Err(io::Error::other("Rex closed the connection.")) };
            }
            Ok(Some(resp_line)) => {
                let val: Value = serde_json::from_str(resp_line.trim()).map_err(|e| io::Error::other(format!("Unexpected response from Rex: {e}")))?;
                if val.get("ok").and_then(Value::as_bool) == Some(true) {
                    return Ok(val);
                }
                if val.get("cancelled").and_then(Value::as_bool) == Some(true) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                }
                let err = val.get("error").and_then(Value::as_str).unwrap_or("Remote operation failed.");
                return Err(io::Error::other(err.to_owned()));
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => {
                let watch = if sent_cancel { CancelWatch::AlreadySent } else { CancelWatch::Active(cancel, cancel_wake) };
                match poll_socket_or_cancel(sock_fd, libc::POLLIN, watch, deadline)? {
                    PollOutcome::Ready => continue,
                    PollOutcome::Cancelled => {
                        if !sent_cancel {
                            sent_cancel = true;
                            send_cancel_best_effort(&mut writer);
                        }
                    }
                    PollOutcome::TimedOut => return Err(io::Error::new(io::ErrorKind::TimedOut, "Timed out waiting for Rex.")),
                }
            }
            Err(e) => {
                return if let Some(we) = write_err { Err(io::Error::other(format!("Couldn’t reach Rex: {we}"))) } else { Err(io::Error::other(format!("Rex didn’t answer: {e}"))) };
            }
        }
    }
}

fn call_with_timeout(req: Value, read_timeout: Duration) -> Result<Value, String> {
    let stream = connect_with_read_timeout(read_timeout)?;
    call_on_stream(stream, req)
}

fn call(req: Value) -> Result<Value, String> {
    call_with_timeout(req, DEFAULT_READ_TIMEOUT)
}

fn entry_from_json(host: &str, parent_dir: &str, v: &Value) -> Option<Entry> {
    let name = v.get("name")?.as_str()?.to_owned();
    let is_dir = v.get("is_dir").and_then(Value::as_bool).unwrap_or(false);
    let is_link = v.get("is_link").and_then(Value::as_bool).unwrap_or(false);
    let size = v.get("size").and_then(Value::as_u64).unwrap_or(0);
    let mode = v.get("mode").and_then(Value::as_u64).unwrap_or(if is_dir { 0o755 } else { 0o644 }) as u32;
    let mtime_secs = v.get("mtime_secs").and_then(Value::as_i64);
    let mtime_nanos = v.get("mtime_nanos").and_then(Value::as_u64).unwrap_or(0) as u32;
    let modified = mtime_secs.and_then(|s| if s >= 0 { UNIX_EPOCH.checked_add(Duration::new(s as u64, mtime_nanos)) } else { UNIX_EPOCH.checked_sub(Duration::from_secs(s.unsigned_abs())) });
    let rp = if name == "/" { RexPath::new(host, "/") } else { RexPath::new(host, parent_dir).join(&name) };
    let kind = kind_for(Path::new(&name), is_dir, mode);
    Some(Entry { path: rp.to_path_buf(), name, is_dir, is_link, broken: false, size: if is_dir { 0 } else { size }, modified, mode, kind, app_icon: None, downloading: None, shown: Shown::default() })
}

fn entry_at_path_from_json(host: &str, full_path: &str, v: &Value) -> Option<Entry> {
    let name = v.get("name")?.as_str()?.to_owned();
    let is_dir = v.get("is_dir").and_then(Value::as_bool).unwrap_or(false);
    let is_link = v.get("is_link").and_then(Value::as_bool).unwrap_or(false);
    let size = v.get("size").and_then(Value::as_u64).unwrap_or(0);
    let mode = v.get("mode").and_then(Value::as_u64).unwrap_or(if is_dir { 0o755 } else { 0o644 }) as u32;
    let mtime_secs = v.get("mtime_secs").and_then(Value::as_i64);
    let mtime_nanos = v.get("mtime_nanos").and_then(Value::as_u64).unwrap_or(0) as u32;
    let modified = mtime_secs.and_then(|s| if s >= 0 { UNIX_EPOCH.checked_add(Duration::new(s as u64, mtime_nanos)) } else { UNIX_EPOCH.checked_sub(Duration::from_secs(s.unsigned_abs())) });
    let rp = RexPath::new(host, full_path);
    let kind = kind_for(Path::new(&name), is_dir, mode);
    Some(Entry { path: rp.to_path_buf(), name, is_dir, is_link, broken: false, size: if is_dir { 0 } else { size }, modified, mode, kind, app_icon: None, downloading: None, shown: Shown::default() })
}

pub fn hosts() -> Result<Vec<HostInfo>, String> {
    if demo_mode() {
        return Ok(vec![HostInfo { label: "dev-box".into(), state: HostState::Connected }, HostInfo { label: "build-gpu".into(), state: HostState::Disconnected }]);
    }
    let val = call(json!({ "op": "hosts" }))?;
    let list = val
        .get("hosts")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|h| {
                    let label = h.get("label")?.as_str()?.to_owned();
                    let st = h.get("state").and_then(Value::as_str).unwrap_or("disconnected");
                    let err = h.get("error").and_then(Value::as_str).map(str::to_owned);
                    Some(HostInfo { label, state: HostState::from_wire(st, err) })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(list)
}

pub fn host_state(host: &str) -> HostState {
    if demo_mode() {
        return if host.eq_ignore_ascii_case("build-gpu") { HostState::Disconnected } else { HostState::Connected };
    }
    match call(json!({ "op": "host_state", "host": host })) {
        Ok(val) => {
            if val.get("found").and_then(Value::as_bool) != Some(true) {
                return HostState::Unknown;
            }
            let st = val.get("state").and_then(Value::as_str).unwrap_or("disconnected");
            let err = val.get("error").and_then(Value::as_str).map(str::to_owned);
            HostState::from_wire(st, err)
        }
        Err(e) if e.contains("isn’t running") => HostState::RexNotRunning,
        Err(e) => HostState::Error(e),
    }
}

pub fn list(rex: &RexPath, hidden: bool) -> Result<ListResult, String> {
    if demo_mode() {
        return demo_list(rex, hidden);
    }
    let val = call(json!({ "op": "list", "host": rex.host, "path": rex.path, "hidden": hidden }))?;
    let host = val.get("host").and_then(Value::as_str).unwrap_or(&rex.host).to_owned();
    let resolved = val.get("resolved").and_then(Value::as_str).unwrap_or(&rex.path).to_owned();
    let free_bytes = val.get("free_bytes").and_then(Value::as_u64);
    let entries = val.get("entries").and_then(Value::as_array).map(|arr| arr.iter().filter_map(|e| entry_from_json(&host, &resolved, e)).collect()).unwrap_or_default();
    Ok(ListResult { host, resolved, free_bytes, entries })
}

pub fn stat(rex: &RexPath) -> Result<StatResult, String> {
    if demo_mode() {
        let resolved = if rex.path == "~" { "/home/dev".into() } else { rex.path.clone() };
        let is_dir = !resolved.contains('.');
        let name = resolved.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("/");
        let entry = Entry {
            path: RexPath::new(&rex.host, &resolved).to_path_buf(),
            name: name.to_owned(),
            is_dir,
            is_link: false,
            broken: false,
            size: if is_dir { 0 } else { 4096 },
            modified: Some(SystemTime::now()),
            mode: if is_dir { 0o755 } else { 0o644 },
            kind: kind_for(Path::new(name), is_dir, if is_dir { 0o755 } else { 0o644 }),
            app_icon: None,
            downloading: None,
            shown: Shown::default(),
        };
        return Ok(StatResult {
            host: rex.host.clone(),
            resolved,
            exists: true,
            entry: Some(entry),
            endpoint_id: Some("demo".into()),
            dev: None,
            ino: None,
            size: Some(if is_dir { 0 } else { 4096 }),
            mtime_secs: None,
            mtime_nanos: None,
            ctime_secs: None,
            ctime_nanos: None,
            lstat_dev: None,
            lstat_ino: None,
            lstat_size: Some(if is_dir { 0 } else { 4096 }),
            lstat_mtime_secs: None,
            lstat_mtime_nanos: None,
            lstat_ctime_secs: None,
            lstat_ctime_nanos: None,
        });
    }
    let val = call(json!({ "op": "stat", "host": rex.host, "path": rex.path }))?;
    let host = val.get("host").and_then(Value::as_str).unwrap_or(&rex.host).to_owned();
    let resolved = val.get("resolved").and_then(Value::as_str).unwrap_or(&rex.path).to_owned();
    let exists = val.get("exists").and_then(Value::as_bool).unwrap_or(false);
    let endpoint_id = val.get("endpoint_id").and_then(Value::as_str).map(str::to_owned);
    let raw_entry = val.get("entry").filter(|v| !v.is_null());
    let entry = raw_entry.and_then(|v| entry_at_path_from_json(&host, &resolved, v));
    Ok(StatResult {
        host,
        resolved,
        exists,
        entry,
        endpoint_id,
        dev: raw_entry.and_then(|v| v.get("dev")).and_then(Value::as_u64),
        ino: raw_entry.and_then(|v| v.get("ino")).and_then(Value::as_u64),
        size: raw_entry.and_then(|v| v.get("size")).and_then(Value::as_u64),
        mtime_secs: raw_entry.and_then(|v| v.get("mtime_secs")).and_then(Value::as_i64),
        mtime_nanos: raw_entry.and_then(|v| v.get("mtime_nanos")).and_then(Value::as_u64).map(|n| n as u32),
        ctime_secs: raw_entry.and_then(|v| v.get("ctime_secs")).and_then(Value::as_i64),
        ctime_nanos: raw_entry.and_then(|v| v.get("ctime_nanos")).and_then(Value::as_u64).map(|n| n as u32),
        lstat_dev: raw_entry.and_then(|v| v.get("lstat_dev")).and_then(Value::as_u64),
        lstat_ino: raw_entry.and_then(|v| v.get("lstat_ino")).and_then(Value::as_u64),
        lstat_size: raw_entry.and_then(|v| v.get("lstat_size")).and_then(Value::as_u64),
        lstat_mtime_secs: raw_entry.and_then(|v| v.get("lstat_mtime_secs")).and_then(Value::as_i64),
        lstat_mtime_nanos: raw_entry.and_then(|v| v.get("lstat_mtime_nanos")).and_then(Value::as_u64).map(|n| n as u32),
        lstat_ctime_secs: raw_entry.and_then(|v| v.get("lstat_ctime_secs")).and_then(Value::as_i64),
        lstat_ctime_nanos: raw_entry.and_then(|v| v.get("lstat_ctime_nanos")).and_then(Value::as_u64).map(|n| n as u32),
    })
}

pub fn mkdir(rex: &RexPath, unique: bool) -> Result<(RexPath, Option<Entry>), String> {
    let val = call(json!({ "op": "mkdir", "host": rex.host, "path": rex.path, "unique": unique }))?;
    let host = val.get("host").and_then(Value::as_str).unwrap_or(&rex.host).to_owned();
    let path = val.get("path").and_then(Value::as_str).unwrap_or(&rex.path).to_owned();
    let entry = val.get("entry").filter(|v| !v.is_null()).and_then(|v| entry_at_path_from_json(&host, &path, v));
    Ok((RexPath::new(host, path), entry))
}

pub fn rename(from: &RexPath, to: &RexPath, unique: bool) -> Result<RexPath, String> {
    if !from.host.eq_ignore_ascii_case(&to.host) {
        return Err("Can’t rename across different hosts.".into());
    }
    let val = call(json!({ "op": "rename", "host": from.host, "from": from.path, "to": to.path, "unique": unique }))?;
    let host = val.get("host").and_then(Value::as_str).unwrap_or(&from.host).to_owned();
    let path = val.get("path").and_then(Value::as_str).unwrap_or(&to.path).to_owned();
    Ok(RexPath::new(host, path))
}

pub fn copy_remote(from: &RexPath, to: &RexPath, cancel: &AtomicBool, cancel_wake: Option<&CancelWake>) -> io::Result<RexPath> {
    if !from.host.eq_ignore_ascii_case(&to.host) {
        return Err(io::Error::other("Can’t copy directly across different hosts."));
    }
    let sock = olo_kit_base::tracker::rex_files_socket();
    let stream = UnixStream::connect(&sock).map_err(|_| io::Error::other("Rex isn’t running. Open Rex to browse files on remote hosts."))?;
    let val = call_on_stream_cancellable(stream, json!({ "op": "copy_remote", "host": from.host, "from": from.path, "to": to.path }), COPY_REMOTE_READ_TIMEOUT, cancel, cancel_wake)?;
    let host = val.get("host").and_then(Value::as_str).unwrap_or(&from.host).to_owned();
    let path = val.get("path").and_then(Value::as_str).unwrap_or(&to.path).to_owned();
    Ok(RexPath::new(host, path))
}

pub fn remove(rex: &RexPath) -> Result<(), String> {
    let _ = call(json!({ "op": "remove", "host": rex.host, "path": rex.path }))?;
    Ok(())
}

/// Remove a remote path via the dedicated `verified_remove` op (rejected by
/// legacy Rex binaries that lack verified-remove support) ONLY if the live
/// authenticated SSH handle still matches `expect.endpoint_id` and the source
/// entry's `lstat` metadata still matches its pre-copy snapshot.
pub fn remove_verified(rex: &RexPath, expect: &RemoveExpect) -> Result<(), String> {
    let mut req = json!({
        "op": "verified_remove",
        "host": rex.host,
        "path": rex.path,
        "is_dir": expect.is_dir,
        "expect_endpoint": expect.endpoint_id,
        "expect_dev": expect.dev,
        "expect_ino": expect.ino,
        "expect_mtime_secs": expect.mtime_secs,
        "expect_mtime_nanos": expect.mtime_nanos,
        "expect_ctime_secs": expect.ctime_secs,
        "expect_ctime_nanos": expect.ctime_nanos,
    });
    if let Some(s) = expect.size {
        req["expect_size"] = json!(s);
    }
    let _ = call(req)?;
    Ok(())
}

pub fn read_to_cache(rex: &RexPath) -> Result<PathBuf, String> {
    let val = call_with_timeout(json!({ "op": "read_to_cache", "host": rex.host, "path": rex.path }), READ_CACHE_TIMEOUT)?;
    let local = val.get("local").and_then(Value::as_str).ok_or_else(|| "Rex didn’t return a cached file path.".to_owned())?;
    Ok(PathBuf::from(local))
}

#[derive(Debug)]
struct StagedFileGuard {
    path: PathBuf,
    // Original exclusively created file handle holding advisory `LOCK_EX` for
    // the entire lifetime of the upload (no extra `try_clone()` leak window) so
    // another Tracker session's stale-stage sweeper never deletes a live upload.
    file: File,
}

impl Drop for StagedFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn is_valid_paste_stage_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("paste-").and_then(|s| s.strip_suffix(".bin")) else {
        return false;
    };
    let mut parts = rest.split('-');
    let (Some(a), Some(b), Some(c), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !a.is_empty() && !b.is_empty() && !c.is_empty() && [a, b, c].iter().all(|s| s.bytes().all(|ch| ch.is_ascii_digit()))
}

fn try_flock_exclusive(f: &File) -> bool {
    // SAFETY: `f` is an open file descriptor; LOCK_EX | LOCK_NB never blocks.
    unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

/// Owner-only scratch directory on disk (`/var/tmp`, never `/tmp`) for staging
/// large pastes before streaming them to Rex.
fn paste_stage_dir() -> PathBuf {
    // SAFETY: geteuid is a pure read of the current process's effective UID.
    let uid = unsafe { libc::geteuid() };
    PathBuf::from(format!("/var/tmp/olo-tracker-stage-{uid}"))
}

fn ensure_owner_stage_dir(dir: &Path) -> io::Result<()> {
    // SAFETY: geteuid is a pure read of the current process's effective UID.
    let my_uid = unsafe { libc::geteuid() };
    match std::fs::symlink_metadata(dir) {
        Ok(md) => {
            if md.file_type().is_symlink() || !md.is_dir() || md.uid() != my_uid {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("staging dir {} is unsafe", dir.display())));
            }
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let mut b = std::fs::DirBuilder::new();
            b.mode(0o700);
            match b.create(dir) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let md = std::fs::symlink_metadata(dir)?;
                    if md.file_type().is_symlink() || !md.is_dir() || md.uid() != my_uid {
                        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("staging dir {} is unsafe", dir.display())));
                    }
                    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        Err(e) => Err(e),
    }
}

fn sweep_abandoned_paste_stages(dir: &Path, max_age: Duration) {
    // SAFETY: geteuid is a pure read of the current process's effective UID.
    let my_uid = unsafe { libc::geteuid() };
    let Ok(md) = std::fs::symlink_metadata(dir) else { return };
    if md.file_type().is_symlink() || !md.is_dir() || md.uid() != my_uid {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_s) = name.to_str() else { continue };
        if !is_valid_paste_stage_name(name_s) {
            continue;
        }
        let path = entry.path();
        let Ok(emd) = std::fs::symlink_metadata(&path) else { continue };
        if !emd.file_type().is_file() || emd.uid() != my_uid {
            continue;
        }
        let Ok(modified) = emd.modified() else { continue };
        if now.duration_since(modified).is_ok_and(|age| age >= max_age) {
            // Only remove if we can acquire a non-blocking exclusive advisory
            // lock (proving no live Tracker process is still uploading it).
            if let Ok(f) = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(&path)
                && try_flock_exclusive(&f)
            {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

fn maybe_sweep_paste_stages_once(dir: PathBuf) {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        std::thread::spawn(move || sweep_abandoned_paste_stages(&dir, STALE_STAGE_AGE));
    });
}

/// Create a private `0600` staged file in `stage_dir`, locking it with an
/// advisory `LOCK_EX` and arming [`StagedFileGuard`] immediately after
/// exclusive creation (BEFORE writing a single byte) so any write/flush error
/// or cancellation leaves zero clipboard bytes behind and concurrent sweepers
/// never unlink a live upload.
fn write_staged_paste(stage_dir: &Path, bytes: &[u8], cancel: &AtomicBool) -> io::Result<StagedFileGuard> {
    static STAGE_SEQ: AtomicU64 = AtomicU64::new(0);
    ensure_owner_stage_dir(stage_dir)?;
    let seq = STAGE_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let stage_path = stage_dir.join(format!("paste-{}-{seq}-{nanos}.bin", std::process::id()));
    let f = std::fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(&stage_path)?;
    // Guard takes ownership of the original File IMMEDIATELY after creation
    // (before flock or any write) so every error path unlinks automatically.
    let mut guard = StagedFileGuard { path: stage_path, file: f };
    if !try_flock_exclusive(&guard.file) {
        return Err(io::Error::other("could not acquire exclusive advisory lock on staged paste file"));
    }
    for chunk in bytes.chunks(256 * 1024) {
        if cancel.load(Ordering::Acquire) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        guard.file.write_all(chunk)?;
    }
    guard.file.flush()?;
    Ok(guard)
}

fn require_utf8_path<'a>(path: &'a Path, what: &str) -> io::Result<&'a str> {
    path.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("{what} path contains non-UTF-8 bytes and cannot be transferred safely")))
}

fn split_remote_parent_and_name(path: &str) -> Result<(String, String), String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return Err(format!("invalid remote file path: {path}"));
    }
    match trimmed.rfind('/') {
        Some(0) => Ok(("/".to_owned(), trimmed[1..].to_owned())),
        Some(i) => Ok((trimmed[..i].to_owned(), trimmed[i + 1..].to_owned())),
        None => Ok(("~".to_owned(), trimmed.to_owned())),
    }
}

pub fn write_file(rex: &RexPath, unique: bool, bytes: &[u8]) -> Result<RexPath, String> {
    let progress = Progress::default();
    let cancel = AtomicBool::new(false);
    write_file_with_progress(rex, unique, bytes, &progress, &cancel, None).map_err(|e| e.to_string())
}

/// Write `bytes` to `rex`, streaming progress and honoring `cancel`. Payloads
/// larger than [`MAX_INLINE_WRITE_BYTES`] (4 MiB) are staged privately in
/// `/var/tmp` and uploaded via `olo-rex-sync` with remote default new-file
/// permissions (`default_mode = true`).
pub fn write_file_with_progress(rex: &RexPath, unique: bool, bytes: &[u8], progress: &Progress, cancel: &AtomicBool, cancel_wake: Option<&CancelWake>) -> io::Result<RexPath> {
    let sock = olo_kit_base::tracker::rex_files_socket();
    let stage_dir = paste_stage_dir();
    maybe_sweep_paste_stages_once(stage_dir.clone());
    write_file_to_socket_with_progress(&sock, &stage_dir, rex, unique, bytes, progress, cancel, cancel_wake)
}

#[allow(clippy::too_many_arguments)]
fn write_file_to_socket_with_progress(
    sock: &Path,
    stage_dir: &Path,
    rex: &RexPath,
    unique: bool,
    bytes: &[u8],
    progress: &Progress,
    cancel: &AtomicBool,
    cancel_wake: Option<&CancelWake>,
) -> io::Result<RexPath> {
    if cancel.load(Ordering::Acquire) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
    }
    if bytes.len() > MAX_INLINE_WRITE_BYTES {
        if !unique {
            return Err(io::Error::other(format!("Payload ({} bytes) exceeds {} byte inline write limit.", bytes.len(), MAX_INLINE_WRITE_BYTES)));
        }
        let (parent_dir, name) = split_remote_parent_and_name(&rex.path).map_err(io::Error::other)?;
        let guard = write_staged_paste(stage_dir, bytes, cancel).map_err(|e| if e.kind() == io::ErrorKind::Interrupted { e } else { io::Error::other(format!("Couldn’t stage large paste: {e}")) })?;
        let local_path_str = require_utf8_path(&guard.path, "staged paste")?;
        let stream = UnixStream::connect(sock).map_err(|_| io::Error::other("Rex isn’t running. Open Rex to transfer files with remote hosts."))?;
        let req = json!({
            "op": "upload",
            "host": rex.host,
            "local_path": local_path_str,
            "remote_dest_dir": parent_dir,
            "remote_name": name,
            "is_dir": false,
            "default_mode": true,
        });
        let val = stream_transfer_on(stream, req, progress, cancel, cancel_wake, STREAM_IDLE_TIMEOUT)?;
        let host = val.get("host").and_then(Value::as_str).unwrap_or(&rex.host).to_owned();
        let path = val.get("path").and_then(Value::as_str).ok_or_else(|| io::Error::other("missing uploaded path"))?;
        return Ok(RexPath::new(host, path));
    }
    // 15 s slack over Rex's server upload deadline (120 s + 1 s per 20 KiB).
    let deadline = Duration::from_secs(135 + bytes.len() as u64 / 20_000);
    let stream = UnixStream::connect(sock).map_err(|_| io::Error::other("Rex isn’t running. Open Rex to browse files on remote hosts."))?;
    let val = call_on_stream_cancellable(
        stream,
        json!({
            "op": "write_file",
            "host": rex.host,
            "path": rex.path,
            "unique": unique,
            "base64": encode_base64(bytes),
        }),
        deadline,
        cancel,
        cancel_wake,
    )?;
    progress.done_bytes.fetch_add(bytes.len() as u64, Ordering::Relaxed);
    let host = val.get("host").and_then(Value::as_str).unwrap_or(&rex.host).to_owned();
    let path = val.get("path").and_then(Value::as_str).unwrap_or(&rex.path).to_owned();
    Ok(RexPath::new(host, path))
}

/// Download a file or folder from a Rex host to `local_dest_dir/local_name` via
/// Rex's `olo-rex-sync` transfer, returning completeness & endpoint metadata
/// so Move refuses source deletion against older Rex binaries.
pub fn download_checked(
    rex: &RexPath,
    local_dest_dir: &Path,
    local_name: &str,
    is_dir: bool,
    progress: &Progress,
    cancel: &AtomicBool,
    cancel_wake: Option<&CancelWake>,
) -> io::Result<DownloadResult> {
    let dest_str = require_utf8_path(local_dest_dir, "local destination")?;
    let req = json!({
        "op": "download",
        "host": rex.host,
        "remote_path": rex.path,
        "local_dest_dir": dest_str,
        "local_name": local_name,
        "is_dir": is_dir,
    });
    let val = stream_transfer(req, progress, cancel, cancel_wake)?;
    parse_download_result(&val)
}

fn parse_download_result(val: &Value) -> io::Result<DownloadResult> {
    let path = val.get("path").and_then(Value::as_str).ok_or_else(|| io::Error::other("missing downloaded path"))?;
    let complete = val.get("complete").and_then(Value::as_bool).unwrap_or(false);
    let endpoint_id = val.get("endpoint_id").and_then(Value::as_str).map(str::to_owned);
    Ok(DownloadResult { path: PathBuf::from(path), complete, endpoint_id })
}

/// Upload a local file or folder to `remote_dest_dir/remote_name` on a Rex host
/// via Rex's `olo-rex-sync` transfer, rejecting non-UTF-8 local paths and
/// returning completeness & endpoint metadata.
pub fn upload_checked(
    local_path: &Path,
    remote_dest_dir: &RexPath,
    remote_name: &str,
    is_dir: bool,
    progress: &Progress,
    cancel: &AtomicBool,
    cancel_wake: Option<&CancelWake>,
) -> io::Result<UploadResult> {
    let local_str = require_utf8_path(local_path, "local source")?;
    let req = json!({
        "op": "upload",
        "host": remote_dest_dir.host,
        "local_path": local_str,
        "remote_dest_dir": remote_dest_dir.path,
        "remote_name": remote_name,
        "is_dir": is_dir,
        "default_mode": false,
    });
    let val = stream_transfer(req, progress, cancel, cancel_wake)?;
    parse_upload_result(&val, &remote_dest_dir.host)
}

fn parse_upload_result(val: &Value, default_host: &str) -> io::Result<UploadResult> {
    let host = val.get("host").and_then(Value::as_str).unwrap_or(default_host).to_owned();
    let path = val.get("path").and_then(Value::as_str).ok_or_else(|| io::Error::other("missing uploaded path"))?;
    let complete = val.get("complete").and_then(Value::as_bool).unwrap_or(false);
    Ok(UploadResult { path: RexPath::new(host, path), complete })
}

fn stream_transfer(req: Value, progress: &Progress, cancel: &AtomicBool, cancel_wake: Option<&CancelWake>) -> io::Result<Value> {
    let sock = olo_kit_base::tracker::rex_files_socket();
    let stream = UnixStream::connect(&sock).map_err(|_| io::Error::other("Rex isn’t running. Open Rex to transfer files with remote hosts."))?;
    stream_transfer_on(stream, req, progress, cancel, cancel_wake, STREAM_IDLE_TIMEOUT)
}

fn stream_transfer_on(mut stream: UnixStream, req: Value, progress: &Progress, cancel: &AtomicBool, cancel_wake: Option<&CancelWake>, idle_timeout: Duration) -> io::Result<Value> {
    stream.set_nonblocking(true)?;
    let write_deadline = Instant::now() + Duration::from_secs(15);

    let mut line = serde_json::to_vec(&req).map_err(io::Error::other)?;
    drop(req);
    line.push(b'\n');
    // Preserve BrokenPipe/ConnectionReset and fall through to reading Rex's
    // JSON error frame (e.g. "Rex files server is busy; try again").
    let write_err = write_all_cancellable(&mut stream, &line, cancel, cancel_wake, write_deadline)?;
    drop(line);

    let mut writer = stream.try_clone()?;
    let sock_fd = stream.as_raw_fd();
    let mut reader = BufReader::new(stream);
    let mut partial_buf = Vec::new();
    let mut sent_cancel = false;
    let base_done = progress.done_bytes.load(Ordering::Relaxed);
    let mut last_activity = Instant::now();

    loop {
        if cancel.load(Ordering::Acquire) && !sent_cancel {
            sent_cancel = true;
            send_cancel_best_effort(&mut writer);
        }
        match read_bounded_line_into(&mut reader, &mut partial_buf, MAX_REPLY_LINE_BYTES) {
            Ok(None) => {
                if cancel.load(Ordering::Acquire) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                }
                return if let Some(we) = write_err { Err(io::Error::other(format!("Couldn’t reach Rex: {we}"))) } else { Err(io::Error::other("Rex closed the connection during transfer.")) };
            }
            Ok(Some(buf)) => {
                last_activity = Instant::now();
                let val: Value = serde_json::from_str(buf.trim()).map_err(|e| io::Error::other(format!("Unexpected response from Rex: {e}")))?;
                if val.get("event").and_then(Value::as_str) == Some("progress") {
                    if let Some(done) = val.get("done_bytes").and_then(Value::as_u64) {
                        progress.done_bytes.store(base_done.saturating_add(done), Ordering::Relaxed);
                        progress.counting.store(false, Ordering::Relaxed);
                    }
                    continue;
                }
                if val.get("ok").and_then(Value::as_bool) == Some(true) {
                    return Ok(val);
                }
                if val.get("cancelled").and_then(Value::as_bool) == Some(true) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                }
                let err = val.get("error").and_then(Value::as_str).unwrap_or("Remote transfer failed.");
                return Err(io::Error::other(err.to_owned()));
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => {
                let deadline = last_activity + idle_timeout;
                let watch = if sent_cancel { CancelWatch::AlreadySent } else { CancelWatch::Active(cancel, cancel_wake) };
                match poll_socket_or_cancel(sock_fd, libc::POLLIN, watch, deadline)? {
                    PollOutcome::Ready => continue,
                    PollOutcome::Cancelled => {
                        if !sent_cancel {
                            sent_cancel = true;
                            send_cancel_best_effort(&mut writer);
                        }
                    }
                    PollOutcome::TimedOut => {
                        send_cancel_best_effort(&mut writer);
                        let _ = writer.shutdown(Shutdown::Both);
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "Remote transfer timed out waiting for Rex."));
                    }
                }
            }
            Err(e) => {
                return if let Some(we) = write_err { Err(io::Error::other(format!("Couldn’t reach Rex: {we}"))) } else { Err(e) };
            }
        }
    }
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_encodes_rfc4648_vectors() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn split_remote_parent_and_name_handles_paths() {
        assert_eq!(split_remote_parent_and_name("/home/dev/pic.png").unwrap(), ("/home/dev".into(), "pic.png".into()));
        assert_eq!(split_remote_parent_and_name("/pic.png").unwrap(), ("/".into(), "pic.png".into()));
        assert_eq!(split_remote_parent_and_name("pic.png").unwrap(), ("~".into(), "pic.png".into()));
        assert_eq!(split_remote_parent_and_name("~/docs/pic.png").unwrap(), ("~/docs".into(), "pic.png".into()));
        assert!(split_remote_parent_and_name("/").is_err());
    }

    #[test]
    fn call_reads_server_error_after_deterministic_broken_pipe() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let _ = client.set_read_timeout(Some(Duration::from_secs(2)));
        // Queue the server's JSON error reply, then shut down the client's
        // write half BEFORE calling `call_on_stream` so `write_all` hits
        // `BrokenPipe` deterministically regardless of socket buffer size.
        server.write_all(b"{\"ok\":false,\"error\":\"request exceeds 67108864 byte inline limit\"}\n").unwrap();
        server.flush().unwrap();
        drop(server);
        client.shutdown(Shutdown::Write).unwrap();

        let err = call_on_stream(client, json!({ "op": "write_file", "base64": "AAAA" })).unwrap_err();
        assert!(err.contains("exceeds 67108864 byte inline limit"), "expected Rex JSON error, got: {err}");
    }

    #[test]
    fn read_bounded_line_rejects_oversize_replies_before_utf8() {
        let exact = format!("{}\n", "a".repeat(63));
        assert_eq!(read_bounded_line(&mut BufReader::new(exact.as_bytes()), 64).unwrap().as_deref(), Some(exact.as_str()));

        let over_utf8 = format!("{}é\n", "a".repeat(64));
        let err = read_bounded_line(&mut BufReader::new(over_utf8.as_bytes()), 64).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("exceeded"), "{err}");
    }

    #[test]
    fn write_staged_paste_arms_guard_before_write_and_uses_private_permissions() {
        let dir = PathBuf::from(format!("/var/tmp/olo-tracker-stage-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // 1. Cancel mid-write: partial staged file is deleted immediately by guard.
        let cancel_mid = AtomicBool::new(true);
        let err = write_staged_paste(&dir, &vec![b'x'; 512 * 1024], &cancel_mid).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "aborted staging must leave zero files behind");

        // 2. Concurrent pastes produce distinct 0600 files, stay locked against
        //    concurrent sweeper while alive, and clean up on drop.
        let ok_cancel = AtomicBool::new(false);
        let g1 = write_staged_paste(&dir, b"first", &ok_cancel).unwrap();
        let g2 = write_staged_paste(&dir, b"second", &ok_cancel).unwrap();
        assert_ne!(g1.path, g2.path);
        assert_eq!(std::fs::metadata(&g1.path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&g2.path).unwrap().permissions().mode() & 0o777, 0o600);

        // Age g1's mtime past STALE_STAGE_AGE while g1 is still live: sweeper
        // MUST NOT delete it because its advisory flock is held!
        let old_time = SystemTime::now() - Duration::from_secs(7200);
        let _ = std::fs::OpenOptions::new().write(true).open(&g1.path).unwrap().set_modified(old_time);
        // Also place an unlocked abandoned paste file with old mtime: sweeper MUST remove it.
        let abandoned = dir.join("paste-999999-0-12345.bin");
        std::fs::write(&abandoned, b"abandoned").unwrap();
        let _ = std::fs::OpenOptions::new().write(true).open(&abandoned).unwrap().set_modified(old_time);
        sweep_abandoned_paste_stages(&dir, STALE_STAGE_AGE);
        assert!(g1.path.exists(), "sweeper must NOT remove a live locked paste stage");
        assert!(!abandoned.exists(), "sweeper must remove unlocked abandoned paste stage");

        drop(g1);
        drop(g2);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stream_transfer_expires_silent_server_with_idle_timeout() {
        let (client, _silent_server) = UnixStream::pair().unwrap();
        let progress = Progress::default();
        let cancel = AtomicBool::new(false);
        let wake = CancelWake::new();
        let began = Instant::now();
        let err = stream_transfer_on(client, json!({ "op": "upload" }), &progress, &cancel, Some(&wake), Duration::from_millis(250)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
        assert!(began.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn split_json_reply_across_read_pause_preserves_partial_frame() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let progress = Progress::default();
        let cancel = AtomicBool::new(false);
        let wake = CancelWake::new();
        let t = std::thread::spawn(move || {
            let mut line = String::new();
            BufReader::new(server.try_clone().unwrap()).read_line(&mut line).unwrap();
            // Send first half of JSON reply, pause 260 ms (> old 200 ms SO_RCVTIMEO), then send rest.
            server.write_all(b"{\"ok\":true,\"complete\":true,\"path\":\"/home/dev/").unwrap();
            server.flush().unwrap();
            std::thread::sleep(Duration::from_millis(260));
            server.write_all(b"pic.png\",\"host\":\"dev-box\"}\n").unwrap();
            server.flush().unwrap();
        });
        let val = stream_transfer_on(client, json!({ "op": "upload" }), &progress, &cancel, Some(&wake), Duration::from_secs(5)).unwrap();
        t.join().unwrap();
        let parsed = parse_upload_result(&val, "dev-box").unwrap();
        assert!(parsed.complete);
        assert_eq!(parsed.path.path, "/home/dev/pic.png");
    }

    #[test]
    fn stop_during_silent_wait_wakes_immediately_via_eventfd_and_legacy_reply_is_unconfirmed() {
        use std::sync::Arc;
        // 1. CancelWake wakes poll_socket_or_cancel instantaneously (no 200 ms polling).
        let (client, _silent_server) = UnixStream::pair().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(CancelWake::new());
        let c2 = cancel.clone();
        let w2 = wake.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            c2.store(true, Ordering::Release);
            w2.wake();
        });
        let began = Instant::now();
        let res = poll_socket_or_cancel(client.as_raw_fd(), libc::POLLIN, CancelWatch::Active(&cancel, Some(&wake)), Instant::now() + Duration::from_secs(30)).unwrap();
        t.join().unwrap();
        assert_eq!(res, PollOutcome::Cancelled);
        assert!(began.elapsed() < Duration::from_secs(1), "CancelWake must wake immediately: {:?}", began.elapsed());

        // 2. Old Rex reply missing `"complete": true` yields `complete == false`
        //    so Tracker's Move leaves the source untouched.
        let legacy_dl = parse_download_result(&json!({ "ok": true, "path": "/var/tmp/x", "bytes": 5 })).unwrap();
        assert!(!legacy_dl.complete, "legacy Rex reply without complete flag must be treated as unconfirmed");
        let legacy_up = parse_upload_result(&json!({ "ok": true, "path": "/home/dev/x", "host": "dev-box", "bytes": 5 }), "dev-box").unwrap();
        assert!(!legacy_up.complete);

        // 3. Stop received before/during request transmission aborts immediately.
        let (mut c_wr, _s_wr) = UnixStream::pair().unwrap();
        c_wr.set_nonblocking(true).unwrap();
        let stop_early = AtomicBool::new(true);
        let wake_early = CancelWake::new();
        let w_err = write_all_cancellable(&mut c_wr, b"{\"op\":\"write_file\"}\n", &stop_early, Some(&wake_early), Instant::now() + Duration::from_secs(5)).unwrap_err();
        assert_eq!(w_err.kind(), io::ErrorKind::Interrupted);

        // 4. Non-UTF-8 local path is rejected rather than lossily matching a namesake sibling.
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let bad_dir = PathBuf::from(OsStr::from_bytes(b"/var/tmp/bad-\xff-dir"));
        let err = require_utf8_path(&bad_dir, "local source").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn large_paste_stages_privately_sets_default_mode_reports_progress_and_cleans_up() {
        use std::os::unix::net::UnixListener;
        let dir = PathBuf::from(format!("/var/tmp/olo-tracker-large-paste-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("files.sock");
        let stage_dir = dir.join("stage");
        let listener = UnixListener::bind(&sock).unwrap();

        let payload = vec![0x5Au8; MAX_INLINE_WRITE_BYTES + 1024];
        let expected_len = payload.len();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(conn.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let req: Value = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(req["op"], "upload");
            assert_eq!(req["remote_dest_dir"], "~/docs");
            assert_eq!(req["remote_name"], "big.png");
            assert_eq!(req["default_mode"], true);
            let local_staged = PathBuf::from(req["local_path"].as_str().unwrap());
            assert_eq!(std::fs::metadata(&local_staged).unwrap().len() as usize, expected_len);
            assert_eq!(std::fs::metadata(&local_staged).unwrap().permissions().mode() & 0o777, 0o600);
            conn.write_all(b"{\"event\":\"progress\",\"done_bytes\":1048576}\n").unwrap();
            conn.write_all(b"{\"ok\":true,\"complete\":true,\"host\":\"dev-box\",\"path\":\"/home/dev/docs/big.png\",\"bytes\":4195328}\n").unwrap();
            conn.flush().unwrap();
            local_staged
        });

        let progress = Progress::default();
        let cancel = AtomicBool::new(false);
        let wake = CancelWake::new();
        let made = write_file_to_socket_with_progress(&sock, &stage_dir, &RexPath::new("dev-box", "~/docs/big.png"), true, &payload, &progress, &cancel, Some(&wake)).unwrap();
        let staged_path = server.join().unwrap();
        assert_eq!(made.host, "dev-box");
        assert_eq!(made.path, "/home/dev/docs/big.png");
        assert_eq!(progress.done_bytes.load(Ordering::Relaxed), 1_048_576);
        assert!(!staged_path.exists(), "staged paste file must be removed immediately after upload");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_against_silent_peer_keeps_nonblocking_and_never_hangs() {
        use std::sync::Arc;
        // 1. call_on_stream_cancellable: cancelling while peer stays silent and open
        //    must keep O_NONBLOCK on the shared fd and time out cleanly at deadline!
        let (client, _silent_server) = UnixStream::pair().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(CancelWake::new());
        let c2 = cancel.clone();
        let w2 = wake.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            c2.store(true, Ordering::Release);
            w2.wake();
        });
        let began = Instant::now();
        let err = call_on_stream_cancellable(client, json!({ "op": "copy_remote" }), Duration::from_millis(250), &cancel, Some(&wake)).unwrap_err();
        t.join().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "must time out cleanly instead of hanging: {err}");
        assert!(began.elapsed() < Duration::from_secs(2), "hung past deadline: {:?}", began.elapsed());

        // 2. stream_transfer_on: cancelling while peer stays silent and open must
        //    also time out cleanly at idle_timeout instead of blocking forever!
        let (client2, _silent_server2) = UnixStream::pair().unwrap();
        let progress = Progress::default();
        let cancel2 = Arc::new(AtomicBool::new(false));
        let wake2 = Arc::new(CancelWake::new());
        let c3 = cancel2.clone();
        let w3 = wake2.clone();
        let t2 = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            c3.store(true, Ordering::Release);
            w3.wake();
        });
        let began2 = Instant::now();
        let err2 = stream_transfer_on(client2, json!({ "op": "upload" }), &progress, &cancel2, Some(&wake2), Duration::from_millis(250)).unwrap_err();
        t2.join().unwrap();
        assert_eq!(err2.kind(), io::ErrorKind::TimedOut, "must time out cleanly instead of hanging: {err2}");
        assert!(began2.elapsed() < Duration::from_secs(2), "hung past idle_timeout: {:?}", began2.elapsed());

        // 3. send_cancel_best_effort MUST preserve O_NONBLOCK on shared cloned fds.
        let (mut s_a, _s_b) = UnixStream::pair().unwrap();
        s_a.set_nonblocking(true).unwrap();
        let clone_a = s_a.try_clone().unwrap();
        send_cancel_best_effort(&mut s_a);
        let flags = unsafe { libc::fcntl(clone_a.as_raw_fd(), libc::F_GETFL) };
        assert_ne!(flags & libc::O_NONBLOCK, 0, "send_cancel_best_effort must never clear O_NONBLOCK");
    }

    #[test]
    fn stream_transfer_reads_server_busy_error_after_broken_pipe() {
        let (client, mut server) = UnixStream::pair().unwrap();
        server.write_all(b"{\"ok\":false,\"error\":\"Rex files server is busy; try again\"}\n").unwrap();
        server.flush().unwrap();
        drop(server);
        client.shutdown(Shutdown::Write).unwrap();

        let progress = Progress::default();
        let cancel = AtomicBool::new(false);
        let wake = CancelWake::new();
        let err = stream_transfer_on(client, json!({ "op": "download" }), &progress, &cancel, Some(&wake), Duration::from_secs(2)).unwrap_err();
        assert!(err.to_string().contains("busy; try again"), "expected Rex busy JSON error, got: {err}");
    }

    #[test]
    fn remove_expect_uses_lstat_for_symlinks_and_rejects_missing_or_unverified() {
        let make_stat = |exists: bool, is_dir: bool, is_link: bool, ep: Option<&str>| StatResult {
            host: "dev-box".into(),
            resolved: "/home/dev/link".into(),
            exists,
            entry: Some(Entry {
                path: RexPath::new("dev-box", "/home/dev/link").to_path_buf(),
                name: "link".into(),
                is_dir,
                is_link,
                broken: false,
                size: 999,
                modified: None,
                mode: 0o644,
                kind: kind_for(Path::new("link"), is_dir, 0o644),
                app_icon: None,
                downloading: None,
                shown: Shown::default(),
            }),
            endpoint_id: ep.map(str::to_owned),
            dev: Some(10),
            ino: Some(200),
            size: Some(999),
            mtime_secs: Some(1000),
            mtime_nanos: Some(1),
            ctime_secs: Some(1000),
            ctime_nanos: Some(2),
            lstat_dev: Some(10),
            lstat_ino: Some(777),
            lstat_size: Some(14),
            lstat_mtime_secs: Some(1500),
            lstat_mtime_nanos: Some(3),
            lstat_ctime_secs: Some(1500),
            lstat_ctime_nanos: Some(4),
        };

        // Valid symlink to directory: treated as symlink (!is_dir), uses lstat_* fields!
        let st_link_dir = make_stat(true, true, true, Some("v2|au=dev|h=box|k=abc"));
        let exp = RemoveExpect::from_stat(&st_link_dir).unwrap();
        assert!(!exp.is_dir, "symlink to dir must not be treated as a real directory");
        assert_eq!(exp.ino, 777, "must use lstat_ino instead of followed target ino");
        assert_eq!(exp.size, Some(14), "must use lstat_size for symlink");

        // exists: false rejected
        assert!(RemoveExpect::from_stat(&make_stat(false, false, false, Some("v2|au=dev|k=abc"))).is_err());
        // unverified endpoint rejected
        assert!(RemoveExpect::from_stat(&make_stat(true, false, false, Some("v2|u=dev|h=box|unverified"))).is_err());
        // missing lstat fields rejected
        let mut legacy = make_stat(true, false, false, Some("v2|au=dev|k=abc"));
        legacy.lstat_ino = None;
        assert!(RemoveExpect::from_stat(&legacy).is_err());
    }
}
