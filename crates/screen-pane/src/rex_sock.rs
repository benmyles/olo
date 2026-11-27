//! Wire types, validation, bounded framing, peer-authenticated nonblocking
//! connect, and secure atomic state-file I/O for Rex's local Agent Screens
//! service (`rex_screens_socket()` and `rex_screens_state_path()`).
//!
//! This module has **no GUI dependencies** so both `apps/screen` (the client)
//! and `apps/rex` (the server) share one exact schema, one validator, and one
//! set of size caps.
//!
//! # Transport overview
//! - **State file** (`rex_screens_state_path()`): atomic JSON snapshot
//!   ([`RexScreensState`]) written mode `0600` in a verified `0700` directory
//!   whenever hosts or screens change, watched via `inotify` (`olo_kit::watch`)
//!   with zero polling.
//! - **Unix socket** (`rex_screens_socket()`, mode `0600`, peer UID verified
//!   via `SO_PEERCRED` on BOTH server and client before the first byte is
//!   written): client writes one newline-terminated JSON [`RexSockRequest`].
//!   - For `list` and lifecycle RPCs, server replies with one JSON
//!     [`RexSockResponse`] line and closes.
//!   - For `follow`, server replies with a persistent NDJSON stream of
//!     [`RexFollowLine`] events (starting with an immediate `Snapshot`).
//!   - For `stream`, `history`, and `export`, server replies with one JSON
//!     [`RexSockResponse::Ready`] line (or `Error` and closes), then switches
//!     the socket directly into a raw bidirectional byte relay of
//!     `olo-rex-link screen stream|history-stream|export`'s own wire protocol
//!     (`olo_rex_link::screen_wire`).

use std::{
    collections::HashSet,
    ffi::CString,
    fs::File,
    io::{self, BufRead, Read, Write},
    os::{
        fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd},
        unix::{ffi::OsStrExt as _, fs::MetadataExt as _, net::UnixStream},
    },
    path::Path,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, Instant},
};

pub use olo_rex_link::screen_wire::{HostScreen, HostScreenState, HostScreenStatus, OwnerPane};
use serde::{Deserialize, Serialize};

use crate::screens::require_valid_name;

/// Current schema version for [`RexScreensState`].
pub const STATE_VERSION: u32 = 1;

/// Maximum accepted byte size of `state-<display>.json` (8 MiB).
pub const MAX_STATE_FILE_BYTES: usize = 8 * 1024 * 1024;
/// Maximum accepted byte length of one NDJSON [`RexSockRequest`] line (64 KiB).
pub const MAX_REQUEST_LINE_BYTES: usize = 64 * 1024;
/// Maximum accepted byte length of one NDJSON [`RexSockResponse`] or
/// [`RexFollowLine`] line (8 MiB).
pub const MAX_RESPONSE_LINE_BYTES: usize = 8 * 1024 * 1024;

/// Maximum remote hosts accepted in one [`RexScreensState`].
pub const MAX_HOSTS: usize = 256;
/// Maximum screens accepted for any single host in one [`RexScreensState`].
pub const MAX_SCREENS_PER_HOST: usize = 1024;
/// Maximum total screens accepted across all remote hosts combined.
pub const MAX_TOTAL_SCREENS: usize = 4096;
/// Maximum UTF-8 byte length of a host key (`RexHost::key`).
pub const MAX_HOST_KEY_LEN: usize = 128;
/// Maximum UTF-8 byte length of a host label or newly edited screen title.
pub const MAX_LABEL_LEN: usize = 256;
/// Maximum UTF-8 byte length accepted for existing stored screen titles
/// (matches `history.rs` and `crates/rex-link/src/screen_follow.rs`).
pub const MAX_TITLE_LEN: usize = 4096;
/// Maximum UTF-8 byte length of an error or disabled-reason string.
pub const MAX_ERROR_LEN: usize = 8 * 1024;
/// Maximum number of capability strings accepted per host or protocol reply.
pub const MAX_CAPABILITIES: usize = 64;
/// Maximum number of command names accepted in a protocol reply.
pub const MAX_COMMANDS: usize = 64;
/// Maximum UTF-8 byte length of a single capability or command name.
pub const MAX_CAP_LEN: usize = 128;
/// Maximum screen pixel width/height accepted on the wire (never wraps `i32`).
pub const MAX_SCREEN_DIM: u32 = 16_384;
/// Maximum arguments allowed in a [`RexSockRequest::Export`] request.
pub const MAX_EXPORT_ARGS: usize = 16;
/// Maximum byte length of a single `--part` spec inside [`RexSockRequest::Export`]
/// (shared with `olo_rex_link::screen_wire` and desktop `stream.rs`).
pub const MAX_EXPORT_PART_SPEC_LEN: usize = olo_rex_link::screen_wire::MAX_EXPORT_PART_SPEC_LEN;
/// Reserved key used by Agent Screens for `This Computer`; remote hosts may
/// never claim it.
pub const RESERVED_LOCAL_HOST_KEY: &str = "local";

fn default_scale() -> f64 {
    1.0
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// Strip ASCII and C1 control characters from untrusted display text before
/// writing to terminals or normalizing existing host titles.
pub fn sanitize_display_text(s: &str) -> String {
    s.chars().filter(|&c| !c.is_control()).collect()
}

/// Normalize an existing stored screen title at the publication boundary:
/// strip control characters and truncate cleanly on a UTF-8 boundary to
/// [`MAX_TITLE_LEN`] so pre-existing long titles never break host discovery.
pub fn normalize_stored_title(title: Option<String>) -> Option<String> {
    let raw = title?;
    let clean = sanitize_display_text(&raw);
    if clean.trim().is_empty() {
        return None;
    }
    if clean.len() <= MAX_TITLE_LEN {
        return Some(clean);
    }
    let mut end = MAX_TITLE_LEN;
    while end > 0 && !clean.is_char_boundary(end) {
        end -= 1;
    }
    let truncated = clean[..end].trim_end().to_owned();
    (!truncated.is_empty()).then_some(truncated)
}

fn has_control_char(s: &str) -> bool {
    s.chars().any(char::is_control)
}

// ---------------------------------------------------------------------------
// Published state file & host/screen snapshot types
// ---------------------------------------------------------------------------

/// Connection state of a remote host in Rex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RexHostConn {
    Connected,
    Connecting,
    Reconnecting,
    Disconnected,
    Error,
}

impl RexHostConn {
    pub fn is_connected(self) -> bool {
        matches!(self, RexHostConn::Connected)
    }
}

/// One screen on a remote Rex host, as published in [`RexScreensState`] and
/// [`RexFollowLine`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RexScreen {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub state: HostScreenState,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default = "default_scale")]
    pub scale: f64,
    #[serde(default)]
    pub sound: bool,
    /// Session PID while running (`None` when stopped/archived).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    /// Seconds since the Unix epoch when this running session started (used as
    /// the generation token for confirmed `stop`/`delete`/`clear_history`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<u64>,
    /// Milliseconds since the Unix epoch of the latest action (`at + ms`), if
    /// known (used as the generation token for confirmed `clear_history` /
    /// `delete` / `stop` requests; `Some(0)` on requests means "observed empty").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<u64>,
    /// Total recorded actions on this screen, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions: Option<usize>,
    /// Whether a person currently holds Take Control on this screen.
    #[serde(default)]
    pub controlled: bool,
    /// Current controlling Rex pane, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<OwnerPane>,
    /// Rex pane that originally started this screen, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_by: Option<OwnerPane>,
    /// Human-readable name of the controlling agent/tab (e.g. `"Lead"` or
    /// `"builder-1"`), if Rex can resolve `owner`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controlled_by: Option<String>,
}

impl RexScreen {
    pub fn display_name(&self) -> &str {
        self.title.as_deref().filter(|t| !t.trim().is_empty()).unwrap_or(&self.name)
    }

    pub fn is_running(&self) -> bool {
        self.state == HostScreenState::Running
    }
}

impl From<HostScreen> for RexScreen {
    fn from(s: HostScreen) -> Self {
        RexScreen {
            name: s.name,
            title: normalize_stored_title(s.title),
            state: s.state,
            width: s.width.min(MAX_SCREEN_DIM),
            height: s.height.min(MAX_SCREEN_DIM),
            scale: if s.scale.is_finite() && (0.25..=8.0).contains(&s.scale) { s.scale } else { 1.0 },
            sound: s.sound,
            pid: s.pid,
            started: s.started,
            last_used: s.last_used,
            actions: s.actions,
            controlled: s.controlled,
            owner: s.owner,
            started_by: s.started_by,
            controlled_by: None,
        }
    }
}

/// One remote host published by Rex in canonical `host_order`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RexHost {
    /// Stable host identifier in Rex (`HubState` key).
    pub key: String,
    /// Display name shown in sidebar headings and remote screen headers.
    pub label: String,
    /// Connection state in Rex.
    pub state: RexHostConn,
    /// Plain-language error detail when `state` is [`RexHostConn::Error`] or
    /// [`RexHostConn::Reconnecting`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Milliseconds since the Unix epoch when contact with this host was lost
    /// (populated while `state != Connected` so last-known screens can show
    /// when they were last seen).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disconnected_at: Option<u64>,
    /// Version of Olo installed for screens on this host (`share/olo/version`),
    /// or `None` when Olo for screens is not installed on this host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed: Option<String>,
    /// Plain-language reason reported by the host's helper when Olo for
    /// screens is not set up (e.g. missing packages or older libraries).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_set_up_reason: Option<String>,
    /// Commit timestamp (seconds since Unix epoch) of the installed Olo
    /// release, if recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed: Option<u64>,
    /// Where `olo screen` opens screens on this host by default: `true` = on
    /// the host itself, `false` = on the person's desktop.
    #[serde(default)]
    pub default_on_host: bool,
    /// Whether this host's installed Olo supports Take Control locking
    /// (`NAME.control.lock`). `None` when unknown or Olo is not installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_control: Option<bool>,
    /// Plain-language reason shown when `can_control == Some(false)` (e.g.
    /// `"Update Olo on this host to take control."`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_disabled_reason: Option<String>,
    /// Cached `olo-rex-link screen --protocol` capability names advertised by
    /// this host's helper (empty if not yet probed or older helper).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    /// Known screens on this host. When `state != Connected`, Rex retains the
    /// last-known screens so Agent Screens can render them with hollow rings
    /// and keep a watched screen selected until the host reconnects.
    #[serde(default)]
    pub screens: Vec<RexScreen>,
}

impl RexHost {
    /// Whether Olo for screens is installed on this host.
    pub fn has_olo_screens(&self) -> bool {
        self.installed.as_deref().is_some_and(|v| !v.trim().is_empty())
    }

    /// Whether a new screen can currently be started on this host.
    pub fn can_start_screen(&self) -> bool {
        self.state.is_connected() && self.has_olo_screens()
    }
}

/// Top-level JSON document published atomically by Rex at
/// [`crate::paths::rex_screens_state_path()`] and returned by
/// [`RexSockRequest::List`] / [`RexFollowLine::Snapshot`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RexScreensState {
    pub version: u32,
    /// PID of the Rex process publishing this file (used to detect a stale
    /// file left behind after a crash).
    #[serde(default)]
    pub pid: u32,
    /// Path to Rex's screens Unix socket (`rex_screens_socket()`).
    pub socket: String,
    /// Monotonic revision bumped on every state change published by Rex.
    #[serde(default)]
    pub rev: u64,
    /// Remote hosts in Rex's canonical `host_order`.
    #[serde(default)]
    pub hosts: Vec<RexHost>,
}

// ---------------------------------------------------------------------------
// Unix socket request / response / follow-stream types
// ---------------------------------------------------------------------------

/// One JSON line written by a client to `rex_screens_socket()`.
///
/// For `Stop`, `ClearHistory`, and `Delete`, `expected_started` (seconds since
/// epoch; `Some(0)` means "observed not started / stopped") and
/// `expected_last_used` (ms since epoch; `Some(0)` means "observed no recorded
/// activity") carry the generation of the screen the person saw when the
/// confirmation dialog opened. Rex forwards these options straight to the host
/// helper under its lifecycle lock and refuses the act if the host helper lacks
/// the matching capability (`stop-expected`, `clear-expected`,
/// `delete-expected`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum RexSockRequest {
    /// Return a single [`RexSockResponse::Snapshot`] line and close.
    List,
    /// Subscribe to a persistent NDJSON stream of [`RexFollowLine`] events
    /// (starting immediately with a full `Snapshot`).
    Follow,
    /// Query `olo-rex-link screen --protocol` for `host` (cached by Rex per
    /// host connection). Returns [`RexSockResponse::Protocol`] or `Error`.
    Protocol { host: String },
    /// Open a raw bidirectional relay to `olo-rex-link screen stream NAME`.
    /// Server answers with [`RexSockResponse::Ready`] (or `Error` and closes),
    /// then relays binary `ServerMsg`/`ClientMsg` bytes verbatim until EOF.
    Stream { host: String, name: String },
    /// Open a raw bidirectional relay to `olo-rex-link screen history-stream NAME`.
    /// Server answers with [`RexSockResponse::Ready`] (or `Error` and closes),
    /// then relays `HistoryMsg`/`HistoryRequest` bytes verbatim until EOF.
    History { host: String, name: String },
    /// Open a raw bidirectional relay to `olo-rex-link screen export NAME <args...>`.
    /// Server answers with [`RexSockResponse::Ready`] (or `Error` and closes),
    /// then relays binary `ExportMsg` bytes verbatim until EOF; client closing
    /// the write half cancels the host export cleanly.
    Export {
        host: String,
        name: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// Start a new screen (`name: None`) or restart an existing stopped /
    /// archived screen (`name: Some(n)`, `keep: true`) on `host`.
    Start {
        host: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default)]
        keep: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<(u32, u32)>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale: Option<f64>,
        #[serde(default)]
        sound: bool,
    },
    /// Stop a running screen on `host`, verifying under the host's lifecycle
    /// lock that `started` and/or `last_used` still match what the person saw.
    Stop {
        host: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_started: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_last_used: Option<u64>,
    },
    /// Clear the history of a stopped screen on `host`, verifying under the
    /// host's lifecycle lock that it is stopped and matches `expected_started`
    /// and/or `expected_last_used` (`Some(0)` = observed empty).
    ClearHistory {
        host: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_started: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_last_used: Option<u64>,
    },
    /// Set (`Some(title)`) or clear (`None`) a screen's display title on `host`.
    Rename {
        host: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// Archive (`archived: true`) or unarchive (`archived: false`) a stopped
    /// screen on `host`.
    Archive { host: String, name: String, archived: bool },
    /// Delete a stopped screen on `host` (moving its state to Trash),
    /// verifying under the host's lifecycle lock that it is stopped and
    /// matches `expected_started` and/or `expected_last_used` (`Some(0)` =
    /// observed empty).
    Delete {
        host: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_started: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_last_used: Option<u64>,
    },
    /// Focus Rex's Host window for `host` (e.g. to reconnect or install Olo
    /// for screens).
    OpenHost { host: String },
}

/// One JSON response line written by Rex on `rex_screens_socket()`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RexSockResponse {
    /// Generic success for one-shot RPCs (`stop`, `clear_history`, `rename`,
    /// `archive`, `delete`, `open_host`).
    Ok,
    /// Response to [`RexSockRequest::List`].
    Snapshot { state: RexScreensState },
    /// Response to [`RexSockRequest::Protocol`].
    Protocol {
        proto: u32,
        #[serde(default)]
        commands: Vec<String>,
        #[serde(default)]
        capabilities: Vec<String>,
    },
    /// Handshake line immediately preceding raw byte relay for `stream`,
    /// `history`, and `export`. After the terminating `\n`, the socket carries
    /// raw helper wire bytes verbatim.
    Ready {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale: Option<f64>,
    },
    /// Response to [`RexSockRequest::Start`].
    Started { screen: Box<RexScreen> },
    /// Refusal or error with plain-language words to show to the person.
    Error { message: String },
}

/// One NDJSON line pushed by Rex on a [`RexSockRequest::Follow`] connection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RexFollowLine {
    /// Complete host & screen snapshot (always sent first on `follow`, and on
    /// bulk host-order changes).
    Snapshot { state: RexScreensState },
    /// One host was added or updated (connection state, install status, or
    /// screen list changed).
    HostUpdated { rev: u64, host: Box<RexHost> },
    /// A host was removed from Rex's host list.
    HostRemoved { rev: u64, key: String },
    /// Lightweight `last_used` (and optional live `actions` count) update for
    /// one screen (avoids resending the whole host snapshot during active agent
    /// work).
    ScreenLastUsed {
        host: String,
        name: String,
        last_used: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actions: Option<usize>,
    },
    /// Non-fatal informational notice or error on the follow stream.
    Error { message: String },
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate a Rex host key (non-empty, bounded, no control chars, not
/// option-shaped, and never the reserved `"local"` key).
pub fn validate_host_key(key: &str) -> Result<(), String> {
    if key.is_empty() || key.len() > MAX_HOST_KEY_LEN {
        return Err(format!("host key length {} out of range 1..={MAX_HOST_KEY_LEN}", key.len()));
    }
    if key == RESERVED_LOCAL_HOST_KEY {
        return Err(format!("host key {key:?} is reserved for This Computer"));
    }
    if key.starts_with('-') {
        return Err(format!("host key {key:?} cannot start with '-'"));
    }
    if has_control_char(key) {
        return Err("host key contains control characters".into());
    }
    Ok(())
}

/// Validate a screen name using the shared Agent Screens naming rule.
pub fn validate_screen_name(name: &str) -> Result<(), String> {
    require_valid_name(name)
}

/// Validate a NEW screen display title being written by `Rename` (`None` clears
/// title; `Some` bounded to [`MAX_LABEL_LEN`], no control chars).
pub fn validate_title(title: Option<&str>) -> Result<(), String> {
    if let Some(t) = title {
        if t.len() > MAX_LABEL_LEN {
            return Err(format!("title exceeds {MAX_LABEL_LEN} bytes"));
        }
        if has_control_char(t) {
            return Err("title contains control characters".into());
        }
    }
    Ok(())
}

/// Validate an existing stored screen title arriving from disk or a host
/// snapshot (allows up to [`MAX_TITLE_LEN`] matching `history.rs` and
/// `screen_follow.rs`, rejecting control chars).
pub fn validate_stored_title(title: Option<&str>) -> Result<(), String> {
    if let Some(t) = title {
        if t.len() > MAX_TITLE_LEN {
            return Err(format!("stored title exceeds {MAX_TITLE_LEN} bytes"));
        }
        if has_control_char(t) {
            return Err("stored title contains control characters".into());
        }
    }
    Ok(())
}

/// Validate one [`RexScreen`] record (used for snapshots, host updates, and
/// [`RexSockResponse::Started`]).
pub fn validate_screen(s: &RexScreen) -> Result<(), String> {
    validate_screen_name(&s.name)?;
    validate_stored_title(s.title.as_deref())?;
    if s.width > MAX_SCREEN_DIM || s.height > MAX_SCREEN_DIM {
        return Err(format!("screen {:?} size {}×{} exceeds {MAX_SCREEN_DIM}", s.name, s.width, s.height));
    }
    if !s.scale.is_finite() || (s.scale != 0.0 && !(0.25..=8.0).contains(&s.scale)) {
        return Err(format!("screen {:?} scale {} out of range", s.name, s.scale));
    }
    if let Some(cb) = &s.controlled_by {
        if cb.len() > MAX_LABEL_LEN {
            return Err(format!("controlled_by exceeds {MAX_LABEL_LEN} bytes"));
        }
        if has_control_char(cb) {
            return Err("controlled_by contains control characters".into());
        }
    }
    Ok(())
}

fn validate_string_list(items: &[String], max_count: usize, label: &str) -> Result<(), String> {
    if items.len() > max_count {
        return Err(format!("{label} count {} exceeds {max_count}", items.len()));
    }
    for s in items {
        if s.is_empty() || s.len() > MAX_CAP_LEN {
            return Err(format!("{label} entry length {} out of range 1..={MAX_CAP_LEN}", s.len()));
        }
        if has_control_char(s) {
            return Err(format!("{label} entry contains control characters"));
        }
    }
    Ok(())
}

/// Validate export subcommand arguments forwarded to `olo-rex-link screen export`
/// using the helper's own `--part` range parser and checking `--from <= --to`.
pub fn validate_export_args(args: &[String]) -> Result<(), String> {
    if args.len() > MAX_EXPORT_ARGS {
        return Err(format!("export args count {} exceeds {MAX_EXPORT_ARGS}", args.len()));
    }
    let mut from: Option<usize> = None;
    let mut to: Option<usize> = None;
    let mut has_part = false;
    let mut has_zoom = false;
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--part" => {
                if has_part {
                    return Err("duplicate --part option".into());
                }
                let spec = args.get(i + 1).ok_or("--part requires a value")?;
                if spec.is_empty() || spec.len() > MAX_EXPORT_PART_SPEC_LEN {
                    return Err(format!("--part spec length {} out of range 1..={MAX_EXPORT_PART_SPEC_LEN}", spec.len()));
                }
                if !spec.bytes().all(|b| b.is_ascii_digit() || b == b'-' || b == b',') {
                    return Err(format!("invalid --part spec {spec:?}"));
                }
                let _ = olo_rex_link::screen_wire::parse_part_ranges(spec)?;
                has_part = true;
                i += 2;
            }
            "--from" => {
                if from.is_some() {
                    return Err("duplicate --from option".into());
                }
                let n = args.get(i + 1).ok_or("missing range value")?;
                let parsed: usize = n.parse().map_err(|_| format!("invalid range number {n:?}"))?;
                if parsed == 0 {
                    return Err("range index must be >= 1".into());
                }
                from = Some(parsed);
                i += 2;
            }
            "--to" => {
                if to.is_some() {
                    return Err("duplicate --to option".into());
                }
                let n = args.get(i + 1).ok_or("missing range value")?;
                let parsed: usize = n.parse().map_err(|_| format!("invalid range number {n:?}"))?;
                if parsed == 0 {
                    return Err("range index must be >= 1".into());
                }
                to = Some(parsed);
                i += 2;
            }
            "--zoom" => {
                if has_zoom {
                    return Err("duplicate --zoom option".into());
                }
                let z = args.get(i + 1).ok_or("--zoom requires a value")?;
                let parsed: usize = z.parse().map_err(|_| format!("invalid --zoom {z:?}"))?;
                if !(1..=3).contains(&parsed) {
                    return Err(format!("--zoom {parsed} out of range 1..=3"));
                }
                has_zoom = true;
                i += 2;
            }
            other => return Err(format!("unsupported export argument {other:?}")),
        }
    }
    if let (Some(f), Some(t)) = (from, to)
        && f > t
    {
        return Err(format!("invalid export range: --from {f} exceeds --to {t}"));
    }
    Ok(())
}

/// Validate a [`RexHost`] record.
pub fn validate_host(host: &RexHost) -> Result<(), String> {
    validate_host_key(&host.key)?;
    if host.label.is_empty() || host.label.len() > MAX_LABEL_LEN {
        return Err(format!("host label length {} out of range 1..={MAX_LABEL_LEN}", host.label.len()));
    }
    if has_control_char(&host.label) {
        return Err("host label contains control characters".into());
    }
    if host.screens.len() > MAX_SCREENS_PER_HOST {
        return Err(format!("host {:?} screen count {} exceeds {MAX_SCREENS_PER_HOST}", host.key, host.screens.len()));
    }
    validate_string_list(&host.capabilities, MAX_CAPABILITIES, "capabilities")?;
    if let Some(inst) = &host.installed {
        if inst.len() > MAX_LABEL_LEN {
            return Err(format!("installed version exceeds {MAX_LABEL_LEN} bytes"));
        }
        if has_control_char(inst) {
            return Err("installed version contains control characters".into());
        }
    }
    if let Some(err) = &host.error
        && err.len() > MAX_ERROR_LEN
    {
        return Err(format!("host error exceeds {MAX_ERROR_LEN} bytes"));
    }
    if let Some(reason) = &host.control_disabled_reason
        && reason.len() > MAX_ERROR_LEN
    {
        return Err(format!("control_disabled_reason exceeds {MAX_ERROR_LEN} bytes"));
    }
    if let Some(reason) = &host.not_set_up_reason
        && reason.len() > MAX_ERROR_LEN
    {
        return Err(format!("not_set_up_reason exceeds {MAX_ERROR_LEN} bytes"));
    }
    let mut seen_screens = HashSet::with_capacity(host.screens.len());
    for s in &host.screens {
        validate_screen(s)?;
        if !seen_screens.insert(s.name.as_str()) {
            return Err(format!("duplicate screen name {:?} on host {:?}", s.name, host.key));
        }
    }
    Ok(())
}

/// Validate a complete [`RexScreensState`] snapshot.
pub fn validate_state(state: &RexScreensState) -> Result<(), String> {
    if state.version != STATE_VERSION {
        return Err(format!("unsupported Rex screens state version {} (expected {STATE_VERSION})", state.version));
    }
    if state.hosts.len() > MAX_HOSTS {
        return Err(format!("host count {} exceeds {MAX_HOSTS}", state.hosts.len()));
    }
    let mut seen_hosts = HashSet::with_capacity(state.hosts.len());
    let mut total_screens = 0usize;
    for h in &state.hosts {
        validate_host(h)?;
        if !seen_hosts.insert(h.key.as_str()) {
            return Err(format!("duplicate host key {:?}", h.key));
        }
        total_screens = total_screens.saturating_add(h.screens.len());
        if total_screens > MAX_TOTAL_SCREENS {
            return Err(format!("total screen count exceeds {MAX_TOTAL_SCREENS}"));
        }
    }
    Ok(())
}

/// Validate every variant of [`RexSockResponse`] on read and write.
pub fn validate_response(resp: &RexSockResponse) -> Result<(), String> {
    match resp {
        RexSockResponse::Ok => Ok(()),
        RexSockResponse::Snapshot { state } => validate_state(state),
        RexSockResponse::Protocol { commands, capabilities, .. } => {
            validate_string_list(commands, MAX_COMMANDS, "commands")?;
            validate_string_list(capabilities, MAX_CAPABILITIES, "capabilities")
        }
        RexSockResponse::Ready { width, height, scale } => {
            if let Some(w) = width
                && *w > MAX_SCREEN_DIM
            {
                return Err(format!("ready width {w} exceeds {MAX_SCREEN_DIM}"));
            }
            if let Some(h) = height
                && *h > MAX_SCREEN_DIM
            {
                return Err(format!("ready height {h} exceeds {MAX_SCREEN_DIM}"));
            }
            if let Some(s) = scale
                && (!s.is_finite() || !(0.25..=8.0).contains(s))
            {
                return Err(format!("ready scale {s} out of range"));
            }
            Ok(())
        }
        RexSockResponse::Started { screen } => validate_screen(screen),
        RexSockResponse::Error { message } => {
            if message.len() > MAX_ERROR_LEN {
                return Err(format!("error message exceeds {MAX_ERROR_LEN} bytes"));
            }
            Ok(())
        }
    }
}

/// Validate a [`RexSockRequest`] before transmitting or executing it.
pub fn validate_request(req: &RexSockRequest) -> Result<(), String> {
    match req {
        RexSockRequest::List | RexSockRequest::Follow => Ok(()),
        RexSockRequest::Protocol { host } | RexSockRequest::OpenHost { host } => validate_host_key(host),
        RexSockRequest::Stream { host, name } | RexSockRequest::History { host, name } => {
            validate_host_key(host)?;
            validate_screen_name(name)
        }
        RexSockRequest::Export { host, name, args } => {
            validate_host_key(host)?;
            validate_screen_name(name)?;
            validate_export_args(args)
        }
        RexSockRequest::Start { host, name, size, scale, .. } => {
            validate_host_key(host)?;
            if let Some(n) = name {
                validate_screen_name(n)?;
            }
            if let Some((w, h)) = size
                && (*w == 0 || *h == 0 || *w > MAX_SCREEN_DIM || *h > MAX_SCREEN_DIM)
            {
                return Err(format!("screen size {w}×{h} out of range"));
            }
            if let Some(s) = scale
                && (!s.is_finite() || *s < 0.5 || *s > 4.0)
            {
                return Err(format!("screen scale {s} out of range"));
            }
            Ok(())
        }
        RexSockRequest::Stop { host, name, .. } | RexSockRequest::ClearHistory { host, name, .. } | RexSockRequest::Archive { host, name, .. } | RexSockRequest::Delete { host, name, .. } => {
            validate_host_key(host)?;
            validate_screen_name(name)
        }
        RexSockRequest::Rename { host, name, title } => {
            validate_host_key(host)?;
            validate_screen_name(name)?;
            validate_title(title.as_deref())
        }
    }
}

/// Validate a decoded [`RexFollowLine`].
pub fn validate_follow_line(line: &RexFollowLine) -> Result<(), String> {
    match line {
        RexFollowLine::Snapshot { state } => validate_state(state),
        RexFollowLine::HostUpdated { host, .. } => validate_host(host),
        RexFollowLine::HostRemoved { key, .. } => validate_host_key(key),
        RexFollowLine::ScreenLastUsed { host, name, .. } => {
            validate_host_key(host)?;
            validate_screen_name(name)
        }
        RexFollowLine::Error { message } => {
            if message.len() > MAX_ERROR_LEN {
                return Err(format!("error message exceeds {MAX_ERROR_LEN} bytes"));
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Bounded NDJSON readers & writers
// ---------------------------------------------------------------------------

/// `Write` sink into a `Vec<u8>` that aborts serialization immediately once
/// `max_bytes` would be exceeded (prevents unbounded pre-allocation on encode).
struct BoundedVecWriter {
    buf: Vec<u8>,
    max: usize,
    label: &'static str,
}

impl BoundedVecWriter {
    fn new(max: usize, label: &'static str) -> Self {
        BoundedVecWriter { buf: Vec::with_capacity(max.min(4096)), max, label }
    }
}

impl Write for BoundedVecWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.buf.len().saturating_add(data.len()) > self.max {
            return Err(invalid(format!("{} exceeds {} bytes", self.label, self.max)));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Serialize `value` into a single bounded `\n`-terminated JSON line without
/// ever allocating past `max_bytes`.
pub fn encode_bounded_json_line<T: Serialize>(value: &T, max_bytes: usize, label: &'static str) -> io::Result<Vec<u8>> {
    // Reserve 1 byte for the trailing '\n'.
    let mut sink = BoundedVecWriter::new(max_bytes.saturating_sub(1), label);
    serde_json::to_writer(&mut sink, value).map_err(|e| invalid(format!("encode {label}: {e}")))?;
    sink.buf.push(b'\n');
    Ok(sink.buf)
}

/// Read one `\n`-terminated line from `r` without ever buffering more than
/// `max_bytes` bytes (prevents unbounded allocations on malformed peers).
/// Returns `Ok(None)` on clean EOF at a line boundary.
pub fn read_bounded_line<R: BufRead>(r: &mut R, max_bytes: usize) -> io::Result<Option<Vec<u8>>> {
    let mut out = Vec::new();
    loop {
        let avail = match r.fill_buf() {
            Ok([]) => {
                return if out.is_empty() { Ok(None) } else { Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated JSON line before newline")) };
            }
            Ok(buf) => buf,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if let Some(pos) = avail.iter().position(|&b| b == b'\n') {
            if out.len() + pos + 1 > max_bytes {
                r.consume(pos + 1);
                return Err(invalid(format!("JSON line exceeds {max_bytes} bytes")));
            }
            out.extend_from_slice(&avail[..pos]);
            r.consume(pos + 1);
            if out.last() == Some(&b'\r') {
                out.pop();
            }
            return Ok(Some(out));
        }
        let n = avail.len();
        if out.len() + n > max_bytes {
            r.consume(n);
            return Err(invalid(format!("JSON line exceeds {max_bytes} bytes")));
        }
        out.extend_from_slice(avail);
        r.consume(n);
    }
}

fn write_json_line<W: Write, T: Serialize>(w: &mut W, value: &T, max_bytes: usize, label: &'static str) -> io::Result<()> {
    let bytes = encode_bounded_json_line(value, max_bytes, label)?;
    w.write_all(&bytes)?;
    w.flush()
}

pub fn write_request<W: Write>(w: &mut W, req: &RexSockRequest) -> io::Result<()> {
    validate_request(req).map_err(invalid)?;
    write_json_line(w, req, MAX_REQUEST_LINE_BYTES, "RexSockRequest")
}

pub fn read_request<R: BufRead>(r: &mut R) -> io::Result<Option<RexSockRequest>> {
    let Some(line) = read_bounded_line(r, MAX_REQUEST_LINE_BYTES)? else { return Ok(None) };
    let req: RexSockRequest = serde_json::from_slice(&line).map_err(|e| invalid(format!("invalid RexSockRequest JSON: {e}")))?;
    validate_request(&req).map_err(invalid)?;
    Ok(Some(req))
}

pub fn write_response<W: Write>(w: &mut W, resp: &RexSockResponse) -> io::Result<()> {
    validate_response(resp).map_err(invalid)?;
    write_json_line(w, resp, MAX_RESPONSE_LINE_BYTES, "RexSockResponse")
}

pub fn read_response<R: BufRead>(r: &mut R) -> io::Result<Option<RexSockResponse>> {
    let Some(line) = read_bounded_line(r, MAX_RESPONSE_LINE_BYTES)? else { return Ok(None) };
    let resp: RexSockResponse = serde_json::from_slice(&line).map_err(|e| invalid(format!("invalid RexSockResponse JSON: {e}")))?;
    validate_response(&resp).map_err(invalid)?;
    Ok(Some(resp))
}

pub fn write_follow_line<W: Write>(w: &mut W, line: &RexFollowLine) -> io::Result<()> {
    validate_follow_line(line).map_err(invalid)?;
    write_json_line(w, line, MAX_RESPONSE_LINE_BYTES, "RexFollowLine")
}

pub fn read_follow_line<R: BufRead>(r: &mut R) -> io::Result<Option<RexFollowLine>> {
    let Some(raw) = read_bounded_line(r, MAX_RESPONSE_LINE_BYTES)? else { return Ok(None) };
    let line: RexFollowLine = serde_json::from_slice(&raw).map_err(|e| invalid(format!("invalid RexFollowLine JSON: {e}")))?;
    validate_follow_line(&line).map_err(invalid)?;
    Ok(Some(line))
}

// ---------------------------------------------------------------------------
// Secure private directory & atomic state file I/O
// ---------------------------------------------------------------------------

static TMP_SEQ: AtomicU64 = AtomicU64::new(1);

fn c_path(p: &Path) -> io::Result<CString> {
    CString::new(p.as_os_str().as_bytes()).map_err(|_| invalid("path contains interior NUL"))
}

fn c_name(name: &str) -> io::Result<CString> {
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return Err(invalid(format!("invalid file name {name:?}")));
    }
    CString::new(name.as_bytes()).map_err(|_| invalid("file name contains interior NUL"))
}

fn my_euid() -> u32 {
    // SAFETY: `geteuid` is a pure read of the process's effective UID.
    unsafe { libc::geteuid() }
}

/// Open `dir` without following symlinks on the leaf component, verifying it is
/// a directory owned by `geteuid()`. When `ensure_private` is true, creates
/// missing leaf directory `0700` (after ensuring existing ancestor dirs) and
/// tightens permissions via `fchmod(fd, 0o700)` on the opened descriptor
/// (never calling `chmod(path)` on a symlink). When `ensure_private` is false,
/// rejects group/world-writable directories (`mode & 0o022 != 0`).
pub fn open_private_dir(dir: &Path, ensure_private: bool) -> io::Result<OwnedFd> {
    if ensure_private {
        // Ensure ancestor exists first, then create the leaf `0700` without
        // ever calling `chmod` on a path that could be a symlink.
        if let Some(ancestor) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(ancestor)?;
        }
        let cdir = c_path(dir)?;
        // SAFETY: valid NUL-terminated path.
        let rc = unsafe { libc::mkdir(cdir.as_ptr(), 0o700) };
        if rc != 0 {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::AlreadyExists {
                return Err(e);
            }
        }
    }
    let cdir = c_path(dir)?;
    // SAFETY: `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC` rejects symlinks and non-dirs atomically.
    let fd = unsafe { libc::open(cdir.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if fd < 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::ELOOP) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "refusing symlinked Rex screens directory"));
        }
        return Err(e);
    }
    // SAFETY: `fd >= 0` is a freshly opened directory descriptor.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: valid fd and writable stat buffer.
    if unsafe { libc::fstat(owned.as_raw_fd(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if (st.st_mode & libc::S_IFMT) != libc::S_IFDIR {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "Rex screens path is not a directory"));
    }
    let euid = my_euid();
    if st.st_uid != euid {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("Rex screens directory owned by uid {}, expected {euid}", st.st_uid)));
    }
    let perms = st.st_mode & 0o777;
    if ensure_private {
        if (perms & 0o077) != 0 {
            // Tighten via `fchmod` on the verified fd (never via path).
            // SAFETY: valid owned directory fd.
            if unsafe { libc::fchmod(owned.as_raw_fd(), 0o700) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    } else if (perms & 0o022) != 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("Rex screens directory mode {perms:04o} is group/world writable")));
    }
    Ok(owned)
}

/// Write `state` atomically to `path` via a verified `0700` directory fd
/// (`openat` exclusive `0600` sibling temp file + `renameat`). Refuses
/// symlinked service directories and propagates all permission errors.
pub fn write_state_atomic(path: &Path, state: &RexScreensState) -> io::Result<()> {
    validate_state(state).map_err(invalid)?;
    let mut sink = BoundedVecWriter::new(MAX_STATE_FILE_BYTES, "RexScreensState");
    serde_json::to_writer_pretty(&mut sink, state).map_err(|e| invalid(format!("encode RexScreensState: {e}")))?;
    let bytes = sink.buf;

    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| invalid("invalid state file path"))?;
    let cdst = c_name(file_name)?;
    let dir_fd = open_private_dir(parent, true)?;

    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_name = format!(".{file_name}.tmp.{}.{seq}", std::process::id());
    let ctmp = c_name(&tmp_name)?;
    // SAFETY: `dir_fd` is a verified `0700` directory fd owned by `euid`.
    let fd = unsafe { libc::openat(dir_fd.as_raw_fd(), ctmp.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o600) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd >= 0` is freshly opened.
    let mut file = unsafe { File::from_raw_fd(fd) };
    // Ensure exact `0600` mode regardless of process umask.
    if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } != 0 {
        let e = io::Error::last_os_error();
        drop(file);
        unsafe {
            libc::unlinkat(dir_fd.as_raw_fd(), ctmp.as_ptr(), 0);
        }
        return Err(e);
    }
    if let Err(e) = file.write_all(&bytes).and_then(|()| file.flush()) {
        drop(file);
        unsafe {
            libc::unlinkat(dir_fd.as_raw_fd(), ctmp.as_ptr(), 0);
        }
        return Err(e);
    }
    drop(file);
    // SAFETY: `dir_fd`, `ctmp`, and `cdst` are valid within the same directory.
    if unsafe { libc::renameat(dir_fd.as_raw_fd(), ctmp.as_ptr(), dir_fd.as_raw_fd(), cdst.as_ptr()) } != 0 {
        let e = io::Error::last_os_error();
        unsafe {
            libc::unlinkat(dir_fd.as_raw_fd(), ctmp.as_ptr(), 0);
        }
        return Err(e);
    }
    Ok(())
}

/// Read and validate `state-<display>.json` safely (refusing symlinked parent
/// dirs or files, wrong UID, group/world-writable dirs or files, FIFOs, and
/// files exceeding [`MAX_STATE_FILE_BYTES`]).
pub fn read_state_file(path: &Path) -> io::Result<RexScreensState> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| invalid("invalid state file path"))?;
    let cfile = c_name(file_name)?;
    let dir_fd = open_private_dir(parent, false)?;
    // SAFETY: `dir_fd` is verified non-symlink, owned by `euid`, non-group/world-writable.
    let fd = unsafe { libc::openat(dir_fd.as_raw_fd(), cfile.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd >= 0` is freshly opened.
    let file = unsafe { File::from_raw_fd(fd) };
    let meta = file.metadata()?;
    if !meta.file_type().is_file() {
        return Err(invalid("Rex screens state path is not a regular file"));
    }
    let my_uid = my_euid();
    if meta.uid() != my_uid {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "Rex screens state file owned by another UID"));
    }
    let mode = meta.mode() & 0o777;
    if (mode & 0o022) != 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("Rex screens state file mode {mode:04o} is group/world writable")));
    }
    if meta.len() > MAX_STATE_FILE_BYTES as u64 {
        return Err(invalid(format!("Rex screens state file ({} bytes) exceeds {MAX_STATE_FILE_BYTES}", meta.len())));
    }
    let mut buf = Vec::with_capacity((meta.len() as usize).min(64 * 1024));
    file.take(MAX_STATE_FILE_BYTES as u64 + 1).read_to_end(&mut buf)?;
    if buf.len() > MAX_STATE_FILE_BYTES {
        return Err(invalid(format!("Rex screens state file exceeds {MAX_STATE_FILE_BYTES} bytes")));
    }
    let state: RexScreensState = serde_json::from_slice(&buf).map_err(|e| invalid(format!("invalid RexScreensState JSON: {e}")))?;
    validate_state(&state).map_err(invalid)?;
    Ok(state)
}

// ---------------------------------------------------------------------------
// Peer-authenticated nonblocking connect, deadline I/O, and liveness
// ---------------------------------------------------------------------------

/// Verify via `SO_PEERCRED` that the connected Unix socket peer has the same
/// effective UID as this process.
pub fn verify_peer_uid(stream: &UnixStream) -> io::Result<()> {
    let mut cred = libc::ucred { pid: 0, uid: u32::MAX, gid: u32::MAX };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: valid socket fd and properly sized `ucred` output buffer.
    let rc = unsafe { libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    verify_peer_uid_match(cred.uid, my_euid())
}

/// Pure UID check extracted for unit testing.
pub fn verify_peer_uid_match(peer_uid: u32, expected_uid: u32) -> io::Result<()> {
    if peer_uid != expected_uid {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("peer uid {peer_uid} does not match {expected_uid}")));
    }
    Ok(())
}

/// Poll `sock_fd` for `events` (`POLLIN` or `POLLOUT`) until `deadline`,
/// waking immediately on `wake_fd` (if supplied) or `cancelled` (checked at
/// short intervals ONLY during transient handshake I/O when no wake fd exists).
fn poll_until(sock_fd: RawFd, events: i16, deadline: Instant, wake_fd: Option<RawFd>, cancelled: Option<&AtomicBool>) -> io::Result<()> {
    loop {
        if cancelled.is_some_and(|c| c.load(Ordering::Acquire)) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out waiting for Rex screens service"));
        }
        let rem = deadline.saturating_duration_since(now);
        // When `cancelled` has no companion wake fd, cap each poll slice to 25 ms
        // during the short handshake so deselection aborts promptly.
        let slice = if wake_fd.is_none() && cancelled.is_some() { rem.min(Duration::from_millis(25)) } else { rem };
        let timeout_ms = slice.as_millis().clamp(1, i32::MAX as u128) as i32;
        let mut fds = [libc::pollfd { fd: sock_fd, events, revents: 0 }, libc::pollfd { fd: wake_fd.unwrap_or(-1), events: libc::POLLIN, revents: 0 }];
        let nfds: libc::nfds_t = if wake_fd.is_some() { 2 } else { 1 };
        // SAFETY: valid `pollfd` array of length `nfds`.
        let rc = unsafe { libc::poll(fds.as_mut_ptr(), nfds, timeout_ms) };
        if rc < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if rc == 0 {
            continue;
        }
        if wake_fd.is_some() && (fds[1].revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP)) != 0 {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        if (fds[0].revents & (events | libc::POLLERR | libc::POLLHUP | libc::POLLNVAL)) != 0 {
            return Ok(());
        }
    }
}

fn set_nonblocking(fd: RawFd, nonblocking: bool) -> io::Result<()> {
    // SAFETY: valid file descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let next = if nonblocking { flags | libc::O_NONBLOCK } else { flags & !libc::O_NONBLOCK };
    if next != flags && unsafe { libc::fcntl(fd, libc::F_SETFL, next) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Validate an ALREADY-EXISTING directory `dir` without ever creating it or
/// changing its permissions (Astra #8 / Final pass B.4: Agent Screens never
/// creates or chmods Rex's socket directory).
///
/// Opens `dir` with `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC` and verifies via
/// `fstat` that it is owned by `geteuid()` and strictly private
/// (`(mode & 0o077) == 0`).
pub fn validate_existing_private_dir(dir: &Path) -> io::Result<OwnedFd> {
    let cdir = c_path(dir)?;
    // SAFETY: `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC` rejects symlinks and non-dirs atomically.
    let fd = unsafe { libc::open(cdir.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if fd < 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::ELOOP) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "refusing symlinked Rex screens directory"));
        }
        return Err(e);
    }
    // SAFETY: `fd >= 0` is a freshly opened directory descriptor.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: valid fd and writable stat buffer.
    if unsafe { libc::fstat(owned.as_raw_fd(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if (st.st_mode & libc::S_IFMT) != libc::S_IFDIR {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "Rex screens path is not a directory"));
    }
    let euid = my_euid();
    if st.st_uid != euid {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("Rex screens directory owned by uid {}, expected {euid}", st.st_uid)));
    }
    let perms = st.st_mode & 0o777;
    if (perms & 0o077) != 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("Rex screens directory mode {perms:04o} is not private (expected 0700)")));
    }
    Ok(owned)
}

/// Nonblocking connect to Rex's Unix socket with:
/// 1. Strict existing parent-directory check (`validate_existing_private_dir`:
///    never creates or chmods, refuses symlinks / wrong UID / non-`0700` mode)
///    and leaf symlink/UID check.
/// 2. Bounded monotonic `deadline` and optional `wake_fd` / `cancelled` abort,
///    retrying Linux `AF_UNIX` `EAGAIN` (full accept backlog) with a fresh
///    connect attempt rather than falling into `POLLOUT` as if `EINPROGRESS`.
/// 3. Fail-closed `SO_PEERCRED` UID verification **before** returning the
///    stream (so not a single byte can ever be sent to an unverified peer).
pub fn connect_verified(path: &Path, deadline: Instant, wake_fd: Option<RawFd>, cancelled: Option<&AtomicBool>) -> io::Result<UnixStream> {
    if cancelled.is_some_and(|c| c.load(Ordering::Acquire)) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        // B.4 (#8): verify parent directory is existing, non-symlink, owned by
        // us, and strictly private (`mode & 0o077 == 0`) without creating or chmodding it.
        let _dir_fd = validate_existing_private_dir(parent)?;
    }
    connect_unix_peer_verified(path, deadline, wake_fd, cancelled, "Rex screens")
}

/// Nonblocking connect to a screen compositor's Unix socket (`olo_socket`) with
/// bounded deadline, `AF_UNIX` `EAGAIN` backlog retries, leaf non-symlink/UID
/// check, and `SO_PEERCRED` peer-UID authentication — without Rex's `0700`
/// parent-directory requirement since `$XDG_RUNTIME_DIR` may be `0755`/`0775`
/// in containers and test harnesses (Agy-review R1 #3).
pub(crate) fn connect_compositor_verified(path: &Path, deadline: Instant) -> io::Result<UnixStream> {
    connect_unix_peer_verified(path, deadline, None, None, "compositor")
}

fn connect_unix_peer_verified(path: &Path, deadline: Instant, wake_fd: Option<RawFd>, cancelled: Option<&AtomicBool>, label: &str) -> io::Result<UnixStream> {
    if cancelled.is_some_and(|c| c.load(Ordering::Acquire)) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
    }
    // Reject a symlink at the socket path itself before connecting.
    let lmeta = std::fs::symlink_metadata(path)?;
    if lmeta.file_type().is_symlink() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("refusing symlinked {label} socket")));
    }
    if lmeta.uid() != my_euid() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{label} socket owned by another UID")));
    }

    let raw_bytes = path.as_os_str().as_bytes();
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    if raw_bytes.is_empty() || raw_bytes.len() >= addr.sun_path.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{label} socket path too long")));
    }
    for (dst, &b) in addr.sun_path.iter_mut().zip(raw_bytes.iter()) {
        *dst = b as libc::c_char;
    }
    let addr_len = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + raw_bytes.len() + 1) as libc::socklen_t;

    let owned = loop {
        if cancelled.is_some_and(|c| c.load(Ordering::Acquire)) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        // SAFETY: standard Linux nonblocking `AF_UNIX` stream socket creation.
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd >= 0` is a freshly created socket fd.
        let sock = unsafe { OwnedFd::from_raw_fd(fd) };

        // SAFETY: `sock` is valid, `addr` is properly initialized for `addr_len`.
        let rc = unsafe { libc::connect(sock.as_raw_fd(), (&addr as *const libc::sockaddr_un).cast(), addr_len) };
        if rc == 0 {
            break sock;
        }
        let e = io::Error::last_os_error();
        let errno = e.raw_os_error().unwrap_or(0);
        if errno == libc::EINPROGRESS {
            poll_until(sock.as_raw_fd(), libc::POLLOUT, deadline, wake_fd, cancelled)?;
            let mut so_err: libc::c_int = 0;
            let mut so_len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
            // SAFETY: valid socket fd and `so_err` output buffer.
            if unsafe { libc::getsockopt(sock.as_raw_fd(), libc::SOL_SOCKET, libc::SO_ERROR, (&mut so_err as *mut libc::c_int).cast(), &mut so_len) } != 0 {
                return Err(io::Error::last_os_error());
            }
            if so_err != 0 {
                return Err(io::Error::from_raw_os_error(so_err));
            }
            break sock;
        }
        // B.7 (#11/#12): On Linux, nonblocking `AF_UNIX` connect returns `EAGAIN`
        // when the listener's accept backlog is full; the socket has NOT entered
        // the queue, so wait briefly (respecting deadline, wake_fd, cancelled)
        // and issue a fresh `connect()` attempt rather than polling `POLLOUT`.
        if errno == libc::EAGAIN || errno == libc::EWOULDBLOCK || errno == libc::EINTR {
            let rem = deadline.saturating_duration_since(Instant::now());
            if rem.is_zero() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, format!("timed out waiting for {label} service (accept queue full)")));
            }
            let step = rem.min(Duration::from_millis(10));
            if let Some(wfd) = wake_fd {
                let mut p = libc::pollfd { fd: wfd, events: libc::POLLIN, revents: 0 };
                let ms = step.as_millis().clamp(1, i32::MAX as u128) as i32;
                // SAFETY: valid pollfd on wake_fd.
                let pr = unsafe { libc::poll(&mut p, 1, ms) };
                if pr > 0 && (p.revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP)) != 0 {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                }
            } else {
                std::thread::sleep(step);
            }
            continue;
        }
        return Err(e);
    };

    // Transfer ownership of the connected Unix stream fd into `UnixStream`.
    let stream = UnixStream::from(owned);
    // Authenticate the listening peer BEFORE returning or writing a single byte.
    verify_peer_uid(&stream)?;
    Ok(stream)
}

/// Write `bytes` completely to a nonblocking `UnixStream` within one monotonic
/// `deadline`, honoring `wake_fd` / `cancelled` promptly.
pub fn write_all_deadline(stream: &UnixStream, bytes: &[u8], deadline: Instant, wake_fd: Option<RawFd>, cancelled: Option<&AtomicBool>) -> io::Result<()> {
    set_nonblocking(stream.as_raw_fd(), true)?;
    let mut written = 0usize;
    while written < bytes.len() {
        poll_until(stream.as_raw_fd(), libc::POLLOUT, deadline, wake_fd, cancelled)?;
        // SAFETY: valid socket fd and slice bounds.
        let n = unsafe { libc::send(stream.as_raw_fd(), bytes[written..].as_ptr().cast(), bytes.len() - written, libc::MSG_NOSIGNAL) };
        if n > 0 {
            written += n as usize;
        } else if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "Rex closed the socket while writing"));
        } else {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted || e.kind() == io::ErrorKind::WouldBlock {
                continue;
            }
            return Err(e);
        }
    }
    Ok(())
}

/// Read one `\n`-terminated line (at most `max_bytes`) from a `UnixStream`
/// within a single monotonic `deadline` (so slow-drip bytes never outlive the
/// timeout), returning `(line_without_crlf, unread_tail_bytes)` so raw binary
/// relays never lose bytes read past the `Ready` newline.
///
/// Returns `Ok(None)` on clean EOF before any bytes were read.
pub fn read_line_deadline(stream: &UnixStream, max_bytes: usize, deadline: Instant, wake_fd: Option<RawFd>, cancelled: Option<&AtomicBool>) -> io::Result<Option<(Vec<u8>, Vec<u8>)>> {
    set_nonblocking(stream.as_raw_fd(), true)?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        poll_until(stream.as_raw_fd(), libc::POLLIN, deadline, wake_fd, cancelled)?;
        // SAFETY: valid socket fd and writable stack buffer.
        let n = unsafe { libc::recv(stream.as_raw_fd(), chunk.as_mut_ptr().cast(), chunk.len(), 0) };
        if n == 0 {
            return if buf.is_empty() { Ok(None) } else { Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated JSON line before newline")) };
        }
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted || e.kind() == io::ErrorKind::WouldBlock {
                continue;
            }
            return Err(e);
        }
        let got = &chunk[..n as usize];
        if let Some(pos) = got.iter().position(|&b| b == b'\n') {
            if buf.len() + pos + 1 > max_bytes {
                return Err(invalid(format!("JSON line exceeds {max_bytes} bytes")));
            }
            buf.extend_from_slice(&got[..pos]);
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
            let tail = got[pos + 1..].to_vec();
            return Ok(Some((buf, tail)));
        }
        if buf.len() + got.len() > max_bytes {
            return Err(invalid(format!("JSON line exceeds {max_bytes} bytes")));
        }
        buf.extend_from_slice(got);
    }
}

/// Restore blocking mode on a `UnixStream` after nonblocking deadline handshakes
/// complete (before entering long-lived blocking binary relay).
pub fn set_stream_blocking(stream: &UnixStream) -> io::Result<()> {
    set_nonblocking(stream.as_raw_fd(), false)
}

/// Whether `pid` currently refers to a live process **owned by our effective
/// UID**. Never returns `true` for `pid == 0` or another account's PID
/// (`EPERM`).
pub fn pid_is_alive(pid: u32) -> bool {
    let Ok(pid_i32) = i32::try_from(pid) else { return false };
    if pid_i32 <= 0 {
        return false;
    }
    // SAFETY: `kill(pid, 0)` sends no signal and only probes process existence.
    let rc = unsafe { libc::kill(pid_i32, 0) };
    if rc != 0 {
        return false;
    }
    // Confirm `/proc/<pid>` is owned by our effective UID (guards against PID
    // belonging to another user if running privileged or in shared namespaces).
    std::fs::metadata(format!("/proc/{pid}")).is_ok_and(|m| m.uid() == my_euid())
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufReader, Cursor},
        os::unix::{fs::PermissionsExt as _, net::UnixListener},
    };

    use super::*;

    fn sample_state() -> RexScreensState {
        RexScreensState {
            version: STATE_VERSION,
            pid: std::process::id(),
            socket: "/var/tmp/olo-rex-screens/rex-wayland-0.sock".into(),
            rev: 7,
            hosts: vec![
                RexHost {
                    key: "dev-box".into(),
                    label: "dev-box".into(),
                    state: RexHostConn::Connected,
                    error: None,
                    disconnected_at: None,
                    installed: Some("2026.11.27-a1b2c3d4".into()),
                    not_set_up_reason: None,
                    committed: Some(1_795_715_000),
                    default_on_host: true,
                    can_control: Some(true),
                    control_disabled_reason: None,
                    capabilities: vec!["control-ack".into(), "discontiguous_part".into(), "stop-expected-started".into()],
                    screens: vec![RexScreen {
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
                        actions: Some(3),
                        controlled: false,
                        owner: Some(OwnerPane { host_key: Some("dev-box".into()), session_id: "s-1".into(), block_id: "b-9".into() }),
                        started_by: Some(OwnerPane { host_key: Some("dev-box".into()), session_id: "s-1".into(), block_id: "b-1".into() }),
                        controlled_by: Some("Lead".into()),
                    }],
                },
                RexHost {
                    key: "build-farm".into(),
                    label: "build-farm".into(),
                    state: RexHostConn::Disconnected,
                    error: None,
                    disconnected_at: Some(1_795_714_900_000),
                    installed: Some("2026.11.20-deadbeef".into()),
                    not_set_up_reason: None,
                    committed: None,
                    default_on_host: false,
                    can_control: Some(false),
                    control_disabled_reason: Some("Update Olo on this host to take control.".into()),
                    capabilities: Vec::new(),
                    screens: vec![RexScreen {
                        name: "nightly".into(),
                        title: None,
                        state: HostScreenState::Stopped,
                        width: 1280,
                        height: 800,
                        scale: 1.0,
                        sound: false,
                        pid: None,
                        started: None,
                        last_used: Some(1_795_700_000_000),
                        actions: None,
                        controlled: false,
                        owner: None,
                        started_by: None,
                        controlled_by: None,
                    }],
                },
                RexHost {
                    key: "docs-box".into(),
                    label: "docs-box".into(),
                    state: RexHostConn::Connected,
                    error: None,
                    disconnected_at: None,
                    installed: None,
                    not_set_up_reason: Some("Olo’s screen runtime isn’t installed on this host yet.".into()),
                    committed: None,
                    default_on_host: false,
                    can_control: None,
                    control_disabled_reason: None,
                    capabilities: Vec::new(),
                    screens: Vec::new(),
                },
            ],
        }
    }

    #[test]
    fn request_response_and_follow_roundtrip_all_variants() {
        let state = sample_state();
        let requests = vec![
            RexSockRequest::List,
            RexSockRequest::Follow,
            RexSockRequest::Protocol { host: "dev-box".into() },
            RexSockRequest::Stream { host: "dev-box".into(), name: "review".into() },
            RexSockRequest::History { host: "dev-box".into(), name: "review".into() },
            RexSockRequest::Export { host: "dev-box".into(), name: "review".into(), args: vec!["--part".into(), "0,2-4".into(), "--zoom".into(), "2".into()] },
            RexSockRequest::Start { host: "dev-box".into(), name: None, keep: false, size: Some((1440, 900)), scale: Some(1.0), sound: false },
            RexSockRequest::Start { host: "dev-box".into(), name: Some("review".into()), keep: true, size: None, scale: None, sound: false },
            RexSockRequest::Stop { host: "dev-box".into(), name: "review".into(), expected_started: Some(1_795_716_120), expected_last_used: Some(1_795_716_400_000) },
            RexSockRequest::ClearHistory { host: "dev-box".into(), name: "review".into(), expected_started: Some(0), expected_last_used: Some(1_795_716_400_000) },
            RexSockRequest::Rename { host: "dev-box".into(), name: "review".into(), title: Some("Checkout QA".into()) },
            RexSockRequest::Rename { host: "dev-box".into(), name: "review".into(), title: None },
            RexSockRequest::Archive { host: "dev-box".into(), name: "review".into(), archived: true },
            RexSockRequest::Delete { host: "dev-box".into(), name: "review".into(), expected_started: Some(0), expected_last_used: Some(0) },
            RexSockRequest::OpenHost { host: "build-farm".into() },
        ];
        let mut wire = Vec::new();
        for r in &requests {
            write_request(&mut wire, r).unwrap();
        }
        let mut reader = BufReader::new(Cursor::new(wire));
        for expected in &requests {
            assert_eq!(read_request(&mut reader).unwrap().as_ref(), Some(expected));
        }
        assert_eq!(read_request(&mut reader).unwrap(), None);

        let responses = vec![
            RexSockResponse::Ok,
            RexSockResponse::Snapshot { state: state.clone() },
            RexSockResponse::Protocol { proto: 1, commands: vec!["stream".into(), "history-stream".into()], capabilities: vec!["control-ack".into()] },
            RexSockResponse::Ready { width: Some(1440), height: Some(900), scale: Some(1.0) },
            RexSockResponse::Started { screen: Box::new(state.hosts[0].screens[0].clone()) },
            RexSockResponse::Error { message: "screen was replaced by a newer session".into() },
        ];
        let mut rwire = Vec::new();
        for resp in &responses {
            write_response(&mut rwire, resp).unwrap();
        }
        let mut rr = BufReader::new(Cursor::new(rwire));
        for expected in &responses {
            assert_eq!(read_response(&mut rr).unwrap().as_ref(), Some(expected));
        }
        assert_eq!(read_response(&mut rr).unwrap(), None);

        let follow = vec![
            RexFollowLine::Snapshot { state: state.clone() },
            RexFollowLine::HostUpdated { rev: 8, host: Box::new(state.hosts[1].clone()) },
            RexFollowLine::HostRemoved { rev: 9, key: "docs-box".into() },
            RexFollowLine::ScreenLastUsed { host: "dev-box".into(), name: "review".into(), last_used: 1_795_716_500_000, actions: Some(7) },
            RexFollowLine::Error { message: "host reconnecting".into() },
        ];
        let mut fwire = Vec::new();
        for fl in &follow {
            write_follow_line(&mut fwire, fl).unwrap();
        }
        let mut fr = BufReader::new(Cursor::new(fwire));
        for expected in &follow {
            assert_eq!(read_follow_line(&mut fr).unwrap().as_ref(), Some(expected));
        }
        assert_eq!(read_follow_line(&mut fr).unwrap(), None);
    }

    #[test]
    fn round3_f15_f33_f34_validators_enforce_bounds_duplicates_ranges_and_long_stored_titles() {
        // Finding 15: reserved key "local", duplicate host/screen keys, oversized response Error, invalid Started name, wrapping dims.
        assert!(validate_host_key("local").is_err());
        assert!(validate_request(&RexSockRequest::Stream { host: "--evil".into(), name: "qa".into() }).is_err());
        assert!(validate_request(&RexSockRequest::Stream { host: "dev-box".into(), name: "../escape".into() }).is_err());

        let mut dup_state = sample_state();
        dup_state.hosts.push(dup_state.hosts[0].clone());
        assert!(validate_state(&dup_state).is_err(), "duplicate host key must be rejected");

        let mut dup_screen = sample_state();
        let sc = dup_screen.hosts[0].screens[0].clone();
        dup_screen.hosts[0].screens.push(sc);
        assert!(validate_state(&dup_screen).is_err(), "duplicate screen name must be rejected");

        let mut wrap_dim = sample_state();
        wrap_dim.hosts[0].screens[0].width = u32::MAX;
        assert!(validate_state(&wrap_dim).is_err(), "u32::MAX width must be rejected");

        let bad_err = RexSockResponse::Error { message: "x".repeat(MAX_ERROR_LEN + 1) };
        let raw_bad_err = format!("{}\n", serde_json::to_string(&bad_err).unwrap());
        assert!(read_response(&mut BufReader::new(Cursor::new(raw_bad_err))).is_err());

        let mut bad_started = sample_state().hosts[0].screens[0].clone();
        bad_started.name = "../escape".into();
        let raw_bad_started = format!("{}\n", serde_json::to_string(&RexSockResponse::Started { screen: Box::new(bad_started) }).unwrap());
        assert!(read_response(&mut BufReader::new(Cursor::new(raw_bad_started))).is_err());

        let bad_caps = RexSockResponse::Protocol { proto: 1, commands: vec![], capabilities: vec!["x".into(); MAX_CAPABILITIES + 1] };
        let raw_bad_caps = format!("{}\n", serde_json::to_string(&bad_caps).unwrap());
        assert!(read_response(&mut BufReader::new(Cursor::new(raw_bad_caps))).is_err());

        // Finding 33: existing 300-byte stored title is accepted in snapshots, while new UI Rename > 256 bytes is rejected.
        let mut long_stored = sample_state();
        long_stored.hosts[0].screens[0].title = Some("t".repeat(300));
        assert!(validate_state(&long_stored).is_ok());
        assert!(validate_title(Some(&"t".repeat(300))).is_err());

        // Finding 34: inverted --part and --from > --to rejected; 3000-byte valid --part accepted.
        assert!(validate_export_args(&["--part".into(), "5-2".into()]).is_err());
        assert!(validate_export_args(&["--from".into(), "5".into(), "--to".into(), "2".into()]).is_err());
        let big_part = (0..200).map(|i| format!("{}-{}", 100_000 + i * 3, 100_000 + i * 3 + 1)).collect::<Vec<_>>().join(",");
        assert!(big_part.len() > 2048 && big_part.len() <= MAX_EXPORT_PART_SPEC_LEN);
        assert!(validate_export_args(&["--part".into(), big_part]).is_ok());

        // Bounded line & writer checks.
        let mut oversized = vec![b'a'; MAX_REQUEST_LINE_BYTES + 128];
        oversized.push(b'\n');
        assert_eq!(read_request(&mut BufReader::new(Cursor::new(oversized))).unwrap_err().kind(), io::ErrorKind::InvalidData);
        let mut trunc = BufReader::new(Cursor::new(b"{\"op\":\"list\"}".as_slice()));
        assert_eq!(read_request(&mut trunc).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn round3_f1_f8_f9_f12_secure_dir_state_and_verified_connect() {
        let base = std::path::PathBuf::from(format!("/var/tmp/olo-rex-sock-r3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        // Finding 8: symlinked parent directory is rejected by write_state_atomic without altering target mode.
        let real_target = base.join("victim-dir");
        std::fs::create_dir_all(&real_target).unwrap();
        std::fs::set_permissions(&real_target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let sym_dir = base.join("sym-dir");
        std::os::unix::fs::symlink(&real_target, &sym_dir).unwrap();
        let state = sample_state();
        assert!(write_state_atomic(&sym_dir.join("state.json"), &state).is_err());
        assert_eq!(std::fs::metadata(&real_target).unwrap().permissions().mode() & 0o777, 0o755, "symlink target permissions must not be changed");

        // Normal private directory writes 0700 dir and 0600 file.
        let priv_dir = base.join("priv");
        let path = priv_dir.join("state-wayland-test.json");
        write_state_atomic(&path, &state).unwrap();
        assert_eq!(std::fs::metadata(&priv_dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(read_state_file(&path).unwrap(), state);

        // Finding 8: group-writable state file is rejected on read.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o660)).unwrap();
        assert!(read_state_file(&path).is_err(), "group-writable state file must be rejected");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        // Symlinked state file is refused.
        let link = priv_dir.join("state-link.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(read_state_file(&link).is_err());

        // Finding 1: connect_verified refuses symlinked socket and checks SO_PEERCRED before first byte.
        let sock = priv_dir.join("rex-test.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let sock_link = priv_dir.join("rex-link.sock");
        std::os::unix::fs::symlink(&sock, &sock_link).unwrap();
        let dl = Instant::now() + Duration::from_secs(2);
        assert!(connect_verified(&sock_link, dl, None, None).is_err(), "symlinked socket must be refused");
        assert!(verify_peer_uid_match(9999, my_euid()).is_err());

        // Valid verified connect + deadline slow-drip timeout check (Finding 12).
        let srv = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let _ = conn.write_all(b"{\"kind\":");
            std::thread::sleep(Duration::from_millis(250));
        });
        let stream = connect_verified(&sock, Instant::now() + Duration::from_secs(2), None, None).unwrap();
        let err = read_line_deadline(&stream, MAX_RESPONSE_LINE_BYTES, Instant::now() + Duration::from_millis(80), None, None).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        drop(stream);
        srv.join().unwrap();

        // Finding 9: pid_is_alive rejects 0 and root PID 1 (owned by another UID when non-root).
        assert!(pid_is_alive(std::process::id()));
        assert!(!pid_is_alive(0));
        if my_euid() != 0 {
            assert!(!pid_is_alive(1), "another account's PID must not be treated as a live Rex instance");
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn final_pass_b4_f8_validate_existing_private_dir_never_chmods_or_creates() {
        let base = std::path::PathBuf::from(format!("/var/tmp/olo-rex-sock-b4-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        // Missing directory must fail with NotFound and MUST NOT be created.
        let missing = base.join("never-created");
        let err = validate_existing_private_dir(&missing).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(!missing.exists(), "validate_existing_private_dir must never create a directory");

        // Shared user-owned 0770 directory must be refused WITHOUT chmodding it to 0700.
        let shared = base.join("shared-project");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o770)).unwrap();
        let sock = shared.join("rex-test.sock");
        let _listener = UnixListener::bind(&sock).unwrap();

        let err_dir = validate_existing_private_dir(&shared).unwrap_err();
        assert_eq!(err_dir.kind(), io::ErrorKind::PermissionDenied);
        let err_conn = connect_verified(&sock, Instant::now() + Duration::from_secs(1), None, None).unwrap_err();
        assert_eq!(err_conn.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(std::fs::metadata(&shared).unwrap().permissions().mode() & 0o777, 0o770, "shared project directory permissions must remain untouched");

        // 0755 directory must also be refused untouched.
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(validate_existing_private_dir(&shared).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(std::fs::metadata(&shared).unwrap().permissions().mode() & 0o777, 0o755);

        // 0700 private directory succeeds.
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o700)).unwrap();
        validate_existing_private_dir(&shared).unwrap();

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn final_pass_b7_f11_f12_connect_verified_retries_af_unix_eagain() {
        let base = std::path::PathBuf::from(format!("/var/tmp/olo-rex-sock-b7-eagain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
        let sock = base.join("backlog.sock");

        // Bind a Unix listener with backlog=1 so filling the accept queue triggers Linux AF_UNIX EAGAIN.
        let listener = UnixListener::bind(&sock).unwrap();
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 1) }, 0);

        let mut fillers = Vec::new();
        let mut saw_eagain = false;
        for _ in 0..64 {
            let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC, 0) };
            assert!(fd >= 0);
            let owned = unsafe { OwnedFd::from_raw_fd(fd) };
            let raw_bytes = sock.as_os_str().as_bytes();
            let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
            addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
            for (dst, &b) in addr.sun_path.iter_mut().zip(raw_bytes.iter()) {
                *dst = b as libc::c_char;
            }
            let addr_len = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + raw_bytes.len() + 1) as libc::socklen_t;
            let rc = unsafe { libc::connect(owned.as_raw_fd(), (&addr as *const libc::sockaddr_un).cast(), addr_len) };
            if rc == 0 {
                fillers.push(owned);
            } else {
                let err = io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EAGAIN) || err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                    saw_eagain = true;
                    break;
                }
                panic!("unexpected error filling Unix backlog: {err}");
            }
        }
        assert!(saw_eagain, "expected Linux AF_UNIX backlog=1 queue to return EAGAIN");

        // Drain the listener after 40 ms so `connect_verified` must retry through EAGAIN and succeed.
        let count_to_accept = fillers.len() + 1;
        let drainer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            let mut accepted = Vec::with_capacity(count_to_accept);
            for _ in 0..count_to_accept {
                let (c, _) = listener.accept().unwrap();
                accepted.push(c);
            }
            // Keep accepted sockets alive briefly so SO_PEERCRED succeeds cleanly.
            std::thread::sleep(Duration::from_millis(50));
            drop(accepted);
        });

        let stream = connect_verified(&sock, Instant::now() + Duration::from_secs(2), None, None).expect("connect_verified must retry AF_UNIX EAGAIN until listener drains");
        drop(stream);
        drop(fillers);
        drainer.join().unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }
}
