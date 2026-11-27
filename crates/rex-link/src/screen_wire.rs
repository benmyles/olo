//! Wire protocol shared by the host helper (`olo-rex-link screen follow|stream`)
//! and the desktop client (`crates/rex`, `crates/screen-pane`).
//!
//! No GUI or C dependencies: the library stays light so `olo-rex-client` and
//! pure-Rust helper builds can depend on it directly. Tile-delta pixel encoding
//! and decoding use the single shared `olo-screen-codec` (`OLOT` binary format);
//! this module frames stream messages around it (`Frame`, `Status`, `ScreenGone`,
//! `Ack`, `Input`, `Control`, `Pause`), enforces strict length and geometry
//! bounds before allocating, and defines `follow` / ownership schemas.

use std::io::{self, BufRead, Read, Write};

use serde::{Deserialize, Serialize};

/// Wire protocol version for `screen follow` and `screen stream`.
pub const PROTO: u32 = 1;

/// Exit status returned by guarded `screen stop|delete|history --clear` when
/// the screen's live `started` or `last_used` no longer matches the caller's
/// expected value (checked and acted on under `NAME.lifecycle.lock`).
pub const EXIT_STALE: i32 = 75;

/// Magic header of `olo-screen-codec`'s `OLOT` tile-delta payload.
pub const OLOT_MAGIC: &[u8; 4] = b"OLOT";

/// Supported `OLOT` binary payload version.
pub const OLOT_VERSION: u8 = 1;

/// Maximum accepted frame width or height in pixels (matches `olo-screen-codec::MAX_NET_DIM`).
pub const MAX_DIM: u32 = 16_384;

/// Maximum accepted pixels in a single frame (64M pixels = 256 MiB RGBA,
/// matching `olo-screen-codec::MAX_NET_FRAME_BYTES` and covering 5K @ 2× and 8K @ 1×).
pub const MAX_FRAME_PIXELS: usize = 64 * 1024 * 1024;

/// Maximum accepted tile count in a single frame (`(16384 / 64)^2`).
pub const MAX_TILES: usize = 65_536;

/// Maximum payload length of a server → client binary message (258 MiB,
/// covering a worst-case incompressible frame up to `MAX_FRAME_PIXELS` plus headers).
pub const MAX_SERVER_PAYLOAD: u32 = (256 << 20) + (2 << 20);

/// Maximum payload length of a `MSG_STATUS` JSON message (64 KiB).
pub const MAX_STATUS_PAYLOAD: u32 = 64 << 10;

/// Maximum payload length of a `MSG_SCREEN_GONE` reason string (4 KiB).
pub const MAX_GONE_PAYLOAD: u32 = 4096;

/// Maximum payload length of a client → server binary message (256 KiB).
pub const MAX_CLIENT_PAYLOAD: u32 = 256 << 10;

/// Maximum NDJSON line length accepted by [`read_follow_event`] (4 MiB).
pub const MAX_JSON_LINE: usize = 4 << 20;

/// Maximum serialized byte length of the QUIC `SCREEN` subcommand selector.
pub const MAX_QUIC_SCREEN_ARGS_LEN: u16 = 4096;

/// Maximum argument count in a QUIC `SCREEN` subcommand selector.
pub const MAX_QUIC_SCREEN_ARGS: usize = 32;

/// Subcommands allowed over a QUIC `SCREEN` stream.
pub const QUIC_SCREEN_SUBCOMMANDS: &[&str] = &["follow", "stream", "history", "history-stream", "export"];

/// Server → client binary message tags (§3.1 live stream).
pub const MSG_FRAME: u8 = 0x01;
pub const MSG_STATUS: u8 = 0x02;
pub const MSG_SCREEN_GONE: u8 = 0x03;
pub const MSG_CONTROL_STATE: u8 = 0x04;
pub const MSG_WARNING: u8 = 0x05;

/// Client → server binary message tags (§3.1 live stream).
pub const MSG_ACK: u8 = 0x10;
pub const MSG_INPUT: u8 = 0x11;
pub const MSG_CONTROL: u8 = 0x12;
pub const MSG_PAUSE: u8 = 0x13;
pub const MSG_CONTROL_SEQ: u8 = 0x14;

/// Server → client binary message tags (`olo-rex-link screen history-stream NAME`).
pub const MSG_HISTORY_ACTIONS: u8 = 0x20;
pub const MSG_HISTORY_INDEX: u8 = 0x21;
pub const MSG_HISTORY_FRAME: u8 = 0x22;
pub const MSG_HISTORY_MISS: u8 = 0x23;
pub const MSG_HISTORY_ERROR: u8 = 0x24;
pub const MSG_HISTORY_FRAME_ERROR: u8 = 0x25;
pub const MSG_HISTORY_SNAPSHOT_DONE: u8 = 0x26;

/// Server → client binary message tags (`olo-rex-link screen export NAME`).
pub const MSG_EXPORT_PROGRESS: u8 = 0x30;
pub const MSG_EXPORT_META: u8 = 0x31;
pub const MSG_EXPORT_CHUNK: u8 = 0x32;
pub const MSG_EXPORT_DONE: u8 = 0x33;
pub const MSG_EXPORT_ERROR: u8 = 0x34;

/// Maximum NDJSON request line length on `screen history-stream`'s stdin (4 KiB).
pub const MAX_HISTORY_REQ_LINE: usize = 4096;

/// Maximum payload length of `MSG_HISTORY_ACTIONS` (16 MiB; `actions.jsonl` caps at 8 MiB).
pub const MAX_HISTORY_ACTIONS_PAYLOAD: u32 = 16 << 20;

/// Maximum number of action lines in a single `MSG_HISTORY_ACTIONS` message so
/// millions of tiny `{}\n` lines cannot explode heap allocations below 16 MiB.
pub const MAX_HISTORY_ACTION_LINES: usize = 200_000;

/// Canonical 64×64 tile dimension used by `olo-screen-codec`.
const TILE_SIZE: u32 = 64;

/// Maximum number of frame marks in a single `MSG_HISTORY_INDEX` message.
pub const MAX_HISTORY_INDEX_FRAMES: usize = 1_000_000;

/// Maximum payload length of `MSG_HISTORY_INDEX` (`13 + 9 * MAX_HISTORY_INDEX_FRAMES`).
pub const MAX_HISTORY_INDEX_PAYLOAD: u32 = 13 + (MAX_HISTORY_INDEX_FRAMES as u32) * 9;

/// Maximum payload length of one `MSG_EXPORT_CHUNK` (1 MiB).
pub const MAX_EXPORT_CHUNK_PAYLOAD: u32 = 1 << 20;

/// Maximum payload length of `MSG_EXPORT_PROGRESS` (4 KiB).
pub const MAX_EXPORT_PROGRESS_PAYLOAD: u32 = 4096;

/// Fixed header size of `OLOT` (`magic[4] + ver + enc + key + pad + w[4] + h[4] + rect_count[4] + data_len[4]`).
const OLOT_MIN_BYTES: usize = 24;
const OLOT_PREFIX_BYTES: usize = 20;
const OLOT_RECT_BYTES: usize = 8;
/// Chunk stride for incremental wire payload reads so a large declared frame
/// length never reserves hundreds of megabytes before bytes arrive.
const READ_CHUNK_BYTES: usize = 64 << 10;
/// Maximum number of ranges accepted in a discontiguous `--part` specification.
pub const MAX_EXPORT_PART_RANGES: usize = 256;
/// Maximum byte length of a `--part` specification string.
pub const MAX_EXPORT_PART_SPEC_LEN: usize = 4096;

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// Read exactly `len` bytes incrementally in bounded chunks into `out` (which
/// may already hold a validated prefix), growing storage only as bytes arrive.
fn read_exact_incremental_into<R: Read>(r: &mut R, len: usize, out: &mut Vec<u8>) -> io::Result<()> {
    let mut remaining = len;
    let mut scratch = [0u8; READ_CHUNK_BYTES];
    while remaining > 0 {
        let want = remaining.min(scratch.len());
        match r.read(&mut scratch[..want]) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "unexpected EOF while reading framed message")),
            Ok(n) => {
                out.extend_from_slice(&scratch[..n]);
                remaining -= n;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Read exactly `len` bytes incrementally without allocating `len` up front.
fn read_exact_incremental<R: Read>(r: &mut R, len: usize) -> io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(len.min(READ_CHUNK_BYTES));
    read_exact_incremental_into(r, len, &mut out)?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Screen ownership (`NAME.owner.json` & `$XDG_STATE_HOME/olo/screens/NAME/owner.json`)
// ---------------------------------------------------------------------------

/// A Rex pane that owns or originally started a screen.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerPane {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_key: Option<String>,
    pub session_id: String,
    pub block_id: String,
}

/// Sidecar metadata recorded beside a screen (`NAME.owner.json` in the runtime
/// folder and `owner.json` in the screen's state folder).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenOwner {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_key: Option<String>,
    pub session_id: String,
    pub block_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_by: Option<OwnerPane>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub began: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rex_socket: Option<String>,
    /// Milliseconds since the Unix epoch when ownership was last updated.
    #[serde(default)]
    pub at: u64,
    /// Session PID this sidecar was recorded for (prevents stale runtime sidecars
    /// from attributing a replacement screen started outside Rex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_pid: Option<i32>,
    /// Session start epoch seconds (`RuntimeScreen.started`) this sidecar belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_started: Option<u64>,
    /// Kernel start ticks (`/proc/PID/stat` field 22) of `screen_pid` when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_start_ticks: Option<u64>,
}

impl ScreenOwner {
    /// Current controlling pane of this owner record.
    pub fn pane(&self) -> OwnerPane {
        OwnerPane { host_key: self.host_key.clone(), session_id: self.session_id.clone(), block_id: self.block_id.clone() }
    }

    /// Whether this record already attributes the screen to `(session_id, block_id)`.
    pub fn is_same_pane(&self, session_id: &str, block_id: &str) -> bool {
        self.session_id == session_id && self.block_id == block_id
    }
}

// ---------------------------------------------------------------------------
// `olo-rex-link screen follow` NDJSON events
// ---------------------------------------------------------------------------

/// Host-wide Agent Screens installation and default status.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostScreenStatus {
    /// Version of Olo installed for screens on this host (`share/olo/version`),
    /// or `None` if Olo for screens is not installed.
    #[serde(default)]
    pub installed: Option<String>,
    /// Commit timestamp (seconds since epoch) of that Olo release, if recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed: Option<u64>,
    /// Whether `olo screen` defaults to `"host"` (`true`) or `"desktop"` (`false`).
    #[serde(default)]
    pub default_on_host: bool,
    /// Plain-words explanation when Agent Screens cannot run on this host yet
    /// (not installed, older/missing system libraries, or missing Mesa
    /// software EGL/renderer), matching `screen status`. `None` when ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_set_up_reason: Option<String>,
}

/// Lifecycle state of one screen on a host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostScreenState {
    Running,
    Stopped,
    Archived,
}

/// One screen on a host, as emitted by `olo-rex-link screen follow`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostScreen {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub state: HostScreenState,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    #[serde(default)]
    pub sound: bool,
    /// Session PID while running (`None` when stopped/archived).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    /// Seconds since the Unix epoch when started (running screens).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<u64>,
    /// Milliseconds since the Unix epoch of the latest action (`at + ms`), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<u64>,
    /// Number of recorded actions in `actions.jsonl` (`Some(0)` when empty;
    /// `None` from older host helpers, while a large history is being scanned
    /// or rescanned, or when `actions.jsonl` cannot be read).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions: Option<usize>,
    /// Whether Take Control is currently held on this screen.
    #[serde(default)]
    pub controlled: bool,
    /// Current controlling Rex pane, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<OwnerPane>,
    /// Rex pane that originally started this screen, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_by: Option<OwnerPane>,
}

/// One NDJSON line emitted by `olo-rex-link screen follow`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum FollowEvent {
    /// Initial full snapshot of host status and all known screens.
    Snapshot { proto: u32, status: HostScreenStatus, screens: Vec<HostScreen> },
    /// A screen was added or one of its fields (state, title, size, controlled,
    /// owner, …) changed.
    Screen { screen: Box<HostScreen> },
    /// Lightweight update when only a screen's `last_used` timestamp / action
    /// count advanced (debounced to at most 2/s per screen).
    LastUsed {
        name: String,
        last_used: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actions: Option<usize>,
    },
    /// A screen no longer exists (removed from both runtime and state).
    Gone { name: String },
    /// Host Olo install state, version, or default-on-host setting changed.
    Status { status: HostScreenStatus },
}

/// Write one [`FollowEvent`] as a single JSON line and flush.
pub fn write_follow_event<W: Write>(w: &mut W, event: &FollowEvent) -> io::Result<()> {
    let mut line = serde_json::to_vec(event).map_err(|e| invalid(e.to_string()))?;
    if line.len() + 1 > MAX_JSON_LINE {
        return Err(invalid(format!("follow event exceeds {MAX_JSON_LINE} bytes")));
    }
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()
}

/// Read the next [`FollowEvent`] line from `r`, enforcing [`MAX_JSON_LINE`]
/// before allocating unbounded bytes. Returns `Ok(None)` on clean EOF.
pub fn read_follow_event<R: BufRead>(r: &mut R) -> io::Result<Option<FollowEvent>> {
    let mut buf = Vec::new();
    loop {
        let avail = r.fill_buf()?;
        if avail.is_empty() {
            if buf.is_empty() {
                return Ok(None);
            }
            break;
        }
        if let Some(pos) = memchr::memchr(b'\n', avail) {
            if buf.len() + pos + 1 > MAX_JSON_LINE {
                return Err(invalid(format!("follow line exceeds {MAX_JSON_LINE} bytes")));
            }
            buf.extend_from_slice(&avail[..pos]);
            r.consume(pos + 1);
            break;
        }
        let n = avail.len();
        if buf.len() + n > MAX_JSON_LINE {
            return Err(invalid(format!("follow line exceeds {MAX_JSON_LINE} bytes")));
        }
        buf.extend_from_slice(avail);
        r.consume(n);
    }
    if buf.ends_with(b"\r") {
        buf.pop();
    }
    if buf.is_empty() {
        return Err(invalid("empty follow line"));
    }
    let event: FollowEvent = serde_json::from_slice(&buf).map_err(|e| invalid(format!("invalid follow JSON: {e}")))?;
    validate_follow_event(&event)?;
    Ok(Some(event))
}

/// Validate all screen names carried inside a decoded [`FollowEvent`] so
/// path-shaped or option-shaped names never cross the wire boundary.
pub fn validate_follow_event(event: &FollowEvent) -> io::Result<()> {
    match event {
        FollowEvent::Snapshot { screens, .. } => {
            for s in screens {
                if !valid_screen_name(&s.name) {
                    return Err(invalid(format!("invalid screen name {:?} in follow snapshot", s.name)));
                }
            }
        }
        FollowEvent::Screen { screen } => {
            if !valid_screen_name(&screen.name) {
                return Err(invalid(format!("invalid screen name {:?} in follow screen event", screen.name)));
            }
        }
        FollowEvent::LastUsed { name, .. } => {
            if !valid_screen_name(name) {
                return Err(invalid(format!("invalid screen name {:?} in follow last_used event", name)));
            }
        }
        FollowEvent::Gone { name } => {
            if !valid_screen_name(name) {
                return Err(invalid(format!("invalid screen name {:?} in follow gone event", name)));
            }
        }
        FollowEvent::Status { .. } => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// `olo-rex-link screen stream NAME` binary protocol (§3.1)
// ---------------------------------------------------------------------------

/// Validate the fixed 20-byte `OLOT` header prefix against geometry, grid, and
/// total payload bounds *before* reading or allocating the tile payload.
pub fn validate_olot_prefix(prefix: &[u8; OLOT_PREFIX_BYTES], total_olot_len: usize) -> io::Result<()> {
    if total_olot_len < OLOT_MIN_BYTES || &prefix[0..4] != OLOT_MAGIC {
        return Err(invalid("not an OLOT tile frame"));
    }
    if prefix[4] != OLOT_VERSION {
        return Err(invalid(format!("unsupported OLOT version {}", prefix[4])));
    }
    if !matches!(prefix[5], 0 | 1) {
        return Err(invalid(format!("unknown OLOT tile encoding {}", prefix[5])));
    }
    if !matches!(prefix[6], 0 | 1) {
        return Err(invalid(format!("invalid OLOT keyframe flag {}", prefix[6])));
    }
    let keyframe = prefix[6] == 1;
    let width = u32::from_le_bytes(prefix[8..12].try_into().unwrap());
    let height = u32::from_le_bytes(prefix[12..16].try_into().unwrap());
    if width == 0 || height == 0 || width > MAX_DIM || height > MAX_DIM {
        return Err(invalid(format!("invalid OLOT frame dimensions {width}x{height}")));
    }
    let pixels = (width as usize).checked_mul(height as usize).ok_or_else(|| invalid("OLOT pixel count overflow"))?;
    if pixels > MAX_FRAME_PIXELS {
        return Err(invalid(format!("OLOT pixel count {pixels} exceeds {MAX_FRAME_PIXELS}")));
    }
    let cols = width.div_ceil(TILE_SIZE) as usize;
    let rows = height.div_ceil(TILE_SIZE) as usize;
    let grid_count = cols.checked_mul(rows).ok_or_else(|| invalid("OLOT grid count overflow"))?;
    let rect_count = u32::from_le_bytes(prefix[16..20].try_into().unwrap()) as usize;
    if rect_count > MAX_TILES || rect_count > grid_count {
        return Err(invalid(format!("OLOT tile count {rect_count} exceeds grid count {grid_count}")));
    }
    if keyframe && rect_count != grid_count {
        return Err(invalid(format!("OLOT keyframe has {rect_count} tiles, expected full {grid_count} grid tiles for {width}x{height}")));
    }
    let rects_bytes = rect_count.checked_mul(OLOT_RECT_BYTES).ok_or_else(|| invalid("OLOT rect table overflow"))?;
    let rects_end = OLOT_PREFIX_BYTES.checked_add(rects_bytes).ok_or_else(|| invalid("OLOT rect offset overflow"))?;
    let data_len_end = rects_end.checked_add(4).ok_or_else(|| invalid("OLOT data len offset overflow"))?;
    if total_olot_len < data_len_end {
        return Err(invalid("truncated OLOT rect list"));
    }
    Ok(())
}

/// Stream-read an `OLOT` payload of exact length `olot_len`, validating the
/// 20-byte header first and growing the buffer incrementally as bytes arrive.
fn read_olot_streaming<R: Read>(r: &mut R, olot_len: usize) -> io::Result<Vec<u8>> {
    if olot_len < OLOT_MIN_BYTES {
        return Err(invalid("truncated OLOT tile frame"));
    }
    let mut prefix = [0u8; OLOT_PREFIX_BYTES];
    r.read_exact(&mut prefix)?;
    validate_olot_prefix(&prefix, olot_len)?;
    let mut delta = Vec::with_capacity(olot_len.min(READ_CHUNK_BYTES));
    delta.extend_from_slice(&prefix);
    read_exact_incremental_into(r, olot_len - OLOT_PREFIX_BYTES, &mut delta)?;
    validate_olot_bytes(&delta)?;
    Ok(delta)
}

/// Validate an `OLOT` binary tile-delta payload (from `olo-screen-codec`)
/// strictly against dimension, tile-count, coordinate, and length bounds.
pub fn validate_olot_bytes(bytes: &[u8]) -> io::Result<()> {
    let prefix: &[u8; OLOT_PREFIX_BYTES] = bytes.get(0..OLOT_PREFIX_BYTES).and_then(|s| s.try_into().ok()).ok_or_else(|| invalid("not an OLOT tile frame"))?;
    validate_olot_prefix(prefix, bytes.len())?;
    let width = u32::from_le_bytes(prefix[8..12].try_into().unwrap());
    let height = u32::from_le_bytes(prefix[12..16].try_into().unwrap());
    let pixels = (width as usize) * (height as usize);
    let cols = width.div_ceil(TILE_SIZE) as usize;
    let rows = height.div_ceil(TILE_SIZE) as usize;
    let grid_count = cols * rows;
    let rect_count = u32::from_le_bytes(prefix[16..20].try_into().unwrap()) as usize;
    let rects_end = OLOT_PREFIX_BYTES + rect_count * OLOT_RECT_BYTES;
    let data_len_end = rects_end + 4;
    let max_raw = pixels.checked_mul(4).ok_or_else(|| invalid("OLOT frame byte count overflow"))?;
    let mut seen = vec![0u64; grid_count.div_ceil(64)];
    let mut total_raw = 0usize;
    let mut at = 20;
    for _ in 0..rect_count {
        let x = u32::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]));
        let y = u32::from(u16::from_le_bytes([bytes[at + 2], bytes[at + 3]]));
        let w = u32::from(u16::from_le_bytes([bytes[at + 4], bytes[at + 5]]));
        let h = u32::from(u16::from_le_bytes([bytes[at + 6], bytes[at + 7]]));
        if w == 0 || h == 0 {
            return Err(invalid("zero-sized OLOT tile"));
        }
        if x >= width || y >= height || x.checked_add(w).is_none_or(|x2| x2 > width) || y.checked_add(h).is_none_or(|y2| y2 > height) {
            return Err(invalid(format!("OLOT tile ({x},{y} {w}x{h}) exceeds frame {width}x{height}")));
        }
        if !x.is_multiple_of(TILE_SIZE) || !y.is_multiple_of(TILE_SIZE) {
            return Err(invalid(format!("OLOT tile ({x},{y} {w}x{h}) is not aligned to {TILE_SIZE}px grid")));
        }
        let expect_w = TILE_SIZE.min(width - x);
        let expect_h = TILE_SIZE.min(height - y);
        if w != expect_w || h != expect_h {
            return Err(invalid(format!("OLOT tile ({x},{y} {w}x{h}) differs from canonical {expect_w}x{expect_h}")));
        }
        let idx = (y / TILE_SIZE) as usize * cols + (x / TILE_SIZE) as usize;
        let word = idx / 64;
        let bit = 1u64 << (idx % 64);
        if seen[word] & bit != 0 {
            return Err(invalid(format!("duplicate OLOT tile at ({x},{y})")));
        }
        seen[word] |= bit;
        let tile_bytes = (w as usize) * (h as usize) * 4;
        total_raw = total_raw.checked_add(tile_bytes).ok_or_else(|| invalid("OLOT tile byte sum overflow"))?;
        if total_raw > max_raw {
            return Err(invalid(format!("OLOT tile bytes {total_raw} exceed frame bytes {max_raw}")));
        }
        at += OLOT_RECT_BYTES;
    }
    let data_len = u32::from_le_bytes(bytes[rects_end..data_len_end].try_into().unwrap()) as usize;
    if bytes.len() - data_len_end != data_len {
        return Err(invalid(format!("OLOT data length {data_len} does not match remaining payload {}", bytes.len() - data_len_end)));
    }
    if rect_count == 0 && data_len != 0 {
        return Err(invalid("unexpected OLOT data with zero tiles"));
    }
    Ok(())
}

/// Status or error message on a live stream (`MSG_STATUS`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamStatus {
    pub message: String,
    #[serde(default)]
    pub reconnecting: bool,
    #[serde(default)]
    pub error: bool,
}

/// Authoritative control-lease state transition (`MSG_CONTROL_STATE`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ControlStateBody {
    held: bool,
    #[serde(default)]
    seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// Message sent from host (`olo-rex-link screen stream`) to desktop client.
///
/// `Frame` carries a monotonic `seq` followed by `olo-screen-codec`'s `OLOT`
/// binary tile-delta payload (`EncodedDelta::encode_bytes()`), validated by
/// [`validate_olot_bytes`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerMsg {
    Frame {
        seq: u64,
        delta: Vec<u8>,
    },
    Status(StreamStatus),
    ScreenGone {
        reason: String,
    },
    /// Explicit control lease grant, refusal, completed release, or host-side
    /// revocation (e.g. compositor reconnect or input overflow). `seq` echoes
    /// `ClientMsg::ControlSeq.seq` (`0` for untagged `Control` or unsolicited
    /// host revocations).
    Control {
        held: bool,
        seq: u64,
        reason: Option<String>,
    },
    /// Non-fatal capture or stream warning (e.g. transient screenshot error or
    /// oversized live frame). Never revokes control or disconnects the stream.
    Warning {
        message: String,
    },
}

/// Message sent from desktop client to host (`olo-rex-link screen stream`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientMsg {
    Ack {
        seq: u64,
    },
    /// UTF-8 JSON array of `olo_protocol::SyntheticEvent` objects.
    Input {
        json_events: String,
    },
    /// Untagged control request (equivalent to `ControlSeq { on, seq: 0 }`).
    Control {
        on: bool,
    },
    Pause {
        paused: bool,
    },
    /// Request-tagged control acquisition or release; host replies with
    /// [`ServerMsg::Control`] carrying the same `seq` once completed on disk.
    ControlSeq {
        on: bool,
        seq: u64,
    },
}

/// Encode a [`ServerMsg`] into `(msg_type, payload)` after validating bounds.
pub fn encode_server_msg(msg: &ServerMsg) -> io::Result<(u8, Vec<u8>)> {
    match msg {
        ServerMsg::Frame { seq, delta } => {
            validate_olot_bytes(delta)?;
            let payload_len = 8usize.checked_add(delta.len()).ok_or_else(|| invalid("frame payload length overflow"))?;
            if payload_len > MAX_SERVER_PAYLOAD as usize {
                return Err(invalid(format!("frame payload {payload_len} exceeds {MAX_SERVER_PAYLOAD}")));
            }
            let mut payload = Vec::with_capacity(payload_len);
            payload.extend_from_slice(&seq.to_le_bytes());
            payload.extend_from_slice(delta);
            Ok((MSG_FRAME, payload))
        }
        ServerMsg::Status(s) => {
            let payload = serde_json::to_vec(s).map_err(|e| invalid(e.to_string()))?;
            if payload.len() > MAX_STATUS_PAYLOAD as usize {
                return Err(invalid(format!("status payload {} exceeds {MAX_STATUS_PAYLOAD}", payload.len())));
            }
            Ok((MSG_STATUS, payload))
        }
        ServerMsg::ScreenGone { reason } => {
            let payload = reason.as_bytes().to_vec();
            if payload.len() > MAX_GONE_PAYLOAD as usize {
                return Err(invalid(format!("screen gone reason {} exceeds {MAX_GONE_PAYLOAD}", payload.len())));
            }
            Ok((MSG_SCREEN_GONE, payload))
        }
        ServerMsg::Control { held, seq, reason } => {
            let payload = serde_json::to_vec(&ControlStateBody { held: *held, seq: *seq, reason: reason.clone() }).map_err(|e| invalid(e.to_string()))?;
            if payload.len() > MAX_STATUS_PAYLOAD as usize {
                return Err(invalid(format!("control state payload {} exceeds {MAX_STATUS_PAYLOAD}", payload.len())));
            }
            Ok((MSG_CONTROL_STATE, payload))
        }
        ServerMsg::Warning { message } => {
            let payload = message.as_bytes().to_vec();
            if payload.len() > MAX_STATUS_PAYLOAD as usize {
                return Err(invalid(format!("warning payload {} exceeds {MAX_STATUS_PAYLOAD}", payload.len())));
            }
            Ok((MSG_WARNING, payload))
        }
    }
}

/// Decode a [`ServerMsg`] from its `(msg_type, payload)` bytes.
pub fn decode_server_msg(msg_type: u8, payload: &[u8]) -> io::Result<ServerMsg> {
    match msg_type {
        MSG_FRAME => {
            if payload.len() > MAX_SERVER_PAYLOAD as usize {
                return Err(invalid(format!("frame payload {} exceeds {MAX_SERVER_PAYLOAD}", payload.len())));
            }
            if payload.len() < 8 + OLOT_MIN_BYTES {
                return Err(invalid("truncated frame message"));
            }
            let seq = u64::from_le_bytes(payload[0..8].try_into().unwrap());
            let delta = &payload[8..];
            validate_olot_bytes(delta)?;
            Ok(ServerMsg::Frame { seq, delta: delta.to_vec() })
        }
        MSG_STATUS => {
            if payload.len() > MAX_STATUS_PAYLOAD as usize {
                return Err(invalid(format!("status payload {} exceeds {MAX_STATUS_PAYLOAD}", payload.len())));
            }
            let status: StreamStatus = serde_json::from_slice(payload).map_err(|e| invalid(format!("invalid status JSON: {e}")))?;
            Ok(ServerMsg::Status(status))
        }
        MSG_SCREEN_GONE => {
            if payload.len() > MAX_GONE_PAYLOAD as usize {
                return Err(invalid(format!("screen gone payload {} exceeds {MAX_GONE_PAYLOAD}", payload.len())));
            }
            let reason = std::str::from_utf8(payload).map_err(|e| invalid(format!("invalid screen gone UTF-8: {e}")))?.to_owned();
            Ok(ServerMsg::ScreenGone { reason })
        }
        MSG_CONTROL_STATE => {
            if payload.len() > MAX_STATUS_PAYLOAD as usize {
                return Err(invalid(format!("control state payload {} exceeds {MAX_STATUS_PAYLOAD}", payload.len())));
            }
            let body: ControlStateBody = serde_json::from_slice(payload).map_err(|e| invalid(format!("invalid control state JSON: {e}")))?;
            Ok(ServerMsg::Control { held: body.held, seq: body.seq, reason: body.reason })
        }
        MSG_WARNING => {
            if payload.len() > MAX_STATUS_PAYLOAD as usize {
                return Err(invalid(format!("warning payload {} exceeds {MAX_STATUS_PAYLOAD}", payload.len())));
            }
            let message = std::str::from_utf8(payload).map_err(|e| invalid(format!("invalid warning UTF-8: {e}")))?.to_owned();
            Ok(ServerMsg::Warning { message })
        }
        other => Err(invalid(format!("unknown server message type 0x{other:02x}"))),
    }
}

/// Write a length-prefixed [`ServerMsg`] to `w` and flush.
pub fn write_server_msg<W: Write>(w: &mut W, msg: &ServerMsg) -> io::Result<()> {
    let (tag, payload) = encode_server_msg(msg)?;
    let mut head = [0u8; 5];
    head[0] = tag;
    head[1..5].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    w.write_all(&head)?;
    w.write_all(&payload)?;
    w.flush()
}

/// Check the maximum allowed payload length for a server message tag before
/// allocating a buffer.
pub fn max_server_payload_for(msg_type: u8) -> io::Result<u32> {
    match msg_type {
        MSG_FRAME => Ok(MAX_SERVER_PAYLOAD),
        MSG_STATUS => Ok(MAX_STATUS_PAYLOAD),
        MSG_SCREEN_GONE => Ok(MAX_GONE_PAYLOAD),
        MSG_CONTROL_STATE => Ok(MAX_STATUS_PAYLOAD),
        MSG_WARNING => Ok(MAX_STATUS_PAYLOAD),
        other => Err(invalid(format!("unknown server message type 0x{other:02x}"))),
    }
}

/// Read the next length-prefixed [`ServerMsg`] from `r`.
///
/// Validates the message tag, length, and (for `MSG_FRAME`) fixed `OLOT`
/// geometry prefix *before* allocating the payload, and grows buffers
/// incrementally as bytes arrive. Returns `Ok(None)` on clean EOF.
pub fn read_server_msg<R: Read>(r: &mut R) -> io::Result<Option<ServerMsg>> {
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
        return Err(invalid(format!("server msg 0x{:02x} payload length {len} exceeds {max_len}", tag[0])));
    }
    if tag[0] == MSG_FRAME {
        let total = len as usize;
        if total < 8 + OLOT_MIN_BYTES {
            return Err(invalid("truncated frame message"));
        }
        let mut seq_bytes = [0u8; 8];
        r.read_exact(&mut seq_bytes)?;
        let seq = u64::from_le_bytes(seq_bytes);
        let delta = read_olot_streaming(r, total - 8)?;
        return Ok(Some(ServerMsg::Frame { seq, delta }));
    }
    let payload = read_exact_incremental(r, len as usize)?;
    decode_server_msg(tag[0], &payload).map(Some)
}

/// Encode a [`ClientMsg`] into `(msg_type, payload)` after validating bounds.
pub fn encode_client_msg(msg: &ClientMsg) -> io::Result<(u8, Vec<u8>)> {
    match msg {
        ClientMsg::Ack { seq } => Ok((MSG_ACK, seq.to_le_bytes().to_vec())),
        ClientMsg::Input { json_events } => {
            let bytes = json_events.as_bytes();
            if bytes.len() > MAX_CLIENT_PAYLOAD as usize {
                return Err(invalid(format!("input payload {} exceeds {MAX_CLIENT_PAYLOAD}", bytes.len())));
            }
            Ok((MSG_INPUT, bytes.to_vec()))
        }
        ClientMsg::Control { on } => Ok((MSG_CONTROL, vec![u8::from(*on)])),
        ClientMsg::Pause { paused } => Ok((MSG_PAUSE, vec![u8::from(*paused)])),
        ClientMsg::ControlSeq { on, seq } => {
            let mut buf = Vec::with_capacity(9);
            buf.push(u8::from(*on));
            buf.extend_from_slice(&seq.to_le_bytes());
            Ok((MSG_CONTROL_SEQ, buf))
        }
    }
}

/// Check the allowed payload length for a client message tag before allocating.
pub fn validate_client_header(msg_type: u8, payload_len: u32) -> io::Result<()> {
    match msg_type {
        MSG_ACK if payload_len == 8 => Ok(()),
        MSG_ACK => Err(invalid(format!("ack payload length must be 8, got {payload_len}"))),
        MSG_CONTROL if payload_len == 1 => Ok(()),
        MSG_CONTROL => Err(invalid(format!("control payload length must be 1, got {payload_len}"))),
        MSG_CONTROL_SEQ if payload_len == 9 => Ok(()),
        MSG_CONTROL_SEQ => Err(invalid(format!("control_seq payload length must be 9, got {payload_len}"))),
        MSG_PAUSE if payload_len == 1 => Ok(()),
        MSG_PAUSE => Err(invalid(format!("pause payload length must be 1, got {payload_len}"))),
        MSG_INPUT if payload_len <= MAX_CLIENT_PAYLOAD => Ok(()),
        MSG_INPUT => Err(invalid(format!("input payload length {payload_len} exceeds {MAX_CLIENT_PAYLOAD}"))),
        other => Err(invalid(format!("unknown client message type 0x{other:02x}"))),
    }
}

/// Decode a [`ClientMsg`] from `(msg_type, payload)`.
pub fn decode_client_msg(msg_type: u8, payload: &[u8]) -> io::Result<ClientMsg> {
    let len = u32::try_from(payload.len()).map_err(|_| invalid("client payload length exceeds u32"))?;
    validate_client_header(msg_type, len)?;
    match msg_type {
        MSG_ACK => Ok(ClientMsg::Ack { seq: u64::from_le_bytes(payload[0..8].try_into().unwrap()) }),
        MSG_INPUT => {
            let json_events = std::str::from_utf8(payload).map_err(|e| invalid(format!("invalid input UTF-8: {e}")))?.to_owned();
            Ok(ClientMsg::Input { json_events })
        }
        MSG_CONTROL => match payload[0] {
            0 => Ok(ClientMsg::Control { on: false }),
            1 => Ok(ClientMsg::Control { on: true }),
            v => Err(invalid(format!("invalid control bool byte {v}"))),
        },
        MSG_CONTROL_SEQ => {
            let on = match payload[0] {
                0 => false,
                1 => true,
                v => return Err(invalid(format!("invalid control bool byte {v}"))),
            };
            let seq = u64::from_le_bytes(payload[1..9].try_into().unwrap());
            Ok(ClientMsg::ControlSeq { on, seq })
        }
        MSG_PAUSE => match payload[0] {
            0 => Ok(ClientMsg::Pause { paused: false }),
            1 => Ok(ClientMsg::Pause { paused: true }),
            v => Err(invalid(format!("invalid pause bool byte {v}"))),
        },
        _ => unreachable!(),
    }
}

/// Write a length-prefixed [`ClientMsg`] to `w` and flush.
pub fn write_client_msg<W: Write>(w: &mut W, msg: &ClientMsg) -> io::Result<()> {
    let (tag, payload) = encode_client_msg(msg)?;
    let mut head = [0u8; 5];
    head[0] = tag;
    head[1..5].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    w.write_all(&head)?;
    w.write_all(&payload)?;
    w.flush()
}

/// Read the next length-prefixed [`ClientMsg`] from `r`.
///
/// Validates tag and length *before* allocating. Returns `Ok(None)` on clean EOF.
pub fn read_client_msg<R: Read>(r: &mut R) -> io::Result<Option<ClientMsg>> {
    let mut tag = [0u8; 1];
    match r.read(&mut tag)? {
        0 => return Ok(None),
        1 => {}
        _ => unreachable!(),
    }
    let mut len_bytes = [0u8; 4];
    r.read_exact(&mut len_bytes)?;
    let len = u32::from_le_bytes(len_bytes);
    validate_client_header(tag[0], len)?;
    let payload = read_exact_incremental(r, len as usize)?;
    decode_client_msg(tag[0], &payload).map(Some)
}

// ---------------------------------------------------------------------------
// `olo-rex-link screen history NAME` protocol (NDJSON stdin, binary stdout)
// ---------------------------------------------------------------------------

/// Client → host NDJSON request line on `olo-rex-link screen history NAME`'s stdin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum HistoryRequest {
    FrameAt { req: u64, at: u64 },
}

/// Write one [`HistoryRequest`] line and flush.
pub fn write_history_request<W: Write>(w: &mut W, req: &HistoryRequest) -> io::Result<()> {
    let mut line = serde_json::to_vec(req).map_err(|e| invalid(e.to_string()))?;
    if line.len() + 1 > MAX_HISTORY_REQ_LINE {
        return Err(invalid(format!("history request exceeds {MAX_HISTORY_REQ_LINE} bytes")));
    }
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()
}

/// Read one [`HistoryRequest`] line from `r`, enforcing [`MAX_HISTORY_REQ_LINE`]
/// before allocating unbounded bytes. Returns `Ok(None)` on clean EOF.
pub fn read_history_request<R: BufRead>(r: &mut R) -> io::Result<Option<HistoryRequest>> {
    let mut buf = Vec::new();
    loop {
        let avail = r.fill_buf()?;
        if avail.is_empty() {
            if buf.is_empty() {
                return Ok(None);
            }
            break;
        }
        if let Some(pos) = memchr::memchr(b'\n', avail) {
            if buf.len() + pos + 1 > MAX_HISTORY_REQ_LINE {
                return Err(invalid(format!("history request line exceeds {MAX_HISTORY_REQ_LINE} bytes")));
            }
            buf.extend_from_slice(&avail[..pos]);
            r.consume(pos + 1);
            break;
        }
        let n = avail.len();
        if buf.len() + n > MAX_HISTORY_REQ_LINE {
            return Err(invalid(format!("history request line exceeds {MAX_HISTORY_REQ_LINE} bytes")));
        }
        buf.extend_from_slice(avail);
        r.consume(n);
    }
    if buf.ends_with(b"\r") {
        buf.pop();
    }
    if buf.is_empty() {
        return Err(invalid("empty history request line"));
    }
    let req = serde_json::from_slice(&buf).map_err(|e| invalid(format!("invalid history request JSON: {e}")))?;
    Ok(Some(req))
}

/// One frame mark in a screen's recorded frame index (9 bytes on the wire).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryFrameMark {
    pub at: u64,
    pub key: bool,
}

/// Host → desktop binary message emitted by `olo-rex-link screen history NAME`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryMsg {
    /// Raw JSON lines from `actions.jsonl` (each line is one `Action` JSON object).
    /// `reset == true` replaces the viewer's action list; `false` appends.
    Actions { reset: bool, lines: Vec<String> },
    /// Recorded `.olof` frame timestamps and last recorded frame dimensions.
    /// `reset == true` replaces the viewer's `FrameIndex`; `false` appends.
    Index { reset: bool, last_size: Option<(u32, u32)>, frames: Vec<HistoryFrameMark> },
    /// Answer to `HistoryRequest::FrameAt { req, at }` with the actual recorded
    /// frame timestamp `frame_at` and an `OLOT` tile-delta payload (keyframe on
    /// first answer or dimension change, delta against last sent frame thereafter).
    Frame { req: u64, frame_at: u64, delta: Vec<u8> },
    /// Answer to `HistoryRequest::FrameAt { req, at }` when no frame exists at or
    /// before `at`.
    Miss { req: u64 },
    /// Stream-level error string.
    Error { message: String },
    /// Per-request non-fatal error for `HistoryRequest::FrameAt { req, .. }`
    /// (for example a single recorded frame exceeding remote wire dimensions);
    /// the history stream remains open and usable for other timestamps.
    FrameError { req: u64, message: String },
    /// Marks the completion of a (possibly multi-chunk) initial or reset
    /// snapshot. `actions_gen` and `index_gen` increment monotonically on every
    /// full actions/index replacement so reconnect deduplication waits for the
    /// complete snapshot and equal-timestamp recording replacements invalidate
    /// cached pixels.
    SnapshotDone { actions_gen: u64, index_gen: u64 },
}

/// Check the maximum allowed payload length for a history message tag before
/// allocating.
pub fn max_history_payload_for(msg_type: u8) -> io::Result<u32> {
    match msg_type {
        MSG_HISTORY_ACTIONS => Ok(MAX_HISTORY_ACTIONS_PAYLOAD),
        MSG_HISTORY_INDEX => Ok(MAX_HISTORY_INDEX_PAYLOAD),
        MSG_HISTORY_FRAME => Ok(MAX_SERVER_PAYLOAD),
        MSG_HISTORY_MISS => Ok(8),
        MSG_HISTORY_ERROR => Ok(MAX_STATUS_PAYLOAD),
        MSG_HISTORY_FRAME_ERROR => Ok(8 + MAX_STATUS_PAYLOAD),
        MSG_HISTORY_SNAPSHOT_DONE => Ok(16),
        other => Err(invalid(format!("unknown history message type 0x{other:02x}"))),
    }
}

/// Encode a [`HistoryMsg`] into `(msg_type, payload)` after validating bounds.
pub fn encode_history_msg(msg: &HistoryMsg) -> io::Result<(u8, Vec<u8>)> {
    match msg {
        HistoryMsg::Actions { reset, lines } => {
            if lines.len() > MAX_HISTORY_ACTION_LINES {
                return Err(invalid(format!("history actions line count {} exceeds {MAX_HISTORY_ACTION_LINES}", lines.len())));
            }
            let mut total = 1usize;
            for line in lines {
                if line.as_bytes().contains(&b'\n') || line.as_bytes().contains(&b'\r') {
                    return Err(invalid("history action line contains newline"));
                }
                total = total.checked_add(line.len() + 1).ok_or_else(|| invalid("history actions length overflow"))?;
            }
            if total > MAX_HISTORY_ACTIONS_PAYLOAD as usize {
                return Err(invalid(format!("history actions payload {total} exceeds {MAX_HISTORY_ACTIONS_PAYLOAD}")));
            }
            let mut payload = Vec::with_capacity(total);
            payload.push(u8::from(*reset));
            for line in lines {
                payload.extend_from_slice(line.as_bytes());
                payload.push(b'\n');
            }
            Ok((MSG_HISTORY_ACTIONS, payload))
        }
        HistoryMsg::Index { reset, last_size, frames } => {
            if frames.len() > MAX_HISTORY_INDEX_FRAMES {
                return Err(invalid(format!("history index count {} exceeds {MAX_HISTORY_INDEX_FRAMES}", frames.len())));
            }
            let (w, h) = match *last_size {
                None => (0u32, 0u32),
                Some((w, h)) if w > 0 && h > 0 && w <= MAX_DIM && h <= MAX_DIM => (w, h),
                Some((w, h)) => return Err(invalid(format!("invalid history index dimensions {w}x{h}"))),
            };
            let payload_len = 13 + frames.len() * 9;
            let mut payload = Vec::with_capacity(payload_len);
            payload.push(u8::from(*reset));
            payload.extend_from_slice(&w.to_le_bytes());
            payload.extend_from_slice(&h.to_le_bytes());
            payload.extend_from_slice(&(frames.len() as u32).to_le_bytes());
            for f in frames {
                payload.extend_from_slice(&f.at.to_le_bytes());
                payload.push(u8::from(f.key));
            }
            Ok((MSG_HISTORY_INDEX, payload))
        }
        HistoryMsg::Frame { req, frame_at, delta } => {
            validate_olot_bytes(delta)?;
            let payload_len = 16usize.checked_add(delta.len()).ok_or_else(|| invalid("history frame payload overflow"))?;
            if payload_len > MAX_SERVER_PAYLOAD as usize {
                return Err(invalid(format!("history frame payload {payload_len} exceeds {MAX_SERVER_PAYLOAD}")));
            }
            let mut payload = Vec::with_capacity(payload_len);
            payload.extend_from_slice(&req.to_le_bytes());
            payload.extend_from_slice(&frame_at.to_le_bytes());
            payload.extend_from_slice(delta);
            Ok((MSG_HISTORY_FRAME, payload))
        }
        HistoryMsg::Miss { req } => Ok((MSG_HISTORY_MISS, req.to_le_bytes().to_vec())),
        HistoryMsg::Error { message } => {
            let bytes = message.as_bytes();
            if bytes.len() > MAX_STATUS_PAYLOAD as usize {
                return Err(invalid(format!("history error {} exceeds {MAX_STATUS_PAYLOAD}", bytes.len())));
            }
            Ok((MSG_HISTORY_ERROR, bytes.to_vec()))
        }
        HistoryMsg::FrameError { req, message } => {
            let bytes = message.as_bytes();
            if bytes.len() > MAX_STATUS_PAYLOAD as usize {
                return Err(invalid(format!("history frame error {} exceeds {MAX_STATUS_PAYLOAD}", bytes.len())));
            }
            let mut payload = Vec::with_capacity(8 + bytes.len());
            payload.extend_from_slice(&req.to_le_bytes());
            payload.extend_from_slice(bytes);
            Ok((MSG_HISTORY_FRAME_ERROR, payload))
        }
        HistoryMsg::SnapshotDone { actions_gen, index_gen } => {
            let mut payload = Vec::with_capacity(16);
            payload.extend_from_slice(&actions_gen.to_le_bytes());
            payload.extend_from_slice(&index_gen.to_le_bytes());
            Ok((MSG_HISTORY_SNAPSHOT_DONE, payload))
        }
    }
}

/// Decode a [`HistoryMsg`] from `(msg_type, payload)`.
pub fn decode_history_msg(msg_type: u8, payload: &[u8]) -> io::Result<HistoryMsg> {
    let max_len = max_history_payload_for(msg_type)? as usize;
    if payload.len() > max_len {
        return Err(invalid(format!("history msg 0x{msg_type:02x} payload {} exceeds {max_len}", payload.len())));
    }
    match msg_type {
        MSG_HISTORY_ACTIONS => {
            if payload.is_empty() {
                return Err(invalid("empty history actions payload"));
            }
            let reset = match payload[0] {
                0 => false,
                1 => true,
                v => return Err(invalid(format!("invalid history actions reset flag {v}"))),
            };
            let body = std::str::from_utf8(&payload[1..]).map_err(|e| invalid(format!("invalid history actions UTF-8: {e}")))?;
            let mut lines = Vec::new();
            for line in body.lines() {
                if line.is_empty() {
                    continue;
                }
                if lines.len() >= MAX_HISTORY_ACTION_LINES {
                    return Err(invalid(format!("history actions line count exceeds {MAX_HISTORY_ACTION_LINES}")));
                }
                lines.push(line.to_owned());
            }
            Ok(HistoryMsg::Actions { reset, lines })
        }
        MSG_HISTORY_INDEX => {
            if payload.len() < 13 {
                return Err(invalid("truncated history index header"));
            }
            let reset = match payload[0] {
                0 => false,
                1 => true,
                v => return Err(invalid(format!("invalid history index reset flag {v}"))),
            };
            let w = u32::from_le_bytes(payload[1..5].try_into().unwrap());
            let h = u32::from_le_bytes(payload[5..9].try_into().unwrap());
            let last_size = match (w, h) {
                (0, 0) => None,
                (w, h) if w > 0 && h > 0 && w <= MAX_DIM && h <= MAX_DIM => Some((w, h)),
                _ => return Err(invalid(format!("invalid history index dimensions {w}x{h}"))),
            };
            let count = u32::from_le_bytes(payload[9..13].try_into().unwrap()) as usize;
            if count > MAX_HISTORY_INDEX_FRAMES {
                return Err(invalid(format!("history index count {count} exceeds {MAX_HISTORY_INDEX_FRAMES}")));
            }
            let expected = 13usize.checked_add(count.checked_mul(9).ok_or_else(|| invalid("history index byte count overflow"))?).ok_or_else(|| invalid("history index length overflow"))?;
            if payload.len() != expected {
                return Err(invalid(format!("history index payload {} != expected {expected}", payload.len())));
            }
            let mut frames = Vec::with_capacity(count);
            for chunk in payload[13..].as_chunks::<9>().0 {
                let at = u64::from_le_bytes(chunk[0..8].try_into().unwrap());
                let key = match chunk[8] {
                    0 => false,
                    1 => true,
                    v => return Err(invalid(format!("invalid history key flag {v}"))),
                };
                frames.push(HistoryFrameMark { at, key });
            }
            Ok(HistoryMsg::Index { reset, last_size, frames })
        }
        MSG_HISTORY_FRAME => {
            if payload.len() < 16 + OLOT_MIN_BYTES {
                return Err(invalid("truncated history frame message"));
            }
            let req = u64::from_le_bytes(payload[0..8].try_into().unwrap());
            let frame_at = u64::from_le_bytes(payload[8..16].try_into().unwrap());
            let delta = &payload[16..];
            validate_olot_bytes(delta)?;
            Ok(HistoryMsg::Frame { req, frame_at, delta: delta.to_vec() })
        }
        MSG_HISTORY_MISS => {
            if payload.len() != 8 {
                return Err(invalid(format!("history miss payload length must be 8, got {}", payload.len())));
            }
            let req = u64::from_le_bytes(payload[0..8].try_into().unwrap());
            Ok(HistoryMsg::Miss { req })
        }
        MSG_HISTORY_ERROR => {
            let message = std::str::from_utf8(payload).map_err(|e| invalid(format!("invalid history error UTF-8: {e}")))?.to_owned();
            Ok(HistoryMsg::Error { message })
        }
        MSG_HISTORY_FRAME_ERROR => {
            if payload.len() < 8 {
                return Err(invalid("truncated history frame error"));
            }
            let req = u64::from_le_bytes(payload[0..8].try_into().unwrap());
            let message = std::str::from_utf8(&payload[8..]).map_err(|e| invalid(format!("invalid history frame error UTF-8: {e}")))?.to_owned();
            Ok(HistoryMsg::FrameError { req, message })
        }
        MSG_HISTORY_SNAPSHOT_DONE => {
            if payload.len() != 16 {
                return Err(invalid(format!("history snapshot_done length must be 16, got {}", payload.len())));
            }
            let actions_gen = u64::from_le_bytes(payload[0..8].try_into().unwrap());
            let index_gen = u64::from_le_bytes(payload[8..16].try_into().unwrap());
            Ok(HistoryMsg::SnapshotDone { actions_gen, index_gen })
        }
        _ => unreachable!(),
    }
}

/// Write a length-prefixed [`HistoryMsg`] to `w` and flush.
pub fn write_history_msg<W: Write>(w: &mut W, msg: &HistoryMsg) -> io::Result<()> {
    let (tag, payload) = encode_history_msg(msg)?;
    let mut head = [0u8; 5];
    head[0] = tag;
    head[1..5].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    w.write_all(&head)?;
    w.write_all(&payload)?;
    w.flush()
}

/// Read the next length-prefixed [`HistoryMsg`] from `r`, validating tag,
/// length, and (for `MSG_HISTORY_FRAME`) fixed `OLOT` prefix *before*
/// allocating. Returns `Ok(None)` on clean EOF.
pub fn read_history_msg<R: Read>(r: &mut R) -> io::Result<Option<HistoryMsg>> {
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
        return Err(invalid(format!("history msg 0x{:02x} payload length {len} exceeds {max_len}", tag[0])));
    }
    if tag[0] == MSG_HISTORY_MISS && len != 8 {
        return Err(invalid(format!("history miss length must be 8, got {len}")));
    }
    if tag[0] == MSG_HISTORY_SNAPSHOT_DONE && len != 16 {
        return Err(invalid(format!("history snapshot_done length must be 16, got {len}")));
    }
    if tag[0] == MSG_HISTORY_FRAME {
        let total = len as usize;
        if total < 16 + OLOT_MIN_BYTES {
            return Err(invalid("truncated history frame message"));
        }
        let mut hdr = [0u8; 16];
        r.read_exact(&mut hdr)?;
        let req = u64::from_le_bytes(hdr[0..8].try_into().unwrap());
        let frame_at = u64::from_le_bytes(hdr[8..16].try_into().unwrap());
        let delta = read_olot_streaming(r, total - 16)?;
        return Ok(Some(HistoryMsg::Frame { req, frame_at, delta }));
    }
    let payload = read_exact_incremental(r, len as usize)?;
    decode_history_msg(tag[0], &payload).map(Some)
}

// ---------------------------------------------------------------------------
// `olo-rex-link screen export NAME [--part …] [--zoom Z]` binary protocol
// ---------------------------------------------------------------------------

/// Summary metadata emitted before streaming an exported MP4's bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportMeta {
    pub bytes: u64,
    pub frames: u64,
    pub encoded: u64,
    pub encoder: String,
    pub size: (u32, u32),
}

#[derive(Serialize, Deserialize)]
struct ExportProgressBody {
    done: f32,
}

/// Host → desktop binary message emitted by `olo-rex-link screen export NAME`.
#[derive(Clone, Debug, PartialEq)]
pub enum ExportMsg {
    Progress { done: f32 },
    Meta(ExportMeta),
    Chunk(Vec<u8>),
    Done,
    Error { message: String },
}

/// Check the maximum allowed payload length for an export message tag before
/// allocating.
pub fn max_export_payload_for(msg_type: u8) -> io::Result<u32> {
    match msg_type {
        MSG_EXPORT_PROGRESS => Ok(MAX_EXPORT_PROGRESS_PAYLOAD),
        MSG_EXPORT_META => Ok(MAX_STATUS_PAYLOAD),
        MSG_EXPORT_CHUNK => Ok(MAX_EXPORT_CHUNK_PAYLOAD),
        MSG_EXPORT_DONE => Ok(0),
        MSG_EXPORT_ERROR => Ok(MAX_STATUS_PAYLOAD),
        other => Err(invalid(format!("unknown export message type 0x{other:02x}"))),
    }
}

/// Encode an [`ExportMsg`] into `(msg_type, payload)` after validating bounds.
pub fn encode_export_msg(msg: &ExportMsg) -> io::Result<(u8, Vec<u8>)> {
    match msg {
        ExportMsg::Progress { done } => {
            if !done.is_finite() {
                return Err(invalid("export progress is not finite"));
            }
            let payload = serde_json::to_vec(&ExportProgressBody { done: done.clamp(0.0, 1.0) }).map_err(|e| invalid(e.to_string()))?;
            if payload.len() > MAX_EXPORT_PROGRESS_PAYLOAD as usize {
                return Err(invalid("export progress payload too large"));
            }
            Ok((MSG_EXPORT_PROGRESS, payload))
        }
        ExportMsg::Meta(meta) => {
            let payload = serde_json::to_vec(meta).map_err(|e| invalid(e.to_string()))?;
            if payload.len() > MAX_STATUS_PAYLOAD as usize {
                return Err(invalid(format!("export meta {} exceeds {MAX_STATUS_PAYLOAD}", payload.len())));
            }
            Ok((MSG_EXPORT_META, payload))
        }
        ExportMsg::Chunk(bytes) => {
            if bytes.is_empty() || bytes.len() > MAX_EXPORT_CHUNK_PAYLOAD as usize {
                return Err(invalid(format!("export chunk length {} out of 1..={MAX_EXPORT_CHUNK_PAYLOAD}", bytes.len())));
            }
            Ok((MSG_EXPORT_CHUNK, bytes.clone()))
        }
        ExportMsg::Done => Ok((MSG_EXPORT_DONE, Vec::new())),
        ExportMsg::Error { message } => {
            let bytes = message.as_bytes();
            if bytes.len() > MAX_STATUS_PAYLOAD as usize {
                return Err(invalid(format!("export error {} exceeds {MAX_STATUS_PAYLOAD}", bytes.len())));
            }
            Ok((MSG_EXPORT_ERROR, bytes.to_vec()))
        }
    }
}

/// Decode an [`ExportMsg`] from `(msg_type, payload)`.
pub fn decode_export_msg(msg_type: u8, payload: &[u8]) -> io::Result<ExportMsg> {
    let max_len = max_export_payload_for(msg_type)? as usize;
    if payload.len() > max_len {
        return Err(invalid(format!("export msg 0x{msg_type:02x} payload {} exceeds {max_len}", payload.len())));
    }
    match msg_type {
        MSG_EXPORT_PROGRESS => {
            let body: ExportProgressBody = serde_json::from_slice(payload).map_err(|e| invalid(format!("invalid export progress JSON: {e}")))?;
            if !body.done.is_finite() {
                return Err(invalid("export progress is not finite"));
            }
            Ok(ExportMsg::Progress { done: body.done })
        }
        MSG_EXPORT_META => {
            let meta: ExportMeta = serde_json::from_slice(payload).map_err(|e| invalid(format!("invalid export meta JSON: {e}")))?;
            Ok(ExportMsg::Meta(meta))
        }
        MSG_EXPORT_CHUNK => {
            if payload.is_empty() {
                return Err(invalid("empty export chunk"));
            }
            Ok(ExportMsg::Chunk(payload.to_vec()))
        }
        MSG_EXPORT_DONE => Ok(ExportMsg::Done),
        MSG_EXPORT_ERROR => {
            let message = std::str::from_utf8(payload).map_err(|e| invalid(format!("invalid export error UTF-8: {e}")))?.to_owned();
            Ok(ExportMsg::Error { message })
        }
        _ => unreachable!(),
    }
}

/// Write a length-prefixed [`ExportMsg`] to `w` and flush.
pub fn write_export_msg<W: Write>(w: &mut W, msg: &ExportMsg) -> io::Result<()> {
    let (tag, payload) = encode_export_msg(msg)?;
    let mut head = [0u8; 5];
    head[0] = tag;
    head[1..5].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    w.write_all(&head)?;
    w.write_all(&payload)?;
    w.flush()
}

/// Read the next length-prefixed [`ExportMsg`] from `r`, validating tag and
/// length *before* allocating. Returns `Ok(None)` on clean EOF.
pub fn read_export_msg<R: Read>(r: &mut R) -> io::Result<Option<ExportMsg>> {
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
        return Err(invalid(format!("export msg 0x{:02x} payload length {len} exceeds {max_len}", tag[0])));
    }
    if tag[0] == MSG_EXPORT_CHUNK && len == 0 {
        return Err(invalid("empty export chunk"));
    }
    let payload = read_exact_incremental(r, len as usize)?;
    decode_export_msg(tag[0], &payload).map(Some)
}

// ---------------------------------------------------------------------------
// QUIC `SCREEN` stream command selector (follows `TOKEN + quic_wire::SCREEN`)
// ---------------------------------------------------------------------------

/// Valid screen name for host streaming, history, export, and discovery.
/// Rejects option-shaped names (leading `-`) so a screen name can never be
/// interpreted as a flag such as `--clear` or `--help` by older host CLIs.
pub fn valid_screen_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 40 && !name.starts_with('-') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Parsed, validated arguments for `olo-rex-link screen export`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportArgs {
    pub name: String,
    pub from: Option<usize>,
    pub to: Option<usize>,
    /// Discontiguous 0-based inclusive ranges when `--part` spans more than one
    /// contiguous interval (empty when exporting all actions or a single range
    /// represented by `from`/`to`).
    pub part_ranges: Vec<(usize, usize)>,
    pub zoom: Option<usize>,
    pub encoder: Option<String>,
}

impl ExportArgs {
    /// Format `part_ranges` as a canonical 0-based `--part` spec (`"2-4,8"`).
    pub fn part_spec(&self) -> Option<String> {
        if self.part_ranges.is_empty() {
            return None;
        }
        let tokens: Vec<String> = self.part_ranges.iter().map(|&(lo, hi)| if lo == hi { lo.to_string() } else { format!("{lo}-{hi}") }).collect();
        Some(tokens.join(","))
    }
}

/// Parse `--part SPEC` (0-based comma-separated indices and/or `a-b` / `a..=b`
/// inclusive ranges) into sorted, coalesced 0-based inclusive ranges.
pub fn parse_part_ranges(spec: &str) -> Result<Vec<(usize, usize)>, String> {
    if spec.len() > MAX_EXPORT_PART_SPEC_LEN {
        return Err(format!("--part specification exceeds {MAX_EXPORT_PART_SPEC_LEN} bytes"));
    }
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for raw in spec.split(',') {
        let token = raw.trim();
        if token.is_empty() {
            return Err(format!("empty range token in --part {spec:?}"));
        }
        if ranges.len() >= MAX_EXPORT_PART_RANGES {
            return Err(format!("--part exceeds maximum {MAX_EXPORT_PART_RANGES} ranges"));
        }
        if let Some((a, b)) = token.split_once("..=").or_else(|| token.split_once('-')) {
            let a: usize = a.trim().parse().map_err(|_| format!("invalid --part range {token:?}"))?;
            let b: usize = b.trim().parse().map_err(|_| format!("invalid --part range {token:?}"))?;
            if a > b {
                return Err(format!("inverted --part range {token:?}"));
            }
            ranges.push((a, b));
        } else {
            let v: usize = token.parse().map_err(|_| format!("invalid --part index {token:?}"))?;
            ranges.push((v, v));
        }
    }
    if ranges.is_empty() {
        return Err("empty --part specification".into());
    }
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (lo, hi) in ranges {
        if let Some(last) = merged.last_mut()
            && lo <= last.1.saturating_add(1)
        {
            last.1 = last.1.max(hi);
        } else {
            merged.push((lo, hi));
        }
    }
    for &(lo, hi) in &merged {
        lo.checked_add(1).ok_or_else(|| "action index overflow in --part".to_owned())?;
        hi.checked_add(1).ok_or_else(|| "action index overflow in --part".to_owned())?;
    }
    Ok(merged)
}

/// Parse a single contiguous `--part SPEC` into 1-based `(from, to)` bounds.
pub fn parse_part_spec(spec: &str) -> Result<(usize, usize), String> {
    let merged = parse_part_ranges(spec)?;
    if merged.len() != 1 {
        return Err("discontiguous --part specification spans multiple ranges".into());
    }
    let (lo, hi) = merged[0];
    Ok((lo + 1, hi + 1))
}

/// Validate and parse CLI / QUIC arguments for `screen export`.
pub fn parse_export_args(args: &[String]) -> Result<ExportArgs, String> {
    let Some(name) = args.first() else {
        return Err("missing screen name".into());
    };
    if !valid_screen_name(name) {
        return Err(format!("invalid screen name {name:?}"));
    }
    let mut from: Option<usize> = None;
    let mut to: Option<usize> = None;
    let mut part_ranges: Vec<(usize, usize)> = Vec::new();
    let mut zoom: Option<usize> = None;
    let mut encoder: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--part" => {
                let spec = args.get(i + 1).ok_or("--part needs a range or index list")?;
                let ranges = parse_part_ranges(spec)?;
                if ranges.len() == 1 {
                    from = Some(ranges[0].0 + 1);
                    to = Some(ranges[0].1 + 1);
                    part_ranges.clear();
                } else {
                    from = None;
                    to = None;
                    part_ranges = ranges;
                }
                i += 2;
            }
            "--from" => {
                let v = args.get(i + 1).ok_or("--from needs an action number")?;
                let n: usize = v.parse().map_err(|_| "--from is an action number, from 1")?;
                if n == 0 {
                    return Err("--from is an action number, from 1".into());
                }
                from = Some(n);
                part_ranges.clear();
                i += 2;
            }
            "--to" => {
                let v = args.get(i + 1).ok_or("--to needs an action number")?;
                let n: usize = v.parse().map_err(|_| "--to is an action number, from 1")?;
                if n == 0 {
                    return Err("--to is an action number, from 1".into());
                }
                to = Some(n);
                part_ranges.clear();
                i += 2;
            }
            "--zoom" => {
                let v = args.get(i + 1).ok_or("--zoom needs 1, 2, or 3")?;
                let z: usize = v.trim_end_matches(['x', '×']).parse().map_err(|_| "--zoom is 1, 2, or 3")?;
                if !(1..=3).contains(&z) {
                    return Err("--zoom is 1, 2, or 3".into());
                }
                zoom = Some(z);
                i += 2;
            }
            "--encoder" => {
                let v = args.get(i + 1).ok_or("--encoder needs auto, cpu, nvidia, or vaapi")?;
                if !matches!(v.as_str(), "auto" | "cpu" | "nvidia" | "vaapi") {
                    return Err("--encoder is auto, cpu, nvidia, or vaapi".into());
                }
                encoder = Some(v.clone());
                i += 2;
            }
            other => return Err(format!("unexpected argument {other:?}")),
        }
    }
    if let (Some(f), Some(t)) = (from, to)
        && f > t
    {
        return Err(format!("--from {f} exceeds --to {t}"));
    }
    Ok(ExportArgs { name: name.clone(), from, to, part_ranges, zoom, encoder })
}

/// Optional generation/activity guard passed to `screen stop`, `screen delete`,
/// and `screen history NAME --clear`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExpectedScreen {
    pub expected_started: Option<u64>,
    pub expected_last_used: Option<u64>,
}

impl ExpectedScreen {
    pub fn is_guarded(self) -> bool {
        self.expected_started.is_some() || self.expected_last_used.is_some()
    }
}

/// Which guarded one-shot screen lifecycle action is being requested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardedVerb {
    Stop,
    Delete,
    ClearHistory,
}

/// Parsed and validated guarded `screen` action (`stop`, `delete`, or
/// `history NAME --clear` with `--expected-started` / `--expected-last-used`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuardedAction {
    pub verb: GuardedVerb,
    pub name: String,
    pub expected: ExpectedScreen,
}

/// True when `arg` looks like an `--expected*` guard flag (exact or malformed
/// such as `--expected-started=10` or `--expected-bogus`), so callers never
/// accidentally fall through to an unguarded destructive path (Finding 16).
pub fn is_guard_like_arg(arg: &str) -> bool {
    arg.starts_with("--expected")
}

/// Parse `--expected-started SECONDS` and `--expected-last-used MILLIS` flags
/// (at most once each, in any order), rejecting unknown or malformed flags.
pub fn parse_expected_flags(flags: &[String]) -> Result<ExpectedScreen, String> {
    let mut out = ExpectedScreen::default();
    let mut i = 0usize;
    while i < flags.len() {
        match flags[i].as_str() {
            "--expected-started" => {
                if out.expected_started.is_some() {
                    return Err("duplicate --expected-started".into());
                }
                let v = flags.get(i + 1).ok_or("--expected-started needs a session timestamp (seconds)")?;
                let n = v.parse::<u64>().map_err(|_| "--expected-started must be a non-negative integer")?;
                out.expected_started = Some(n);
                i += 2;
            }
            "--expected-last-used" => {
                if out.expected_last_used.is_some() {
                    return Err("duplicate --expected-last-used".into());
                }
                let v = flags.get(i + 1).ok_or("--expected-last-used needs an activity timestamp (milliseconds)")?;
                let n = v.parse::<u64>().map_err(|_| "--expected-last-used must be a non-negative integer")?;
                out.expected_last_used = Some(n);
                i += 2;
            }
            other => return Err(format!("unexpected argument {other:?}")),
        }
    }
    Ok(out)
}

/// Validate CLI/SSH arguments for a guarded one-shot action (`stop NAME`,
/// `delete NAME`, or `history NAME --clear` with `--expected-started` /
/// `--expected-last-used`), parsing sequentially left-to-right so missing
/// option values (e.g. `--expected-started --clear 0`) and malformed forms
/// (e.g. `--expected-started=0`) are rejected with a usage error (Finding 16).
pub fn parse_guarded_action_args(args: &[String]) -> Result<GuardedAction, String> {
    let Some(cmd) = args.first().map(String::as_str) else {
        return Err("missing screen subcommand".into());
    };
    match cmd {
        "stop" | "delete" => {
            let Some(name) = args.get(1) else {
                return Err(format!("{cmd} needs a screen's name"));
            };
            if !valid_screen_name(name) {
                return Err(format!("invalid screen name {name:?}"));
            }
            let expected = parse_expected_flags(&args[2..])?;
            let verb = if cmd == "stop" { GuardedVerb::Stop } else { GuardedVerb::Delete };
            Ok(GuardedAction { verb, name: name.clone(), expected })
        }
        "history" => {
            let Some(name) = args.get(1) else {
                return Err("history needs a screen's name".into());
            };
            if !valid_screen_name(name) {
                return Err(format!("invalid screen name {name:?}"));
            }
            let mut saw_clear = false;
            let mut expected = ExpectedScreen::default();
            let mut i = 2usize;
            while i < args.len() {
                match args[i].as_str() {
                    "--clear" => {
                        if saw_clear {
                            return Err("duplicate --clear".into());
                        }
                        saw_clear = true;
                        i += 1;
                    }
                    "--expected-started" => {
                        if expected.expected_started.is_some() {
                            return Err("duplicate --expected-started".into());
                        }
                        let v = args.get(i + 1).ok_or("--expected-started needs a session timestamp (seconds)")?;
                        let n = v.parse::<u64>().map_err(|_| "--expected-started must be a non-negative integer")?;
                        expected.expected_started = Some(n);
                        i += 2;
                    }
                    "--expected-last-used" => {
                        if expected.expected_last_used.is_some() {
                            return Err("duplicate --expected-last-used".into());
                        }
                        let v = args.get(i + 1).ok_or("--expected-last-used needs an activity timestamp (milliseconds)")?;
                        let n = v.parse::<u64>().map_err(|_| "--expected-last-used must be a non-negative integer")?;
                        expected.expected_last_used = Some(n);
                        i += 2;
                    }
                    other => return Err(format!("unexpected argument {other:?}")),
                }
            }
            if !saw_clear {
                return Err("--expected-started and --expected-last-used are only valid with `history NAME --clear`".into());
            }
            Ok(GuardedAction { verb: GuardedVerb::ClearHistory, name: name.clone(), expected })
        }
        other => Err(format!("`--expected-started` and `--expected-last-used` are not valid on `{other}`")),
    }
}

/// Parsed entry-point decision for `stop`, `delete`, or `history` CLI commands
/// (Item 9 / Finding 16): separates local help, guarded in-process actions,
/// valid unguarded forwarding, and usage errors cleanly before any delegation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleCliAction {
    /// Print local usage/help text and exit 0 (never invoke host destructive CLI).
    Help,
    /// Execute guarded mutation in-process under `NAME.lifecycle.lock`.
    Guarded(GuardedAction),
    /// Valid unguarded command: forward to installed host `olo-screen`.
    Forward,
}

/// Strictly parse and classify `stop`, `delete`, and `history` CLI arguments
/// left-to-right at the entry point (Finding 16):
/// - Handles `-h` / `--help` locally (so `delete qa --help` never deletes `qa`).
/// - Rejects unknown options and typos like `--expectd-started` (exit 2) before
///   forwarding to host `olo-screen`.
/// - Preserves documented unguarded options (`stop qa --quick`, `history qa
///   --search --expected-started`, `--look N [OUT]`, `--export`, etc.).
pub fn classify_lifecycle_cli_args(args: &[String]) -> Result<LifecycleCliAction, String> {
    let Some(cmd) = args.first().map(String::as_str) else {
        return Err("missing screen subcommand".into());
    };
    match cmd {
        "stop" | "delete" => {
            let mut name: Option<String> = None;
            let mut expected = ExpectedScreen::default();
            let mut saw_guard = false;
            let mut saw_quick = false;
            let mut i = 1usize;
            while i < args.len() {
                match args[i].as_str() {
                    "-h" | "--help" => return Ok(LifecycleCliAction::Help),
                    "--quick" if cmd == "stop" => {
                        saw_quick = true;
                        i += 1;
                    }
                    "--expected-started" => {
                        if expected.expected_started.is_some() {
                            return Err("duplicate --expected-started".into());
                        }
                        let v = args.get(i + 1).ok_or("--expected-started needs a session timestamp (seconds)")?;
                        let n = v.parse::<u64>().map_err(|_| "--expected-started must be a non-negative integer")?;
                        expected.expected_started = Some(n);
                        saw_guard = true;
                        i += 2;
                    }
                    "--expected-last-used" => {
                        if expected.expected_last_used.is_some() {
                            return Err("duplicate --expected-last-used".into());
                        }
                        let v = args.get(i + 1).ok_or("--expected-last-used needs an activity timestamp (milliseconds)")?;
                        let n = v.parse::<u64>().map_err(|_| "--expected-last-used must be a non-negative integer")?;
                        expected.expected_last_used = Some(n);
                        saw_guard = true;
                        i += 2;
                    }
                    other if other.starts_with('-') => return Err(format!("unexpected option {other:?}")),
                    positional => {
                        if name.is_some() {
                            return Err(format!("unexpected argument {positional:?}"));
                        }
                        if !valid_screen_name(positional) {
                            return Err(format!("invalid screen name {positional:?}"));
                        }
                        name = Some(positional.to_owned());
                        i += 1;
                    }
                }
            }
            let Some(name) = name else {
                return Err(format!("{cmd} needs a screen's name"));
            };
            if saw_guard {
                if saw_quick {
                    return Err("unexpected option \"--quick\"".into());
                }
                let verb = if cmd == "stop" { GuardedVerb::Stop } else { GuardedVerb::Delete };
                Ok(LifecycleCliAction::Guarded(GuardedAction { verb, name, expected }))
            } else {
                Ok(LifecycleCliAction::Forward)
            }
        }
        "history" => {
            if matches!(args.get(1).map(String::as_str), Some("-h" | "--help")) {
                return Ok(LifecycleCliAction::Help);
            }
            let Some(name) = args.get(1) else {
                return Err("history needs a screen's name".into());
            };
            if name.starts_with('-') {
                return Err(format!("unexpected option {name:?}"));
            }
            if !valid_screen_name(name) {
                return Err(format!("invalid screen name {name:?}"));
            }
            let mut saw_clear = false;
            let mut saw_guard = false;
            let mut saw_read_opt = false;
            let mut expected = ExpectedScreen::default();
            let mut i = 2usize;
            while i < args.len() {
                match args[i].as_str() {
                    "-h" | "--help" => return Ok(LifecycleCliAction::Help),
                    "--clear" => {
                        if saw_clear {
                            return Err("duplicate --clear".into());
                        }
                        saw_clear = true;
                        i += 1;
                    }
                    "--expected-started" => {
                        if expected.expected_started.is_some() {
                            return Err("duplicate --expected-started".into());
                        }
                        let v = args.get(i + 1).ok_or("--expected-started needs a session timestamp (seconds)")?;
                        let n = v.parse::<u64>().map_err(|_| "--expected-started must be a non-negative integer")?;
                        expected.expected_started = Some(n);
                        saw_guard = true;
                        i += 2;
                    }
                    "--expected-last-used" => {
                        if expected.expected_last_used.is_some() {
                            return Err("duplicate --expected-last-used".into());
                        }
                        let v = args.get(i + 1).ok_or("--expected-last-used needs an activity timestamp (milliseconds)")?;
                        let n = v.parse::<u64>().map_err(|_| "--expected-last-used must be a non-negative integer")?;
                        expected.expected_last_used = Some(n);
                        saw_guard = true;
                        i += 2;
                    }
                    opt @ ("--search" | "--kind" | "--last" | "--export" | "--part" | "--from" | "--to" | "--zoom" | "--encoder") => {
                        if args.get(i + 1).is_none() {
                            return Err(format!("{opt} needs a value"));
                        }
                        saw_read_opt = true;
                        i += 2;
                    }
                    "--look" => {
                        if args.get(i + 1).is_none() {
                            return Err("--look needs an action number".into());
                        }
                        saw_read_opt = true;
                        i += 2;
                        if args.get(i).is_some_and(|next| !next.starts_with('-')) {
                            i += 1;
                        }
                    }
                    "--json" => {
                        saw_read_opt = true;
                        i += 1;
                    }
                    other => return Err(format!("unexpected argument {other:?}")),
                }
            }
            if saw_clear && saw_read_opt {
                return Err("--clear cannot be combined with history query or export options".into());
            }
            if saw_guard && !saw_clear {
                return Err("--expected-started and --expected-last-used are only valid with `history NAME --clear`".into());
            }
            if saw_guard { Ok(LifecycleCliAction::Guarded(GuardedAction { verb: GuardedVerb::ClearHistory, name: name.clone(), expected })) } else { Ok(LifecycleCliAction::Forward) }
        }
        other => Err(format!("unexpected lifecycle command {other:?}")),
    }
}

/// Validate arguments for a QUIC `SCREEN` stream (`follow`, `stream NAME`,
/// `history NAME`, `history-stream NAME`, `export NAME [--part …] [--from N] [--to N] [--zoom Z] [--encoder E]`).
///
/// One-shot mutating actions (`stop`, `delete`, `history NAME --clear`) and
/// `--expected-started` / `--expected-last-used` guards are strictly refused on
/// QUIC `SCREEN` streams (they run over SSH exec so exit 75 and stderr reach
/// the caller).
pub fn validate_quic_screen_args(args: &[String]) -> io::Result<()> {
    if args.is_empty() || args.len() > MAX_QUIC_SCREEN_ARGS {
        return Err(invalid(format!("invalid QUIC screen arg count {}", args.len())));
    }
    for a in args {
        if a.as_bytes().contains(&0) {
            return Err(invalid("NUL byte in QUIC screen argument"));
        }
    }
    if !QUIC_SCREEN_SUBCOMMANDS.contains(&args[0].as_str()) {
        return Err(invalid(format!("disallowed QUIC screen subcommand {:?}", args[0])));
    }
    if args[1..].iter().any(|a| is_guard_like_arg(a)) {
        return Err(invalid(format!("QUIC `{}` does not accept --expected-started or --expected-last-used", args[0])));
    }
    match args[0].as_str() {
        "follow" => {
            if args.len() != 1 {
                return Err(invalid("QUIC `follow` takes no arguments"));
            }
        }
        "stream" | "history" | "history-stream" => {
            if args.len() != 2 || !valid_screen_name(&args[1]) {
                return Err(invalid(format!("QUIC `{}` expects a single valid screen name", args[0])));
            }
        }
        "export" => {
            parse_export_args(&args[1..]).map_err(invalid)?;
        }
        _ => unreachable!(),
    }
    Ok(())
}

/// Encode a QUIC `SCREEN` subcommand selector as `[u16 LE len][JSON array]`.
pub fn encode_quic_screen_args(args: &[String]) -> io::Result<Vec<u8>> {
    validate_quic_screen_args(args)?;
    let json = serde_json::to_vec(args).map_err(|e| invalid(e.to_string()))?;
    if json.len() > MAX_QUIC_SCREEN_ARGS_LEN as usize {
        return Err(invalid(format!("QUIC screen args {} bytes exceed {MAX_QUIC_SCREEN_ARGS_LEN}", json.len())));
    }
    let mut out = Vec::with_capacity(2 + json.len());
    out.extend_from_slice(&(json.len() as u16).to_le_bytes());
    out.extend_from_slice(&json);
    Ok(out)
}

/// Decode and validate the JSON body of a QUIC `SCREEN` subcommand selector.
pub fn decode_quic_screen_args(body: &[u8]) -> io::Result<Vec<String>> {
    if body.is_empty() || body.len() > MAX_QUIC_SCREEN_ARGS_LEN as usize {
        return Err(invalid(format!("invalid QUIC screen args body length {}", body.len())));
    }
    let args: Vec<String> = serde_json::from_slice(body).map_err(|e| invalid(format!("invalid QUIC screen args JSON: {e}")))?;
    validate_quic_screen_args(&args)?;
    Ok(args)
}

/// Write a QUIC `SCREEN` subcommand selector to `w`.
pub fn write_quic_screen_args<W: Write>(w: &mut W, args: &[String]) -> io::Result<()> {
    let encoded = encode_quic_screen_args(args)?;
    w.write_all(&encoded)?;
    w.flush()
}

/// Read and validate a QUIC `SCREEN` subcommand selector from `r`.
pub fn read_quic_screen_args<R: Read>(r: &mut R) -> io::Result<Vec<String>> {
    let mut len_bytes = [0u8; 2];
    r.read_exact(&mut len_bytes)?;
    let len = u16::from_le_bytes(len_bytes);
    if len == 0 || len > MAX_QUIC_SCREEN_ARGS_LEN {
        return Err(invalid(format!("QUIC screen args length {len} out of range 1..={MAX_QUIC_SCREEN_ARGS_LEN}")));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    decode_quic_screen_args(&body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sample_olot(w: u32, h: u32, keyframe: bool, encoding: u8, rects: &[(u16, u16, u16, u16)], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(OLOT_MAGIC);
        out.push(OLOT_VERSION);
        out.push(encoding);
        out.push(u8::from(keyframe));
        out.push(0);
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(&(rects.len() as u32).to_le_bytes());
        for &(x, y, rw, rh) in rects {
            out.extend_from_slice(&x.to_le_bytes());
            out.extend_from_slice(&y.to_le_bytes());
            out.extend_from_slice(&rw.to_le_bytes());
            out.extend_from_slice(&rh.to_le_bytes());
        }
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    fn sample_full_keyframe(w: u32, h: u32, encoding: u8, data: &[u8]) -> Vec<u8> {
        let mut rects = Vec::new();
        let mut y = 0u32;
        while y < h {
            let rh = TILE_SIZE.min(h - y) as u16;
            let mut x = 0u32;
            while x < w {
                let rw = TILE_SIZE.min(w - x) as u16;
                rects.push((x as u16, y as u16, rw, rh));
                x += TILE_SIZE;
            }
            y += TILE_SIZE;
        }
        sample_olot(w, h, true, encoding, &rects, data)
    }

    #[test]
    fn server_messages_round_trip() {
        let msgs = vec![
            ServerMsg::Frame { seq: 42, delta: sample_full_keyframe(160, 96, 1, &[0x28, 0xb5, 0x2f, 0xfd, 1, 2]) },
            ServerMsg::Frame { seq: 43, delta: sample_olot(1440, 900, false, 0, &[(64, 128, 64, 64), (1408, 896, 32, 4)], &[9, 8, 7]) },
            ServerMsg::Status(StreamStatus { message: "Reconnecting…".into(), reconnecting: true, error: false }),
            ServerMsg::ScreenGone { reason: "stopped".into() },
            ServerMsg::ScreenGone { reason: String::new() },
        ];
        let mut wire = Vec::new();
        for m in &msgs {
            write_server_msg(&mut wire, m).unwrap();
        }
        let mut cur = Cursor::new(wire);
        for expected in &msgs {
            assert_eq!(read_server_msg(&mut cur).unwrap().as_ref(), Some(expected));
        }
        assert_eq!(read_server_msg(&mut cur).unwrap(), None);
    }

    #[test]
    fn client_messages_round_trip() {
        let msgs = vec![
            ClientMsg::Ack { seq: 123456789 },
            ClientMsg::Input { json_events: r#"[{"type":"move","x":10.5,"y":20.0},{"type":"key","key":30,"pressed":true}]"#.into() },
            ClientMsg::Control { on: true },
            ClientMsg::Control { on: false },
            ClientMsg::Pause { paused: true },
            ClientMsg::Pause { paused: false },
        ];
        let mut wire = Vec::new();
        for m in &msgs {
            write_client_msg(&mut wire, m).unwrap();
        }
        let mut cur = Cursor::new(wire);
        for expected in &msgs {
            assert_eq!(read_client_msg(&mut cur).unwrap().as_ref(), Some(expected));
        }
        assert_eq!(read_client_msg(&mut cur).unwrap(), None);
    }

    #[test]
    fn follow_events_round_trip() {
        let screen = HostScreen {
            name: "review".into(),
            title: Some("Checkout QA".into()),
            state: HostScreenState::Running,
            width: 1440,
            height: 900,
            scale: 1.0,
            sound: false,
            pid: Some(4242),
            started: Some(1_795_716_120),
            last_used: Some(1_795_716_400_000),
            actions: Some(7),
            controlled: true,
            owner: Some(OwnerPane { host_key: None, session_id: "s-1".into(), block_id: "b-9".into() }),
            started_by: Some(OwnerPane { host_key: None, session_id: "s-1".into(), block_id: "b-1".into() }),
        };
        let events = vec![
            FollowEvent::Snapshot {
                proto: PROTO,
                status: HostScreenStatus { installed: Some("2026.11.27-a1b2c3d4".into()), committed: Some(1_795_715_000), default_on_host: true, not_set_up_reason: None },
                screens: vec![screen.clone()],
            },
            FollowEvent::Screen { screen: Box::new(screen) },
            FollowEvent::LastUsed { name: "review".into(), last_used: 1_795_716_450_000, actions: Some(8) },
            FollowEvent::Gone { name: "old".into() },
            FollowEvent::Status {
                status: HostScreenStatus { installed: None, committed: None, default_on_host: false, not_set_up_reason: Some("Olo isn't installed on this host yet (`olo screen install`).".into()) },
            },
        ];
        let mut wire = Vec::new();
        for ev in &events {
            write_follow_event(&mut wire, ev).unwrap();
        }
        let mut cur = Cursor::new(wire);
        for expected in &events {
            assert_eq!(read_follow_event(&mut cur).unwrap().as_ref(), Some(expected));
        }
        assert_eq!(read_follow_event(&mut cur).unwrap(), None);
    }

    #[test]
    fn readers_reject_oversized_or_malformed_lengths_without_allocating() {
        // Oversized server frame (4 GiB) rejected before allocation.
        let mut bad_frame = vec![MSG_FRAME];
        bad_frame.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(read_server_msg(&mut Cursor::new(bad_frame)).unwrap_err().kind(), io::ErrorKind::InvalidData);

        // Oversized status message (> 64 KiB) rejected before allocation.
        let mut bad_status = vec![MSG_STATUS];
        bad_status.extend_from_slice(&(MAX_STATUS_PAYLOAD + 1).to_le_bytes());
        assert_eq!(read_server_msg(&mut Cursor::new(bad_status)).unwrap_err().kind(), io::ErrorKind::InvalidData);

        // Unknown server tag rejected.
        let mut bad_tag = vec![0x99];
        bad_tag.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(read_server_msg(&mut Cursor::new(bad_tag)).unwrap_err().kind(), io::ErrorKind::InvalidData);

        // Oversized client input rejected before allocation.
        let mut bad_input = vec![MSG_INPUT];
        bad_input.extend_from_slice(&(MAX_CLIENT_PAYLOAD + 1).to_le_bytes());
        assert_eq!(read_client_msg(&mut Cursor::new(bad_input)).unwrap_err().kind(), io::ErrorKind::InvalidData);

        // Wrong Ack / Control lengths rejected.
        let mut bad_ack = vec![MSG_ACK];
        bad_ack.extend_from_slice(&4u32.to_le_bytes());
        assert_eq!(read_client_msg(&mut Cursor::new(bad_ack)).unwrap_err().kind(), io::ErrorKind::InvalidData);

        let mut bad_ctl = vec![MSG_CONTROL];
        bad_ctl.extend_from_slice(&1u32.to_le_bytes());
        bad_ctl.push(2);
        assert_eq!(read_client_msg(&mut Cursor::new(bad_ctl)).unwrap_err().kind(), io::ErrorKind::InvalidData);

        // Out-of-bounds OLOT tile rejected.
        let bad_tile = sample_olot(100, 100, true, 1, &[(64, 0, 64, 64)], &[]);
        let mut payload = 1u64.to_le_bytes().to_vec();
        payload.extend_from_slice(&bad_tile);
        assert_eq!(decode_server_msg(MSG_FRAME, &payload).unwrap_err().kind(), io::ErrorKind::InvalidData);

        // Claimed huge tile count in OLOT rejected.
        let mut huge_tiles = 1u64.to_le_bytes().to_vec();
        huge_tiles.extend_from_slice(OLOT_MAGIC);
        huge_tiles.extend_from_slice(&[OLOT_VERSION, 1, 1, 0]);
        huge_tiles.extend_from_slice(&100u32.to_le_bytes());
        huge_tiles.extend_from_slice(&100u32.to_le_bytes());
        huge_tiles.extend_from_slice(&u32::MAX.to_le_bytes());
        huge_tiles.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(decode_server_msg(MSG_FRAME, &huge_tiles).unwrap_err().kind(), io::ErrorKind::InvalidData);

        // Trailing garbage after OLOT data rejected.
        let mut trailing = sample_olot(100, 100, true, 1, &[(0, 0, 64, 64)], &[1, 2, 3]);
        trailing.push(0xff);
        assert_eq!(validate_olot_bytes(&trailing).unwrap_err().kind(), io::ErrorKind::InvalidData);

        // Oversized follow line rejected without reading forever.
        let big_line = vec![b' '; MAX_JSON_LINE + 10];
        assert_eq!(read_follow_event(&mut Cursor::new(big_line)).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn history_messages_and_requests_round_trip() {
        let reqs = vec![HistoryRequest::FrameAt { req: 1, at: 1_795_716_000_000 }, HistoryRequest::FrameAt { req: 99, at: 1_795_716_500_000 }];
        let mut req_wire = Vec::new();
        for r in &reqs {
            write_history_request(&mut req_wire, r).unwrap();
        }
        let mut cur = Cursor::new(req_wire);
        for expected in &reqs {
            assert_eq!(read_history_request(&mut cur).unwrap().as_ref(), Some(expected));
        }
        assert_eq!(read_history_request(&mut cur).unwrap(), None);

        let msgs = vec![
            HistoryMsg::Actions { reset: true, lines: vec![r#"{"at":1000,"ms":12,"kind":"start","args":[]}"#.into(), r#"{"at":2000,"ms":5,"kind":"click","args":["--name","Save"]}"#.into()] },
            HistoryMsg::Actions { reset: false, lines: vec![r#"{"at":3000,"ms":8,"kind":"type","args":["hi"]}"#.into()] },
            HistoryMsg::Index { reset: true, last_size: Some((1440, 900)), frames: vec![HistoryFrameMark { at: 1000, key: true }, HistoryFrameMark { at: 2000, key: false }] },
            HistoryMsg::Index { reset: false, last_size: None, frames: vec![] },
            HistoryMsg::Frame { req: 7, frame_at: 2000, delta: sample_full_keyframe(160, 96, 1, &[1, 2, 3, 4]) },
            HistoryMsg::Miss { req: 8 },
            HistoryMsg::Error { message: "bad screen".into() },
        ];
        let mut wire = Vec::new();
        for m in &msgs {
            write_history_msg(&mut wire, m).unwrap();
        }
        let mut cur = Cursor::new(wire);
        for expected in &msgs {
            assert_eq!(read_history_msg(&mut cur).unwrap().as_ref(), Some(expected));
        }
        assert_eq!(read_history_msg(&mut cur).unwrap(), None);

        // Bounds checks before allocating.
        let mut bad_idx = vec![MSG_HISTORY_INDEX];
        bad_idx.extend_from_slice(&(MAX_HISTORY_INDEX_PAYLOAD + 1).to_le_bytes());
        assert_eq!(read_history_msg(&mut Cursor::new(bad_idx)).unwrap_err().kind(), io::ErrorKind::InvalidData);

        let mut bad_miss = vec![MSG_HISTORY_MISS];
        bad_miss.extend_from_slice(&4u32.to_le_bytes());
        assert_eq!(read_history_msg(&mut Cursor::new(bad_miss)).unwrap_err().kind(), io::ErrorKind::InvalidData);

        let big_req = vec![b' '; MAX_HISTORY_REQ_LINE + 10];
        assert_eq!(read_history_request(&mut Cursor::new(big_req)).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn export_messages_round_trip() {
        let msgs = vec![
            ExportMsg::Progress { done: 0.0 },
            ExportMsg::Progress { done: 0.5 },
            ExportMsg::Meta(ExportMeta { bytes: 12_345, frames: 40, encoded: 12, encoder: "cpu".into(), size: (1440, 900) }),
            ExportMsg::Chunk(vec![0, 0, 0, 24, b'f', b't', b'y', b'p']),
            ExportMsg::Done,
            ExportMsg::Error { message: "nothing has been done".into() },
        ];
        let mut wire = Vec::new();
        for m in &msgs {
            write_export_msg(&mut wire, m).unwrap();
        }
        let mut cur = Cursor::new(wire);
        for expected in &msgs {
            assert_eq!(read_export_msg(&mut cur).unwrap().as_ref(), Some(expected));
        }
        assert_eq!(read_export_msg(&mut cur).unwrap(), None);

        // Bounds checks before allocating.
        let mut bad_chunk = vec![MSG_EXPORT_CHUNK];
        bad_chunk.extend_from_slice(&(MAX_EXPORT_CHUNK_PAYLOAD + 1).to_le_bytes());
        assert_eq!(read_export_msg(&mut Cursor::new(bad_chunk)).unwrap_err().kind(), io::ErrorKind::InvalidData);

        let mut empty_chunk = vec![MSG_EXPORT_CHUNK];
        empty_chunk.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(read_export_msg(&mut Cursor::new(empty_chunk)).unwrap_err().kind(), io::ErrorKind::InvalidData);

        let mut bad_done = vec![MSG_EXPORT_DONE];
        bad_done.extend_from_slice(&1u32.to_le_bytes());
        assert_eq!(read_export_msg(&mut Cursor::new(bad_done)).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn quic_screen_args_round_trip_and_validate() {
        for args in [
            vec!["follow".to_owned()],
            vec!["stream".to_owned(), "review".to_owned()],
            vec!["history".to_owned(), "qa".to_owned()],
            vec!["export".to_owned(), "qa".to_owned(), "--part".to_owned(), "0-3".to_owned(), "--zoom".to_owned(), "2".to_owned()],
        ] {
            let mut wire = Vec::new();
            write_quic_screen_args(&mut wire, &args).unwrap();
            let got = read_quic_screen_args(&mut Cursor::new(wire)).unwrap();
            assert_eq!(got, args);
        }
        assert!(encode_quic_screen_args(&["remove".into()]).is_err());
        assert!(encode_quic_screen_args(&[]).is_err());
        assert!(encode_quic_screen_args(&["follow".into(), "extra".into()]).is_err());
        assert!(encode_quic_screen_args(&["history".into(), "qa".into(), "--clear".into()]).is_err());
        assert!(encode_quic_screen_args(&["export".into(), "qa".into(), "--out".into(), "/tmp/x".into()]).is_err());
        let len_bad = (MAX_QUIC_SCREEN_ARGS_LEN + 1).to_le_bytes();
        assert_eq!(read_quic_screen_args(&mut Cursor::new(len_bad)).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn review_11_17_18_28_wire_validation_regressions() {
        // Finding 11: option-shaped screen names and invalid export options must be rejected before spawn.
        assert!(encode_quic_screen_args(&["export".into(), "--clear".into()]).is_err());
        assert!(encode_quic_screen_args(&["stream".into(), "--help".into()]).is_err());
        assert!(encode_quic_screen_args(&["history".into(), "-h".into()]).is_err());
        assert!(encode_quic_screen_args(&["export".into(), "qa".into(), "--zoom".into(), "99".into()]).is_err());
        assert!(encode_quic_screen_args(&["export".into(), "qa".into(), "--encoder".into(), "bogus".into()]).is_err());
        assert!(encode_quic_screen_args(&["export".into(), "qa".into(), "--part".into(), "1,5".into()]).is_ok());
        assert!(encode_quic_screen_args(&["export".into(), "qa".into(), "--part".into(), "5-2".into()]).is_err());
        assert!(encode_quic_screen_args(&["export".into(), "qa".into(), "--part".into(), "abc".into()]).is_err());

        // Finding 18: partial keyframes, empty keyframes, unaligned tiles, wrong edge sizes, and duplicates must fail.
        let partial_key = sample_olot(1440, 900, true, 1, &[(0, 0, 64, 64), (1408, 896, 32, 4)], &[1, 2, 3]);
        assert!(validate_olot_bytes(&partial_key).is_err(), "partial keyframe must be rejected");
        let empty_key = sample_olot(128, 64, true, 1, &[], &[]);
        assert!(validate_olot_bytes(&empty_key).is_err(), "empty keyframe must be rejected");
        let dup_tiles = sample_olot(128, 64, true, 1, &[(0, 0, 64, 64), (0, 0, 64, 64)], &[1, 2]);
        assert!(validate_olot_bytes(&dup_tiles).is_err(), "duplicate tiles must be rejected");
        let unaligned = sample_olot(128, 64, false, 1, &[(16, 0, 64, 64)], &[1, 2]);
        assert!(validate_olot_bytes(&unaligned).is_err(), "unaligned tile must be rejected");
        let bad_edge = sample_olot(100, 64, false, 1, &[(64, 0, 16, 64)], &[1, 2]);
        assert!(validate_olot_bytes(&bad_edge).is_err(), "wrong edge tile size must be rejected");
        let empty_delta_with_data = sample_olot(128, 64, false, 1, &[], &[1]);
        assert!(validate_olot_bytes(&empty_delta_with_data).is_err(), "empty delta with trailing data must be rejected");
        let empty_delta_clean = sample_olot(128, 64, false, 1, &[], &[]);
        assert!(validate_olot_bytes(&empty_delta_clean).is_ok(), "clean empty delta is valid");

        // Finding 17: 5K @ 2x (10240x5760) and incompressible 5120x2880 payloads (> 32 MiB) must fit wire limits.
        let big_delta = sample_olot(10_240, 5_760, false, 1, &[(0, 0, 64, 64)], &[1, 2, 3]);
        assert!(validate_olot_bytes(&big_delta).is_ok(), "5K @ 2x frame must be accepted");
        let large_payload = vec![0u8; (33 << 20) + 1];
        let mut big_frame = sample_olot(128, 64, false, 1, &[(0, 0, 64, 64)], &large_payload);
        assert!(encode_server_msg(&ServerMsg::Frame { seq: 1, delta: std::mem::take(&mut big_frame) }).is_ok(), "33 MiB frame must fit wire cap");

        // Finding 28: millions of tiny action lines under 16 MiB must still be capped by line count.
        let mut many_lines = Vec::with_capacity(1 + 250_000 * 3);
        many_lines.push(1u8); // reset = true
        for _ in 0..250_000 {
            many_lines.extend_from_slice(b"{}\n");
        }
        assert!(decode_history_msg(MSG_HISTORY_ACTIONS, &many_lines).is_err(), "250k action lines in one msg must be rejected");
    }

    /// Round 3 Req 3: Older readers lacking `not_set_up_reason` and `actions`
    /// deserialize new `FollowEvent` lines cleanly, and new readers deserialize
    /// legacy lines lacking those fields with `None`.
    #[test]
    fn round3_follow_new_fields_compatible_with_older_readers_and_helpers() {
        #[derive(Debug, PartialEq, Deserialize)]
        struct OldStatus {
            #[serde(default)]
            installed: Option<String>,
            #[serde(default)]
            committed: Option<u64>,
            #[serde(default)]
            default_on_host: bool,
        }
        #[derive(Debug, PartialEq, Deserialize)]
        struct OldScreen {
            name: String,
            state: HostScreenState,
            width: u32,
            height: u32,
            scale: f64,
            #[serde(default)]
            last_used: Option<u64>,
        }
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(tag = "t", rename_all = "snake_case")]
        enum OldEvent {
            Snapshot { proto: u32, status: OldStatus, screens: Vec<OldScreen> },
            Screen { screen: OldScreen },
            LastUsed { name: String, last_used: u64 },
            Status { status: OldStatus },
        }

        let sc = HostScreen {
            name: "qa".into(),
            title: None,
            state: HostScreenState::Stopped,
            width: 1440,
            height: 900,
            scale: 1.0,
            sound: false,
            pid: None,
            started: None,
            last_used: Some(42_000),
            actions: Some(12),
            controlled: false,
            owner: None,
            started_by: None,
        };
        let status = HostScreenStatus { installed: None, committed: None, default_on_host: false, not_set_up_reason: Some("Olo isn't installed on this host yet (`olo screen install`).".into()) };
        for ev in [
            FollowEvent::Snapshot { proto: PROTO, status: status.clone(), screens: vec![sc.clone()] },
            FollowEvent::Screen { screen: Box::new(sc) },
            FollowEvent::LastUsed { name: "qa".into(), last_used: 43_000, actions: Some(13) },
            FollowEvent::Status { status },
        ] {
            let bytes = serde_json::to_vec(&ev).unwrap();
            let _: OldEvent = serde_json::from_slice(&bytes).expect("older reader must ignore unknown fields");
        }

        // Legacy host helper JSON (no `not_set_up_reason`, no `actions`) -> new reader gets `None`.
        let old_snap = br#"{"t":"snapshot","proto":1,"status":{"installed":null,"default_on_host":false},"screens":[{"name":"qa","state":"stopped","width":800,"height":600,"scale":1.0}]}"#;
        let parsed: FollowEvent = serde_json::from_slice(old_snap).unwrap();
        let FollowEvent::Snapshot { status, screens, .. } = parsed else { panic!("expected snapshot") };
        assert_eq!(status.not_set_up_reason, None);
        assert_eq!(screens[0].actions, None);

        let old_lu = br#"{"t":"last_used","name":"qa","last_used":99000}"#;
        let parsed_lu: FollowEvent = serde_json::from_slice(old_lu).unwrap();
        assert_eq!(parsed_lu, FollowEvent::LastUsed { name: "qa".into(), last_used: 99_000, actions: None });
    }

    /// Round 3 Req 4: `parse_guarded_action_args` accepts `--expected-started` /
    /// `--expected-last-used` strictly on `stop`, `delete`, and `history NAME --clear`,
    /// while `validate_quic_screen_args` refuses them on every QUIC stream.
    #[test]
    fn round3_req4_guarded_action_and_quic_arg_validation() {
        let s = |xs: &[&str]| xs.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();

        // Valid guarded CLI/SSH actions.
        assert_eq!(
            parse_guarded_action_args(&s(&["stop", "qa", "--expected-started", "100", "--expected-last-used", "200"])).unwrap(),
            GuardedAction { verb: GuardedVerb::Stop, name: "qa".into(), expected: ExpectedScreen { expected_started: Some(100), expected_last_used: Some(200) } }
        );
        assert_eq!(
            parse_guarded_action_args(&s(&["delete", "qa", "--expected-last-used", "0"])).unwrap(),
            GuardedAction { verb: GuardedVerb::Delete, name: "qa".into(), expected: ExpectedScreen { expected_started: None, expected_last_used: Some(0) } }
        );
        assert_eq!(
            parse_guarded_action_args(&s(&["history", "qa", "--clear", "--expected-started", "0", "--expected-last-used", "555"])).unwrap(),
            GuardedAction { verb: GuardedVerb::ClearHistory, name: "qa".into(), expected: ExpectedScreen { expected_started: Some(0), expected_last_used: Some(555) } }
        );
        assert_eq!(parse_guarded_action_args(&s(&["history", "qa", "--expected-last-used", "555", "--clear"])).unwrap().verb, GuardedVerb::ClearHistory);

        // Invalid guarded CLI/SSH invocations (including Finding 16 malformed forms).
        for bad in [
            s(&["history", "qa", "--expected-last-used", "555"]),
            s(&["history", "qa", "--expected-started", "--clear", "0"]),
            s(&["history", "qa", "--clear", "--expected-last-used=555"]),
            s(&["follow", "--expected-started", "1"]),
            s(&["stream", "qa", "--expected-started", "1"]),
            s(&["export", "qa", "--expected-last-used", "1"]),
            s(&["stop", "qa", "--expected-started"]),
            s(&["stop", "qa", "--expected-started=1"]),
            s(&["stop", "qa", "--expected-started", "abc"]),
            s(&["stop", "qa", "--expected-started", "1", "--expected-started", "2"]),
            s(&["delete", "../bad", "--expected-last-used", "0"]),
            s(&["delete", "qa", "--expected-started=0"]),
            s(&["delete", "qa", "--expected-bogus", "0"]),
            s(&["delete", "qa", "--bogus"]),
        ] {
            assert!(parse_guarded_action_args(&bad).is_err(), "expected error for {bad:?}");
        }
        assert!(is_guard_like_arg("--expected-started=0"));
        assert!(is_guard_like_arg("--expected-bogus"));
        assert!(!is_guard_like_arg("--clear"));

        // QUIC `SCREEN` streams must refuse one-shot mutations and `--expected-*` everywhere.
        for bad_quic in [
            s(&["stop", "qa", "--expected-started", "1"]),
            s(&["delete", "qa", "--expected-last-used", "1"]),
            s(&["history", "qa", "--clear"]),
            s(&["history", "qa", "--clear", "--expected-last-used", "1"]),
            s(&["history", "qa", "--expected-last-used", "1"]),
            s(&["history-stream", "qa", "--expected-last-used", "1"]),
            s(&["follow", "--expected-started", "1"]),
            s(&["stream", "qa", "--expected-started", "1"]),
            s(&["export", "qa", "--expected-last-used", "1"]),
        ] {
            assert!(validate_quic_screen_args(&bad_quic).is_err(), "QUIC must refuse {bad_quic:?}");
        }

        // Item 9 / Finding 16: entry-point CLI classifier for stop/delete/history.
        assert_eq!(classify_lifecycle_cli_args(&s(&["delete", "qa", "--help"])).unwrap(), LifecycleCliAction::Help);
        assert_eq!(classify_lifecycle_cli_args(&s(&["stop", "qa", "--help"])).unwrap(), LifecycleCliAction::Help);
        assert_eq!(classify_lifecycle_cli_args(&s(&["history", "qa", "--clear", "--help"])).unwrap(), LifecycleCliAction::Help);
        assert_eq!(classify_lifecycle_cli_args(&s(&["stop", "qa", "--quick"])).unwrap(), LifecycleCliAction::Forward);
        assert_eq!(classify_lifecycle_cli_args(&s(&["stop", "--quick", "qa"])).unwrap(), LifecycleCliAction::Forward);
        assert_eq!(classify_lifecycle_cli_args(&s(&["history", "qa", "--search", "--expected-started"])).unwrap(), LifecycleCliAction::Forward);
        assert_eq!(classify_lifecycle_cli_args(&s(&["history", "qa", "--search", "--clear"])).unwrap(), LifecycleCliAction::Forward);
        assert_eq!(classify_lifecycle_cli_args(&s(&["history", "qa", "--look", "1", "/var/tmp/out.png"])).unwrap(), LifecycleCliAction::Forward);
        assert!(classify_lifecycle_cli_args(&s(&["history", "qa", "--clear", "--expectd-started", "10"])).is_err());
        assert!(classify_lifecycle_cli_args(&s(&["delete", "qa", "--expectd-started", "10"])).is_err());
        assert!(classify_lifecycle_cli_args(&s(&["stop", "qa", "--expectd-started", "10"])).is_err());
        assert!(classify_lifecycle_cli_args(&s(&["history", "qa", "--clear", "--json"])).is_err());
    }
}
