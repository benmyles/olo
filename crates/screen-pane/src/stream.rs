//! [`StreamScreenSource`]: [`ScreenSource`] for an Agent Screen running on a
//! Rex host, speaking the host helper's binary live, history, and export
//! protocols (`olo_rex_link::screen_wire`) plus one-shot lifecycle commands
//! over any transport implemented by a [`StreamOpener`].
//!
//! Nothing here knows SSH or QUIC: whoever owns the host connection (Rex's
//! hub, or Agent Screens talking over Rex's socket) implements
//! [`StreamOpener`].
//!
//! Architecture (zero polling at rest, latest picture/frame wins, ordered input):
//! - **Live (`watch_live`):**
//!   - `stream-s{id}` opens the live stream via [`StreamOpener::open_live`],
//!     reads length- and geometry-checked `ServerMsg`s via bounded wire readers,
//!     applies `OLOT` tile deltas onto the screen's RGBA canvas via
//!     `olo_screen_codec::tiles::apply_delta`, hands the latest picture to the
//!     single-slot delivery thread, and immediately enqueues `Ack { seq }`.
//!     Reconnects with exponential backoff on transport breaks; stops with plain
//!     words on `ScreenGone` or any protocol violation.
//!   - `stream-w{id}` owns the write half, sleeps on a condvar at rest (0
//!     wakeups/s), and writes `Ack`, `Pause`, `Control`, and `Input` in order
//!     (coalescing consecutive `Move` events via `run_together` and dropping
//!     input whenever control is not held).
//!   - `stream-d{id}` delivers events from a single-slot `Rgba` queue so a busy
//!     pane never builds a frame backlog or stalls acks.
//! - **History & `frame_at` (`follow_history`, `frame_at`, `cancel_frame_at`):**
//!   - Opened lazily by [`crate::ScreenPane`] only while History or Stats is
//!     shown or an export runs (`lazy_history() == true`, controlled via
//!     [`HistoryHandle::set_active`]), and closed cleanly with its handle.
//!   - `hist-s{id}` maintains the host `screen history` stream with zero
//!     polling at rest, computes search texts off the UI thread, maintains
//!     `FrameIndex` incrementally, and deduplicates the host's reconnect
//!     snapshot against already-delivered actions so a transport break never
//!     loses or doubles an action.
//!   - `hist-w{id}` writes `HistoryRequest::FrameAt` from a single-slot latest
//!     queue. Any `frame_at` overtaken by a newer `frame_at` or
//!     [`ScreenSource::cancel_frame_at`] unblocks immediately without waiting
//!     for the host, while the reader still applies every arriving
//!     `HistoryMsg::Frame` delta onto its persistent history canvas so the
//!     host's per-connection delta chain stays bit-exact.
//! - **Export (`export_video`):**
//!   - Opens `screen export` via [`StreamOpener::open_export`], forwards host
//!     progress, writes chunks to a sibling `.part` file, verifies `meta.bytes`,
//!     and renames atomically on `Done`. Returning `false` from `progress`,
//!     receiving an error, or dropping cancels the host stream immediately and
//!     unlinks the `.part` file so nothing half-written remains on disk.
//! - **Lifecycle (`start`, `stop`, `rename`, `clear_history`, `archive`, `delete`):**
//!   - Executed via [`StreamOpener::run_command`], surfacing the helper's own
//!     words whenever it refuses.

use std::{
    collections::VecDeque,
    ffi::CString,
    io::{self, BufReader, BufWriter, Read, Write},
    os::unix::ffi::OsStrExt as _,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use olo_protocol::SyntheticEvent;
use olo_rex_link::screen_wire::{
    ClientMsg, ExportMeta, ExportMsg, HistoryRequest, MAX_DIM, MAX_EXPORT_PART_RANGES, MAX_EXPORT_PART_SPEC_LEN, MAX_HISTORY_ACTION_LINES, MAX_HISTORY_INDEX_FRAMES, MSG_CONTROL_STATE,
    MSG_EXPORT_CHUNK, MSG_EXPORT_DONE, MSG_EXPORT_ERROR, MSG_EXPORT_META, MSG_EXPORT_PROGRESS, MSG_FRAME, MSG_HISTORY_ACTIONS, MSG_HISTORY_ERROR, MSG_HISTORY_FRAME, MSG_HISTORY_FRAME_ERROR,
    MSG_HISTORY_INDEX, MSG_HISTORY_MISS, MSG_HISTORY_SNAPSHOT_DONE, MSG_SCREEN_GONE, MSG_STATUS, MSG_WARNING, max_export_payload_for, max_history_payload_for, max_server_payload_for,
    write_client_msg, write_history_request,
};
use olo_screen_codec::{
    Frame, FrameIndex, FrameMark,
    tiles::{EncodedDelta, apply_delta},
};
use olo_sync::{Condvar, Mutex};
use serde::Deserialize;

use crate::{
    export::{self, ExclusivePartFile},
    history::Action,
    local::run_together,
    picture::Shot,
    screens,
    source::{ExportCancel, HistoryEvent, HistoryHandle, LiveEvent, LiveHandle, LiveStatus, PaneCapabilities, ScreenId, ScreenInfo, ScreenSource, ScreenState},
};

/// Chunk increment used by bounded wire readers so a lying payload length
/// header never triggers a single multi-megabyte upfront allocation.
const WIRE_READ_CHUNK: usize = 64 << 10;
/// Shared write-half + cancel-hook slot torn down cleanly by either an explicit
/// `ExportCancel` callback or a RAII scope guard.
type TeardownPair = (Option<Box<dyn Write + Send>>, Option<Box<dyn FnOnce() + Send>>);
type TeardownSlot = Arc<Mutex<Option<TeardownPair>>>;
/// Maximum total bytes accepted for a single desktop export download (2 GiB).
const DEFAULT_MAX_DESKTOP_EXPORT_BYTES: u64 = 2 << 30;
/// Minimum free disk space preserved on the export destination volume (256 MiB).
const DEFAULT_EXPORT_FREE_RESERVE: u64 = 256 << 20;
/// Default maximum retained actions kept in memory for a remote history stream.
const DEFAULT_MAX_RETAINED_ACTIONS: usize = 50_000;
/// Default maximum retained serialized action bytes for a remote history stream.
const DEFAULT_MAX_RETAINED_ACTION_BYTES: usize = 16 << 20;
/// Default maximum retained frame marks kept in memory for a remote history stream.
const DEFAULT_MAX_RETAINED_FRAMES: usize = 250_000;
/// Default response deadline for remote `frame_at` / `frame_at_pinned` calls.
const DEFAULT_FRAME_AT_TIMEOUT: Duration = Duration::from_secs(10);
/// Bounded wait for host control-release acknowledgement before remote `stop()`.
const RELEASE_ACK_TIMEOUT: Duration = Duration::from_secs(2);
/// Maximum queued outgoing input events on the desktop live writer.
const MAX_QUEUED_INPUT_EVENTS: usize = 1024;
/// Maximum estimated queued outgoing input bytes on the desktop live writer.
const MAX_QUEUED_INPUT_BYTES: usize = 256 << 10;
/// Maximum events per outgoing `ClientMsg::Input` wire message.
const MAX_WIRE_INPUT_EVENTS: usize = 64;
/// Maximum JSON bytes per outgoing `ClientMsg::Input` wire message (safely
/// below `screen_wire::MAX_CLIENT_PAYLOAD = 256 KiB`).
const MAX_WIRE_INPUT_BYTES: usize = 240 << 10;
/// Minimum bytes in a valid `OLOT` payload.
const OLOT_MIN_BYTES: usize = 24;

fn wire_invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn map_delta_reader_err(e: String) -> io::Error {
    if e.contains("failed to fill whole buffer") || e.contains("unexpected end of file") || e.contains("broken pipe") || e.contains("connection reset") || e.contains("timed out") {
        io::Error::new(io::ErrorKind::UnexpectedEof, e)
    } else {
        wire_invalid(e)
    }
}

/// Read `len` exact bytes incrementally in [`WIRE_READ_CHUNK`] steps so a
/// header claiming many megabytes on a stalled/truncated stream never reserves
/// the whole advertised buffer upfront.
fn read_exact_chunked<R: Read>(r: &mut R, len: usize) -> io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(len.min(WIRE_READ_CHUNK));
    let mut scratch = [0u8; 8192];
    let mut rem = len;
    while rem > 0 {
        let want = rem.min(scratch.len());
        let n = r.read(&mut scratch[..want])?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "unexpected EOF while reading framed message"));
        }
        out.extend_from_slice(&scratch[..n]);
        rem -= n;
    }
    Ok(out)
}

/// Live `MSG_STATUS` JSON payload (superset of `olo_rex_link::screen_wire::StreamStatus`,
/// supporting optional explicit control acknowledgements and non-fatal capture warnings
/// while remaining wire-compatible with unchanged hosts).
#[derive(Debug, Deserialize)]
struct WireStatus {
    #[serde(default)]
    message: String,
    #[serde(default)]
    reconnecting: bool,
    #[serde(default)]
    error: bool,
    #[serde(default)]
    control: Option<bool>,
    #[serde(default)]
    warning: bool,
}

#[derive(Debug, Deserialize)]
struct WireControlState {
    held: bool,
    #[serde(default)]
    seq: u64,
    #[serde(default)]
    reason: Option<String>,
}

enum DecodedServerMsg {
    Frame { seq: u64, delta: EncodedDelta },
    Status(WireStatus),
    ScreenGone { reason: String },
    Control { held: bool, seq: u64, reason: Option<String> },
    Warning { message: String },
}

/// Prefix-first bounded reader for the live stream: validates tag, length, and
/// `OLOT` header + canonical tile table *before* allocating pixel data, never
/// pre-allocating 258 MiB or double-copying the delta buffer.
fn read_server_msg_bounded<R: Read>(r: &mut R) -> io::Result<Option<DecodedServerMsg>> {
    let mut tag = [0u8; 1];
    match r.read(&mut tag)? {
        0 => return Ok(None),
        1 => {}
        _ => unreachable!(),
    }
    let mut len_bytes = [0u8; 4];
    r.read_exact(&mut len_bytes)?;
    let len = u32::from_le_bytes(len_bytes);
    let max_len = max_server_payload_for(tag[0])?;
    if len > max_len {
        return Err(wire_invalid(format!("server msg 0x{:02x} payload length {len} exceeds {max_len}", tag[0])));
    }
    match tag[0] {
        MSG_FRAME => {
            let total = len as usize;
            if total < 8 + OLOT_MIN_BYTES {
                return Err(wire_invalid("truncated frame message"));
            }
            let mut seq_b = [0u8; 8];
            r.read_exact(&mut seq_b)?;
            let seq = u64::from_le_bytes(seq_b);
            let delta = EncodedDelta::decode_from_reader(r, total - 8).map_err(map_delta_reader_err)?;
            Ok(Some(DecodedServerMsg::Frame { seq, delta }))
        }
        MSG_STATUS => {
            let payload = read_exact_chunked(r, len as usize)?;
            let status: WireStatus = serde_json::from_slice(&payload).map_err(|e| wire_invalid(format!("invalid status JSON: {e}")))?;
            Ok(Some(DecodedServerMsg::Status(status)))
        }
        MSG_SCREEN_GONE => {
            let payload = read_exact_chunked(r, len as usize)?;
            let reason = std::str::from_utf8(&payload).map_err(|e| wire_invalid(format!("invalid screen gone UTF-8: {e}")))?.to_owned();
            Ok(Some(DecodedServerMsg::ScreenGone { reason }))
        }
        MSG_CONTROL_STATE => {
            let payload = read_exact_chunked(r, len as usize)?;
            let ctl: WireControlState = serde_json::from_slice(&payload).map_err(|e| wire_invalid(format!("invalid control state JSON: {e}")))?;
            Ok(Some(DecodedServerMsg::Control { held: ctl.held, seq: ctl.seq, reason: ctl.reason }))
        }
        MSG_WARNING => {
            let payload = read_exact_chunked(r, len as usize)?;
            let message = std::str::from_utf8(&payload).map_err(|e| wire_invalid(format!("invalid warning UTF-8: {e}")))?.to_owned();
            Ok(Some(DecodedServerMsg::Warning { message }))
        }
        other => Err(wire_invalid(format!("unknown server message type 0x{other:02x}"))),
    }
}

enum DecodedHistoryMsg {
    Actions { reset: bool, actions: Vec<Action>, raw_bytes: usize },
    Index { reset: bool, last_size: Option<(usize, usize)>, frames: Vec<FrameMark> },
    Frame { req: u64, frame_at: u64, delta: EncodedDelta },
    Miss { req: u64 },
    Error { message: String },
    FrameError { req: u64, message: String },
    SnapshotDone { actions_gen: u64, index_gen: u64 },
}

/// Prefix-first bounded reader for the history stream: validates every header
/// before allocating, validates monotonic frame marks and non-overflowing action
/// end timestamps (`at + ms`), and streams `OLOT` frames directly into
/// [`EncodedDelta::decode_from_reader`].
fn read_history_msg_bounded<R: Read>(r: &mut R) -> io::Result<Option<DecodedHistoryMsg>> {
    let mut tag = [0u8; 1];
    match r.read(&mut tag)? {
        0 => return Ok(None),
        1 => {}
        _ => unreachable!(),
    }
    let mut len_bytes = [0u8; 4];
    r.read_exact(&mut len_bytes)?;
    let len = u32::from_le_bytes(len_bytes);
    let max_len = max_history_payload_for(tag[0])?;
    if len > max_len {
        return Err(wire_invalid(format!("history msg 0x{:02x} payload length {len} exceeds {max_len}", tag[0])));
    }
    match tag[0] {
        MSG_HISTORY_ACTIONS => {
            if len == 0 {
                return Err(wire_invalid("empty history actions payload"));
            }
            let mut flag = [0u8; 1];
            r.read_exact(&mut flag)?;
            let reset = match flag[0] {
                0 => false,
                1 => true,
                v => return Err(wire_invalid(format!("invalid history actions reset flag {v}"))),
            };
            let body_bytes = read_exact_chunked(r, (len - 1) as usize)?;
            let body = std::str::from_utf8(&body_bytes).map_err(|e| wire_invalid(format!("invalid history actions UTF-8: {e}")))?;
            let mut actions = Vec::new();
            let mut line_count = 0usize;
            for line in body.lines() {
                if line.is_empty() {
                    continue;
                }
                line_count += 1;
                if line_count > MAX_HISTORY_ACTION_LINES {
                    return Err(wire_invalid(format!("history actions line count exceeds {MAX_HISTORY_ACTION_LINES}")));
                }
                if let Ok(a) = serde_json::from_str::<Action>(line) {
                    if a.at.checked_add(a.ms).is_none() {
                        return Err(wire_invalid("history action timestamp + duration overflows u64"));
                    }
                    actions.push(a);
                }
            }
            Ok(Some(DecodedHistoryMsg::Actions { reset, actions, raw_bytes: body_bytes.len() }))
        }
        MSG_HISTORY_INDEX => {
            let total = len as usize;
            if total < 13 {
                return Err(wire_invalid("truncated history index header"));
            }
            let mut hdr = [0u8; 13];
            r.read_exact(&mut hdr)?;
            let reset = match hdr[0] {
                0 => false,
                1 => true,
                v => return Err(wire_invalid(format!("invalid history index reset flag {v}"))),
            };
            let w = u32::from_le_bytes(hdr[1..5].try_into().unwrap());
            let h = u32::from_le_bytes(hdr[5..9].try_into().unwrap());
            let last_size = match (w, h) {
                (0, 0) => None,
                (w, h) if w > 0 && h > 0 && w <= MAX_DIM && h <= MAX_DIM => Some((w as usize, h as usize)),
                _ => return Err(wire_invalid(format!("invalid history index dimensions {w}x{h}"))),
            };
            let count = u32::from_le_bytes(hdr[9..13].try_into().unwrap()) as usize;
            if count > MAX_HISTORY_INDEX_FRAMES {
                return Err(wire_invalid(format!("history index count {count} exceeds {MAX_HISTORY_INDEX_FRAMES}")));
            }
            let expected = 13usize.checked_add(count.checked_mul(9).ok_or_else(|| wire_invalid("history index overflow"))?).ok_or_else(|| wire_invalid("history index overflow"))?;
            if total != expected {
                return Err(wire_invalid(format!("history index payload {total} != expected {expected}")));
            }
            let mut frames = Vec::with_capacity(count.min(4096));
            let mut prev_at: Option<u64> = None;
            let mut rem = count;
            let mut buf = [0u8; 9 * 1024];
            while rem > 0 {
                let batch = rem.min(1024);
                let byte_len = batch * 9;
                r.read_exact(&mut buf[..byte_len])?;
                for chunk in buf[..byte_len].as_chunks::<9>().0 {
                    let at = u64::from_le_bytes(chunk[0..8].try_into().unwrap());
                    let key = match chunk[8] {
                        0 => false,
                        1 => true,
                        v => return Err(wire_invalid(format!("invalid history key flag {v}"))),
                    };
                    if let Some(prev) = prev_at
                        && at < prev
                    {
                        return Err(wire_invalid(format!("history index timestamps out of order ({at} < {prev})")));
                    }
                    prev_at = Some(at);
                    frames.push(FrameMark { at, key });
                }
                rem -= batch;
            }
            Ok(Some(DecodedHistoryMsg::Index { reset, last_size, frames }))
        }
        MSG_HISTORY_FRAME => {
            let total = len as usize;
            if total < 16 + OLOT_MIN_BYTES {
                return Err(wire_invalid("truncated history frame message"));
            }
            let mut hdr = [0u8; 16];
            r.read_exact(&mut hdr)?;
            let req = u64::from_le_bytes(hdr[0..8].try_into().unwrap());
            let frame_at = u64::from_le_bytes(hdr[8..16].try_into().unwrap());
            let delta = EncodedDelta::decode_from_reader(r, total - 16).map_err(map_delta_reader_err)?;
            Ok(Some(DecodedHistoryMsg::Frame { req, frame_at, delta }))
        }
        MSG_HISTORY_MISS => {
            if len != 8 {
                return Err(wire_invalid(format!("history miss length must be 8, got {len}")));
            }
            let mut b = [0u8; 8];
            r.read_exact(&mut b)?;
            Ok(Some(DecodedHistoryMsg::Miss { req: u64::from_le_bytes(b) }))
        }
        MSG_HISTORY_ERROR => {
            let payload = read_exact_chunked(r, len as usize)?;
            let message = std::str::from_utf8(&payload).map_err(|e| wire_invalid(format!("invalid history error UTF-8: {e}")))?.to_owned();
            Ok(Some(DecodedHistoryMsg::Error { message }))
        }
        MSG_HISTORY_FRAME_ERROR => {
            if len < 8 {
                return Err(wire_invalid("truncated history frame error"));
            }
            let mut req_b = [0u8; 8];
            r.read_exact(&mut req_b)?;
            let req = u64::from_le_bytes(req_b);
            let payload = read_exact_chunked(r, (len - 8) as usize)?;
            let message = std::str::from_utf8(&payload).map_err(|e| wire_invalid(format!("invalid history frame error UTF-8: {e}")))?.to_owned();
            Ok(Some(DecodedHistoryMsg::FrameError { req, message }))
        }
        MSG_HISTORY_SNAPSHOT_DONE => {
            if len != 16 {
                return Err(wire_invalid(format!("history snapshot_done length must be 16, got {len}")));
            }
            let mut b = [0u8; 16];
            r.read_exact(&mut b)?;
            let actions_gen = u64::from_le_bytes(b[0..8].try_into().unwrap());
            let index_gen = u64::from_le_bytes(b[8..16].try_into().unwrap());
            Ok(Some(DecodedHistoryMsg::SnapshotDone { actions_gen, index_gen }))
        }
        other => Err(wire_invalid(format!("unknown history message type 0x{other:02x}"))),
    }
}

#[derive(Deserialize)]
struct ExportProgressJson {
    done: f32,
}

fn read_export_msg_bounded<R: Read>(r: &mut R) -> io::Result<Option<ExportMsg>> {
    let mut tag = [0u8; 1];
    match r.read(&mut tag)? {
        0 => return Ok(None),
        1 => {}
        _ => unreachable!(),
    }
    let mut len_bytes = [0u8; 4];
    r.read_exact(&mut len_bytes)?;
    let len = u32::from_le_bytes(len_bytes);
    let max_len = max_export_payload_for(tag[0])?;
    if len > max_len {
        return Err(wire_invalid(format!("export msg 0x{:02x} payload length {len} exceeds {max_len}", tag[0])));
    }
    let payload = read_exact_chunked(r, len as usize)?;
    match tag[0] {
        MSG_EXPORT_PROGRESS => {
            let p: ExportProgressJson = serde_json::from_slice(&payload).map_err(|e| wire_invalid(format!("invalid export progress JSON: {e}")))?;
            if !p.done.is_finite() {
                return Err(wire_invalid("export progress is not finite"));
            }
            Ok(Some(ExportMsg::Progress { done: p.done.clamp(0.0, 1.0) }))
        }
        MSG_EXPORT_META => {
            let meta: ExportMeta = serde_json::from_slice(&payload).map_err(|e| wire_invalid(format!("invalid export meta JSON: {e}")))?;
            Ok(Some(ExportMsg::Meta(meta)))
        }
        MSG_EXPORT_CHUNK => {
            if payload.is_empty() {
                return Err(wire_invalid("empty export chunk"));
            }
            Ok(Some(ExportMsg::Chunk(payload)))
        }
        MSG_EXPORT_DONE => Ok(Some(ExportMsg::Done)),
        MSG_EXPORT_ERROR => {
            let message = std::str::from_utf8(&payload).map_err(|e| wire_invalid(format!("invalid export error UTF-8: {e}")))?.to_owned();
            Ok(Some(ExportMsg::Error { message }))
        }
        other => Err(wire_invalid(format!("unknown export message type 0x{other:02x}"))),
    }
}

/// Check available disk space on `dir`'s volume before allocating/writing an
/// export download so a peer cannot fill the destination filesystem.
fn check_volume_space(dir: &Path, need_bytes: u64, reserve_bytes: u64) -> Result<(), String> {
    let c = CString::new(dir.as_os_str().as_bytes()).map_err(|_| "invalid destination path".to_owned())?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path and `st` is valid writable memory.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return Err(format!("couldn't check free space on {}: {}", dir.display(), io::Error::last_os_error()));
    }
    let block = if st.f_frsize > 0 { st.f_frsize as u64 } else { st.f_bsize as u64 };
    let avail = (st.f_bavail as u64).saturating_mul(block);
    let required = need_bytes.saturating_add(reserve_bytes);
    if avail < required {
        return Err(format!(
            "not enough free space to save export ({} MiB needed plus {} MiB reserve, {} MiB available)",
            need_bytes.div_ceil(1 << 20),
            reserve_bytes.div_ceil(1 << 20),
            avail / (1 << 20)
        ));
    }
    Ok(())
}

/// One opened bidirectional stream channel (read half, write half, and an
/// optional cancellation hook that unblocks any blocking read/write immediately
/// when closed or cancelled).
pub struct LiveStream {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    /// Optional out-of-band close/shutdown callback (e.g. shutting down a
    /// socket or resetting a QUIC/SSH stream) so dropping a handle or
    /// cancelling an export unblocks a pending `reader.read()` immediately
    /// rather than waiting for remote EOF.
    pub cancel: Option<Box<dyn FnOnce() + Send>>,
}

impl LiveStream {
    pub fn new(reader: impl Read + Send + 'static, writer: impl Write + Send + 'static) -> LiveStream {
        LiveStream { reader: Box::new(reader), writer: Box::new(writer), cancel: None }
    }

    pub fn with_cancel(reader: impl Read + Send + 'static, writer: impl Write + Send + 'static, cancel: impl FnOnce() + Send + 'static) -> LiveStream {
        LiveStream { reader: Box::new(reader), writer: Box::new(writer), cancel: Some(Box::new(cancel)) }
    }
}

/// Output of one host helper command (`olo-rex-link screen <args...>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.status == 0
    }

    /// The helper's own refusal words when `status != 0`.
    pub fn error_words(&self) -> String {
        let err = self.stderr.trim();
        if !err.is_empty() {
            return err.to_owned();
        }
        let out = self.stdout.trim();
        if !out.is_empty() {
            return out.to_owned();
        }
        format!("host screen command failed (exit {})", self.status)
    }
}

/// Transport-neutral opener for a Rex host's screen channels and commands.
///
/// Implemented by Rex's connection hub (QUIC or SSH) and by Agent Screens'
/// Rex socket client; nothing in `crates/screen-pane` knows SSH or QUIC.
pub trait StreamOpener: Send + Sync {
    /// Open a live stream (`olo-rex-link screen stream NAME`).
    ///
    /// `cancelled` turns `true` if the viewer stops waiting while the channel
    /// is being opened; implementations should abort promptly when set.
    fn open_live(&self, name: &str, cancelled: Arc<AtomicBool>) -> Result<LiveStream, String>;

    /// Open a history & `frame_at` stream (`olo-rex-link screen history-stream NAME`).
    fn open_history(&self, name: &str, cancelled: Arc<AtomicBool>) -> Result<LiveStream, String>;

    /// Open an MP4 export stream (`olo-rex-link screen export NAME [args...]`).
    /// Closing the returned stream's write half (or firing its `cancel` hook)
    /// tells the host to cancel the export and clean up immediately.
    fn open_export(&self, name: &str, args: &[String], cancelled: Arc<AtomicBool>) -> Result<LiveStream, String>;

    /// Run a single one-shot helper command (`olo-rex-link screen <args...>`),
    /// returning its exit status, stdout, and stderr.
    fn run_command(&self, args: &[String], cancelled: Arc<AtomicBool>) -> Result<CommandOutput, String>;
}

/// Reconnect backoff schedule used by [`StreamScreenSource`] when a live or
/// history stream breaks unexpectedly. Once all delays are exhausted without
/// recovery, the stream gives up and reports the error instead of hammering an
/// offline host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconnectPolicy {
    pub delays: Vec<Duration>,
}

impl Default for ReconnectPolicy {
    fn default() -> ReconnectPolicy {
        // 5 retries over ~7.75 s (250 ms → 4 s), then give up cleanly.
        ReconnectPolicy { delays: vec![Duration::from_millis(250), Duration::from_millis(500), Duration::from_secs(1), Duration::from_secs(2), Duration::from_secs(4)] }
    }
}

type FrameReply = Result<Option<(u64, Frame)>, String>;

/// Wire and CLI capabilities advertised once per host by
/// `olo-rex-link screen --protocol`.
///
/// An older helper that exits non-zero on `--protocol` or omits
/// `"capabilities"` parses as [`HostCapabilities::legacy()`] (all optional
/// capabilities `false`), keeping every existing flow working safely without
/// sending unknown wire tags (`ClientMsg::ControlSeq`) or unsupported CLI flags
/// (`--expected-started`, discontiguous `--part`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostCapabilities {
    /// Protocol version reported in `"proto"` (`0` for older helpers without `--protocol`).
    pub proto: u32,
    /// Advertised wire subcommands (`"commands"` array, e.g. `["follow", "stream", "history-stream", "export"]`).
    pub commands: Vec<String>,
    /// `"control-ack"`: live stream speaks `ClientMsg::ControlSeq { on, seq }` (`0x14`)
    /// and authoritative `ServerMsg::Control { held, seq, reason }` (`0x04`).
    pub control_ack: bool,
    /// `"warning-status"`: live stream emits non-fatal `ServerMsg::Warning` (`0x05`).
    pub warning_status: bool,
    /// `"history-frame-error"`: history stream emits per-request `HistoryMsg::FrameError` (`0x25`).
    pub history_frame_error: bool,
    /// `"history-snapshot-done"`: history stream closes every initial/reset snapshot
    /// with `HistoryMsg::SnapshotDone { actions_gen, index_gen }` (`0x26`).
    pub history_snapshot_done: bool,
    /// `"export-part-ranges"`: helper `screen export --part` accepts multiple ranges.
    pub export_part_ranges: bool,
    /// `"discontiguous_part"`: host `bin/olo-screen` also supports `--part`, so
    /// discontiguous exports succeed end-to-end.
    pub discontiguous_part: bool,
    /// `"stop-expected-started"`: `screen stop NAME --expected-started <u64>` checks
    /// session generation before stopping.
    pub stop_expected_started: bool,
}

#[derive(Deserialize)]
struct WireProtocol {
    #[serde(default)]
    proto: u32,
    #[serde(default)]
    commands: Vec<String>,
    #[serde(default)]
    capabilities: Vec<String>,
}

impl HostCapabilities {
    /// Capabilities of an older host helper that advertises none of the new
    /// wire extensions.
    pub fn legacy() -> HostCapabilities {
        HostCapabilities::default()
    }

    /// Parse the JSON emitted by `olo-rex-link screen --protocol`.
    pub fn parse_protocol_json(raw: &str) -> Result<HostCapabilities, String> {
        let wire: WireProtocol = serde_json::from_str(raw.trim()).map_err(|e| format!("invalid --protocol JSON: {e}"))?;
        let has = |names: &[&str]| wire.capabilities.iter().any(|c| names.contains(&c.as_str()));
        Ok(HostCapabilities {
            proto: wire.proto,
            commands: wire.commands,
            control_ack: has(&["control-ack", "control_ack"]),
            warning_status: has(&["warning-status", "warning_status"]),
            history_frame_error: has(&["history-frame-error", "history_frame_error"]),
            history_snapshot_done: has(&["history-snapshot-done", "history_snapshot_done"]),
            export_part_ranges: has(&["export-part-ranges", "export_part_ranges"]),
            discontiguous_part: has(&["discontiguous_part", "discontiguous-part"]),
            stop_expected_started: has(&["stop-expected-started", "stop_expected_started"]),
        })
    }

    /// Derive a [`HostCapabilities`] snapshot from an already-configured
    /// [`PaneCapabilities`] (used when a caller constructs a `StreamScreenSource`
    /// directly without a shared [`HostProtocolCache`]).
    pub fn from_pane(caps: &PaneCapabilities) -> HostCapabilities {
        HostCapabilities {
            proto: if caps.explicit_control_ack || caps.history_snapshot_done || caps.stop_expected_started || caps.can_export_discontiguous { 1 } else { 0 },
            commands: Vec::new(),
            control_ack: caps.explicit_control_ack,
            warning_status: caps.warning_status,
            history_frame_error: caps.history_frame_error,
            history_snapshot_done: caps.history_snapshot_done,
            export_part_ranges: caps.can_export_discontiguous,
            discontiguous_part: caps.can_export_discontiguous,
            stop_expected_started: caps.stop_expected_started,
        }
    }

    /// Query `olo-rex-link screen --protocol` once via `opener`, falling back
    /// cleanly to [`HostCapabilities::legacy()`] when an older helper rejects
    /// `--protocol` or returns non-JSON output.
    pub fn probe(opener: &dyn StreamOpener) -> HostCapabilities {
        match opener.run_command(&["--protocol".to_owned()], Arc::new(AtomicBool::new(false))) {
            Ok(out) if out.success() => Self::parse_protocol_json(&out.stdout).unwrap_or_else(|_| Self::legacy()),
            _ => Self::legacy(),
        }
    }

    /// Whether the helper advertises the dedicated `history-stream` verb
    /// (`olo-rex-link screen history-stream NAME`).
    pub fn has_history_stream(&self) -> bool {
        self.commands.iter().any(|c| c == "history-stream")
    }

    /// Apply these wire/CLI capabilities onto a screen's [`PaneCapabilities`]
    /// without overriding `can_control` (which also depends on host `olo-screen`
    /// lock support).
    pub fn apply_to(&self, caps: &mut PaneCapabilities) {
        caps.explicit_control_ack = self.control_ack;
        caps.warning_status = self.warning_status;
        caps.history_frame_error = self.history_frame_error;
        caps.history_snapshot_done = self.history_snapshot_done;
        caps.can_export_discontiguous = self.discontiguous_part;
        caps.stop_expected_started = self.stop_expected_started;
    }

    /// Build [`PaneCapabilities`] for a screen on a host with these probed
    /// wire/CLI capabilities.
    pub fn to_pane_capabilities(&self, can_control: bool, control_disabled_reason: Option<String>) -> PaneCapabilities {
        let mut caps = PaneCapabilities::remote_default();
        caps.can_control = can_control;
        caps.control_disabled_reason = if can_control { None } else { control_disabled_reason.or(caps.control_disabled_reason) };
        self.apply_to(&mut caps);
        caps
    }
}

enum ProtocolCacheState {
    Unprobed,
    Probing,
    Cached(HostCapabilities),
}

/// Per-host cache that reads `olo-rex-link screen --protocol` at most once per
/// host (coalescing concurrent callers) and keeps the parsed
/// [`HostCapabilities`] for every [`StreamScreenSource`] on that host.
pub struct HostProtocolCache {
    state: Mutex<ProtocolCacheState>,
    cvar: Condvar,
}

impl Default for HostProtocolCache {
    fn default() -> HostProtocolCache {
        Self::new()
    }
}

impl HostProtocolCache {
    /// Create an empty cache that will probe `screen --protocol` once on first
    /// use from a worker thread.
    pub fn new() -> HostProtocolCache {
        HostProtocolCache { state: Mutex::new(ProtocolCacheState::Unprobed), cvar: Condvar::new() }
    }

    /// Create a cache already populated with `caps` (zero `--protocol` calls).
    pub fn fixed(caps: HostCapabilities) -> HostProtocolCache {
        HostProtocolCache { state: Mutex::new(ProtocolCacheState::Cached(caps)), cvar: Condvar::new() }
    }

    /// Non-blocking read of already-cached capabilities (`None` if unprobed).
    pub fn get(&self) -> Option<HostCapabilities> {
        match &*self.state.lock().unwrap_or_else(|e| e.into_inner()) {
            ProtocolCacheState::Cached(c) => Some(c.clone()),
            ProtocolCacheState::Unprobed | ProtocolCacheState::Probing => None,
        }
    }

    /// Return cached host capabilities, or probe `olo-rex-link screen --protocol`
    /// once via `opener` and keep the result for all screens on this host.
    ///
    /// An older helper returning a non-zero exit status or malformed JSON is
    /// cached as [`HostCapabilities::legacy()`] so it is never re-probed.
    /// A transport error (`Err(_)`) returns [`HostCapabilities::legacy()`]
    /// without poisoning the cache so a subsequent connection can still probe.
    pub fn get_or_probe(&self, opener: &dyn StreamOpener) -> HostCapabilities {
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                match &*st {
                    ProtocolCacheState::Cached(c) => return c.clone(),
                    ProtocolCacheState::Probing => {
                        st = self.cvar.wait(st).unwrap_or_else(|e| e.into_inner());
                    }
                    ProtocolCacheState::Unprobed => {
                        *st = ProtocolCacheState::Probing;
                        break;
                    }
                }
            }
        }
        struct ResetOnDrop<'a> {
            cache: &'a HostProtocolCache,
            done: bool,
        }
        impl Drop for ResetOnDrop<'_> {
            fn drop(&mut self) {
                if !self.done {
                    let mut st = self.cache.state.lock().unwrap_or_else(|e| e.into_inner());
                    if matches!(*st, ProtocolCacheState::Probing) {
                        *st = ProtocolCacheState::Unprobed;
                    }
                    self.cache.cvar.notify_all();
                }
            }
        }
        let mut guard = ResetOnDrop { cache: self, done: false };
        match opener.run_command(&["--protocol".to_owned()], Arc::new(AtomicBool::new(false))) {
            Ok(out) => {
                let caps = if out.success() { HostCapabilities::parse_protocol_json(&out.stdout).unwrap_or_else(|_| HostCapabilities::legacy()) } else { HostCapabilities::legacy() };
                let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                *st = ProtocolCacheState::Cached(caps.clone());
                guard.done = true;
                self.cvar.notify_all();
                caps
            }
            Err(_) => {
                let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                *st = ProtocolCacheState::Unprobed;
                guard.done = true;
                self.cvar.notify_all();
                HostCapabilities::legacy()
            }
        }
    }
}

/// [`ScreenSource`] backed by a Rex host's `olo-rex-link screen` streams and
/// commands through a transport-neutral [`StreamOpener`].
pub struct StreamScreenSource {
    id: ScreenId,
    info: Arc<Mutex<ScreenInfo>>,
    /// Running session's start timestamp (seconds since epoch) used for
    /// generation-checked `stop --expected-started <u64>`.
    expected_started: Arc<Mutex<Option<u64>>>,
    opener: Arc<dyn StreamOpener>,
    protocol_cache: Arc<HostProtocolCache>,
    reconnect: ReconnectPolicy,
    active: Arc<Mutex<Option<Arc<StreamControl>>>>,
    history: Arc<Mutex<Option<Arc<HistoryControl>>>>,
    max_export_bytes: u64,
    export_free_reserve: u64,
    max_retained_actions: usize,
    max_retained_action_bytes: usize,
    max_retained_frames: usize,
    frame_at_timeout: Duration,
}

impl StreamScreenSource {
    pub fn new(info: ScreenInfo, opener: Arc<dyn StreamOpener>) -> StreamScreenSource {
        Self::with_reconnect(info, opener, ReconnectPolicy::default())
    }

    pub fn with_reconnect(info: ScreenInfo, opener: Arc<dyn StreamOpener>, reconnect: ReconnectPolicy) -> StreamScreenSource {
        let cache = Arc::new(HostProtocolCache::fixed(HostCapabilities::from_pane(&info.capabilities)));
        Self::with_host_cache_and_reconnect(info, opener, cache, reconnect)
    }

    /// Create a remote screen source sharing a per-host [`HostProtocolCache`]
    /// so `olo-rex-link screen --protocol` is queried once per host and kept
    /// across every screen on that host.
    pub fn with_host_cache(info: ScreenInfo, opener: Arc<dyn StreamOpener>, protocol_cache: Arc<HostProtocolCache>) -> StreamScreenSource {
        Self::with_host_cache_and_reconnect(info, opener, protocol_cache, ReconnectPolicy::default())
    }

    pub fn with_host_cache_and_reconnect(mut info: ScreenInfo, opener: Arc<dyn StreamOpener>, protocol_cache: Arc<HostProtocolCache>, reconnect: ReconnectPolicy) -> StreamScreenSource {
        if let Some(cached) = protocol_cache.get() {
            cached.apply_to(&mut info.capabilities);
        }
        let expected_started = info.started_ms.map(|ms| ms / 1000);
        StreamScreenSource {
            id: info.id.clone(),
            info: Arc::new(Mutex::new(info)),
            expected_started: Arc::new(Mutex::new(expected_started)),
            opener,
            protocol_cache,
            reconnect,
            active: Arc::new(Mutex::new(None)),
            history: Arc::new(Mutex::new(None)),
            max_export_bytes: DEFAULT_MAX_DESKTOP_EXPORT_BYTES,
            export_free_reserve: DEFAULT_EXPORT_FREE_RESERVE,
            max_retained_actions: DEFAULT_MAX_RETAINED_ACTIONS,
            max_retained_action_bytes: DEFAULT_MAX_RETAINED_ACTION_BYTES,
            max_retained_frames: DEFAULT_MAX_RETAINED_FRAMES,
            frame_at_timeout: DEFAULT_FRAME_AT_TIMEOUT,
        }
    }

    /// Probe (once per host) or return cached [`HostCapabilities`] and sync
    /// them into this screen's [`ScreenInfo::capabilities`]. Must be called on
    /// a worker thread (never the UI thread).
    pub fn probe_capabilities(&self) -> HostCapabilities {
        let caps = self.protocol_cache.get_or_probe(&*self.opener);
        let mut info = self.info.lock().unwrap_or_else(|e| e.into_inner());
        caps.apply_to(&mut info.capabilities);
        caps
    }

    /// Update the running session's `started` timestamp (seconds since epoch)
    /// used by generation-checked `stop --expected-started`.
    pub fn set_session_started(&self, started: Option<u64>) {
        *self.expected_started.lock().unwrap_or_else(|e| e.into_inner()) = started;
    }

    /// Replace the cached metadata snapshot (called when `screen follow`
    /// reports changes for this screen).
    pub fn set_info(&self, mut info: ScreenInfo) {
        if let Some(cached) = self.protocol_cache.get() {
            cached.apply_to(&mut info.capabilities);
        }
        if let Some(ms) = info.started_ms {
            *self.expected_started.lock().unwrap_or_else(|e| e.into_inner()) = Some(ms / 1000);
        } else if !info.is_running() {
            *self.expected_started.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
        *self.info.lock().unwrap_or_else(|e| e.into_inner()) = info;
    }

    fn run_checked(&self, args: &[String]) -> Result<CommandOutput, String> {
        screens::require_valid_name(&self.id.name)?;
        let out = self.opener.run_command(args, Arc::new(AtomicBool::new(false)))?;
        if out.success() { Ok(out) } else { Err(out.error_words()) }
    }

    /// Fetch a single recorded frame over a dedicated, per-request cancellable
    /// `screen history-stream` channel with a bounded response deadline.
    ///
    /// Used by `frame_at_pinned` (so `Take Screenshot…` never collides with
    /// latest-wins scrubbing or another concurrent screenshot) and by
    /// `frame_at` when no persistent history follower is active.
    fn frame_at_oneshot(&self, at_ms: u64, cancel: &ExportCancel) -> Result<Option<(u64, Frame)>, String> {
        screens::require_valid_name(&self.id.name)?;
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        // Arm a response deadline watchdog so a silent host can never keep a
        // screenshot or one-shot worker blocked forever.
        let timed_out = Arc::new(AtomicBool::new(false));
        let done_flag = Arc::new((Mutex::new(false), Condvar::new()));
        let watchdog_cancel = cancel.clone();
        let watchdog_timed_out = timed_out.clone();
        let watchdog_done = done_flag.clone();
        let timeout = self.frame_at_timeout;
        let _ = std::thread::Builder::new().name("hist-shot-timeout".into()).spawn(move || {
            let mut done = watchdog_done.0.lock().unwrap_or_else(|e| e.into_inner());
            let deadline = Instant::now() + timeout;
            while !*done {
                let rem = deadline.saturating_duration_since(Instant::now());
                if rem.is_zero() {
                    watchdog_timed_out.store(true, Ordering::SeqCst);
                    watchdog_cancel.cancel();
                    return;
                }
                let (next, _) = watchdog_done.1.wait_timeout(done, rem).unwrap_or_else(|e| e.into_inner());
                done = next;
            }
        });
        struct DoneGuard(Arc<(Mutex<bool>, Condvar)>);
        impl Drop for DoneGuard {
            fn drop(&mut self) {
                *self.0.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
                self.0.1.notify_all();
            }
        }
        let _done_guard = DoneGuard(done_flag);

        let LiveStream { reader, mut writer, cancel: stream_cancel } = self.opener.open_history(&self.id.name, cancel.flag())?;
        let handle_slot: TeardownSlot = Arc::new(Mutex::new(None));
        let write_res = write_history_request(&mut writer, &HistoryRequest::FrameAt { req: 1, at: at_ms });
        *handle_slot.lock().unwrap_or_else(|e| e.into_inner()) = Some((Some(writer), stream_cancel));
        let slot_for_hook = handle_slot.clone();
        cancel.register_hook(move || {
            if let Some((w, f)) = slot_for_hook.lock().unwrap_or_else(|e| e.into_inner()).take() {
                drop(w);
                if let Some(f) = f {
                    f();
                }
            }
        });
        struct OneshotGuard(TeardownSlot);
        impl Drop for OneshotGuard {
            fn drop(&mut self) {
                if let Some((w, f)) = self.0.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    drop(w);
                    if let Some(f) = f {
                        f();
                    }
                }
            }
        }
        let _guard = OneshotGuard(handle_slot);
        let cancel_err = || {
            if timed_out.load(Ordering::Acquire) { Err("timed out waiting for recorded frame".into()) } else { Err("cancelled".into()) }
        };
        if let Err(e) = write_res {
            return if cancel.is_cancelled() { cancel_err() } else { Err(format!("request recorded frame: {e}")) };
        }
        let mut buf_reader = BufReader::new(reader);
        let mut canvas = Frame { width: 0, height: 0, rgba: Vec::new() };
        loop {
            if cancel.is_cancelled() {
                return cancel_err();
            }
            match read_history_msg_bounded(&mut buf_reader) {
                Ok(Some(DecodedHistoryMsg::Actions { .. } | DecodedHistoryMsg::Index { .. } | DecodedHistoryMsg::SnapshotDone { .. })) => continue,
                Ok(Some(DecodedHistoryMsg::Frame { req, frame_at, delta })) => {
                    apply_delta(&delta, &mut canvas)?;
                    if req == 1 {
                        return Ok(Some((frame_at, canvas)));
                    }
                }
                Ok(Some(DecodedHistoryMsg::Miss { req: 1 })) => return Ok(None),
                Ok(Some(DecodedHistoryMsg::Miss { .. })) => continue,
                Ok(Some(DecodedHistoryMsg::FrameError { req: 1, message })) => return Err(message),
                Ok(Some(DecodedHistoryMsg::FrameError { .. })) => continue,
                Ok(Some(DecodedHistoryMsg::Error { message })) => return Err(message),
                Ok(None) => return if cancel.is_cancelled() { cancel_err() } else { Err("history stream closed before answering".into()) },
                Err(e) => return if cancel.is_cancelled() { cancel_err() } else { Err(format!("read recorded frame: {e}")) },
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Live stream (`watch_live`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlState {
    Idle,
    Requested,
    Held,
    Releasing,
}

enum WriteCmd {
    Pause(bool),
    Control { on: bool, seq: u64 },
    Input(Vec<SyntheticEvent>),
    Close,
}

fn is_release_event(ev: &SyntheticEvent) -> bool {
    matches!(ev, SyntheticEvent::Key { pressed: false, .. } | SyntheticEvent::Button { pressed: false, .. })
}

fn approx_event_bytes(ev: &SyntheticEvent) -> usize {
    match ev {
        SyntheticEvent::Move { .. } | SyntheticEvent::Scroll { .. } | SyntheticEvent::Wheel { .. } => 48,
        SyntheticEvent::Key { .. } | SyntheticEvent::Button { .. } => 40,
        _ => 64,
    }
}

/// Split a coalesced input batch into wire-sized slices that obey both
/// [`MAX_WIRE_INPUT_EVENTS`] and [`MAX_WIRE_INPUT_BYTES`], preserving exact
/// event and release order.
fn split_input_for_wire(events: &[SyntheticEvent]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < events.len() {
        let mut end = (i + MAX_WIRE_INPUT_EVENTS).min(events.len());
        loop {
            let Ok(json) = serde_json::to_string(&events[i..end]) else {
                i = end;
                break;
            };
            if json.len() <= MAX_WIRE_INPUT_BYTES || end == i + 1 {
                if json.len() <= MAX_WIRE_INPUT_BYTES {
                    out.push(json);
                }
                i = end;
                break;
            }
            // Binary-shrink batch size to fit under the wire byte cap.
            end = i + (end - i) / 2;
        }
    }
    out
}

struct WriteQueue {
    cmds: VecDeque<WriteCmd>,
    /// Cumulative latest frame ack coalesced into a single slot (never builds
    /// an unbounded ACK backlog when the return channel stalls).
    pending_ack: Option<u64>,
    queued_input_events: usize,
    queued_input_bytes: usize,
    connected: bool,
    terminal: bool,
    paused: bool,
    control_state: ControlState,
    /// Monotonic non-zero sequence counter for `ClientMsg::ControlSeq`.
    next_control_seq: u64,
    /// The `seq` of the in-flight `ControlSeq { on: true, seq }` while
    /// `control_state == ControlState::Requested`.
    pending_take_seq: Option<u64>,
    /// The `seq` of the latest `ControlSeq { on: false, seq }` sent.
    release_req_seq: u64,
    /// Highest release `seq` confirmed by the host (or advanced on unsolicited
    /// revocation / disconnect / terminal exit).
    release_done_seq: u64,
}

impl WriteQueue {
    fn alloc_control_seq(&mut self) -> u64 {
        self.next_control_seq = self.next_control_seq.wrapping_add(1);
        if self.next_control_seq == 0 {
            self.next_control_seq = 1;
        }
        self.next_control_seq
    }

    fn drop_queued_input(&mut self) {
        self.cmds.retain(|c| !matches!(c, WriteCmd::Input(_)));
        self.queued_input_events = 0;
        self.queued_input_bytes = 0;
    }

    fn push_input_bounded(&mut self, mut events: Vec<SyntheticEvent>) {
        run_together(&mut events);
        if events.is_empty() {
            return;
        }
        let add_events = events.len();
        let add_bytes: usize = events.iter().map(approx_event_bytes).sum();
        if self.queued_input_events + add_events <= MAX_QUEUED_INPUT_EVENTS && self.queued_input_bytes + add_bytes <= MAX_QUEUED_INPUT_BYTES {
            self.queued_input_events += add_events;
            self.queued_input_bytes += add_bytes;
            self.cmds.push_back(WriteCmd::Input(events));
            return;
        }
        // Over cap: keep only release events so held keys/buttons never stick.
        events.retain(is_release_event);
        if !events.is_empty() {
            let rel_bytes: usize = events.iter().map(approx_event_bytes).sum();
            self.queued_input_events += events.len();
            self.queued_input_bytes += rel_bytes;
            self.cmds.push_back(WriteCmd::Input(events));
        }
    }
}

struct DeliverQueue {
    events: VecDeque<LiveEvent>,
    /// Immediate abort (user dropped handle): discard remaining queued events.
    stop: bool,
    /// Graceful completion (session reached terminal end): drain remaining
    /// queued events (`Failed`/`Disconnected`) and then exit `stream-d`.
    finish: bool,
}

struct CancelSlot {
    stopped: bool,
    hook: Option<Box<dyn FnOnce() + Send>>,
}

struct StreamControl {
    stream_id: usize,
    stop: Arc<AtomicBool>,
    stop_lock: Mutex<()>,
    stop_cvar: Condvar,
    cancel: Mutex<CancelSlot>,
    write_q: Mutex<WriteQueue>,
    write_cvar: Condvar,
    release_cvar: Condvar,
    deliver_q: Mutex<DeliverQueue>,
    deliver_cvar: Condvar,
    explicit_control_ack: AtomicBool,
    /// Live worker thread count owned by this handle (used by tests to prove
    /// zero threads survive drop).
    live_threads: AtomicUsize,
}

impl StreamControl {
    fn new(explicit_control_ack: bool) -> Arc<StreamControl> {
        static NEXT_STREAM: AtomicUsize = AtomicUsize::new(1);
        Arc::new(StreamControl {
            stream_id: NEXT_STREAM.fetch_add(1, Ordering::Relaxed),
            stop: Arc::new(AtomicBool::new(false)),
            stop_lock: Mutex::new(()),
            stop_cvar: Condvar::new(),
            cancel: Mutex::new(CancelSlot { stopped: false, hook: None }),
            write_q: Mutex::new(WriteQueue {
                cmds: VecDeque::new(),
                pending_ack: None,
                queued_input_events: 0,
                queued_input_bytes: 0,
                connected: false,
                terminal: false,
                paused: false,
                control_state: ControlState::Idle,
                next_control_seq: 0,
                pending_take_seq: None,
                release_req_seq: 0,
                release_done_seq: 0,
            }),
            write_cvar: Condvar::new(),
            release_cvar: Condvar::new(),
            deliver_q: Mutex::new(DeliverQueue { events: VecDeque::new(), stop: false, finish: false }),
            deliver_cvar: Condvar::new(),
            explicit_control_ack: AtomicBool::new(explicit_control_ack),
            live_threads: AtomicUsize::new(0),
        })
    }

    fn push_event(&self, ev: LiveEvent) {
        let mut dq = self.deliver_q.lock().unwrap_or_else(|e| e.into_inner());
        if dq.stop || dq.finish {
            return;
        }
        // Latest picture wins: keep at most one `Rgba` frame queued so a busy
        // pane never builds a backlog of megabyte buffers.
        if matches!(ev, LiveEvent::Rgba(_)) {
            dq.events.retain(|e| !matches!(e, LiveEvent::Rgba(_)));
        } else if let LiveEvent::Status(ref st) = ev {
            // Coalesce repeated/superseded non-terminal status updates so a
            // stalled UI never builds an unbounded status queue.
            if !matches!(st, LiveStatus::Disconnected(_)) {
                dq.events.retain(|e| !matches!(e, LiveEvent::Status(prev) if !matches!(prev, LiveStatus::Disconnected(_))));
            }
        }
        dq.events.push_back(ev);
        self.deliver_cvar.notify_one();
    }

    fn finish_delivery(&self) {
        let mut dq = self.deliver_q.lock().unwrap_or_else(|e| e.into_inner());
        dq.finish = true;
        self.deliver_cvar.notify_all();
    }

    fn enqueue_ack(&self, seq: u64) {
        let mut wq = self.write_q.lock().unwrap_or_else(|e| e.into_inner());
        if wq.connected {
            wq.pending_ack = Some(wq.pending_ack.map_or(seq, |prev| prev.max(seq)));
            self.write_cvar.notify_one();
        }
    }

    fn fire_cancel(&self) {
        let hook = self.cancel.lock().unwrap_or_else(|e| e.into_inner()).hook.take();
        if let Some(f) = hook {
            f();
        }
    }

    /// Sleep up to `delay`, waking immediately if `stop` is triggered.
    fn wait_backoff(&self, delay: Duration) -> bool {
        let guard = self.stop_lock.lock().unwrap_or_else(|e| e.into_inner());
        if self.stop.load(Ordering::Acquire) {
            return false;
        }
        let _ = self.stop_cvar.wait_timeout(guard, delay);
        !self.stop.load(Ordering::Acquire)
    }
}

struct ThreadGuard(Arc<StreamControl>);

impl ThreadGuard {
    fn enter(ctrl: Arc<StreamControl>) -> ThreadGuard {
        ctrl.live_threads.fetch_add(1, Ordering::AcqRel);
        ThreadGuard(ctrl)
    }
}

impl Drop for ThreadGuard {
    fn drop(&mut self) {
        self.0.live_threads.fetch_sub(1, Ordering::AcqRel);
    }
}

struct StreamLiveHandle {
    ctrl: Arc<StreamControl>,
    source_active: Option<Arc<Mutex<Option<Arc<StreamControl>>>>>,
    session_join: Option<JoinHandle<()>>,
    deliver_join: Option<JoinHandle<()>>,
}

impl StreamLiveHandle {
    fn shutdown(&mut self) {
        self.ctrl.stop.store(true, Ordering::SeqCst);
        if let Some(active) = self.source_active.take() {
            let mut slot = active.lock().unwrap_or_else(|e| e.into_inner());
            if slot.as_ref().is_some_and(|cur| Arc::ptr_eq(cur, &self.ctrl)) {
                *slot = None;
            }
        }
        // Close write half immediately so the host helper sees EOF and exits.
        {
            let mut wq = self.ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
            wq.connected = false;
            wq.terminal = true;
            wq.control_state = ControlState::Idle;
            wq.pending_take_seq = None;
            wq.pending_ack = None;
            wq.queued_input_events = 0;
            wq.queued_input_bytes = 0;
            wq.cmds.clear();
            wq.cmds.push_back(WriteCmd::Close);
            self.ctrl.write_cvar.notify_all();
            self.ctrl.release_cvar.notify_all();
        }
        // Wake any backoff sleep.
        {
            let _g = self.ctrl.stop_lock.lock().unwrap_or_else(|e| e.into_inner());
            self.ctrl.stop_cvar.notify_all();
        }
        // Fire transport cancel hook under `cancel` lock so it cannot race
        // with `open_live` storing a new hook.
        let hook = {
            let mut c = self.ctrl.cancel.lock().unwrap_or_else(|e| e.into_inner());
            c.stopped = true;
            c.hook.take()
        };
        if let Some(f) = hook {
            f();
        }
        // Wake delivery thread so it exits without running further callbacks.
        {
            let mut dq = self.ctrl.deliver_q.lock().unwrap_or_else(|e| e.into_inner());
            dq.stop = true;
            dq.events.clear();
            self.ctrl.deliver_cvar.notify_all();
        }
        // Reap worker threads off the window thread so closing or switching a
        // remote pane never blocks the UI even on a stalled transport.
        let sj = self.session_join.take();
        let dj = self.deliver_join.take();
        if sj.is_some() || dj.is_some() {
            let _ = std::thread::Builder::new().name("stream-reap".into()).spawn(move || {
                if let Some(h) = sj {
                    let _ = h.join();
                }
                if let Some(h) = dj {
                    let _ = h.join();
                }
            });
        }
    }
}

impl LiveHandle for StreamLiveHandle {
    fn stop(&mut self) {
        self.shutdown();
    }

    fn set_paused(&mut self, paused: bool) {
        let mut wq = self.ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
        if wq.paused == paused {
            return;
        }
        wq.paused = paused;
        if wq.connected {
            wq.cmds.retain(|c| !matches!(c, WriteCmd::Pause(_)));
            wq.cmds.push_back(WriteCmd::Pause(paused));
            self.ctrl.write_cvar.notify_one();
        }
    }
}

impl Drop for StreamLiveHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Per-connection writer worker: owns the write half, sleeps on `write_cvar`
/// at rest, writes coalesced `Ack` plus ordered `Pause`/`Control`/`Input`
/// (splitting large input batches under [`MAX_WIRE_INPUT_BYTES`]), and drops
/// `writer` (sending EOF) on `Close` or write error.
fn spawn_writer(ctrl: Arc<StreamControl>, info: Arc<Mutex<ScreenInfo>>, mut writer: Box<dyn Write + Send>) -> JoinHandle<()> {
    let tname = format!("stream-w{}", ctrl.stream_id);
    std::thread::Builder::new()
        .name(tname)
        .spawn(move || {
            let _guard = ThreadGuard::enter(ctrl.clone());
            loop {
                enum NextWrite {
                    Ack(u64),
                    Cmd(WriteCmd),
                }
                let next = {
                    let mut wq = ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        if let Some(c) = wq.cmds.pop_front() {
                            if let WriteCmd::Input(ref evs) = c {
                                let bytes: usize = evs.iter().map(approx_event_bytes).sum();
                                wq.queued_input_events = wq.queued_input_events.saturating_sub(evs.len());
                                wq.queued_input_bytes = wq.queued_input_bytes.saturating_sub(bytes);
                            }
                            break NextWrite::Cmd(c);
                        }
                        if let Some(seq) = wq.pending_ack.take() {
                            break NextWrite::Ack(seq);
                        }
                        if !wq.connected || ctrl.stop.load(Ordering::Acquire) {
                            break NextWrite::Cmd(WriteCmd::Close);
                        }
                        wq = ctrl.write_cvar.wait(wq).unwrap_or_else(|e| e.into_inner());
                    }
                };
                match next {
                    NextWrite::Cmd(WriteCmd::Close) => break,
                    NextWrite::Ack(seq) => {
                        if write_client_msg(&mut writer, &ClientMsg::Ack { seq }).is_err() {
                            ctrl.fire_cancel();
                            break;
                        }
                    }
                    NextWrite::Cmd(WriteCmd::Pause(paused)) => {
                        if write_client_msg(&mut writer, &ClientMsg::Pause { paused }).is_err() {
                            ctrl.fire_cancel();
                            break;
                        }
                    }
                    NextWrite::Cmd(WriteCmd::Control { on, seq }) => {
                        let explicit_ack = ctrl.explicit_control_ack.load(Ordering::Acquire);
                        let msg = if explicit_ack { ClientMsg::ControlSeq { on, seq } } else { ClientMsg::Control { on } };
                        if write_client_msg(&mut writer, &msg).is_err() {
                            ctrl.fire_cancel();
                            break;
                        }
                        // Legacy compatibility for an older helper that lacks
                        // `"control-ack"` (does not emit `ServerMsg::Control`
                        // replies): confirm locally once the legacy control
                        // message hits the wire.
                        if !explicit_ack {
                            if on {
                                let granted = {
                                    let mut wq = ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
                                    if wq.connected && wq.control_state == ControlState::Requested && wq.pending_take_seq == Some(seq) {
                                        wq.control_state = ControlState::Held;
                                        wq.pending_take_seq = None;
                                        true
                                    } else {
                                        false
                                    }
                                };
                                if granted {
                                    info.lock().unwrap_or_else(|e| e.into_inner()).controlled = true;
                                    ctrl.push_event(LiveEvent::Control { held: true, reason: None });
                                }
                            } else {
                                let mut wq = ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
                                if wq.control_state == ControlState::Releasing && seq >= wq.release_req_seq {
                                    wq.control_state = ControlState::Idle;
                                }
                                if seq > wq.release_done_seq {
                                    wq.release_done_seq = seq;
                                    ctrl.release_cvar.notify_all();
                                }
                            }
                        }
                    }
                    NextWrite::Cmd(WriteCmd::Input(mut batch)) => {
                        // Drain consecutive queued Input batches without overtaking
                        // `Control`, `Pause`, or `Close`.
                        {
                            let mut wq = ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
                            while matches!(wq.cmds.front(), Some(WriteCmd::Input(_))) {
                                if let Some(WriteCmd::Input(more)) = wq.cmds.pop_front() {
                                    let bytes: usize = more.iter().map(approx_event_bytes).sum();
                                    wq.queued_input_events = wq.queued_input_events.saturating_sub(more.len());
                                    wq.queued_input_bytes = wq.queued_input_bytes.saturating_sub(bytes);
                                    batch.extend(more);
                                }
                            }
                        }
                        run_together(&mut batch);
                        if batch.is_empty() {
                            continue;
                        }
                        for json_events in split_input_for_wire(&batch) {
                            if write_client_msg(&mut writer, &ClientMsg::Input { json_events }).is_err() {
                                ctrl.fire_cancel();
                                break;
                            }
                        }
                    }
                }
            }
            drop(writer);
        })
        .expect("spawn stream-write")
}

/// Session thread: opens and maintains the live stream with exponential
/// backoff, decodes frames onto the screen canvas, acks each applied frame,
/// guarantees its per-connection writer thread is cancelled & joined before
/// reconnect or exit, and retires the handle + delivery thread on terminal exit.
fn run_session(
    name: String,
    opener: Arc<dyn StreamOpener>,
    protocol_cache: Arc<HostProtocolCache>,
    reconnect: ReconnectPolicy,
    info: Arc<Mutex<ScreenInfo>>,
    source_active: Arc<Mutex<Option<Arc<StreamControl>>>>,
    ctrl: Arc<StreamControl>,
) {
    let _guard = ThreadGuard::enter(ctrl.clone());
    if let Err(msg) = screens::require_valid_name(&name) {
        ctrl.push_event(LiveEvent::Failed(msg.clone()));
        ctrl.push_event(LiveEvent::Status(LiveStatus::Disconnected(msg)));
        retire_live_session(&source_active, &ctrl);
        return;
    }
    let host_caps = protocol_cache.get_or_probe(&*opener);
    {
        let mut inf = info.lock().unwrap_or_else(|e| e.into_inner());
        host_caps.apply_to(&mut inf.capabilities);
        ctrl.explicit_control_ack.store(inf.capabilities.explicit_control_ack, Ordering::Release);
    }
    let mut attempt: usize = 0;

    loop {
        if ctrl.stop.load(Ordering::Acquire) {
            break;
        }
        let opened = opener.open_live(&name, ctrl.stop.clone());
        if ctrl.stop.load(Ordering::Acquire) {
            if let Ok(LiveStream { cancel: Some(f), .. }) = opened {
                f();
            }
            break;
        }
        let LiveStream { reader, writer, cancel } = match opened {
            Ok(s) => s,
            Err(msg) => {
                if attempt < reconnect.delays.len() {
                    ctrl.push_event(LiveEvent::Status(LiveStatus::Reconnecting));
                    if !ctrl.wait_backoff(reconnect.delays[attempt]) {
                        break;
                    }
                    attempt += 1;
                    continue;
                }
                ctrl.push_event(LiveEvent::Failed(msg.clone()));
                ctrl.push_event(LiveEvent::Status(LiveStatus::Disconnected(msg)));
                break;
            }
        };

        // Install the cancel hook atomically against `shutdown()`.
        {
            let mut slot = ctrl.cancel.lock().unwrap_or_else(|e| e.into_inner());
            if slot.stopped || ctrl.stop.load(Ordering::Acquire) {
                drop(slot);
                if let Some(f) = cancel {
                    f();
                }
                drop(writer);
                drop(reader);
                break;
            }
            slot.hook = cancel;
        }

        // Activate write queue for this connection and spawn its writer thread.
        {
            let mut wq = ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
            wq.cmds.clear();
            wq.pending_ack = None;
            wq.queued_input_events = 0;
            wq.queued_input_bytes = 0;
            wq.connected = true;
            wq.control_state = ControlState::Idle;
            wq.pending_take_seq = None;
            if wq.paused {
                wq.cmds.push_back(WriteCmd::Pause(true));
            }
            ctrl.write_cvar.notify_all();
        }
        let writer_join = spawn_writer(ctrl.clone(), info.clone(), writer);

        let mut buf_reader = BufReader::new(reader);
        let mut canvas = Frame { width: 0, height: 0, rgba: Vec::new() };
        let mut awaiting_keyframe = true;
        let mut screen_gone: Option<String> = None;
        let mut protocol_error: Option<String> = None;
        let mut saw_error_status: Option<String> = None;
        let mut io_error: Option<String> = None;

        loop {
            if ctrl.stop.load(Ordering::Acquire) {
                break;
            }
            match read_server_msg_bounded(&mut buf_reader) {
                Ok(Some(DecodedServerMsg::Frame { seq, delta })) => {
                    if awaiting_keyframe && !delta.keyframe {
                        // Await a full keyframe after connect/reconnect so a
                        // stray delta is never painted onto an empty canvas.
                        ctrl.enqueue_ack(seq);
                        continue;
                    }
                    if let Err(e) = apply_delta(&delta, &mut canvas) {
                        protocol_error = Some(e);
                        break;
                    }
                    awaiting_keyframe = false;
                    saw_error_status = None;
                    attempt = 0;
                    // Update logical dimensions whenever the live canvas size changes
                    // so remote pointer mapping always tracks screen resizes.
                    if canvas.width > 0 && canvas.height > 0 {
                        let mut inf = info.lock().unwrap_or_else(|e| e.into_inner());
                        let scale = inf.scale.max(0.5);
                        inf.size = Some(((canvas.width as f64 / scale).round() as i32, (canvas.height as f64 / scale).round() as i32));
                    }
                    // Hand the rebuilt picture to the latest-wins delivery slot
                    // BEFORE sending `Ack` so an acked frame is already visible
                    // to the pane and superseding older queued shots is ordered.
                    let shot = Arc::new(Shot { size: [canvas.width, canvas.height], rgba: canvas.rgba.clone() });
                    ctrl.push_event(LiveEvent::Rgba(shot));
                    ctrl.enqueue_ack(seq);
                }
                Ok(Some(DecodedServerMsg::Status(st))) => {
                    if st.warning {
                        // Non-fatal capture warning: surface words without dropping control or stream.
                        if !st.message.is_empty() {
                            ctrl.push_event(LiveEvent::Status(LiveStatus::Warning(st.message)));
                        }
                    } else if st.control == Some(true) {
                        // Legacy/JSON explicit host grant of Take Control.
                        let granted = {
                            let mut wq = ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
                            if wq.control_state == ControlState::Requested {
                                wq.control_state = ControlState::Held;
                                wq.pending_take_seq = None;
                                true
                            } else {
                                // Grant arrived after user cancelled/released: immediately
                                // send release back so host doesn't stay locked.
                                if wq.connected && wq.control_state == ControlState::Idle {
                                    let seq = wq.alloc_control_seq();
                                    wq.control_state = ControlState::Releasing;
                                    wq.release_req_seq = seq;
                                    wq.cmds.push_back(WriteCmd::Control { on: false, seq });
                                    ctrl.write_cvar.notify_one();
                                }
                                false
                            }
                        };
                        if granted {
                            info.lock().unwrap_or_else(|e| e.into_inner()).controlled = true;
                            ctrl.push_event(LiveEvent::Control { held: true, reason: None });
                        }
                    } else if st.reconnecting {
                        awaiting_keyframe = true;
                        let lost_ctl = {
                            let mut wq = ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
                            let had = matches!(wq.control_state, ControlState::Requested | ControlState::Held);
                            if had {
                                wq.control_state = ControlState::Idle;
                                wq.pending_take_seq = None;
                                wq.drop_queued_input();
                                // If an older host kept `.control` across compositor
                                // reload (did not send `ServerMsg::Control { held: false }`
                                // or `st.control == Some(false)`), explicitly send release
                                // over the still-open channel so agents aren't locked out.
                                if st.control != Some(false) && wq.connected {
                                    let seq = wq.alloc_control_seq();
                                    wq.release_req_seq = seq;
                                    wq.cmds.push_back(WriteCmd::Control { on: false, seq });
                                    ctrl.write_cvar.notify_one();
                                }
                            }
                            had
                        };
                        if lost_ctl {
                            info.lock().unwrap_or_else(|e| e.into_inner()).controlled = false;
                            ctrl.push_event(LiveEvent::Control { held: false, reason: None });
                        }
                        ctrl.push_event(LiveEvent::Status(LiveStatus::Reconnecting));
                    } else if st.control == Some(false) && !st.error {
                        // Legacy/JSON explicit host release acknowledgement.
                        let was_held_or_req = {
                            let mut wq = ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
                            let had = matches!(wq.control_state, ControlState::Requested | ControlState::Held);
                            wq.control_state = ControlState::Idle;
                            wq.pending_take_seq = None;
                            wq.drop_queued_input();
                            wq.release_done_seq = wq.release_done_seq.max(wq.release_req_seq);
                            ctrl.release_cvar.notify_all();
                            had
                        };
                        info.lock().unwrap_or_else(|e| e.into_inner()).controlled = false;
                        if was_held_or_req {
                            let reason = (!st.message.is_empty()).then_some(st.message);
                            ctrl.push_event(LiveEvent::Control { held: false, reason });
                        }
                    } else if st.error {
                        let was_control_flow = {
                            let mut wq = ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
                            if matches!(wq.control_state, ControlState::Requested | ControlState::Held) || st.control == Some(false) {
                                wq.control_state = ControlState::Idle;
                                wq.pending_take_seq = None;
                                wq.drop_queued_input();
                                wq.release_done_seq = wq.release_done_seq.max(wq.release_req_seq);
                                ctrl.release_cvar.notify_all();
                                true
                            } else {
                                false
                            }
                        };
                        if was_control_flow {
                            info.lock().unwrap_or_else(|e| e.into_inner()).controlled = false;
                            ctrl.push_event(LiveEvent::Control { held: false, reason: Some(st.message) });
                        } else {
                            saw_error_status = Some(st.message.clone());
                            ctrl.push_event(LiveEvent::Failed(st.message.clone()));
                            ctrl.push_event(LiveEvent::Status(LiveStatus::Disconnected(st.message)));
                        }
                    } else if st.message.is_empty() || st.message.eq_ignore_ascii_case("ready") {
                        ctrl.push_event(LiveEvent::Status(LiveStatus::Streaming));
                    } else {
                        ctrl.push_event(LiveEvent::Status(LiveStatus::Message(st.message)));
                    }
                }
                Ok(Some(DecodedServerMsg::ScreenGone { reason })) => {
                    let words = if reason.is_empty() { "The screen has gone.".to_owned() } else { reason };
                    screen_gone = Some(words);
                    break;
                }
                Ok(Some(DecodedServerMsg::Control { held, seq, reason })) => {
                    if held {
                        let granted = {
                            let mut wq = ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
                            let matches_pending = wq.control_state == ControlState::Requested
                                && match wq.pending_take_seq {
                                    Some(want) => seq == want,
                                    None => false,
                                };
                            if matches_pending {
                                wq.control_state = ControlState::Held;
                                wq.pending_take_seq = None;
                                true
                            } else {
                                // Grant arrived after user cancelled/released (or for a stale seq):
                                // if we are Idle (no release already in flight on the wire), send
                                // release immediately so the host does not stay locked.
                                if wq.connected && wq.control_state == ControlState::Idle {
                                    let rel_seq = wq.alloc_control_seq();
                                    wq.control_state = ControlState::Releasing;
                                    wq.release_req_seq = rel_seq;
                                    wq.cmds.push_back(WriteCmd::Control { on: false, seq: rel_seq });
                                    ctrl.write_cvar.notify_one();
                                }
                                false
                            }
                        };
                        if granted {
                            info.lock().unwrap_or_else(|e| e.into_inner()).controlled = true;
                            ctrl.push_event(LiveEvent::Control { held: true, reason: None });
                        }
                    } else {
                        let emit_lost: Option<Option<String>> = {
                            let mut wq = ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
                            if seq == 0 {
                                // Unsolicited host revocation (compositor reconnect #7,
                                // input queue overflow #13) or legacy untagged release.
                                let had = matches!(wq.control_state, ControlState::Requested | ControlState::Held);
                                wq.control_state = ControlState::Idle;
                                wq.pending_take_seq = None;
                                wq.drop_queued_input();
                                wq.release_done_seq = wq.release_done_seq.max(wq.release_req_seq);
                                ctrl.release_cvar.notify_all();
                                (had || reason.is_some()).then_some(reason)
                            } else if wq.pending_take_seq == Some(seq) {
                                // Explicit refusal of our pending Take Control request `seq`.
                                wq.control_state = ControlState::Idle;
                                wq.pending_take_seq = None;
                                wq.drop_queued_input();
                                Some(reason)
                            } else {
                                // Completion of an earlier `ControlSeq { on: false, seq }`
                                // (or stale earlier refusal): unblock waiters without clobbering
                                // any newer `Requested` take already queued behind it.
                                if seq <= wq.release_req_seq {
                                    wq.release_done_seq = wq.release_done_seq.max(seq);
                                    ctrl.release_cvar.notify_all();
                                }
                                if wq.control_state == ControlState::Releasing && seq >= wq.release_req_seq {
                                    wq.control_state = ControlState::Idle;
                                    wq.drop_queued_input();
                                    reason.map(Some)
                                } else {
                                    None
                                }
                            }
                        };
                        if let Some(reason) = emit_lost {
                            info.lock().unwrap_or_else(|e| e.into_inner()).controlled = false;
                            ctrl.push_event(LiveEvent::Control { held: false, reason });
                        }
                    }
                }
                Ok(Some(DecodedServerMsg::Warning { message })) => {
                    // Non-fatal capture or stream warning: surface words without
                    // revoking control or tearing down the live stream.
                    if !message.is_empty() {
                        ctrl.push_event(LiveEvent::Status(LiveStatus::Warning(message)));
                    }
                }
                Ok(None) => break,
                Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                    protocol_error = Some(e.to_string());
                    break;
                }
                Err(e) => {
                    io_error = Some(e.to_string());
                    break;
                }
            }
        }

        // Tear down this connection cleanly: fire transport cancel hook BEFORE
        // joining `writer_join` so a writer blocked on a half-closed/stalled
        // peer always unblocks immediately.
        let lost_control = {
            let mut wq = ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
            let had = matches!(wq.control_state, ControlState::Requested | ControlState::Held);
            wq.connected = false;
            wq.control_state = ControlState::Idle;
            wq.pending_take_seq = None;
            wq.pending_ack = None;
            wq.queued_input_events = 0;
            wq.queued_input_bytes = 0;
            wq.cmds.clear();
            wq.cmds.push_back(WriteCmd::Close);
            wq.release_done_seq = wq.release_done_seq.max(wq.release_req_seq);
            ctrl.write_cvar.notify_all();
            ctrl.release_cvar.notify_all();
            had
        };
        ctrl.fire_cancel();
        let _ = writer_join.join();
        drop(buf_reader);

        if ctrl.stop.load(Ordering::Acquire) {
            break;
        }
        if lost_control {
            info.lock().unwrap_or_else(|e| e.into_inner()).controlled = false;
            ctrl.push_event(LiveEvent::Control { held: false, reason: None });
        }
        if let Some(reason) = screen_gone {
            let mut inf = info.lock().unwrap_or_else(|e| e.into_inner());
            inf.state = ScreenState::Stopped;
            inf.controlled = false;
            drop(inf);
            ctrl.push_event(LiveEvent::Status(LiveStatus::Disconnected(reason)));
            break;
        }
        if let Some(err) = protocol_error {
            let msg = format!("Stream closed: {err}");
            ctrl.push_event(LiveEvent::Failed(msg.clone()));
            ctrl.push_event(LiveEvent::Status(LiveStatus::Disconnected(msg)));
            break;
        }
        if let Some(msg) = saw_error_status {
            ctrl.push_event(LiveEvent::Status(LiveStatus::Disconnected(msg)));
            break;
        }

        // Transport broke mid-stream: reconnect with backoff and await a keyframe.
        if attempt < reconnect.delays.len() {
            ctrl.push_event(LiveEvent::Status(LiveStatus::Reconnecting));
            if !ctrl.wait_backoff(reconnect.delays[attempt]) {
                break;
            }
            attempt += 1;
            continue;
        }
        let msg = match io_error {
            Some(e) => format!("Connection lost: {e}"),
            None => "Connection to host screen lost.".to_owned(),
        };
        ctrl.push_event(LiveEvent::Failed(msg.clone()));
        ctrl.push_event(LiveEvent::Status(LiveStatus::Disconnected(msg)));
        break;
    }

    retire_live_session(&source_active, &ctrl);
}

/// Retire a live session that reached terminal completion: clear `source.active`,
/// mark `WriteQueue::terminal = true` (rejecting any subsequent control calls),
/// and finish `deliver_q` so `stream-d` drains its final events and exits cleanly.
fn retire_live_session(source_active: &Mutex<Option<Arc<StreamControl>>>, ctrl: &Arc<StreamControl>) {
    {
        let mut slot = source_active.lock().unwrap_or_else(|e| e.into_inner());
        if slot.as_ref().is_some_and(|cur| Arc::ptr_eq(cur, ctrl)) {
            *slot = None;
        }
    }
    {
        let mut wq = ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
        wq.connected = false;
        wq.terminal = true;
        wq.control_state = ControlState::Idle;
        wq.pending_take_seq = None;
        wq.release_done_seq = wq.release_done_seq.max(wq.release_req_seq);
        ctrl.release_cvar.notify_all();
    }
    ctrl.finish_delivery();
}

// ---------------------------------------------------------------------------
// History & `frame_at` stream (`follow_history`, `frame_at`, `cancel_frame_at`)
// ---------------------------------------------------------------------------

struct HistoryFrameState {
    /// Monotonic ticket; bumped on every `frame_at` call or `cancel_frame_at`
    /// so any earlier blocked waiter wakes and exits immediately.
    latest_req: u64,
    /// Single-slot outgoing `FrameAt` request; a newer request overwrites any
    /// older unsent one before hitting the wire.
    pending_write: Option<(u64, u64)>,
    /// Currently awaited `(req, at_ms)` so a brief reconnect transparently
    /// replays the latest scrub if still active.
    in_flight: Option<(u64, u64)>,
    /// Reply matching `req` deposited by `hist-s`.
    ready: Option<(u64, FrameReply)>,
    /// True while a connection's `hist-w` writer is running.
    connected: bool,
    /// Set to make `hist-w` exit cleanly on connection teardown.
    writer_stop: bool,
    /// Terminal error aborting any blocked `frame_at` caller.
    abort_reason: Option<String>,
}

struct HistoryControl {
    stream_id: usize,
    stop: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    /// Bumped whenever `set_active(true)` re-enables a previously failed stream.
    resume_epoch: AtomicUsize,
    /// Set on `set_active(true)` so even an unchanged reconnect snapshot emits
    /// a `Loaded` event allowing Replay to clear any earlier error banner.
    need_reactivation_emit: AtomicBool,
    conn_cancel: Mutex<Arc<AtomicBool>>,
    state_lock: Mutex<()>,
    state_cvar: Condvar,
    cancel: Mutex<CancelSlot>,
    frame: Mutex<HistoryFrameState>,
    frame_cvar: Condvar,
    live_threads: AtomicUsize,
}

impl HistoryControl {
    fn new() -> Arc<HistoryControl> {
        static NEXT_HIST: AtomicUsize = AtomicUsize::new(1);
        Arc::new(HistoryControl {
            stream_id: NEXT_HIST.fetch_add(1, Ordering::Relaxed),
            stop: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicBool::new(true)),
            resume_epoch: AtomicUsize::new(0),
            need_reactivation_emit: AtomicBool::new(false),
            conn_cancel: Mutex::new(Arc::new(AtomicBool::new(false))),
            state_lock: Mutex::new(()),
            state_cvar: Condvar::new(),
            cancel: Mutex::new(CancelSlot { stopped: false, hook: None }),
            frame: Mutex::new(HistoryFrameState { latest_req: 0, pending_write: None, in_flight: None, ready: None, connected: false, writer_stop: false, abort_reason: None }),
            frame_cvar: Condvar::new(),
            live_threads: AtomicUsize::new(0),
        })
    }

    fn fire_cancel(&self) {
        let hook = self.cancel.lock().unwrap_or_else(|e| e.into_inner()).hook.take();
        if let Some(f) = hook {
            f();
        }
    }

    /// Sleep while `!active && !stop`, waking immediately when activated or stopped.
    fn wait_until_active(&self) -> bool {
        let mut g = self.state_lock.lock().unwrap_or_else(|e| e.into_inner());
        while !self.active.load(Ordering::Acquire) && !self.stop.load(Ordering::Acquire) {
            g = self.state_cvar.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        !self.stop.load(Ordering::Acquire)
    }

    /// Sleep after a non-retryable error until `set_active(true)` bumps `resume_epoch` or `stop`.
    fn wait_after_terminal_error(&self, failed_epoch: usize) -> bool {
        let mut g = self.state_lock.lock().unwrap_or_else(|e| e.into_inner());
        while !self.stop.load(Ordering::Acquire) && (!self.active.load(Ordering::Acquire) || self.resume_epoch.load(Ordering::Acquire) == failed_epoch) {
            g = self.state_cvar.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        !self.stop.load(Ordering::Acquire)
    }

    /// Sleep up to `delay`, waking immediately if `stop` or `!active`.
    fn wait_backoff(&self, delay: Duration) -> bool {
        let g = self.state_lock.lock().unwrap_or_else(|e| e.into_inner());
        if self.stop.load(Ordering::Acquire) || !self.active.load(Ordering::Acquire) {
            return false;
        }
        let _ = self.state_cvar.wait_timeout(g, delay);
        !self.stop.load(Ordering::Acquire) && self.active.load(Ordering::Acquire)
    }
}

struct HistThreadGuard(Arc<HistoryControl>);

impl HistThreadGuard {
    fn enter(ctrl: Arc<HistoryControl>) -> HistThreadGuard {
        ctrl.live_threads.fetch_add(1, Ordering::AcqRel);
        HistThreadGuard(ctrl)
    }
}

impl Drop for HistThreadGuard {
    fn drop(&mut self) {
        self.0.live_threads.fetch_sub(1, Ordering::AcqRel);
    }
}

struct StreamHistoryHandle {
    ctrl: Arc<HistoryControl>,
    source_history: Option<Arc<Mutex<Option<Arc<HistoryControl>>>>>,
    session_join: Option<JoinHandle<()>>,
}

impl StreamHistoryHandle {
    fn shutdown(&mut self) {
        {
            let _g = self.ctrl.state_lock.lock().unwrap_or_else(|e| e.into_inner());
            self.ctrl.stop.store(true, Ordering::SeqCst);
            self.ctrl.active.store(false, Ordering::SeqCst);
            self.ctrl.conn_cancel.lock().unwrap_or_else(|e| e.into_inner()).store(true, Ordering::SeqCst);
            self.ctrl.state_cvar.notify_all();
        }
        if let Some(slot_arc) = self.source_history.take() {
            let mut slot = slot_arc.lock().unwrap_or_else(|e| e.into_inner());
            if slot.as_ref().is_some_and(|cur| Arc::ptr_eq(cur, &self.ctrl)) {
                *slot = None;
            }
        }
        let hook = {
            let mut c = self.ctrl.cancel.lock().unwrap_or_else(|e| e.into_inner());
            c.stopped = true;
            c.hook.take()
        };
        if let Some(f) = hook {
            f();
        }
        {
            let mut f = self.ctrl.frame.lock().unwrap_or_else(|e| e.into_inner());
            f.latest_req = f.latest_req.wrapping_add(1);
            f.connected = false;
            f.writer_stop = true;
            f.pending_write = None;
            f.in_flight = None;
            f.ready = None;
            self.ctrl.frame_cvar.notify_all();
        }
        // Reap history worker off the UI thread so switching/closing a pane
        // never blocks on a stalled transport.
        if let Some(h) = self.session_join.take() {
            let _ = std::thread::Builder::new().name("hist-reap".into()).spawn(move || {
                let _ = h.join();
            });
        }
    }
}

impl HistoryHandle for StreamHistoryHandle {
    fn stop(&mut self) {
        self.shutdown();
    }

    fn set_active(&mut self, active: bool) {
        let was = {
            let _g = self.ctrl.state_lock.lock().unwrap_or_else(|e| e.into_inner());
            let prev = self.ctrl.active.swap(active, Ordering::SeqCst);
            if active && !prev {
                self.ctrl.resume_epoch.fetch_add(1, Ordering::AcqRel);
                self.ctrl.need_reactivation_emit.store(true, Ordering::Release);
                // Do NOT clear `abort_reason` until the new connection opens and
                // activates its writer; invalidate any pre-reactivation ticket
                // and wake waiters immediately.
                {
                    let mut f = self.ctrl.frame.lock().unwrap_or_else(|e| e.into_inner());
                    f.latest_req = f.latest_req.wrapping_add(1);
                    f.pending_write = None;
                    f.in_flight = None;
                    f.ready = None;
                    self.ctrl.frame_cvar.notify_all();
                }
                self.ctrl.state_cvar.notify_all();
            } else if !active && prev {
                self.ctrl.conn_cancel.lock().unwrap_or_else(|e| e.into_inner()).store(true, Ordering::SeqCst);
                self.ctrl.state_cvar.notify_all();
            }
            prev
        };
        if !active && was {
            self.ctrl.fire_cancel();
            let mut f = self.ctrl.frame.lock().unwrap_or_else(|e| e.into_inner());
            f.latest_req = f.latest_req.wrapping_add(1);
            f.connected = false;
            f.writer_stop = true;
            f.pending_write = None;
            f.in_flight = None;
            f.ready = None;
            self.ctrl.frame_cvar.notify_all();
        }
    }
}

impl Drop for StreamHistoryHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Per-connection writer for `olo-rex-link screen history`: sleeps on
/// `frame_cvar` at rest (0 wakeups/s), writes only the latest queued
/// `HistoryRequest::FrameAt`, and drops `writer` (sending EOF so the host
/// helper exits) when the connection closes.
fn spawn_history_writer(ctrl: Arc<HistoryControl>, conn_cancel: Arc<AtomicBool>, mut writer: Box<dyn Write + Send>) -> JoinHandle<()> {
    let tname = format!("hist-w{}", ctrl.stream_id);
    std::thread::Builder::new()
        .name(tname)
        .spawn(move || {
            let _guard = HistThreadGuard::enter(ctrl.clone());
            loop {
                let req = {
                    let mut f = ctrl.frame.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        if f.writer_stop || !f.connected || ctrl.stop.load(Ordering::Acquire) || conn_cancel.load(Ordering::Acquire) {
                            break None;
                        }
                        if let Some(pair) = f.pending_write.take() {
                            break Some(pair);
                        }
                        f = ctrl.frame_cvar.wait(f).unwrap_or_else(|e| e.into_inner());
                    }
                };
                let Some((req_id, at)) = req else { break };
                if write_history_request(&mut writer, &HistoryRequest::FrameAt { req: req_id, at }).is_err() {
                    ctrl.fire_cancel();
                    break;
                }
            }
            drop(writer);
        })
        .expect("spawn hist-write")
}

struct HistoryRetentionLimits {
    max_actions: usize,
    max_action_bytes: usize,
    max_frames: usize,
}

enum ActionsSync {
    Idle,
    /// Staging a fresh or reset multi-chunk action snapshot until `SnapshotDone`.
    Staging {
        actions: Vec<Action>,
        bytes: Vec<usize>,
        total_bytes: usize,
    },
    /// Incrementally prefix-matching a reconnect snapshot against `delivered_actions`.
    Matching {
        matched: usize,
        tail: Vec<Action>,
        tail_bytes: Vec<usize>,
        tail_total_bytes: usize,
    },
    /// Reconnect snapshot diverged mid-chunk; staging replacement until `SnapshotDone`.
    Diverged {
        actions: Vec<Action>,
        bytes: Vec<usize>,
        total_bytes: usize,
    },
}

fn trim_retained_actions(delivered: &mut Vec<Action>, delivered_bytes: &mut Vec<usize>, total_bytes: &mut usize, limits: &HistoryRetentionLimits) -> bool {
    let mut trimmed = false;
    while delivered.len() > limits.max_actions || (*total_bytes > limits.max_action_bytes && delivered.len() > 1) {
        let drop_n = (delivered.len() / 4).max(1).min(delivered.len() - 1);
        delivered.drain(..drop_n);
        let removed_bytes: usize = delivered_bytes.drain(..drop_n).sum();
        *total_bytes = total_bytes.saturating_sub(removed_bytes);
        trimmed = true;
    }
    trimmed
}

/// Session thread maintaining the host `screen history-stream` channel while
/// `active` is true, sleeping with zero open streams while `active` is false,
/// bounding retained history memory, staging multi-chunk snapshots until
/// `SnapshotDone` (when advertised by the host), invalidating decoded state on
/// new `index_gen`, and deduplicating reconnect snapshots without losing or
/// doubling actions.
#[allow(clippy::too_many_arguments)]
fn run_history_session(
    name: String,
    opener: Arc<dyn StreamOpener>,
    protocol_cache: Arc<HostProtocolCache>,
    info: Arc<Mutex<ScreenInfo>>,
    reconnect: ReconnectPolicy,
    limits: HistoryRetentionLimits,
    ctrl: Arc<HistoryControl>,
    on_event: Box<dyn Fn(HistoryEvent) + Send + Sync>,
) {
    let _guard = HistThreadGuard::enter(ctrl.clone());
    if let Err(msg) = screens::require_valid_name(&name) {
        {
            let mut f = ctrl.frame.lock().unwrap_or_else(|e| e.into_inner());
            f.in_flight = None;
            f.abort_reason = Some(msg.clone());
            ctrl.frame_cvar.notify_all();
        }
        on_event(HistoryEvent::Failed(msg));
        return;
    }
    let host_caps = protocol_cache.get_or_probe(&*opener);
    let use_snapshot_done = {
        let mut inf = info.lock().unwrap_or_else(|e| e.into_inner());
        host_caps.apply_to(&mut inf.capabilities);
        inf.capabilities.history_snapshot_done
    };
    let mut loaded_once = false;
    let mut delivered_actions: Vec<Action> = Vec::new();
    let mut delivered_bytes: Vec<usize> = Vec::new();
    let mut total_action_bytes: usize = 0;
    let mut current_index: Arc<FrameIndex> = Arc::new(FrameIndex::default());
    let mut last_actions_gen: Option<u64> = None;
    let mut last_index_gen: Option<u64> = None;
    let mut attempt: usize = 0;

    loop {
        if !ctrl.wait_until_active() {
            break;
        }
        let open_epoch = ctrl.resume_epoch.load(Ordering::Acquire);
        let conn_cancel = Arc::new(AtomicBool::new(false));
        {
            let _g = ctrl.state_lock.lock().unwrap_or_else(|e| e.into_inner());
            if ctrl.stop.load(Ordering::Acquire) || !ctrl.active.load(Ordering::Acquire) {
                continue;
            }
            *ctrl.conn_cancel.lock().unwrap_or_else(|e| e.into_inner()) = conn_cancel.clone();
        }

        let opened = opener.open_history(&name, conn_cancel.clone());
        if ctrl.stop.load(Ordering::Acquire) || !ctrl.active.load(Ordering::Acquire) || conn_cancel.load(Ordering::Acquire) {
            if let Ok(LiveStream { cancel: Some(f), .. }) = opened {
                f();
            }
            attempt = 0;
            continue;
        }
        let LiveStream { reader, writer, cancel } = match opened {
            Ok(s) => s,
            Err(msg) => {
                if attempt < reconnect.delays.len() {
                    if !ctrl.wait_backoff(reconnect.delays[attempt]) {
                        attempt = 0;
                        continue;
                    }
                    attempt += 1;
                    continue;
                }
                {
                    let mut f = ctrl.frame.lock().unwrap_or_else(|e| e.into_inner());
                    f.in_flight = None;
                    f.abort_reason = Some(msg.clone());
                    ctrl.frame_cvar.notify_all();
                }
                on_event(HistoryEvent::Failed(msg));
                attempt = 0;
                if !ctrl.wait_after_terminal_error(open_epoch) {
                    break;
                }
                continue;
            }
        };

        // Install cancel hook atomically against `stop` / `set_active(false)`.
        {
            let mut slot = ctrl.cancel.lock().unwrap_or_else(|e| e.into_inner());
            if slot.stopped || ctrl.stop.load(Ordering::Acquire) || !ctrl.active.load(Ordering::Acquire) || conn_cancel.load(Ordering::Acquire) {
                drop(slot);
                if let Some(f) = cancel {
                    f();
                }
                drop(writer);
                drop(reader);
                attempt = 0;
                continue;
            }
            slot.hook = cancel;
        }

        // Activate writer and clear previous terminal error ONLY now that a
        // fresh connection is open and ready to service `FrameAt`.
        {
            let mut f = ctrl.frame.lock().unwrap_or_else(|e| e.into_inner());
            f.connected = true;
            f.writer_stop = false;
            f.abort_reason = None;
            if let Some((req, at)) = f.in_flight
                && req == f.latest_req
                && f.ready.is_none()
            {
                f.pending_write = Some((req, at));
            }
            ctrl.frame_cvar.notify_all();
        }
        let writer_join = spawn_history_writer(ctrl.clone(), conn_cancel.clone(), writer);

        let mut buf_reader = BufReader::new(reader);
        let mut canvas = Frame { width: 0, height: 0, rgba: Vec::new() };
        let mut in_snapshot = use_snapshot_done;
        let mut actions_sync = if loaded_once { ActionsSync::Matching { matched: 0, tail: Vec::new(), tail_bytes: Vec::new(), tail_total_bytes: 0 } } else { ActionsSync::Idle };
        let mut staged_index: Option<FrameIndex> = None;
        let mut saw_initial_actions = false;
        let mut saw_initial_index = false;
        let mut emitted_this_conn = false;
        let mut emitted_index_reset_this_snapshot = false;
        let mut protocol_error: Option<String> = None;
        let mut host_error: Option<String> = None;
        let mut io_error: Option<String> = None;

        loop {
            if ctrl.stop.load(Ordering::Acquire) || !ctrl.active.load(Ordering::Acquire) || conn_cancel.load(Ordering::Acquire) {
                break;
            }
            match read_history_msg_bounded(&mut buf_reader) {
                Ok(Some(DecodedHistoryMsg::Actions { reset, actions: parsed, raw_bytes })) => {
                    saw_initial_actions = true;
                    if !use_snapshot_done && saw_initial_actions && saw_initial_index {
                        attempt = 0;
                    }
                    let per_row_bytes = if parsed.is_empty() { 0 } else { (raw_bytes / parsed.len()).max(16) };
                    if use_snapshot_done {
                        if in_snapshot || reset {
                            in_snapshot = true;
                            if !loaded_once {
                                match &mut actions_sync {
                                    ActionsSync::Staging { actions, bytes, total_bytes } if !reset => {
                                        bytes.extend(std::iter::repeat_n(per_row_bytes, parsed.len()));
                                        *total_bytes = total_bytes.saturating_add(per_row_bytes.saturating_mul(parsed.len()));
                                        actions.extend(parsed);
                                        trim_retained_actions(actions, bytes, total_bytes, &limits);
                                    }
                                    _ => {
                                        let mut actions = parsed;
                                        let mut bytes = vec![per_row_bytes; actions.len()];
                                        let mut total_bytes: usize = bytes.iter().sum();
                                        trim_retained_actions(&mut actions, &mut bytes, &mut total_bytes, &limits);
                                        actions_sync = ActionsSync::Staging { actions, bytes, total_bytes };
                                    }
                                }
                            } else {
                                // Already loaded before: either a reconnect prefix-match snapshot or a live reset.
                                match std::mem::replace(&mut actions_sync, ActionsSync::Idle) {
                                    ActionsSync::Matching { mut matched, mut tail, mut tail_bytes, mut tail_total_bytes } if reset && matched == 0 => {
                                        let prefix_len = delivered_actions.len();
                                        if parsed.len() <= prefix_len && parsed[..] == delivered_actions[..parsed.len()] {
                                            matched = parsed.len();
                                            actions_sync = ActionsSync::Matching { matched, tail, tail_bytes, tail_total_bytes };
                                        } else if parsed.len() > prefix_len && parsed[..prefix_len] == delivered_actions[..] {
                                            matched = prefix_len;
                                            let add = &parsed[prefix_len..];
                                            tail_bytes.extend(std::iter::repeat_n(per_row_bytes, add.len()));
                                            tail_total_bytes = tail_total_bytes.saturating_add(per_row_bytes.saturating_mul(add.len()));
                                            tail.extend(add.iter().cloned());
                                            trim_retained_actions(&mut tail, &mut tail_bytes, &mut tail_total_bytes, &limits);
                                            actions_sync = ActionsSync::Matching { matched, tail, tail_bytes, tail_total_bytes };
                                        } else {
                                            let mut actions = parsed;
                                            let mut bytes = vec![per_row_bytes; actions.len()];
                                            let mut total_bytes: usize = bytes.iter().sum();
                                            trim_retained_actions(&mut actions, &mut bytes, &mut total_bytes, &limits);
                                            actions_sync = ActionsSync::Diverged { actions, bytes, total_bytes };
                                        }
                                    }
                                    ActionsSync::Matching { mut matched, mut tail, mut tail_bytes, mut tail_total_bytes } if !reset => {
                                        if matched < delivered_actions.len() {
                                            let rem = delivered_actions.len() - matched;
                                            if parsed.len() <= rem && parsed[..] == delivered_actions[matched..matched + parsed.len()] {
                                                matched += parsed.len();
                                                actions_sync = ActionsSync::Matching { matched, tail, tail_bytes, tail_total_bytes };
                                            } else if parsed.len() > rem && parsed[..rem] == delivered_actions[matched..] {
                                                matched = delivered_actions.len();
                                                let add = &parsed[rem..];
                                                tail_bytes.extend(std::iter::repeat_n(per_row_bytes, add.len()));
                                                tail_total_bytes = tail_total_bytes.saturating_add(per_row_bytes.saturating_mul(add.len()));
                                                tail.extend(add.iter().cloned());
                                                trim_retained_actions(&mut tail, &mut tail_bytes, &mut tail_total_bytes, &limits);
                                                actions_sync = ActionsSync::Matching { matched, tail, tail_bytes, tail_total_bytes };
                                            } else {
                                                let mut actions = delivered_actions[..matched].to_vec();
                                                let mut bytes = delivered_bytes[..matched].to_vec();
                                                bytes.extend(std::iter::repeat_n(per_row_bytes, parsed.len()));
                                                actions.extend(parsed);
                                                let mut total_bytes: usize = bytes.iter().sum();
                                                trim_retained_actions(&mut actions, &mut bytes, &mut total_bytes, &limits);
                                                actions_sync = ActionsSync::Diverged { actions, bytes, total_bytes };
                                            }
                                        } else {
                                            tail_bytes.extend(std::iter::repeat_n(per_row_bytes, parsed.len()));
                                            tail_total_bytes = tail_total_bytes.saturating_add(per_row_bytes.saturating_mul(parsed.len()));
                                            tail.extend(parsed);
                                            trim_retained_actions(&mut tail, &mut tail_bytes, &mut tail_total_bytes, &limits);
                                            actions_sync = ActionsSync::Matching { matched, tail, tail_bytes, tail_total_bytes };
                                        }
                                    }
                                    ActionsSync::Diverged { mut actions, mut bytes, mut total_bytes } if !reset => {
                                        bytes.extend(std::iter::repeat_n(per_row_bytes, parsed.len()));
                                        total_bytes = total_bytes.saturating_add(per_row_bytes.saturating_mul(parsed.len()));
                                        actions.extend(parsed);
                                        trim_retained_actions(&mut actions, &mut bytes, &mut total_bytes, &limits);
                                        actions_sync = ActionsSync::Diverged { actions, bytes, total_bytes };
                                    }
                                    ActionsSync::Staging { mut actions, mut bytes, mut total_bytes } if !reset => {
                                        bytes.extend(std::iter::repeat_n(per_row_bytes, parsed.len()));
                                        total_bytes = total_bytes.saturating_add(per_row_bytes.saturating_mul(parsed.len()));
                                        actions.extend(parsed);
                                        trim_retained_actions(&mut actions, &mut bytes, &mut total_bytes, &limits);
                                        actions_sync = ActionsSync::Staging { actions, bytes, total_bytes };
                                    }
                                    _ => {
                                        // Explicit mid-connection `Actions { reset: true }`.
                                        let mut actions = parsed;
                                        let mut bytes = vec![per_row_bytes; actions.len()];
                                        let mut total_bytes: usize = bytes.iter().sum();
                                        trim_retained_actions(&mut actions, &mut bytes, &mut total_bytes, &limits);
                                        actions_sync = ActionsSync::Staging { actions, bytes, total_bytes };
                                    }
                                }
                            }
                        } else if !parsed.is_empty() {
                            // Incremental live append outside a snapshot.
                            delivered_bytes.extend(std::iter::repeat_n(per_row_bytes, parsed.len()));
                            total_action_bytes = total_action_bytes.saturating_add(per_row_bytes.saturating_mul(parsed.len()));
                            delivered_actions.extend(parsed.iter().cloned());
                            if trim_retained_actions(&mut delivered_actions, &mut delivered_bytes, &mut total_action_bytes, &limits) {
                                emitted_this_conn = true;
                                on_event(HistoryEvent::loaded_with_index(delivered_actions.clone(), true, false, current_index.clone()));
                            } else {
                                emitted_this_conn = true;
                                on_event(HistoryEvent::loaded_with_index(parsed, false, false, current_index.clone()));
                            }
                        }
                    } else if !loaded_once {
                        loaded_once = true;
                        actions_sync = ActionsSync::Idle;
                        delivered_actions = parsed;
                        delivered_bytes = vec![per_row_bytes; delivered_actions.len()];
                        total_action_bytes = delivered_bytes.iter().sum();
                        trim_retained_actions(&mut delivered_actions, &mut delivered_bytes, &mut total_action_bytes, &limits);
                        emitted_this_conn = true;
                        on_event(HistoryEvent::loaded_with_index(delivered_actions.clone(), true, false, current_index.clone()));
                    } else if reset && matches!(actions_sync, ActionsSync::Matching { .. }) {
                        // Legacy helper without `SnapshotDone`: compare incrementally starting at 0.
                        let prefix_len = delivered_actions.len();
                        if parsed.len() <= prefix_len && parsed[..] == delivered_actions[..parsed.len()] {
                            actions_sync = ActionsSync::Matching { matched: parsed.len(), tail: Vec::new(), tail_bytes: Vec::new(), tail_total_bytes: 0 };
                        } else if parsed.len() > prefix_len && parsed[..prefix_len] == delivered_actions[..] {
                            let tail = parsed[prefix_len..].to_vec();
                            actions_sync = ActionsSync::Idle;
                            if !tail.is_empty() {
                                delivered_bytes.extend(std::iter::repeat_n(per_row_bytes, tail.len()));
                                total_action_bytes = total_action_bytes.saturating_add(per_row_bytes * tail.len());
                                delivered_actions.extend(tail.iter().cloned());
                                if trim_retained_actions(&mut delivered_actions, &mut delivered_bytes, &mut total_action_bytes, &limits) {
                                    emitted_this_conn = true;
                                    on_event(HistoryEvent::loaded_with_index(delivered_actions.clone(), true, false, current_index.clone()));
                                } else {
                                    emitted_this_conn = true;
                                    on_event(HistoryEvent::loaded_with_index(tail, false, false, current_index.clone()));
                                }
                            }
                        } else {
                            actions_sync = ActionsSync::Idle;
                            delivered_actions = parsed;
                            delivered_bytes = vec![per_row_bytes; delivered_actions.len()];
                            total_action_bytes = delivered_bytes.iter().sum();
                            trim_retained_actions(&mut delivered_actions, &mut delivered_bytes, &mut total_action_bytes, &limits);
                            emitted_this_conn = true;
                            on_event(HistoryEvent::loaded_with_index(delivered_actions.clone(), true, false, current_index.clone()));
                        }
                    } else if !reset && let ActionsSync::Matching { matched, .. } = actions_sync {
                        let rem = delivered_actions.len().saturating_sub(matched);
                        if parsed.len() <= rem && parsed[..] == delivered_actions[matched..matched + parsed.len()] {
                            actions_sync = ActionsSync::Matching { matched: matched + parsed.len(), tail: Vec::new(), tail_bytes: Vec::new(), tail_total_bytes: 0 };
                        } else if parsed.len() > rem && parsed[..rem] == delivered_actions[matched..] {
                            let tail = parsed[rem..].to_vec();
                            actions_sync = ActionsSync::Idle;
                            if !tail.is_empty() {
                                delivered_bytes.extend(std::iter::repeat_n(per_row_bytes, tail.len()));
                                total_action_bytes = total_action_bytes.saturating_add(per_row_bytes * tail.len());
                                delivered_actions.extend(tail.iter().cloned());
                                if trim_retained_actions(&mut delivered_actions, &mut delivered_bytes, &mut total_action_bytes, &limits) {
                                    emitted_this_conn = true;
                                    on_event(HistoryEvent::loaded_with_index(delivered_actions.clone(), true, false, current_index.clone()));
                                } else {
                                    emitted_this_conn = true;
                                    on_event(HistoryEvent::loaded_with_index(tail, false, false, current_index.clone()));
                                }
                            }
                        } else {
                            actions_sync = ActionsSync::Idle;
                            delivered_actions.truncate(matched);
                            delivered_bytes.truncate(matched);
                            delivered_bytes.extend(std::iter::repeat_n(per_row_bytes, parsed.len()));
                            delivered_actions.extend(parsed);
                            total_action_bytes = delivered_bytes.iter().sum();
                            trim_retained_actions(&mut delivered_actions, &mut delivered_bytes, &mut total_action_bytes, &limits);
                            emitted_this_conn = true;
                            on_event(HistoryEvent::loaded_with_index(delivered_actions.clone(), true, false, current_index.clone()));
                        }
                    } else if reset {
                        actions_sync = ActionsSync::Idle;
                        delivered_actions = parsed;
                        delivered_bytes = vec![per_row_bytes; delivered_actions.len()];
                        total_action_bytes = delivered_bytes.iter().sum();
                        trim_retained_actions(&mut delivered_actions, &mut delivered_bytes, &mut total_action_bytes, &limits);
                        emitted_this_conn = true;
                        on_event(HistoryEvent::loaded_with_index(delivered_actions.clone(), true, false, current_index.clone()));
                    } else {
                        actions_sync = ActionsSync::Idle;
                        if !parsed.is_empty() {
                            delivered_bytes.extend(std::iter::repeat_n(per_row_bytes, parsed.len()));
                            total_action_bytes = total_action_bytes.saturating_add(per_row_bytes * parsed.len());
                            delivered_actions.extend(parsed.iter().cloned());
                            if trim_retained_actions(&mut delivered_actions, &mut delivered_bytes, &mut total_action_bytes, &limits) {
                                emitted_this_conn = true;
                                on_event(HistoryEvent::loaded_with_index(delivered_actions.clone(), true, false, current_index.clone()));
                            } else {
                                emitted_this_conn = true;
                                on_event(HistoryEvent::loaded_with_index(parsed, false, false, current_index.clone()));
                            }
                        }
                    }
                }
                Ok(Some(DecodedHistoryMsg::Index { reset, last_size, frames: new_marks })) => {
                    saw_initial_index = true;
                    if use_snapshot_done {
                        if reset {
                            if new_marks.len() > limits.max_frames {
                                protocol_error = Some(format!("history index exceeds desktop cap of {} frames", limits.max_frames));
                                break;
                            }
                            in_snapshot = true;
                            staged_index = Some(FrameIndex { frames: new_marks, last_size });
                        } else if in_snapshot && let Some(ref mut idx) = staged_index {
                            // Multi-chunk Index reset continuation (staged until `SnapshotDone`).
                            if let (Some(prev_last), Some(first_new)) = (idx.frames.last(), new_marks.first())
                                && first_new.at < prev_last.at
                            {
                                protocol_error = Some(format!("history index append out of order ({} < {})", first_new.at, prev_last.at));
                                break;
                            }
                            let next_len = idx.frames.len().saturating_add(new_marks.len());
                            if next_len > limits.max_frames {
                                protocol_error = Some(format!("history index exceeds desktop cap of {} frames", limits.max_frames));
                                break;
                            }
                            idx.frames.extend(new_marks);
                            idx.last_size = last_size;
                        } else {
                            // Incremental live Index append outside a snapshot.
                            if let (Some(prev_last), Some(first_new)) = (current_index.frames.last(), new_marks.first())
                                && first_new.at < prev_last.at
                            {
                                protocol_error = Some(format!("history index append out of order ({} < {})", first_new.at, prev_last.at));
                                break;
                            }
                            let next_len = current_index.frames.len().saturating_add(new_marks.len());
                            if next_len > limits.max_frames {
                                protocol_error = Some(format!("history index exceeds desktop cap of {} frames", limits.max_frames));
                                break;
                            }
                            if !new_marks.is_empty() || last_size != current_index.last_size {
                                let mut next = (*current_index).clone();
                                next.frames.extend(new_marks);
                                next.last_size = last_size;
                                current_index = Arc::new(next);
                                emitted_this_conn = true;
                                on_event(HistoryEvent::Loaded { actions: Vec::new(), texts: Vec::new(), reset: false, index_reset: false, frames: current_index.clone() });
                            }
                        }
                    } else {
                        // Legacy host without `SnapshotDone`: initial `Index` closes reconnect action-prefix sync.
                        if let ActionsSync::Matching { matched, .. } = actions_sync {
                            actions_sync = ActionsSync::Idle;
                            if matched < delivered_actions.len() {
                                delivered_actions.truncate(matched);
                                delivered_bytes.truncate(matched);
                                total_action_bytes = delivered_bytes.iter().sum();
                                emitted_this_conn = true;
                                on_event(HistoryEvent::loaded_with_index(delivered_actions.clone(), true, false, current_index.clone()));
                            }
                        }
                        if saw_initial_actions && saw_initial_index {
                            attempt = 0;
                        }
                        if reset {
                            if new_marks.len() > limits.max_frames {
                                protocol_error = Some(format!("history index exceeds desktop cap of {} frames", limits.max_frames));
                                break;
                            }
                            current_index = Arc::new(FrameIndex { frames: new_marks, last_size });
                            emitted_this_conn = true;
                            emitted_index_reset_this_snapshot = true;
                            {
                                let mut f = ctrl.frame.lock().unwrap_or_else(|e| e.into_inner());
                                f.ready = None;
                            }
                            on_event(HistoryEvent::Loaded { actions: Vec::new(), texts: Vec::new(), reset: false, index_reset: true, frames: current_index.clone() });
                        } else {
                            if let (Some(prev_last), Some(first_new)) = (current_index.frames.last(), new_marks.first())
                                && first_new.at < prev_last.at
                            {
                                protocol_error = Some(format!("history index append out of order ({} < {})", first_new.at, prev_last.at));
                                break;
                            }
                            let next_len = current_index.frames.len().saturating_add(new_marks.len());
                            if next_len > limits.max_frames {
                                protocol_error = Some(format!("history index exceeds desktop cap of {} frames", limits.max_frames));
                                break;
                            }
                            if !new_marks.is_empty() || last_size != current_index.last_size {
                                let mut next = (*current_index).clone();
                                next.frames.extend(new_marks);
                                next.last_size = last_size;
                                current_index = Arc::new(next);
                                emitted_this_conn = true;
                                on_event(HistoryEvent::Loaded { actions: Vec::new(), texts: Vec::new(), reset: false, index_reset: false, frames: current_index.clone() });
                            }
                        }
                        if saw_initial_actions && saw_initial_index && ctrl.need_reactivation_emit.swap(false, Ordering::AcqRel) && !emitted_this_conn {
                            on_event(HistoryEvent::Loaded { actions: Vec::new(), texts: Vec::new(), reset: false, index_reset: false, frames: current_index.clone() });
                        }
                    }
                }
                Ok(Some(DecodedHistoryMsg::Frame { req, frame_at, delta })) => {
                    // Always decode and apply EVERY incoming frame onto `canvas`
                    // even if `req` was already superseded locally, because the
                    // host's next delta on this stream is encoded against it.
                    if let Err(e) = apply_delta(&delta, &mut canvas) {
                        protocol_error = Some(e);
                        break;
                    }
                    let mut f = ctrl.frame.lock().unwrap_or_else(|e| e.into_inner());
                    if req == f.latest_req {
                        f.in_flight = None;
                        f.ready = Some((req, Ok(Some((frame_at, canvas.clone())))));
                        ctrl.frame_cvar.notify_all();
                    }
                }
                Ok(Some(DecodedHistoryMsg::Miss { req })) => {
                    let mut f = ctrl.frame.lock().unwrap_or_else(|e| e.into_inner());
                    if req == f.latest_req {
                        f.in_flight = None;
                        f.ready = Some((req, Ok(None)));
                        ctrl.frame_cvar.notify_all();
                    }
                }
                Ok(Some(DecodedHistoryMsg::FrameError { req, message })) => {
                    // Per-request non-fatal error (e.g. oversized recorded frame):
                    // fail only this request while keeping the history stream open.
                    let mut f = ctrl.frame.lock().unwrap_or_else(|e| e.into_inner());
                    if req == f.latest_req {
                        f.in_flight = None;
                        f.ready = Some((req, Err(message)));
                        ctrl.frame_cvar.notify_all();
                    }
                }
                Ok(Some(DecodedHistoryMsg::SnapshotDone { actions_gen, index_gen })) => {
                    let _actions_gen_changed = last_actions_gen.is_some_and(|prev| prev != actions_gen);
                    let index_gen_changed = last_index_gen.is_some_and(|prev| prev != index_gen);
                    last_actions_gen = Some(actions_gen);
                    last_index_gen = Some(index_gen);
                    in_snapshot = false;
                    attempt = 0;

                    let mut index_reset = index_gen_changed && !emitted_index_reset_this_snapshot;
                    emitted_index_reset_this_snapshot = false;
                    if let Some(idx) = staged_index.take() {
                        current_index = Arc::new(idx);
                        index_reset = true;
                    }
                    if index_reset {
                        // Drop any unconsumed decoded frame reply from the old `index_gen`.
                        let mut f = ctrl.frame.lock().unwrap_or_else(|e| e.into_inner());
                        f.ready = None;
                    }

                    let (emit_actions, actions_reset) = match std::mem::replace(&mut actions_sync, ActionsSync::Idle) {
                        ActionsSync::Staging { actions, bytes, total_bytes } | ActionsSync::Diverged { actions, bytes, total_bytes } => {
                            loaded_once = true;
                            delivered_actions = actions;
                            delivered_bytes = bytes;
                            total_action_bytes = total_bytes;
                            (delivered_actions.clone(), true)
                        }
                        ActionsSync::Matching { matched, tail, tail_bytes, tail_total_bytes } => {
                            if matched < delivered_actions.len() {
                                delivered_actions.truncate(matched);
                                delivered_bytes.truncate(matched);
                                total_action_bytes = delivered_bytes.iter().sum();
                                (delivered_actions.clone(), true)
                            } else if !tail.is_empty() {
                                delivered_bytes.extend(tail_bytes);
                                total_action_bytes = total_action_bytes.saturating_add(tail_total_bytes);
                                delivered_actions.extend(tail.iter().cloned());
                                if trim_retained_actions(&mut delivered_actions, &mut delivered_bytes, &mut total_action_bytes, &limits) { (delivered_actions.clone(), true) } else { (tail, false) }
                            } else {
                                (Vec::new(), false)
                            }
                        }
                        ActionsSync::Idle => (Vec::new(), false),
                    };

                    let need_react = ctrl.need_reactivation_emit.swap(false, Ordering::AcqRel);
                    if actions_reset || index_reset || !emit_actions.is_empty() || (need_react && !emitted_this_conn) {
                        emitted_this_conn = true;
                        on_event(HistoryEvent::loaded_with_index(emit_actions, actions_reset, index_reset, current_index.clone()));
                    }
                }
                Ok(Some(DecodedHistoryMsg::Error { message })) => {
                    host_error = Some(message);
                    break;
                }
                Ok(None) => break,
                Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                    protocol_error = Some(e.to_string());
                    break;
                }
                Err(e) => {
                    io_error = Some(e.to_string());
                    break;
                }
            }
        }

        // Tear down this connection cleanly: fire transport cancel BEFORE
        // joining `writer_join` so a blocked writer unblocks immediately.
        {
            let mut f = ctrl.frame.lock().unwrap_or_else(|e| e.into_inner());
            f.connected = false;
            f.writer_stop = true;
            f.pending_write = None;
            ctrl.frame_cvar.notify_all();
        }
        ctrl.fire_cancel();
        let _ = writer_join.join();
        drop(buf_reader);

        if ctrl.stop.load(Ordering::Acquire) {
            break;
        }
        if !ctrl.active.load(Ordering::Acquire) {
            attempt = 0;
            continue;
        }
        if let Some(err) = protocol_error {
            let msg = format!("History stream closed: {err}");
            {
                let mut f = ctrl.frame.lock().unwrap_or_else(|e| e.into_inner());
                f.in_flight = None;
                f.abort_reason = Some(msg.clone());
                ctrl.frame_cvar.notify_all();
            }
            on_event(HistoryEvent::Failed(msg));
            attempt = 0;
            if !ctrl.wait_after_terminal_error(open_epoch) {
                break;
            }
            continue;
        }
        if let Some(msg) = host_error {
            {
                let mut f = ctrl.frame.lock().unwrap_or_else(|e| e.into_inner());
                f.in_flight = None;
                f.abort_reason = Some(msg.clone());
                ctrl.frame_cvar.notify_all();
            }
            on_event(HistoryEvent::Failed(msg));
            attempt = 0;
            if !ctrl.wait_after_terminal_error(open_epoch) {
                break;
            }
            continue;
        }

        // Unexpected EOF / transport break: reconnect with backoff and deduplicate.
        if attempt < reconnect.delays.len() {
            if !ctrl.wait_backoff(reconnect.delays[attempt]) {
                attempt = 0;
                continue;
            }
            attempt += 1;
            continue;
        }
        let msg = match io_error {
            Some(e) => format!("History connection lost: {e}"),
            None => "History connection to host screen lost.".to_owned(),
        };
        {
            let mut f = ctrl.frame.lock().unwrap_or_else(|e| e.into_inner());
            f.in_flight = None;
            f.abort_reason = Some(msg.clone());
            ctrl.frame_cvar.notify_all();
        }
        on_event(HistoryEvent::Failed(msg));
        attempt = 0;
        if !ctrl.wait_after_terminal_error(open_epoch) {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Export helpers (`export_video`)
// ---------------------------------------------------------------------------

/// Format 0-based action indices as a compact `--part` spec (`"2-5"` or
/// `"0,2-3,7"`) understood by `olo-rex-link screen export --part`.
fn format_part_spec(actions: &[usize]) -> String {
    let mut sorted = actions.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut parts = Vec::new();
    let mut i = 0;
    while i < sorted.len() {
        let start = sorted[i];
        let mut end = start;
        while i + 1 < sorted.len() && sorted[i + 1] == end + 1 {
            i += 1;
            end = sorted[i];
        }
        if start == end {
            parts.push(start.to_string());
        } else {
            parts.push(format!("{start}-{end}"));
        }
        i += 1;
    }
    parts.join(",")
}

// ---------------------------------------------------------------------------
// `ScreenSource` implementation
// ---------------------------------------------------------------------------

impl ScreenSource for StreamScreenSource {
    fn id(&self) -> &ScreenId {
        &self.id
    }

    fn info(&self) -> ScreenInfo {
        self.info.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn async_control(&self) -> bool {
        true
    }

    fn watch_live(&self, _modifiers: Vec<u64>, on_event: Box<dyn Fn(LiveEvent) + Send + Sync>) -> Box<dyn LiveHandle> {
        let explicit_ack = self.info.lock().unwrap_or_else(|e| e.into_inner()).capabilities.explicit_control_ack;
        let ctrl = StreamControl::new(explicit_ack);
        *self.active.lock().unwrap_or_else(|e| e.into_inner()) = Some(ctrl.clone());

        let deliver_ctrl = ctrl.clone();
        let deliver_name = format!("stream-d{}", ctrl.stream_id);
        let deliver_join = std::thread::Builder::new()
            .name(deliver_name)
            .spawn(move || {
                let _guard = ThreadGuard::enter(deliver_ctrl.clone());
                loop {
                    let ev = {
                        let mut dq = deliver_ctrl.deliver_q.lock().unwrap_or_else(|e| e.into_inner());
                        loop {
                            if dq.stop {
                                return;
                            }
                            if let Some(ev) = dq.events.pop_front() {
                                break ev;
                            }
                            if dq.finish {
                                return;
                            }
                            dq = deliver_ctrl.deliver_cvar.wait(dq).unwrap_or_else(|e| e.into_inner());
                        }
                    };
                    on_event(ev);
                }
            })
            .expect("spawn stream-deliver");

        let session_ctrl = ctrl.clone();
        let session_name = format!("stream-s{}", ctrl.stream_id);
        let name = self.id.name.clone();
        let opener = self.opener.clone();
        let protocol_cache = self.protocol_cache.clone();
        let reconnect = self.reconnect.clone();
        let info = self.info.clone();
        let active_slot = self.active.clone();
        let session_join =
            std::thread::Builder::new().name(session_name).spawn(move || run_session(name, opener, protocol_cache, reconnect, info, active_slot, session_ctrl)).expect("spawn stream-session");

        Box::new(StreamLiveHandle { ctrl, source_active: Some(self.active.clone()), session_join: Some(session_join), deliver_join: Some(deliver_join) })
    }

    fn set_controlled(&self, on: bool) -> Result<(), String> {
        let ctrl = self.active.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some(ctrl) = ctrl else {
            self.info.lock().unwrap_or_else(|e| e.into_inner()).controlled = false;
            return if on { Err("Not connected to this screen.".into()) } else { Ok(()) };
        };
        let mut wq = ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
        if on {
            if wq.terminal || !wq.connected || ctrl.stop.load(Ordering::Acquire) {
                return Err("Not connected to this screen yet.".into());
            }
            if matches!(wq.control_state, ControlState::Idle | ControlState::Releasing) {
                let seq = wq.alloc_control_seq();
                wq.control_state = ControlState::Requested;
                wq.pending_take_seq = Some(seq);
                wq.cmds.push_back(WriteCmd::Control { on: true, seq });
                ctrl.write_cvar.notify_one();
            }
            Ok(())
        } else {
            let had = matches!(wq.control_state, ControlState::Requested | ControlState::Held);
            if had && wq.connected && !wq.terminal {
                let seq = wq.alloc_control_seq();
                wq.control_state = ControlState::Releasing;
                wq.pending_take_seq = None;
                wq.release_req_seq = seq;
                wq.cmds.push_back(WriteCmd::Control { on: false, seq });
                ctrl.write_cvar.notify_one();
            } else {
                wq.control_state = ControlState::Idle;
                wq.pending_take_seq = None;
            }
            drop(wq);
            self.info.lock().unwrap_or_else(|e| e.into_inner()).controlled = false;
            Ok(())
        }
    }

    fn release_control(&self, final_events: Vec<SyntheticEvent>) -> Result<(), String> {
        let ctrl = self.active.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some(ctrl) = ctrl else {
            self.info.lock().unwrap_or_else(|e| e.into_inner()).controlled = false;
            return Ok(());
        };
        let mut wq = ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
        let had = matches!(wq.control_state, ControlState::Requested | ControlState::Held);
        if had && wq.connected && !wq.terminal {
            if !final_events.is_empty() && wq.control_state == ControlState::Held {
                wq.push_input_bounded(final_events);
            }
            let seq = wq.alloc_control_seq();
            wq.control_state = ControlState::Releasing;
            wq.pending_take_seq = None;
            wq.release_req_seq = seq;
            wq.cmds.push_back(WriteCmd::Control { on: false, seq });
            ctrl.write_cvar.notify_one();
        } else {
            wq.control_state = ControlState::Idle;
            wq.pending_take_seq = None;
        }
        drop(wq);
        self.info.lock().unwrap_or_else(|e| e.into_inner()).controlled = false;
        Ok(())
    }

    fn release_control_and_wait(&self, final_events: Vec<SyntheticEvent>) -> Result<(), String> {
        let ctrl = self.active.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some(ctrl) = ctrl else {
            self.info.lock().unwrap_or_else(|e| e.into_inner()).controlled = false;
            return Ok(());
        };
        let target_seq = {
            let mut wq = ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
            let had = matches!(wq.control_state, ControlState::Requested | ControlState::Held | ControlState::Releasing);
            if !had || !wq.connected || wq.terminal {
                wq.control_state = ControlState::Idle;
                wq.pending_take_seq = None;
                drop(wq);
                self.info.lock().unwrap_or_else(|e| e.into_inner()).controlled = false;
                return Ok(());
            }
            if wq.control_state != ControlState::Releasing {
                if !final_events.is_empty() && wq.control_state == ControlState::Held {
                    wq.push_input_bounded(final_events);
                }
                let seq = wq.alloc_control_seq();
                wq.control_state = ControlState::Releasing;
                wq.pending_take_seq = None;
                wq.release_req_seq = seq;
                wq.cmds.push_back(WriteCmd::Control { on: false, seq });
                ctrl.write_cvar.notify_one();
            }
            wq.release_req_seq
        };
        self.info.lock().unwrap_or_else(|e| e.into_inner()).controlled = false;
        let deadline = Instant::now() + RELEASE_ACK_TIMEOUT;
        let mut wq = ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
        while wq.release_done_seq < target_seq && wq.connected && !wq.terminal && !ctrl.stop.load(Ordering::Acquire) {
            let rem = deadline.saturating_duration_since(Instant::now());
            if rem.is_zero() {
                break;
            }
            let (next, _) = ctrl.release_cvar.wait_timeout(wq, rem).unwrap_or_else(|e| e.into_inner());
            wq = next;
        }
        Ok(())
    }

    fn send_input(&self, events: Vec<SyntheticEvent>) {
        if events.is_empty() {
            return;
        }
        let ctrl = self.active.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some(ctrl) = ctrl else { return };
        let mut wq = ctrl.write_q.lock().unwrap_or_else(|e| e.into_inner());
        // Never enqueue or transmit input unless control is explicitly HELD on a live stream.
        if wq.control_state != ControlState::Held || !wq.connected || wq.terminal {
            return;
        }
        wq.push_input_bounded(events);
        ctrl.write_cvar.notify_one();
    }

    fn join_clipboard(&self) -> Option<Box<dyn Send>> {
        // Never bridge the person's clipboard over Rex to a remote host.
        None
    }

    fn lazy_history(&self) -> bool {
        true
    }

    fn follow_history(&self, on_event: Box<dyn Fn(HistoryEvent) + Send + Sync>) -> Box<dyn HistoryHandle> {
        let ctrl = HistoryControl::new();
        *self.history.lock().unwrap_or_else(|e| e.into_inner()) = Some(ctrl.clone());

        let session_ctrl = ctrl.clone();
        let session_name = format!("hist-s{}", ctrl.stream_id);
        let name = self.id.name.clone();
        let opener = self.opener.clone();
        let protocol_cache = self.protocol_cache.clone();
        let info = self.info.clone();
        let reconnect = self.reconnect.clone();
        let limits = HistoryRetentionLimits { max_actions: self.max_retained_actions, max_action_bytes: self.max_retained_action_bytes, max_frames: self.max_retained_frames };
        let session_join = std::thread::Builder::new()
            .name(session_name)
            .spawn(move || run_history_session(name, opener, protocol_cache, info, reconnect, limits, session_ctrl, on_event))
            .expect("spawn hist-session");

        Box::new(StreamHistoryHandle { ctrl, source_history: Some(self.history.clone()), session_join: Some(session_join) })
    }

    fn frame_at(&self, at_ms: u64) -> Result<Option<(u64, Frame)>, String> {
        let ctrl = {
            let slot = self.history.lock().unwrap_or_else(|e| e.into_inner());
            slot.as_ref().filter(|c| c.active.load(Ordering::Acquire) && !c.stop.load(Ordering::Acquire)).cloned()
        };
        let Some(ctrl) = ctrl else {
            let cancel = ExportCancel::new();
            return self.frame_at_oneshot(at_ms, &cancel);
        };

        let mut f = ctrl.frame.lock().unwrap_or_else(|e| e.into_inner());
        // Preserve terminal history errors instead of clearing them and waiting
        // forever with no writer attached.
        if let Some(err) = f.abort_reason.clone() {
            return Err(err);
        }
        let req = f.latest_req.wrapping_add(1);
        f.latest_req = req;
        f.pending_write = Some((req, at_ms));
        f.in_flight = Some((req, at_ms));
        f.ready = None;
        // Wake both the writer thread AND any earlier `frame_at` waiter so the
        // overtaken request exits immediately without waiting for the host.
        ctrl.frame_cvar.notify_all();

        let deadline = Instant::now() + self.frame_at_timeout;
        loop {
            if f.latest_req != req {
                return Ok(None);
            }
            if let Some((r, res)) = f.ready.take()
                && r == req
            {
                return res;
            }
            if ctrl.stop.load(Ordering::Acquire) || !ctrl.active.load(Ordering::Acquire) {
                return Ok(None);
            }
            if let Some(err) = f.abort_reason.clone() {
                return Err(err);
            }
            let rem = deadline.saturating_duration_since(Instant::now());
            if rem.is_zero() {
                if f.latest_req == req {
                    f.in_flight = None;
                    f.pending_write = None;
                }
                return Err("timed out waiting for recorded frame".into());
            }
            let (next, _) = ctrl.frame_cvar.wait_timeout(f, rem).unwrap_or_else(|e| e.into_inner());
            f = next;
        }
    }

    fn frame_at_pinned(&self, at_ms: u64, cancel: &ExportCancel) -> Result<Option<(u64, Frame)>, String> {
        self.frame_at_oneshot(at_ms, cancel)
    }

    fn cancel_frame_at(&self) {
        if let Some(ctrl) = self.history.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            let mut f = ctrl.frame.lock().unwrap_or_else(|e| e.into_inner());
            f.latest_req = f.latest_req.wrapping_add(1);
            f.pending_write = None;
            f.in_flight = None;
            ctrl.frame_cvar.notify_all();
        }
    }

    fn export_video(&self, actions: Option<&[usize]>, zoom: usize, out: &Path, progress: &mut dyn FnMut(f32) -> bool) -> Result<export::Made, String> {
        let cancel = ExportCancel::new();
        self.export_video_cancellable(actions, zoom, out, &cancel, progress)
    }

    fn export_video_cancellable(&self, actions: Option<&[usize]>, zoom: usize, out: &Path, cancel: &ExportCancel, progress: &mut dyn FnMut(f32) -> bool) -> Result<export::Made, String> {
        screens::require_valid_name(&self.id.name)?;
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        let host_caps = self.probe_capabilities();
        let mut args = Vec::new();
        if let Some(which) = actions {
            if which.is_empty() {
                return Err("nothing has been done on this screen".into());
            }
            if export::has_gaps(which) && !host_caps.discontiguous_part {
                return Err("This host only supports exporting a contiguous range of actions. Select adjacent actions or update Olo on the host to export a filtered subset.".into());
            }
            let spec = format_part_spec(which);
            let range_count = spec.split(',').count();
            if range_count > MAX_EXPORT_PART_RANGES || spec.len() > MAX_EXPORT_PART_SPEC_LEN {
                return Err(format!("selection has too many separate action ranges ({range_count}; maximum is {MAX_EXPORT_PART_RANGES})"));
            }
            args.push("--part".to_owned());
            args.push(spec);
        }
        args.push("--zoom".to_owned());
        args.push(zoom.clamp(1, 3).to_string());

        let LiveStream { reader, writer, cancel: stream_cancel } = self.opener.open_export(&self.id.name, &args, cancel.flag())?;
        let transport: TeardownSlot = Arc::new(Mutex::new(Some((Some(writer), stream_cancel))));
        let transport_for_hook = transport.clone();
        cancel.register_hook(move || {
            if let Some((w, f)) = transport_for_hook.lock().unwrap_or_else(|e| e.into_inner()).take() {
                // Drop write half first so host sees stdin EOF and kills its
                // export process group cleanly, then fire transport cancel hook.
                drop(w);
                if let Some(f) = f {
                    f();
                }
            }
        });
        struct TransportGuard(TeardownSlot);
        impl Drop for TransportGuard {
            fn drop(&mut self) {
                if let Some((w, f)) = self.0.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    drop(w);
                    if let Some(f) = f {
                        f();
                    }
                }
            }
        }
        let _transport_guard = TransportGuard(transport);

        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }

        let mut buf_reader = BufReader::new(reader);
        let mut expected_meta: Option<ExportMeta> = None;
        let mut part: Option<(ExclusivePartFile, BufWriter<std::fs::File>)> = None;
        let mut written: u64 = 0;

        loop {
            if cancel.is_cancelled() {
                return Err("cancelled".into());
            }
            match read_export_msg_bounded(&mut buf_reader) {
                Ok(Some(ExportMsg::Progress { done })) => {
                    if cancel.is_cancelled() || !progress(done.clamp(0.0, 1.0)) {
                        cancel.cancel();
                        return Err("cancelled".into());
                    }
                }
                Ok(Some(ExportMsg::Meta(meta))) => {
                    if cancel.is_cancelled() {
                        return Err("cancelled".into());
                    }
                    if expected_meta.is_some() {
                        return Err("duplicate export metadata from host".into());
                    }
                    if meta.bytes > self.max_export_bytes {
                        cancel.cancel();
                        return Err(format!("export size ({} MiB) exceeds desktop limit ({} MiB)", meta.bytes.div_ceil(1 << 20), self.max_export_bytes / (1 << 20)));
                    }
                    let parent = out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
                    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
                    check_volume_space(parent, meta.bytes, self.export_free_reserve)?;
                    let mut exclusive = ExclusivePartFile::create_beside(out)?;
                    let writer_file = exclusive.take_file().ok_or_else(|| "missing temporary export file".to_owned())?;
                    part = Some((exclusive, BufWriter::new(writer_file)));
                    expected_meta = Some(meta);
                }
                Ok(Some(ExportMsg::Chunk(bytes))) => {
                    if cancel.is_cancelled() {
                        return Err("cancelled".into());
                    }
                    let Some(meta) = expected_meta.as_ref() else {
                        return Err("export chunk arrived before metadata".into());
                    };
                    let next = written.checked_add(bytes.len() as u64).ok_or_else(|| "export byte count overflow".to_owned())?;
                    if next > meta.bytes || next > self.max_export_bytes {
                        cancel.cancel();
                        return Err(format!("export payload ({next} bytes) exceeds declared {} bytes", meta.bytes));
                    }
                    if let Some((excl, w)) = part.as_mut() {
                        w.write_all(&bytes).map_err(|e| format!("{}: {e}", excl.path().display()))?;
                    }
                    written = next;
                }
                Ok(Some(ExportMsg::Done)) => {
                    if cancel.is_cancelled() {
                        return Err("cancelled".into());
                    }
                    let Some(meta) = expected_meta.take() else {
                        return Err("export finished without metadata".into());
                    };
                    if written != meta.bytes {
                        return Err(format!("export truncated: received {written} of {} bytes", meta.bytes));
                    }
                    let Some((exclusive, mut bw)) = part.take() else {
                        return Err("export missing temporary file".into());
                    };
                    bw.flush().map_err(|e| format!("{}: {e}", exclusive.path().display()))?;
                    drop(bw);
                    if cancel.is_cancelled() {
                        return Err("cancelled".into());
                    }
                    exclusive.publish(out)?;
                    return Ok(export::Made { frames: meta.frames, encoded: meta.encoded, encoder: meta.encoder, size: (meta.size.0 as usize, meta.size.1 as usize) });
                }
                Ok(Some(ExportMsg::Error { message })) => {
                    return Err(message);
                }
                Ok(None) => {
                    return if cancel.is_cancelled() { Err("cancelled".into()) } else { Err("export stream closed before finishing".into()) };
                }
                Err(e) => {
                    return if cancel.is_cancelled() { Err("cancelled".into()) } else { Err(format!("export stream error: {e}")) };
                }
            }
        }
    }

    fn session_started(&self) -> Option<u64> {
        if let Some(started) = *self.expected_started.lock().unwrap_or_else(|e| e.into_inner()) {
            return Some(started);
        }
        self.info.lock().unwrap_or_else(|e| e.into_inner()).started_ms.map(|ms| ms / 1000)
    }

    fn start(&self) -> Result<ScreenInfo, String> {
        let out = self.run_checked(&["start".to_owned(), self.id.name.clone(), "--keep".to_owned()])?;
        let mut info = self.info.lock().unwrap_or_else(|e| e.into_inner());
        info.state = ScreenState::Running;
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(out.stdout.trim()) {
            if let (Some(w), Some(h)) = (v["width"].as_i64(), v["height"].as_i64()) {
                info.size = Some((w as i32, h as i32));
            }
            if let Some(scale) = v["scale"].as_f64() {
                info.scale = scale;
            }
            if let Some(sound) = v["sound"].as_bool() {
                info.sound = sound;
            }
            if let Some(started) = v["started"].as_u64() {
                info.started_ms = Some(started.saturating_mul(1000));
                *self.expected_started.lock().unwrap_or_else(|e| e.into_inner()) = Some(started);
            }
        }
        Ok(info.clone())
    }

    fn stop(&self) -> Result<(), String> {
        let expected = self.session_started();
        self.stop_if_generation(expected)
    }

    fn stop_if_generation(&self, expected_started: Option<u64>) -> Result<(), String> {
        let host_caps = self.probe_capabilities();
        let mut args = vec!["stop".to_owned(), self.id.name.clone()];
        if host_caps.stop_expected_started
            && let Some(started) = expected_started
        {
            args.push("--expected-started".to_owned());
            args.push(started.to_string());
        }
        self.run_checked(&args)?;
        *self.expected_started.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let mut info = self.info.lock().unwrap_or_else(|e| e.into_inner());
        info.state = ScreenState::Stopped;
        info.controlled = false;
        Ok(())
    }

    fn clear_history(&self) -> Result<(), String> {
        self.run_checked(&["history".to_owned(), self.id.name.clone(), "--clear".to_owned()])?;
        self.info.lock().unwrap_or_else(|e| e.into_inner()).last_used_ms = None;
        Ok(())
    }

    fn rename(&self, title: Option<&str>) -> Result<(), String> {
        let clean = title.map(str::trim).filter(|t| !t.is_empty());
        let mut args = vec!["rename".to_owned(), self.id.name.clone()];
        if let Some(t) = clean {
            args.push(t.to_owned());
        }
        self.run_checked(&args)?;
        self.info.lock().unwrap_or_else(|e| e.into_inner()).title = clean.map(str::to_owned);
        Ok(())
    }

    fn archive(&self, on: bool) -> Result<(), String> {
        let sub = if on { "archive" } else { "unarchive" };
        self.run_checked(&[sub.to_owned(), self.id.name.clone()])?;
        self.info.lock().unwrap_or_else(|e| e.into_inner()).state = if on { ScreenState::Archived } else { ScreenState::Stopped };
        Ok(())
    }

    fn delete(&self) -> Result<(), String> {
        self.run_checked(&["delete".to_owned(), self.id.name.clone()])?;
        let mut info = self.info.lock().unwrap_or_else(|e| e.into_inner());
        info.state = ScreenState::Stopped;
        info.controlled = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::BufRead as _,
        net::Shutdown,
        os::unix::net::UnixStream,
        path::PathBuf,
        sync::mpsc::{Receiver, channel},
    };

    use olo_rex_link::screen_wire::{HistoryFrameMark, HistoryMsg, MSG_FRAME, ServerMsg, StreamStatus, read_client_msg, read_history_request, write_export_msg, write_history_msg, write_server_msg};
    use olo_screen_codec::{TileEncoding, TileRect, tiles::encode_delta};

    use super::*;
    use crate::source::{PaneCapabilities, PasswordsMode};

    fn wait_threads_zero(counter: &AtomicUsize, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while counter.load(Ordering::Acquire) != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(counter.load(Ordering::Acquire), 0, "worker threads did not exit within {timeout:?}");
    }

    fn remote_info(name: &str) -> ScreenInfo {
        ScreenInfo {
            id: ScreenId::host("dev-box", "Dev Box", name),
            title: None,
            state: ScreenState::Running,
            size: Some((128, 64)),
            scale: 1.0,
            sound: false,
            started_ms: None,
            last_used_ms: None,
            controlled: false,
            capabilities: PaneCapabilities::remote_with_control(),
        }
    }

    fn solid_frame(w: usize, h: usize, rgba: [u8; 4]) -> Frame {
        let mut buf = Vec::with_capacity(w * h * 4);
        for _ in 0..(w * h) {
            buf.extend_from_slice(&rgba);
        }
        Frame { width: w, height: h, rgba: buf }
    }

    fn paint_rect(frame: &mut Frame, rx: usize, ry: usize, rw: usize, rh: usize, rgba: [u8; 4]) {
        for y in ry..(ry + rh).min(frame.height) {
            for x in rx..(rx + rw).min(frame.width) {
                let i = (y * frame.width + x) * 4;
                frame.rgba[i..i + 4].copy_from_slice(&rgba);
            }
        }
    }

    fn action_line(at: u64, ms: u64, kind: &str, args: &[&str]) -> String {
        serde_json::to_string(&Action { at, ms, kind: kind.into(), args: args.iter().map(|s| (*s).to_owned()).collect(), result: None, error: None }).unwrap()
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        let d = PathBuf::from("/var/tmp").join(format!("olo-screen-pane-stream-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Stand-in opener returning paired Unix sockets for live, history, and
    /// export channels, plus scripted one-shot command outputs.
    struct MockOpener {
        opens: AtomicUsize,
        history_opens: AtomicUsize,
        export_opens: AtomicUsize,
        plan: Mutex<VecDeque<Result<UnixStream, String>>>,
        history_plan: Mutex<VecDeque<Result<UnixStream, String>>>,
        export_plan: Mutex<VecDeque<Result<UnixStream, String>>>,
        export_args: Mutex<Vec<(String, Vec<String>)>>,
        cmd_plan: Mutex<VecDeque<Result<CommandOutput, String>>>,
        cmd_calls: Mutex<Vec<Vec<String>>>,
    }

    impl MockOpener {
        fn empty() -> MockOpener {
            MockOpener {
                opens: AtomicUsize::new(0),
                history_opens: AtomicUsize::new(0),
                export_opens: AtomicUsize::new(0),
                plan: Mutex::new(VecDeque::new()),
                history_plan: Mutex::new(VecDeque::new()),
                export_plan: Mutex::new(VecDeque::new()),
                export_args: Mutex::new(Vec::new()),
                cmd_plan: Mutex::new(VecDeque::new()),
                cmd_calls: Mutex::new(Vec::new()),
            }
        }

        fn with_hosts(count: usize) -> (Arc<MockOpener>, Vec<UnixStream>) {
            let m = Self::empty();
            let mut hosts = Vec::new();
            for _ in 0..count {
                let (c, h) = UnixStream::pair().expect("socketpair");
                let _ = h.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = h.set_write_timeout(Some(Duration::from_secs(5)));
                m.plan.lock().unwrap().push_back(Ok(c));
                hosts.push(h);
            }
            (Arc::new(m), hosts)
        }

        fn with_history_hosts(count: usize) -> (Arc<MockOpener>, Vec<UnixStream>) {
            let m = Self::empty();
            let mut hosts = Vec::new();
            for _ in 0..count {
                let (c, h) = UnixStream::pair().expect("socketpair");
                let _ = h.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = h.set_write_timeout(Some(Duration::from_secs(5)));
                m.history_plan.lock().unwrap().push_back(Ok(c));
                hosts.push(h);
            }
            (Arc::new(m), hosts)
        }

        fn with_export_hosts(count: usize) -> (Arc<MockOpener>, Vec<UnixStream>) {
            let m = Self::empty();
            let mut hosts = Vec::new();
            for _ in 0..count {
                let (c, h) = UnixStream::pair().expect("socketpair");
                let _ = h.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = h.set_write_timeout(Some(Duration::from_secs(5)));
                m.export_plan.lock().unwrap().push_back(Ok(c));
                hosts.push(h);
            }
            (Arc::new(m), hosts)
        }

        fn with_commands(outputs: Vec<Result<CommandOutput, String>>) -> Arc<MockOpener> {
            let m = Self::empty();
            m.cmd_plan.lock().unwrap().extend(outputs);
            Arc::new(m)
        }

        fn always_err(msg: &str) -> Arc<MockOpener> {
            let m = Self::empty();
            for _ in 0..16 {
                m.plan.lock().unwrap().push_back(Err(msg.to_owned()));
                m.history_plan.lock().unwrap().push_back(Err(msg.to_owned()));
            }
            Arc::new(m)
        }

        fn open_count(&self) -> usize {
            self.opens.load(Ordering::Acquire)
        }

        fn history_open_count(&self) -> usize {
            self.history_opens.load(Ordering::Acquire)
        }

        fn open_sock(sock: UnixStream) -> Result<LiveStream, String> {
            let _ = sock.set_read_timeout(Some(Duration::from_secs(5)));
            let _ = sock.set_write_timeout(Some(Duration::from_secs(5)));
            let reader = sock.try_clone().map_err(|e| e.to_string())?;
            let closer = sock.try_clone().map_err(|e| e.to_string())?;
            Ok(LiveStream::with_cancel(reader, sock, move || {
                let _ = closer.shutdown(Shutdown::Both);
            }))
        }
    }

    impl StreamOpener for MockOpener {
        fn open_live(&self, _name: &str, cancelled: Arc<AtomicBool>) -> Result<LiveStream, String> {
            if cancelled.load(Ordering::Acquire) {
                return Err("cancelled".into());
            }
            self.opens.fetch_add(1, Ordering::AcqRel);
            let sock = self.plan.lock().unwrap().pop_front().unwrap_or_else(|| Err("no more mock live streams".into()))?;
            Self::open_sock(sock)
        }

        fn open_history(&self, _name: &str, cancelled: Arc<AtomicBool>) -> Result<LiveStream, String> {
            if cancelled.load(Ordering::Acquire) {
                return Err("cancelled".into());
            }
            self.history_opens.fetch_add(1, Ordering::AcqRel);
            let sock = self.history_plan.lock().unwrap().pop_front().unwrap_or_else(|| Err("no more mock history streams".into()))?;
            Self::open_sock(sock)
        }

        fn open_export(&self, name: &str, args: &[String], cancelled: Arc<AtomicBool>) -> Result<LiveStream, String> {
            if cancelled.load(Ordering::Acquire) {
                return Err("cancelled".into());
            }
            self.export_opens.fetch_add(1, Ordering::AcqRel);
            self.export_args.lock().unwrap().push((name.to_owned(), args.to_vec()));
            let sock = self.export_plan.lock().unwrap().pop_front().unwrap_or_else(|| Err("no more mock export streams".into()))?;
            Self::open_sock(sock)
        }

        fn run_command(&self, args: &[String], cancelled: Arc<AtomicBool>) -> Result<CommandOutput, String> {
            if cancelled.load(Ordering::Acquire) {
                return Err("cancelled".into());
            }
            self.cmd_calls.lock().unwrap().push(args.to_vec());
            self.cmd_plan.lock().unwrap().pop_front().unwrap_or_else(|| Err("no more mock commands".into()))
        }
    }

    fn recv_within<T>(rx: &Receiver<T>, label: &str) -> T {
        rx.recv_timeout(Duration::from_secs(5)).unwrap_or_else(|e| panic!("timed out waiting for {label}: {e}"))
    }

    // ---- Step 3b live tests ------------------------------------------------

    #[test]
    fn keyframe_then_deltas_rebuild_picture_and_ack_each_frame() {
        let (opener, mut hosts) = MockOpener::with_hosts(1);
        let mut host = hosts.remove(0);
        let src = StreamScreenSource::new(remote_info("qa"), opener);
        let (ev_tx, ev_rx) = channel();
        let handle = src.watch_live(
            Vec::new(),
            Box::new(move |ev| {
                let _ = ev_tx.send(ev);
            }),
        );

        let f1 = solid_frame(128, 64, [20, 40, 60, 255]);
        let d1 = encode_delta(&f1, None, TileEncoding::PackedStream, 1).unwrap();
        write_server_msg(&mut host, &ServerMsg::Frame { seq: 1, delta: d1.encode_bytes() }).unwrap();

        match recv_within(&ev_rx, "frame 1") {
            LiveEvent::Rgba(shot) => {
                assert_eq!(shot.size, [128, 64]);
                assert_eq!(shot.rgba, f1.rgba);
            }
            other => panic!("expected Rgba for frame 1, got {:?}", std::mem::discriminant(&other)),
        }
        assert_eq!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Ack { seq: 1 }));

        // Delta updating a single tile on the right half.
        let mut f2 = f1.clone();
        paint_rect(&mut f2, 70, 10, 24, 16, [220, 80, 30, 255]);
        let d2 = encode_delta(&f2, Some(&f1), TileEncoding::PackedStream, 1).unwrap();
        assert!(!d2.keyframe);
        assert_eq!(d2.rects.len(), 1);
        write_server_msg(&mut host, &ServerMsg::Frame { seq: 2, delta: d2.encode_bytes() }).unwrap();

        match recv_within(&ev_rx, "frame 2") {
            LiveEvent::Rgba(shot) => {
                assert_eq!(shot.size, [128, 64]);
                assert_eq!(shot.rgba, f2.rgba);
            }
            other => panic!("expected Rgba for frame 2, got {:?}", std::mem::discriminant(&other)),
        }
        assert_eq!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Ack { seq: 2 }));

        // Resize keyframe.
        let f3 = solid_frame(192, 96, [90, 180, 240, 255]);
        let d3 = encode_delta(&f3, Some(&f2), TileEncoding::PackedStream, 1).unwrap();
        assert!(d3.keyframe);
        write_server_msg(&mut host, &ServerMsg::Frame { seq: 3, delta: d3.encode_bytes() }).unwrap();

        match recv_within(&ev_rx, "frame 3") {
            LiveEvent::Rgba(shot) => {
                assert_eq!(shot.size, [192, 96]);
                assert_eq!(shot.rgba, f3.rgba);
            }
            other => panic!("expected Rgba for frame 3, got {:?}", std::mem::discriminant(&other)),
        }
        assert_eq!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Ack { seq: 3 }));

        drop(handle);
    }

    #[test]
    fn three_frames_while_busy_show_only_the_last() {
        let (opener, mut hosts) = MockOpener::with_hosts(1);
        let mut host = hosts.remove(0);
        let src = StreamScreenSource::new(remote_info("qa"), opener);

        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let gate_cb = gate.clone();
        let (entered_tx, entered_rx) = channel();
        let (shots_tx, shots_rx) = channel();
        let seen_first = AtomicBool::new(false);

        let handle = src.watch_live(
            Vec::new(),
            Box::new(move |ev| {
                if let LiveEvent::Rgba(shot) = ev {
                    let _ = shots_tx.send(shot);
                    if !seen_first.swap(true, Ordering::AcqRel) {
                        let _ = entered_tx.send(());
                        let mut open = gate_cb.0.lock().unwrap();
                        while !*open {
                            open = gate_cb.1.wait(open).unwrap();
                        }
                    }
                }
            }),
        );

        let f1 = solid_frame(64, 64, [10, 10, 10, 255]);
        write_server_msg(&mut host, &ServerMsg::Frame { seq: 1, delta: encode_delta(&f1, None, TileEncoding::PackedStream, 1).unwrap().encode_bytes() }).unwrap();
        assert_eq!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Ack { seq: 1 }));
        recv_within(&entered_rx, "callback entered on frame 1");

        let f2 = solid_frame(64, 64, [20, 20, 20, 255]);
        let f3 = solid_frame(64, 64, [30, 30, 30, 255]);
        let f4 = solid_frame(64, 64, [40, 40, 40, 255]);
        for (seq, prev, cur) in [(2, &f1, &f2), (3, &f2, &f3), (4, &f3, &f4)] {
            let d = encode_delta(cur, Some(prev), TileEncoding::PackedStream, 1).unwrap();
            write_server_msg(&mut host, &ServerMsg::Frame { seq, delta: d.encode_bytes() }).unwrap();
            assert_eq!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Ack { seq }));
        }

        {
            *gate.0.lock().unwrap() = true;
            gate.1.notify_all();
        }
        let s1 = recv_within(&shots_rx, "first shot");
        assert_eq!(s1.rgba, f1.rgba);
        let s_last = recv_within(&shots_rx, "latest shot");
        assert_eq!(s_last.rgba, f4.rgba);
        assert!(shots_rx.recv_timeout(Duration::from_millis(80)).is_err());

        drop(handle);
    }

    #[test]
    fn pause_sent_when_hidden_and_resumed_when_shown() {
        let (opener, mut hosts) = MockOpener::with_hosts(1);
        let mut host = hosts.remove(0);
        let src = StreamScreenSource::new(remote_info("qa"), opener);
        let mut handle = src.watch_live(Vec::new(), Box::new(|_| {}));

        let f1 = solid_frame(64, 64, [5, 5, 5, 255]);
        write_server_msg(&mut host, &ServerMsg::Frame { seq: 1, delta: encode_delta(&f1, None, TileEncoding::PackedStream, 1).unwrap().encode_bytes() }).unwrap();
        assert_eq!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Ack { seq: 1 }));

        handle.set_paused(true);
        handle.set_paused(true);
        assert_eq!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Pause { paused: true }));

        handle.set_paused(false);
        assert_eq!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Pause { paused: false }));

        drop(handle);
    }

    #[test]
    fn input_gated_before_control_coalesces_moves_and_stops_on_refusal() {
        let (opener, mut hosts) = MockOpener::with_hosts(1);
        let mut host = hosts.remove(0);
        let src = StreamScreenSource::new(remote_info("qa"), opener);
        assert!(src.join_clipboard().is_none());

        let (ev_tx, ev_rx) = channel();
        let mut handle = src.watch_live(
            Vec::new(),
            Box::new(move |ev| {
                let _ = ev_tx.send(ev);
            }),
        );

        let f1 = solid_frame(64, 64, [1, 2, 3, 255]);
        write_server_msg(&mut host, &ServerMsg::Frame { seq: 1, delta: encode_delta(&f1, None, TileEncoding::PackedStream, 1).unwrap().encode_bytes() }).unwrap();
        assert_eq!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Ack { seq: 1 }));
        let _ = recv_within(&ev_rx, "frame 1");

        src.send_input(vec![SyntheticEvent::Key { key: 30, pressed: true }]);
        handle.set_paused(true);
        assert_eq!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Pause { paused: true }));
        handle.set_paused(false);
        assert_eq!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Pause { paused: false }));

        src.set_controlled(true).unwrap();
        let req1 = match read_client_msg(&mut host).unwrap() {
            Some(ClientMsg::ControlSeq { on: true, seq }) => seq,
            other => panic!("expected ControlSeq(true), got {other:?}"),
        };
        assert!(req1 > 0);
        write_server_msg(&mut host, &ServerMsg::Control { held: false, seq: req1, reason: Some("Someone else is controlling this screen.".into()) }).unwrap();

        match recv_within(&ev_rx, "control refusal") {
            LiveEvent::Control { held, reason } => {
                assert!(!held);
                assert_eq!(reason.as_deref(), Some("Someone else is controlling this screen."));
            }
            other => panic!("expected LiveEvent::Control refusal, got {:?}", std::mem::discriminant(&other)),
        }
        src.send_input(vec![SyntheticEvent::Key { key: 30, pressed: true }]);
        handle.set_paused(true);
        assert_eq!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Pause { paused: true }));
        handle.set_paused(false);
        assert_eq!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Pause { paused: false }));

        src.set_controlled(true).unwrap();
        let req2 = match read_client_msg(&mut host).unwrap() {
            Some(ClientMsg::ControlSeq { on: true, seq }) => seq,
            other => panic!("expected ControlSeq(true) #2, got {other:?}"),
        };
        assert!(req2 > req1);
        // Explicit host grant for exact `req2` transitions Requested -> Held and notifies UI.
        write_server_msg(&mut host, &ServerMsg::Control { held: true, seq: req2, reason: None }).unwrap();
        match recv_within(&ev_rx, "control grant") {
            LiveEvent::Control { held, reason } => {
                assert!(held);
                assert!(reason.is_none());
            }
            other => panic!("expected LiveEvent::Control grant, got {:?}", std::mem::discriminant(&other)),
        }
        src.send_input(vec![
            SyntheticEvent::Move { x: 1.0, y: 1.0 },
            SyntheticEvent::Move { x: 5.0, y: 6.0 },
            SyntheticEvent::Button { button: 0x110, pressed: true },
            SyntheticEvent::Move { x: 8.0, y: 9.0 },
            SyntheticEvent::Move { x: 10.0, y: 12.0 },
            SyntheticEvent::Wheel { dx: 0, dy: -1 },
        ]);
        match read_client_msg(&mut host).unwrap() {
            Some(ClientMsg::Input { json_events }) => {
                let parsed: Vec<SyntheticEvent> = serde_json::from_str(&json_events).unwrap();
                assert_eq!(
                    parsed,
                    vec![
                        SyntheticEvent::Move { x: 5.0, y: 6.0 },
                        SyntheticEvent::Button { button: 0x110, pressed: true },
                        SyntheticEvent::Move { x: 10.0, y: 12.0 },
                        SyntheticEvent::Wheel { dx: 0, dy: -1 },
                    ]
                );
            }
            other => panic!("expected ClientMsg::Input, got {other:?}"),
        }

        src.set_controlled(false).unwrap();
        assert!(matches!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::ControlSeq { on: false, seq }) if seq > req2));
        src.send_input(vec![SyntheticEvent::Key { key: 31, pressed: true }]);
        handle.set_paused(true);
        assert_eq!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Pause { paused: true }));

        drop(handle);
    }

    #[test]
    fn dropped_mid_frame_reconnects_and_awaits_keyframe() {
        let (opener, mut hosts) = MockOpener::with_hosts(2);
        let mut host1 = hosts.remove(0);
        let mut host2 = hosts.remove(0);
        let src = StreamScreenSource::with_reconnect(remote_info("qa"), opener.clone(), ReconnectPolicy { delays: vec![Duration::from_millis(15), Duration::from_millis(30)] });
        let (ev_tx, ev_rx) = channel();
        let handle = src.watch_live(
            Vec::new(),
            Box::new(move |ev| {
                let _ = ev_tx.send(ev);
            }),
        );

        let f1 = solid_frame(64, 64, [11, 22, 33, 255]);
        write_server_msg(&mut host1, &ServerMsg::Frame { seq: 1, delta: encode_delta(&f1, None, TileEncoding::PackedStream, 1).unwrap().encode_bytes() }).unwrap();
        assert_eq!(read_client_msg(&mut host1).unwrap(), Some(ClientMsg::Ack { seq: 1 }));
        assert!(matches!(recv_within(&ev_rx, "frame 1"), LiveEvent::Rgba(_)));

        host1.write_all(&[MSG_FRAME, 0x00, 0x04, 0x00, 0x00, 1, 2, 3]).unwrap();
        drop(host1);

        assert!(matches!(recv_within(&ev_rx, "reconnecting"), LiveEvent::Status(LiveStatus::Reconnecting)));

        let mut f_partial = f1.clone();
        paint_rect(&mut f_partial, 0, 0, 16, 16, [200, 0, 0, 255]);
        let delta_only = encode_delta(&f_partial, Some(&f1), TileEncoding::PackedStream, 1).unwrap();
        assert!(!delta_only.keyframe);
        write_server_msg(&mut host2, &ServerMsg::Frame { seq: 1, delta: delta_only.encode_bytes() }).unwrap();
        assert_eq!(read_client_msg(&mut host2).unwrap(), Some(ClientMsg::Ack { seq: 1 }));
        assert!(ev_rx.recv_timeout(Duration::from_millis(60)).is_err(), "non-keyframe after reconnect must not produce a picture");

        let f2 = solid_frame(64, 64, [44, 55, 66, 255]);
        write_server_msg(&mut host2, &ServerMsg::Frame { seq: 2, delta: encode_delta(&f2, None, TileEncoding::PackedStream, 1).unwrap().encode_bytes() }).unwrap();
        assert_eq!(read_client_msg(&mut host2).unwrap(), Some(ClientMsg::Ack { seq: 2 }));
        match recv_within(&ev_rx, "reconnected keyframe") {
            LiveEvent::Rgba(shot) => assert_eq!(shot.rgba, f2.rgba),
            other => panic!("expected Rgba after keyframe, got {:?}", std::mem::discriminant(&other)),
        }
        assert_eq!(opener.open_count(), 2);

        drop(handle);
    }

    #[test]
    fn backoff_gives_up_cleanly_and_screen_gone_ends_without_retry() {
        let opener = MockOpener::always_err("host is offline");
        let src =
            StreamScreenSource::with_reconnect(remote_info("qa"), opener.clone(), ReconnectPolicy { delays: vec![Duration::from_millis(15), Duration::from_millis(25), Duration::from_millis(40)] });
        let (ev_tx, ev_rx) = channel();
        let t0 = Instant::now();
        let handle = src.watch_live(
            Vec::new(),
            Box::new(move |ev| {
                let _ = ev_tx.send(ev);
            }),
        );

        let mut reconnecting = 0;
        let disconnected = loop {
            match recv_within(&ev_rx, "backoff event") {
                LiveEvent::Status(LiveStatus::Reconnecting) => reconnecting += 1,
                LiveEvent::Status(LiveStatus::Disconnected(msg)) => break msg,
                _ => {}
            }
        };
        assert_eq!(reconnecting, 3);
        assert_eq!(opener.open_count(), 4);
        assert_eq!(disconnected, "host is offline");
        assert!(t0.elapsed() >= Duration::from_millis(70), "expected backoff pacing");
        drop(handle);

        let (opener2, mut hosts) = MockOpener::with_hosts(1);
        let mut host = hosts.remove(0);
        let src2 = StreamScreenSource::new(remote_info("qa"), opener2.clone());
        let (ev_tx2, ev_rx2) = channel();
        let handle2 = src2.watch_live(
            Vec::new(),
            Box::new(move |ev| {
                let _ = ev_tx2.send(ev);
            }),
        );
        write_server_msg(&mut host, &ServerMsg::Status(StreamStatus { message: "Waiting for screen".into(), reconnecting: false, error: false })).unwrap();
        assert!(matches!(recv_within(&ev_rx2, "status"), LiveEvent::Status(LiveStatus::Message(ref m)) if m == "Waiting for screen"));
        write_server_msg(&mut host, &ServerMsg::ScreenGone { reason: "screen qa stopped".into() }).unwrap();
        assert!(matches!(recv_within(&ev_rx2, "screen gone"), LiveEvent::Status(LiveStatus::Disconnected(ref m)) if m == "screen qa stopped"));
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(opener2.open_count(), 1);
        assert_eq!(src2.info().state, ScreenState::Stopped);
        drop(handle2);
    }

    #[test]
    fn drop_closes_both_halves_and_joins_all_threads() {
        let (opener, mut hosts) = MockOpener::with_hosts(1);
        let mut host = hosts.remove(0);
        let src = StreamScreenSource::new(remote_info("qa"), opener);
        let (ev_tx, ev_rx) = channel();
        let handle = src.watch_live(
            Vec::new(),
            Box::new(move |ev| {
                let _ = ev_tx.send(ev);
            }),
        );

        let f1 = solid_frame(64, 64, [9, 9, 9, 255]);
        write_server_msg(&mut host, &ServerMsg::Frame { seq: 1, delta: encode_delta(&f1, None, TileEncoding::PackedStream, 1).unwrap().encode_bytes() }).unwrap();
        assert_eq!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Ack { seq: 1 }));
        let _ = recv_within(&ev_rx, "frame 1");

        let ctrl = src.active.lock().unwrap().clone().expect("active stream");
        let sid = ctrl.stream_id;
        let os_threads = |id: usize| -> usize {
            let want = [format!("stream-s{id}"), format!("stream-w{id}"), format!("stream-d{id}")];
            std::fs::read_dir("/proc/self/task")
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| std::fs::read_to_string(e.path().join("comm")).ok().is_some_and(|c| want.iter().any(|w| w == c.trim())))
                .count()
        };
        assert_eq!(ctrl.live_threads.load(Ordering::Acquire), 3);
        assert_eq!(os_threads(sid), 3);

        drop(handle);
        wait_threads_zero(&ctrl.live_threads, Duration::from_secs(2));
        let deadline = Instant::now() + Duration::from_secs(2);
        while os_threads(sid) != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(os_threads(sid), 0, "no OS threads may remain after dropping LiveHandle");
        assert!(src.active.lock().unwrap().is_none());
        assert_eq!(read_client_msg(&mut host).unwrap(), None, "write half must be closed on drop");
        let mut wrote_after = Ok(());
        for _ in 0..8 {
            wrote_after = host.write_all(&[0u8; 4096]);
            if wrote_after.is_err() {
                break;
            }
        }
        assert!(wrote_after.is_err(), "read half must be closed on drop");
    }

    #[test]
    fn lying_headers_bad_tiles_and_zstd_bomb_end_stream_with_words_never_panic() {
        {
            let (opener, mut hosts) = MockOpener::with_hosts(1);
            let mut host = hosts.remove(0);
            let src = StreamScreenSource::new(remote_info("qa"), opener.clone());
            let (ev_tx, ev_rx) = channel();
            let handle = src.watch_live(
                Vec::new(),
                Box::new(move |ev| {
                    let _ = ev_tx.send(ev);
                }),
            );
            host.write_all(&[MSG_FRAME, 0xFF, 0xFF, 0xFF, 0x7F]).unwrap();
            let err = match recv_within(&ev_rx, "oversized header failure") {
                LiveEvent::Failed(m) => m,
                LiveEvent::Status(LiveStatus::Disconnected(m)) => m,
                other => panic!("expected failure, got {:?}", std::mem::discriminant(&other)),
            };
            assert!(err.contains("exceeds") || err.contains("Stream closed"), "unexpected words: {err}");
            std::thread::sleep(Duration::from_millis(40));
            assert_eq!(opener.open_count(), 1, "protocol error must not reconnect");
            drop(handle);
        }

        {
            let (opener, mut hosts) = MockOpener::with_hosts(1);
            let mut host = hosts.remove(0);
            let src = StreamScreenSource::new(remote_info("qa"), opener.clone());
            let (ev_tx, ev_rx) = channel();
            let handle = src.watch_live(
                Vec::new(),
                Box::new(move |ev| {
                    let _ = ev_tx.send(ev);
                }),
            );
            let bad_olot =
                EncodedDelta { width: 64, height: 64, keyframe: true, encoding: TileEncoding::PackedStream, rects: vec![TileRect { x: 60, y: 0, width: 10, height: 10 }], data: vec![] }.encode_bytes();
            let mut payload = Vec::with_capacity(8 + bad_olot.len());
            payload.extend_from_slice(&1u64.to_le_bytes());
            payload.extend_from_slice(&bad_olot);
            let mut raw = vec![MSG_FRAME];
            raw.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            raw.extend_from_slice(&payload);
            host.write_all(&raw).unwrap();

            let err = match recv_within(&ev_rx, "bad tile failure") {
                LiveEvent::Failed(m) => m,
                LiveEvent::Status(LiveStatus::Disconnected(m)) => m,
                other => panic!("expected failure, got {:?}", std::mem::discriminant(&other)),
            };
            assert!(err.contains("exceeds") || err.contains("out of bounds") || err.contains("Stream closed"), "unexpected words: {err}");
            assert_eq!(opener.open_count(), 1);
            drop(handle);
        }

        {
            let (opener, mut hosts) = MockOpener::with_hosts(1);
            let mut host = hosts.remove(0);
            let src = StreamScreenSource::new(remote_info("qa"), opener.clone());
            let (ev_tx, ev_rx) = channel();
            let handle = src.watch_live(
                Vec::new(),
                Box::new(move |ev| {
                    let _ = ev_tx.send(ev);
                }),
            );
            let bomb = zstd::encode_all(&vec![0u8; 8 * 1024 * 1024][..], 3).unwrap();
            let bomb_olot =
                EncodedDelta { width: 64, height: 64, keyframe: true, encoding: TileEncoding::PackedStream, rects: vec![TileRect { x: 0, y: 0, width: 64, height: 64 }], data: bomb }.encode_bytes();
            write_server_msg(&mut host, &ServerMsg::Frame { seq: 1, delta: bomb_olot }).unwrap();

            let err = match recv_within(&ev_rx, "zstd bomb failure") {
                LiveEvent::Failed(m) => m,
                LiveEvent::Status(LiveStatus::Disconnected(m)) => m,
                other => panic!("expected failure, got {:?}", std::mem::discriminant(&other)),
            };
            assert!(err.contains("exceeds expected"), "unexpected words: {err}");
            assert_eq!(opener.open_count(), 1);
            drop(handle);
        }
    }

    // ---- Step 3c history, frame_at, export, and one-shot command tests -----

    #[test]
    fn history_actions_appended_in_pieces_reset_and_index_grown() {
        let (opener, mut hosts) = MockOpener::with_history_hosts(1);
        let mut host = hosts.remove(0);
        let src = StreamScreenSource::new(remote_info("qa"), opener);
        assert!(src.lazy_history());

        let (tx, rx) = channel();
        let handle = src.follow_history(Box::new(move |ev| {
            let _ = tx.send(ev);
        }));

        // Initial snapshot: 2 actions + 2 frame marks + SnapshotDone commits together.
        write_history_msg(&mut host, &HistoryMsg::Actions { reset: true, lines: vec![action_line(1_000, 20, "start", &[]), action_line(2_000, 15, "click", &["--name", "Save"])] }).unwrap();
        write_history_msg(
            &mut host,
            &HistoryMsg::Index { reset: true, last_size: Some((1440, 900)), frames: vec![HistoryFrameMark { at: 1_000, key: true }, HistoryFrameMark { at: 2_000, key: false }] },
        )
        .unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(40)).is_err(), "snapshot must wait for SnapshotDone");
        write_history_msg(&mut host, &HistoryMsg::SnapshotDone { actions_gen: 1, index_gen: 1 }).unwrap();
        match recv_within(&rx, "initial snapshot") {
            HistoryEvent::Loaded { actions, texts, reset, frames, .. } => {
                assert!(reset);
                assert_eq!(actions.len(), 2);
                assert_eq!(texts.len(), 2);
                assert!(texts[1].contains("Save"), "search text must be computed off UI thread: {:?}", texts[1]);
                assert_eq!(frames.len(), 2);
                assert_eq!(frames.last_size, Some((1440, 900)));
            }
            HistoryEvent::Failed(e) => panic!("unexpected failure: {e}"),
        }

        // Incremental action append in two separate pieces + incremental index growth.
        write_history_msg(&mut host, &HistoryMsg::Actions { reset: false, lines: vec![action_line(3_000, 10, "type", &["hello"])] }).unwrap();
        match recv_within(&rx, "append piece 1") {
            HistoryEvent::Loaded { actions, texts, reset, frames, .. } => {
                assert!(!reset);
                assert_eq!(actions.len(), 1);
                assert_eq!(actions[0].kind, "type");
                assert!(texts[0].contains("hello"));
                assert_eq!(frames.len(), 2);
            }
            HistoryEvent::Failed(e) => panic!("unexpected failure: {e}"),
        }

        write_history_msg(&mut host, &HistoryMsg::Actions { reset: false, lines: vec![action_line(4_000, 5, "key", &["Return"])] }).unwrap();
        match recv_within(&rx, "append piece 2") {
            HistoryEvent::Loaded { actions, reset, .. } => {
                assert!(!reset);
                assert_eq!(actions.len(), 1);
                assert_eq!(actions[0].kind, "key");
            }
            HistoryEvent::Failed(e) => panic!("unexpected failure: {e}"),
        }

        write_history_msg(
            &mut host,
            &HistoryMsg::Index { reset: false, last_size: Some((1440, 900)), frames: vec![HistoryFrameMark { at: 3_500, key: false }, HistoryFrameMark { at: 4_100, key: true }] },
        )
        .unwrap();
        match recv_within(&rx, "index grown") {
            HistoryEvent::Loaded { reset, frames, .. } => {
                assert!(!reset);
                assert_eq!(frames.len(), 4);
                assert_eq!(frames.last(), Some(4_100));
            }
            HistoryEvent::Failed(e) => panic!("unexpected failure: {e}"),
        }

        // Explicit reset while connected (e.g. clear history) commits on SnapshotDone.
        write_history_msg(&mut host, &HistoryMsg::Actions { reset: true, lines: vec![action_line(9_000, 12, "start", &[])] }).unwrap();
        write_history_msg(&mut host, &HistoryMsg::SnapshotDone { actions_gen: 2, index_gen: 1 }).unwrap();
        match recv_within(&rx, "reset actions") {
            HistoryEvent::Loaded { actions, reset, .. } => {
                assert!(reset);
                assert_eq!(actions.len(), 1);
                assert_eq!(actions[0].at, 9_000);
            }
            HistoryEvent::Failed(e) => panic!("unexpected failure: {e}"),
        }

        drop(handle);
    }

    #[test]
    fn three_frame_at_at_once_answered_only_for_the_last_and_miss_is_ok_none() {
        let (opener, mut hosts) = MockOpener::with_history_hosts(1);
        let mut host = hosts.remove(0);
        let src = Arc::new(StreamScreenSource::new(remote_info("qa"), opener));
        let (ev_tx, ev_rx) = channel();
        let handle = src.follow_history(Box::new(move |ev| {
            let _ = ev_tx.send(ev);
        }));

        // Establish connection with initial snapshot.
        write_history_msg(&mut host, &HistoryMsg::Actions { reset: true, lines: vec![] }).unwrap();
        write_history_msg(&mut host, &HistoryMsg::Index { reset: true, last_size: None, frames: vec![] }).unwrap();
        write_history_msg(&mut host, &HistoryMsg::SnapshotDone { actions_gen: 1, index_gen: 1 }).unwrap();
        let _ = recv_within(&ev_rx, "init snapshot");

        let mut host_reader = BufReader::new(host.try_clone().unwrap());

        // 1. Launch first `frame_at(1000)` and confirm it reached the host (host holds back reply).
        let s1 = src.clone();
        let t1 = std::thread::spawn(move || s1.frame_at(1_000));
        let req1 = match read_history_request(&mut host_reader).unwrap() {
            Some(HistoryRequest::FrameAt { req, at }) => {
                assert_eq!(at, 1_000);
                req
            }
            other => panic!("expected req1, got {other:?}"),
        };

        // 2. While req1 is unanswered, issue `frame_at(2000)` then `frame_at(3000)`.
        //    Wait for req2 to overtake req1, then req3 to overtake req2.
        let s2 = src.clone();
        let t2 = std::thread::spawn(move || s2.frame_at(2_000));
        // t1 must unblock immediately as Ok(None) without the host ever replying to req1!
        assert_eq!(t1.join().unwrap().unwrap(), None, "overtaken frame_at(1000) must not wait for host");

        let s3 = src.clone();
        let t3 = std::thread::spawn(move || s3.frame_at(3_000));
        // t2 must unblock immediately as Ok(None) without the host ever replying to req2!
        assert_eq!(t2.join().unwrap().unwrap(), None, "overtaken frame_at(2000) must not wait for host");

        // Drain requests on host until we see `at == 3_000` (skipping any overtaken 2_000).
        let req3 = loop {
            match read_history_request(&mut host_reader).unwrap() {
                Some(HistoryRequest::FrameAt { req, at: 3_000 }) => break req,
                Some(HistoryRequest::FrameAt { .. }) => continue,
                other => panic!("unexpected req: {other:?}"),
            }
        };
        assert!(req3 > req1);

        // Host answers ONLY req3 with a keyframe.
        let f3 = solid_frame(128, 64, [10, 120, 220, 255]);
        let d3 = encode_delta(&f3, None, TileEncoding::PackedStream, 1).unwrap();
        write_history_msg(&mut host, &HistoryMsg::Frame { req: req3, frame_at: 2_950, delta: d3.encode_bytes() }).unwrap();
        assert_eq!(t3.join().unwrap().unwrap(), Some((2_950, f3.clone())));

        // 3. Follow-up `frame_at(4000)` answered with a tile delta against `f3` -> bit-exact rebuild.
        let s4 = src.clone();
        let t4 = std::thread::spawn(move || s4.frame_at(4_000));
        let req4 = match read_history_request(&mut host_reader).unwrap() {
            Some(HistoryRequest::FrameAt { req, at }) => {
                assert_eq!(at, 4_000);
                req
            }
            other => panic!("expected req4, got {other:?}"),
        };
        let mut f4 = f3.clone();
        paint_rect(&mut f4, 64, 0, 32, 32, [240, 60, 40, 255]);
        let d4 = encode_delta(&f4, Some(&f3), TileEncoding::PackedStream, 1).unwrap();
        assert!(!d4.keyframe);
        write_history_msg(&mut host, &HistoryMsg::Frame { req: req4, frame_at: 3_980, delta: d4.encode_bytes() }).unwrap();
        assert_eq!(t4.join().unwrap().unwrap(), Some((3_980, f4)));

        // 4. A miss returns Ok(None), NOT Err.
        let s5 = src.clone();
        let t5 = std::thread::spawn(move || s5.frame_at(50));
        let req5 = match read_history_request(&mut host_reader).unwrap() {
            Some(HistoryRequest::FrameAt { req, .. }) => req,
            other => panic!("expected req5, got {other:?}"),
        };
        write_history_msg(&mut host, &HistoryMsg::Miss { req: req5 }).unwrap();
        assert_eq!(t5.join().unwrap(), Ok(None), "HistoryMsg::Miss must return Ok(None), not Err");

        // 5. `cancel_frame_at()` unblocks an already-blocked worker immediately.
        let s6 = src.clone();
        let t6 = std::thread::spawn(move || s6.frame_at(5_000));
        let _ = read_history_request(&mut host_reader).unwrap();
        src.cancel_frame_at();
        assert_eq!(t6.join().unwrap(), Ok(None), "cancel_frame_at must unblock waiting worker immediately");

        drop(handle);
    }

    #[test]
    fn history_stream_dropped_and_reopened_without_losing_or_doubling_actions() {
        let (opener, mut hosts) = MockOpener::with_history_hosts(2);
        let mut host1 = hosts.remove(0);
        let mut host2 = hosts.remove(0);
        let src = StreamScreenSource::with_reconnect(remote_info("qa"), opener.clone(), ReconnectPolicy { delays: vec![Duration::from_millis(15), Duration::from_millis(30)] });
        let (tx, rx) = channel();
        let handle = src.follow_history(Box::new(move |ev| {
            let _ = tx.send(ev);
        }));

        let l1 = action_line(1_000, 10, "start", &[]);
        let l2 = action_line(2_000, 20, "click", &["100", "100"]);
        let l3 = action_line(3_000, 30, "type", &["after drop"]);

        // Connection 1 sends [l1, l2] and index [1000, 2000] + SnapshotDone, then drops abruptly.
        write_history_msg(&mut host1, &HistoryMsg::Actions { reset: true, lines: vec![l1.clone(), l2.clone()] }).unwrap();
        write_history_msg(
            &mut host1,
            &HistoryMsg::Index { reset: true, last_size: Some((800, 600)), frames: vec![HistoryFrameMark { at: 1_000, key: true }, HistoryFrameMark { at: 2_000, key: false }] },
        )
        .unwrap();
        write_history_msg(&mut host1, &HistoryMsg::SnapshotDone { actions_gen: 1, index_gen: 1 }).unwrap();

        let mut collected = match recv_within(&rx, "conn1 snapshot") {
            HistoryEvent::Loaded { actions, reset, frames, .. } => {
                assert!(reset);
                assert_eq!(frames.len(), 2);
                actions
            }
            HistoryEvent::Failed(e) => panic!("unexpected failure: {e}"),
        };
        assert_eq!(collected.len(), 2);

        // Drop host1 cleanly; source reconnects on host2.
        drop(host1);

        // Host2 always sends its full snapshot on open: [l1, l2, l3] + index [1000, 2000, 3000] + SnapshotDone.
        write_history_msg(&mut host2, &HistoryMsg::Actions { reset: true, lines: vec![l1, l2, l3] }).unwrap();
        write_history_msg(
            &mut host2,
            &HistoryMsg::Index {
                reset: true,
                last_size: Some((800, 600)),
                frames: vec![HistoryFrameMark { at: 1_000, key: true }, HistoryFrameMark { at: 2_000, key: false }, HistoryFrameMark { at: 3_000, key: false }],
            },
        )
        .unwrap();
        write_history_msg(&mut host2, &HistoryMsg::SnapshotDone { actions_gen: 1, index_gen: 1 }).unwrap();

        // Source must emit ONLY the unseen tail [l3] with `reset = false` (no duplicates, no loss).
        match recv_within(&rx, "conn2 deduplicated tail") {
            HistoryEvent::Loaded { actions, reset, frames, .. } => {
                assert!(!reset, "reconnect with matching prefix must append tail without reset");
                assert_eq!(actions.len(), 1);
                assert_eq!(actions[0].kind, "type");
                assert_eq!(frames.len(), 3);
                collected.extend(actions);
            }
            HistoryEvent::Failed(e) => panic!("unexpected failure: {e}"),
        }
        assert_eq!(collected.iter().map(|a| a.at).collect::<Vec<_>>(), vec![1_000, 2_000, 3_000]);
        assert_eq!(opener.history_open_count(), 2);

        drop(handle);
    }

    #[test]
    fn history_set_active_closes_stream_and_reopens_cleanly() {
        let (opener, mut hosts) = MockOpener::with_history_hosts(2);
        let mut host1 = hosts.remove(0);
        let mut host2 = hosts.remove(0);
        let src = StreamScreenSource::new(remote_info("qa"), opener.clone());
        let (tx, rx) = channel();
        let mut handle = src.follow_history(Box::new(move |ev| {
            let _ = tx.send(ev);
        }));

        let l1 = action_line(1_000, 10, "start", &[]);
        write_history_msg(&mut host1, &HistoryMsg::Actions { reset: true, lines: vec![l1.clone()] }).unwrap();
        write_history_msg(&mut host1, &HistoryMsg::SnapshotDone { actions_gen: 1, index_gen: 1 }).unwrap();
        let _ = recv_within(&rx, "conn1 actions");

        // Deactivate (e.g. switched back to Live tab): host1 must see stdin EOF and close.
        handle.set_active(false);
        let mut r1 = BufReader::new(&mut host1);
        let mut buf = String::new();
        assert_eq!(r1.read_line(&mut buf).unwrap(), 0, "deactivating history must close write half");
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(opener.history_open_count(), 1, "inactive history must not reopen");

        // Re-activate (switched back to History tab): opens host2 and syncs only new action l2.
        handle.set_active(true);
        let l2 = action_line(2_000, 10, "click", &["10", "20"]);
        write_history_msg(&mut host2, &HistoryMsg::Actions { reset: true, lines: vec![l1, l2] }).unwrap();
        write_history_msg(&mut host2, &HistoryMsg::SnapshotDone { actions_gen: 1, index_gen: 1 }).unwrap();
        match recv_within(&rx, "reopened tail") {
            HistoryEvent::Loaded { actions, reset, .. } => {
                assert!(!reset);
                assert_eq!(actions.len(), 1);
                assert_eq!(actions[0].kind, "click");
            }
            HistoryEvent::Failed(e) => panic!("unexpected failure: {e}"),
        }
        assert_eq!(opener.history_open_count(), 2);

        let ctrl = src.history.lock().unwrap().clone().unwrap();
        drop(handle);
        wait_threads_zero(&ctrl.live_threads, Duration::from_secs(2));
    }

    #[test]
    fn export_streams_progress_writes_part_and_renames_atomically() {
        let dir = scratch_dir("export-ok");
        let out = dir.join("clip.mp4");
        let (opener, mut hosts) = MockOpener::with_export_hosts(1);
        let mut host = hosts.remove(0);
        let payload = b"fake-mp4-content-0123456789abcdef".to_vec();
        let payload_host = payload.clone();

        let host_thread = std::thread::spawn(move || {
            write_export_msg(&mut host, &ExportMsg::Progress { done: 0.25 }).unwrap();
            write_export_msg(&mut host, &ExportMsg::Progress { done: 0.75 }).unwrap();
            write_export_msg(&mut host, &ExportMsg::Meta(ExportMeta { bytes: payload_host.len() as u64, frames: 42, encoded: 12, encoder: "x264".into(), size: (1280, 720) })).unwrap();
            write_export_msg(&mut host, &ExportMsg::Chunk(payload_host[..10].to_vec())).unwrap();
            write_export_msg(&mut host, &ExportMsg::Chunk(payload_host[10..].to_vec())).unwrap();
            write_export_msg(&mut host, &ExportMsg::Done).unwrap();
        });

        let src = StreamScreenSource::new(remote_info("qa"), opener.clone());
        let mut seen = Vec::new();
        let made = src
            .export_video(Some(&[2, 3, 4, 8]), 2, &out, &mut |p| {
                seen.push(p);
                true
            })
            .unwrap();
        host_thread.join().unwrap();

        assert_eq!(seen, vec![0.25, 0.75]);
        assert_eq!(made, export::Made { frames: 42, encoded: 12, encoder: "x264".into(), size: (1280, 720) });
        assert_eq!(std::fs::read(&out).unwrap(), payload);
        let parts: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".part")).collect();
        assert!(parts.is_empty(), "no .part file may linger after atomic rename: {parts:?}");
        assert_eq!(opener.export_args.lock().unwrap().as_slice(), &[("qa".to_owned(), vec!["--part".to_owned(), "2-4,8".to_owned(), "--zoom".to_owned(), "2".to_owned()])]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_cancelled_part_way_tells_host_and_leaves_no_file() {
        let dir = scratch_dir("export-cancel");
        let out = dir.join("cancelled.mp4");
        let (opener, mut hosts) = MockOpener::with_export_hosts(1);
        let mut host = hosts.remove(0);

        let host_saw_eof = Arc::new(AtomicBool::new(false));
        let host_saw_eof_w = host_saw_eof.clone();
        let host_thread = std::thread::spawn(move || {
            write_export_msg(&mut host, &ExportMsg::Meta(ExportMeta { bytes: 1024, frames: 100, encoded: 20, encoder: "cpu".into(), size: (640, 480) })).unwrap();
            write_export_msg(&mut host, &ExportMsg::Chunk(vec![7u8; 256])).unwrap();
            write_export_msg(&mut host, &ExportMsg::Progress { done: 0.5 }).unwrap();
            // Wait for client's write half to close (EOF) signaling immediate cancellation.
            let mut b = [0u8; 16];
            if matches!(host.read(&mut b), Ok(0) | Err(_)) {
                host_saw_eof_w.store(true, Ordering::Release);
            }
        });

        let src = StreamScreenSource::new(remote_info("qa"), opener);
        let err = src.export_video(None, 1, &out, &mut |done| done < 0.4).unwrap_err();
        host_thread.join().unwrap();

        assert_eq!(err, "cancelled");
        assert!(host_saw_eof.load(Ordering::Acquire), "host must be told immediately via EOF on cancel");
        assert!(!out.exists(), "cancelled export must leave no final file");
        let parts: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".part")).collect();
        assert!(parts.is_empty(), "cancelled export must leave no .part file: {parts:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lifecycle_commands_and_refused_start_return_helpers_words() {
        let opener = MockOpener::with_commands(vec![
            Ok(CommandOutput { status: 1, stdout: String::new(), stderr: "olo-screen: screen qa is already running\n".into() }),
            Ok(CommandOutput { status: 0, stdout: r#"{"name":"qa","width":1920,"height":1080,"scale":2.0,"sound":true,"started":1700000000}"#.into(), stderr: String::new() }),
            Ok(CommandOutput { status: 0, stdout: String::new(), stderr: String::new() }),
            Ok(CommandOutput { status: 0, stdout: String::new(), stderr: String::new() }),
            Ok(CommandOutput { status: 0, stdout: String::new(), stderr: String::new() }),
            Ok(CommandOutput { status: 0, stdout: String::new(), stderr: String::new() }),
            Ok(CommandOutput { status: 0, stdout: String::new(), stderr: String::new() }),
            Ok(CommandOutput { status: 0, stdout: String::new(), stderr: String::new() }),
            Ok(CommandOutput { status: 1, stdout: String::new(), stderr: "screen qa is running: stop it first".into() }),
        ]);

        let mut info = remote_info("qa");
        info.state = ScreenState::Stopped;
        let src = StreamScreenSource::new(info, opener.clone());

        // Refused start surfaces the helper's exact words.
        assert_eq!(src.start().unwrap_err(), "olo-screen: screen qa is already running");

        // Successful start parses JSON dimensions/scale/sound/started into ScreenInfo.
        let started = src.start().unwrap();
        assert_eq!(started.state, ScreenState::Running);
        assert_eq!(started.size, Some((1920, 1080)));
        assert_eq!(started.scale, 2.0);
        assert!(started.sound);
        assert_eq!(started.started_ms, Some(1_700_000_000_000));

        // Stop, rename (set + clear), clear history, archive, unarchive, delete (refused).
        src.stop().unwrap();
        assert_eq!(src.info().state, ScreenState::Stopped);
        src.rename(Some("QA Box")).unwrap();
        assert_eq!(src.info().title.as_deref(), Some("QA Box"));
        src.rename(None).unwrap();
        assert_eq!(src.info().title, None);
        src.clear_history().unwrap();
        src.archive(true).unwrap();
        assert_eq!(src.info().state, ScreenState::Archived);
        src.archive(false).unwrap();
        assert_eq!(src.info().state, ScreenState::Stopped);
        assert_eq!(src.delete().unwrap_err(), "screen qa is running: stop it first");

        // Passwords remain plainly unavailable, never faked.
        assert!(matches!(src.info().capabilities.passwords, PasswordsMode::Unavailable(_)));

        let calls = opener.cmd_calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![
                vec!["start", "qa", "--keep"],
                vec!["start", "qa", "--keep"],
                vec!["stop", "qa", "--expected-started", "1700000000"],
                vec!["rename", "qa", "QA Box"],
                vec!["rename", "qa"],
                vec!["history", "qa", "--clear"],
                vec!["archive", "qa"],
                vec!["unarchive", "qa"],
                vec!["delete", "qa"],
            ]
        );
    }

    // ---- Review 2 regression tests ----------------------------------------

    #[test]
    fn review2_f2_f11_f17_export_budget_exclusive_part_and_cancel_during_download() {
        let dir = scratch_dir("r2-export");
        let out = dir.join("clip.mp4");
        let unrelated_part = dir.join("clip.mp4.part");
        std::fs::write(&unrelated_part, b"someone-elses-work").unwrap();

        // 1. Finding #2: host refusal before Meta must NEVER delete pre-existing `.part`.
        {
            let (opener, mut hosts) = MockOpener::with_export_hosts(1);
            let mut host = hosts.remove(0);
            let t = std::thread::spawn(move || {
                write_export_msg(&mut host, &ExportMsg::Error { message: "refused".into() }).unwrap();
            });
            let src = StreamScreenSource::new(remote_info("qa"), opener);
            assert_eq!(src.export_video(None, 1, &out, &mut |_| true).unwrap_err(), "refused");
            t.join().unwrap();
            assert_eq!(std::fs::read(&unrelated_part).unwrap(), b"someone-elses-work");
        }

        // 2. Finding #11: Meta exceeding desktop byte budget is rejected BEFORE creating any file.
        {
            let (opener, mut hosts) = MockOpener::with_export_hosts(1);
            let mut host = hosts.remove(0);
            let t = std::thread::spawn(move || {
                write_export_msg(&mut host, &ExportMsg::Meta(ExportMeta { bytes: 8 << 20, frames: 10, encoded: 5, encoder: "cpu".into(), size: (640, 480) })).unwrap();
            });
            let mut src = StreamScreenSource::new(remote_info("qa"), opener);
            src.max_export_bytes = 1 << 20;
            let err = src.export_video(None, 1, &out, &mut |_| true).unwrap_err();
            assert!(err.contains("exceeds desktop limit"), "got: {err}");
            t.join().unwrap();
            assert!(!out.exists());
            assert_eq!(std::fs::read(&unrelated_part).unwrap(), b"someone-elses-work");
        }

        // 3. Finding #17: cancelling via `ExportCancel` while blocked in download (after last
        //    Progress, zero further progress messages!) unblocks immediately and tells host.
        {
            let (opener, mut hosts) = MockOpener::with_export_hosts(1);
            let mut host = hosts.remove(0);
            let (chunk_sent_tx, chunk_sent_rx) = channel();
            let host_saw_eof = Arc::new(AtomicBool::new(false));
            let eof_w = host_saw_eof.clone();
            let host_t = std::thread::spawn(move || {
                write_export_msg(&mut host, &ExportMsg::Progress { done: 1.0 }).unwrap();
                write_export_msg(&mut host, &ExportMsg::Meta(ExportMeta { bytes: 65_536, frames: 20, encoded: 10, encoder: "cpu".into(), size: (640, 480) })).unwrap();
                write_export_msg(&mut host, &ExportMsg::Chunk(vec![42u8; 1024])).unwrap();
                let _ = chunk_sent_tx.send(());
                // Stall: send no more chunks or progress; wait for client cancel EOF.
                let mut b = [0u8; 8];
                if matches!(host.read(&mut b), Ok(0) | Err(_)) {
                    eof_w.store(true, Ordering::Release);
                }
            });

            let cancel = ExportCancel::new();
            let cancel_worker = cancel.clone();
            let out_w = out.clone();
            let src = Arc::new(StreamScreenSource::new(remote_info("qa"), opener));
            let worker = std::thread::spawn(move || src.export_video_cancellable(None, 1, &out_w, &cancel_worker, &mut |_| true));

            recv_within(&chunk_sent_rx, "first chunk sent");
            std::thread::sleep(Duration::from_millis(30));
            let t0 = Instant::now();
            cancel.cancel();
            assert_eq!(worker.join().unwrap().unwrap_err(), "cancelled");
            assert!(t0.elapsed() < Duration::from_secs(1), "download cancel must unblock promptly, took {:?}", t0.elapsed());
            host_t.join().unwrap();
            assert!(host_saw_eof.load(Ordering::Acquire), "host must observe EOF on download cancel");
            assert!(!out.exists());
            assert_eq!(std::fs::read(&unrelated_part).unwrap(), b"someone-elses-work");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn review2_f12_f40_f50_f55_history_bounds_multichunk_reconnect_index_reset_and_timestamps() {
        // 1. Finding #50: multi-chunk reconnect actions snapshot (2 chunks) matches
        //    delivered prefix across chunks without resetting, appending only new tail.
        {
            let (opener, mut hosts) = MockOpener::with_history_hosts(2);
            let mut host1 = hosts.remove(0);
            let mut host2 = hosts.remove(0);
            let src = StreamScreenSource::with_reconnect(remote_info("qa"), opener, ReconnectPolicy { delays: vec![Duration::from_millis(10)] });
            let (tx, rx) = channel();
            let handle = src.follow_history(Box::new(move |ev| {
                let _ = tx.send(ev);
            }));

            let lines: Vec<String> = (1..=6).map(|i| action_line(i * 1_000, 10, "click", &[&i.to_string()])).collect();
            write_history_msg(&mut host1, &HistoryMsg::Actions { reset: true, lines: lines[0..4].to_vec() }).unwrap();
            write_history_msg(&mut host1, &HistoryMsg::Index { reset: true, last_size: Some((640, 480)), frames: vec![HistoryFrameMark { at: 1_000, key: true }] }).unwrap();
            write_history_msg(&mut host1, &HistoryMsg::SnapshotDone { actions_gen: 1, index_gen: 1 }).unwrap();
            let _ = recv_within(&rx, "init snapshot");

            // Drop conn 1; conn 2 replays the 4 existing actions in TWO chunks (2 + 2) plus 2 new ones!
            drop(host1);
            write_history_msg(&mut host2, &HistoryMsg::Actions { reset: true, lines: lines[0..2].to_vec() }).unwrap();
            write_history_msg(&mut host2, &HistoryMsg::Actions { reset: false, lines: lines[2..6].to_vec() }).unwrap();
            // Finding #40: new `index_gen` with IDENTICAL timestamp must STILL emit index_reset: true!
            write_history_msg(&mut host2, &HistoryMsg::Index { reset: true, last_size: Some((640, 480)), frames: vec![HistoryFrameMark { at: 1_000, key: true }] }).unwrap();
            assert!(rx.recv_timeout(Duration::from_millis(40)).is_err(), "multi-chunk reconnect must wait for SnapshotDone");
            write_history_msg(&mut host2, &HistoryMsg::SnapshotDone { actions_gen: 1, index_gen: 2 }).unwrap();

            match recv_within(&rx, "multi-chunk reconnect tail + index_reset") {
                HistoryEvent::Loaded { actions, reset, index_reset, .. } => {
                    assert!(!reset, "multi-chunk matching reconnect must NOT reset action list");
                    assert!(index_reset, "new index_gen at same timestamp must set index_reset=true");
                    assert_eq!(actions.len(), 2, "must emit only the 2 unseen tail actions");
                    assert_eq!(actions[0].at, 5_000);
                    assert_eq!(actions[1].at, 6_000);
                }
                HistoryEvent::Failed(e) => panic!("unexpected failure: {e}"),
            }
            drop(handle);
        }

        // 2. Finding #55: overflowing action `at + ms` and out-of-order frame marks are rejected cleanly.
        {
            let (opener, mut hosts) = MockOpener::with_history_hosts(1);
            let mut host = hosts.remove(0);
            let src = StreamScreenSource::new(remote_info("qa"), opener);
            let (tx, rx) = channel();
            let handle = src.follow_history(Box::new(move |ev| {
                let _ = tx.send(ev);
            }));
            write_history_msg(&mut host, &HistoryMsg::Actions { reset: true, lines: vec![action_line(u64::MAX - 5, 100, "click", &[])] }).unwrap();
            match recv_within(&rx, "overflow action rejected") {
                HistoryEvent::Failed(msg) => assert!(msg.contains("overflow"), "got: {msg}"),
                other => panic!("expected Failed on overflowing timestamp, got {:?}", std::mem::discriminant(&other)),
            }
            drop(handle);
        }
        {
            let (opener, mut hosts) = MockOpener::with_history_hosts(1);
            let mut host = hosts.remove(0);
            let src = StreamScreenSource::new(remote_info("qa"), opener);
            let (tx, rx) = channel();
            let handle = src.follow_history(Box::new(move |ev| {
                let _ = tx.send(ev);
            }));
            write_history_msg(
                &mut host,
                &HistoryMsg::Index { reset: true, last_size: Some((640, 480)), frames: vec![HistoryFrameMark { at: 2_000, key: true }, HistoryFrameMark { at: 1_000, key: false }] },
            )
            .unwrap();
            match recv_within(&rx, "out-of-order marks rejected") {
                HistoryEvent::Failed(msg) => assert!(msg.contains("out of order"), "got: {msg}"),
                other => panic!("expected Failed on out-of-order frame marks, got {:?}", std::mem::discriminant(&other)),
            }
            drop(handle);
        }

        // 3. Finding #12: desktop bounds retained remote actions.
        {
            let (opener, mut hosts) = MockOpener::with_history_hosts(1);
            let mut host = hosts.remove(0);
            let mut src = StreamScreenSource::new(remote_info("qa"), opener);
            src.max_retained_actions = 4;
            let (tx, rx) = channel();
            let handle = src.follow_history(Box::new(move |ev| {
                let _ = tx.send(ev);
            }));
            let lines: Vec<String> = (1..=8).map(|i| action_line(i * 100, 10, "click", &[])).collect();
            write_history_msg(&mut host, &HistoryMsg::Actions { reset: true, lines }).unwrap();
            write_history_msg(&mut host, &HistoryMsg::SnapshotDone { actions_gen: 1, index_gen: 1 }).unwrap();
            match recv_within(&rx, "bounded actions") {
                HistoryEvent::Loaded { actions, reset, .. } => {
                    assert!(reset);
                    assert!(actions.len() <= 4, "retained actions must respect cap: got {}", actions.len());
                }
                HistoryEvent::Failed(e) => panic!("unexpected failure: {e}"),
            }
            drop(handle);
        }
    }

    #[test]
    fn review2_f16_drop_returns_immediately_and_cancels_blocked_writer() {
        struct StalledWriterOpener(Arc<AtomicBool>);
        impl StreamOpener for StalledWriterOpener {
            fn open_live(&self, _name: &str, _cancelled: Arc<AtomicBool>) -> Result<LiveStream, String> {
                struct BlockedReader(Arc<(Mutex<bool>, Condvar)>);
                impl Read for BlockedReader {
                    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
                        let mut g = self.0.0.lock().unwrap();
                        while !*g {
                            g = self.0.1.wait(g).unwrap();
                        }
                        Ok(0)
                    }
                }
                struct SlowWriter;
                impl Write for SlowWriter {
                    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                        Ok(buf.len())
                    }
                    fn flush(&mut self) -> io::Result<()> {
                        Ok(())
                    }
                }
                let gate = Arc::new((Mutex::new(false), Condvar::new()));
                let g2 = gate.clone();
                let flag = self.0.clone();
                Ok(LiveStream::with_cancel(BlockedReader(gate), SlowWriter, move || {
                    flag.store(true, Ordering::SeqCst);
                    // Simulate slow cancel teardown that must NEVER block UI drop!
                    std::thread::sleep(Duration::from_millis(180));
                    *g2.0.lock().unwrap() = true;
                    g2.1.notify_all();
                }))
            }
            fn open_history(&self, _: &str, _: Arc<AtomicBool>) -> Result<LiveStream, String> {
                Err("unused".into())
            }
            fn open_export(&self, _: &str, _: &[String], _: Arc<AtomicBool>) -> Result<LiveStream, String> {
                Err("unused".into())
            }
            fn run_command(&self, _: &[String], _: Arc<AtomicBool>) -> Result<CommandOutput, String> {
                Err("unused".into())
            }
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let src = StreamScreenSource::new(remote_info("qa"), Arc::new(StalledWriterOpener(cancelled.clone())));
        let handle = src.watch_live(Vec::new(), Box::new(|_| {}));
        std::thread::sleep(Duration::from_millis(20));
        let ctrl = src.active.lock().unwrap().clone().unwrap();
        // Cancel hook itself blocks 180 ms in our mock; verify UI drop finishes
        // without waiting on `session_join` after cancel completes, and all threads exit.
        drop(handle);
        assert!(cancelled.load(Ordering::Acquire));
        wait_threads_zero(&ctrl.live_threads, Duration::from_secs(2));
    }

    #[test]
    fn review2_f21_f23_prefix_bounded_frame_and_input_batch_splitting() {
        // 1. Finding #21: lying 250 MiB length prefix with immediate EOF fails cleanly
        //    without trying to allocate 250 MiB.
        let lying: &[u8] = &[MSG_FRAME, 0x00, 0x00, 0xA0, 0x0F, 1, 0, 0, 0, 0, 0, 0, 0];
        let mut cur = io::Cursor::new(lying);
        let err = read_server_msg_bounded(&mut cur).err().expect("truncated lying frame must fail");
        assert!(matches!(err.kind(), io::ErrorKind::UnexpectedEof | io::ErrorKind::InvalidData));

        // 2. Finding #23: `split_input_for_wire` splits 150 keystrokes into chunks of <= 64
        //    and <= MAX_WIRE_INPUT_BYTES while preserving exact order.
        let events: Vec<SyntheticEvent> = (0..150).map(|i| SyntheticEvent::Key { key: 30 + (i % 10), pressed: i % 2 == 0 }).collect();
        let chunks = split_input_for_wire(&events);
        assert!(chunks.len() >= 3, "150 events must split into >= 3 wire messages");
        let mut rebuilt = Vec::new();
        for c in &chunks {
            assert!(c.len() <= MAX_WIRE_INPUT_BYTES);
            let parsed: Vec<SyntheticEvent> = serde_json::from_str(c).unwrap();
            assert!(parsed.len() <= MAX_WIRE_INPUT_EVENTS);
            rebuilt.extend(parsed);
        }
        assert_eq!(rebuilt, events);
    }

    #[test]
    fn review2_f31_f32_f33_terminal_error_pinned_screenshot_timeout_and_backoff_gate() {
        // 1. Finding #31: `frame_at` after terminal host error returns Err immediately
        //    (never hangs waiting for a dead writer), and reactivation with identical
        //    snapshot emits Loaded to clear Replay's error banner.
        {
            let (opener, mut hosts) = MockOpener::with_history_hosts(2);
            let mut host1 = hosts.remove(0);
            let mut host2 = hosts.remove(0);
            let src = StreamScreenSource::new(remote_info("qa"), opener);
            let (tx, rx) = channel();
            let mut handle = src.follow_history(Box::new(move |ev| {
                let _ = tx.send(ev);
            }));
            let l1 = action_line(1_000, 10, "start", &[]);
            write_history_msg(&mut host1, &HistoryMsg::Actions { reset: true, lines: vec![l1.clone()] }).unwrap();
            write_history_msg(&mut host1, &HistoryMsg::Index { reset: true, last_size: Some((640, 480)), frames: vec![HistoryFrameMark { at: 1_000, key: true }] }).unwrap();
            write_history_msg(&mut host1, &HistoryMsg::SnapshotDone { actions_gen: 1, index_gen: 1 }).unwrap();
            let _ = recv_within(&rx, "init snapshot");

            write_history_msg(&mut host1, &HistoryMsg::Error { message: "disk gone".into() }).unwrap();
            match recv_within(&rx, "host error") {
                HistoryEvent::Failed(m) => assert_eq!(m, "disk gone"),
                other => panic!("expected Failed, got {:?}", std::mem::discriminant(&other)),
            }
            // Wait briefly for hist-s to park after setting abort_reason.
            std::thread::sleep(Duration::from_millis(30));
            let err = src.frame_at(1_000).unwrap_err();
            assert_eq!(err, "disk gone", "frame_at after terminal error must fail immediately, never hang");

            // Deactivate then reactivate with identical snapshot -> must emit Loaded so Replay clears error.
            handle.set_active(false);
            handle.set_active(true);
            write_history_msg(&mut host2, &HistoryMsg::Actions { reset: true, lines: vec![l1] }).unwrap();
            write_history_msg(&mut host2, &HistoryMsg::Index { reset: true, last_size: Some((640, 480)), frames: vec![HistoryFrameMark { at: 1_000, key: true }] }).unwrap();
            write_history_msg(&mut host2, &HistoryMsg::SnapshotDone { actions_gen: 1, index_gen: 1 }).unwrap();
            assert!(matches!(recv_within(&rx, "reactivated Loaded"), HistoryEvent::Loaded { .. }));
            drop(handle);
        }

        // 2. Finding #32: `frame_at_pinned` uses its own per-request cancel & timeout,
        //    so `cancel_frame_at` (scrubbing) cannot abort a screenshot, and a silent host times out.
        {
            let (opener, mut hosts) = MockOpener::with_history_hosts(1);
            let _silent_host = hosts.remove(0);
            let mut src = StreamScreenSource::new(remote_info("qa"), opener);
            src.frame_at_timeout = Duration::from_millis(120);
            let cancel = ExportCancel::new();
            let t0 = Instant::now();
            let err = src.frame_at_pinned(1_000, &cancel).unwrap_err();
            assert!(err.contains("timed out"), "silent host must time out cleanly, got: {err}");
            assert!(t0.elapsed() < Duration::from_secs(1));
        }

        // 3. Finding #33: host that sends Actions and immediately dies BEFORE Index
        //    must NOT reset backoff (exhausts retries cleanly instead of spinning forever).
        {
            let (opener, hosts) = MockOpener::with_history_hosts(4);
            for mut h in hosts {
                write_history_msg(&mut h, &HistoryMsg::Actions { reset: true, lines: vec![] }).unwrap();
                drop(h);
            }
            let src = StreamScreenSource::with_reconnect(remote_info("qa"), opener.clone(), ReconnectPolicy { delays: vec![Duration::from_millis(10), Duration::from_millis(10)] });
            let (tx, rx) = channel();
            let handle = src.follow_history(Box::new(move |ev| {
                let _ = tx.send(ev);
            }));
            loop {
                match recv_within(&rx, "backoff exhaustion") {
                    HistoryEvent::Failed(_) => break,
                    HistoryEvent::Loaded { .. } => continue,
                }
            }
            assert_eq!(opener.history_open_count(), 3, "incomplete snapshot must exhaust retries (1 + 2), not reset backoff forever");
            drop(handle);
        }
    }

    #[test]
    fn review2_f7_f37_f38_f47_explicit_control_release_ack_reconnect_and_resize() {
        let (opener, mut hosts) = MockOpener::with_hosts(1);
        let mut host = hosts.remove(0);
        let src = Arc::new(StreamScreenSource::new(remote_info("qa"), opener));

        // Finding #37: set_controlled(true) before watch_live fails.
        assert!(src.set_controlled(true).is_err());

        let (ev_tx, ev_rx) = channel();
        let handle = src.watch_live(
            Vec::new(),
            Box::new(move |ev| {
                let _ = ev_tx.send(ev);
            }),
        );
        let f1 = solid_frame(128, 64, [1, 2, 3, 255]);
        write_server_msg(&mut host, &ServerMsg::Frame { seq: 1, delta: encode_delta(&f1, None, TileEncoding::PackedStream, 1).unwrap().encode_bytes() }).unwrap();
        assert_eq!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Ack { seq: 1 }));
        let _ = recv_within(&ev_rx, "frame 1");

        // Finding #47: resize frame updates `info.size` dynamically (not only when initially None).
        let f2 = solid_frame(256, 128, [4, 5, 6, 255]);
        write_server_msg(&mut host, &ServerMsg::Frame { seq: 2, delta: encode_delta(&f2, Some(&f1), TileEncoding::PackedStream, 1).unwrap().encode_bytes() }).unwrap();
        assert_eq!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Ack { seq: 2 }));
        let _ = recv_within(&ev_rx, "frame 2");
        assert_eq!(src.info().size, Some((256, 128)));

        // Finding #37: while control is Requested (before host grants), input MUST be dropped.
        src.set_controlled(true).unwrap();
        let seq1 = match read_client_msg(&mut host).unwrap() {
            Some(ClientMsg::ControlSeq { on: true, seq }) => seq,
            other => panic!("expected ControlSeq(true), got {other:?}"),
        };
        src.send_input(vec![SyntheticEvent::Key { key: 30, pressed: true }]);
        // Grant control explicitly for exact `seq1`.
        write_server_msg(&mut host, &ServerMsg::Control { held: true, seq: seq1, reason: None }).unwrap();
        assert!(matches!(recv_within(&ev_rx, "grant"), LiveEvent::Control { held: true, .. }));

        // Finding #7: host emits unsolicited Control(held=false, seq=0, reason) before Reconnecting.
        write_server_msg(&mut host, &ServerMsg::Control { held: false, seq: 0, reason: Some("Compositor restarted; control released.".into()) }).unwrap();
        write_server_msg(&mut host, &ServerMsg::Status(StreamStatus { message: "Reconnecting…".into(), reconnecting: true, error: false })).unwrap();
        assert!(matches!(
            recv_within(&ev_rx, "lost control"),
            LiveEvent::Control { held: false, reason: Some(ref r) } if r.contains("Compositor restarted")
        ));
        assert!(matches!(recv_within(&ev_rx, "reconnecting"), LiveEvent::Status(LiveStatus::Reconnecting)));

        // Re-take control and test Finding #38: `release_control_and_wait` blocks until host sends Control(held=false, seq)!
        src.set_controlled(true).unwrap();
        let seq2 = match read_client_msg(&mut host).unwrap() {
            Some(ClientMsg::ControlSeq { on: true, seq }) => seq,
            other => panic!("expected ControlSeq(true) #2, got {other:?}"),
        };
        write_server_msg(&mut host, &ServerMsg::Control { held: true, seq: seq2, reason: None }).unwrap();
        assert!(matches!(recv_within(&ev_rx, "grant 2"), LiveEvent::Control { held: true, .. }));

        let src_w = src.clone();
        let rel_done = Arc::new(AtomicBool::new(false));
        let rel_done_w = rel_done.clone();
        let rel_t = std::thread::spawn(move || {
            src_w.release_control_and_wait(vec![SyntheticEvent::Key { key: 30, pressed: false }]).unwrap();
            rel_done_w.store(true, Ordering::SeqCst);
        });
        assert!(matches!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Input { .. })));
        let rel_seq = match read_client_msg(&mut host).unwrap() {
            Some(ClientMsg::ControlSeq { on: false, seq }) => seq,
            other => panic!("expected ControlSeq(false), got {other:?}"),
        };
        assert!(rel_seq > seq2);
        std::thread::sleep(Duration::from_millis(50));
        assert!(!rel_done.load(Ordering::Acquire), "release_control_and_wait must wait for host release ack");
        write_server_msg(&mut host, &ServerMsg::Control { held: false, seq: rel_seq, reason: None }).unwrap();
        rel_t.join().unwrap();
        assert!(rel_done.load(Ordering::Acquire));

        // Finding #37: terminal ScreenGone retires handle and exits stream-d cleanly; subsequent set_controlled fails.
        let ctrl = src.active.lock().unwrap().clone().unwrap();
        write_server_msg(&mut host, &ServerMsg::ScreenGone { reason: "stopped".into() }).unwrap();
        assert!(matches!(recv_within(&ev_rx, "screen gone"), LiveEvent::Status(LiveStatus::Disconnected(_))));
        wait_threads_zero(&ctrl.live_threads, Duration::from_secs(2));
        assert!(src.set_controlled(true).is_err(), "terminal handle must reject control requests");
        drop(handle);
    }

    #[test]
    fn review2_f42_stream_source_rejects_invalid_and_option_shaped_names() {
        let opener = Arc::new(MockOpener::empty());
        for bad in ["../escape", "--clear", "-x"] {
            let src = StreamScreenSource::new(remote_info(bad), opener.clone());
            assert!(src.stop().is_err(), "stop must reject {bad:?}");
            assert!(src.clear_history().is_err(), "clear_history must reject {bad:?}");
            assert!(src.export_video(None, 1, Path::new("/var/tmp/unused.mp4"), &mut |_| true).is_err(), "export must reject {bad:?}");
            assert!(src.frame_at_pinned(100, &ExportCancel::new()).is_err(), "frame_at_pinned must reject {bad:?}");
        }
        assert!(opener.cmd_calls.lock().unwrap().is_empty(), "invalid names must never invoke StreamOpener");
    }

    #[test]
    fn review2_host_wire_control_state_warning_frame_error_and_snapshot_done() {
        // Live wire: MSG_CONTROL_STATE (0x04) + MSG_WARNING (0x05).
        let (opener, mut hosts) = MockOpener::with_hosts(1);
        let mut host = hosts.remove(0);
        let src = Arc::new(StreamScreenSource::new(remote_info("qa"), opener));
        let (ev_tx, ev_rx) = channel();
        let handle = src.watch_live(
            Vec::new(),
            Box::new(move |ev| {
                let _ = ev_tx.send(ev);
            }),
        );

        let f1 = solid_frame(64, 64, [10, 20, 30, 255]);
        write_server_msg(&mut host, &ServerMsg::Frame { seq: 1, delta: encode_delta(&f1, None, TileEncoding::PackedStream, 1).unwrap().encode_bytes() }).unwrap();
        assert_eq!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Ack { seq: 1 }));
        let _ = recv_within(&ev_rx, "frame 1");

        src.set_controlled(true).unwrap();
        let take_seq = match read_client_msg(&mut host).unwrap() {
            Some(ClientMsg::ControlSeq { on: true, seq }) => seq,
            other => panic!("expected ControlSeq(true), got {other:?}"),
        };
        // Stale/wrong seq grant must be ignored and must NOT unlock control or input.
        write_server_msg(&mut host, &ServerMsg::Control { held: true, seq: take_seq + 99, reason: None }).unwrap();
        assert!(ev_rx.recv_timeout(Duration::from_millis(40)).is_err(), "mismatched seq must not grant control");
        assert!(!src.info().controlled);

        // Exact seq grants control.
        write_server_msg(&mut host, &ServerMsg::Control { held: true, seq: take_seq, reason: None }).unwrap();
        assert!(matches!(recv_within(&ev_rx, "control state grant"), LiveEvent::Control { held: true, .. }));

        // Non-fatal warning must surface LiveStatus::Warning without dropping control or ending stream.
        write_server_msg(&mut host, &ServerMsg::Warning { message: "transient capture hiccup".into() }).unwrap();
        assert!(matches!(recv_within(&ev_rx, "warning"), LiveEvent::Status(LiveStatus::Warning(m)) if m == "transient capture hiccup"));
        assert!(src.info().controlled, "warning must not revoke control");

        // Release ack via MSG_CONTROL_STATE unblocks release_control_and_wait.
        let src_w = src.clone();
        let rel_t = std::thread::spawn(move || src_w.release_control_and_wait(Vec::new()).unwrap());
        let rel_seq = match read_client_msg(&mut host).unwrap() {
            Some(ClientMsg::ControlSeq { on: false, seq }) => seq,
            other => panic!("expected ControlSeq(false), got {other:?}"),
        };
        write_server_msg(&mut host, &ServerMsg::Control { held: false, seq: rel_seq, reason: None }).unwrap();
        rel_t.join().unwrap();
        assert!(!src.info().controlled);
        drop(handle);

        // History wire: MSG_HISTORY_FRAME_ERROR (0x25) fails only that req without closing stream,
        // and MSG_HISTORY_SNAPSHOT_DONE (0x26) commits snapshot + advances index_gen cleanly.
        let (h_opener, mut h_hosts) = MockOpener::with_history_hosts(1);
        let mut h_host = h_hosts.remove(0);
        let h_src = Arc::new(StreamScreenSource::new(remote_info("qa"), h_opener));
        let (hev_tx, hev_rx) = channel();
        let h_handle = h_src.follow_history(Box::new(move |ev| {
            let _ = hev_tx.send(ev);
        }));
        write_history_msg(&mut h_host, &HistoryMsg::Actions { reset: true, lines: vec![action_line(1_000, 10, "click", &[])] }).unwrap();
        write_history_msg(&mut h_host, &HistoryMsg::Index { reset: true, last_size: Some((64, 64)), frames: vec![HistoryFrameMark { at: 1_000, key: true }] }).unwrap();
        write_history_msg(&mut h_host, &HistoryMsg::SnapshotDone { actions_gen: 1, index_gen: 1 }).unwrap();
        let _ = recv_within(&hev_rx, "snapshot");

        let mut h_reader = BufReader::new(h_host.try_clone().unwrap());
        let h_src_1 = h_src.clone();
        let t_err = std::thread::spawn(move || h_src_1.frame_at(1_000));
        let Some(HistoryRequest::FrameAt { req, .. }) = read_history_request(&mut h_reader).unwrap() else { panic!("expected FrameAt") };
        write_history_msg(&mut h_host, &HistoryMsg::FrameError { req, message: "frame exceeds remote wire limit".into() }).unwrap();
        let err = t_err.join().unwrap().unwrap_err();
        assert!(err.contains("wire limit"), "unexpected err: {err}");

        // Stream MUST remain open and serve the next FrameAt request cleanly!
        let h_src_2 = h_src.clone();
        let t_ok = std::thread::spawn(move || h_src_2.frame_at(1_000));
        let Some(HistoryRequest::FrameAt { req: req2, .. }) = read_history_request(&mut h_reader).unwrap() else { panic!("expected FrameAt 2") };
        write_history_msg(&mut h_host, &HistoryMsg::Frame { req: req2, frame_at: 1_000, delta: encode_delta(&f1, None, TileEncoding::PackedStream, 1).unwrap().encode_bytes() }).unwrap();
        let (ts, frame) = t_ok.join().unwrap().unwrap().unwrap();
        assert_eq!((ts, frame.width, frame.height), (1_000, 64, 64));
        drop(h_handle);
    }

    #[test]
    fn follow_wire_release_ack_does_not_clobber_newer_take_seq() {
        let (opener, mut hosts) = MockOpener::with_hosts(1);
        let mut host = hosts.remove(0);
        let src = Arc::new(StreamScreenSource::new(remote_info("qa"), opener));
        let (ev_tx, ev_rx) = channel();
        let handle = src.watch_live(
            Vec::new(),
            Box::new(move |ev| {
                let _ = ev_tx.send(ev);
            }),
        );

        let f1 = solid_frame(64, 64, [1, 2, 3, 255]);
        write_server_msg(&mut host, &ServerMsg::Frame { seq: 1, delta: encode_delta(&f1, None, TileEncoding::PackedStream, 1).unwrap().encode_bytes() }).unwrap();
        assert_eq!(read_client_msg(&mut host).unwrap(), Some(ClientMsg::Ack { seq: 1 }));
        let _ = recv_within(&ev_rx, "frame 1");

        // Take #1 -> granted -> release #2 -> immediately take #3 BEFORE release #2 ack arrives.
        src.set_controlled(true).unwrap();
        let seq1 = match read_client_msg(&mut host).unwrap() {
            Some(ClientMsg::ControlSeq { on: true, seq }) => seq,
            other => panic!("expected take #1, got {other:?}"),
        };
        write_server_msg(&mut host, &ServerMsg::Control { held: true, seq: seq1, reason: None }).unwrap();
        assert!(matches!(recv_within(&ev_rx, "grant #1"), LiveEvent::Control { held: true, .. }));

        src.set_controlled(false).unwrap();
        let seq2 = match read_client_msg(&mut host).unwrap() {
            Some(ClientMsg::ControlSeq { on: false, seq }) => seq,
            other => panic!("expected release #2, got {other:?}"),
        };
        src.set_controlled(true).unwrap();
        let seq3 = match read_client_msg(&mut host).unwrap() {
            Some(ClientMsg::ControlSeq { on: true, seq }) => seq,
            other => panic!("expected take #3, got {other:?}"),
        };
        assert!(seq3 > seq2 && seq2 > seq1);

        // Host answers release #2 first: MUST NOT cancel pending take #3!
        write_server_msg(&mut host, &ServerMsg::Control { held: false, seq: seq2, reason: None }).unwrap();
        // Then host grants take #3 -> control MUST become Held!
        write_server_msg(&mut host, &ServerMsg::Control { held: true, seq: seq3, reason: None }).unwrap();
        assert!(matches!(recv_within(&ev_rx, "grant #3"), LiveEvent::Control { held: true, .. }));
        assert!(src.info().controlled);
        drop(handle);
    }

    #[test]
    fn follow_wire_multichunk_history_oversized_last_size_none_and_index_gen_reset() {
        let (opener, mut hosts) = MockOpener::with_history_hosts(1);
        let mut host = hosts.remove(0);
        let src = Arc::new(StreamScreenSource::new(remote_info("qa"), opener));
        let (tx, rx) = channel();
        let handle = src.follow_history(Box::new(move |ev| {
            let _ = tx.send(ev);
        }));

        // Multi-chunk initial snapshot: 2 Actions chunks (including an oversized placeholder at index 1)
        // + 2 Index chunks with `last_size: None` -> nothing emitted until SnapshotDone!
        let l0 = action_line(1_000, 10, "start", &[]);
        let l1 = r#"{"at":0,"ms":0,"kind":"oversized","args":[]}"#.to_owned();
        let l2 = action_line(3_000, 15, "click", &["--name", "Save"]);
        write_history_msg(&mut host, &HistoryMsg::Actions { reset: true, lines: vec![l0, l1] }).unwrap();
        write_history_msg(&mut host, &HistoryMsg::Actions { reset: false, lines: vec![l2] }).unwrap();
        write_history_msg(&mut host, &HistoryMsg::Index { reset: true, last_size: Some((1280, 720)), frames: vec![HistoryFrameMark { at: 1_000, key: true }] }).unwrap();
        write_history_msg(&mut host, &HistoryMsg::Index { reset: false, last_size: None, frames: vec![HistoryFrameMark { at: 3_000, key: false }] }).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err(), "must stage all chunks until SnapshotDone");

        write_history_msg(&mut host, &HistoryMsg::SnapshotDone { actions_gen: 1, index_gen: 1 }).unwrap();
        match recv_within(&rx, "multi-chunk initial snapshot") {
            HistoryEvent::Loaded { actions, texts, reset, index_reset, frames } => {
                assert!(reset);
                assert!(index_reset);
                assert_eq!(actions.len(), 3);
                assert_eq!(texts.len(), 3);
                assert!(!actions[0].is_oversized());
                assert!(actions[1].is_oversized(), "oversized placeholder must preserve index 1");
                assert_eq!(actions[2].at, 3_000);
                assert!(texts[1].contains("Action too large to show"));
                assert_eq!(frames.len(), 2);
                assert_eq!(frames.last_size, None, "last_size: None must be preserved cleanly");
            }
            HistoryEvent::Failed(e) => panic!("unexpected failure: {e}"),
        }

        // Incremental index append updating `last_size` to Some, then back to None when oversized frame arrives.
        write_history_msg(&mut host, &HistoryMsg::Index { reset: false, last_size: Some((800, 600)), frames: vec![HistoryFrameMark { at: 4_000, key: true }] }).unwrap();
        match recv_within(&rx, "incremental size Some") {
            HistoryEvent::Loaded { frames, .. } => assert_eq!(frames.last_size, Some((800, 600))),
            HistoryEvent::Failed(e) => panic!("unexpected failure: {e}"),
        }
        write_history_msg(&mut host, &HistoryMsg::Index { reset: false, last_size: None, frames: vec![HistoryFrameMark { at: 5_000, key: true }] }).unwrap();
        match recv_within(&rx, "incremental size None") {
            HistoryEvent::Loaded { frames, .. } => {
                assert_eq!(frames.len(), 4);
                assert_eq!(frames.last_size, None, "incremental Index must propagate last_size: None");
            }
            HistoryEvent::Failed(e) => panic!("unexpected failure: {e}"),
        }

        // Stale frame reply invalidated when new `index_gen` arrives on SnapshotDone.
        let ctrl = src.history.lock().unwrap().clone().unwrap();
        {
            let mut f = ctrl.frame.lock().unwrap();
            f.ready = Some((77, Ok(Some((1_000, solid_frame(16, 16, [1, 2, 3, 255]))))));
        }
        write_history_msg(&mut host, &HistoryMsg::Index { reset: true, last_size: Some((640, 480)), frames: vec![HistoryFrameMark { at: 1_000, key: true }] }).unwrap();
        write_history_msg(&mut host, &HistoryMsg::SnapshotDone { actions_gen: 1, index_gen: 2 }).unwrap();
        match recv_within(&rx, "index_gen 2 reset") {
            HistoryEvent::Loaded { reset, index_reset, frames, .. } => {
                assert!(!reset, "actions did not reset");
                assert!(index_reset, "new index_gen must emit index_reset=true");
                assert_eq!(frames.len(), 1);
            }
            HistoryEvent::Failed(e) => panic!("unexpected failure: {e}"),
        }
        assert!(ctrl.frame.lock().unwrap().ready.is_none(), "new index_gen must drop cached decoded frame from old generation");
        drop(handle);
    }

    #[test]
    fn follow_wire_protocol_cache_probed_once_per_host_and_legacy_helper_fallback() {
        // 1. Modern helper: shared HostProtocolCache probes `screen --protocol` ONCE across
        //    multiple screens and operations (including concurrent probes).
        let protocol_json = r#"{"proto":1,"commands":["follow","stream","history-stream","export"],"capabilities":["control-ack","warning-status","history-frame-error","history-snapshot-done","export-part-ranges","stop-expected-started","discontiguous_part"]}"#;
        let opener = MockOpener::with_commands(vec![
            Ok(CommandOutput { status: 0, stdout: protocol_json.into(), stderr: String::new() }),
            Ok(CommandOutput { status: 0, stdout: String::new(), stderr: String::new() }),
        ]);
        let cache = Arc::new(HostProtocolCache::new());
        let mut info1 = remote_info("qa-1");
        info1.started_ms = Some(1_700_000_042_000);
        info1.capabilities = PaneCapabilities::remote_default();
        info1.capabilities.can_control = true;
        let mut info2 = remote_info("qa-2");
        info2.capabilities = PaneCapabilities::remote_default();
        info2.capabilities.can_control = true;

        let src1 = Arc::new(StreamScreenSource::with_host_cache(info1, opener.clone(), cache.clone()));
        let src2 = Arc::new(StreamScreenSource::with_host_cache(info2, opener.clone(), cache.clone()));

        let s1 = src1.clone();
        let s2 = src2.clone();
        let t1 = std::thread::spawn(move || s1.probe_capabilities());
        let t2 = std::thread::spawn(move || s2.probe_capabilities());
        let c1 = t1.join().unwrap();
        let c2 = t2.join().unwrap();
        assert_eq!(c1, c2);
        assert!(c1.control_ack && c1.warning_status && c1.history_frame_error && c1.history_snapshot_done && c1.discontiguous_part && c1.stop_expected_started);
        assert!(c1.has_history_stream());
        assert!(src1.info().capabilities.explicit_control_ack);
        assert!(src2.info().capabilities.can_export_discontiguous);

        // `stop` uses cached capabilities without re-probing and passes `--expected-started`.
        src1.stop().unwrap();
        let calls = opener.cmd_calls.lock().unwrap().clone();
        assert_eq!(calls, vec![vec!["--protocol"], vec!["stop", "qa-1", "--expected-started", "1700000042"]]);

        // 2. Older helper (non-zero `--protocol`): cached once as legacy, works as before!
        let (legacy_opener, mut live_hosts) = MockOpener::with_hosts(1);
        let mut live_host = live_hosts.remove(0);
        let (hc, mut hist_host) = UnixStream::pair().unwrap();
        let _ = hist_host.set_read_timeout(Some(Duration::from_secs(5)));
        legacy_opener.history_plan.lock().unwrap().push_back(Ok(hc));
        let (ec, mut exp_host) = UnixStream::pair().unwrap();
        let _ = exp_host.set_read_timeout(Some(Duration::from_secs(5)));
        legacy_opener.export_plan.lock().unwrap().push_back(Ok(ec));
        legacy_opener.cmd_plan.lock().unwrap().extend([
            Ok(CommandOutput { status: 2, stdout: String::new(), stderr: "unknown option --protocol\n".into() }),
            Ok(CommandOutput { status: 0, stdout: String::new(), stderr: String::new() }),
        ]);

        let legacy_cache = Arc::new(HostProtocolCache::new());
        let mut legacy_info = remote_info("legacy");
        legacy_info.started_ms = Some(1_700_000_099_000);
        legacy_info.capabilities = PaneCapabilities::remote_legacy_with_control();
        let legacy_src = Arc::new(StreamScreenSource::with_host_cache(legacy_info, legacy_opener.clone(), legacy_cache));

        // Live control uses legacy `ClientMsg::Control { on }` (0x12) and works without ServerMsg::Control.
        let (lev_tx, lev_rx) = channel();
        let lhandle = legacy_src.watch_live(
            Vec::new(),
            Box::new(move |ev| {
                let _ = lev_tx.send(ev);
            }),
        );
        let f1 = solid_frame(64, 64, [7, 8, 9, 255]);
        write_server_msg(&mut live_host, &ServerMsg::Frame { seq: 1, delta: encode_delta(&f1, None, TileEncoding::PackedStream, 1).unwrap().encode_bytes() }).unwrap();
        assert_eq!(read_client_msg(&mut live_host).unwrap(), Some(ClientMsg::Ack { seq: 1 }));
        let _ = recv_within(&lev_rx, "legacy frame");

        legacy_src.set_controlled(true).unwrap();
        assert_eq!(read_client_msg(&mut live_host).unwrap(), Some(ClientMsg::Control { on: true }));
        assert!(matches!(recv_within(&lev_rx, "legacy local grant"), LiveEvent::Control { held: true, .. }));
        legacy_src.send_input(vec![SyntheticEvent::Wheel { dx: 0, dy: 1 }]);
        assert!(matches!(read_client_msg(&mut live_host).unwrap(), Some(ClientMsg::Input { .. })));
        let lrel = legacy_src.clone();
        let rel_t = std::thread::spawn(move || lrel.release_control_and_wait(Vec::new()).unwrap());
        assert_eq!(read_client_msg(&mut live_host).unwrap(), Some(ClientMsg::Control { on: false }));
        rel_t.join().unwrap();
        drop(lhandle);

        // Legacy history commits immediately without requiring SnapshotDone.
        let (htx, hrx) = channel();
        let hhandle = legacy_src.follow_history(Box::new(move |ev| {
            let _ = htx.send(ev);
        }));
        write_history_msg(&mut hist_host, &HistoryMsg::Actions { reset: true, lines: vec![action_line(1_000, 10, "start", &[])] }).unwrap();
        assert!(matches!(recv_within(&hrx, "legacy history without SnapshotDone"), HistoryEvent::Loaded { actions, .. } if actions.len() == 1));
        drop(hhandle);

        // Discontiguous export refused in plain words; contiguous export still succeeds.
        let dir = scratch_dir("legacy-export");
        let err = legacy_src.export_video(Some(&[0, 2]), 1, &dir.join("gap.mp4"), &mut |_| true).unwrap_err();
        assert!(err.contains("contiguous"), "got: {err}");
        let payload = b"legacy-contiguous-mp4".to_vec();
        let payload_h = payload.clone();
        let exp_t = std::thread::spawn(move || {
            write_export_msg(&mut exp_host, &ExportMsg::Meta(ExportMeta { bytes: payload_h.len() as u64, frames: 2, encoded: 2, encoder: "cpu".into(), size: (64, 64) })).unwrap();
            write_export_msg(&mut exp_host, &ExportMsg::Chunk(payload_h)).unwrap();
            write_export_msg(&mut exp_host, &ExportMsg::Done).unwrap();
        });
        let ok_path = dir.join("ok.mp4");
        legacy_src.export_video(Some(&[0, 1]), 1, &ok_path, &mut |_| true).unwrap();
        exp_t.join().unwrap();
        assert_eq!(std::fs::read(&ok_path).unwrap(), payload);
        let _ = std::fs::remove_dir_all(&dir);

        // Legacy `stop` omits `--expected-started` and never re-probed `--protocol`.
        legacy_src.stop().unwrap();
        let legacy_calls = legacy_opener.cmd_calls.lock().unwrap().clone();
        assert_eq!(legacy_calls, vec![vec!["--protocol"], vec!["stop", "legacy"]]);
    }
}
