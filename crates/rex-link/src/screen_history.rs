//! `olo-rex-link screen history-stream NAME`: zero-polling history follower and
//! on-demand recorded-frame server (`frame_at`) for one Agent Screen.
//!
//! Architecture (single `poll`, zero polling at rest):
//! - Watches `$XDG_STATE_HOME/olo/screens/NAME` via `inotify` (watching the
//!   deepest existing ancestor with fixed-point step-down when the screen's
//!   folder does not yet exist, removing moved watches explicitly, verifying
//!   directory inodes, and rebuilding the inotify instance on `IN_Q_OVERFLOW`).
//! - Streams incremental `actions.jsonl` lines in bounded batches
//!   (`HistoryMsg::Actions`, with `reset: true` on initial load, clear,
//!   truncation, or inode replacement) and chunked incremental `.olof` frame
//!   index marks (`HistoryMsg::Index`, using a per-segment `(path, dev, ino)`
//!   cursor so appends never re-clone the full index and inode replacements
//!   never serve stale pixels).
//! - Reads NDJSON `{"op":"frame_at","req":N,"at":MS}` lines on stdin chunk by
//!   chunk, enforcing `MAX_HISTORY_REQ_LINE` immediately and keeping only the
//!   latest complete request plus one bounded partial line.
//! - Uses non-blocking cancellable stdout writes so a stalled reader never
//!   defeats stdin EOF or signal exit.
//! - Decodes frames with `olo_screen_codec::olof::OwnedPlayer` and encodes
//!   responses with `olo_screen_codec::tiles::encode_delta` (`PackedStream`,
//!   zstd level 1) after validating physical geometry against wire limits.
//! - Sleeps indefinitely in `poll(..., -1)` at rest (0 wakeups/s, 0% CPU,
//!   0 bytes/s); exits immediately on stdin EOF, broken stdout, or signal.

use std::ffi::CString;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{FileExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use olo_rex_link::screen_wire::{HistoryFrameMark, HistoryMsg, HistoryRequest, MAX_DIM, MAX_FRAME_PIXELS, MAX_HISTORY_REQ_LINE, encode_history_msg};
use olo_screen_codec::checked_net_frame_bytes;
use olo_screen_codec::olof::{Frame, OwnedPlayer, Segment, SegmentCache};
use olo_screen_codec::tiles::{TileEncoding, encode_delta};
use serde_json::Value;

use crate::screens::{CancelFd, OwnedFd, open_regular_at, open_regular_nonblock, open_verified_subdir, screens_state_dir, set_nonblocking, timeout_ms_ceil, valid_name, write_all_cancellable};

/// Brief coalesce window after `inotify` writes so multi-write `.olof` records
/// and rapid action bursts flush cleanly without extra messages, then return
/// immediately to infinite `poll(..., -1)`.
const COALESCE_DELAY: Duration = Duration::from_millis(25);

/// Bounded action batch limits per `HistoryMsg::Actions` message.
const ACTIONS_BATCH_BYTES: usize = 2 * 1024 * 1024;
const ACTIONS_BATCH_LINES: usize = 10_000;
/// Maximum size of a single JSON action line read intact (independently of the
/// 2 MiB batch read buffer, staying well under `MAX_HISTORY_ACTIONS_PAYLOAD`).
const MAX_ACTION_RECORD_BYTES: usize = 8 * 1024 * 1024;
/// Index-preserving placeholder emitted if a single malformed/giant action line
/// exceeds `MAX_ACTION_RECORD_BYTES`, so 0-based action indices never shift
/// relative to `olo-screen history --export --part` (Finding 45).
const OVERSIZED_ACTION_PLACEHOLDER: &str = r#"{"at":0,"ms":0,"kind":"oversized","args":[]}"#;

/// Bounded frame mark chunk size per `HistoryMsg::Index` message (~590 KiB),
/// so recordings exceeding 1,000,000 frames stream cleanly in reset+append chunks.
const INDEX_CHUNK_MARKS: usize = 65_536;

const DIR_MASK: u32 =
    libc::IN_CREATE | libc::IN_DELETE | libc::IN_MODIFY | libc::IN_CLOSE_WRITE | libc::IN_MOVED_FROM | libc::IN_MOVED_TO | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_ONLYDIR;

const ANCESTOR_MASK: u32 = libc::IN_CREATE | libc::IN_MOVED_TO | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_ONLYDIR;

fn dir_dev_ino(path: &Path) -> Option<(u64, u64)> {
    // Finding 6: never follow symlink screen directories when verifying watch identity.
    let meta = std::fs::symlink_metadata(path).ok()?;
    meta.file_type().is_dir().then_some((meta.dev(), meta.ino()))
}

fn is_permanent_watch_err(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(libc::ENOSPC | libc::EMFILE | libc::ENFILE | libc::ENOMEM | libc::EACCES | libc::EPERM))
}

/// Single-target `inotify` watcher with fixed-point ancestor step-down,
/// ancestor directory watching (up to 2 levels above `target` so moving
/// `$XDG_STATE_HOME/olo/screens` or `olo` never strands the follower, Finding 49),
/// directory `(dev, ino)` tracking, explicit removal of moved watches, and
/// full rebuild on `IN_Q_OVERFLOW`.
struct TargetWatch {
    ifd: OwnedFd,
    target: PathBuf,
    wd: Option<i32>,
    watched: Option<PathBuf>,
    dev_ino: Option<(u64, u64)>,
    want_child: Option<String>,
    /// Up to 2 existing parent directories above `watched` (`screens`, `olo`),
    /// stored as `(dir_path, expected_child_name, wd, dev_ino)`.
    ancestors: Vec<(PathBuf, String, i32, (u64, u64))>,
}

impl TargetWatch {
    fn new(target: PathBuf) -> io::Result<Self> {
        // SAFETY: creates a non-blocking, close-on-exec inotify descriptor.
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { ifd: OwnedFd::new(fd), target, wd: None, watched: None, dev_ino: None, want_child: None, ancestors: Vec::new() })
    }

    fn rebuild(&mut self) -> io::Result<()> {
        let fresh = Self::new(self.target.clone())?;
        self.ifd = fresh.ifd;
        self.wd = None;
        self.watched = None;
        self.dev_ino = None;
        self.want_child = None;
        self.ancestors.clear();
        Ok(())
    }

    fn fd(&self) -> i32 {
        self.ifd.raw()
    }

    fn is_at_target(&self) -> bool {
        self.want_child.is_none() && self.watched.as_deref() == Some(self.target.as_path())
    }

    fn clear_watch(&mut self) {
        if let Some(wd) = self.wd.take() {
            // SAFETY: `self.ifd` is valid and `wd` came from `inotify_add_watch`.
            unsafe {
                libc::inotify_rm_watch(self.ifd.raw(), wd);
            }
        }
        for (_, _, awd, _) in self.ancestors.drain(..) {
            // SAFETY: `self.ifd` is valid and `awd` came from `inotify_add_watch`.
            unsafe {
                libc::inotify_rm_watch(self.ifd.raw(), awd);
            }
        }
        self.watched = None;
        self.dev_ino = None;
        self.want_child = None;
    }

    /// Explicitly drop a moved/deleted watch descriptor from the kernel
    /// (unless `IN_IGNORED` already dropped it), clearing our active watch
    /// and any descendant watches when either the target or a watched ancestor
    /// was moved/deleted (Finding 49).
    fn handle_self_event(&mut self, wd: i32, already_ignored: bool) -> bool {
        if !already_ignored {
            // SAFETY: drop moved watch so old moved-away inodes never leak watches.
            unsafe {
                libc::inotify_rm_watch(self.ifd.raw(), wd);
            }
        }
        let hit_primary = self.wd == Some(wd);
        let hit_ancestor = self.ancestors.iter().any(|(_, _, awd, _)| *awd == wd);
        if hit_primary || hit_ancestor {
            if hit_primary {
                self.wd = None;
            }
            if hit_ancestor {
                self.ancestors.retain(|(_, _, awd, _)| *awd != wd);
            }
            self.clear_watch();
            true
        } else {
            false
        }
    }

    /// True if `wd` is one of our watched ancestor directories and `fname`
    /// matches the child component leading toward `self.target`.
    fn ancestor_event_matches(&self, wd: i32, fname: &str) -> bool {
        self.ancestors.iter().any(|(_, child, awd, _)| *awd == wd && (child.is_empty() || fname == child))
    }

    /// Desired ancestor directories above `watched` (`screens`, `olo`) that
    /// currently exist as real directories, up to 2 levels.
    fn desired_ancestors(&self, watched: &Path) -> Vec<(PathBuf, String)> {
        let mut out = Vec::with_capacity(2);
        let mut cur = watched;
        for _ in 0..2 {
            let Some(parent) = cur.parent() else { break };
            if parent.as_os_str().is_empty() || parent == Path::new("/") {
                break;
            }
            let child = cur.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if dir_dev_ino(parent).is_some() {
                out.push((parent.to_path_buf(), child));
            }
            cur = parent;
        }
        out
    }

    fn ancestors_current(&self, want_anc: &[(PathBuf, String)]) -> bool {
        if self.ancestors.len() != want_anc.len() {
            return false;
        }
        self.ancestors.iter().zip(want_anc.iter()).all(|((p, c, _, id), (wp, wc))| p == wp && c == wc && dir_dev_ino(p) == Some(*id))
    }

    /// Reconcile watch to `self.target` (or its deepest existing ancestor) PLUS
    /// up to 2 ancestor levels above it, looping to a fixed point to eliminate
    /// `mkdir -p` and ancestor-rename TOCTOU races (Finding 49).
    fn reconcile(&mut self) -> io::Result<bool> {
        let mut changed = false;
        let mut permanent_err: Option<io::Error> = None;
        for _ in 0..16 {
            let (want_path, want_child) = deepest_existing(&self.target);
            let Some(cur_id) = dir_dev_ino(&want_path) else {
                continue;
            };
            let want_anc = self.desired_ancestors(&want_path);
            if self.watched.as_ref() == Some(&want_path) && self.want_child == want_child && self.dev_ino == Some(cur_id) && self.ancestors_current(&want_anc) {
                if let Some(child) = &self.want_child
                    && !child.is_empty()
                    && want_path.join(child).is_dir()
                {
                    // Child appeared after our previous watch install; step down.
                } else {
                    break;
                }
            }
            self.clear_watch();
            let mask = if want_child.is_none() { DIR_MASK } else { ANCESTOR_MASK };
            let Ok(c) = CString::new(want_path.as_os_str().as_bytes()) else { break };
            // SAFETY: `self.ifd` is valid and `c` is NUL-terminated.
            let wd = unsafe { libc::inotify_add_watch(self.ifd.raw(), c.as_ptr(), mask) };
            if wd >= 0 {
                let after_id = dir_dev_ino(&want_path).unwrap_or(cur_id);
                self.wd = Some(wd);
                self.watched = Some(want_path.clone());
                self.dev_ino = Some(after_id);
                self.want_child = want_child.clone();
                changed = true;

                // Install watches on up to 2 ancestor levels (`screens`, `olo`) so
                // renaming an ancestor delivers `IN_MOVE_SELF` immediately.
                let mut ancestor_raced = false;
                for (ap, achild) in &want_anc {
                    let Some(aid_before) = dir_dev_ino(ap) else {
                        ancestor_raced = true;
                        break;
                    };
                    let Ok(ac) = CString::new(ap.as_os_str().as_bytes()) else { continue };
                    // SAFETY: `self.ifd` is valid and `ac` is NUL-terminated.
                    let awd = unsafe { libc::inotify_add_watch(self.ifd.raw(), ac.as_ptr(), ANCESTOR_MASK) };
                    if awd >= 0 {
                        let aid_after = dir_dev_ino(ap).unwrap_or(aid_before);
                        self.ancestors.push((ap.clone(), achild.clone(), awd, aid_after));
                        if aid_after != aid_before {
                            ancestor_raced = true;
                            break;
                        }
                    } else {
                        let err = io::Error::last_os_error();
                        if is_permanent_watch_err(&err) {
                            permanent_err = Some(err);
                            break;
                        }
                        ancestor_raced = true;
                        break;
                    }
                }
                if permanent_err.is_some() {
                    break;
                }
                if after_id != cur_id || ancestor_raced || dir_dev_ino(&want_path) != Some(after_id) {
                    continue;
                }
                // Close TOCTOU window where awaited child appeared between `is_dir()` and `inotify_add_watch`.
                if let Some(child) = &want_child
                    && !child.is_empty()
                    && want_path.join(child).is_dir()
                {
                    continue;
                }
                break;
            }
            let err = io::Error::last_os_error();
            if is_permanent_watch_err(&err) {
                permanent_err = Some(err);
                break;
            }
            // Directory disappeared mid-reconcile; loop to fall back to surviving ancestor.
        }
        if let Some(err) = permanent_err {
            return Err(err);
        }
        Ok(changed)
    }
}

fn deepest_existing(target: &Path) -> (PathBuf, Option<String>) {
    if std::fs::symlink_metadata(target).is_ok_and(|m| m.file_type().is_dir()) {
        return (target.to_path_buf(), None);
    }
    let mut cur = target;
    while let Some(parent) = cur.parent() {
        if parent.as_os_str().is_empty() {
            break;
        }
        if std::fs::symlink_metadata(parent).is_ok_and(|m| m.file_type().is_dir()) {
            let want_child = cur.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            return (parent.to_path_buf(), Some(want_child));
        }
        cur = parent;
    }
    (PathBuf::from("/"), Some(String::new()))
}

/// Incremental bounded `actions.jsonl` reader: opens relative to a verified
/// screen-directory fd (`openat` with `O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC` and
/// `is_file()` — never follows symlink screen dirs or blocks on FIFOs).
#[derive(Default)]
struct ActionsTail {
    dev_ino: Option<(u64, u64)>,
    offset: u64,
}

impl ActionsTail {
    /// Read one bounded batch of complete JSON object lines, returning
    /// `(lines, reset, has_more)`.
    fn next_batch(&mut self, dir_fd: Option<&OwnedFd>) -> (Vec<String>, bool, bool) {
        let Some(dfd) = dir_fd else {
            let reset = self.dev_ino.take().is_some();
            self.offset = 0;
            return (Vec::new(), reset, false);
        };
        let Ok((mut f, meta)) = open_regular_at(dfd, "actions.jsonl") else {
            let reset = self.dev_ino.take().is_some();
            self.offset = 0;
            return (Vec::new(), reset, false);
        };
        let id = (meta.dev(), meta.ino());
        let reset = self.dev_ino != Some(id) || meta.len() < self.offset;
        if reset {
            self.dev_ino = Some(id);
            self.offset = 0;
        }
        let remaining = meta.len().saturating_sub(self.offset);
        if remaining == 0 {
            return (Vec::new(), reset, false);
        }
        let to_read = remaining.min(ACTIONS_BATCH_BYTES as u64) as usize;
        let mut raw = vec![0u8; to_read];
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return (Vec::new(), reset, false);
        }
        let mut filled = 0usize;
        while filled < to_read {
            match f.read(&mut raw[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        raw.truncate(filled);
        // Only whole lines: a half-written trailing line waits for its `\n`.
        let whole = match memchr::memrchr(b'\n', &raw) {
            Some(i) => i + 1,
            None => {
                // Finding 45: if a single valid action line spans past the 2 MiB
                // batch buffer, keep reading up to `MAX_ACTION_RECORD_BYTES` so we
                // never drop a valid action or shift subsequent 0-based indices!
                if filled == ACTIONS_BATCH_BYTES && meta.len() > self.offset + filled as u64 {
                    let mut extra = vec![0u8; 256 * 1024];
                    loop {
                        let room = MAX_ACTION_RECORD_BYTES.saturating_sub(raw.len());
                        if room == 0 {
                            // Line exceeds `MAX_ACTION_RECORD_BYTES`: scan forward
                            // to its terminating `\n` without growing memory and
                            // emit an index-preserving placeholder!
                            let mut skipped = raw.len() as u64;
                            let mut discard = [0u8; 64 * 1024];
                            loop {
                                match f.read(&mut discard) {
                                    Ok(0) => return (Vec::new(), reset, false),
                                    Ok(n) => {
                                        if let Some(nl) = memchr::memchr(b'\n', &discard[..n]) {
                                            skipped += (nl + 1) as u64;
                                            self.offset += skipped;
                                            return (vec![OVERSIZED_ACTION_PLACEHOLDER.to_owned()], reset, meta.len() > self.offset);
                                        }
                                        skipped += n as u64;
                                    }
                                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                                    Err(_) => return (Vec::new(), reset, false),
                                }
                            }
                        }
                        let ask = extra.len().min(room);
                        match f.read(&mut extra[..ask]) {
                            Ok(0) => return (Vec::new(), reset, false),
                            Ok(n) => {
                                let base = raw.len();
                                raw.extend_from_slice(&extra[..n]);
                                if let Some(rel) = memchr::memchr(b'\n', &raw[base..]) {
                                    let end = base + rel + 1;
                                    self.offset += end as u64;
                                    let text = String::from_utf8_lossy(&raw[..end]);
                                    let trimmed = text.trim();
                                    let lines = if !trimmed.is_empty() && serde_json::from_str::<Value>(trimmed).ok().is_some_and(|v| v.is_object()) { vec![trimmed.to_owned()] } else { Vec::new() };
                                    return (lines, reset, meta.len() > self.offset);
                                }
                            }
                            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                            Err(_) => return (Vec::new(), reset, false),
                        }
                    }
                }
                return (Vec::new(), reset, false);
            }
        };
        let mut lines = Vec::new();
        let mut consumed = 0usize;
        for slice in raw[..whole].split_inclusive(|&b| b == b'\n') {
            if lines.len() >= ACTIONS_BATCH_LINES {
                break;
            }
            consumed += slice.len();
            let text = String::from_utf8_lossy(slice);
            let trimmed = text.trim();
            if !trimmed.is_empty() && serde_json::from_str::<Value>(trimmed).ok().is_some_and(|v| v.is_object()) {
                lines.push(trimmed.to_owned());
            }
        }
        self.offset += consumed as u64;
        let has_more = consumed < whole || (meta.len() > self.offset && filled == to_read);
        (lines, reset, has_more)
    }
}

fn setup_signals() -> Option<OwnedFd> {
    // SAFETY: single-threaded process signal setup before poll loop.
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
        let fd = libc::signalfd(-1, &set, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK);
        (fd >= 0).then(|| OwnedFd::new(fd))
    }
}

/// Drain all complete NDJSON lines from `buf`, enforcing `MAX_HISTORY_REQ_LINE`
/// immediately and returning the latest valid `HistoryRequest::FrameAt` if any.
fn drain_latest_request(buf: &mut Vec<u8>) -> Result<Option<(u64, u64)>, String> {
    let mut latest: Option<(u64, u64)> = None;
    let mut start = 0usize;
    while let Some(rel) = memchr::memchr(b'\n', &buf[start..]) {
        let end = start + rel;
        let line_len = end - start + 1;
        if line_len > MAX_HISTORY_REQ_LINE {
            return Err(format!("history request line exceeds {MAX_HISTORY_REQ_LINE} bytes"));
        }
        let mut line = &buf[start..end];
        if line.ends_with(b"\r") {
            line = &line[..line.len() - 1];
        }
        start = end + 1;
        if line.is_empty() {
            continue;
        }
        match serde_json::from_slice::<HistoryRequest>(line) {
            Ok(HistoryRequest::FrameAt { req, at }) => {
                latest = Some((req, at));
            }
            Err(e) => {
                return Err(format!("invalid history request: {e}"));
            }
        }
    }
    if start > 0 {
        buf.drain(..start);
    }
    if buf.len() > MAX_HISTORY_REQ_LINE {
        return Err(format!("history request line exceeds {MAX_HISTORY_REQ_LINE} bytes"));
    }
    Ok(latest)
}

fn send_history_res(msg: &HistoryMsg, cancels: &[CancelFd]) -> io::Result<()> {
    let (tag, payload) = encode_history_msg(msg)?;
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.push(tag);
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    write_all_cancellable(1, &frame, cancels)
}

fn send_history(msg: &HistoryMsg, cancels: &[CancelFd]) -> bool {
    send_history_res(msg, cancels).is_ok()
}

/// Emit `marks` in bounded `INDEX_CHUNK_MARKS` slices (first chunk carries
/// `reset`, subsequent chunks carry `reset: false`), so recordings with >1M
/// frames never exceed the wire message cap.
fn send_index_chunked(reset: bool, last_size: Option<(u32, u32)>, marks: &[HistoryFrameMark], cancels: &[CancelFd]) -> bool {
    if marks.is_empty() {
        return send_history(&HistoryMsg::Index { reset, last_size, frames: Vec::new() }, cancels);
    }
    for (idx, chunk) in marks.chunks(INDEX_CHUNK_MARKS).enumerate() {
        let is_reset = reset && idx == 0;
        if !send_history(&HistoryMsg::Index { reset: is_reset, last_size, frames: chunk.to_vec() }, cancels) {
            return false;
        }
    }
    true
}

type SegmentMeta = (PathBuf, usize);

struct RefreshResult {
    segs: Vec<Segment>,
    size: Option<(u32, u32)>,
    meta: Vec<SegmentMeta>,
    replaced: bool,
}

/// Scan `.olof` record headers in `path` (21 bytes per record, zero pixel
/// decompression) to check the dimensions of the frame at `target_at` and its
/// nearest preceding keyframe against wire bounds BEFORE calling
/// `OwnedPlayer::at` (Finding 15).
fn precheck_segment_headers(path: &Path, target_at: u64) -> Result<(), String> {
    let Ok((file, meta)) = open_regular_nonblock(path) else { return Ok(()) };
    let file_len = meta.len();
    let mut magic = [0u8; 8];
    if file.read_exact_at(&mut magic, 0).is_err() || &magic != b"OLOFRM1\n" {
        return Ok(());
    }
    let mut off = 8u64;
    let mut last_key_dims: Option<(usize, usize)> = None;
    let mut target_dims: Option<(usize, usize)> = None;
    let mut head = [0u8; 21];
    while off.saturating_add(21) <= file_len {
        if file.read_exact_at(&mut head, off).is_err() {
            break;
        }
        let rec_len = u32::from_le_bytes(head[0..4].try_into().unwrap()) as u64;
        let rec_at = u64::from_le_bytes(head[4..12].try_into().unwrap());
        let w = u32::from_le_bytes(head[12..16].try_into().unwrap()) as usize;
        let h = u32::from_le_bytes(head[16..20].try_into().unwrap()) as usize;
        let is_key = head[20] == 1;
        let Some(next) = off.checked_add(4).and_then(|o| o.checked_add(rec_len)) else { break };
        if rec_len < 21 || next > file_len {
            break;
        }
        if rec_at > target_at {
            break;
        }
        if is_key {
            last_key_dims = Some((w, h));
        }
        target_dims = Some((w, h));
        off = next;
    }
    if let Some((w, h)) = target_dims
        && checked_net_frame_bytes(w, h).is_err()
    {
        return Err(format!("recorded frame {w}×{h} exceeds wire size limits"));
    }
    if let Some((w, h)) = last_key_dims
        && checked_net_frame_bytes(w, h).is_err()
    {
        return Err(format!("recorded keyframe {w}×{h} exceeds wire size limits"));
    }
    Ok(())
}

/// Check the target `.olof` record (and its base keyframe) for `at` against
/// wire geometry limits BEFORE calling `OwnedPlayer::at` so an oversized local
/// recording (e.g. 23040×12960 = 1.11 GiB) is rejected without allocating a
/// pixel buffer (Finding 15).
fn precheck_frame_at_wire_bounds(segments: &[Segment], at: u64) -> Result<(), String> {
    let Some(seg) = segments.iter().rev().find(|s| s.frames.first().is_some_and(|f| f.at <= at)) else {
        return Ok(());
    };
    precheck_segment_headers(&seg.path, at)
}

/// Per-segment incremental `.olof` indexer backed by `olo_screen_codec::olof::SegmentCache`
/// (Findings 6, 19, 33, 34): opens only regular files inside the verified
/// screen directory fd, reads `last_size` directly from record headers, and
/// emits `last_size: None` when the last recording exceeds wire dimensions.
#[derive(Default)]
struct IndexTracker {
    cache: SegmentCache,
    sent_segs: Vec<SegmentMeta>,
    sent_size: Option<(u32, u32)>,
    last_mark_at: Option<u64>,
    index_gen: u64,
}

impl IndexTracker {
    fn clear_cache(&mut self) {
        self.cache = SegmentCache::new();
    }

    /// Incrementally refresh `.olof` segments inside the verified `screen_dir`
    /// (empty when the screen directory is absent, a symlink, or fails identity
    /// verification against `dir_fd`).
    fn refresh(&mut self, screen_dir: &Path, dir_fd: Option<&(OwnedFd, (u64, u64))>) -> RefreshResult {
        use std::os::unix::fs::MetadataExt as _;
        let Some((_, expected_id)) = dir_fd else {
            let replaced = !self.sent_segs.is_empty();
            self.cache = SegmentCache::new();
            return RefreshResult { segs: Vec::new(), size: None, meta: Vec::new(), replaced };
        };
        let still_same_dir = || std::fs::symlink_metadata(screen_dir).ok().is_some_and(|m| m.file_type().is_dir() && (m.dev(), m.ino()) == *expected_id);
        if !still_same_dir() {
            let replaced = !self.sent_segs.is_empty();
            self.cache = SegmentCache::new();
            return RefreshResult { segs: Vec::new(), size: None, meta: Vec::new(), replaced };
        }
        let scan = self.cache.scan(screen_dir);
        if !still_same_dir() {
            let replaced = !self.sent_segs.is_empty();
            self.cache = SegmentCache::new();
            return RefreshResult { segs: Vec::new(), size: None, meta: Vec::new(), replaced };
        }
        // Finding 33: if the last recorded frame exceeds wire limits (e.g.
        // 7680x4320@3x = 23040x12960), keep the timestamp index usable with
        // `last_size: None` instead of failing index encoding.
        let new_size = scan.last_size.filter(|&(w, h)| checked_net_frame_bytes(w, h).is_ok()).map(|(w, h)| (w as u32, h as u32));
        let cur_meta: Vec<SegmentMeta> = scan.segments.iter().map(|s| (s.path.clone(), s.frames.len())).collect();
        RefreshResult { segs: scan.segments, size: new_size, meta: cur_meta, replaced: scan.replaced }
    }

    /// Emit either an incremental tail append (O(delta)) or a chunked full reset
    /// directly from segments without copying all marks into an intermediate Vec.
    /// Returns `Some(was_reset)` on success, `None` on write/cancel failure.
    fn emit_diff(&mut self, segs: &[Segment], new_size: Option<(u32, u32)>, cur_meta: Vec<SegmentMeta>, force_reset: bool, cancels: &[CancelFd]) -> Option<bool> {
        let can_append = !force_reset && !self.sent_segs.is_empty() && cur_meta.len() >= self.sent_segs.len() && {
            let last_i = self.sent_segs.len() - 1;
            let prefix_same = self.sent_segs[..last_i].iter().zip(&cur_meta[..last_i]).all(|(a, b)| a == b);
            let tail_grew = cur_meta[last_i].0 == self.sent_segs[last_i].0 && cur_meta[last_i].1 >= self.sent_segs[last_i].1;
            prefix_same && tail_grew
        };

        let last_at = segs.last().and_then(|s| s.frames.last()).map(|f| f.at);
        if can_append {
            let last_i = self.sent_segs.len() - 1;
            let old_tail_len = self.sent_segs[last_i].1;
            let mut added = Vec::new();
            for f in &segs[last_i].frames[old_tail_len..] {
                added.push(HistoryFrameMark { at: f.at, key: f.key });
            }
            for seg in &segs[last_i + 1..] {
                for f in &seg.frames {
                    added.push(HistoryFrameMark { at: f.at, key: f.key });
                }
            }
            if !added.is_empty() || new_size != self.sent_size {
                if !send_index_chunked(false, new_size, &added, cancels) {
                    return None;
                }
                self.sent_segs = cur_meta;
                self.sent_size = new_size;
                self.last_mark_at = last_at;
            }
            return Some(false);
        }

        if !force_reset && cur_meta == self.sent_segs && new_size == self.sent_size {
            return Some(false);
        }

        // Finding 34: stream reset marks in bounded chunks directly from
        // `segs` without building a full `all_marks` vector in memory.
        let mut chunk = Vec::with_capacity(INDEX_CHUNK_MARKS);
        let mut sent_any = false;
        for seg in segs {
            for f in &seg.frames {
                chunk.push(HistoryFrameMark { at: f.at, key: f.key });
                if chunk.len() >= INDEX_CHUNK_MARKS {
                    if !send_history(&HistoryMsg::Index { reset: !sent_any, last_size: new_size, frames: std::mem::take(&mut chunk) }, cancels) {
                        return None;
                    }
                    sent_any = true;
                }
            }
        }
        if (!sent_any || !chunk.is_empty()) && !send_history(&HistoryMsg::Index { reset: !sent_any, last_size: new_size, frames: chunk }, cancels) {
            return None;
        }
        self.sent_segs = cur_meta;
        self.sent_size = new_size;
        self.last_mark_at = last_at;
        self.index_gen += 1;
        Some(true)
    }
}

fn emit_actions_all(actions_tail: &mut ActionsTail, dir_fd: Option<&OwnedFd>, force_initial_reset: bool, cancels: &[CancelFd]) -> Option<bool> {
    let mut first = true;
    let mut any_reset = force_initial_reset;
    loop {
        let (lines, reset, has_more) = actions_tail.next_batch(dir_fd);
        let send_reset = (first && force_initial_reset) || reset;
        any_reset |= send_reset;
        if (send_reset || !lines.is_empty()) && !send_history(&HistoryMsg::Actions { reset: send_reset, lines }, cancels) {
            return None;
        }
        first = false;
        if !has_more {
            break;
        }
    }
    Some(any_reset)
}

pub fn run(args: &[String]) -> ! {
    if args.first().is_none_or(|a| a == "--help" || a == "-h") {
        println!("usage: olo-rex-link screen history-stream NAME");
        std::process::exit(if args.is_empty() { 2 } else { 0 });
    }
    let name = &args[0];
    if !valid_name(name) {
        let _ = olo_rex_link::screen_wire::write_history_msg(&mut io::stdout().lock(), &HistoryMsg::Error { message: format!("invalid screen name {name:?}") });
        std::process::exit(2);
    }
    std::process::exit(history_loop(name))
}

fn history_loop(name: &str) -> i32 {
    let sigfd = setup_signals();
    set_nonblocking(0);
    set_nonblocking(1);
    let mut cancels = vec![CancelFd::StdinEof(0)];
    if let Some(s) = &sigfd {
        cancels.push(CancelFd::Readable(s.raw()));
    }

    let state_root = screens_state_dir();
    let screen_dir = state_root.join(name);

    let mut watch = match TargetWatch::new(screen_dir.clone()) {
        Ok(w) => w,
        Err(e) => {
            let _ = send_history(&HistoryMsg::Error { message: format!("inotify: {e}") }, &cancels);
            return 1;
        }
    };
    // Install watches BEFORE the initial scan so no append slips past startup.
    if let Err(e) = watch.reconcile() {
        let _ = send_history(&HistoryMsg::Error { message: format!("inotify watch failed: {e}") }, &cancels);
        return 1;
    }

    let mut actions_tail = ActionsTail::default();
    let mut tracker = IndexTracker::default();
    let mut actions_gen: u64 = 0;

    // Finding 6: pin `screens/NAME` via verified `openat(O_DIRECTORY | O_NOFOLLOW)`.
    let mut dir_fd: Option<(OwnedFd, (u64, u64))> = open_verified_subdir(&state_root, name).ok().and_then(|fd| {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        (unsafe { libc::fstat(fd.raw(), &mut st) } == 0).then_some((fd, (st.st_dev, st.st_ino)))
    });

    // Initial snapshot: always emit Actions(reset=true), Index(reset=true), and
    // SnapshotDone so reconnect deduplication knows the complete snapshot boundary.
    if emit_actions_all(&mut actions_tail, dir_fd.as_ref().map(|(f, _)| f), true, &cancels).is_none() {
        return 0;
    }
    actions_gen += 1;

    let init = tracker.refresh(&screen_dir, dir_fd.as_ref());
    if tracker.emit_diff(&init.segs, init.size, init.meta, true, &cancels).is_none() {
        return 0;
    }
    if !send_history(&HistoryMsg::SnapshotDone { actions_gen, index_gen: tracker.index_gen }, &cancels) {
        return 0;
    }

    let mut player = OwnedPlayer::new(init.segs);
    let mut prev_sent_frame: Option<Frame> = None;

    let mut inotify_buf = vec![0u8; 32 * 1024];
    let mut stdin_buf: Vec<u8> = Vec::with_capacity(1024);
    let mut stdin_chunk = [0u8; 4096];

    let mut actions_dirty = false;
    let mut olof_dirty = false;
    let mut flush_at: Option<Instant> = None;
    let mut pending_req: Option<(u64, u64)> = None;

    let refresh_dir_fd = |dir_fd: &mut Option<(OwnedFd, (u64, u64))>| -> bool {
        let next = open_verified_subdir(&state_root, name).ok().and_then(|fd| {
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            (unsafe { libc::fstat(fd.raw(), &mut st) } == 0).then_some((fd, (st.st_dev, st.st_ino)))
        });
        let changed = dir_fd.as_ref().map(|(_, id)| *id) != next.as_ref().map(|(_, id)| *id);
        if changed {
            *dir_fd = next;
        }
        changed
    };

    let flush_updates = |actions_dirty: &mut bool,
                         olof_dirty: &mut bool,
                         actions_tail: &mut ActionsTail,
                         tracker: &mut IndexTracker,
                         player: &mut OwnedPlayer,
                         dir_fd: &mut Option<(OwnedFd, (u64, u64))>,
                         actions_gen: &mut u64|
     -> bool {
        let dir_changed = refresh_dir_fd(dir_fd);
        if dir_changed {
            tracker.clear_cache();
            player.reset();
        }
        let mut any_reset = false;
        if *actions_dirty || dir_changed {
            *actions_dirty = false;
            match emit_actions_all(actions_tail, dir_fd.as_ref().map(|(f, _)| f), false, &cancels) {
                Some(true) => {
                    *actions_gen += 1;
                    any_reset = true;
                }
                Some(false) => {}
                None => return false,
            }
        }
        if *olof_dirty || dir_changed {
            *olof_dirty = false;
            let refreshed = tracker.refresh(&screen_dir, dir_fd.as_ref());
            match tracker.emit_diff(&refreshed.segs, refreshed.size, refreshed.meta, refreshed.replaced || dir_changed, &cancels) {
                Some(true) => any_reset = true,
                Some(false) => {}
                None => return false,
            }
            if refreshed.replaced || dir_changed {
                player.reset();
            }
            player.update(refreshed.segs);
        }
        !any_reset || send_history(&HistoryMsg::SnapshotDone { actions_gen: *actions_gen, index_gen: tracker.index_gen }, &cancels)
    };

    loop {
        // Compute poll timeout: 0 if a request is ready to decode, ceiling-rounded
        // coalesce window if disk writes just happened (never spinning on sub-ms
        // remainders), else -1 (sleep forever).
        let timeout_ms: i32 = if pending_req.is_some() {
            0
        } else if let Some(deadline) = flush_at {
            timeout_ms_ceil(deadline.saturating_duration_since(Instant::now()))
        } else {
            -1
        };

        let mut pfds = Vec::with_capacity(4);
        pfds.push(libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 });
        // Finding 35: poll stdout with `events: 0` so closing the output reader
        // wakes the idle loop immediately via `POLLERR | POLLHUP`.
        pfds.push(libc::pollfd { fd: 1, events: 0, revents: 0 });
        pfds.push(libc::pollfd { fd: watch.fd(), events: libc::POLLIN, revents: 0 });
        let sig_idx = sigfd.as_ref().map(|s| {
            let idx = pfds.len();
            pfds.push(libc::pollfd { fd: s.raw(), events: libc::POLLIN, revents: 0 });
            idx
        });

        // SAFETY: `pfds` holds valid file descriptors for the duration of `poll`.
        let n = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, timeout_ms) };
        if n < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return 1;
        }

        // 1. Signal received or stdout closed: exit cleanly.
        if let Some(idx) = sig_idx
            && pfds[idx].revents != 0
        {
            return 0;
        }
        if pfds[1].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return 0;
        }

        // 2. Read stdin in bounded chunks, parsing after EVERY chunk so
        //    `stdin_buf` stays <= `MAX_HISTORY_REQ_LINE` and overtaken requests
        //    are dropped immediately.
        if pfds[0].revents != 0 {
            let mut saw_eof = false;
            for _ in 0..16 {
                // SAFETY: non-blocking read from stdin into `stdin_chunk`.
                let got = unsafe { libc::read(0, stdin_chunk.as_mut_ptr().cast(), stdin_chunk.len()) };
                if got > 0 {
                    stdin_buf.extend_from_slice(&stdin_chunk[..got as usize]);
                    match drain_latest_request(&mut stdin_buf) {
                        Ok(Some(req)) => pending_req = Some(req),
                        Ok(None) => {}
                        Err(message) => {
                            let _ = send_history(&HistoryMsg::Error { message }, &cancels);
                            return 0;
                        }
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
                return 0;
            }
            if pfds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 && pending_req.is_none() {
                return 0;
            }
        }

        // 3. Drain inotify events.
        let mut need_reconcile = false;
        if pfds[2].revents != 0 {
            loop {
                // SAFETY: non-blocking read from `inotify` descriptor.
                let got = unsafe { libc::read(watch.fd(), inotify_buf.as_mut_ptr().cast(), inotify_buf.len()) };
                if got <= 0 {
                    break;
                }
                let mut at = 0usize;
                let mut rebuilt = false;
                while at + std::mem::size_of::<libc::inotify_event>() <= got as usize {
                    // SAFETY: kernel writes complete `inotify_event` structs into `inotify_buf`.
                    let ev = unsafe { std::ptr::read_unaligned(inotify_buf.as_ptr().add(at).cast::<libc::inotify_event>()) };
                    let name_at = at + std::mem::size_of::<libc::inotify_event>();
                    let name_end = (name_at + ev.len as usize).min(got as usize);
                    let fname = std::ffi::CStr::from_bytes_until_nul(&inotify_buf[name_at..name_end]).ok().and_then(|c| c.to_str().ok()).unwrap_or("");
                    at = name_end;

                    if ev.mask & libc::IN_Q_OVERFLOW != 0 {
                        if let Err(e) = watch.rebuild() {
                            let _ = send_history(&HistoryMsg::Error { message: format!("inotify rebuild: {e}") }, &cancels);
                            return 1;
                        }
                        need_reconcile = true;
                        actions_dirty = true;
                        olof_dirty = true;
                        tracker.clear_cache();
                        rebuilt = true;
                        break;
                    }
                    if ev.mask & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_IGNORED) != 0 {
                        let already_ignored = ev.mask & libc::IN_IGNORED != 0;
                        if watch.handle_self_event(ev.wd, already_ignored) {
                            need_reconcile = true;
                            actions_dirty = true;
                            olof_dirty = true;
                            tracker.clear_cache();
                        }
                        continue;
                    }
                    if watch.ancestor_event_matches(ev.wd, fname) {
                        need_reconcile = true;
                        actions_dirty = true;
                        olof_dirty = true;
                        continue;
                    }
                    if watch.wd != Some(ev.wd) {
                        continue;
                    }
                    if watch.is_at_target() {
                        if fname == "actions.jsonl" {
                            actions_dirty = true;
                        } else if fname.ends_with(".olof") {
                            olof_dirty = true;
                        }
                    } else if let Some(want) = &watch.want_child
                        && (want.is_empty() || fname == want)
                    {
                        need_reconcile = true;
                    }
                }
                if rebuilt {
                    break;
                }
            }
        }

        if need_reconcile {
            match watch.reconcile() {
                Ok(true) => {
                    actions_dirty = true;
                    olof_dirty = true;
                }
                Ok(false) => {}
                Err(e) => {
                    let _ = send_history(&HistoryMsg::Error { message: format!("inotify watch failed: {e}") }, &cancels);
                    return 1;
                }
            }
        }

        // Arm the brief coalesce timer on new disk events; flush immediately if
        // a `frame_at` request is waiting or the coalesce window elapsed.
        if (actions_dirty || olof_dirty) && flush_at.is_none() {
            flush_at = Some(Instant::now() + COALESCE_DELAY);
        }
        let should_flush = (actions_dirty || olof_dirty) && (pending_req.is_some() || flush_at.is_some_and(|d| Instant::now() >= d));
        if should_flush {
            flush_at = None;
            if !flush_updates(&mut actions_dirty, &mut olof_dirty, &mut actions_tail, &mut tracker, &mut player, &mut dir_fd, &mut actions_gen) {
                return 0;
            }
        }

        // 4. Answer the latest pending `frame_at` request (if any).
        if let Some((req, at)) = pending_req.take() {
            // If the caller asks past what we have indexed (or before any frames),
            // refresh once in case a write beat inotify delivery.
            let past_known = tracker.last_mark_at.is_none_or(|last| at > last);
            if past_known && watch.is_at_target() {
                olof_dirty = true;
                if !flush_updates(&mut actions_dirty, &mut olof_dirty, &mut actions_tail, &mut tracker, &mut player, &mut dir_fd, &mut actions_gen) {
                    return 0;
                }
            }
            // Finding 15 & 33: inspect the target `.olof` header BEFORE decoding
            // pixels so a 23040x12960 recording returns a per-request FrameError
            // without allocating 1.11 GiB or killing the history stream!
            if let Err(msg) = precheck_frame_at_wire_bounds(player.segments(), at) {
                if !send_history(&HistoryMsg::FrameError { req, message: msg }, &cancels) {
                    return 0;
                }
                continue;
            }
            match player.at(at) {
                Some((when, frame)) => {
                    if frame.width > MAX_DIM as usize || frame.height > MAX_DIM as usize || frame.width.saturating_mul(frame.height) > MAX_FRAME_PIXELS {
                        if !send_history(&HistoryMsg::FrameError { req, message: format!("recorded frame {}×{} exceeds wire size limits", frame.width, frame.height) }, &cancels) {
                            return 0;
                        }
                        continue;
                    }
                    let delta = match encode_delta(frame, prev_sent_frame.as_ref(), TileEncoding::PackedStream, 1) {
                        Ok(d) => d,
                        Err(e) => {
                            if !send_history(&HistoryMsg::FrameError { req, message: format!("encode frame: {e}") }, &cancels) {
                                return 0;
                            }
                            continue;
                        }
                    };
                    let olot = delta.encode_bytes();
                    match send_history_res(&HistoryMsg::Frame { req, frame_at: when, delta: olot }, &cancels) {
                        Ok(()) => {}
                        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                            if !send_history(&HistoryMsg::FrameError { req, message: format!("recorded frame exceeds wire limit: {e}") }, &cancels) {
                                return 0;
                            }
                            continue;
                        }
                        Err(_) => return 0,
                    }
                    // Reuse `prev_sent_frame` pixel buffer when dimensions match.
                    match &mut prev_sent_frame {
                        Some(prev) if prev.width == frame.width && prev.height == frame.height => {
                            prev.rgba.copy_from_slice(&frame.rgba);
                        }
                        slot => {
                            *slot = Some(frame.clone());
                        }
                    }
                }
                None => {
                    if !send_history(&HistoryMsg::Miss { req }, &cancels) {
                        return 0;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drains_multiple_requests_keeping_only_latest() {
        let mut buf = b"{\"op\":\"frame_at\",\"req\":1,\"at\":100}\n{\"op\":\"frame_at\",\"req\":2,\"at\":200}\n{\"op\":\"frame_at\",\"req\":3,\"at\":300}\n{\"op\":\"frame".to_vec();
        assert_eq!(drain_latest_request(&mut buf).unwrap(), Some((3, 300)));
        assert_eq!(buf, b"{\"op\":\"frame");
    }

    #[test]
    fn rejects_oversized_history_request_line() {
        let mut buf = vec![b'x'; MAX_HISTORY_REQ_LINE + 1];
        assert!(drain_latest_request(&mut buf).is_err());
    }
}
