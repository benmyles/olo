//! Host Agent Screens follower, desktop screen ownership, and local Unix socket
//! server for the Agent Screens app (`apps/screen`).
//!
//! Rex owns every SSH/QUIC connection to remote hosts. For each connected SSH
//! host, [`ScreensService`] runs one zero-polling `olo-rex-link screen follow`
//! stream and probes `olo-rex-link screen --protocol` once per connection,
//! keeping the latest host/screen snapshot in memory, feeding Rex's own Host
//! window and populating `HubState::{host_screens, desktop_screen_owners}`,
//! publishing an atomic `state-<display>.json`
//! file off the UI/I/O threads, and serving live updates, binary relays
//! (`stream`, `history`, `export`), and guarded lifecycle RPCs over
//! `rex-<display>.sock`.
//!
//! Wire types, bounds, validators, and peer UID checks come straight from
//! [`olo_screen_pane::rex_sock`] and [`olo_screen_pane::paths`].

use std::{
    collections::{BTreeMap, HashMap},
    ffi::CString,
    future::Future,
    io::{self, Read as _, Write as _},
    os::{
        fd::{AsRawFd as _, FromRawFd as _, OwnedFd},
        unix::{
            ffi::OsStrExt as _,
            fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _},
            net::UnixStream as StdUnixStream,
        },
    },
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use olo_rex_client::{RexEndpoint, SshAskpass, SshSessionPool, SshTarget, ssh::screen_command, transport::AsyncReadWrite};
use olo_rex_link::screen_wire::{self, FollowEvent, HostScreen, HostScreenState, HostScreenStatus, MAX_JSON_LINE, OwnerPane, ScreenOwner};
use olo_screen_pane::{
    paths as screen_paths,
    rex_sock::{
        self, MAX_CAPABILITIES, MAX_ERROR_LEN, MAX_HOST_KEY_LEN, MAX_HOSTS, MAX_LABEL_LEN, MAX_REQUEST_LINE_BYTES, MAX_RESPONSE_LINE_BYTES, MAX_SCREEN_DIM, MAX_SCREENS_PER_HOST, MAX_STATE_FILE_BYTES,
        MAX_TITLE_LEN, MAX_TOTAL_SCREENS, RexFollowLine, RexHost, RexHostConn, RexScreen, RexScreensState, RexSockRequest, RexSockResponse, STATE_VERSION,
    },
};
use olo_sync::Mutex;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader, Interest, unix::AsyncFd},
    net::{UnixListener, UnixStream},
    sync::{Notify, OwnedSemaphorePermit, Semaphore, broadcast, watch},
};

use crate::hub::{BlockKey, HostConnState, HubNotice, HubState, session_tab_names, tab_title};

/// Broadcast channel capacity for `{"op":"follow"}` subscribers. Lagging
/// subscribers are resynced with a full [`RexFollowLine::Snapshot`] rather than
/// buffering unbounded messages.
const FOLLOW_BROADCAST_CAP: usize = 256;
/// Maximum simultaneous local Unix socket connections to `rex-<display>.sock`.
const MAX_CONCURRENT_CLIENTS: usize = 64;
/// Maximum simultaneous open relays (`stream`, `history`, `export`) per remote
/// host so screen relays cannot starve Rex's SSH session pool.
const MAX_RELAYS_PER_HOST: usize = 8;
/// Maximum protocol command names accepted per host.
const MAX_COMMANDS: usize = 32;
/// Maximum length of a single capability or command name string.
const MAX_CAP_NAME_LEN: usize = 64;
/// Soft serialized byte allowance for a single host's screen list inside
/// `RexScreensState` so one noisy host cannot take all other hosts offline.
const MAX_HOST_SCREENS_BYTES: usize = 256 * 1024;
/// Maximum output bytes retained from a single one-shot host screen command.
const MAX_RPC_OUTPUT_BYTES: usize = 256 * 1024;
/// Maximum single-line bytes retained from a one-shot host screen command.
const MAX_RPC_LINE_BYTES: usize = 64 * 1024;

/// Deadline for reading the initial request line on a new client connection.
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Deadline for writing a single NDJSON response or follow line to a local client.
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// Total handshake deadline for `protocol`, `stream`, `history`, and `export`
/// (kept strictly below the client's 15 s handshake timeout).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(12);
/// Deadline for opening a follower stream and receiving its initial snapshot.
const FOLLOW_OPEN_TIMEOUT: Duration = Duration::from_secs(15);
/// Minimum healthy duration a `follow` stream must stay up before resetting
/// reconnect backoff (prevents tight 1 s reconnect loops on broken helpers).
const FOLLOW_STABLE_WINDOW: Duration = Duration::from_secs(30);
/// Total deadline for one-shot lifecycle RPCs (`start`, `stop`, `clear_history`,
/// `rename`, `archive`, `delete`), kept strictly below the client's 60 s timeout.
const RPC_TIMEOUT: Duration = Duration::from_secs(55);
/// Initial reconnect delay for transient follower failures.
const FOLLOW_RETRY_BASE: Duration = Duration::from_secs(1);
/// Maximum reconnect delay for transient follower failures.
const FOLLOW_RETRY_MAX: Duration = Duration::from_secs(60);
/// Bounded wait for acquiring a desktop screen's `.owner.lock`.
const OWNER_LOCK_TIMEOUT: Duration = Duration::from_millis(500);
/// Bounded drain window when a relay's client closes its write half while the
/// remote stream is backpressured.
const RELAY_RDHUP_DRAIN: Duration = Duration::from_millis(200);
/// Bounded shutdown window when tearing down relay write halves after select exits.
const RELAY_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(50);

const NOT_INSTALLED_REASON: &str = "Olo’s screen runtime isn’t installed on this host yet.";
const OLD_HELPER_REASON: &str = "This host’s Olo helper needs an update before Agent Screens can be set up.";
const OLD_OLO_CONTROL_REASON: &str = "Update Olo on this host to take control.";
const TOO_MANY_SCREENS_REASON: &str = "Host reported more screens than Rex can publish.";

/// Commands on a desktop screen that update `NAME.owner.json` when driven by a
/// Rex pane (matches `crates/rex-link/src/screens.rs::OWNER_ACTIONS` and
/// `crates/screen-pane/src/history.rs::ACTIONS`).
const DESKTOP_OWNER_ACTIONS: &[&str] = &[
    "run",
    "take",
    "give",
    "click",
    "move",
    "down",
    "up",
    "drag",
    "scroll",
    "wheel",
    "key",
    "type",
    "set",
    "select",
    "scroll-to",
    "invoke",
    "focus",
    "close",
    "minimize",
    "zoom",
    "maximize",
    "resize",
    "place",
];

static NEXT_HOST_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);
static TMP_FILE_SEQ: AtomicU64 = AtomicU64::new(1);

/// Allocate a globally unique, monotonic host instance ID (never reused across
/// remove + re-add of the same host key).
pub fn next_host_instance_id() -> u64 {
    NEXT_HOST_INSTANCE_ID.fetch_add(1, Ordering::Relaxed)
}

/// Allocate a globally unique, monotonic connection ID (never reused across
/// reconnects or host replacements).
pub fn next_conn_id() -> u64 {
    NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed)
}

/// True when `endpoint_raw` parses as an SSH Rex host via Rex's own endpoint
/// parser (`dev-box`, `user@dev-box`, `ssh://...` — the only host transport
/// that carries `olo-rex-link screen …`).
pub fn is_ssh_endpoint(endpoint_raw: &str) -> bool {
    static PARSE_POOL: std::sync::LazyLock<Arc<SshSessionPool>> = std::sync::LazyLock::new(|| Arc::new(SshSessionPool::default()));
    matches!(RexEndpoint::parse(endpoint_raw, PARSE_POOL.clone(), None), Ok(RexEndpoint::Ssh { .. }))
}

// ---------------------------------------------------------------------------
// ScreenOpener trait & real RexHub implementation
// ---------------------------------------------------------------------------

pub type BoxedScreenStream = Pin<Box<dyn AsyncReadWrite + Send>>;
pub type ScreenStreamFuture = Pin<Box<dyn Future<Output = Result<BoxedScreenStream, String>> + Send>>;
pub type ScreenCommandFuture = Pin<Box<dyn Future<Output = Result<CommandExecOutput, String>> + Send>>;

/// Output of a one-shot `olo-rex-link screen …` invocation with stdout and
/// stderr kept separate and truncation flagged explicitly.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandExecOutput {
    pub status: Option<u32>,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

impl CommandExecOutput {
    pub fn ok(&self) -> bool {
        self.status == Some(0)
    }

    pub fn error_words(&self, fallback: &str) -> String {
        let err = self.stderr.trim();
        if !err.is_empty() {
            return truncate_utf8(err, MAX_ERROR_LEN);
        }
        let out = self.stdout.trim();
        if !out.is_empty() {
            return truncate_utf8(out, MAX_ERROR_LEN);
        }
        fallback.to_owned()
    }
}

/// Concrete SSH target pinned at the moment a host connection is established,
/// so an in-flight probe or lifecycle command can never cross to a replacement
/// machine whose address was edited under the same host key.
#[derive(Clone)]
pub struct PinnedSshTarget {
    pub target: SshTarget,
    pub pool: Arc<SshSessionPool>,
    pub askpass: Option<Arc<dyn SshAskpass>>,
    pub preferred_helper: Option<&'static str>,
}

/// Resolved connection identity for a screen stream or command dispatch.
#[derive(Clone)]
pub struct ScreenTarget {
    pub host_key: String,
    pub host_instance_id: u64,
    pub conn_id: u64,
    pub pinned: Option<Arc<PinnedSshTarget>>,
}

/// Transport abstraction for reaching `olo-rex-link screen …` on a remote host
/// (plus focusing Rex's Host window). Replaced by an in-memory double in tests.
pub trait ScreenOpener: Send + Sync + 'static {
    /// Open a bidirectional byte stream to `olo-rex-link screen <args...>` on
    /// `target` (`follow`, `stream`, `history-stream`, `export`).
    fn open_screen_stream(&self, target: ScreenTarget, args: Vec<String>) -> ScreenStreamFuture;

    /// Run a one-shot `olo-rex-link screen <args...>` command on `target`,
    /// keeping stdout and stderr separate and bounding output & wait time.
    fn run_screen_command(&self, target: ScreenTarget, args: Vec<String>, timeout: Duration) -> ScreenCommandFuture;

    /// Activate Rex's Host window for `host_key`.
    fn open_host(&self, host_key: &str) -> Result<(), String>;
}

/// Production [`ScreenOpener`] backed by Rex's [`HubState`] and pinned
/// [`SshSessionPool`] targets. Never installs or modifies remote helpers on its
/// own (helper installation belongs exclusively to `RexHubInner::look_after_helper`).
pub struct HubScreenOpener {
    state: Arc<Mutex<HubState>>,
    notify: Arc<dyn Fn(HubNotice) + Send + Sync>,
    ui_ready: Arc<AtomicBool>,
}

impl HubScreenOpener {
    pub fn new(state: Arc<Mutex<HubState>>, notify: Arc<dyn Fn(HubNotice) + Send + Sync>, ui_ready: Arc<AtomicBool>) -> Self {
        Self { state, notify, ui_ready }
    }

    fn verify_pinned(&self, target: &ScreenTarget) -> Result<(String, Arc<PinnedSshTarget>), String> {
        let pinned = target.pinned.clone().ok_or_else(|| format!("“{}” isn’t reached over SSH.", target.host_key))?;
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(h) = st.hosts.get(&target.host_key).filter(|h| !h.is_local) else {
            return Err(format!("“{}” isn’t one of Rex’s hosts.", target.host_key));
        };
        if h.host_instance_id != target.host_instance_id || h.screens_conn_id != target.conn_id || !matches!(h.conn_state, HostConnState::Connected) {
            return Err(format!("“{}” is disconnected. Reconnect in Rex to resume.", h.label));
        }
        Ok((h.label.clone(), pinned))
    }
}

impl ScreenOpener for HubScreenOpener {
    fn open_screen_stream(&self, target: ScreenTarget, args: Vec<String>) -> ScreenStreamFuture {
        let verify = self.verify_pinned(&target);
        Box::pin(async move {
            let (_label, pinned) = verify?;
            // Reject invalid streaming arguments before touching QUIC or SSH;
            // never use a catch-all SSH fallback on validation failure.
            // Validated legacy `["history", NAME, "--wire"]` is routed through
            // `open_screen_stream` as well (onto the isolated SSH screen
            // connection, never QUIC and never the primary terminal channel).
            let is_legacy_wire_history = args.len() == 3 && args[0] == "history" && args[2] == "--wire" && rex_sock::validate_screen_name(&args[1]).is_ok();
            if !is_legacy_wire_history {
                screen_wire::validate_quic_screen_args(&args).map_err(|e| e.to_string())?;
            }
            pinned.pool.open_screen_stream(&pinned.target, pinned.askpass.clone(), &args).await.map_err(|e| e.to_string())
        })
    }

    fn run_screen_command(&self, target: ScreenTarget, args: Vec<String>, timeout: Duration) -> ScreenCommandFuture {
        let verify = self.verify_pinned(&target);
        Box::pin(async move {
            let (label, pinned) = verify?;
            // Use the transport's shared `screen_command` selector so streams,
            // probes, and one-shot commands always pick the exact same verified
            // helper binary, and read `ran.stderr` directly (Findings 8 & 18).
            let cmd = screen_command(pinned.preferred_helper, &args);
            let ran = tokio::time::timeout(timeout, pinned.pool.exec_bounded(&pinned.target, pinned.askpass.clone(), &cmd, MAX_RPC_OUTPUT_BYTES, MAX_RPC_LINE_BYTES, &mut |_| {}))
                .await
                .map_err(|_| format!("“{label}” didn’t answer within {} s.", timeout.as_secs().max(1)))?
                .map_err(|e| e.to_string())?;
            if ran.status == Some(69) {
                return Err("the host has no screens-capable Olo helper".to_owned());
            }
            Ok(CommandExecOutput { status: ran.status, stdout: ran.stdout, stderr: ran.stderr, truncated: ran.truncated })
        })
    }

    fn open_host(&self, host_key: &str) -> Result<(), String> {
        if !self.ui_ready.load(Ordering::Acquire) {
            return Err("Rex window isn’t ready yet.".to_owned());
        }
        let exists = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.hosts.get(host_key).is_some_and(|h| !h.is_local)
        };
        if !exists {
            return Err(format!("“{host_key}” isn’t one of Rex’s hosts."));
        }
        (self.notify)(HubNotice::OpenHost(host_key.to_owned()));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Hub screen owner -> display name (`controlled_by`) resolution
// ---------------------------------------------------------------------------

/// Resolve a remote host's controlling [`OwnerPane`] to a human-readable tab or
/// subagent name from [`HubState`], strictly bound to the authenticated
/// `host_key` (never trusting a remote host's self-reported `pane.host_key`).
pub fn resolve_controlled_by_in_hub(state: &Arc<Mutex<HubState>>, host_key: &str, pane: &OwnerPane) -> Option<String> {
    if pane.block_id.is_empty() {
        return None;
    }
    let st = state.lock().unwrap_or_else(|e| e.into_inner());
    let sid_opt = (!pane.session_id.is_empty()).then_some(pane.session_id.as_str());

    // 1. Check native subagent registry first (subagents carry explicit titles).
    if let Some(sub) = st.subagents.by_block(host_key, sid_opt, &pane.block_id) {
        let name = if !sub.title.trim().is_empty() { sub.title.trim() } else { sub.cli.display_name() };
        if !name.is_empty() {
            return Some(sanitize_label(name, MAX_LABEL_LEN));
        }
    }

    // 2. Check the authenticated host's live sessions and tabs.
    let h = st.hosts.get(host_key)?;
    let local_home = if h.is_local { crate::hub::local_home() } else { None };
    let session_iter: Vec<&olo_rex_client::SessionView> = if let Some(sid) = sid_opt { h.sessions.get(sid).into_iter().collect() } else { h.sessions.values().collect() };

    for sess in session_iter {
        let tabs = session_tab_names(&st, host_key, sess);
        for (idx, w) in sess.windows.iter().enumerate() {
            if w.all_blocks().any(|b| b.block_id == pane.block_id) {
                if let Some(tab) = tabs.get(idx).map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    return Some(sanitize_label(tab, MAX_LABEL_LEN));
                }
                let bk = BlockKey { host_key: host_key.to_owned(), session_id: sess.session_id.clone(), block_id: pane.block_id.clone() };
                if let Some(br) = st.blocks.get(&bk) {
                    let g = br.lock().unwrap_or_else(|e| e.into_inner());
                    let program = g.program_title();
                    let home = g.process.as_ref().and_then(|p| p.home.as_deref()).or(local_home.as_deref());
                    let t = tab_title(&g.title, &g.label, program.as_deref(), &g.pwd, home);
                    if !t.trim().is_empty() {
                        return Some(sanitize_label(t.trim(), MAX_LABEL_LEN));
                    }
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Verified private directory & descriptor-relative file I/O helpers
// ---------------------------------------------------------------------------

fn my_euid() -> u32 {
    // SAFETY: `geteuid()` has no preconditions.
    unsafe { libc::geteuid() }
}

fn c_path(p: &Path) -> io::Result<CString> {
    CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL byte in path"))
}

fn c_name(name: &str) -> io::Result<CString> {
    if name.is_empty() || name.contains('/') || name.contains('\0') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("invalid filename {name:?}")));
    }
    CString::new(name).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL byte in filename"))
}

/// Verify that every existing ancestor directory of `dir` is either root-owned
/// without world/group write (unless sticky) or owned by our UID without world
/// write. Prevents trusting a symlinked or other-user-writable directory tree.
fn verify_safe_ancestors(dir: &Path, uid: u32) -> io::Result<()> {
    let mut anc = dir.parent();
    while let Some(p) = anc {
        if p.as_os_str().is_empty() || p == Path::new("/") {
            break;
        }
        if let Ok(m) = std::fs::metadata(p) {
            let mode = m.mode();
            let auid = m.uid();
            let safe_root = auid == 0 && ((mode & 0o022) == 0 || (mode & libc::S_ISVTX) != 0);
            let safe_self = auid == uid && (mode & 0o002) == 0;
            if !safe_root && !safe_self {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("unsafe ancestor directory {}", p.display())));
            }
        }
        anc = p.parent();
    }
    Ok(())
}

/// Open or create a private `0700` directory pinned by an `O_DIRECTORY |
/// O_NOFOLLOW | O_CLOEXEC` file descriptor.
///
/// - If `dir` does **not** exist, creates it with mode `0700` (after verifying
///   ancestors) and ensures mode `0700` via `fchmod`.
/// - If `dir` **already** exists, refuses symlinks (`ELOOP`), non-directories
///   (`ENOTDIR`), directories owned by another UID, and directories with any
///   group/other permission bits (`mode & 0o077 != 0`) **without** modifying
///   the pre-existing directory's permissions.
fn open_verified_private_dir(dir: &Path) -> io::Result<OwnedFd> {
    let uid = my_euid();
    verify_safe_ancestors(dir, uid)?;

    let created = match std::fs::symlink_metadata(dir) {
        Ok(m) => {
            if !m.file_type().is_dir() {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{} is not a real directory", dir.display())));
            }
            false
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let mut b = std::fs::DirBuilder::new();
            b.recursive(true).mode(0o700);
            b.create(dir)?;
            true
        }
        Err(e) => return Err(e),
    };

    let c = c_path(dir)?;
    // SAFETY: `c` is NUL-terminated; `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC` refuses symlinks atomically.
    let raw = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: freshly opened owned descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if (st.st_mode & libc::S_IFMT) != libc::S_IFDIR || st.st_uid != uid {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{} is not owned by uid {uid}", dir.display())));
    }
    if created && (st.st_mode & 0o777) != 0o700 {
        if unsafe { libc::fchmod(fd.as_raw_fd(), 0o700) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    if (st.st_mode & 0o077) != 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("refusing directory {} with mode {:04o} (must be 0700)", dir.display(), st.st_mode & 0o777)));
    }
    Ok(fd)
}

/// Open an existing directory owned by `my_euid()` without following symlinks
/// (allows `0700` or `0755` user-owned directories like `$XDG_STATE_HOME/olo/screens`
/// while rejecting symlinks, non-directories, world-writable, and other-UID dirs).
fn open_verified_owned_dir(dir: &Path, create_if_missing: bool) -> io::Result<OwnedFd> {
    let uid = my_euid();
    verify_safe_ancestors(dir, uid)?;
    if create_if_missing && std::fs::symlink_metadata(dir).is_err() {
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true).mode(0o700);
        b.create(dir)?;
    }
    let c = c_path(dir)?;
    // SAFETY: `c` is NUL-terminated.
    let raw = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: freshly opened owned descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if (st.st_mode & libc::S_IFMT) != libc::S_IFDIR || st.st_uid != uid || (st.st_mode & 0o002) != 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("unsafe directory {}", dir.display())));
    }
    Ok(fd)
}

/// Open (optionally creating via `mkdirat`) a direct child directory `name`
/// inside pinned `parent_fd` with `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC`.
fn open_verified_subdir_at(parent_fd: &OwnedFd, name: &str, create_if_missing: bool) -> io::Result<OwnedFd> {
    let uid = my_euid();
    let cname = c_name(name)?;
    if create_if_missing {
        // SAFETY: valid dirfd and single component name.
        let rc = unsafe { libc::mkdirat(parent_fd.as_raw_fd(), cname.as_ptr(), 0o700) };
        if rc != 0 {
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::AlreadyExists {
                return Err(err);
            }
        }
    }
    // SAFETY: valid dirfd and single component name; refuses symlinks atomically.
    let raw = unsafe { libc::openat(parent_fd.as_raw_fd(), cname.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: freshly opened owned descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if (st.st_mode & libc::S_IFMT) != libc::S_IFDIR || st.st_uid != uid || (st.st_mode & 0o002) != 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("unsafe screen directory {name:?}")));
    }
    Ok(fd)
}

/// Read a bounded regular file directly inside pinned `dir_fd` (refusing
/// symlinks, FIFOs, wrong UID, and files exceeding `max_bytes`).
fn read_bounded_regular_at(dir_fd: &OwnedFd, file_name: &str, max_bytes: usize) -> Option<Vec<u8>> {
    let cname = c_name(file_name).ok()?;
    // SAFETY: valid dirfd and NUL-terminated component name.
    let raw = unsafe { libc::openat(dir_fd.as_raw_fd(), cname.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if raw < 0 {
        return None;
    }
    // SAFETY: freshly opened owned descriptor.
    let f = unsafe { std::fs::File::from_raw_fd(raw) };
    let meta = f.metadata().ok()?;
    if !meta.file_type().is_file() || meta.len() > max_bytes as u64 || meta.uid() != my_euid() {
        return None;
    }
    let mut buf = Vec::with_capacity((meta.len() as usize).min(64 << 10));
    f.take(max_bytes as u64 + 1).read_to_end(&mut buf).ok()?;
    (buf.len() <= max_bytes).then_some(buf)
}

fn regular_exists_at(dir_fd: &OwnedFd, file_name: &str) -> bool {
    let Ok(cname) = c_name(file_name) else { return false };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: valid dirfd and single component name.
    unsafe { libc::fstatat(dir_fd.as_raw_fd(), cname.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFREG }
}

/// Atomically write `bytes` to `file_name` inside pinned `dir_fd` using an
/// exclusively created mode `0600` temp file (`openat(O_CREAT | O_EXCL |
/// O_NOFOLLOW)`) + `renameat`. Never unlinks a temp name whose creation failed.
fn write_file_atomic_at(dir_fd: &OwnedFd, file_name: &str, bytes: &[u8]) -> io::Result<()> {
    let target_c = c_name(file_name)?;
    let seq = TMP_FILE_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_name = format!(".{file_name}.tmp.{}.{seq}", std::process::id());
    let tmp_c = c_name(&tmp_name)?;
    // SAFETY: valid dirfd and single component name.
    let raw = unsafe { libc::openat(dir_fd.as_raw_fd(), tmp_c.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o600) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: exclusively created fd owned by us.
    let mut file = unsafe { std::fs::File::from_raw_fd(raw) };
    let write_res = file.write_all(bytes).and_then(|()| file.flush());
    drop(file);
    if let Err(e) = write_res {
        // SAFETY: we exclusively created `tmp_c`.
        unsafe { libc::unlinkat(dir_fd.as_raw_fd(), tmp_c.as_ptr(), 0) };
        return Err(e);
    }
    // SAFETY: valid dirfd and single component names.
    if unsafe { libc::renameat(dir_fd.as_raw_fd(), tmp_c.as_ptr(), dir_fd.as_raw_fd(), target_c.as_ptr()) } != 0 {
        let err = io::Error::last_os_error();
        unsafe { libc::unlinkat(dir_fd.as_raw_fd(), tmp_c.as_ptr(), 0) };
        return Err(err);
    }
    Ok(())
}

/// Open a regular mode-`0600` lock file inside pinned `dir_fd` with
/// `O_RDWR | O_CREAT | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC`, verify `S_ISREG`
/// and `st_uid == my_euid()`, and acquire an exclusive flock (non-blocking or
/// bounded to `wait`).
fn acquire_lock_at(dir_fd: &OwnedFd, lock_name: &str, wait: Option<Duration>) -> io::Result<OwnedFd> {
    let uid = my_euid();
    let cname = c_name(lock_name)?;
    // SAFETY: valid dirfd and single component name.
    let raw = unsafe { libc::openat(dir_fd.as_raw_fd(), cname.as_ptr(), libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC, 0o600) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: freshly opened owned descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if (st.st_mode & libc::S_IFMT) != libc::S_IFREG || st.st_uid != uid {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("unsafe lock file {lock_name:?}")));
    }
    let deadline = wait.map(|d| Instant::now() + d);
    loop {
        // SAFETY: valid open regular file descriptor.
        if unsafe { libc::flock(fd.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(fd);
        }
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if err.kind() != io::ErrorKind::WouldBlock {
            return Err(err);
        }
        let Some(dl) = deadline else {
            return Err(io::Error::new(io::ErrorKind::WouldBlock, format!("lock {lock_name:?} is held by another process")));
        };
        let now = Instant::now();
        if now >= dl {
            return Err(io::Error::new(io::ErrorKind::TimedOut, format!("timed out waiting for lock {lock_name:?}")));
        }
        std::thread::sleep(Duration::from_millis(5).min(dl.saturating_duration_since(now)));
    }
}

/// Write a validated [`RexScreensState`] atomically inside pinned `dir_fd`.
fn write_state_atomic_at(dir_fd: &OwnedFd, file_name: &str, state: &RexScreensState) -> io::Result<()> {
    rex_sock::validate_state(state).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let bytes = serde_json::to_vec_pretty(state).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("encode RexScreensState: {e}")))?;
    if bytes.len() > MAX_STATE_FILE_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("state file ({} bytes) exceeds {MAX_STATE_FILE_BYTES}", bytes.len())));
    }
    write_file_atomic_at(dir_fd, file_name, &bytes)
}

fn read_state_file_at(dir_fd: &OwnedFd, file_name: &str) -> Option<RexScreensState> {
    let bytes = read_bounded_regular_at(dir_fd, file_name, MAX_STATE_FILE_BYTES)?;
    let state: RexScreensState = serde_json::from_slice(&bytes).ok()?;
    rex_sock::validate_state(&state).ok()?;
    Some(state)
}

// ---------------------------------------------------------------------------
// Desktop screen ownership (`NAME.owner.json` & `$XDG_STATE_HOME/olo/screens/NAME/owner.json`)
// ---------------------------------------------------------------------------

/// Running desktop screen generation snapshot used to prevent late bridge
/// completions from claiming or unlinking a replacement session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DesktopScreenGen {
    pub pid: i32,
    pub started: u64,
    pub start_ticks: Option<u64>,
}

/// Action taken by a host agent on a desktop screen through Rex's bridge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BridgeScreenAction {
    /// Screen was started (`start` or `take --new`), carrying the created
    /// generation from `olo-screen`'s stdout JSON when present.
    Started { name: String, pid: Option<i32>, started: Option<u64> },
    /// Existing screen was mutated (`run`, `click`, `type`, `take NAME`, ...).
    Mutated { name: String },
    /// Screen was stopped (unlink runtime owner if same generation; retain persistent owner).
    Stopped { name: String },
    /// Screen was deleted (unlink runtime owner and forget from hub if not replaced).
    Deleted { name: String },
}

/// Extract the target screen name from raw `olo.screen` bridge arguments BEFORE
/// invoking `olo-screen` so the caller can snapshot its running generation.
pub fn target_name_for_bridge_args(args: &[String]) -> Option<String> {
    let cmd = args.first().map(String::as_str)?;
    let rest = &args[1..];
    let candidate = match cmd {
        "stop" | "delete" => rest.iter().find(|a| !a.starts_with("--")),
        "take" => {
            if rest.iter().any(|a| a == "--new") {
                None
            } else {
                rest.iter().find(|a| !a.starts_with("--"))
            }
        }
        other if DESKTOP_OWNER_ACTIONS.contains(&other) => rest.iter().find(|a| a.as_str() != "--quick" && !a.starts_with("--")),
        _ => None,
    }?;
    rex_sock::validate_screen_name(candidate).ok()?;
    Some(candidate.clone())
}

/// Inspect a successful bridged `olo-screen` invocation (`args` + `stdout`) to
/// determine whether and how desktop screen ownership changed.
pub fn extract_bridge_screen_action(args: &[String], stdout: &str) -> Option<BridgeScreenAction> {
    let cmd = args.first().map(String::as_str)?;
    let rest = &args[1..];
    match cmd {
        "start" => {
            let v = parse_json_object_from_stdout(stdout).ok()?;
            let name = v["name"].as_str()?.to_owned();
            rex_sock::validate_screen_name(&name).ok()?;
            let pid = v["pid"].as_i64().and_then(|p| i32::try_from(p).ok());
            let started = v["started"].as_u64();
            Some(BridgeScreenAction::Started { name, pid, started })
        }
        "take" => {
            let is_new = rest.iter().any(|a| a == "--new");
            let parsed = parse_json_object_from_stdout(stdout).ok();
            let name = parsed.as_ref().and_then(|v| v["name"].as_str().map(str::to_owned)).or_else(|| (!is_new).then(|| rest.iter().find(|a| !a.starts_with("--")).cloned()).flatten())?;
            rex_sock::validate_screen_name(&name).ok()?;
            if is_new {
                let pid = parsed.as_ref().and_then(|v| v["pid"].as_i64()).and_then(|p| i32::try_from(p).ok());
                let started = parsed.as_ref().and_then(|v| v["started"].as_u64());
                Some(BridgeScreenAction::Started { name, pid, started })
            } else {
                Some(BridgeScreenAction::Mutated { name })
            }
        }
        "stop" => {
            let name = rest.iter().find(|a| !a.starts_with("--"))?.clone();
            rex_sock::validate_screen_name(&name).ok()?;
            Some(BridgeScreenAction::Stopped { name })
        }
        "delete" => {
            let name = rest.iter().find(|a| !a.starts_with("--"))?.clone();
            rex_sock::validate_screen_name(&name).ok()?;
            Some(BridgeScreenAction::Deleted { name })
        }
        other if DESKTOP_OWNER_ACTIONS.contains(&other) => {
            let name = rest.iter().find(|a| a.as_str() != "--quick" && !a.starts_with("--"))?.clone();
            rex_sock::validate_screen_name(&name).ok()?;
            Some(BridgeScreenAction::Mutated { name })
        }
        _ => None,
    }
}

#[derive(Deserialize)]
struct DesktopRuntimeScreen {
    name: String,
    pid: i32,
    #[serde(default)]
    started: u64,
}

fn desktop_proc_start_ticks(pid: i32) -> Option<u64> {
    if pid <= 0 {
        return None;
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    fields.get(19)?.parse().ok()
}

fn read_owner_file_at(dir_fd: &OwnedFd, file_name: &str) -> Option<ScreenOwner> {
    let bytes = read_bounded_regular_at(dir_fd, file_name, 64 * 1024)?;
    let owner = serde_json::from_slice::<ScreenOwner>(&bytes).ok()?;
    (!owner.block_id.is_empty()).then_some(owner)
}

fn read_desktop_runtime_screen_at(rt_fd: &OwnedFd, name: &str) -> Option<DesktopRuntimeScreen> {
    let bytes = read_bounded_regular_at(rt_fd, &format!("{name}.json"), 64 * 1024)?;
    let s = serde_json::from_slice::<DesktopRuntimeScreen>(&bytes).ok()?;
    (s.name == name).then_some(s)
}

fn desktop_gen_of(s: &DesktopRuntimeScreen) -> DesktopScreenGen {
    DesktopScreenGen { pid: s.pid, started: s.started, start_ticks: desktop_proc_start_ticks(s.pid) }
}

fn desktop_gen_matches(actual: &DesktopScreenGen, expected: &DesktopScreenGen) -> bool {
    if actual.pid != expected.pid || actual.started != expected.started {
        return false;
    }
    if let (Some(a), Some(e)) = (actual.start_ticks, expected.start_ticks)
        && a != e
    {
        return false;
    }
    true
}

fn owner_matches_running(owner: &ScreenOwner, s: &DesktopRuntimeScreen) -> bool {
    if owner.screen_pid.is_some_and(|p| p != s.pid) {
        return false;
    }
    if owner.screen_started.is_some_and(|st| st != s.started) {
        return false;
    }
    if let Some(expected_ticks) = owner.screen_start_ticks
        && desktop_proc_start_ticks(s.pid).is_some_and(|actual| actual != expected_ticks)
    {
        return false;
    }
    true
}

/// Snapshot the currently running desktop screen generation for `name` (used
/// before executing a bridged `olo-screen` command).
pub fn snapshot_desktop_screen_gen(name: &str) -> Option<DesktopScreenGen> {
    if rex_sock::validate_screen_name(name).is_err() {
        return None;
    }
    let (rt_dir, _) = desktop_screen_dirs();
    let rt_fd = open_verified_owned_dir(&rt_dir, false).ok()?;
    let s = read_desktop_runtime_screen_at(&rt_fd, name)?;
    if !rex_sock::pid_is_alive(s.pid.max(0) as u32) {
        return None;
    }
    Some(desktop_gen_of(&s))
}

/// Record ownership of a desktop screen (`NAME.owner.json` in `rt_dir` and
/// `owner.json` under `state_root/NAME/`) for `(host_key, session_id, block_id)`.
///
/// Uses pinned directory descriptors (`O_DIRECTORY | O_NOFOLLOW`), a bounded
/// regular-file `flock`, atomic `openat(O_CREAT | O_EXCL)` + `renameat` writes,
/// generation matching against `expected_gen` so late completions never claim a
/// replacement session, and returns persistence errors cleanly instead of
/// silently updating memory.
#[allow(clippy::too_many_arguments)]
pub fn record_desktop_owner_in(
    rt_dir: &Path,
    state_root: &Path,
    name: &str,
    host_key: &str,
    session_id: &str,
    block_id: &str,
    is_start: bool,
    expected_gen: Option<DesktopScreenGen>,
) -> io::Result<Option<ScreenOwner>> {
    if rex_sock::validate_screen_name(name).is_err() || block_id.is_empty() {
        return Ok(None);
    }

    // Fail closed on untrusted directories (symlinks, foreign UID, loose mode);
    // only treat NotFound (when `!is_start`) as absent (Agy Finding 5).
    let rt_fd = match open_verified_owned_dir(rt_dir, is_start) {
        Ok(fd) => Some(fd),
        Err(e) if e.kind() == io::ErrorKind::NotFound && !is_start => None,
        Err(e) => return Err(e),
    };
    let state_root_fd = match open_verified_owned_dir(state_root, is_start) {
        Ok(fd) => Some(fd),
        Err(e) if e.kind() == io::ErrorKind::NotFound && !is_start => None,
        Err(e) => return Err(e),
    };

    let runtime_screen_exists = rt_fd.as_ref().is_some_and(|fd| regular_exists_at(fd, &format!("{name}.json")));
    // Verify `state_root/NAME` atomically without following symlinks!
    let state_dir_fd = match state_root_fd.as_ref() {
        Some(root_fd) => match open_verified_subdir_at(root_fd, name, is_start) {
            Ok(fd) => Some(fd),
            Err(e) if e.kind() == io::ErrorKind::NotFound && !is_start => None,
            Err(e) => return Err(e),
        },
        None => None,
    };
    let state_exists = state_dir_fd.is_some();
    if !is_start && !runtime_screen_exists && !state_exists {
        return Ok(None);
    }

    let _owner_lock = if let Some(ref sfd) = state_dir_fd {
        acquire_lock_at(sfd, ".owner.lock", Some(OWNER_LOCK_TIMEOUT))?
    } else if let Some(ref rfd) = rt_fd {
        acquire_lock_at(rfd, &format!(".{name}.owner.lock"), Some(OWNER_LOCK_TIMEOUT))?
    } else {
        return Err(io::Error::new(io::ErrorKind::NotFound, "no verified screens directory for owner lock"));
    };

    let rt_screen = rt_fd.as_ref().and_then(|fd| read_desktop_runtime_screen_at(fd, name));
    // Generation guard (Finding 29): if a caller observed or created a specific
    // running generation, refuse to attribute a different running session!
    if let Some(exp) = expected_gen {
        match rt_screen.as_ref() {
            Some(cur) => {
                let cur_gen = desktop_gen_of(cur);
                if !desktop_gen_matches(&cur_gen, &exp) {
                    return Ok(None);
                }
            }
            None if is_start => {
                // Started screen already exited before owner write: don't fabricate a live runtime sidecar.
                return Ok(None);
            }
            None => {}
        }
    }

    let rt_file = format!("{name}.owner.json");
    let rt_existing = rt_fd.as_ref().and_then(|fd| read_owner_file_at(fd, &rt_file));
    let st_existing = state_dir_fd.as_ref().and_then(|fd| read_owner_file_at(fd, "owner.json"));
    // Only inherit `started_by` from an existing owner that belongs to THIS running generation!
    let existing = match rt_screen.as_ref() {
        Some(cur) => rt_existing.as_ref().filter(|o| owner_matches_running(o, cur)).or_else(|| st_existing.as_ref().filter(|o| owner_matches_running(o, cur))),
        None => st_existing.as_ref().or(rt_existing.as_ref()),
    };

    let screen_pid = rt_screen.as_ref().map(|s| s.pid);
    let screen_started = rt_screen.as_ref().map(|s| s.started);
    let screen_start_ticks = screen_pid.and_then(desktop_proc_start_ticks);

    let started_by =
        if is_start { Some(OwnerPane { host_key: Some(host_key.to_owned()), session_id: session_id.to_owned(), block_id: block_id.to_owned() }) } else { existing.and_then(|o| o.started_by.clone()) };

    let need_runtime = (is_start || runtime_screen_exists) && rt_fd.is_some();
    let need_state = (is_start || state_exists) && state_dir_fd.is_some();
    let matches_target = |o: &ScreenOwner| {
        o.host_key.as_deref() == Some(host_key)
            && o.is_same_pane(session_id, block_id)
            && (!is_start || o.started_by == started_by)
            && (screen_pid.is_none() || o.screen_pid == screen_pid)
            && (screen_started.is_none() || o.screen_started == screen_started)
    };
    let rt_ok = !need_runtime || rt_existing.as_ref().is_some_and(matches_target);
    let st_ok = !need_state || st_existing.as_ref().is_some_and(matches_target);
    let copies_agree = !need_runtime || !need_state || rt_existing == st_existing;
    if rt_ok && st_ok && copies_agree {
        return Ok(rt_existing.or(st_existing));
    }

    if let Some(auth) = rt_existing.as_ref().filter(|o| matches_target(o)).or_else(|| st_existing.as_ref().filter(|o| matches_target(o))) {
        let bytes = serde_json::to_vec_pretty(auth).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if need_runtime
            && rt_existing.as_ref() != Some(auth)
            && let Some(ref fd) = rt_fd
        {
            write_file_atomic_at(fd, &rt_file, &bytes)?;
        }
        if need_state
            && st_existing.as_ref() != Some(auth)
            && let Some(ref fd) = state_dir_fd
        {
            write_file_atomic_at(fd, "owner.json", &bytes)?;
        }
        return Ok(Some(auth.clone()));
    }

    let owner = ScreenOwner {
        host_key: Some(host_key.to_owned()),
        session_id: session_id.to_owned(),
        block_id: block_id.to_owned(),
        started_by,
        pid: None,
        began: None,
        rex_socket: None,
        at: now_unix_ms(),
        screen_pid,
        screen_started,
        screen_start_ticks,
    };
    let bytes = serde_json::to_vec_pretty(&owner).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if need_runtime && let Some(ref fd) = rt_fd {
        write_file_atomic_at(fd, &rt_file, &bytes)?;
    }
    if need_state && let Some(ref fd) = state_dir_fd {
        write_file_atomic_at(fd, "owner.json", &bytes)?;
    }
    Ok(Some(owner))
}

/// Remove the runtime `NAME.owner.json` sidecar for a desktop screen only if no
/// newer replacement session has taken `name` (Finding 29 & 30). Returns `true`
/// if unlinked or already gone without a live replacement.
pub fn unlink_desktop_runtime_owner_in(rt_dir: &Path, state_root: &Path, name: &str, expected_gen: Option<DesktopScreenGen>) -> bool {
    if rex_sock::validate_screen_name(name).is_err() {
        return false;
    }
    // Fail closed on untrusted/unreadable `rt_dir` or `state_root` (only
    // treat NotFound as absent) so we never evict `desktop_screen_owners`
    // without verifying whether a replacement screen is running (Agy Finding 5).
    let rt_fd = match open_verified_owned_dir(rt_dir, false) {
        Ok(fd) => fd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return true,
        Err(_) => return false,
    };
    let state_dir_fd = match open_verified_owned_dir(state_root, false) {
        Ok(r) => match open_verified_subdir_at(&r, name, false) {
            Ok(fd) => Some(fd),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(_) => return false,
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(_) => return false,
    };
    let _lock = match state_dir_fd.as_ref() {
        Some(sfd) => match acquire_lock_at(sfd, ".owner.lock", Some(OWNER_LOCK_TIMEOUT)) {
            Ok(l) => l,
            Err(_) => return false,
        },
        None => match acquire_lock_at(&rt_fd, &format!(".{name}.owner.lock"), Some(OWNER_LOCK_TIMEOUT)) {
            Ok(l) => l,
            Err(_) => return false,
        },
    };
    if let Some(cur) = read_desktop_runtime_screen_at(&rt_fd, name)
        && rex_sock::pid_is_alive(cur.pid.max(0) as u32)
    {
        let cur_gen = desktop_gen_of(&cur);
        match expected_gen {
            Some(exp) if !desktop_gen_matches(&cur_gen, &exp) => return false,
            None => return false,
            Some(_) => {}
        }
    }
    let rt_file = format!("{name}.owner.json");
    if let Ok(cname) = c_name(&rt_file) {
        // SAFETY: valid dirfd and single component name.
        unsafe { libc::unlinkat(rt_fd.as_raw_fd(), cname.as_ptr(), 0) };
    }
    true
}

/// Default desktop screens runtime and state directories, matching
/// `olo_screen_pane::screens::dir()` and `olo_screen_pane::history::root_dir()`
/// (`$XDG_STATE_HOME/olo/screens`, since `olo_xdg::state_dir()` already ends in `olo`).
pub fn desktop_screen_dirs() -> (PathBuf, PathBuf) {
    (olo_screen_pane::screens::dir(), olo_xdg::state_dir().join("screens"))
}

/// Scan desktop screen owner records from runtime (`NAME.owner.json`) and
/// persistent state (`NAME/owner.json`), validating running screens against
/// their live `(pid, started, start_ticks)` generation and rejecting symlinks.
pub fn load_desktop_owners_in(rt_dir: &Path, state_root: &Path) -> HashMap<String, ScreenOwner> {
    let mut running: HashMap<String, DesktopRuntimeScreen> = HashMap::new();
    let rt_fd = open_verified_owned_dir(rt_dir, false).ok();
    if let (Some(rfd), Ok(rd)) = (rt_fd.as_ref(), std::fs::read_dir(rt_dir)) {
        for entry in rd.flatten() {
            let Ok(fname) = entry.file_name().into_string() else { continue };
            if !fname.ends_with(".json") || fname.ends_with("-settings.json") || fname.ends_with(".owner.json") {
                continue;
            }
            let Some(name) = fname.strip_suffix(".json") else { continue };
            if rex_sock::validate_screen_name(name).is_err() {
                continue;
            }
            if let Some(s) = read_desktop_runtime_screen_at(rfd, name)
                && rex_sock::pid_is_alive(s.pid.max(0) as u32)
            {
                running.insert(name.to_owned(), s);
            }
        }
    }

    let mut names: std::collections::HashSet<String> = running.keys().cloned().collect();
    let state_root_fd = open_verified_owned_dir(state_root, false).ok();
    let mut state_sub_fds: HashMap<String, OwnedFd> = HashMap::new();
    if let (Some(sroot), Ok(rd)) = (state_root_fd.as_ref(), std::fs::read_dir(state_root)) {
        for entry in rd.flatten() {
            if !entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                continue;
            }
            let Ok(name) = entry.file_name().into_string() else { continue };
            if rex_sock::validate_screen_name(&name).is_ok()
                && let Ok(sub_fd) = open_verified_subdir_at(sroot, &name, false)
                && regular_exists_at(&sub_fd, "owner.json")
                && (regular_exists_at(&sub_fd, "actions.jsonl") || regular_exists_at(&sub_fd, "screen.json"))
            {
                names.insert(name.clone());
                state_sub_fds.insert(name, sub_fd);
            }
        }
    }

    let mut out = HashMap::new();
    for name in names {
        let rt_owner = || rt_fd.as_ref().and_then(|fd| read_owner_file_at(fd, &format!("{name}.owner.json")));
        let st_owner = || state_sub_fds.get(&name).and_then(|fd| read_owner_file_at(fd, "owner.json"));
        let owner = match running.get(&name) {
            Some(rt) => rt_owner().filter(|o| owner_matches_running(o, rt)).or_else(|| st_owner().filter(|o| owner_matches_running(o, rt))),
            None => st_owner().or_else(rt_owner),
        };
        if let Some(o) = owner {
            out.insert(name, o);
        }
    }
    out
}

/// Apply a successful bridged `olo.screen` action to disk (`NAME.owner.json` +
/// `owner.json`) and `HubState::desktop_screen_owners`, guarding against stale
/// completions overwriting or unlinking a replacement screen generation.
pub fn record_bridge_screen_outcome(state: &Arc<Mutex<HubState>>, host_key: &str, session_id: Option<&str>, block_id: Option<&str>, args: &[String], stdout: &str, pre_gen: Option<DesktopScreenGen>) {
    let Some(action) = extract_bridge_screen_action(args, stdout) else { return };
    let (rt_dir, state_root) = desktop_screen_dirs();
    let block_id = block_id.unwrap_or_default();
    let session_id = session_id.unwrap_or_default();
    match action {
        BridgeScreenAction::Started { name, pid, started } => {
            let expected_gen = match (pid, started) {
                (Some(p), Some(s)) => Some(DesktopScreenGen { pid: p, started: s, start_ticks: None }),
                _ => None,
            };
            if block_id.is_empty() {
                if unlink_desktop_runtime_owner_in(&rt_dir, &state_root, &name, expected_gen) {
                    state.lock().unwrap_or_else(|e| e.into_inner()).desktop_screen_owners.remove(&name);
                }
                return;
            }
            match record_desktop_owner_in(&rt_dir, &state_root, &name, host_key, session_id, block_id, true, expected_gen) {
                Ok(Some(owner)) => {
                    state.lock().unwrap_or_else(|e| e.into_inner()).desktop_screen_owners.insert(name, owner);
                }
                Ok(None) => {}
                Err(e) => tracing::warn!("Rex desktop screen owner write for {name:?} failed: {e}"),
            }
        }
        BridgeScreenAction::Mutated { name } => {
            if block_id.is_empty() {
                return;
            }
            match record_desktop_owner_in(&rt_dir, &state_root, &name, host_key, session_id, block_id, false, pre_gen) {
                Ok(Some(owner)) => {
                    state.lock().unwrap_or_else(|e| e.into_inner()).desktop_screen_owners.insert(name, owner);
                }
                Ok(None) => {}
                Err(e) => tracing::warn!("Rex desktop screen owner update for {name:?} failed: {e}"),
            }
        }
        BridgeScreenAction::Stopped { name } => {
            let _ = unlink_desktop_runtime_owner_in(&rt_dir, &state_root, &name, pre_gen);
        }
        BridgeScreenAction::Deleted { name } => {
            if unlink_desktop_runtime_owner_in(&rt_dir, &state_root, &name, pre_gen) {
                state.lock().unwrap_or_else(|e| e.into_inner()).desktop_screen_owners.remove(&name);
            }
        }
    }
}

/// Filter `olo_kit::watch` change batches so irrelevant lock/settings/log files
/// do not trigger owner rescans, while directory-only batches (e.g. overflow or
/// directory creation/removal) still refresh cleanly.
pub(crate) fn is_desktop_owner_watch_relevant(rt_watch: &Path, state_watch: &Path, changed: &[PathBuf]) -> bool {
    if changed.is_empty() {
        return false;
    }
    let has_specific_children = changed.iter().any(|p| p != rt_watch && p != state_watch);
    if !has_specific_children {
        // Directory-only event (overflow or watched dir appeared/disappeared).
        return true;
    }
    // `olo_kit::watch` emits the watched directory alongside any touched child
    // inside it (`[state_root/NAME, state_root/NAME/.owner.lock]`). When a
    // grandchild under `state_root/NAME` is present in the batch, judge that
    // screen directory strictly by its children so `.owner.lock` / `.tmp`
    // writes inside a watched subdirectory never trigger spurious rescans.
    let subdirs_with_children: std::collections::HashSet<&Path> = changed.iter().filter_map(|p| p.parent().filter(|d| d.parent() == Some(state_watch))).collect();
    changed.iter().any(|p| {
        if p == rt_watch || p == state_watch {
            return false;
        }
        let Some(fname) = p.file_name().and_then(|n| n.to_str()) else { return false };
        if fname.ends_with(".tmp") || fname.ends_with(".lock") || fname.ends_with(".bus") {
            return false;
        }
        if p.parent() == Some(rt_watch) {
            if matches!(fname, "logs" | "views" | "no-sound") || fname.ends_with("-settings.json") {
                return false;
            }
            return fname.ends_with(".json");
        }
        if p.parent() == Some(state_watch) {
            if subdirs_with_children.contains(p.as_path()) {
                return false;
            }
            return rex_sock::validate_screen_name(fname).is_ok();
        }
        if p.parent().and_then(Path::parent) == Some(state_watch) {
            return matches!(fname, "owner.json" | "actions.jsonl" | "screen.json");
        }
        false
    })
}

/// Owned handle for [`watch_desktop_screen_owners`]: dropping it stops both
/// the `olo_kit::watch::Watch` and the background refresher task cleanly.
pub struct DesktopOwnersWatch {
    watch: Arc<Mutex<Option<olo_kit::watch::Watch>>>,
    shutdown_tx: watch::Sender<bool>,
    #[cfg(test)]
    init_done: watch::Receiver<bool>,
    #[cfg(test)]
    task_done: Arc<Notify>,
}

impl Drop for DesktopOwnersWatch {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send_replace(true);
        *self.watch.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Collect `[rt_dir, state_root]` plus verified existing screen subdirectories
/// under `state_root` (bounded by `MAX_SCREENS_PER_HOST`), because
/// `olo_kit::watch` is non-recursive and needs explicit watches on each
/// `state_root/<NAME>` directory to see updates to `owner.json` inside it
/// (Agy Finding 3).
fn desktop_owner_watch_paths(rt_dir: &Path, state_root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![rt_dir.to_path_buf(), state_root.to_path_buf()];
    if let (Ok(sroot), Ok(rd)) = (open_verified_owned_dir(state_root, false), std::fs::read_dir(state_root)) {
        let mut subdirs: Vec<String> = rd
            .flatten()
            .filter_map(|entry| {
                if !entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                    return None;
                }
                let name = entry.file_name().into_string().ok()?;
                if rex_sock::validate_screen_name(&name).is_ok() && open_verified_subdir_at(&sroot, &name, false).is_ok() { Some(name) } else { None }
            })
            .collect();
        subdirs.sort();
        subdirs.truncate(MAX_SCREENS_PER_HOST);
        for name in subdirs {
            paths.push(state_root.join(name));
        }
    }
    paths
}

/// Watch `$XDG_RUNTIME_DIR/olo-screens/`, `$XDG_STATE_HOME/olo/screens/`, and
/// each verified screen subdirectory inside `$XDG_STATE_HOME/olo/screens/`
/// with `olo_kit::watch` (zero polling) BEFORE triggering the initial scan, and
/// serialize all scans onto a background blocking task so neither startup nor
/// the shared `kit-watch` thread ever runs directory scans synchronously.
pub fn watch_desktop_screen_owners(rt: tokio::runtime::Handle, state: Arc<Mutex<HubState>>, notify: Arc<dyn Fn(HubNotice) + Send + Sync>) -> DesktopOwnersWatch {
    let (rt_dir, state_root) = desktop_screen_dirs();
    watch_desktop_screen_owners_in(rt, rt_dir, state_root, state, notify)
}

pub(crate) fn watch_desktop_screen_owners_in(
    rt: tokio::runtime::Handle,
    rt_dir: PathBuf,
    state_root: PathBuf,
    state: Arc<Mutex<HubState>>,
    notify: Arc<dyn Fn(HubNotice) + Send + Sync>,
) -> DesktopOwnersWatch {
    let dirty = Arc::new(AtomicBool::new(true));
    let wake = Arc::new(Notify::new());
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    #[cfg(test)]
    let (init_done_tx, init_done) = watch::channel(false);
    #[cfg(test)]
    let task_done = Arc::new(Notify::new());
    #[cfg(test)]
    let task_done_bg = task_done.clone();

    let rt_watch = rt_dir.clone();
    let state_watch = state_root.clone();
    let dirty_cb = dirty.clone();
    let wake_cb = wake.clone();

    // Register ONLY `[rt_dir, state_root]` synchronously so startup never does
    // synchronous directory reads on the UI thread; the background refresher
    // discovers verified `state_root/<NAME>` subdirs and updates `Watch::set`
    // BEFORE running `load_desktop_owners_in` on every pass (Round 2 Finding 2).
    let initial_paths = vec![rt_dir.clone(), state_root.clone()];
    let watch = Arc::new(Mutex::new(olo_kit::watch::watch(initial_paths.clone(), move |changed| {
        if is_desktop_owner_watch_relevant(&rt_watch, &state_watch, changed) {
            dirty_cb.store(true, Ordering::Release);
            wake_cb.notify_one();
        }
    })));
    let watch_bg = watch.clone();

    // Single serialized background refresher (0 wakeups/s at rest; exits cleanly on drop).
    let rt_scan = rt_dir;
    let state_scan = state_root;
    rt.spawn(async move {
        let mut tracked_paths = initial_paths;
        loop {
            if *shutdown_rx.borrow() {
                break;
            }
            while dirty.swap(false, Ordering::AcqRel) {
                if *shutdown_rx.borrow() {
                    break;
                }
                let r = rt_scan.clone();
                let s = state_scan.clone();
                let watch_io = watch_bg.clone();
                let prev_paths = tracked_paths.clone();
                let Ok((next, next_paths)) = tokio::task::spawn_blocking(move || {
                    // Discover subdirs and install inotify watches via `Watch::set`
                    // BEFORE reading owner files so any write to `owner.json` inside
                    // a newly created subdir is either seen by this scan or caught
                    // by the already-installed watch (Round 2 Finding 2).
                    let paths = desktop_owner_watch_paths(&r, &s);
                    if paths != prev_paths
                        && let Some(w) = watch_io.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
                    {
                        w.set(paths.clone());
                    }
                    let owners = load_desktop_owners_in(&r, &s);
                    (owners, paths)
                })
                .await
                else {
                    break;
                };
                if *shutdown_rx.borrow() {
                    break;
                }
                tracked_paths = next_paths;
                let changed = {
                    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                    if st.desktop_screen_owners == next {
                        false
                    } else {
                        st.desktop_screen_owners = next;
                        true
                    }
                };
                if changed {
                    notify(HubNotice::StateChanged);
                }
                #[cfg(test)]
                let _ = init_done_tx.send_replace(true);
            }
            if *shutdown_rx.borrow() {
                break;
            }
            tokio::select! {
                _ = wake.notified() => {}
                _ = wait_true(&mut shutdown_rx) => break,
            }
        }
        #[cfg(test)]
        task_done_bg.notify_waiters();
    });

    DesktopOwnersWatch {
        watch,
        shutdown_tx,
        #[cfg(test)]
        init_done,
        #[cfg(test)]
        task_done,
    }
}

// ---------------------------------------------------------------------------
// ScreensService: tracked host screens, follow tasks, protocol cache, state file
// ---------------------------------------------------------------------------

/// Live per-host screens snapshot carried into [`HubState::host_screens`] and
/// merged monotonically into the Host window's `ScreensThere`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveHostScreens {
    pub host_instance_id: u64,
    pub conn_id: u64,
    pub rev: u64,
    pub has_follow_snapshot: bool,
    pub status: Option<HostScreenStatus>,
    pub screens: Vec<RexScreen>,
}

/// Persisted last-known host screens snapshot carried across in-place Rex
/// upgrades (`SavedAppState`), matched strictly by `(host_key, endpoint_raw)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedHostScreens {
    pub host_key: String,
    pub endpoint_raw: String,
    pub host: RexHost,
}

/// Ordered update delivered from [`ScreensService`]'s single background
/// publisher to `HubState` / Host window.
#[derive(Clone, Debug)]
pub struct HostScreensUpdate {
    pub host_key: String,
    pub host_instance_id: u64,
    pub conn_id: u64,
    pub rev: u64,
    pub has_follow_snapshot: bool,
    pub status: Option<HostScreenStatus>,
    pub screens: Vec<RexScreen>,
}

/// Lightweight live `last_used` update delivered to `HubState::host_screens`
/// without rewriting disk, carrying `(host_instance_id, conn_id)` so a delayed
/// tick can never touch a replacement host's row (Finding 15).
#[derive(Clone, Debug)]
pub struct LastUsedUpdate {
    pub host_key: String,
    pub host_instance_id: u64,
    pub conn_id: u64,
    pub name: String,
    pub last_used: u64,
    pub actions: Option<usize>,
}

type OwnerResolver = Arc<dyn Fn(&str, &OwnerPane) -> Option<String> + Send + Sync>;
type HostScreensCallback = Arc<dyn Fn(HostScreensUpdate) + Send + Sync>;
type HostRemovedCallback = Arc<dyn Fn(String, u64) + Send + Sync>;
type LastUsedCallback = Arc<dyn Fn(LastUsedUpdate) + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
struct CachedProtocol {
    conn_id: u64,
    helper_gen: u64,
    install_gen: u64,
    installed: Option<String>,
    committed: Option<u64>,
    proto: u32,
    commands: Vec<String>,
    capabilities: Vec<String>,
}

#[derive(Clone, Debug)]
struct BroadcastEvent {
    seq: u64,
    line: RexFollowLine,
}

enum PendingPublishOp {
    Host { rev: u64, raw: Box<RawHostSnapshot> },
    Removed { rev: u64, key: String, host_instance_id: u64 },
    LastUsed { host: String, host_instance_id: u64, conn_id: u64, name: String, last_used: u64, actions: Option<usize> },
}

/// Resources owned by a running [`ScreensService`] listener, held under the
/// per-display `flock` until `shutdown()` finishes cleaning up.
struct OwnedServiceFiles {
    sock_dir_fd: OwnedFd,
    sock_name: String,
    sock_inode: (u64, u64),
    state_dir_fd: OwnedFd,
    state_name: String,
    _lock_fd: OwnedFd,
}

struct TrackedHost {
    key: String,
    host_instance_id: u64,
    label: String,
    /// True connection state (`Connected` as soon as Rex's SSH host connects).
    conn_state: RexHostConn,
    /// True ONLY while an active `follow` stream has delivered a full
    /// `FollowEvent::Snapshot` for this `conn_id`; cleared immediately when the
    /// follow stream closes or restarts so stale rows never outrank Check Again
    /// or live freshness gates (Findings 12 & 26).
    has_live_follow: bool,
    /// True when the connected host's helper is confirmed incompatible (lacks
    /// `screen follow`), tracked separately from `has_live_follow`.
    helper_known_incompatible: bool,
    error: Option<String>,
    disconnected_at: Option<u64>,
    conn_id: u64,
    /// Monotonic watermark of the highest `conn_id` ended by a disconnect, so a
    /// disconnect that arrives before its `host_connected(conn_id)` is never lost
    /// (Finding 14).
    last_ended_conn_id: u64,
    /// Monotonic state sequence from `HostRuntime::screens_state_seq` so
    /// delayed unversioned state transitions (`Connecting`/`Reconnecting`/
    /// `Disconnected`/`Error`) can never overwrite a newer transition or kill a
    /// live connection (Agy Finding 2).
    last_state_seq: u64,
    helper_gen: u64,
    /// Monotonic generation bumped whenever helper availability or installed
    /// `(version, committed)` changes (Finding 20).
    install_gen: u64,
    pinned: Option<Arc<PinnedSshTarget>>,
    follow_status: Option<HostScreenStatus>,
    lacking_reason: Option<String>,
    helper_has_screens: Option<bool>,
    protocol: Option<CachedProtocol>,
    screens: BTreeMap<String, HostScreen>,
    /// Last published `controlled_by` per screen name (so `refresh_owner_labels`
    /// only bumps `rev` when a label actually changes).
    last_published_owners: HashMap<String, Option<String>>,
    /// True when this host's raw screen payload exceeded `MAX_HOST_SCREENS_BYTES`.
    screens_truncated: bool,
    follower: Option<tokio::task::AbortHandle>,
    follower_wake: Arc<Notify>,
    snapshot_ready: Arc<Notify>,
    relay_cancel: watch::Sender<bool>,
    relay_permits: Arc<Semaphore>,
    protocol_probe_lock: Arc<tokio::sync::Mutex<()>>,
    #[cfg(test)]
    probe_wait_notify: Arc<Notify>,
}

impl TrackedHost {
    fn new(key: String, host_instance_id: u64, label: String, conn_state: RexHostConn) -> Self {
        let (relay_cancel, _) = watch::channel(false);
        Self {
            key,
            host_instance_id,
            label,
            conn_state,
            has_live_follow: false,
            helper_known_incompatible: false,
            error: None,
            disconnected_at: None,
            conn_id: 0,
            last_ended_conn_id: 0,
            last_state_seq: 0,
            helper_gen: 1,
            install_gen: 1,
            pinned: None,
            follow_status: None,
            lacking_reason: None,
            helper_has_screens: None,
            protocol: None,
            screens: BTreeMap::new(),
            last_published_owners: HashMap::new(),
            screens_truncated: false,
            follower: None,
            follower_wake: Arc::new(Notify::new()),
            snapshot_ready: Arc::new(Notify::new()),
            relay_cancel,
            relay_permits: Arc::new(Semaphore::new(MAX_RELAYS_PER_HOST)),
            protocol_probe_lock: Arc::new(tokio::sync::Mutex::new(())),
            #[cfg(test)]
            probe_wait_notify: Arc::new(Notify::new()),
        }
    }

    fn current_installed(&self) -> Option<String> {
        self.follow_status.as_ref().and_then(|s| s.installed.as_ref()).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
    }

    fn current_committed(&self) -> Option<u64> {
        self.follow_status.as_ref().and_then(|s| s.committed)
    }

    fn current_install_key(&self) -> (Option<String>, Option<u64>) {
        (self.current_installed(), self.current_committed())
    }

    fn is_fresh_for_rpc(&self) -> bool {
        self.has_live_follow || self.helper_known_incompatible
    }

    fn published_conn_state(&self) -> RexHostConn {
        if self.conn_state == RexHostConn::Connected && !self.is_fresh_for_rpc() { RexHostConn::Connecting } else { self.conn_state }
    }

    fn cancel_relays_and_follower(&mut self) {
        if let Some(h) = self.follower.take() {
            h.abort();
        }
        let _ = self.relay_cancel.send_replace(true);
        let (fresh, _) = watch::channel(false);
        self.relay_cancel = fresh;
        self.snapshot_ready.notify_waiters();
    }

    fn screen_target(&self) -> ScreenTarget {
        ScreenTarget { host_key: self.key.clone(), host_instance_id: self.host_instance_id, conn_id: self.conn_id, pinned: self.pinned.clone() }
    }

    fn raw_snapshot(&self) -> RawHostSnapshot {
        RawHostSnapshot {
            key: self.key.clone(),
            host_instance_id: self.host_instance_id,
            conn_id: self.conn_id,
            label: self.label.clone(),
            state: self.published_conn_state(),
            has_follow_snapshot: self.has_live_follow && self.follow_status.is_some(),
            error: self.error.clone().or_else(|| self.screens_truncated.then(|| TOO_MANY_SCREENS_REASON.to_owned())),
            disconnected_at: self.disconnected_at,
            follow_status: self.follow_status.clone(),
            lacking_reason: self.lacking_reason.clone(),
            helper_has_screens: self.helper_has_screens,
            protocol: self.protocol.clone(),
            screens: self.screens.values().cloned().collect(),
        }
    }
}

struct RawHostSnapshot {
    key: String,
    host_instance_id: u64,
    conn_id: u64,
    label: String,
    state: RexHostConn,
    has_follow_snapshot: bool,
    error: Option<String>,
    disconnected_at: Option<u64>,
    follow_status: Option<HostScreenStatus>,
    lacking_reason: Option<String>,
    helper_has_screens: Option<bool>,
    protocol: Option<CachedProtocol>,
    screens: Vec<HostScreen>,
}

impl RawHostSnapshot {
    fn into_rex_host(self, resolver: &OwnerResolver) -> RexHost {
        let installed = self.follow_status.as_ref().and_then(|s| s.installed.clone()).filter(|v| !v.trim().is_empty());
        let committed = self.follow_status.as_ref().and_then(|s| s.committed);
        let default_on_host = self.follow_status.as_ref().is_some_and(|s| s.default_on_host);
        let follow_reason = self.follow_status.as_ref().and_then(|s| s.not_set_up_reason.clone()).filter(|v| !v.trim().is_empty());
        let capabilities = self.protocol.as_ref().map(|p| p.capabilities.clone()).unwrap_or_default();

        let (can_control, control_disabled_reason) = compute_can_control(installed.is_some(), self.protocol.as_ref());

        let not_set_up_reason = if installed.is_none() {
            if self.helper_has_screens == Some(false) {
                Some(OLD_HELPER_REASON.to_owned())
            } else if let Some(reason) = follow_reason {
                Some(truncate_utf8(&reason, MAX_ERROR_LEN))
            } else if let Some(lacking) = self.lacking_reason {
                Some(truncate_utf8(&lacking, MAX_ERROR_LEN))
            } else {
                Some(NOT_INSTALLED_REASON.to_owned())
            }
        } else {
            follow_reason.or(self.lacking_reason).map(|r| truncate_utf8(&r, MAX_ERROR_LEN))
        };

        let mut screens: Vec<RexScreen> =
            self.screens.into_iter().filter(|s| rex_sock::validate_screen_name(&s.name).is_ok()).take(MAX_SCREENS_PER_HOST).map(|s| host_screen_to_rex_screen(&self.key, s, resolver)).collect();
        sort_rex_screens(&mut screens);

        let cleaned = sanitize_label(&self.label, MAX_LABEL_LEN);
        let label = if cleaned.trim().is_empty() { sanitize_label(&self.key, MAX_LABEL_LEN) } else { cleaned };

        RexHost {
            key: self.key,
            label,
            state: self.state,
            error: self.error.map(|e| truncate_utf8(&e, MAX_ERROR_LEN)),
            disconnected_at: self.disconnected_at,
            installed,
            not_set_up_reason,
            committed,
            default_on_host,
            can_control,
            control_disabled_reason,
            capabilities,
            screens,
        }
    }
}

/// Sanitize and cap an ingested [`OwnerPane`] so untrusted host metadata cannot
/// bloat memory or spoof another host's key.
fn sanitize_owner_pane(mut pane: OwnerPane, host_key: &str) -> Option<OwnerPane> {
    let block_id = sanitize_label(&pane.block_id, MAX_HOST_KEY_LEN);
    if block_id.is_empty() {
        return None;
    }
    pane.host_key = Some(host_key.to_owned());
    pane.session_id = sanitize_label(&pane.session_id, MAX_HOST_KEY_LEN);
    pane.block_id = block_id;
    Some(pane)
}

/// Sanitize and bound an ingested [`HostScreen`] before storing it in memory.
fn sanitize_host_screen(host_key: &str, mut s: HostScreen) -> Option<HostScreen> {
    if rex_sock::validate_screen_name(&s.name).is_err() {
        return None;
    }
    s.title = s.title.map(|t| sanitize_label(&t, MAX_TITLE_LEN)).filter(|t| !t.trim().is_empty());
    s.width = s.width.min(MAX_SCREEN_DIM);
    s.height = s.height.min(MAX_SCREEN_DIM);
    if !s.scale.is_finite() || !(0.25..=8.0).contains(&s.scale) {
        s.scale = 1.0;
    }
    s.owner = s.owner.and_then(|o| sanitize_owner_pane(o, host_key));
    s.started_by = s.started_by.and_then(|o| sanitize_owner_pane(o, host_key));
    Some(s)
}

fn sanitize_host_status(mut status: HostScreenStatus) -> HostScreenStatus {
    status.installed = status.installed.map(|v| sanitize_label(&v, MAX_LABEL_LEN)).filter(|v| !v.trim().is_empty());
    status.not_set_up_reason = status.not_set_up_reason.map(|v| sanitize_label(&v, MAX_ERROR_LEN)).filter(|v| !v.trim().is_empty());
    status
}

/// Rough upper bound of serialized bytes for one [`HostScreen`] so ingestion
/// enforces per-host memory budgets in O(1) without serializing on every tick.
fn est_host_screen_bytes(s: &HostScreen) -> usize {
    let base = 240usize + s.name.len() + s.title.as_ref().map_or(0, String::len);
    let pane_len = |p: &Option<OwnerPane>| p.as_ref().map_or(0, |o| 64 + o.host_key.as_ref().map_or(0, String::len) + o.session_id.len() + o.block_id.len());
    base + pane_len(&s.owner) + pane_len(&s.started_by)
}

fn host_screens_est_bytes(screens: &BTreeMap<String, HostScreen>) -> usize {
    screens.values().map(est_host_screen_bytes).sum()
}

fn compute_can_control(installed: bool, proto: Option<&CachedProtocol>) -> (Option<bool>, Option<String>) {
    if !installed {
        return (None, None);
    }
    let Some(p) = proto else {
        return (None, None);
    };
    let has_cap = |a: &str, b: &str| p.capabilities.iter().any(|c| c == a || c == b);
    let has_control_ack = has_cap("control-ack", "control_ack");
    // Require explicit `control-lock` (from `host_cli_honors_helper_control()`);
    // never infer lock support from `discontiguous_part`.
    let has_control_lock = has_cap("control-lock", "control_lock");
    if p.proto >= 1 && has_control_ack && has_control_lock { (Some(true), None) } else { (Some(false), Some(OLD_OLO_CONTROL_REASON.to_owned())) }
}

fn host_screen_to_rex_screen(host_key: &str, s: HostScreen, resolver: &OwnerResolver) -> RexScreen {
    let actions = s.actions;
    let mut rs = RexScreen::from(s);
    rs.actions = actions;
    if let Some(ref mut title) = rs.title {
        let cleaned = sanitize_label(title, MAX_TITLE_LEN);
        if cleaned.trim().is_empty() {
            rs.title = None;
        } else {
            *title = cleaned;
        }
    }
    // Always bind remote host-reported pane IDs strictly to the authenticated host.
    if let Some(ref mut owner) = rs.owner {
        owner.host_key = Some(host_key.to_owned());
    }
    if let Some(ref mut started_by) = rs.started_by {
        started_by.host_key = Some(host_key.to_owned());
    }
    rs.controlled_by = rs.owner.as_ref().and_then(|o| resolver(host_key, o)).or_else(|| rs.started_by.as_ref().and_then(|o| resolver(host_key, o)));
    rs
}

fn sort_rex_screens(screens: &mut [RexScreen]) {
    screens.sort_by(|a, b| {
        let rank = |s: HostScreenState| match s {
            HostScreenState::Running => 0u8,
            HostScreenState::Stopped => 1u8,
            HostScreenState::Archived => 2u8,
        };
        (rank(a.state), std::cmp::Reverse(a.started.filter(|_| a.is_running())), &a.name).cmp(&(rank(b.state), std::cmp::Reverse(b.started.filter(|_| b.is_running())), &b.name))
    });
}

/// Ensure a complete [`RexScreensState`] always satisfies `MAX_TOTAL_SCREENS`
/// and `MAX_STATE_FILE_BYTES`, trimming only the largest host(s) if an aggregate
/// snapshot would otherwise fail validation and take every host offline.
fn enforce_aggregate_snapshot_budget(state: &mut RexScreensState) {
    // 1. Enforce `MAX_TOTAL_SCREENS` across all hosts by trimming the largest host(s).
    let mut total: usize = state.hosts.iter().map(|h| h.screens.len()).sum();
    while total > MAX_TOTAL_SCREENS {
        let Some(idx) = state.hosts.iter().enumerate().max_by_key(|(_, h)| h.screens.len()).map(|(i, _)| i) else { break };
        if state.hosts[idx].screens.is_empty() {
            break;
        }
        let excess = total - MAX_TOTAL_SCREENS;
        let drop_n = excess.min(state.hosts[idx].screens.len());
        let new_len = state.hosts[idx].screens.len() - drop_n;
        state.hosts[idx].screens.truncate(new_len);
        if state.hosts[idx].error.is_none() {
            state.hosts[idx].error = Some(TOO_MANY_SCREENS_REASON.to_owned());
        }
        total -= drop_n;
    }

    // 2. Enforce `MAX_STATE_FILE_BYTES` (and `MAX_RESPONSE_LINE_BYTES`) if ever exceeded.
    while serde_json::to_vec_pretty(state).map_or(true, |b| b.len() > MAX_STATE_FILE_BYTES) {
        let Some(idx) = state.hosts.iter().enumerate().filter(|(_, h)| !h.screens.is_empty()).max_by_key(|(_, h)| h.screens.len()).map(|(i, _)| i) else {
            break;
        };
        let keep = state.hosts[idx].screens.len() / 2;
        state.hosts[idx].screens.truncate(keep);
        if state.hosts[idx].error.is_none() {
            state.hosts[idx].error = Some(TOO_MANY_SCREENS_REASON.to_owned());
        }
    }
}

struct ServiceState {
    rev: u64,
    seq: u64,
    host_order: Vec<String>,
    hosts: HashMap<String, TrackedHost>,
    /// Bounded, strictly `seq`-ordered pending publication map populated INSIDE
    /// the same critical section that assigns `seq` (Findings 9, 15, 24, 35):
    /// holds at most ONE pending structural update per host and at most ONE
    /// pending `LastUsed` per `(host, screen)`.
    pending_ops: BTreeMap<u64, PendingPublishOp>,
    pending_host_seq: HashMap<String, u64>,
    pending_last_used_seq: HashMap<(String, String), u64>,
}

impl ServiceState {
    fn find_host_key(&self, query: &str) -> Option<String> {
        if self.hosts.contains_key(query) {
            return Some(query.to_owned());
        }
        if let Some(k) = self.host_order.iter().find(|k| self.hosts.get(*k).is_some_and(|h| h.label == query)) {
            return Some(k.clone());
        }
        self.host_order.iter().find(|k| self.hosts.get(*k).is_some_and(|h| h.label.eq_ignore_ascii_case(query))).cloned()
    }

    fn queue_host(&mut self, raw: RawHostSnapshot) {
        self.rev += 1;
        self.seq += 1;
        let (seq, rev) = (self.seq, self.rev);
        if let Some(old_seq) = self.pending_host_seq.insert(raw.key.clone(), seq) {
            self.pending_ops.remove(&old_seq);
        }
        self.pending_ops.insert(seq, PendingPublishOp::Host { rev, raw: Box::new(raw) });
    }

    fn queue_removed(&mut self, key: String, host_instance_id: u64) {
        self.rev += 1;
        self.seq += 1;
        let (seq, rev) = (self.seq, self.rev);
        if let Some(old_seq) = self.pending_host_seq.insert(key.clone(), seq) {
            self.pending_ops.remove(&old_seq);
        }
        let stale_lu: Vec<(String, String)> = self.pending_last_used_seq.keys().filter(|(h, _)| h == &key).cloned().collect();
        for k in stale_lu {
            if let Some(old_seq) = self.pending_last_used_seq.remove(&k) {
                self.pending_ops.remove(&old_seq);
            }
        }
        self.pending_ops.insert(seq, PendingPublishOp::Removed { rev, key, host_instance_id });
    }

    fn queue_last_used(&mut self, host: String, host_instance_id: u64, conn_id: u64, name: String, last_used: u64, actions: Option<usize>) {
        self.seq += 1;
        let seq = self.seq;
        if let Some(old_seq) = self.pending_last_used_seq.insert((host.clone(), name.clone()), seq) {
            self.pending_ops.remove(&old_seq);
        }
        self.pending_ops.insert(seq, PendingPublishOp::LastUsed { host, host_instance_id, conn_id, name, last_used, actions });
    }

    fn pop_next_pending(&mut self) -> Option<(u64, PendingPublishOp)> {
        let (seq, op) = self.pending_ops.pop_first()?;
        match &op {
            PendingPublishOp::Host { raw, .. } => {
                if self.pending_host_seq.get(&raw.key).copied() == Some(seq) {
                    self.pending_host_seq.remove(&raw.key);
                }
            }
            PendingPublishOp::Removed { key, .. } => {
                if self.pending_host_seq.get(key).copied() == Some(seq) {
                    self.pending_host_seq.remove(key);
                }
            }
            PendingPublishOp::LastUsed { host, name, .. } => {
                let k = (host.clone(), name.clone());
                if self.pending_last_used_seq.get(&k).copied() == Some(seq) {
                    self.pending_last_used_seq.remove(&k);
                }
            }
        }
        Some((seq, op))
    }
}

/// Core state and broadcast coordinator for Rex's remote screens service.
pub struct ScreensService {
    socket_path: PathBuf,
    state_path: PathBuf,
    pid: u32,
    write_files: AtomicBool,
    publisher_started: AtomicBool,
    rpc_timeout: Mutex<Duration>,
    handshake_timeout: Mutex<Duration>,
    follow_open_timeout: Mutex<Duration>,
    follow_stable_window: Mutex<Duration>,
    client_write_timeout: Mutex<Duration>,
    opener: Arc<dyn ScreenOpener>,
    owner_resolver: OwnerResolver,
    on_host_screens: Option<HostScreensCallback>,
    on_host_removed: Mutex<Option<HostRemovedCallback>>,
    on_last_used: Option<LastUsedCallback>,
    tx: broadcast::Sender<BroadcastEvent>,
    pub_notify: Arc<Notify>,
    disk_notify: Arc<Notify>,
    shutdown_tx: watch::Sender<bool>,
    client_permits: Arc<Semaphore>,
    inner: Mutex<ServiceState>,
    owned_files: Mutex<Option<OwnedServiceFiles>>,
    file_lock: Mutex<()>,
}

impl ScreensService {
    pub fn new(
        socket_path: PathBuf,
        state_path: PathBuf,
        pid: u32,
        opener: Arc<dyn ScreenOpener>,
        owner_resolver: OwnerResolver,
        on_host_screens: Option<HostScreensCallback>,
        on_last_used: Option<LastUsedCallback>,
    ) -> Arc<Self> {
        let (tx, _) = broadcast::channel(FOLLOW_BROADCAST_CAP);
        let (shutdown_tx, _) = watch::channel(false);
        Arc::new(Self {
            socket_path,
            state_path,
            pid,
            write_files: AtomicBool::new(false),
            publisher_started: AtomicBool::new(false),
            rpc_timeout: Mutex::new(RPC_TIMEOUT),
            handshake_timeout: Mutex::new(HANDSHAKE_TIMEOUT),
            follow_open_timeout: Mutex::new(FOLLOW_OPEN_TIMEOUT),
            follow_stable_window: Mutex::new(FOLLOW_STABLE_WINDOW),
            client_write_timeout: Mutex::new(CLIENT_WRITE_TIMEOUT),
            opener,
            owner_resolver,
            on_host_screens,
            on_host_removed: Mutex::new(None),
            on_last_used,
            tx,
            pub_notify: Arc::new(Notify::new()),
            disk_notify: Arc::new(Notify::new()),
            shutdown_tx,
            client_permits: Arc::new(Semaphore::new(MAX_CONCURRENT_CLIENTS)),
            inner: Mutex::new(ServiceState {
                rev: 1,
                seq: 1,
                host_order: Vec::new(),
                hosts: HashMap::new(),
                pending_ops: BTreeMap::new(),
                pending_host_seq: HashMap::new(),
                pending_last_used_seq: HashMap::new(),
            }),
            owned_files: Mutex::new(None),
            file_lock: Mutex::new(()),
        })
    }

    /// Register a callback invoked whenever a host is removed so `HubState`
    /// cleans up `host_screens` for the removed host instance (Finding 14).
    pub fn set_on_host_removed(&self, cb: HostRemovedCallback) {
        *self.on_host_removed.lock().unwrap_or_else(|e| e.into_inner()) = Some(cb);
    }

    fn current_rpc_timeout(&self) -> Duration {
        *self.rpc_timeout.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn current_handshake_timeout(&self) -> Duration {
        *self.handshake_timeout.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn current_follow_open_timeout(&self) -> Duration {
        *self.follow_open_timeout.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn current_follow_stable_window(&self) -> Duration {
        *self.follow_stable_window.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn current_client_write_timeout(&self) -> Duration {
        *self.client_write_timeout.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Populate initial SSH-eligible remote hosts in canonical `host_order`
    /// before the socket is bound (does not write disk or broadcast yet).
    pub fn set_initial_hosts(&self, hosts: Vec<(String, u64, String, RexHostConn)>) {
        let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        st.host_order.clear();
        st.hosts.clear();
        for (key, instance_id, label, conn) in hosts {
            if rex_sock::validate_host_key(&key).is_err() || st.hosts.contains_key(&key) {
                continue;
            }
            if st.host_order.len() >= MAX_HOSTS {
                break;
            }
            st.host_order.push(key.clone());
            st.hosts.insert(key.clone(), TrackedHost::new(key, instance_id, label, conn));
        }
    }

    /// Restore last-known host screens saved across an in-place Rex upgrade,
    /// strictly verifying matching `(host_key, endpoint_raw)` so edited hosts
    /// never inherit another machine's rows. Returns restored `(host_key, LiveHostScreens)`
    /// entries so `HubState::host_screens` is seeded immediately even for `let_go`
    /// hosts that will not reconnect (Finding 26).
    pub fn restore_cached_hosts(&self, restored: Vec<SavedHostScreens>, host_endpoints: &HashMap<String, String>) -> Vec<(String, LiveHostScreens)> {
        let mut restored_raw = Vec::new();
        {
            let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            for saved in restored {
                if host_endpoints.get(&saved.host_key).map(String::as_str) != Some(saved.endpoint_raw.as_str()) {
                    continue;
                }
                let Some(h) = st.hosts.get_mut(&saved.host_key) else { continue };
                h.disconnected_at = saved.host.disconnected_at.or_else(|| Some(now_unix_ms()));
                if saved.host.installed.is_some() || saved.host.not_set_up_reason.is_some() {
                    h.follow_status = Some(sanitize_host_status(HostScreenStatus {
                        installed: saved.host.installed,
                        committed: saved.host.committed,
                        default_on_host: saved.host.default_on_host,
                        not_set_up_reason: saved.host.not_set_up_reason,
                    }));
                }
                h.screens.clear();
                let mut bytes = 0usize;
                for rs in saved.host.screens.into_iter().take(MAX_SCREENS_PER_HOST) {
                    let hs = HostScreen {
                        name: rs.name,
                        title: rs.title,
                        state: rs.state,
                        width: rs.width,
                        height: rs.height,
                        scale: rs.scale,
                        sound: rs.sound,
                        pid: rs.pid,
                        started: rs.started,
                        last_used: rs.last_used,
                        actions: rs.actions,
                        controlled: false,
                        owner: rs.owner,
                        started_by: rs.started_by,
                    };
                    if let Some(clean) = sanitize_host_screen(&h.key, hs) {
                        let est = est_host_screen_bytes(&clean);
                        if bytes + est > MAX_HOST_SCREENS_BYTES {
                            h.screens_truncated = true;
                            break;
                        }
                        bytes += est;
                        h.screens.insert(clean.name.clone(), clean);
                    }
                }
                let raw = h.raw_snapshot();
                restored_raw.push((st.rev, raw));
            }
        }
        restored_raw
            .into_iter()
            .map(|(rev, raw)| {
                let host_key = raw.key.clone();
                let host_instance_id = raw.host_instance_id;
                let conn_id = raw.conn_id;
                let status = raw.follow_status.clone();
                let rex_host = raw.into_rex_host(&self.owner_resolver);
                (host_key, LiveHostScreens { host_instance_id, conn_id, rev, has_follow_snapshot: false, status, screens: rex_host.screens })
            })
            .collect()
    }

    /// Snapshot current tracked hosts for preservation across an in-place Rex
    /// upgrade (`SavedAppState`), marking every restored host as disconnected.
    pub fn saved_hosts_for_upgrade(&self, host_endpoints: &HashMap<String, String>) -> Vec<SavedHostScreens> {
        let snap = self.snapshot();
        let now = now_unix_ms();
        snap.hosts
            .into_iter()
            .filter_map(|mut host| {
                let ep = host_endpoints.get(&host.key)?.clone();
                if host.screens.is_empty() && host.installed.is_none() {
                    return None;
                }
                host.state = RexHostConn::Disconnected;
                if host.disconnected_at.is_none() {
                    host.disconnected_at = Some(now);
                }
                for sc in &mut host.screens {
                    sc.controlled = false;
                }
                Some(SavedHostScreens { host_key: host.key.clone(), endpoint_raw: ep, host })
            })
            .collect()
    }

    /// Build a complete, validated, budget-enforced [`RexScreensState`] snapshot
    /// along with its exact monotonic event sequence number `seq`.
    fn snapshot_with_seq(&self) -> (u64, RexScreensState) {
        let (rev, seq, raw_hosts): (u64, u64, Vec<RawHostSnapshot>) = {
            let st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let list = st.host_order.iter().filter_map(|k| st.hosts.get(k)).take(MAX_HOSTS).map(TrackedHost::raw_snapshot).collect();
            (st.rev, st.seq, list)
        };
        let hosts = raw_hosts.into_iter().map(|raw| raw.into_rex_host(&self.owner_resolver)).collect();
        let mut snap = RexScreensState { version: STATE_VERSION, pid: self.pid, socket: self.socket_path.to_string_lossy().into_owned(), rev, hosts };
        enforce_aggregate_snapshot_budget(&mut snap);
        (seq, snap)
    }

    /// Build a complete, validated [`RexScreensState`] snapshot.
    pub fn snapshot(&self) -> RexScreensState {
        self.snapshot_with_seq().1
    }

    fn flush_state_file_sync(&self) -> io::Result<()> {
        if !self.write_files.load(Ordering::Acquire) {
            return Ok(());
        }
        let _guard = self.file_lock.lock().unwrap_or_else(|e| e.into_inner());
        if !self.write_files.load(Ordering::Acquire) {
            return Ok(());
        }
        let snap = self.snapshot();
        let owned = self.owned_files.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(files) = owned.as_ref() { write_state_atomic_at(&files.state_dir_fd, &files.state_name, &snap) } else { Ok(()) }
    }

    /// Add a newly configured SSH remote host to canonical `host_order`.
    pub fn host_added(&self, key: &str, host_instance_id: u64, label: &str, conn: RexHostConn) {
        if rex_sock::validate_host_key(key).is_err() {
            return;
        }
        let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if st.hosts.contains_key(key) || st.host_order.len() >= MAX_HOSTS {
            return;
        }
        st.host_order.push(key.to_owned());
        let host = TrackedHost::new(key.to_owned(), host_instance_id, label.to_owned(), conn);
        let raw = host.raw_snapshot();
        st.hosts.insert(key.to_owned(), host);
        st.queue_host(raw);
        drop(st);
        self.pub_notify.notify_one();
    }

    /// Update a remote host's display label.
    pub fn host_renamed(&self, key: &str, host_instance_id: u64, label: &str) {
        let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(h) = st.hosts.get_mut(key) else { return };
        if h.host_instance_id != host_instance_id || h.label == label {
            return;
        }
        h.label = label.to_owned();
        let raw = h.raw_snapshot();
        st.queue_host(raw);
        drop(st);
        self.pub_notify.notify_one();
    }

    /// Remove a remote host from Rex: stops its follower, cancels active relays,
    /// removes it from `host_order`, broadcasts `HostRemoved`, and updates disk.
    pub fn host_removed(&self, key: &str, host_instance_id: u64) {
        let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if !st.hosts.get(key).is_some_and(|h| h.host_instance_id == host_instance_id) {
            return;
        }
        if let Some(mut h) = st.hosts.remove(key) {
            h.cancel_relays_and_follower();
        }
        st.host_order.retain(|k| k != key);
        st.queue_removed(key.to_owned(), host_instance_id);
        drop(st);
        self.pub_notify.notify_one();
    }

    /// Record an SSH remote host transitioning to `Connected` under its
    /// authoritative `(host_instance_id, conn_id)` and pinned endpoint.
    /// Never implicitly resurrects a removed or replaced host, and rejects any
    /// `conn_id` already ended by an earlier disconnect watermark (Finding 14).
    #[allow(clippy::too_many_arguments)]
    pub fn host_connected(self: &Arc<Self>, rt: &tokio::runtime::Handle, key: &str, host_instance_id: u64, conn_id: u64, state_seq: u64, label: &str, pinned: Option<Arc<PinnedSshTarget>>) {
        if rex_sock::validate_host_key(key).is_err() || conn_id == 0 {
            return;
        }
        let (wake, host_cancel, target) = {
            let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let Some(h) = st.hosts.get_mut(key) else { return };
            if h.host_instance_id != host_instance_id || conn_id <= h.conn_id || conn_id <= h.last_ended_conn_id || state_seq < h.last_state_seq || (state_seq != 0 && state_seq == h.last_state_seq) {
                return;
            }
            h.last_state_seq = h.last_state_seq.max(state_seq);
            h.cancel_relays_and_follower();
            h.label = label.to_owned();
            h.conn_state = RexHostConn::Connected;
            // Hold live status until the fresh follower snapshot arrives so
            // screens stopped while offline are never presented as live.
            h.has_live_follow = false;
            h.helper_known_incompatible = false;
            h.error = None;
            h.conn_id = conn_id;
            h.install_gen += 1;
            h.pinned = pinned;
            h.protocol = None;
            h.helper_has_screens = None;
            h.lacking_reason = None;
            let wake = h.follower_wake.clone();
            let host_cancel = h.relay_cancel.subscribe();
            let target = h.screen_target();
            let raw = h.raw_snapshot();
            st.queue_host(raw);
            (wake, host_cancel, target)
        };
        self.pub_notify.notify_one();

        // Spawn one zero-polling `olo-rex-link screen follow` stream for this connection.
        // `--protocol` is probed once the initial `Snapshot` arrives (or lazily on
        // the first RPC), avoiding duplicate probes across connect + snapshot (Finding 20).
        let svc = self.clone();
        let rt_for_follow = rt.clone();
        let target_follow = target.clone();
        let follow_handle = rt
            .spawn(async move {
                svc.run_host_follower(rt_for_follow, target_follow, wake, host_cancel).await;
            })
            .abort_handle();
        {
            let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(h) = st.hosts.get_mut(key) {
                if h.host_instance_id == host_instance_id && h.conn_id == conn_id && h.conn_state == RexHostConn::Connected {
                    if let Some(old) = h.follower.replace(follow_handle) {
                        old.abort();
                    }
                } else {
                    follow_handle.abort();
                }
            } else {
                follow_handle.abort();
            }
        }
    }

    /// Update a remote host's non-connected state (`Disconnected`, `Connecting`,
    /// `Reconnecting`, `Error`), rejecting notifications for removed, replaced,
    /// or superseded connections while recording a monotonic disconnect
    /// watermark so an early disconnect never loses to a delayed connect (Finding 14).
    #[allow(clippy::too_many_arguments)]
    pub fn host_state_changed(&self, key: &str, host_instance_id: u64, expected_conn_id: Option<u64>, state_seq: u64, state: RexHostConn, error: Option<String>, disconnected_at: Option<u64>) {
        if state == RexHostConn::Connected {
            return;
        }
        let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(h) = st.hosts.get_mut(key) else { return };
        if h.host_instance_id != host_instance_id {
            return;
        }
        if state_seq < h.last_state_seq || (state_seq != 0 && state_seq == h.last_state_seq) {
            return;
        }
        if let Some(exp_conn) = expected_conn_id {
            if exp_conn < h.conn_id {
                return;
            }
            // Remember this ended connection ID even if `host_connected(exp_conn)`
            // has not arrived yet, so the delayed connect is rejected cleanly.
            h.last_ended_conn_id = h.last_ended_conn_id.max(exp_conn);
        }
        h.last_state_seq = h.last_state_seq.max(state_seq);
        let was_connected = h.conn_state == RexHostConn::Connected;
        let prev_published = h.published_conn_state();
        h.cancel_relays_and_follower();
        h.has_live_follow = false;
        h.helper_known_incompatible = false;
        h.pinned = None;
        h.protocol = None;
        let next_disconnected_at = if was_connected { Some(disconnected_at.unwrap_or_else(now_unix_ms)) } else { h.disconnected_at.or(disconnected_at) };
        let changed = h.conn_state != state || prev_published != state || h.error != error || h.disconnected_at != next_disconnected_at;
        if !changed {
            return;
        }
        h.conn_state = state;
        h.error = error;
        h.disconnected_at = next_disconnected_at;
        let raw = h.raw_snapshot();
        st.queue_host(raw);
        drop(st);
        self.pub_notify.notify_one();
    }

    /// Update the plain-language lacking/setup reason from `ask_what_the_host_has`
    /// (`ScreensThere::lacking()`), ignoring stale probes from earlier connections.
    /// When `helper_has_screens == Some(true)` and the follower had parked
    /// incompatible, wakes the follower immediately (Findings 12, 17, 18).
    pub fn set_host_lacking(&self, key: &str, host_instance_id: u64, conn_id: u64, lacking: Option<String>, helper_has_screens: Option<bool>) {
        let wake = {
            let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let Some(h) = st.hosts.get_mut(key) else { return };
            if h.host_instance_id != host_instance_id || (conn_id != 0 && h.conn_id != conn_id) {
                return;
            }
            let mut changed = false;
            let mut wake_to_notify: Option<Arc<Notify>> = None;
            if h.lacking_reason != lacking {
                h.lacking_reason = lacking;
                changed = true;
            }
            if let Some(has) = helper_has_screens {
                if !has {
                    // Never let an out-of-band probe (`ask_what_the_host_has`)
                    // wipe or downgrade an active live `follow` stream (e.g.
                    // during connect-time auto-upgrade or multi-helper hosts):
                    // `run_host_follower` is the sole authority to mark a live
                    // host incompatible and clear `follow_status`/`screens`
                    // (Round 2 Confirming Finding 1).
                    if !h.has_live_follow && h.helper_has_screens != Some(false) {
                        h.helper_has_screens = Some(false);
                        changed = true;
                    }
                } else {
                    if h.helper_has_screens != Some(true) {
                        h.helper_has_screens = Some(true);
                        changed = true;
                    }
                    // Check Again found a screens-capable helper while the follower
                    // was parked incompatible: invalidate & wake it once!
                    if h.helper_known_incompatible && h.conn_state == RexHostConn::Connected {
                        h.helper_known_incompatible = false;
                        h.has_live_follow = false;
                        h.helper_gen += 1;
                        h.install_gen += 1;
                        h.protocol = None;
                        wake_to_notify = Some(h.follower_wake.clone());
                        changed = true;
                    }
                }
            }
            if !changed {
                return;
            }
            let raw = h.raw_snapshot();
            st.queue_host(raw);
            wake_to_notify
        };
        self.pub_notify.notify_one();
        if let Some(w) = wake {
            w.notify_one();
        }
    }

    /// Wake a connected host's follower after a helper install, update, removal,
    /// or Check Again discovery: bumps `(helper_gen, install_gen)`, immediately
    /// clears & publishes invalidated capabilities, and restarts any parked or
    /// running follower (Findings 17, 18, 20).
    pub fn wake_host_follower(self: &Arc<Self>, _rt: &tokio::runtime::Handle, key: &str, host_instance_id: u64) {
        let wake = {
            let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let Some(h) = st.hosts.get_mut(key) else { return };
            if h.host_instance_id != host_instance_id || h.conn_state != RexHostConn::Connected {
                return;
            }
            h.helper_gen += 1;
            h.install_gen += 1;
            h.protocol = None;
            h.helper_known_incompatible = false;
            h.has_live_follow = false;
            h.helper_has_screens = None;
            let wake = h.follower_wake.clone();
            let raw = h.raw_snapshot();
            st.queue_host(raw);
            wake
        };
        self.pub_notify.notify_one();
        wake.notify_one();
    }

    /// Wake a connected host's follower ONLY if it is currently parked
    /// incompatible (used when a background check sees `Stands::Already` so
    /// already-healthy hosts are not disrupted unnecessarily).
    pub fn wake_host_if_parked(self: &Arc<Self>, rt: &tokio::runtime::Handle, key: &str, host_instance_id: u64) {
        let parked = {
            let st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            st.hosts.get(key).is_some_and(|h| h.host_instance_id == host_instance_id && h.conn_state == RexHostConn::Connected && h.helper_known_incompatible)
        };
        if parked {
            self.wake_host_follower(rt, key, host_instance_id);
        }
    }

    /// Re-evaluate resolved `controlled_by` labels after Rex sessions or
    /// subagents change, publishing ONLY hosts whose resolved labels actually changed.
    pub fn refresh_owner_labels(&self, only_host: Option<&str>) {
        // Snapshot candidate owner panes without nesting HubState and inner locks.
        struct HostOwnerCandidate {
            host_key: String,
            inst_id: u64,
            screens: Vec<(String, Option<OwnerPane>, Option<OwnerPane>)>,
        }
        let candidates: Vec<HostOwnerCandidate> = {
            let st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            st.host_order
                .iter()
                .filter(|k| only_host.is_none_or(|want| want == k.as_str()))
                .filter_map(|k| st.hosts.get(k))
                .filter(|h| !h.screens.is_empty())
                .map(|h| HostOwnerCandidate {
                    host_key: h.key.clone(),
                    inst_id: h.host_instance_id,
                    screens: h.screens.values().map(|s| (s.name.clone(), s.owner.clone(), s.started_by.clone())).collect(),
                })
                .collect()
        };
        if candidates.is_empty() {
            return;
        }
        let mut queued = false;
        for HostOwnerCandidate { host_key: hk, inst_id, screens } in candidates {
            let resolved: HashMap<String, Option<String>> = screens
                .into_iter()
                .map(|(name, owner, started_by)| {
                    let label = owner.as_ref().and_then(|o| (self.owner_resolver)(&hk, o)).or_else(|| started_by.as_ref().and_then(|o| (self.owner_resolver)(&hk, o)));
                    (name, label)
                })
                .collect();
            let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let Some(h) = st.hosts.get_mut(&hk) else { continue };
            if h.host_instance_id != inst_id {
                continue;
            }
            if h.last_published_owners == resolved {
                continue;
            }
            h.last_published_owners = resolved;
            let raw = h.raw_snapshot();
            st.queue_host(raw);
            queued = true;
        }
        if queued {
            self.pub_notify.notify_one();
        }
    }

    async fn run_host_follower(self: Arc<Self>, rt: tokio::runtime::Handle, target: ScreenTarget, wake: Arc<Notify>, mut host_cancel: watch::Receiver<bool>) {
        let mut backoff = FOLLOW_RETRY_BASE;
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let host_key = target.host_key.clone();

        loop {
            if !self.is_host_at_target(&target) || *shutdown_rx.borrow() || *host_cancel.borrow() {
                break;
            }

            let open_timeout = self.current_follow_open_timeout();
            enum OpenStep {
                Opened(Result<BoxedScreenStream, String>),
                TimedOut,
                Woken,
                Stopped,
            }
            let open_step = tokio::select! {
                res = self.opener.open_screen_stream(target.clone(), vec!["follow".to_owned()]) => OpenStep::Opened(res),
                _ = tokio::time::sleep(open_timeout) => OpenStep::TimedOut,
                _ = wake.notified() => OpenStep::Woken,
                _ = wait_true(&mut host_cancel) => OpenStep::Stopped,
                _ = wait_true(&mut shutdown_rx) => OpenStep::Stopped,
            };

            let mut received_snapshot = false;
            let mut confirmed_incompatible = false;
            let mut woken_mid_stream = false;
            let stream_started = Instant::now();

            match open_step {
                OpenStep::Stopped => break,
                OpenStep::Woken => {
                    backoff = FOLLOW_RETRY_BASE;
                    continue;
                }
                OpenStep::TimedOut => {
                    tracing::debug!("Rex screens follow open timed out for {host_key}");
                }
                OpenStep::Opened(Err(e)) => {
                    tracing::debug!("Rex screens follow open failed for {host_key}: {e}");
                    confirmed_incompatible = is_confirmed_helper_incompatible(&e);
                }
                OpenStep::Opened(Ok(stream)) => {
                    // Hold `_keep_stdin_open` alive for the lifetime of the stream so
                    // `olo-rex-link screen follow` stays asleep in `poll(-1)` until
                    // this host disconnects, helper updates, or Rex shuts down.
                    let (read_half, _keep_stdin_open) = tokio::io::split(stream);
                    let mut reader = BufReader::new(read_half);
                    let first_snapshot_deadline = tokio::time::Instant::now() + self.current_follow_open_timeout();
                    loop {
                        enum ReadStep {
                            Line(io::Result<Option<Vec<u8>>>),
                            FirstSnapshotTimedOut,
                            Woken,
                            Stopped,
                        }
                        let step = if !received_snapshot {
                            tokio::select! {
                                res = read_bounded_line_async(&mut reader, MAX_JSON_LINE) => ReadStep::Line(res),
                                _ = tokio::time::sleep_until(first_snapshot_deadline) => ReadStep::FirstSnapshotTimedOut,
                                _ = wake.notified() => ReadStep::Woken,
                                _ = wait_true(&mut host_cancel) => ReadStep::Stopped,
                                _ = wait_true(&mut shutdown_rx) => ReadStep::Stopped,
                            }
                        } else {
                            // Zero-timer idle wait once initial snapshot has arrived!
                            tokio::select! {
                                res = read_bounded_line_async(&mut reader, MAX_JSON_LINE) => ReadStep::Line(res),
                                _ = wake.notified() => ReadStep::Woken,
                                _ = wait_true(&mut host_cancel) => ReadStep::Stopped,
                                _ = wait_true(&mut shutdown_rx) => ReadStep::Stopped,
                            }
                        };
                        match step {
                            ReadStep::Stopped => return,
                            ReadStep::Woken => {
                                woken_mid_stream = true;
                                break;
                            }
                            ReadStep::FirstSnapshotTimedOut => {
                                tracing::debug!("Rex screens follow timed out waiting for initial snapshot from {host_key}");
                                break;
                            }
                            ReadStep::Line(Ok(Some(raw_line))) => {
                                if raw_line.is_empty() {
                                    continue;
                                }
                                match serde_json::from_slice::<FollowEvent>(&raw_line) {
                                    Ok(ev) => {
                                        if let Err(e) = screen_wire::validate_follow_event(&ev) {
                                            tracing::warn!("Rex screens follow invalid event from {host_key}: {e}");
                                            continue;
                                        }
                                        if matches!(ev, FollowEvent::Snapshot { .. }) {
                                            received_snapshot = true;
                                        }
                                        self.apply_follow_event(&rt, &target, ev);
                                    }
                                    Err(e) => {
                                        let text = String::from_utf8_lossy(&raw_line);
                                        if !received_snapshot && is_confirmed_helper_incompatible(&text) {
                                            confirmed_incompatible = true;
                                            break;
                                        }
                                        tracing::debug!("Rex screens follow ignored non-JSON line from {host_key}: {e}");
                                    }
                                }
                            }
                            ReadStep::Line(Ok(None)) => break,
                            ReadStep::Line(Err(e)) => {
                                let msg = e.to_string();
                                if !received_snapshot && is_confirmed_helper_incompatible(&msg) {
                                    confirmed_incompatible = true;
                                }
                                tracing::debug!("Rex screens follow stream read ended for {host_key}: {msg}");
                                break;
                            }
                        }
                    }
                }
            }

            if !self.is_host_at_target(&target) || *shutdown_rx.borrow() || *host_cancel.borrow() {
                break;
            }

            if received_snapshot {
                // Follow stream closed while SSH stays connected: immediately
                // drop follower freshness so Check Again's own status/list wins
                // and live acts wait for the next full snapshot (Findings 12 & 26).
                self.mark_follow_stream_lost(&target);
            }

            if woken_mid_stream {
                backoff = FOLLOW_RETRY_BASE;
                continue;
            }

            if confirmed_incompatible {
                self.mark_helper_incompatible(&target);
                // Confirmed older/missing helper: park silently with 0 wakeups
                // until an authorized helper install/update or Check Again wakes
                // us, or the host disconnects.
                tokio::select! {
                    _ = wake.notified() => {
                        backoff = FOLLOW_RETRY_BASE;
                    }
                    _ = wait_true(&mut host_cancel) => break,
                    _ = wait_true(&mut shutdown_rx) => break,
                }
                continue;
            }

            // Reset backoff ONLY if the stream stayed healthy for a full stable
            // window; otherwise back off exponentially up to FOLLOW_RETRY_MAX.
            let stable_window = self.current_follow_stable_window();
            if received_snapshot && stream_started.elapsed() >= stable_window {
                backoff = FOLLOW_RETRY_BASE;
            }
            let wait = backoff;
            backoff = (backoff * 2).min(FOLLOW_RETRY_MAX);

            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = wake.notified() => {
                    backoff = FOLLOW_RETRY_BASE;
                }
                _ = wait_true(&mut host_cancel) => break,
                _ = wait_true(&mut shutdown_rx) => break,
            }
        }
    }

    fn is_host_at_target(&self, target: &ScreenTarget) -> bool {
        let st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        st.hosts.get(&target.host_key).is_some_and(|h| h.host_instance_id == target.host_instance_id && h.conn_id == target.conn_id && h.conn_state == RexHostConn::Connected)
    }

    fn mark_follow_stream_lost(&self, target: &ScreenTarget) {
        let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(h) = st.hosts.get_mut(&target.host_key) else { return };
        if h.host_instance_id != target.host_instance_id || h.conn_id != target.conn_id || h.conn_state != RexHostConn::Connected {
            return;
        }
        if !h.has_live_follow {
            return;
        }
        h.has_live_follow = false;
        let raw = h.raw_snapshot();
        st.queue_host(raw);
        drop(st);
        self.pub_notify.notify_one();
    }

    fn mark_helper_incompatible(&self, target: &ScreenTarget) {
        let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(h) = st.hosts.get_mut(&target.host_key) else { return };
        if h.host_instance_id != target.host_instance_id || h.conn_id != target.conn_id || h.conn_state != RexHostConn::Connected {
            return;
        }
        if h.helper_known_incompatible && !h.has_live_follow && h.follow_status.is_none() && h.screens.is_empty() {
            return;
        }
        h.helper_has_screens = Some(false);
        h.helper_known_incompatible = true;
        h.has_live_follow = false;
        h.disconnected_at = None;
        h.screens.clear();
        h.follow_status = None;
        h.protocol = None;
        h.snapshot_ready.notify_waiters();
        let raw = h.raw_snapshot();
        st.queue_host(raw);
        drop(st);
        self.pub_notify.notify_one();
    }

    /// Apply one validated [`FollowEvent`] from `olo-rex-link screen follow`.
    /// Every sequence number is assigned and enqueued in the same critical
    /// section so subscribers can never observe out-of-order drops (Finding 15).
    fn apply_follow_event(self: &Arc<Self>, rt: &tokio::runtime::Handle, target: &ScreenTarget, ev: FollowEvent) {
        let (notify_pub, probe_protocol) = {
            let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let Some(h) = st.hosts.get_mut(&target.host_key) else { return };
            if h.host_instance_id != target.host_instance_id || h.conn_id != target.conn_id || h.conn_state != RexHostConn::Connected {
                return;
            }
            match ev {
                FollowEvent::Snapshot { status, screens, .. } => {
                    let status = sanitize_host_status(status);
                    let prev_had_status = h.follow_status.is_some();
                    let prev_key = h.current_install_key();
                    let next_key = (status.installed.as_ref().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()), status.committed);
                    let install_changed = prev_had_status && prev_key != next_key;
                    let proto_mismatch = h.protocol.as_ref().is_some_and(|p| p.installed != next_key.0 || p.committed != next_key.1);
                    if install_changed || proto_mismatch {
                        // Invalidate stale installation capabilities BEFORE publishing!
                        h.install_gen += 1;
                        h.protocol = None;
                    }
                    let need_probe = h.protocol.is_none();
                    h.helper_has_screens = Some(true);
                    h.helper_known_incompatible = false;
                    h.has_live_follow = true;
                    h.disconnected_at = None;
                    h.follow_status = Some(status);
                    h.screens.clear();
                    h.screens_truncated = false;
                    let mut est_bytes = 0usize;
                    for raw_s in screens {
                        let Some(s) = sanitize_host_screen(&h.key, raw_s) else { continue };
                        if h.screens.len() >= MAX_SCREENS_PER_HOST {
                            h.screens_truncated = true;
                            break;
                        }
                        let est = est_host_screen_bytes(&s);
                        if est_bytes + est > MAX_HOST_SCREENS_BYTES {
                            h.screens_truncated = true;
                            continue;
                        }
                        est_bytes += est;
                        h.screens.insert(s.name.clone(), s);
                    }
                    h.snapshot_ready.notify_waiters();
                    let raw = h.raw_snapshot();
                    st.queue_host(raw);
                    (true, need_probe)
                }
                FollowEvent::Screen { screen } => {
                    let Some(s) = sanitize_host_screen(&h.key, *screen) else {
                        return;
                    };
                    let exists = h.screens.contains_key(&s.name);
                    if !exists && h.screens.len() >= MAX_SCREENS_PER_HOST {
                        h.screens_truncated = true;
                        (false, false)
                    } else {
                        let cur_bytes = host_screens_est_bytes(&h.screens);
                        let old_est = h.screens.get(&s.name).map_or(0, est_host_screen_bytes);
                        let new_est = est_host_screen_bytes(&s);
                        if cur_bytes.saturating_sub(old_est) + new_est > MAX_HOST_SCREENS_BYTES {
                            h.screens_truncated = true;
                            (false, false)
                        } else {
                            h.screens.insert(s.name.clone(), s);
                            let raw = h.raw_snapshot();
                            st.queue_host(raw);
                            (true, false)
                        }
                    }
                }
                FollowEvent::LastUsed { name, last_used, actions } => {
                    // Reject/ignore any screen name that wouldn't pass outgoing
                    // desktop validation (Finding 25) so followers never drop.
                    if rex_sock::validate_screen_name(&name).is_err() {
                        (false, false)
                    } else if let Some(s) = h.screens.get_mut(&name) {
                        let ts_advanced = s.last_used.is_none_or(|prev| last_used > prev);
                        let actions_changed = actions.is_some() && s.actions != actions;
                        if !ts_advanced && !actions_changed {
                            (false, false)
                        } else {
                            if ts_advanced {
                                s.last_used = Some(last_used);
                            }
                            if let Some(a) = actions {
                                s.actions = Some(a);
                            }
                            let eff_ts = s.last_used.unwrap_or(last_used);
                            let eff_actions = s.actions;
                            let inst_id = h.host_instance_id;
                            let conn_id = h.conn_id;
                            // Debounced contract rule: ordered broadcast + in-memory
                            // HubState update, with ZERO state-file rewrite on disk.
                            st.queue_last_used(target.host_key.clone(), inst_id, conn_id, name, eff_ts, eff_actions);
                            (true, false)
                        }
                    } else {
                        (false, false)
                    }
                }
                FollowEvent::Gone { name } => {
                    if rex_sock::validate_screen_name(&name).is_err() {
                        (false, false)
                    } else if h.screens.remove(&name).is_some() {
                        let raw = h.raw_snapshot();
                        st.queue_host(raw);
                        (true, false)
                    } else {
                        (false, false)
                    }
                }
                FollowEvent::Status { status } => {
                    let status = sanitize_host_status(status);
                    let prev_key = h.current_install_key();
                    let next_key = (status.installed.as_ref().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()), status.committed);
                    let install_changed = next_key != prev_key;
                    if h.follow_status.as_ref() == Some(&status) {
                        (false, false)
                    } else {
                        if install_changed {
                            // Clear previous installation's capabilities BEFORE publishing!
                            h.install_gen += 1;
                            h.protocol = None;
                        }
                        h.helper_has_screens = Some(true);
                        h.follow_status = Some(status);
                        let raw = h.raw_snapshot();
                        st.queue_host(raw);
                        (true, install_changed)
                    }
                }
            }
        };

        if notify_pub {
            self.pub_notify.notify_one();
        }
        if probe_protocol {
            let svc = self.clone();
            let target = target.clone();
            let deadline = Instant::now() + self.current_handshake_timeout();
            rt.spawn(async move {
                let _ = svc.ensure_protocol(&target, deadline).await;
            });
        }
    }

    /// Return cached `--protocol` info for `(target, helper_gen, install_gen)`
    /// or probe once (serialized per host so concurrent callers share one probe,
    /// bounded by one absolute `deadline`, and cancelled immediately if the host
    /// disconnects or is replaced).
    async fn ensure_protocol(&self, target: &ScreenTarget, deadline: Instant) -> Result<CachedProtocol, String> {
        #[cfg(test)]
        let probe_wait_notify;
        let (label, probe_lock, mut host_cancel) = {
            let st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let Some(h) = st.hosts.get(&target.host_key) else {
                return Err(format!("“{}” isn’t one of Rex’s hosts.", target.host_key));
            };
            if h.host_instance_id != target.host_instance_id || h.conn_id != target.conn_id || h.conn_state != RexHostConn::Connected {
                return Err(format!("“{}” is disconnected. Reconnect in Rex to resume.", h.label));
            }
            if let Some(cached) = h.protocol.as_ref().filter(|p| p.conn_id == target.conn_id && p.helper_gen == h.helper_gen && p.install_gen == h.install_gen) {
                return Ok(cached.clone());
            }
            #[cfg(test)]
            {
                probe_wait_notify = h.probe_wait_notify.clone();
            }
            (h.label.clone(), h.protocol_probe_lock.clone(), h.relay_cancel.subscribe())
        };

        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let rem = remaining_until(deadline, &label)?;
        #[cfg(test)]
        probe_wait_notify.notify_waiters();
        let _probe_guard = tokio::select! {
            g = probe_lock.lock() => g,
            _ = tokio::time::sleep(rem) => return Err(format!("“{label}” didn’t answer in time.")),
            _ = wait_true(&mut host_cancel) => return Err(format!("“{label}” is disconnected. Reconnect in Rex to resume.")),
            _ = wait_true(&mut shutdown_rx) => return Err("Rex is shutting down.".to_owned()),
        };

        // Re-check cache and sample current generation/installation cleanly after
        // acquiring the single-flight lock (never bypassing a valid cache or
        // retaining a pre-transition `Some(installed)` after transition to `None`).
        let (helper_gen, install_gen, sampled_installed, sampled_committed) = {
            let st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let Some(h) = st.hosts.get(&target.host_key) else {
                return Err(format!("“{}” isn’t one of Rex’s hosts.", target.host_key));
            };
            if h.host_instance_id != target.host_instance_id || h.conn_id != target.conn_id || h.conn_state != RexHostConn::Connected {
                return Err(format!("“{}” is disconnected. Reconnect in Rex to resume.", h.label));
            }
            if let Some(cached) = h.protocol.as_ref().filter(|p| p.conn_id == target.conn_id && p.helper_gen == h.helper_gen && p.install_gen == h.install_gen) {
                return Ok(cached.clone());
            }
            (h.helper_gen, h.install_gen, h.current_installed(), h.current_committed())
        };

        let rem = remaining_until(deadline, &label)?;
        let out = tokio::select! {
            res = tokio::time::timeout(rem, self.opener.run_screen_command(target.clone(), vec!["--protocol".to_owned()], rem)) => {
                res.map_err(|_| format!("“{label}” didn’t answer within {} s.", rem.as_secs().max(1)))??
            }
            _ = wait_true(&mut host_cancel) => return Err(format!("“{label}” is disconnected. Reconnect in Rex to resume.")),
            _ = wait_true(&mut shutdown_rx) => return Err("Rex is shutting down.".to_owned()),
        };

        // Never poison the cache as legacy on missing helper, truncation, signal,
        // malformed exit-0 output, or arbitrary transient failures (Findings 8, 18, 20).
        if out.status == Some(69) || out.status == Some(127) {
            return Err("the host has no screens-capable Olo helper".to_owned());
        }
        if out.truncated {
            return Err("host protocol reply exceeded output limit".to_owned());
        }

        let parsed = if out.ok() {
            parse_protocol_json(&out.stdout, target.conn_id, helper_gen, install_gen, sampled_installed.clone(), sampled_committed).ok_or_else(|| "host returned invalid protocol reply".to_owned())?
        } else if is_explicit_protocol_unsupported(out.status, &out.stderr, &out.stdout) {
            // Helper explicitly ran and rejected `--protocol`: cache legacy once for this generation.
            CachedProtocol {
                conn_id: target.conn_id,
                helper_gen,
                install_gen,
                installed: sampled_installed.clone(),
                committed: sampled_committed,
                proto: 0,
                commands: Vec::new(),
                capabilities: Vec::new(),
            }
        } else {
            return Err(out.error_words("couldn’t query host screen protocol"));
        };

        let queued = {
            let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let Some(h) = st.hosts.get_mut(&target.host_key) else {
                return Err(format!("“{}” isn’t one of Rex’s hosts.", target.host_key));
            };
            if h.host_instance_id != target.host_instance_id || h.conn_id != target.conn_id || h.conn_state != RexHostConn::Connected {
                return Err(format!("“{}” is disconnected. Reconnect in Rex to resume.", h.label));
            }
            // Reject stale probe if helper or installed `(version, committed)` changed in flight!
            if h.helper_gen != helper_gen || h.install_gen != install_gen || h.current_installed() != sampled_installed || h.current_committed() != sampled_committed {
                return Err("host screen runtime changed during capability check; retrying.".to_owned());
            }
            if h.protocol.as_ref() == Some(&parsed) {
                false
            } else {
                h.protocol = Some(parsed.clone());
                let raw = h.raw_snapshot();
                st.queue_host(raw);
                true
            }
        };
        if queued {
            self.pub_notify.notify_one();
        }
        Ok(parsed)
    }

    async fn run_one_shot(&self, target: &ScreenTarget, label: &str, argv: Vec<String>, deadline: Instant, mut host_cancel: watch::Receiver<bool>) -> Result<CommandExecOutput, String> {
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let rem = remaining_until(deadline, label)?;
        let out = tokio::select! {
            res = tokio::time::timeout(rem, self.opener.run_screen_command(target.clone(), argv, rem)) => {
                res.map_err(|_| format!("“{label}” didn’t answer within {} s.", rem.as_secs().max(1)))??
            }
            _ = wait_true(&mut host_cancel) => return Err(format!("“{label}” is disconnected. Reconnect in Rex to resume.")),
            _ = wait_true(&mut shutdown_rx) => return Err("Rex is shutting down.".to_owned()),
        };
        Ok(out)
    }

    fn cleanup_owned_files(&self, files: OwnedServiceFiles) {
        // Verify socket entry is still our exact bound inode before unlinking!
        if let Ok(sc) = c_name(&files.sock_name) {
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            // SAFETY: valid pinned dirfd and single component name.
            if unsafe { libc::fstatat(files.sock_dir_fd.as_raw_fd(), sc.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } == 0
                && (st.st_mode & libc::S_IFMT) == libc::S_IFSOCK
                && (st.st_dev, st.st_ino) == files.sock_inode
            {
                unsafe { libc::unlinkat(files.sock_dir_fd.as_raw_fd(), sc.as_ptr(), 0) };
            }
        }
        // Only remove the state file if it still carries our PID!
        if read_state_file_at(&files.state_dir_fd, &files.state_name).is_some_and(|s| s.pid == self.pid)
            && let Ok(stc) = c_name(&files.state_name)
        {
            // SAFETY: valid pinned dirfd and single component name.
            unsafe { libc::unlinkat(files.state_dir_fd.as_raw_fd(), stc.as_ptr(), 0) };
        }
        // Explicitly unlock before closing so a concurrent mid-`fork()`
        // child process (before `exec` clears `O_CLOEXEC`) cannot hold
        // the OFD flock open across handoff.
        // SAFETY: valid owned lock fd.
        unsafe { libc::flock(files._lock_fd.as_raw_fd(), libc::LOCK_UN) };
        // `_lock_fd` closes here as `files` drops AFTER cleanup!
    }

    /// Stop all host followers, active relays, publisher/writer tasks, and
    /// socket accept loops, then unlink ONLY our verified socket inode and
    /// PID-matched state file while still holding the per-display flock.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send_replace(true);
        self.pub_notify.notify_one();
        self.disk_notify.notify_one();
        {
            let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            for h in st.hosts.values_mut() {
                h.cancel_relays_and_follower();
            }
        }
        // Always serialize with `start_server()` under `file_lock` (Agy Finding 1)
        // so an in-flight `start_server()` can never install `owned_files` after
        // `shutdown()` returns without being cleaned up immediately.
        let _guard = self.file_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.write_files.store(false, Ordering::Release);
        let files = self.owned_files.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(files) = files {
            self.cleanup_owned_files(files);
        }
    }

    /// Start the single strictly `seq`-ordered background publisher task
    /// independently of local socket startup (Findings 9, 15, 24, 35): even if
    /// socket bind fails or demo mode skips binding, `HubState::host_screens`
    /// and the Host window still receive every follower update, pending
    /// publications remain bounded and coalesced per host, and disk writes are
    /// gated strictly on `write_files`. Safe to call multiple times (idempotent).
    pub fn start_publisher(self: &Arc<Self>, rt: &tokio::runtime::Handle) {
        if self.publisher_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let svc_pub = self.clone();
        let pub_notify = self.pub_notify.clone();
        let mut shutdown_pub = self.shutdown_tx.subscribe();
        rt.spawn(async move {
            loop {
                let next = {
                    let mut st = svc_pub.inner.lock().unwrap_or_else(|e| e.into_inner());
                    st.pop_next_pending()
                };
                let Some((seq, job)) = next else {
                    if *shutdown_pub.borrow() {
                        break;
                    }
                    tokio::select! {
                        _ = pub_notify.notified() => continue,
                        _ = wait_true(&mut shutdown_pub) => break,
                    }
                };
                if *shutdown_pub.borrow() {
                    break;
                }
                match job {
                    PendingPublishOp::Host { rev, raw } => {
                        let host_instance_id = raw.host_instance_id;
                        let conn_id = raw.conn_id;
                        let has_follow_snapshot = raw.has_follow_snapshot;
                        let follow_status = raw.follow_status.clone();
                        let host = raw.into_rex_host(&svc_pub.owner_resolver);
                        {
                            let mut st = svc_pub.inner.lock().unwrap_or_else(|e| e.into_inner());
                            if let Some(h) = st.hosts.get_mut(&host.key)
                                && h.host_instance_id == host_instance_id
                            {
                                h.last_published_owners = host.screens.iter().map(|s| (s.name.clone(), s.controlled_by.clone())).collect();
                            }
                        }
                        if let Some(cb) = &svc_pub.on_host_screens {
                            cb(HostScreensUpdate { host_key: host.key.clone(), host_instance_id, conn_id, rev, has_follow_snapshot, status: follow_status, screens: host.screens.clone() });
                        }
                        if svc_pub.tx.receiver_count() > 0 {
                            let _ = svc_pub.tx.send(BroadcastEvent { seq, line: RexFollowLine::HostUpdated { rev, host: Box::new(host) } });
                        }
                        if svc_pub.write_files.load(Ordering::Acquire) {
                            svc_pub.disk_notify.notify_one();
                        }
                    }
                    PendingPublishOp::Removed { rev, key, host_instance_id } => {
                        if let Some(cb) = svc_pub.on_host_removed.lock().unwrap_or_else(|e| e.into_inner()).clone() {
                            cb(key.clone(), host_instance_id);
                        }
                        if svc_pub.tx.receiver_count() > 0 {
                            let _ = svc_pub.tx.send(BroadcastEvent { seq, line: RexFollowLine::HostRemoved { rev, key } });
                        }
                        if svc_pub.write_files.load(Ordering::Acquire) {
                            svc_pub.disk_notify.notify_one();
                        }
                    }
                    PendingPublishOp::LastUsed { host, host_instance_id, conn_id, name, last_used, actions } => {
                        if let Some(cb) = &svc_pub.on_last_used {
                            cb(LastUsedUpdate { host_key: host.clone(), host_instance_id, conn_id, name: name.clone(), last_used, actions });
                        }
                        if svc_pub.tx.receiver_count() > 0 {
                            let _ = svc_pub.tx.send(BroadcastEvent { seq, line: RexFollowLine::ScreenLastUsed { host, name, last_used, actions } });
                        }
                        // Contract rule: never rewrite state-<display>.json on LastUsed alone.
                    }
                }
            }
        });
    }

    /// Verify private directories, acquire the per-display ownership flock,
    /// safely reclaim a verified stale socket (never unlinking non-sockets or
    /// live listeners), bind `socket_path` (`0600`), write initial `state_path`
    /// (`0600`) propagating errors cleanly, and spawn accept + ordered publisher
    /// + coalescing disk-writer tasks on `rt`.
    pub fn start_server(self: &Arc<Self>, rt: &tokio::runtime::Handle) -> io::Result<()> {
        if *self.shutdown_tx.borrow() {
            return Ok(());
        }
        // Ensure hub updates always drain cleanly even if socket startup fails.
        self.start_publisher(rt);
        let sock_parent = self.socket_path.parent().filter(|p| !p.as_os_str().is_empty()).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid socket path"))?;
        let sock_name = self.socket_path.file_name().and_then(|n| n.to_str()).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid socket filename"))?.to_owned();
        let state_parent = self.state_path.parent().filter(|p| !p.as_os_str().is_empty()).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid state path"))?;
        let state_name = self.state_path.file_name().and_then(|n| n.to_str()).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid state filename"))?.to_owned();

        // 1. Verify BOTH socket and state directories before modifying anything!
        let sock_dir_fd = open_verified_private_dir(sock_parent)?;
        let state_dir_fd = open_verified_private_dir(state_parent)?;

        // 2. Acquire exclusive non-blocking per-display lock so two live Rex
        //    instances can never overwrite or unlink each other's socket/state.
        let lock_fd = acquire_lock_at(&sock_dir_fd, &format!("{sock_name}.lock"), None)?;

        // 3. Inspect any pre-existing entry at `sock_name` without following symlinks.
        let sock_c = c_name(&sock_name)?;
        let mut existing_st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstatat(sock_dir_fd.as_raw_fd(), sock_c.as_ptr(), &mut existing_st, libc::AT_SYMLINK_NOFOLLOW) } == 0 {
            if (existing_st.st_mode & libc::S_IFMT) != libc::S_IFSOCK {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("refusing to remove non-socket entry {}", self.socket_path.display())));
            }
            if existing_st.st_uid != my_euid() {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("socket {} owned by another uid", self.socket_path.display())));
            }
            match StdUnixStream::connect(&self.socket_path) {
                Ok(_) => {
                    return Err(io::Error::new(io::ErrorKind::AddrInUse, format!("another live listener already serves {}", self.socket_path.display())));
                }
                Err(e) if matches!(e.kind(), io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound) => {
                    // Verified stale socket from a crashed/re-exec'd Rex: unlink safely.
                    if unsafe { libc::unlinkat(sock_dir_fd.as_raw_fd(), sock_c.as_ptr(), 0) } != 0 {
                        let err = io::Error::last_os_error();
                        if err.kind() != io::ErrorKind::NotFound {
                            return Err(err);
                        }
                    }
                }
                Err(e) => return Err(e),
            }
        }

        // 4. Bind socket and record its exact (dev, ino) before writing state.
        let _enter = rt.enter();
        let listener = UnixListener::bind(&self.socket_path)?;
        if let Err(e) = std::fs::set_permissions(&self.socket_path, std::fs::Permissions::from_mode(0o600)) {
            unsafe { libc::unlinkat(sock_dir_fd.as_raw_fd(), sock_c.as_ptr(), 0) };
            return Err(e);
        }
        let mut bound_st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstatat(sock_dir_fd.as_raw_fd(), sock_c.as_ptr(), &mut bound_st, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
            let err = io::Error::last_os_error();
            unsafe { libc::unlinkat(sock_dir_fd.as_raw_fd(), sock_c.as_ptr(), 0) };
            return Err(err);
        }
        let sock_inode = (bound_st.st_dev, bound_st.st_ino);

        // 5. Write initial state file ONLY AFTER bind succeeds, propagating any
        //    failure and cleaning up our newly bound socket on error (Finding 35).
        let init_snap = self.snapshot();
        if let Err(e) = write_state_atomic_at(&state_dir_fd, &state_name, &init_snap) {
            drop(listener);
            unsafe { libc::unlinkat(sock_dir_fd.as_raw_fd(), sock_c.as_ptr(), 0) };
            return Err(e);
        }

        let files = OwnedServiceFiles { sock_dir_fd, sock_name, sock_inode, state_dir_fd, state_name, _lock_fd: lock_fd };
        {
            // Serialize file ownership installation against `shutdown()` (Agy Finding 1):
            // if shutdown raced ahead while we were binding/writing, tear down our
            // newly bound socket, state file, and flock immediately and exit.
            let _guard = self.file_lock.lock().unwrap_or_else(|e| e.into_inner());
            if *self.shutdown_tx.borrow() {
                drop(listener);
                self.cleanup_owned_files(files);
                return Ok(());
            }
            *self.owned_files.lock().unwrap_or_else(|e| e.into_inner()) = Some(files);
            self.write_files.store(true, Ordering::Release);
        }

        tracing::info!("Rex screens socket listening at {} (state: {})", self.socket_path.display(), self.state_path.display());

        // 6. Spawn single coalescing disk-writer task (runs file I/O on blocking
        //    pool so GUI and `rex-io` threads never block on disk writes; 0 wakeups at rest).
        let svc_disk = self.clone();
        let disk_notify = self.disk_notify.clone();
        let mut shutdown_disk = self.shutdown_tx.subscribe();
        rt.spawn(async move {
            loop {
                tokio::select! {
                    _ = disk_notify.notified() => {}
                    _ = wait_true(&mut shutdown_disk) => break,
                }
                if *shutdown_disk.borrow() {
                    break;
                }
                let svc_io = svc_disk.clone();
                let res = tokio::task::spawn_blocking(move || svc_io.flush_state_file_sync()).await;
                if let Ok(Err(e)) = res {
                    tracing::warn!("Rex screens state write {} failed: {e}", svc_disk.state_path.display());
                }
            }
        });

        // 8. Spawn local Unix socket accept loop with admission control.
        let svc = self.clone();
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        rt.spawn(async move {
            loop {
                tokio::select! {
                    _ = wait_true(&mut shutdown_rx) => break,
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((stream, _)) => {
                                match svc.client_permits.clone().try_acquire_owned() {
                                    Ok(permit) => {
                                        let svc_client = svc.clone();
                                        tokio::spawn(async move {
                                            let _permit = permit;
                                            svc_client.handle_client(stream).await;
                                        });
                                    }
                                    Err(_) => {
                                        let write_timeout = svc.current_client_write_timeout();
                                        tokio::spawn(async move {
                                            let (_r, mut w) = stream.into_split();
                                            let _ = write_response_async(&mut w, &RexSockResponse::Error { message: "too many concurrent screen connections".into() }, write_timeout).await;
                                        });
                                    }
                                }
                            }
                            Err(e) => {
                                if *shutdown_rx.borrow() {
                                    break;
                                }
                                tracing::warn!("Rex screens socket accept error: {e}");
                                tokio::time::sleep(Duration::from_millis(200)).await;
                            }
                        }
                    }
                }
            }
        });
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Client connection handler
    // -----------------------------------------------------------------------

    async fn handle_client(self: Arc<Self>, stream: UnixStream) {
        // Peer authentication via SO_PEERCRED (fail closed).
        let Ok(cred) = stream.peer_cred() else { return };
        if rex_sock::verify_peer_uid_match(cred.uid(), my_euid()).is_err() {
            return;
        }

        let write_timeout = self.current_client_write_timeout();
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);

        let raw_line = match tokio::time::timeout(REQUEST_READ_TIMEOUT, read_bounded_line_async(&mut reader, MAX_REQUEST_LINE_BYTES)).await {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => return,
            Ok(Err(e)) => {
                let _ = write_response_async(&mut write_half, &RexSockResponse::Error { message: format!("invalid request: {e}") }, write_timeout).await;
                return;
            }
            Err(_) => {
                let _ = write_response_async(&mut write_half, &RexSockResponse::Error { message: "timed out waiting for request".into() }, write_timeout).await;
                return;
            }
        };

        let req: RexSockRequest = match serde_json::from_slice(&raw_line) {
            Ok(r) => r,
            Err(e) => {
                let _ = write_response_async(&mut write_half, &RexSockResponse::Error { message: format!("invalid request JSON: {e}") }, write_timeout).await;
                return;
            }
        };
        if let Err(why) = rex_sock::validate_request(&req) {
            let _ = write_response_async(&mut write_half, &RexSockResponse::Error { message: why }, write_timeout).await;
            return;
        }

        match req {
            RexSockRequest::List => {
                let state = self.snapshot();
                let _ = write_response_async(&mut write_half, &RexSockResponse::Snapshot { state }, write_timeout).await;
            }
            RexSockRequest::Follow => {
                let read_half = reader.into_inner();
                self.handle_follow(read_half, write_half).await;
            }
            RexSockRequest::OpenHost { host } => {
                let resp = match self.resolve_any_host(&host) {
                    Ok(key) => match self.opener.open_host(&key) {
                        Ok(()) => RexSockResponse::Ok,
                        Err(message) => RexSockResponse::Error { message: truncate_utf8(&message, MAX_ERROR_LEN) },
                    },
                    Err(message) => RexSockResponse::Error { message },
                };
                let _ = write_response_async(&mut write_half, &resp, write_timeout).await;
            }
            RexSockRequest::Protocol { host } => {
                let deadline = Instant::now() + self.current_handshake_timeout();
                let resp = match self.resolve_connected_host(&host) {
                    Ok(ctx) => match self.ensure_protocol(&ctx.target, deadline).await {
                        Ok(p) => RexSockResponse::Protocol { proto: p.proto, commands: p.commands, capabilities: p.capabilities },
                        Err(message) => RexSockResponse::Error { message: truncate_utf8(&message, MAX_ERROR_LEN) },
                    },
                    Err(message) => RexSockResponse::Error { message },
                };
                let _ = write_response_async(&mut write_half, &resp, write_timeout).await;
            }
            RexSockRequest::Stream { host, name } => {
                let leftover = reader.buffer().to_vec();
                let read_half = reader.into_inner();
                self.handle_relay_stream(read_half, write_half, leftover, &host, &name).await;
            }
            RexSockRequest::History { host, name } => {
                let leftover = reader.buffer().to_vec();
                let read_half = reader.into_inner();
                self.handle_relay_history(read_half, write_half, leftover, &host, &name).await;
            }
            RexSockRequest::Export { host, name, args } => {
                let leftover = reader.buffer().to_vec();
                let read_half = reader.into_inner();
                self.handle_relay_export(read_half, write_half, leftover, &host, &name, args).await;
            }
            RexSockRequest::Start { host, name, keep, size, scale, sound } => {
                let resp = self.handle_start(&host, name, keep, size, scale, sound).await;
                let _ = write_response_async(&mut write_half, &resp, write_timeout).await;
            }
            RexSockRequest::Stop { host, name, expected_started, expected_last_used } => {
                let resp = self.handle_stop(&host, &name, expected_started, expected_last_used).await;
                let _ = write_response_async(&mut write_half, &resp, write_timeout).await;
            }
            RexSockRequest::ClearHistory { host, name, expected_started, expected_last_used } => {
                let resp = self.handle_clear_history(&host, &name, expected_started, expected_last_used).await;
                let _ = write_response_async(&mut write_half, &resp, write_timeout).await;
            }
            RexSockRequest::Rename { host, name, title } => {
                let resp = self.handle_rename(&host, &name, title).await;
                let _ = write_response_async(&mut write_half, &resp, write_timeout).await;
            }
            RexSockRequest::Archive { host, name, archived } => {
                let resp = self.handle_archive(&host, &name, archived).await;
                let _ = write_response_async(&mut write_half, &resp, write_timeout).await;
            }
            RexSockRequest::Delete { host, name, expected_started, expected_last_used } => {
                let resp = self.handle_delete(&host, &name, expected_started, expected_last_used).await;
                let _ = write_response_async(&mut write_half, &resp, write_timeout).await;
            }
        }
    }

    fn resolve_any_host(&self, query: &str) -> Result<String, String> {
        let st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        st.find_host_key(query).ok_or_else(|| format!("“{query}” isn’t one of Rex’s hosts."))
    }

    fn resolve_connected_host(&self, query: &str) -> Result<ConnectedHostCtx, String> {
        let st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(key) = st.find_host_key(query) else {
            return Err(format!("“{query}” isn’t one of Rex’s hosts."));
        };
        let h = &st.hosts[&key];
        if h.conn_state != RexHostConn::Connected {
            return Err(format!("“{}” is disconnected. Reconnect in Rex to resume.", h.label));
        }
        Ok(ConnectedHostCtx {
            target: h.screen_target(),
            label: h.label.clone(),
            host_cancel: h.relay_cancel.subscribe(),
            relay_permits: h.relay_permits.clone(),
            fresh_snapshot: h.is_fresh_for_rpc(),
            snapshot_ready: h.snapshot_ready.clone(),
        })
    }

    fn resolve_connected_host_instance(&self, key: &str, host_instance_id: u64) -> Result<ConnectedHostCtx, String> {
        let st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(h) = st.hosts.get(key) else {
            return Err(format!("“{key}” isn’t one of Rex’s hosts."));
        };
        if h.host_instance_id != host_instance_id {
            return Err(format!("“{}” changed before this action could run.", h.label));
        }
        if h.conn_state != RexHostConn::Connected {
            return Err(format!("“{}” is disconnected. Reconnect in Rex to resume.", h.label));
        }
        Ok(ConnectedHostCtx {
            target: h.screen_target(),
            label: h.label.clone(),
            host_cancel: h.relay_cancel.subscribe(),
            relay_permits: h.relay_permits.clone(),
            fresh_snapshot: h.is_fresh_for_rpc(),
            snapshot_ready: h.snapshot_ready.clone(),
        })
    }

    /// Wait up to `deadline` for a newly connected/reconnected host (or a host
    /// whose follow stream closed and is reconnecting) to receive its fresh
    /// follower snapshot so live actions never operate on stale rows.
    async fn wait_fresh_snapshot(&self, ctx: &mut ConnectedHostCtx, deadline: Instant) -> Result<(), String> {
        if ctx.fresh_snapshot {
            return Ok(());
        }
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        loop {
            let notified = ctx.snapshot_ready.notified();
            {
                let st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                let Some(h) = st.hosts.get(&ctx.target.host_key) else {
                    return Err(format!("“{}” isn’t one of Rex’s hosts.", ctx.target.host_key));
                };
                if h.host_instance_id != ctx.target.host_instance_id || h.conn_id != ctx.target.conn_id || h.conn_state != RexHostConn::Connected {
                    return Err(format!("“{}” is disconnected. Reconnect in Rex to resume.", ctx.label));
                }
                if h.is_fresh_for_rpc() {
                    ctx.fresh_snapshot = true;
                    return Ok(());
                }
            }
            let rem = remaining_until(deadline, &ctx.label)?;
            tokio::select! {
                _ = notified => {}
                _ = tokio::time::sleep(rem) => return Err(format!("“{}” is still connecting.", ctx.label)),
                _ = wait_true(&mut ctx.host_cancel) => return Err(format!("“{}” is disconnected. Reconnect in Rex to resume.", ctx.label)),
                _ = wait_true(&mut shutdown_rx) => return Err("Rex is shutting down.".to_owned()),
            }
        }
    }

    async fn handle_follow(&self, mut read_half: tokio::net::unix::OwnedReadHalf, mut write_half: tokio::net::unix::OwnedWriteHalf) {
        // Subscribe BEFORE taking the initial snapshot so no event slips past.
        let mut rx = self.tx.subscribe();
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let write_timeout = self.current_client_write_timeout();

        let (mut last_seq, snap) = self.snapshot_with_seq();
        if write_follow_line_cancellable(&mut write_half, &RexFollowLine::Snapshot { state: snap }, write_timeout, &mut shutdown_rx).await.is_err() {
            return;
        }

        let mut eof_buf = [0u8; 64];
        loop {
            tokio::select! {
                _ = wait_true(&mut shutdown_rx) => break,
                read_res = read_half.read(&mut eof_buf) => {
                    match read_res {
                        Ok(0) | Err(_) => break,
                        Ok(_) => continue,
                    }
                }
                recv_res = rx.recv() => {
                    match recv_res {
                        Ok(ev) => {
                            // Strictly ordered monotonic sequence check covers BOTH
                            // structural events AND `ScreenLastUsed` (Findings 15 & 16).
                            if ev.seq <= last_seq {
                                continue;
                            }
                            last_seq = ev.seq;
                            if write_follow_line_cancellable(&mut write_half, &ev.line, write_timeout, &mut shutdown_rx).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            // Slow follower: resync with a full snapshot and skip
                            // every retained event (including ScreenLastUsed) <= snap_seq.
                            let (snap_seq, snap) = self.snapshot_with_seq();
                            last_seq = snap_seq;
                            if write_follow_line_cancellable(&mut write_half, &RexFollowLine::Snapshot { state: snap }, write_timeout, &mut shutdown_rx).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    }

    async fn open_and_confirm_relay(
        &self,
        ctx: &mut ConnectedHostCtx,
        read_half: &tokio::net::unix::OwnedReadHalf,
        args: Vec<String>,
        deadline: Instant,
    ) -> Result<(BoxedScreenStream, Vec<u8>, RelayWatchers, OwnedSemaphorePermit), String> {
        // Fail closed if client hangup/rdhup epoll watchers cannot be created:
        // a relay that cannot observe its client closing is never started (Finding 5).
        let watchers = make_relay_watchers(read_half.as_ref().as_raw_fd())?;
        let permit = ctx.relay_permits.clone().try_acquire_owned().map_err(|_| format!("Too many open screen streams for “{}”.", ctx.label))?;
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let rem = remaining_until(deadline, &ctx.label)?;
        let mut remote = tokio::select! {
            res = tokio::time::timeout(rem, self.opener.open_screen_stream(ctx.target.clone(), args)) => {
                res.map_err(|_| format!("“{}” didn’t answer in time.", ctx.label))??
            }
            _ = watchers.hup.wait() => return Err("client closed before stream was ready".to_owned()),
            _ = wait_true(&mut ctx.host_cancel) => return Err(format!("“{}” is disconnected. Reconnect in Rex to resume.", ctx.label)),
            _ = wait_true(&mut shutdown_rx) => return Err("Rex is shutting down.".to_owned()),
        };
        if *ctx.host_cancel.borrow() {
            return Err(format!("“{}” disconnected while opening stream.", ctx.label));
        }

        // Confirm remote startup before writing `Ready` (Finding 21): over SSH,
        // `ExecOutput::Starting` only surfaces exec/helper failures on the first
        // read. All three host binary streams (`stream`, `history-stream`, `export`)
        // emit an initial server message immediately without waiting for stdin.
        let rem = remaining_until(deadline, &ctx.label)?;
        let mut first = [0u8; 4096];
        let n = tokio::select! {
            res = tokio::time::timeout(rem, remote.read(&mut first)) => {
                match res {
                    Ok(Ok(0)) => return Err("host closed stream before starting".to_owned()),
                    Ok(Ok(n)) => n,
                    Ok(Err(e)) => return Err(truncate_utf8(&e.to_string(), MAX_ERROR_LEN)),
                    Err(_) => return Err(format!("“{}” didn’t answer in time.", ctx.label)),
                }
            }
            _ = watchers.hup.wait() => return Err("client closed before stream was ready".to_owned()),
            _ = wait_true(&mut ctx.host_cancel) => return Err(format!("“{}” disconnected while opening stream.", ctx.label)),
            _ = wait_true(&mut shutdown_rx) => return Err("Rex is shutting down.".to_owned()),
        };
        if *ctx.host_cancel.borrow() {
            return Err(format!("“{}” disconnected while opening stream.", ctx.label));
        }
        Ok((remote, first[..n].to_vec(), watchers, permit))
    }

    async fn handle_relay_stream(&self, read_half: tokio::net::unix::OwnedReadHalf, mut write_half: tokio::net::unix::OwnedWriteHalf, leftover: Vec<u8>, host: &str, name: &str) {
        let deadline = Instant::now() + self.current_handshake_timeout();
        let write_timeout = self.current_client_write_timeout();
        let mut ctx = match self.resolve_connected_host(host) {
            Ok(v) => v,
            Err(message) => {
                let _ = write_response_async(&mut write_half, &RexSockResponse::Error { message }, write_timeout).await;
                return;
            }
        };
        if let Err(message) = self.wait_fresh_snapshot(&mut ctx, deadline).await {
            let _ = write_response_async(&mut write_half, &RexSockResponse::Error { message }, write_timeout).await;
            return;
        }
        let (width, height, scale) = {
            let st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            st.hosts
                .get(&ctx.target.host_key)
                .and_then(|h| h.screens.get(name))
                .map_or((None, None, None), |s| (Some(s.width).filter(|&w| w > 0), Some(s.height).filter(|&h| h > 0), Some(s.scale).filter(|&sc| sc.is_finite() && sc > 0.0)))
        };
        let (remote, initial_remote, watchers, _permit) = match self.open_and_confirm_relay(&mut ctx, &read_half, vec!["stream".to_owned(), name.to_owned()], deadline).await {
            Ok(v) => v,
            Err(message) => {
                let _ = write_response_async(&mut write_half, &RexSockResponse::Error { message: truncate_utf8(&message, MAX_ERROR_LEN) }, write_timeout).await;
                return;
            }
        };
        if write_response_async(&mut write_half, &RexSockResponse::Ready { width, height, scale }, write_timeout).await.is_err() {
            return;
        }
        relay_bidirectional(read_half, write_half, watchers, leftover, initial_remote, remote, ctx.host_cancel, self.shutdown_tx.subscribe()).await;
    }

    async fn handle_relay_history(&self, read_half: tokio::net::unix::OwnedReadHalf, mut write_half: tokio::net::unix::OwnedWriteHalf, leftover: Vec<u8>, host: &str, name: &str) {
        let deadline = Instant::now() + self.current_handshake_timeout();
        let write_timeout = self.current_client_write_timeout();
        let mut ctx = match self.resolve_connected_host(host) {
            Ok(v) => v,
            Err(message) => {
                let _ = write_response_async(&mut write_half, &RexSockResponse::Error { message }, write_timeout).await;
                return;
            }
        };
        if let Err(message) = self.wait_fresh_snapshot(&mut ctx, deadline).await {
            let _ = write_response_async(&mut write_half, &RexSockResponse::Error { message }, write_timeout).await;
            return;
        }
        // Only open binary history on a helper confirmed to speak binary history
        // (`history-stream`, or explicitly advertised `history-wire`). Never send
        // binary expectations to an older text-only helper (Finding 21).
        let proto = match self.ensure_protocol(&ctx.target, deadline).await {
            Ok(p) => p,
            Err(message) => {
                let _ = write_response_async(&mut write_half, &RexSockResponse::Error { message: truncate_utf8(&message, MAX_ERROR_LEN) }, write_timeout).await;
                return;
            }
        };
        let args = if proto.proto >= 1 && proto.commands.iter().any(|c| c == "history-stream") {
            vec!["history-stream".to_owned(), name.to_owned()]
        } else if proto.proto >= 1 && proto.commands.iter().any(|c| c == "history-wire" || c == "history --wire") {
            vec!["history".to_owned(), name.to_owned(), "--wire".to_owned()]
        } else {
            let msg = format!("Update Olo’s helper on “{}” to view screen history.", ctx.label);
            let _ = write_response_async(&mut write_half, &RexSockResponse::Error { message: msg }, write_timeout).await;
            return;
        };
        let (remote, initial_remote, watchers, _permit) = match self.open_and_confirm_relay(&mut ctx, &read_half, args, deadline).await {
            Ok(v) => v,
            Err(message) => {
                let _ = write_response_async(&mut write_half, &RexSockResponse::Error { message: truncate_utf8(&message, MAX_ERROR_LEN) }, write_timeout).await;
                return;
            }
        };
        if write_response_async(&mut write_half, &RexSockResponse::Ready { width: None, height: None, scale: None }, write_timeout).await.is_err() {
            return;
        }
        relay_bidirectional(read_half, write_half, watchers, leftover, initial_remote, remote, ctx.host_cancel, self.shutdown_tx.subscribe()).await;
    }

    async fn handle_relay_export(&self, read_half: tokio::net::unix::OwnedReadHalf, mut write_half: tokio::net::unix::OwnedWriteHalf, leftover: Vec<u8>, host: &str, name: &str, args: Vec<String>) {
        let deadline = Instant::now() + self.current_handshake_timeout();
        let write_timeout = self.current_client_write_timeout();
        let mut cmd_args = Vec::with_capacity(2 + args.len());
        cmd_args.push("export".to_owned());
        cmd_args.push(name.to_owned());
        cmd_args.extend(args);
        // Validate full export range semantics (`--from <= --to`, `--part` ranges)
        // before touching any transport (Finding 22).
        if let Err(message) = screen_wire::parse_export_args(&cmd_args[1..]) {
            let _ = write_response_async(&mut write_half, &RexSockResponse::Error { message }, write_timeout).await;
            return;
        }
        let mut ctx = match self.resolve_connected_host(host) {
            Ok(v) => v,
            Err(message) => {
                let _ = write_response_async(&mut write_half, &RexSockResponse::Error { message }, write_timeout).await;
                return;
            }
        };
        if let Err(message) = self.wait_fresh_snapshot(&mut ctx, deadline).await {
            let _ = write_response_async(&mut write_half, &RexSockResponse::Error { message }, write_timeout).await;
            return;
        }
        let (remote, initial_remote, watchers, _permit) = match self.open_and_confirm_relay(&mut ctx, &read_half, cmd_args, deadline).await {
            Ok(v) => v,
            Err(message) => {
                let _ = write_response_async(&mut write_half, &RexSockResponse::Error { message: truncate_utf8(&message, MAX_ERROR_LEN) }, write_timeout).await;
                return;
            }
        };
        if write_response_async(&mut write_half, &RexSockResponse::Ready { width: None, height: None, scale: None }, write_timeout).await.is_err() {
            return;
        }
        relay_bidirectional(read_half, write_half, watchers, leftover, initial_remote, remote, ctx.host_cancel, self.shutdown_tx.subscribe()).await;
    }

    async fn handle_start(&self, host: &str, name: Option<String>, keep: bool, size: Option<(u32, u32)>, scale: Option<f64>, sound: bool) -> RexSockResponse {
        let deadline = Instant::now() + self.current_rpc_timeout();
        let mut ctx = match self.resolve_connected_host(host) {
            Ok(v) => v,
            Err(message) => return RexSockResponse::Error { message },
        };
        if let Err(message) = self.wait_fresh_snapshot(&mut ctx, deadline).await {
            return RexSockResponse::Error { message };
        }
        // Refuse immediately if Olo screens aren't installed.
        {
            let st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(h) = st.hosts.get(&ctx.target.host_key)
                && let Some(status) = &h.follow_status
                && status.installed.as_deref().is_none_or(|v| v.trim().is_empty())
            {
                return RexSockResponse::Error { message: format!("Olo screens are not installed on “{}”.", ctx.label) };
            }
        }
        let mut argv = vec!["start".to_owned()];
        if let Some(n) = name {
            argv.push(n);
        }
        if keep {
            argv.push("--keep".to_owned());
        }
        if let Some((w, h)) = size {
            argv.push("--size".to_owned());
            argv.push(format!("{w}x{h}"));
        }
        if let Some(s) = scale {
            argv.push("--scale".to_owned());
            argv.push(format!("{s}"));
        }
        if sound {
            argv.push("--sound".to_owned());
        }
        let out = match self.run_one_shot(&ctx.target, &ctx.label, argv, deadline, ctx.host_cancel.clone()).await {
            Ok(o) => o,
            Err(message) => return RexSockResponse::Error { message: truncate_utf8(&message, MAX_ERROR_LEN) },
        };
        if !out.ok() {
            return RexSockResponse::Error { message: out.error_words("start failed") };
        }
        if out.truncated {
            return RexSockResponse::Error { message: "host start response exceeded output limit".into() };
        }
        let screen = match parse_started_screen_json(&out.stdout) {
            Ok(s) => s,
            Err(message) => return RexSockResponse::Error { message },
        };
        let Some(clean_screen) = sanitize_host_screen(&ctx.target.host_key, screen) else {
            return RexSockResponse::Error { message: "host returned invalid screen name".into() };
        };
        // Return `Started` to the caller and let the host's authoritative
        // `follow` stream own `TrackedHost.screens` so a delayed `start` reply
        // can NEVER overwrite a newer event or resurrect a screen the host has
        // since reported gone (Findings 9 & 33).
        let rex_screen = host_screen_to_rex_screen(&ctx.target.host_key, clean_screen, &self.owner_resolver);
        RexSockResponse::Started { screen: Box::new(rex_screen) }
    }

    async fn run_guarded_stop(&self, mut ctx: ConnectedHostCtx, name: &str, expected_started: Option<u64>, expected_last_used: Option<u64>, deadline: Instant) -> Result<(), String> {
        let Some(exp_started) = expected_started else {
            return Err("Update Olo on this computer to stop a screen from here.".into());
        };
        self.wait_fresh_snapshot(&mut ctx, deadline).await?;
        // Require the current helper's atomic `stop-expected` capability (not
        // the older released `stop-expected-started`, which dropped its lock
        // before stopping) (Finding 2).
        let proto = self.ensure_protocol(&ctx.target, deadline).await.map_err(|m| truncate_utf8(&m, MAX_ERROR_LEN))?;
        let has_cap = proto.proto >= 1 && proto.capabilities.iter().any(|c| c == "stop-expected" || c == "stop_expected");
        if !has_cap {
            return Err(format!("Update Olo on “{}” to stop a screen from here.", ctx.label));
        }
        let mut argv = vec!["stop".to_owned(), name.to_owned(), "--expected-started".to_owned(), exp_started.to_string()];
        if let Some(lu) = expected_last_used {
            argv.push("--expected-last-used".to_owned());
            argv.push(lu.to_string());
        }
        match self.run_one_shot(&ctx.target, &ctx.label, argv, deadline, ctx.host_cancel).await {
            Ok(out) if out.ok() => Ok(()),
            Ok(out) => Err(out.error_words("stop failed")),
            Err(message) => Err(truncate_utf8(&message, MAX_ERROR_LEN)),
        }
    }

    /// Guarded Stop entrypoint for Rex's Host window: pins both `host_key` and
    /// `host_instance_id` and routes through the exact same `stop-expected` +
    /// `--expected-started` check used on the Unix socket.
    pub async fn stop_screen_for_host_instance(&self, host_key: &str, host_instance_id: u64, name: &str, expected_started: Option<u64>) -> Result<(), String> {
        rex_sock::validate_screen_name(name)?;
        let deadline = Instant::now() + self.current_rpc_timeout();
        let ctx = self.resolve_connected_host_instance(host_key, host_instance_id)?;
        // A Host window Stop click on a legacy remote screen whose descriptor
        // lacks `started` means the remote host's Olo is too old (not this
        // computer's Rex) (Agy Finding 9).
        if expected_started.is_none() {
            return Err(format!("Update Olo on “{}” to stop a screen from here.", ctx.label));
        }
        self.run_guarded_stop(ctx, name, expected_started, None, deadline).await
    }

    async fn handle_stop(&self, host: &str, name: &str, expected_started: Option<u64>, expected_last_used: Option<u64>) -> RexSockResponse {
        if expected_started.is_none() {
            return RexSockResponse::Error { message: "Update Olo on this computer to stop a screen from here.".into() };
        }
        let deadline = Instant::now() + self.current_rpc_timeout();
        let ctx = match self.resolve_connected_host(host) {
            Ok(v) => v,
            Err(message) => return RexSockResponse::Error { message },
        };
        match self.run_guarded_stop(ctx, name, expected_started, expected_last_used, deadline).await {
            Ok(()) => RexSockResponse::Ok,
            Err(message) => RexSockResponse::Error { message },
        }
    }

    async fn handle_clear_history(&self, host: &str, name: &str, expected_started: Option<u64>, expected_last_used: Option<u64>) -> RexSockResponse {
        // Refuse ambiguous legacy requests lacking observed activity (use `Some(0)`
        // for observed empty) and always send the stopped-state guard (Finding 1).
        let Some(exp_lu) = expected_last_used else {
            return RexSockResponse::Error { message: "Update Olo on this computer to clear a screen’s history from here.".into() };
        };
        let exp_started = expected_started.unwrap_or(0);
        let deadline = Instant::now() + self.current_rpc_timeout();
        let mut ctx = match self.resolve_connected_host(host) {
            Ok(v) => v,
            Err(message) => return RexSockResponse::Error { message },
        };
        if let Err(message) = self.wait_fresh_snapshot(&mut ctx, deadline).await {
            return RexSockResponse::Error { message };
        }
        // Require `clear-expected` on the host helper; never check Rex's cache in its place.
        let proto = match self.ensure_protocol(&ctx.target, deadline).await {
            Ok(p) => p,
            Err(message) => return RexSockResponse::Error { message: truncate_utf8(&message, MAX_ERROR_LEN) },
        };
        let has_cap = proto.proto >= 1 && proto.capabilities.iter().any(|c| c == "clear-expected" || c == "clear_expected");
        if !has_cap {
            return RexSockResponse::Error { message: format!("Update Olo on “{}” to clear a screen’s history from here.", ctx.label) };
        }
        let argv = vec!["history".to_owned(), name.to_owned(), "--clear".to_owned(), "--expected-started".to_owned(), exp_started.to_string(), "--expected-last-used".to_owned(), exp_lu.to_string()];
        match self.run_one_shot(&ctx.target, &ctx.label, argv, deadline, ctx.host_cancel).await {
            Ok(out) if out.ok() => RexSockResponse::Ok,
            Ok(out) => RexSockResponse::Error { message: out.error_words("clear history failed") },
            Err(message) => RexSockResponse::Error { message: truncate_utf8(&message, MAX_ERROR_LEN) },
        }
    }

    async fn handle_rename(&self, host: &str, name: &str, title: Option<String>) -> RexSockResponse {
        let deadline = Instant::now() + self.current_rpc_timeout();
        let mut ctx = match self.resolve_connected_host(host) {
            Ok(v) => v,
            Err(message) => return RexSockResponse::Error { message },
        };
        if let Err(message) = self.wait_fresh_snapshot(&mut ctx, deadline).await {
            return RexSockResponse::Error { message };
        }
        let mut argv = vec!["rename".to_owned(), name.to_owned()];
        if let Some(t) = title.filter(|t| !t.is_empty()) {
            argv.push(t);
        }
        match self.run_one_shot(&ctx.target, &ctx.label, argv, deadline, ctx.host_cancel).await {
            Ok(out) if out.ok() => RexSockResponse::Ok,
            Ok(out) => RexSockResponse::Error { message: out.error_words("rename failed") },
            Err(message) => RexSockResponse::Error { message: truncate_utf8(&message, MAX_ERROR_LEN) },
        }
    }

    async fn handle_archive(&self, host: &str, name: &str, archived: bool) -> RexSockResponse {
        let deadline = Instant::now() + self.current_rpc_timeout();
        let mut ctx = match self.resolve_connected_host(host) {
            Ok(v) => v,
            Err(message) => return RexSockResponse::Error { message },
        };
        if let Err(message) = self.wait_fresh_snapshot(&mut ctx, deadline).await {
            return RexSockResponse::Error { message };
        }
        let verb = if archived { "archive" } else { "unarchive" };
        let argv = vec![verb.to_owned(), name.to_owned()];
        match self.run_one_shot(&ctx.target, &ctx.label, argv, deadline, ctx.host_cancel).await {
            Ok(out) if out.ok() => RexSockResponse::Ok,
            Ok(out) => RexSockResponse::Error { message: out.error_words(if archived { "archive failed" } else { "unarchive failed" }) },
            Err(message) => RexSockResponse::Error { message: truncate_utf8(&message, MAX_ERROR_LEN) },
        }
    }

    async fn handle_delete(&self, host: &str, name: &str, expected_started: Option<u64>, expected_last_used: Option<u64>) -> RexSockResponse {
        // Refuse ambiguous legacy requests lacking observed activity (use `Some(0)`
        // for observed empty) and always send the stopped-state guard (Finding 1).
        let Some(exp_lu) = expected_last_used else {
            return RexSockResponse::Error { message: "Update Olo on this computer to delete a screen from here.".into() };
        };
        let exp_started = expected_started.unwrap_or(0);
        let deadline = Instant::now() + self.current_rpc_timeout();
        let mut ctx = match self.resolve_connected_host(host) {
            Ok(v) => v,
            Err(message) => return RexSockResponse::Error { message },
        };
        if let Err(message) = self.wait_fresh_snapshot(&mut ctx, deadline).await {
            return RexSockResponse::Error { message };
        }
        // Require `delete-expected` on the host helper; never check Rex's cache in its place.
        let proto = match self.ensure_protocol(&ctx.target, deadline).await {
            Ok(p) => p,
            Err(message) => return RexSockResponse::Error { message: truncate_utf8(&message, MAX_ERROR_LEN) },
        };
        let has_cap = proto.proto >= 1 && proto.capabilities.iter().any(|c| c == "delete-expected" || c == "delete_expected");
        if !has_cap {
            return RexSockResponse::Error { message: format!("Update Olo on “{}” to delete a screen from here.", ctx.label) };
        }
        let argv = vec!["delete".to_owned(), name.to_owned(), "--expected-started".to_owned(), exp_started.to_string(), "--expected-last-used".to_owned(), exp_lu.to_string()];
        match self.run_one_shot(&ctx.target, &ctx.label, argv, deadline, ctx.host_cancel).await {
            Ok(out) if out.ok() => RexSockResponse::Ok,
            Ok(out) => RexSockResponse::Error { message: out.error_words("delete failed") },
            Err(message) => RexSockResponse::Error { message: truncate_utf8(&message, MAX_ERROR_LEN) },
        }
    }
}

struct ConnectedHostCtx {
    target: ScreenTarget,
    label: String,
    host_cancel: watch::Receiver<bool>,
    relay_permits: Arc<Semaphore>,
    fresh_snapshot: bool,
    snapshot_ready: Arc<Notify>,
}

// ---------------------------------------------------------------------------
// Raw bidirectional relay with backpressure-proof two-way teardown (Finding 5)
// ---------------------------------------------------------------------------

/// Lightweight Linux `epoll` fd wrapped in Tokio [`AsyncFd`] to observe
/// `EPOLLHUP | EPOLLERR` (with `events == 0`) or `EPOLLRDHUP` (with
/// `events == EPOLLRDHUP`) on a Unix socket **without** waking on unread input
/// data (`EPOLLIN`), using zero threads and zero polling.
struct EpollWatcher {
    fd: AsyncFd<OwnedFd>,
}

impl EpollWatcher {
    fn new(target_fd: std::os::fd::RawFd, events: u32) -> io::Result<Self> {
        // SAFETY: `epoll_create1` creates a new close-on-exec epoll instance.
        let ep = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
        if ep < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: freshly created owned fd.
        let owned = unsafe { OwnedFd::from_raw_fd(ep) };
        let mut ev = libc::epoll_event { events, u64: 0 };
        // SAFETY: `target_fd` is valid and outlives this watcher within `relay_bidirectional`.
        if unsafe { libc::epoll_ctl(owned.as_raw_fd(), libc::EPOLL_CTL_ADD, target_fd, &mut ev) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `owned` is an `OwnedFd` for a freshly created Linux epoll
        // instance owned exclusively by this `AsyncFd` until dropped.
        let fd = unsafe { AsyncFd::register_with_interest(owned, Interest::READABLE) }?;
        Ok(Self { fd })
    }

    async fn wait(&self) {
        let _ = self.fd.readable().await;
    }
}

/// Pair of required client-socket epoll watchers (`EPOLLHUP|EPOLLERR` and
/// `EPOLLRDHUP`). A relay is never started if either fails to register (Finding 5).
struct RelayWatchers {
    hup: EpollWatcher,
    rdhup: EpollWatcher,
}

fn make_relay_watchers(client_fd: std::os::fd::RawFd) -> Result<RelayWatchers, String> {
    let hup = EpollWatcher::new(client_fd, 0).map_err(|e| format!("couldn’t monitor screen connection: {e}"))?;
    let rdhup = EpollWatcher::new(client_fd, libc::EPOLLRDHUP as u32).map_err(|e| format!("couldn’t monitor screen connection: {e}"))?;
    Ok(RelayWatchers { hup, rdhup })
}

/// Write `slice` to `dst`, bounded by `rdhup_watch` if the peer has already
/// closed its write half while `dst` is backpressured.
async fn write_chunk_cancellable<W: tokio::io::AsyncWrite + Unpin>(dst: &mut W, mut slice: &[u8], rdhup_watch: &EpollWatcher, rdhup_seen: &mut bool) -> io::Result<()> {
    while !slice.is_empty() {
        let n = if *rdhup_seen {
            tokio::time::timeout(RELAY_RDHUP_DRAIN, dst.write(slice)).await.map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "remote backpressured after client write-close"))??
        } else {
            tokio::select! {
                res = dst.write(slice) => res?,
                _ = rdhup_watch.wait() => {
                    *rdhup_seen = true;
                    continue;
                }
            }
        };
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "remote closed"));
        }
        slice = &slice[n..];
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn relay_bidirectional(
    mut client_read: tokio::net::unix::OwnedReadHalf,
    mut client_write: tokio::net::unix::OwnedWriteHalf,
    watchers: RelayWatchers,
    leftover: Vec<u8>,
    initial_remote: Vec<u8>,
    remote: BoxedScreenStream,
    mut host_disconnected: watch::Receiver<bool>,
    mut server_shutdown: watch::Receiver<bool>,
) {
    let RelayWatchers { hup: client_hup, rdhup: client_rdhup } = watchers;

    let (mut remote_read, mut remote_write) = tokio::io::split(remote);

    // Put `leftover` INSIDE the cancellable `c2r` pump (never before select!),
    // and select strictly on copy completion without unbounded `shutdown()`.
    let c2r = async {
        let mut rdhup_seen = false;
        if !leftover.is_empty() && write_chunk_cancellable(&mut remote_write, &leftover, &client_rdhup, &mut rdhup_seen).await.is_err() {
            return;
        }
        let mut buf = [0u8; 16 * 1024];
        loop {
            match client_read.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if write_chunk_cancellable(&mut remote_write, &buf[..n], &client_rdhup, &mut rdhup_seen).await.is_err() {
                        break;
                    }
                }
            }
        }
    };

    let r2c = async {
        if !initial_remote.is_empty() && client_write.write_all(&initial_remote).await.is_err() {
            return;
        }
        let mut buf = [0u8; 16 * 1024];
        loop {
            match remote_read.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if client_write.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            }
        }
    };

    let wait_disconnect = async {
        if *host_disconnected.borrow() {
            return;
        }
        let _ = host_disconnected.changed().await;
    };
    let wait_shutdown = async {
        if *server_shutdown.borrow() {
            return;
        }
        let _ = server_shutdown.changed().await;
    };

    tokio::select! {
        _ = c2r => {}
        _ = r2c => {}
        _ = client_hup.wait() => {}
        _ = wait_disconnect => {}
        _ = wait_shutdown => {}
    }

    // Drop epoll watchers BEFORE dropping the socket halves they monitor.
    drop(client_hup);
    drop(client_rdhup);

    // Bounded shutdown followed by immediate drop so `ExecOutput::drop` /
    // `quinn::SendStream::drop` closes the far end even if backpressured.
    let _ = tokio::time::timeout(RELAY_SHUTDOWN_TIMEOUT, remote_write.shutdown()).await;
    let _ = tokio::time::timeout(RELAY_SHUTDOWN_TIMEOUT, client_write.shutdown()).await;
}

// ---------------------------------------------------------------------------
// Helpers: framing, validation, parsing, host-conn conversion, demo isolation
// ---------------------------------------------------------------------------

fn remaining_until(deadline: Instant, label: &str) -> Result<Duration, String> {
    let now = Instant::now();
    if now >= deadline { Err(format!("“{label}” didn’t answer in time.")) } else { Ok(deadline.saturating_duration_since(now)) }
}

fn is_confirmed_helper_incompatible(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("no screens-capable olo helper")
        || lower.contains("exited with status 69")
        || lower.contains("exit status: 69")
        || lower.contains("unknown subcommand")
        || lower.contains("usage: olo-rex-link")
        || lower.contains("was not found on the host")
}

pub fn conn_state_to_rex(s: &HostConnState) -> (RexHostConn, Option<String>) {
    match s {
        HostConnState::Connected => (RexHostConn::Connected, None),
        HostConnState::Connecting => (RexHostConn::Connecting, None),
        HostConnState::Reconnecting { last_error, .. } => (RexHostConn::Reconnecting, (!last_error.is_empty()).then(|| last_error.clone())),
        HostConnState::Disconnected => (RexHostConn::Disconnected, None),
        HostConnState::Error(e) => (RexHostConn::Error, (!e.is_empty()).then(|| e.clone())),
    }
}

/// Default socket and state file paths (from `olo_screen_pane::paths`).
pub fn default_paths() -> (PathBuf, PathBuf) {
    (screen_paths::rex_screens_socket(), screen_paths::rex_screens_state_path())
}

/// Isolated paths for demo mode (`OLO_REX_DEMO=1`): requires a non-empty
/// `OLO_REX_SCREENS_DIR` and derives BOTH socket and state paths strictly inside
/// that directory so demo runs can never touch or overwrite live discovery state.
pub fn demo_screens_paths() -> Option<(PathBuf, PathBuf)> {
    demo_screens_paths_from(std::env::var("OLO_REX_SCREENS_DIR").ok().as_deref(), std::env::var("OLO_REX_SCREENS_SOCKET").ok().as_deref())
}

pub(crate) fn demo_screens_paths_from(dir_env: Option<&str>, sock_env: Option<&str>) -> Option<(PathBuf, PathBuf)> {
    let dir_str = dir_env?.trim();
    if dir_str.is_empty() {
        return None;
    }
    let dir = PathBuf::from(dir_str);
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    let safe: String = display.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect();
    let state = dir.join(format!("state-{safe}.json"));
    let sock = match sock_env.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => {
            let p = PathBuf::from(s);
            if p.parent() == Some(dir.as_path()) { p } else { return None }
        }
        None => dir.join(format!("rex-{safe}.sock")),
    };
    Some((sock, state))
}

/// Convert live [`RexScreen`]s from the follower into [`crate::host_screens::ScreenRow`]s
/// for Rex's Host window Screens tab.
pub fn rex_screens_to_rows(screens: &[RexScreen]) -> Vec<crate::host_screens::ScreenRow> {
    let mut rows: Vec<crate::host_screens::ScreenRow> = screens
        .iter()
        .map(|s| crate::host_screens::ScreenRow {
            name: s.name.clone(),
            title: s.title.clone(),
            running: s.state == HostScreenState::Running,
            archived: s.state == HostScreenState::Archived,
            size: (s.width, s.height),
            started: s.started,
        })
        .collect();
    crate::host_screens::in_order(&mut rows);
    rows
}

fn now_unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn truncate_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

fn sanitize_label(s: &str, max_bytes: usize) -> String {
    let cleaned: String = s.chars().filter(|&c| !c.is_control()).collect();
    truncate_utf8(&cleaned, max_bytes)
}

/// True ONLY when an older host helper explicitly ran and rejected `--protocol`
/// (e.g. unknown flag/subcommand or usage line with exit 1 or 2), never on
/// missing exit status, exit 69/127, timeout, or transient nonzero errors.
fn is_explicit_protocol_unsupported(status: Option<u32>, stderr: &str, stdout: &str) -> bool {
    if !matches!(status, Some(1 | 2)) {
        return false;
    }
    let combined = format!("{stderr}\n{stdout}").to_ascii_lowercase();
    combined.contains("usage: olo screen") || combined.contains("unknown") || combined.contains("unrecognized") || combined.contains("unexpected argument") || combined.contains("not a valid")
}

/// Parse a complete JSON object (either multi-line pretty-printed, as emitted
/// by `olo-screen start`, or compact single-line) from bounded command stdout.
fn parse_json_object_from_stdout(stdout: &str) -> Result<serde_json::Value, String> {
    if stdout.len() > MAX_RPC_OUTPUT_BYTES {
        return Err("host JSON response exceeded size limit".into());
    }
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Err("host returned empty JSON response".into());
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed)
        && v.is_object()
    {
        return Ok(v);
    }
    // Fallback if leading non-JSON lines precede a JSON object: scan from each
    // `{` on a line start (last complete object wins).
    let mut last_obj: Option<serde_json::Value> = None;
    for (idx, _) in trimmed.match_indices('{') {
        let mut de = serde_json::Deserializer::from_str(&trimmed[idx..]).into_iter::<serde_json::Value>();
        if let Some(Ok(v)) = de.next()
            && v.is_object()
            && trimmed[idx + de.byte_offset()..].trim().is_empty()
        {
            last_obj = Some(v);
        }
    }
    last_obj.ok_or_else(|| "host returned invalid JSON response".into())
}

fn parse_protocol_json(stdout: &str, conn_id: u64, helper_gen: u64, install_gen: u64, installed: Option<String>, committed: Option<u64>) -> Option<CachedProtocol> {
    let v = parse_json_object_from_stdout(stdout).ok()?;
    let proto = v.get("proto")?.as_u64()? as u32;
    let str_vec = |k: &str, max_items: usize| -> Vec<String> {
        v.get(k)
            .and_then(|a| a.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str()).map(|s| sanitize_label(s, MAX_CAP_NAME_LEN)).filter(|s| !s.is_empty()).take(max_items).collect())
            .unwrap_or_default()
    };
    Some(CachedProtocol { conn_id, helper_gen, install_gen, installed, committed, proto, commands: str_vec("commands", MAX_COMMANDS), capabilities: str_vec("capabilities", MAX_CAPABILITIES) })
}

fn parse_started_screen_json(stdout: &str) -> Result<HostScreen, String> {
    let v = parse_json_object_from_stdout(stdout).map_err(|e| format!("invalid start response JSON: {e}"))?;
    let name = v.get("name").and_then(|s| s.as_str()).ok_or_else(|| "start response missing screen name".to_owned())?.to_owned();
    rex_sock::validate_screen_name(&name)?;
    Ok(HostScreen {
        name,
        title: v.get("title").and_then(|s| s.as_str()).filter(|t| !t.trim().is_empty()).map(str::to_owned),
        state: HostScreenState::Running,
        width: v.get("width").and_then(|n| n.as_u64()).unwrap_or(1440) as u32,
        height: v.get("height").and_then(|n| n.as_u64()).unwrap_or(900) as u32,
        scale: v.get("scale").and_then(|n| n.as_f64()).filter(|s| s.is_finite() && *s > 0.0).unwrap_or(1.0),
        sound: v.get("sound").and_then(|b| b.as_bool()).unwrap_or(false),
        pid: v.get("pid").and_then(|n| n.as_i64()).and_then(|p| i32::try_from(p).ok()),
        started: v.get("started").and_then(|n| n.as_u64()),
        last_used: v.get("last_used").and_then(|n| n.as_u64()),
        actions: v.get("actions").and_then(|n| n.as_u64()).map(|n| n as usize),
        controlled: v.get("controlled").and_then(|b| b.as_bool()).unwrap_or(false),
        owner: None,
        started_by: None,
    })
}

async fn read_bounded_line_async<R: AsyncBufRead + Unpin>(r: &mut R, max_bytes: usize) -> io::Result<Option<Vec<u8>>> {
    let mut out = Vec::new();
    loop {
        let avail = r.fill_buf().await?;
        if avail.is_empty() {
            return if out.is_empty() { Ok(None) } else { Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated JSON line before newline")) };
        }
        if let Some(pos) = avail.iter().position(|&b| b == b'\n') {
            if out.len() + pos + 1 > max_bytes {
                r.consume(pos + 1);
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("JSON line exceeds {max_bytes} bytes")));
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
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("JSON line exceeds {max_bytes} bytes")));
        }
        out.extend_from_slice(avail);
        r.consume(n);
    }
}

async fn write_response_async(w: &mut tokio::net::unix::OwnedWriteHalf, resp: &RexSockResponse, timeout: Duration) -> io::Result<()> {
    rex_sock::validate_response(resp).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut bytes = serde_json::to_vec(resp).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    bytes.push(b'\n');
    if bytes.len() > MAX_RESPONSE_LINE_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("RexSockResponse ({} bytes) exceeds {MAX_RESPONSE_LINE_BYTES}", bytes.len())));
    }
    tokio::time::timeout(timeout, async {
        w.write_all(&bytes).await?;
        w.flush().await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "timed out writing response to local client"))?
}

/// Resolve immediately if a `watch::Receiver<bool>` is already `true` (even
/// when subscribed AFTER `send(true)`), or await the transition to `true`
/// (Agy Finding 1).
async fn wait_true(rx: &mut watch::Receiver<bool>) {
    while !*rx.borrow() {
        if rx.changed().await.is_err() {
            return;
        }
    }
}

async fn write_follow_line_cancellable(w: &mut tokio::net::unix::OwnedWriteHalf, line: &RexFollowLine, timeout: Duration, shutdown_rx: &mut watch::Receiver<bool>) -> io::Result<()> {
    rex_sock::validate_follow_line(line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut bytes = serde_json::to_vec(line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    bytes.push(b'\n');
    if bytes.len() > MAX_RESPONSE_LINE_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("RexFollowLine ({} bytes) exceeds {MAX_RESPONSE_LINE_BYTES}", bytes.len())));
    }
    tokio::select! {
        res = tokio::time::timeout(timeout, async {
            w.write_all(&bytes).await?;
            w.flush().await
        }) => {
            res.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "timed out writing follow update to slow client"))?
        }
        _ = wait_true(shutdown_rx) => {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "server shutting down"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, sync::atomic::AtomicUsize};

    /// Allocate a fresh, unpredictable, private `0700` directory under `/var/tmp`
    /// without ever deleting a pre-existing predictable path (Finding 10).
    fn scratch_dir(tag: &str) -> PathBuf {
        static SEQ: AtomicUsize = AtomicUsize::new(1);
        for _ in 0..64 {
            let id = SEQ.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
            let dir = PathBuf::from(format!("/var/tmp/olo-rex-r3-{tag}-{}-{id}-{nanos}", std::process::id()));
            let mut b = std::fs::DirBuilder::new();
            b.mode(0o700);
            if b.create(&dir).is_ok() {
                return dir;
            }
        }
        panic!("could not create private scratch dir for {tag}");
    }

    struct ScratchCleanup(PathBuf);
    impl Drop for ScratchCleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    type StreamQueue = VecDeque<Result<BoxedScreenStream, String>>;
    type StreamQueueMap = HashMap<(String, String), StreamQueue>;
    type StreamHandler = Box<dyn Fn(&ScreenTarget, &[String]) -> Result<BoxedScreenStream, String> + Send + Sync>;
    type CommandHandler = Box<dyn Fn(&ScreenTarget, &[String], Duration) -> ScreenCommandFuture + Send + Sync>;

    struct DoubleOpener {
        streams: Mutex<StreamQueueMap>,
        stream_fallback: Mutex<Option<StreamHandler>>,
        opened_streams: Mutex<Vec<(String, u64, Vec<String>)>>,
        commands: Mutex<Vec<(String, u64, Vec<String>)>>,
        command_handler: Mutex<CommandHandler>,
        opened_hosts: Mutex<Vec<String>>,
        open_host_ready: AtomicBool,
    }

    impl DoubleOpener {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                streams: Mutex::new(HashMap::new()),
                stream_fallback: Mutex::new(None),
                opened_streams: Mutex::new(Vec::new()),
                commands: Mutex::new(Vec::new()),
                command_handler: Mutex::new(Box::new(|_target, args, _timeout| {
                    let args = args.to_vec();
                    Box::pin(async move {
                        if args.first().map(String::as_str) == Some("--protocol") {
                            Ok(CommandExecOutput {
                                status: Some(0),
                                stdout: r#"{"proto":1,"commands":["follow","stream","history-stream","export"],"capabilities":["control-ack","control-lock","warning-status","history-frame-error","history-snapshot-done","export-part-ranges","stop-expected","stop-expected-started","delete-expected","clear-expected","discontiguous_part"]}"#.to_owned(),
                                stderr: String::new(),
                                truncated: false,
                            })
                        } else {
                            Ok(CommandExecOutput { status: Some(0), stdout: String::new(), stderr: String::new(), truncated: false })
                        }
                    })
                })),
                opened_hosts: Mutex::new(Vec::new()),
                open_host_ready: AtomicBool::new(true),
            })
        }

        fn push_stream(&self, host: &str, verb: &str, stream: BoxedScreenStream) {
            self.streams.lock().unwrap_or_else(|e| e.into_inner()).entry((host.to_owned(), verb.to_owned())).or_default().push_back(Ok(stream));
        }

        fn push_stream_err(&self, host: &str, verb: &str, err: impl Into<String>) {
            self.streams.lock().unwrap_or_else(|e| e.into_inner()).entry((host.to_owned(), verb.to_owned())).or_default().push_back(Err(err.into()));
        }

        fn set_command_handler(&self, f: impl Fn(&ScreenTarget, &[String], Duration) -> ScreenCommandFuture + Send + Sync + 'static) {
            *self.command_handler.lock().unwrap_or_else(|e| e.into_inner()) = Box::new(f);
        }

        fn recorded_streams(&self) -> Vec<(String, u64, Vec<String>)> {
            self.opened_streams.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }

        fn recorded_commands(&self) -> Vec<(String, u64, Vec<String>)> {
            self.commands.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }

        fn take_commands(&self) -> Vec<(String, Vec<String>)> {
            std::mem::take(&mut *self.commands.lock().unwrap_or_else(|e| e.into_inner())).into_iter().map(|(h, _, a)| (h, a)).collect()
        }

        fn recorded_open_hosts(&self) -> Vec<String> {
            self.opened_hosts.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    impl ScreenOpener for DoubleOpener {
        fn open_screen_stream(&self, target: ScreenTarget, args: Vec<String>) -> ScreenStreamFuture {
            let verb = args.first().cloned().unwrap_or_default();
            self.opened_streams.lock().unwrap_or_else(|e| e.into_inner()).push((target.host_key.clone(), target.conn_id, args.clone()));
            let queued = self.streams.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&(target.host_key.clone(), verb)).and_then(VecDeque::pop_front);
            let res = match queued {
                Some(r) => r,
                None => {
                    if let Some(fb) = self.stream_fallback.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                        fb(&target, &args)
                    } else {
                        Err(format!("no queued test stream for {} {:?}", target.host_key, args))
                    }
                }
            };
            Box::pin(async move { res })
        }

        fn run_screen_command(&self, target: ScreenTarget, args: Vec<String>, timeout: Duration) -> ScreenCommandFuture {
            self.commands.lock().unwrap_or_else(|e| e.into_inner()).push((target.host_key.clone(), target.conn_id, args.clone()));
            let h = self.command_handler.lock().unwrap_or_else(|e| e.into_inner());
            h(&target, &args, timeout)
        }

        fn open_host(&self, host_key: &str) -> Result<(), String> {
            if !self.open_host_ready.load(Ordering::Acquire) {
                return Err("Rex window isn’t ready yet.".to_owned());
            }
            self.opened_hosts.lock().unwrap_or_else(|e| e.into_inner()).push(host_key.to_owned());
            Ok(())
        }
    }

    fn sample_host_screen(name: &str, state: HostScreenState, started: Option<u64>, last_used: Option<u64>) -> HostScreen {
        HostScreen {
            name: name.to_owned(),
            title: None,
            state,
            width: 1440,
            height: 900,
            scale: 1.0,
            sound: false,
            pid: (state == HostScreenState::Running).then_some(12345),
            started,
            last_used,
            actions: Some(3),
            controlled: false,
            owner: None,
            started_by: None,
        }
    }

    fn sample_status(installed: Option<&str>, default_on_host: bool, reason: Option<&str>) -> HostScreenStatus {
        HostScreenStatus { installed: installed.map(str::to_owned), committed: installed.map(|_| 1_795_700_000), default_on_host, not_set_up_reason: reason.map(str::to_owned) }
    }

    async fn send_follow_event(w: &mut tokio::io::DuplexStream, ev: &FollowEvent) {
        let mut line = serde_json::to_vec(ev).unwrap();
        line.push(b'\n');
        w.write_all(&line).await.unwrap();
        w.flush().await.unwrap();
    }

    async fn rpc_once(sock: &Path, req: &RexSockRequest) -> RexSockResponse {
        let mut s = UnixStream::connect(sock).await.unwrap();
        let mut line = serde_json::to_vec(req).unwrap();
        line.push(b'\n');
        s.write_all(&line).await.unwrap();
        s.flush().await.unwrap();
        let (r, _w) = s.into_split();
        let mut reader = BufReader::new(r);
        let raw = tokio::time::timeout(Duration::from_secs(5), read_bounded_line_async(&mut reader, MAX_RESPONSE_LINE_BYTES)).await.expect("rpc timed out").unwrap().expect("rpc unexpected EOF");
        serde_json::from_slice(&raw).unwrap()
    }

    async fn read_next_follow_line(r: &mut BufReader<tokio::net::unix::OwnedReadHalf>) -> RexFollowLine {
        let raw = tokio::time::timeout(Duration::from_secs(5), read_bounded_line_async(r, MAX_RESPONSE_LINE_BYTES))
            .await
            .expect("timed out reading follow line")
            .unwrap()
            .expect("follow stream closed unexpectedly");
        serde_json::from_slice(&raw).unwrap()
    }

    async fn wait_follow_until<T>(r: &mut BufReader<tokio::net::unix::OwnedReadHalf>, mut pred: impl FnMut(&RexFollowLine) -> Option<T>) -> T {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let rem = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(!rem.is_zero(), "timed out waiting for expected follow event");
            let raw = tokio::time::timeout(rem, read_bounded_line_async(r, MAX_RESPONSE_LINE_BYTES))
                .await
                .expect("timed out waiting for follow line")
                .unwrap()
                .expect("follow stream closed before expected event");
            let line: RexFollowLine = serde_json::from_slice(&raw).unwrap();
            if let Some(v) = pred(&line) {
                return v;
            }
        }
    }

    async fn wait_state_file_until<T>(state_path: &Path, mut pred: impl FnMut(&RexScreensState) -> Option<T>) -> T {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(st) = rex_sock::read_state_file(state_path)
                && let Some(v) = pred(&st)
            {
                return v;
            }
            assert!(tokio::time::Instant::now() < deadline, "timed out waiting for state file condition");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn open_follow_client(sock: &Path) -> (BufReader<tokio::net::unix::OwnedReadHalf>, tokio::net::unix::OwnedWriteHalf, RexScreensState) {
        let s = UnixStream::connect(sock).await.unwrap();
        let (r, mut w) = s.into_split();
        w.write_all(b"{\"op\":\"follow\"}\n").await.unwrap();
        w.flush().await.unwrap();
        let mut reader = BufReader::new(r);
        let first = read_next_follow_line(&mut reader).await;
        match first {
            RexFollowLine::Snapshot { state } => (reader, w, state),
            other => panic!("expected Snapshot as first follow line, got {other:?}"),
        }
    }

    /// Connect a host in tests and deliver its initial snapshot, waiting until
    /// the ordered publisher has surfaced `Connected` + expected condition.
    async fn connect_and_seed(
        svc: &Arc<ScreensService>,
        opener: &Arc<DoubleOpener>,
        sock: &Path,
        key: &str,
        inst_id: u64,
        status: HostScreenStatus,
        screens: Vec<HostScreen>,
    ) -> (u64, tokio::io::DuplexStream) {
        let rt = tokio::runtime::Handle::current();
        let (fc, mut fh) = tokio::io::duplex(16 * 1024);
        opener.push_stream(key, "follow", Box::pin(fc));
        let conn_id = next_conn_id();
        svc.host_connected(&rt, key, inst_id, conn_id, 0, key, None);
        send_follow_event(&mut fh, &FollowEvent::Snapshot { proto: 1, status, screens }).await;
        let (mut fr, _fw, snap) = open_follow_client(sock).await;
        let ready = |s: &RexScreensState| s.hosts.iter().any(|h| h.key == key && h.state == RexHostConn::Connected);
        if !ready(&snap) {
            wait_follow_until(&mut fr, |line| match line {
                RexFollowLine::HostUpdated { host, .. } if host.key == key && host.state == RexHostConn::Connected => Some(()),
                _ => None,
            })
            .await;
        }
        (conn_id, fh)
    }

    // -----------------------------------------------------------------------
    // Core lifecycle, order, debounce, install transition, permissions
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn lifecycle_cases_host_order_last_used_debounce_and_install_transition() {
        let dir = scratch_dir("lifecycle");
        let _cleanup = ScratchCleanup(dir.clone());
        // Finding 37: pass a NON-EXISTENT leaf directory so `start_server` itself
        // creates it and we genuinely test server-created `0700` permissions!
        let srv_dir = dir.join("created-by-server");
        let sock = srv_dir.join("rex-test.sock");
        let state_path = srv_dir.join("state-test.json");
        let opener = DoubleOpener::new();
        let last_used_seen = Arc::new(Mutex::new(Vec::<LastUsedUpdate>::new()));
        let lu_cb = last_used_seen.clone();

        let svc = ScreensService::new(
            sock.clone(),
            state_path.clone(),
            std::process::id(),
            opener.clone(),
            Arc::new(|_, _| None),
            None,
            Some(Arc::new(move |u| lu_cb.lock().unwrap_or_else(|e| e.into_inner()).push(u))),
        );
        let id_a = next_host_instance_id();
        let id_b = next_host_instance_id();
        let id_c = next_host_instance_id();
        svc.set_initial_hosts(vec![
            ("alpha".into(), id_a, "Alpha".into(), RexHostConn::Disconnected),
            ("beta".into(), id_b, "Beta".into(), RexHostConn::Disconnected),
            ("gamma".into(), id_c, "Gamma".into(), RexHostConn::Disconnected),
        ]);
        let rt = tokio::runtime::Handle::current();
        assert!(!srv_dir.exists(), "server directory must not exist before start_server");
        svc.start_server(&rt).unwrap();

        // Finding 37: explicitly assert server-created dir `0700`, socket `0600`, and state file `0600` modes!
        assert_eq!(std::fs::metadata(&srv_dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&state_path).unwrap().permissions().mode() & 0o777, 0o600);

        let (mut freader, _fw, snap0) = open_follow_client(&sock).await;
        assert_eq!(snap0.hosts.iter().map(|h| h.key.as_str()).collect::<Vec<_>>(), vec!["alpha", "beta", "gamma"]);

        // 1. Alpha connects with screens; Beta with 0 screens; Gamma with Olo not installed.
        let (alpha_c, mut alpha_h) = tokio::io::duplex(16 * 1024);
        let (beta_c, mut beta_h) = tokio::io::duplex(16 * 1024);
        let (gamma_c, mut gamma_h) = tokio::io::duplex(16 * 1024);
        opener.push_stream("alpha", "follow", Box::pin(alpha_c));
        opener.push_stream("beta", "follow", Box::pin(beta_c));
        opener.push_stream("gamma", "follow", Box::pin(gamma_c));

        let ca1 = next_conn_id();
        let cb1 = next_conn_id();
        let cg1 = next_conn_id();
        svc.host_connected(&rt, "alpha", id_a, ca1, 0, "Alpha", None);
        svc.host_connected(&rt, "beta", id_b, cb1, 0, "Beta", None);
        svc.host_connected(&rt, "gamma", id_c, cg1, 0, "Gamma", None);

        send_follow_event(
            &mut alpha_h,
            &FollowEvent::Snapshot { proto: 1, status: sample_status(Some("2026.11.27"), true, None), screens: vec![sample_host_screen("qa", HostScreenState::Running, Some(1000), Some(2000))] },
        )
        .await;
        send_follow_event(&mut beta_h, &FollowEvent::Snapshot { proto: 1, status: sample_status(Some("2026.11.27"), false, None), screens: vec![] }).await;
        send_follow_event(&mut gamma_h, &FollowEvent::Snapshot { proto: 1, status: sample_status(None, false, Some("Olo isn’t installed yet.")), screens: vec![] }).await;

        wait_follow_until(&mut freader, |_| {
            let s = svc.snapshot();
            let a = s.hosts.iter().find(|h| h.key == "alpha")?;
            let b = s.hosts.iter().find(|h| h.key == "beta")?;
            let g = s.hosts.iter().find(|h| h.key == "gamma")?;
            (a.state == RexHostConn::Connected
                && a.can_control == Some(true)
                && a.screens.len() == 1
                && b.state == RexHostConn::Connected
                && b.installed.is_some()
                && g.state == RexHostConn::Connected
                && g.installed.is_none())
            .then_some(())
        })
        .await;

        let disk1 = wait_state_file_until(&state_path, |st| {
            let a = st.hosts.iter().find(|h| h.key == "alpha")?;
            (a.state == RexHostConn::Connected && a.can_control == Some(true) && a.screens.len() == 1).then(|| st.clone())
        })
        .await;
        assert_eq!(disk1.hosts.iter().map(|h| h.key.as_str()).collect::<Vec<_>>(), vec!["alpha", "beta", "gamma"]);
        assert_eq!(disk1.hosts[0].screens[0].actions, Some(3));
        assert_eq!(disk1.hosts[2].not_set_up_reason.as_deref(), Some("Olo isn’t installed yet."));

        // 2. LastUsed debounce: broadcasts ScreenLastUsed and invokes on_last_used,
        //    updates actions count, and does NOT rewrite state-<display>.json!
        let rev_before_lu = disk1.rev;
        let ino_before_lu = std::fs::metadata(&state_path).unwrap().ino();
        send_follow_event(&mut alpha_h, &FollowEvent::LastUsed { name: "qa".into(), last_used: 9_999, actions: Some(4) }).await;
        let (lu_ts, lu_name, lu_actions) = wait_follow_until(&mut freader, |line| match line {
            RexFollowLine::ScreenLastUsed { host, name, last_used, actions } if host == "alpha" => Some((*last_used, name.clone(), *actions)),
            _ => None,
        })
        .await;
        assert_eq!((lu_name.as_str(), lu_ts, lu_actions), ("qa", 9_999, Some(4)));
        // Same-ms action increment is also preserved AND forwarded on the wire (Item 12 / Finding 16 follow-up)!
        send_follow_event(&mut alpha_h, &FollowEvent::LastUsed { name: "qa".into(), last_used: 9_999, actions: Some(5) }).await;
        let _ =
            wait_follow_until(&mut freader, |line| matches!(line, RexFollowLine::ScreenLastUsed { host, name, last_used: 9_999, actions: Some(5) } if host == "alpha" && name == "qa").then_some(()))
                .await;
        assert_eq!(svc.snapshot().hosts[0].screens[0].actions, Some(5));
        assert_eq!(last_used_seen.lock().unwrap_or_else(|e| e.into_inner()).last().and_then(|u| u.actions), Some(5));
        assert_eq!(std::fs::metadata(&state_path).unwrap().ino(), ino_before_lu, "LastUsed alone must not rewrite state file");
        assert_eq!(rex_sock::read_state_file(&state_path).unwrap().rev, rev_before_lu);

        // 3. Gamma installs Olo mid-connection -> Status flips installed, clears & re-probes protocol.
        send_follow_event(&mut gamma_h, &FollowEvent::Status { status: sample_status(Some("2026.11.27"), true, None) }).await;
        wait_follow_until(&mut freader, |line| match line {
            RexFollowLine::HostUpdated { host, .. } if host.key == "gamma" && host.installed.as_deref() == Some("2026.11.27") && host.can_control == Some(true) => Some(()),
            _ => None,
        })
        .await;

        // 4. Alpha disconnects -> keeps last-known screens + sets disconnected_at.
        svc.host_state_changed("alpha", id_a, Some(ca1), 0, RexHostConn::Disconnected, None, Some(555_000));
        let disc_host = wait_follow_until(&mut freader, |line| match line {
            RexFollowLine::HostUpdated { host, .. } if host.key == "alpha" && host.state == RexHostConn::Disconnected => Some((**host).clone()),
            _ => None,
        })
        .await;
        assert_eq!(disc_host.disconnected_at, Some(555_000));
        assert_eq!(disc_host.screens.len(), 1);
        assert_eq!(disc_host.screens[0].name, "qa");

        // 5. Finding 26: Alpha reconnects -> BEFORE fresh snapshot arrives, stale
        //    rows MUST stay Connecting with disconnected_at preserved (never live!).
        let (alpha_c2, mut alpha_h2) = tokio::io::duplex(16 * 1024);
        opener.push_stream("alpha", "follow", Box::pin(alpha_c2));
        let ca2 = next_conn_id();
        svc.host_connected(&rt, "alpha", id_a, ca2, 0, "Alpha", None);
        let mid = wait_follow_until(&mut freader, |line| match line {
            RexFollowLine::HostUpdated { host, .. } if host.key == "alpha" && host.state == RexHostConn::Connecting => Some((**host).clone()),
            _ => None,
        })
        .await;
        assert_eq!(mid.disconnected_at, Some(555_000), "must retain disconnected_at until fresh snapshot arrives");
        assert_eq!(mid.screens[0].name, "qa");

        // Once fresh snapshot arrives, state flips to Connected and disconnected_at clears!
        send_follow_event(
            &mut alpha_h2,
            &FollowEvent::Snapshot { proto: 1, status: sample_status(Some("2026.11.27"), true, None), screens: vec![sample_host_screen("qa", HostScreenState::Stopped, None, Some(9_999))] },
        )
        .await;
        let live = wait_follow_until(&mut freader, |line| match line {
            RexFollowLine::HostUpdated { host, .. } if host.key == "alpha" && host.state == RexHostConn::Connected => Some((**host).clone()),
            _ => None,
        })
        .await;
        assert_eq!(live.disconnected_at, None);
        assert_eq!(live.screens[0].state, HostScreenState::Stopped);

        // 6. Beta removed -> HostRemoved emitted, remaining order [alpha, gamma].
        svc.host_removed("beta", id_b);
        let removed_key = wait_follow_until(&mut freader, |line| match line {
            RexFollowLine::HostRemoved { key, .. } => Some(key.clone()),
            _ => None,
        })
        .await;
        assert_eq!(removed_key, "beta");
        assert_eq!(svc.snapshot().hosts.iter().map(|h| h.key.as_str()).collect::<Vec<_>>(), vec!["alpha", "gamma"]);

        svc.shutdown();
        drop((alpha_h, alpha_h2, beta_h, gamma_h));
    }

    // -----------------------------------------------------------------------
    // Findings 1 & 2: Guarded Stop, ClearHistory, Delete (no Rex cache fallback)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn round3_f1_f2_guarded_lifecycle_forwards_expected_and_refuses_uncapable_helper() {
        let dir = scratch_dir("f1f2");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let opener = DoubleOpener::new();
        opener.set_command_handler(|target, args, _timeout| {
            let host = target.host_key.clone();
            let args = args.to_vec();
            Box::pin(async move {
                match args.first().map(String::as_str) {
                    Some("--protocol") => {
                        if host == "old-box" {
                            // Released helper advertises `stop-expected-started` WITHOUT `stop-expected`.
                            Ok(CommandExecOutput {
                                status: Some(0),
                                stdout: r#"{"proto":1,"commands":["follow","stream"],"capabilities":["control-ack","stop-expected-started"]}"#.into(),
                                stderr: String::new(),
                                truncated: false,
                            })
                        } else {
                            Ok(CommandExecOutput {
                                status: Some(0),
                                stdout: r#"{"proto":1,"commands":["follow","stream","history-stream","export"],"capabilities":["control-ack","control-lock","stop-expected","stop-expected-started","delete-expected","clear-expected"]}"#.into(),
                                stderr: String::new(),
                                truncated: false,
                            })
                        }
                    }
                    Some("delete") if args.windows(2).any(|w| w == ["--expected-last-used", "111"]) => {
                        Ok(CommandExecOutput { status: Some(75), stdout: String::new(), stderr: "qa has newer activity since you asked".into(), truncated: false })
                    }
                    Some("stop") if args.windows(2).any(|w| w == ["--expected-started", "999"]) => {
                        Ok(CommandExecOutput { status: Some(75), stdout: String::new(), stderr: "qa was started again since you asked".into(), truncated: false })
                    }
                    _ => Ok(CommandExecOutput { status: Some(0), stdout: String::new(), stderr: String::new(), truncated: false }),
                }
            })
        });

        let svc = ScreensService::new(sock.clone(), state_path.clone(), std::process::id(), opener.clone(), Arc::new(|_, _| None), None, None);
        let id_old = next_host_instance_id();
        let id_new = next_host_instance_id();
        svc.set_initial_hosts(vec![("old-box".into(), id_old, "Old Box".into(), RexHostConn::Disconnected), ("new-box".into(), id_new, "New Box".into(), RexHostConn::Disconnected)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();
        let (_co, _oh) =
            connect_and_seed(&svc, &opener, &sock, "old-box", id_old, sample_status(Some("2026.11.20"), true, None), vec![sample_host_screen("qa", HostScreenState::Running, Some(500), Some(1000))])
                .await;
        let (_cn, _nh) =
            connect_and_seed(&svc, &opener, &sock, "new-box", id_new, sample_status(Some("2026.11.27"), true, None), vec![sample_host_screen("qa", HostScreenState::Stopped, None, Some(2000))]).await;
        let _ = rpc_once(&sock, &RexSockRequest::Protocol { host: "old-box".into() }).await;
        let _ = rpc_once(&sock, &RexSockRequest::Protocol { host: "new-box".into() }).await;

        // 1. Uncapable helper: MUST refuse Stop, ClearHistory, and Delete with plain words and ZERO host dispatches!
        let before = opener.recorded_commands().len();
        let r_stop = rpc_once(&sock, &RexSockRequest::Stop { host: "old-box".into(), name: "qa".into(), expected_started: Some(500), expected_last_used: Some(1000) }).await;
        assert!(matches!(r_stop, RexSockResponse::Error { ref message } if message.contains("Update Olo")), "got: {r_stop:?}");
        let r_clear = rpc_once(&sock, &RexSockRequest::ClearHistory { host: "old-box".into(), name: "qa".into(), expected_started: Some(0), expected_last_used: Some(0) }).await;
        assert!(matches!(r_clear, RexSockResponse::Error { ref message } if message.contains("Update Olo")), "got: {r_clear:?}");
        let r_del = rpc_once(&sock, &RexSockRequest::Delete { host: "old-box".into(), name: "qa".into(), expected_started: Some(0), expected_last_used: Some(1000) }).await;
        assert!(matches!(r_del, RexSockResponse::Error { ref message } if message.contains("Update Olo")), "got: {r_del:?}");
        assert_eq!(opener.recorded_commands().len(), before, "refused commands must never invoke host helper");

        // 2. Capable helper: passes BOTH `--expected-started` and `--expected-last-used` straight through (including `0`)!
        assert_eq!(rpc_once(&sock, &RexSockRequest::Stop { host: "new-box".into(), name: "qa".into(), expected_started: Some(500), expected_last_used: Some(2000) }).await, RexSockResponse::Ok);
        assert!(opener.recorded_commands().last().is_some_and(|(_, _, a)| a == &["stop", "qa", "--expected-started", "500", "--expected-last-used", "2000"]));

        assert_eq!(rpc_once(&sock, &RexSockRequest::ClearHistory { host: "new-box".into(), name: "qa".into(), expected_started: Some(0), expected_last_used: Some(0) }).await, RexSockResponse::Ok);
        assert!(opener.recorded_commands().last().is_some_and(|(_, _, a)| a == &["history", "qa", "--clear", "--expected-started", "0", "--expected-last-used", "0"]));

        let stale_del = rpc_once(&sock, &RexSockRequest::Delete { host: "new-box".into(), name: "qa".into(), expected_started: Some(0), expected_last_used: Some(111) }).await;
        assert_eq!(stale_del, RexSockResponse::Error { message: "qa has newer activity since you asked".into() });
        assert!(opener.recorded_commands().last().is_some_and(|(_, _, a)| a == &["delete", "qa", "--expected-started", "0", "--expected-last-used", "111"]));

        let stale_stop = rpc_once(&sock, &RexSockRequest::Stop { host: "new-box".into(), name: "qa".into(), expected_started: Some(999), expected_last_used: None }).await;
        assert_eq!(stale_stop, RexSockResponse::Error { message: "qa was started again since you asked".into() });

        svc.shutdown();
    }

    // -----------------------------------------------------------------------
    // Findings 3 & 14: Connection identity pinning & no host resurrection
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn round3_f3_f14_inflight_command_cancelled_on_host_replace_and_removed_host_not_resurrected() {
        let dir = scratch_dir("f3f14");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let opener = DoubleOpener::new();
        let probe_entered = Arc::new(Notify::new());
        let probe_release = Arc::new(Notify::new());
        let pe = probe_entered.clone();
        let pr = probe_release.clone();

        let id1 = next_host_instance_id();
        let id2 = next_host_instance_id();
        opener.set_command_handler(move |target, args, _timeout| {
            let inst = target.host_instance_id;
            let args = args.to_vec();
            let pe = pe.clone();
            let pr = pr.clone();
            Box::pin(async move {
                if args.first().map(String::as_str) == Some("--protocol") && inst == id1 {
                    pe.notify_one();
                    pr.notified().await;
                    return Ok(CommandExecOutput {
                        status: Some(0),
                        stdout: r#"{"proto":1,"commands":["follow"],"capabilities":["stop-expected","stop-expected-started"]}"#.into(),
                        stderr: String::new(),
                        truncated: false,
                    });
                }
                Ok(CommandExecOutput { status: Some(0), stdout: r#"{"proto":1,"commands":["follow"],"capabilities":["control-ack","control-lock"]}"#.into(), stderr: String::new(), truncated: false })
            })
        });

        let svc = ScreensService::new(sock.clone(), state_path.clone(), std::process::id(), opener.clone(), Arc::new(|_, _| None), None, None);
        svc.set_initial_hosts(vec![("box".into(), id1, "Old Machine".into(), RexHostConn::Disconnected)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();

        let (c1, mut h1) = tokio::io::duplex(8192);
        opener.push_stream("box", "follow", Box::pin(c1));
        let conn1 = next_conn_id();
        svc.host_connected(&rt, "box", id1, conn1, 0, "Old Machine", None);
        send_follow_event(&mut h1, &FollowEvent::Snapshot { proto: 1, status: sample_status(Some("2026.11.27"), true, None), screens: vec![] }).await;
        probe_entered.notified().await;

        // Subscribe to the deterministic `probe_wait_notify` barrier BEFORE
        // spawning `stop_task`, so we never rely on a 30 ms sleep (Agy Finding 10).
        let stop_waiting = {
            let st = svc.inner.lock().unwrap_or_else(|e| e.into_inner());
            st.hosts.get("box").unwrap().probe_wait_notify.clone()
        };
        let stop_waiting_fut = stop_waiting.notified();
        tokio::pin!(stop_waiting_fut);
        stop_waiting_fut.as_mut().enable();
        // Start a Stop on Old Machine that awaits the in-flight `--protocol` probe.
        let sock_c = sock.clone();
        let stop_task = tokio::spawn(async move { rpc_once(&sock_c, &RexSockRequest::Stop { host: "box".into(), name: "qa".into(), expected_started: Some(100), expected_last_used: None }).await });
        tokio::time::timeout(Duration::from_secs(3), stop_waiting_fut).await.expect("stop_task never reached probe_lock");

        // Remove Old Machine and replace key `"box"` with New Machine (`id2`)!
        svc.host_removed("box", id1);
        // Late notification for removed `id1` MUST NOT resurrect `"box"`!
        svc.host_connected(&rt, "box", id1, next_conn_id(), 0, "Ghost", None);
        assert!(svc.snapshot().hosts.is_empty(), "late host_connected for removed instance must NOT resurrect host");

        // Add New Machine under the same key `"box"` with fresh `id2`.
        svc.host_added("box", id2, "New Machine", RexHostConn::Disconnected);
        let (c2, mut h2) = tokio::io::duplex(8192);
        opener.push_stream("box", "follow", Box::pin(c2));
        let conn2 = next_conn_id();
        svc.host_connected(&rt, "box", id2, conn2, 0, "New Machine", None);
        send_follow_event(&mut h2, &FollowEvent::Snapshot { proto: 1, status: sample_status(Some("2026.11.27"), true, None), screens: vec![] }).await;
        // Late disconnect from Old Machine (`id1` / `conn1`) MUST NOT disconnect New Machine!
        svc.host_state_changed("box", id1, Some(conn1), 0, RexHostConn::Disconnected, None, None);

        // Release old probe -> in-flight Stop MUST fail as disconnected and NEVER dispatch to New Machine (`conn2`)!
        probe_release.notify_one();
        let stop_resp = tokio::time::timeout(Duration::from_secs(2), stop_task).await.unwrap().unwrap();
        assert!(matches!(stop_resp, RexSockResponse::Error { ref message } if message.contains("disconnected")), "got: {stop_resp:?}");
        assert!(!opener.recorded_commands().iter().any(|(h, cid, a)| h == "box" && *cid == conn2 && a.first().map(String::as_str) == Some("stop")), "in-flight command crossed over to New Machine!");
        // New Machine stays Connected!
        assert_eq!(svc.snapshot().hosts[0].label, "New Machine");

        svc.shutdown();
        drop((h1, h2));
    }

    // -----------------------------------------------------------------------
    // Finding 4: Pretty-printed multi-line `olo-screen start` JSON
    // -----------------------------------------------------------------------

    #[test]
    fn round3_f4_parse_pretty_printed_multiline_start_json() {
        let actual_cli_stdout = "{\n  \"name\": \"qa\",\n  \"wayland_display\": \"olo-screen-qa\",\n  \"olo_socket\": \"/run/user/1000/olo-screens/qa.sock\",\n  \"pid\": 4242,\n  \"width\": 1440,\n  \"height\": 900,\n  \"scale\": 1.5,\n  \"sound\": true,\n  \"started\": 1730000000\n}\n";
        let sc = parse_started_screen_json(actual_cli_stdout).expect("must parse multi-line pretty-printed olo-screen start JSON");
        assert_eq!(sc.name, "qa");
        assert_eq!(sc.pid, Some(4242));
        assert_eq!(sc.started, Some(1730000000));
        assert_eq!(sc.scale, 1.5);
        assert!(sc.sound);
    }

    // -----------------------------------------------------------------------
    // Findings 5, 21, 22, 23: Relays (backpressure, startup confirm, invalid
    // export ranges, text-only history refusal, admission permits)
    // -----------------------------------------------------------------------

    struct BackpressureBarrierStream {
        inner: tokio::io::DuplexStream,
        blocked: Arc<Notify>,
    }

    impl tokio::io::AsyncRead for BackpressureBarrierStream {
        fn poll_read(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &mut tokio::io::ReadBuf<'_>) -> std::task::Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_read(cx, buf)
        }
    }

    impl tokio::io::AsyncWrite for BackpressureBarrierStream {
        fn poll_write(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &[u8]) -> std::task::Poll<io::Result<usize>> {
            let poll = Pin::new(&mut self.inner).poll_write(cx, buf);
            if poll.is_pending() {
                self.blocked.notify_waiters();
            }
            poll
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(cx)
        }

        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    #[tokio::test]
    async fn round3_f5_backpressured_leftover_and_midstream_close_on_client_drop() {
        let dir = scratch_dir("f5-bp");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let listener = UnixListener::bind(&sock).unwrap();

        // Case A: backpressured `leftover` tears down immediately on client drop!
        let (accepted, client) = tokio::join!(listener.accept(), UnixStream::connect(&sock));
        let (sr, sw) = accepted.unwrap().0.into_split();
        let client = client.unwrap();
        let (relay_remote, _never_read) = tokio::io::duplex(64);
        let bp_a = Arc::new(Notify::new());
        let bp_a_wait = bp_a.notified();
        tokio::pin!(bp_a_wait);
        bp_a_wait.as_mut().enable();
        let (_h_tx, h_rx) = watch::channel(false);
        let (_s_tx, s_rx) = watch::channel(false);
        let watchers = make_relay_watchers(sr.as_ref().as_raw_fd()).unwrap();
        let task = tokio::spawn(relay_bidirectional(sr, sw, watchers, vec![b'X'; 4096], Vec::new(), Box::pin(BackpressureBarrierStream { inner: relay_remote, blocked: bp_a.clone() }), h_rx, s_rx));
        tokio::time::timeout(Duration::from_secs(2), bp_a_wait).await.expect("leftover write never entered backpressure");
        drop(client);
        tokio::time::timeout(Duration::from_millis(800), task).await.expect("relay hung on backpressured leftover").unwrap();

        // Case B: backpressured mid-stream write tears down on client `shutdown(Write)`!
        let (accepted2, client2) = tokio::join!(listener.accept(), UnixStream::connect(&sock));
        let (sr2, sw2) = accepted2.unwrap().0.into_split();
        let mut client2 = client2.unwrap();
        let (relay_remote2, _never_read2) = tokio::io::duplex(64);
        let bp_b = Arc::new(Notify::new());
        let bp_b_wait = bp_b.notified();
        tokio::pin!(bp_b_wait);
        bp_b_wait.as_mut().enable();
        let (_h_tx2, h_rx2) = watch::channel(false);
        let (_s_tx2, s_rx2) = watch::channel(false);
        let watchers2 = make_relay_watchers(sr2.as_ref().as_raw_fd()).unwrap();
        let task2 = tokio::spawn(relay_bidirectional(sr2, sw2, watchers2, Vec::new(), Vec::new(), Box::pin(BackpressureBarrierStream { inner: relay_remote2, blocked: bp_b.clone() }), h_rx2, s_rx2));
        client2.write_all(&vec![b'Y'; 4096]).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), bp_b_wait).await.expect("midstream write never entered backpressure");
        client2.shutdown().await.unwrap();
        tokio::time::timeout(Duration::from_millis(800), task2).await.expect("relay hung on backpressured write after client write-close").unwrap();
    }

    #[tokio::test]
    async fn round3_f21_f22_f23_startup_confirm_export_validation_and_relay_limits() {
        let dir = scratch_dir("f21-23");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let opener = DoubleOpener::new();
        opener.set_command_handler(|target, args, _timeout| {
            let host = target.host_key.clone();
            let args = args.to_vec();
            Box::pin(async move {
                if args.first().map(String::as_str) == Some("--protocol") {
                    if host == "text-old" {
                        return Ok(CommandExecOutput { status: Some(2), stdout: String::new(), stderr: "unknown flag --protocol".into(), truncated: false });
                    }
                    return Ok(CommandExecOutput {
                        status: Some(0),
                        stdout: r#"{"proto":1,"commands":["follow","stream","history-stream","export"],"capabilities":["control-ack","control-lock"]}"#.into(),
                        stderr: String::new(),
                        truncated: false,
                    });
                }
                Ok(CommandExecOutput { status: Some(0), stdout: String::new(), stderr: String::new(), truncated: false })
            })
        });

        let svc = ScreensService::new(sock.clone(), state_path.clone(), std::process::id(), opener.clone(), Arc::new(|_, _| None), None, None);
        let id_old = next_host_instance_id();
        let id_dev = next_host_instance_id();
        svc.set_initial_hosts(vec![("text-old".into(), id_old, "text-old".into(), RexHostConn::Disconnected), ("dev-box".into(), id_dev, "dev-box".into(), RexHostConn::Disconnected)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();
        let (_co, _oh) = connect_and_seed(&svc, &opener, &sock, "text-old", id_old, sample_status(Some("2026.11.20"), true, None), vec![]).await;
        let (_cd, _dh) =
            connect_and_seed(&svc, &opener, &sock, "dev-box", id_dev, sample_status(Some("2026.11.27"), true, None), vec![sample_host_screen("qa", HostScreenState::Running, Some(100), Some(200))])
                .await;
        let _ = rpc_once(&sock, &RexSockRequest::Protocol { host: "text-old".into() }).await;
        let _ = rpc_once(&sock, &RexSockRequest::Protocol { host: "dev-box".into() }).await;

        // Finding 21a: text-only legacy helper refused cleanly before opening stream!
        let streams_before = opener.recorded_streams().len();
        let r_hist = rpc_once(&sock, &RexSockRequest::History { host: "text-old".into(), name: "qa".into() }).await;
        assert!(matches!(r_hist, RexSockResponse::Error { ref message } if message.contains("Update")), "got: {r_hist:?}");
        assert_eq!(opener.recorded_streams().len(), streams_before);

        // Finding 21b: SSH ExecOutput failing on first read returns Error instead of Ready!
        struct ImmediateFailStream;
        impl tokio::io::AsyncRead for ImmediateFailStream {
            fn poll_read(self: Pin<&mut Self>, _cx: &mut std::task::Context<'_>, _buf: &mut tokio::io::ReadBuf<'_>) -> std::task::Poll<io::Result<()>> {
                std::task::Poll::Ready(Err(io::Error::other("the host has no screens-capable Olo helper")))
            }
        }
        impl tokio::io::AsyncWrite for ImmediateFailStream {
            fn poll_write(self: Pin<&mut Self>, _cx: &mut std::task::Context<'_>, buf: &[u8]) -> std::task::Poll<io::Result<usize>> {
                std::task::Poll::Ready(Ok(buf.len()))
            }
            fn poll_flush(self: Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }
            fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }
        }
        opener.push_stream("dev-box", "stream", Box::pin(ImmediateFailStream));
        let r_fail = rpc_once(&sock, &RexSockRequest::Stream { host: "dev-box".into(), name: "qa".into() }).await;
        assert!(matches!(r_fail, RexSockResponse::Error { ref message } if message.contains("no screens-capable Olo helper")), "got: {r_fail:?}");

        // Finding 21c: prefetched bytes from startup confirmation are preserved verbatim after Ready!
        let (srv_s, mut rem_s) = tokio::io::duplex(4096);
        opener.push_stream("dev-box", "stream", Box::pin(srv_s));
        let rem_task = tokio::spawn(async move {
            rem_s.write_all(&[0x01, 0x00, 0xfe, 0xff]).await.unwrap();
            let mut back = [0u8; 4];
            rem_s.read_exact(&mut back).await.unwrap();
            assert_eq!(&back, &[0x10, 0x02, 0x03, 0x04]);
            let mut eof = [0u8; 4];
            assert_eq!(tokio::time::timeout(Duration::from_secs(2), rem_s.read(&mut eof)).await.unwrap().unwrap(), 0);
        });
        let mut cli = UnixStream::connect(&sock).await.unwrap();
        let mut req = serde_json::to_vec(&RexSockRequest::Stream { host: "dev-box".into(), name: "qa".into() }).unwrap();
        req.push(b'\n');
        req.extend_from_slice(&[0x10, 0x02, 0x03, 0x04]); // pipelined binary bytes after request line!
        cli.write_all(&req).await.unwrap();
        let (cr, _cw) = cli.into_split();
        let mut cr = BufReader::new(cr);
        let ready: RexSockResponse = serde_json::from_slice(&read_bounded_line_async(&mut cr, MAX_RESPONSE_LINE_BYTES).await.unwrap().unwrap()).unwrap();
        assert_eq!(ready, RexSockResponse::Ready { width: Some(1440), height: Some(900), scale: Some(1.0) });
        let mut got = [0u8; 4];
        cr.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, &[0x01, 0x00, 0xfe, 0xff]);
        drop((cr, _cw));
        rem_task.await.unwrap();

        // Findings 22 & 37: reversed export range against CONNECTED host refused with ZERO dispatches!
        let streams_before = opener.recorded_streams().len();
        let r_exp = rpc_once(&sock, &RexSockRequest::Export { host: "dev-box".into(), name: "qa".into(), args: vec!["--from".into(), "5".into(), "--to".into(), "2".into()] }).await;
        assert!(matches!(r_exp, RexSockResponse::Error { .. }), "got: {r_exp:?}");
        assert_eq!(opener.recorded_streams().len(), streams_before, "invalid export range must not open stream");

        // Finding 23: per-host relay permit limit caps concurrent relays cleanly.
        let mut open_clients = Vec::new();
        let mut open_remotes = Vec::new();
        for _ in 0..MAX_RELAYS_PER_HOST {
            let (s, mut r) = tokio::io::duplex(1024);
            r.write_all(b"H").await.unwrap();
            opener.push_stream("dev-box", "stream", Box::pin(s));
            let mut c = UnixStream::connect(&sock).await.unwrap();
            let mut line = serde_json::to_vec(&RexSockRequest::Stream { host: "dev-box".into(), name: "qa".into() }).unwrap();
            line.push(b'\n');
            c.write_all(&line).await.unwrap();
            let (cr, cw) = c.into_split();
            let mut br = BufReader::new(cr);
            let resp: RexSockResponse = serde_json::from_slice(&read_bounded_line_async(&mut br, MAX_RESPONSE_LINE_BYTES).await.unwrap().unwrap()).unwrap();
            assert!(matches!(resp, RexSockResponse::Ready { .. }));
            open_clients.push((br, cw));
            open_remotes.push(r);
        }
        let r_over = rpc_once(&sock, &RexSockRequest::Stream { host: "dev-box".into(), name: "qa".into() }).await;
        assert!(matches!(r_over, RexSockResponse::Error { ref message } if message.contains("Too many open screen streams")), "got: {r_over:?}");
        drop((open_clients, open_remotes));

        svc.shutdown();
    }

    // -----------------------------------------------------------------------
    // Findings 6, 7, 35: Two-instance exclusion, directory verification,
    // non-socket refusal, OpenHost readiness, startup write failure cleanup
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn round3_f6_f7_f35_dir_verification_two_instance_lock_and_open_host_readiness() {
        let dir = scratch_dir("f6-7-35");
        let _cleanup = ScratchCleanup(dir.clone());
        let rt = tokio::runtime::Handle::current();

        // 1. Finding 7: pre-existing `0755` dir refused WITHOUT modifying its mode!
        let shared = dir.join("shared-0755");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).unwrap();
        let svc_shared = ScreensService::new(shared.join("rex.sock"), shared.join("state.json"), 1, DoubleOpener::new(), Arc::new(|_, _| None), None, None);
        assert!(svc_shared.start_server(&rt).is_err());
        assert_eq!(std::fs::metadata(&shared).unwrap().permissions().mode() & 0o777, 0o755, "must not chmod pre-existing shared dir");

        // 2. Finding 7: symlinked dir refused!
        let real_target = dir.join("real-0700");
        std::fs::create_dir(&real_target).unwrap();
        std::fs::set_permissions(&real_target, std::fs::Permissions::from_mode(0o700)).unwrap();
        let sym_dir = dir.join("sym-dir");
        std::os::unix::fs::symlink(&real_target, &sym_dir).unwrap();
        let svc_sym = ScreensService::new(sym_dir.join("rex.sock"), sym_dir.join("state.json"), 1, DoubleOpener::new(), Arc::new(|_, _| None), None, None);
        assert!(svc_sym.start_server(&rt).is_err());

        // 3. Finding 6: refusing to remove a non-socket regular file at socket path!
        let reg_dir = dir.join("reg-0700");
        std::fs::create_dir(&reg_dir).unwrap();
        std::fs::set_permissions(&reg_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let reg_file = reg_dir.join("rex.sock");
        std::fs::write(&reg_file, b"important user file").unwrap();
        let svc_reg = ScreensService::new(reg_file.clone(), reg_dir.join("state.json"), 1, DoubleOpener::new(), Arc::new(|_, _| None), None, None);
        assert!(svc_reg.start_server(&rt).is_err());
        assert_eq!(std::fs::read(&reg_file).unwrap(), b"important user file");

        // 4. Finding 6: two live instances cannot unlink or clobber each other; handoff after shutdown works!
        let priv_dir = dir.join("priv-0700");
        std::fs::create_dir(&priv_dir).unwrap();
        std::fs::set_permissions(&priv_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let sock = priv_dir.join("rex.sock");
        let state_path = priv_dir.join("state.json");
        let op1 = DoubleOpener::new();
        op1.open_host_ready.store(false, Ordering::Release);
        let svc1 = ScreensService::new(sock.clone(), state_path.clone(), 1001, op1.clone(), Arc::new(|_, _| None), None, None);
        svc1.set_initial_hosts(vec![("dev-box".into(), next_host_instance_id(), "dev-box".into(), RexHostConn::Disconnected)]);
        svc1.start_server(&rt).unwrap();

        // Finding 35: OpenHost before UI is ready returns explicit Error instead of silent Ok!
        let oh_early = rpc_once(&sock, &RexSockRequest::OpenHost { host: "dev-box".into() }).await;
        assert!(matches!(oh_early, RexSockResponse::Error { ref message } if message.contains("ready")), "got: {oh_early:?}");
        op1.open_host_ready.store(true, Ordering::Release);
        assert_eq!(rpc_once(&sock, &RexSockRequest::OpenHost { host: "dev-box".into() }).await, RexSockResponse::Ok);
        assert_eq!(op1.recorded_open_hosts(), vec!["dev-box".to_owned()]);

        let svc2 = ScreensService::new(sock.clone(), state_path.clone(), 1002, DoubleOpener::new(), Arc::new(|_, _| None), None, None);
        assert!(svc2.start_server(&rt).is_err(), "live second instance must fail flock");
        assert!(matches!(rpc_once(&sock, &RexSockRequest::List).await, RexSockResponse::Snapshot { state } if state.pid == 1001));

        // Clean shutdown of svc1 releases flock -> svc2 takes over cleanly!
        svc1.shutdown();
        assert!(!sock.exists());
        assert!(!state_path.exists());
        svc2.start_server(&rt).unwrap();
        assert!(matches!(rpc_once(&sock, &RexSockRequest::List).await, RexSockResponse::Snapshot { state } if state.pid == 1002));
        // Stale second call to svc1.shutdown() must NOT touch svc2's files!
        svc1.shutdown();
        assert!(sock.exists());
        assert_eq!(rex_sock::read_state_file(&state_path).unwrap().pid, 1002);
        svc2.shutdown();

        // 5. Findings 35 & 37: when socket bind SUCCEEDS in a valid 0700 dir,
        //    but the initial atomic state-file rename fails (because a non-empty
        //    directory blocks `state.json`), `start_server` propagates the error
        //    AND unlinks the already-bound socket!
        let ok_sock_dir = dir.join("sock-ok");
        std::fs::create_dir(&ok_sock_dir).unwrap();
        std::fs::set_permissions(&ok_sock_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let bad_state_sock = ok_sock_dir.join("rex.sock");
        let blocking_state_dir = ok_sock_dir.join("state.json");
        std::fs::create_dir(&blocking_state_dir).unwrap();
        std::fs::write(blocking_state_dir.join("blocker"), b"blocks renameat").unwrap();
        let svc_bad_state = ScreensService::new(bad_state_sock.clone(), blocking_state_dir.clone(), 1, DoubleOpener::new(), Arc::new(|_, _| None), None, None);
        assert!(svc_bad_state.start_server(&rt).is_err(), "rename failure over non-empty dir must fail start_server");
        assert!(!bad_state_sock.exists(), "failed initial state write after bind must unlink socket");
    }

    // -----------------------------------------------------------------------
    // Findings 9, 15, 16, 25: Aggregate budget, ordered publish, lag resync
    // skipping stale LastUsed, and `_legacy` name consistency
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn round3_f9_f16_f25_budget_lag_resync_and_underscore_names() {
        let dir = scratch_dir("f9-16-25");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let opener = DoubleOpener::new();
        let svc = ScreensService::new(sock.clone(), state_path.clone(), std::process::id(), opener.clone(), Arc::new(|_, _| None), None, None);
        let id_a = next_host_instance_id();
        let id_b = next_host_instance_id();
        svc.set_initial_hosts(vec![("noisy".into(), id_a, "Noisy".into(), RexHostConn::Disconnected), ("clean".into(), id_b, "Clean".into(), RexHostConn::Disconnected)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();

        // Finding 9: noisy host sends oversized titles & many screens; clean host stays online & valid!
        let mut big_screens = Vec::new();
        for i in 0..200 {
            let mut s = sample_host_screen(&format!("sc-{i}"), HostScreenState::Stopped, None, Some(1000));
            s.title = Some("T".repeat(10_000));
            big_screens.push(s);
        }
        let (_ca, mut ha) = connect_and_seed(&svc, &opener, &sock, "noisy", id_a, sample_status(Some("2026.11.27"), true, None), big_screens).await;
        let (_cb, mut hb) = connect_and_seed(
            &svc,
            &opener,
            &sock,
            "clean",
            id_b,
            sample_status(Some("2026.11.27"), true, None),
            vec![sample_host_screen("_legacy", HostScreenState::Running, Some(100), Some(1000)), sample_host_screen("qa", HostScreenState::Stopped, None, Some(500))],
        )
        .await;

        let snap = svc.snapshot();
        rex_sock::validate_state(&snap).expect("aggregate snapshot must always validate");
        let clean_host = snap.hosts.iter().find(|h| h.key == "clean").unwrap();
        assert_eq!(clean_host.screens.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["qa"]);

        // Finding 25: `_legacy` LastUsed must be ignored and must NOT disconnect follower!
        let (mut freader, _fw, _) = open_follow_client(&sock).await;
        send_follow_event(&mut hb, &FollowEvent::LastUsed { name: "_legacy".into(), last_used: 9000, actions: Some(9) }).await;
        send_follow_event(&mut hb, &FollowEvent::LastUsed { name: "qa".into(), last_used: 600, actions: Some(4) }).await;
        wait_follow_until(&mut freader, |line| matches!(line, RexFollowLine::ScreenLastUsed { host, name, last_used: 600, actions: Some(4) } if host == "clean" && name == "qa").then_some(())).await;

        // Findings 15 & 16: open a second follower, stall its kernel socket buffer
        // with large `noisy` host updates so `handle_follow` genuinely falls behind
        // `FOLLOW_BROADCAST_CAP`, flood `LastUsed` + clear (`last_used: None`),
        // then verify lag-resync delivers a Snapshot with `last_used == None`,
        // NEVER replays any pre-clear `ScreenLastUsed`, and maintains strictly
        // monotonic `rev` across all `HostUpdated` lines!
        let (mut lag_reader, _lag_w, _) = open_follow_client(&sock).await;
        for step in 0..8u64 {
            let mut big_touch = sample_host_screen("sc-0", HostScreenState::Running, Some(100 + step), Some(2000 + step));
            big_touch.title = Some(format!("step-{step}-{}", "X".repeat(3000)));
            send_follow_event(&mut ha, &FollowEvent::Screen { screen: Box::new(big_touch) }).await;
        }
        // Drain `freader` so the publisher has definitely pushed the big updates.
        let mut last_rev = 0u64;
        wait_follow_until(&mut freader, |line| {
            if let RexFollowLine::HostUpdated { rev, host } = line {
                assert!(*rev > last_rev, "HostUpdated rev must be strictly monotonic: {rev} <= {last_rev}");
                last_rev = *rev;
                if host.key == "noisy" && host.screens.iter().any(|s| s.name == "sc-0" && s.started == Some(107)) {
                    return Some(());
                }
            }
            None
        })
        .await;
        for i in 1..=(FOLLOW_BROADCAST_CAP as u64 + 60) {
            let ts = 1000 + i;
            send_follow_event(&mut hb, &FollowEvent::LastUsed { name: "qa".into(), last_used: ts, actions: None }).await;
            // Wait for `freader` to observe each broadcast so the coalescing
            // publisher genuinely emits > FOLLOW_BROADCAST_CAP broadcast lines
            // while `lag_reader` stays completely unread.
            wait_follow_until(&mut freader, |line| matches!(line, RexFollowLine::ScreenLastUsed { host, name, last_used, .. } if host == "clean" && name == "qa" && *last_used == ts).then_some(()))
                .await;
        }
        // Now clear qa's activity (`last_used: None`).
        send_follow_event(&mut hb, &FollowEvent::Screen { screen: Box::new(sample_host_screen("qa", HostScreenState::Stopped, None, None)) }).await;
        // Wait until publisher processed the clear!
        wait_follow_until(&mut freader, |line| match line {
            RexFollowLine::HostUpdated { rev, host } => {
                assert!(*rev > last_rev, "HostUpdated rev must be strictly monotonic: {rev} <= {last_rev}");
                last_rev = *rev;
                (host.key == "clean" && host.screens.iter().any(|s| s.name == "qa" && s.last_used.is_none())).then_some(())
            }
            _ => None,
        })
        .await;

        // Read from lagged subscriber until Snapshot resync arrives (with `qa.last_used == None`),
        // THEN send one fresh event (`99_999`) and verify NO stale `ScreenLastUsed` (1000..1400)
        // is ever replayed between the resync Snapshot and `99_999`!
        let mut saw_resync_snap = false;
        let mut saw_stale_after_resync = false;
        let mut lag_last_rev = 0u64;
        loop {
            let line = read_next_follow_line(&mut lag_reader).await;
            match line {
                RexFollowLine::Snapshot { state } => {
                    lag_last_rev = state.rev;
                    if state.hosts.iter().any(|h| h.key == "clean" && h.screens.iter().any(|s| s.name == "qa" && s.last_used.is_none())) {
                        saw_resync_snap = true;
                        send_follow_event(&mut hb, &FollowEvent::LastUsed { name: "qa".into(), last_used: 99_999, actions: None }).await;
                    }
                }
                RexFollowLine::HostUpdated { rev, .. } => {
                    assert!(rev > lag_last_rev, "lagged follower received out-of-order HostUpdated rev {rev} <= {lag_last_rev}");
                    lag_last_rev = rev;
                }
                RexFollowLine::ScreenLastUsed { last_used, .. } => {
                    if saw_resync_snap && last_used < 99_999 {
                        saw_stale_after_resync = true;
                    }
                    if last_used == 99_999 {
                        break;
                    }
                }
                _ => {}
            }
        }
        assert!(saw_resync_snap, "expected lag resync snapshot");
        assert!(!saw_stale_after_resync, "lag resync replayed stale ScreenLastUsed after cleared Snapshot!");

        // Findings 23 & 37: a follower that never reads hits `client_write_timeout`,
        // is actually disconnected (peer observes EOF after buffered bytes), and
        // releases its client permit without blocking other local RPC clients.
        drop((freader, _fw, lag_reader, _lag_w));
        let deadline_drain = Instant::now() + Duration::from_secs(3);
        while svc.client_permits.available_permits() < MAX_CONCURRENT_CLIENTS && Instant::now() < deadline_drain {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(svc.client_permits.available_permits(), MAX_CONCURRENT_CLIENTS);

        *svc.client_write_timeout.lock().unwrap_or_else(|e| e.into_inner()) = Duration::from_millis(80);
        let (mut stalled_r, _stalled_w, _) = open_follow_client(&sock).await;
        assert_eq!(svc.client_permits.available_permits(), MAX_CONCURRENT_CLIENTS - 1);
        for step in 8..24u64 {
            let mut big_touch = sample_host_screen("sc-0", HostScreenState::Running, Some(100 + step), Some(2000 + step));
            big_touch.title = Some(format!("stall-{step}-{}", "Y".repeat(3000)));
            send_follow_event(&mut ha, &FollowEvent::Screen { screen: Box::new(big_touch) }).await;
        }
        // Wait for server's write timeout to drop the stalled client and release its permit!
        let release_dl = Instant::now() + Duration::from_secs(5);
        while svc.client_permits.available_permits() < MAX_CONCURRENT_CLIENTS && Instant::now() < release_dl {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(svc.client_permits.available_permits(), MAX_CONCURRENT_CLIENTS, "stalled follower permit was never released");
        // Drain kernel buffer and confirm server closed the stalled client (actual EOF)!
        let mut drain_buf = [0u8; 8192];
        let saw_eof = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match stalled_r.read(&mut drain_buf).await {
                    Ok(0) => return true,
                    Ok(_) => continue,
                    Err(_) => return true,
                }
            }
        })
        .await
        .expect("timed out draining stalled follower for EOF");
        assert!(saw_eof, "stalled follower never reached EOF");

        let fast_list = tokio::time::timeout(Duration::from_secs(3), rpc_once(&sock, &RexSockRequest::List)).await.expect("stalled follower must not block List RPC");
        assert!(matches!(fast_list, RexSockResponse::Snapshot { .. }));

        svc.shutdown();
        drop((ha, hb));
    }

    // -----------------------------------------------------------------------
    // Findings 17, 18, 19, 20, 33: Follower backoff/incompat, helper_gen
    // invalidation, single deadline, control-lock, delayed Start reply
    // -----------------------------------------------------------------------

    #[test]
    fn round3_f20_can_control_requires_explicit_control_lock() {
        let no_lock = CachedProtocol {
            conn_id: 1,
            helper_gen: 1,
            install_gen: 1,
            installed: Some("1".into()),
            committed: None,
            proto: 1,
            commands: vec![],
            capabilities: vec!["control-ack".into(), "discontiguous_part".into()],
        };
        assert_eq!(compute_can_control(true, Some(&no_lock)), (Some(false), Some(OLD_OLO_CONTROL_REASON.to_owned())));
        let with_lock = CachedProtocol {
            conn_id: 1,
            helper_gen: 1,
            install_gen: 1,
            installed: Some("1".into()),
            committed: None,
            proto: 1,
            commands: vec![],
            capabilities: vec!["control-ack".into(), "control-lock".into()],
        };
        assert_eq!(compute_can_control(true, Some(&with_lock)), (Some(true), None));
    }

    #[tokio::test]
    async fn round3_f17_f18_f33_transient_vs_incompat_follower_and_delayed_start() {
        let dir = scratch_dir("f17-18-33");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let opener = DoubleOpener::new();
        let start_entered = Arc::new(Notify::new());
        let start_release = Arc::new(Notify::new());
        let se = start_entered.clone();
        let sr = start_release.clone();
        let proto_count = Arc::new(AtomicUsize::new(0));
        let pc = proto_count.clone();

        let start_count = Arc::new(AtomicUsize::new(0));
        let sc = start_count.clone();

        opener.set_command_handler(move |target, args, _timeout| {
            let host = target.host_key.clone();
            let args = args.to_vec();
            let se = se.clone();
            let sr = sr.clone();
            let pc = pc.clone();
            let sc = sc.clone();
            Box::pin(async move {
                match args.first().map(String::as_str) {
                    Some("--protocol") => {
                        if host == "dev-box" {
                            let n = pc.fetch_add(1, Ordering::Relaxed);
                            if n == 0 {
                                // First attempt fails with exit 127 (absent helper during upgrade) -> MUST NOT cache legacy!
                                return Ok(CommandExecOutput { status: Some(127), stdout: String::new(), stderr: "not found".into(), truncated: false });
                            }
                            if n == 1 {
                                // Finding 8: truncated protocol output MUST NOT cache legacy either!
                                return Ok(CommandExecOutput { status: Some(0), stdout: r#"{"proto":1"#.into(), stderr: String::new(), truncated: true });
                            }
                        }
                        Ok(CommandExecOutput {
                            status: Some(0),
                            stdout: r#"{"proto":1,"commands":["follow","stream"],"capabilities":["control-ack","control-lock"]}"#.into(),
                            stderr: String::new(),
                            truncated: false,
                        })
                    }
                    Some("start") => {
                        let sn = sc.fetch_add(1, Ordering::Relaxed);
                        if sn == 0 {
                            // Finding 8: truncated stdout on `start` must fail cleanly!
                            return Ok(CommandExecOutput { status: Some(0), stdout: r#"{"name":"qa""#.into(), stderr: String::new(), truncated: true });
                        }
                        se.notify_one();
                        sr.notified().await;
                        Ok(CommandExecOutput {
                            status: Some(0),
                            stdout: "{\n  \"name\": \"qa\",\n  \"width\": 1440,\n  \"height\": 900,\n  \"scale\": 1.0,\n  \"sound\": false,\n  \"pid\": 777,\n  \"started\": 2000\n}\n".into(),
                            stderr: String::new(),
                            truncated: false,
                        })
                    }
                    _ => Ok(CommandExecOutput { status: Some(0), stdout: String::new(), stderr: String::new(), truncated: false }),
                }
            })
        });

        let svc = ScreensService::new(sock.clone(), state_path.clone(), std::process::id(), opener.clone(), Arc::new(|_, _| None), None, None);
        let id_dev = next_host_instance_id();
        let id_flaky = next_host_instance_id();
        svc.set_initial_hosts(vec![("dev-box".into(), id_dev, "Dev".into(), RexHostConn::Disconnected), ("flaky-box".into(), id_flaky, "Flaky".into(), RexHostConn::Disconnected)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();

        // Subscribe BEFORE host_connected so fast transitions are never missed!
        let (mut fr, _fw, _) = open_follow_client(&sock).await;

        // Finding 17a: transient open error retries cleanly WITHOUT ever parking with OLD_HELPER_REASON!
        opener.push_stream_err("flaky-box", "follow", "temporary ssh channel reset");
        let (flaky_c, mut flaky_h) = tokio::io::duplex(16 * 1024);
        opener.push_stream("flaky-box", "follow", Box::pin(flaky_c));
        svc.host_connected(&rt, "flaky-box", id_flaky, next_conn_id(), 0, "Flaky", None);
        send_follow_event(&mut flaky_h, &FollowEvent::Snapshot { proto: 1, status: sample_status(Some("2026.11.27"), true, None), screens: vec![] }).await;
        wait_follow_until(&mut fr, |_| {
            let s = svc.snapshot();
            let f = s.hosts.iter().find(|h| h.key == "flaky-box")?;
            assert_ne!(f.not_set_up_reason.as_deref(), Some(OLD_HELPER_REASON), "transient error must not mark helper incompatible");
            (f.state == RexHostConn::Connected && f.installed.as_deref() == Some("2026.11.27")).then_some(())
        })
        .await;

        // Finding 17b: confirmed incompatibility parks immediately with OLD_HELPER_REASON,
        // and `wake_host_follower` restarts it + re-probes protocol cleanly (Finding 18)!
        opener.push_stream_err("dev-box", "follow", "the host has no screens-capable Olo helper");
        let cid = next_conn_id();
        svc.host_connected(&rt, "dev-box", id_dev, cid, 0, "Dev", None);
        wait_follow_until(&mut fr, |_| {
            let s = svc.snapshot();
            let d = s.hosts.iter().find(|h| h.key == "dev-box")?;
            (d.state == RexHostConn::Connected && d.not_set_up_reason.as_deref() == Some(OLD_HELPER_REASON)).then_some(())
        })
        .await;

        // First protocol probe (no premature connect-time probe!) fails with exit 127 -> MUST NOT cache legacy (Finding 18)!
        let first_proto = rpc_once(&sock, &RexSockRequest::Protocol { host: "dev-box".into() }).await;
        assert!(matches!(first_proto, RexSockResponse::Error { .. }), "got: {first_proto:?}");

        // Second protocol probe returns truncated -> rejected without caching legacy (Finding 8)!
        let trunc_proto = rpc_once(&sock, &RexSockRequest::Protocol { host: "dev-box".into() }).await;
        assert!(matches!(trunc_proto, RexSockResponse::Error { ref message } if message.contains("output limit")), "got: {trunc_proto:?}");

        // Helper is now installed -> wake_host_follower restarts follower and re-probes protocol!
        let (fc, mut fh) = tokio::io::duplex(16 * 1024);
        opener.push_stream("dev-box", "follow", Box::pin(fc));
        svc.wake_host_follower(&rt, "dev-box", id_dev);
        send_follow_event(&mut fh, &FollowEvent::Snapshot { proto: 1, status: sample_status(Some("2026.11.27"), true, None), screens: vec![] }).await;
        wait_follow_until(&mut fr, |_| {
            let s = svc.snapshot();
            let d = s.hosts.iter().find(|h| h.key == "dev-box")?;
            (d.installed.as_deref() == Some("2026.11.27") && d.can_control == Some(true)).then_some(())
        })
        .await;

        // Finding 8: truncated `start` output returns Error instead of corrupt screen!
        let trunc_start = rpc_once(&sock, &RexSockRequest::Start { host: "dev-box".into(), name: Some("qa".into()), keep: false, size: None, scale: None, sound: false }).await;
        assert!(matches!(trunc_start, RexSockResponse::Error { ref message } if message.contains("output limit")), "got: {trunc_start:?}");

        // Finding 33: delayed Start reply must NOT overwrite a newer follower Stopped+Title event!
        let sock_c = sock.clone();
        let start_task =
            tokio::spawn(async move { rpc_once(&sock_c, &RexSockRequest::Start { host: "dev-box".into(), name: Some("qa".into()), keep: true, size: None, scale: None, sound: false }).await });
        start_entered.notified().await;
        let mut stopped = sample_host_screen("qa", HostScreenState::Stopped, None, Some(2500));
        stopped.title = Some("Authoritative Title".into());
        send_follow_event(&mut fh, &FollowEvent::Screen { screen: Box::new(stopped) }).await;
        wait_follow_until(&mut fr, |line| match line {
            RexFollowLine::HostUpdated { host, .. } if host.key == "dev-box" && host.screens.iter().any(|s| s.name == "qa" && s.state == HostScreenState::Stopped) => Some(()),
            _ => None,
        })
        .await;
        start_release.notify_one();
        let started_resp = start_task.await.unwrap();
        assert!(matches!(started_resp, RexSockResponse::Started { .. }));
        let snap = svc.snapshot();
        let dev = snap.hosts.iter().find(|h| h.key == "dev-box").unwrap();
        let qa = dev.screens.iter().find(|s| s.name == "qa").unwrap();
        assert_eq!(qa.state, HostScreenState::Stopped, "delayed start reply rewound Stopped to Running");
        assert_eq!(qa.title.as_deref(), Some("Authoritative Title"));

        svc.shutdown();
        drop((fh, flaky_h));
    }

    // -----------------------------------------------------------------------
    // Findings 19 & 37: Barrier-proved stalled host isolation + total deadline
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn round3_f19_f37_stalled_host_blocked_on_barrier_does_not_delay_other_host() {
        let dir = scratch_dir("f19-37");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let opener = DoubleOpener::new();
        let slow_entered = Arc::new(Notify::new());
        let slow_release = Arc::new(Notify::new());
        let se = slow_entered.clone();
        let sr = slow_release.clone();
        let hang_forever = Arc::new(AtomicBool::new(false));
        let hf = hang_forever.clone();
        opener.set_command_handler(move |target, args, _timeout| {
            let host = target.host_key.clone();
            let args = args.to_vec();
            let se = se.clone();
            let sr = sr.clone();
            let hf = hf.clone();
            Box::pin(async move {
                if host == "slow-box" && args.first().map(String::as_str) == Some("rename") {
                    se.notify_one();
                    if hf.load(Ordering::Acquire) {
                        std::future::pending::<()>().await;
                    } else {
                        sr.notified().await;
                    }
                }
                if args.first().map(String::as_str) == Some("--protocol") {
                    return Ok(CommandExecOutput {
                        status: Some(0),
                        stdout: r#"{"proto":1,"commands":["follow"],"capabilities":["control-ack","control-lock"]}"#.into(),
                        stderr: String::new(),
                        truncated: false,
                    });
                }
                Ok(CommandExecOutput { status: Some(0), stdout: String::new(), stderr: String::new(), truncated: false })
            })
        });

        let svc = ScreensService::new(sock.clone(), state_path.clone(), std::process::id(), opener.clone(), Arc::new(|_, _| None), None, None);
        // Generous timeout during barrier isolation check so no scheduler pause can race a clock!
        *svc.rpc_timeout.lock().unwrap_or_else(|e| e.into_inner()) = Duration::from_secs(5);
        let id_s = next_host_instance_id();
        let id_f = next_host_instance_id();
        svc.set_initial_hosts(vec![("slow-box".into(), id_s, "slow-box".into(), RexHostConn::Disconnected), ("fast-box".into(), id_f, "fast-box".into(), RexHostConn::Disconnected)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();
        let (_cs, _sh) = connect_and_seed(&svc, &opener, &sock, "slow-box", id_s, sample_status(Some("2026.11.27"), true, None), vec![]).await;
        let (_cf, _fh) = connect_and_seed(&svc, &opener, &sock, "fast-box", id_f, sample_status(Some("2026.11.27"), true, None), vec![]).await;

        // 1. Launch slow RPC and wait with bounded timeout until it is parked on the barrier.
        let sock_slow = sock.clone();
        let slow_task = tokio::spawn(async move { rpc_once(&sock_slow, &RexSockRequest::Rename { host: "slow-box".into(), name: "qa".into(), title: Some("Blocked".into()) }).await });
        tokio::time::timeout(Duration::from_secs(5), slow_entered.notified()).await.expect("slow-box did not enter barrier");

        // Prove fast-box completes while slow-box is STILL held behind `slow_release` (zero clock race!).
        let fast_resp = tokio::time::timeout(Duration::from_secs(3), rpc_once(&sock, &RexSockRequest::Rename { host: "fast-box".into(), name: "qa".into(), title: Some("Quick".into()) }))
            .await
            .expect("fast-box was serialized behind slow-box!");
        assert_eq!(fast_resp, RexSockResponse::Ok);
        assert!(!slow_task.is_finished(), "slow-box must still be blocked on barrier after fast-box finished");
        slow_release.notify_one();
        assert_eq!(tokio::time::timeout(Duration::from_secs(5), slow_task).await.unwrap().unwrap(), RexSockResponse::Ok);

        // 2. Separately verify RPC deadline expiration cleanly returns timeout error.
        hang_forever.store(true, Ordering::Release);
        *svc.rpc_timeout.lock().unwrap_or_else(|e| e.into_inner()) = Duration::from_millis(120);
        let slow_resp = rpc_once(&sock, &RexSockRequest::Rename { host: "slow-box".into(), name: "qa".into(), title: Some("Timeout".into()) }).await;
        assert!(matches!(slow_resp, RexSockResponse::Error { ref message } if message.contains("didn’t answer")), "got: {slow_resp:?}");
        svc.shutdown();
    }

    // -----------------------------------------------------------------------
    // Findings 26, 28, 29, 30, 31, 32, 34: Reload cache, desktop ownership,
    // symlink/FIFO refusal, watch filter, remote owner spoofing, demo paths
    // -----------------------------------------------------------------------

    #[test]
    fn round3_f26_f28_f29_f30_f31_f32_f34_ownership_reload_watch_and_demo_isolation() {
        // Finding 28: desktop_screen_dirs points at olo/screens (never olo/olo/screens)!
        let (_rt_def, st_def) = desktop_screen_dirs();
        assert_eq!(st_def, olo_xdg::state_dir().join("screens"));
        assert!(!st_def.to_string_lossy().contains("/olo/olo/screens"));

        // Finding 34: demo_screens_paths requires non-empty OLO_REX_SCREENS_DIR and keeps both paths inside it!
        assert_eq!(demo_screens_paths_from(None, Some("/var/tmp/x.sock")), None);
        assert_eq!(demo_screens_paths_from(Some(""), Some("/var/tmp/x.sock")), None);
        assert_eq!(demo_screens_paths_from(Some("/var/tmp/demo"), Some("/var/tmp/elsewhere/x.sock")), None);
        let (ds, dst) = demo_screens_paths_from(Some("/var/tmp/demo"), None).unwrap();
        assert_eq!(ds.parent(), Some(Path::new("/var/tmp/demo")));
        assert_eq!(dst.parent(), Some(Path::new("/var/tmp/demo")));

        // Finding 31 & 37: watch filter ignores `.lock` / `-settings.json` / `logs` when children are named,
        // and triggers on directory-only overflow or actual `.json`/state events (neutral fixture paths).
        let rt_w = Path::new("/var/tmp/olo-fixture-rt/olo-screens");
        let st_w = Path::new("/var/tmp/olo-fixture-state/screens");
        assert!(!is_desktop_owner_watch_relevant(rt_w, st_w, &[rt_w.to_path_buf(), rt_w.join("qa.control.lock")]));
        assert!(!is_desktop_owner_watch_relevant(rt_w, st_w, &[rt_w.to_path_buf(), rt_w.join("qa-settings.json")]));
        assert!(is_desktop_owner_watch_relevant(rt_w, st_w, &[rt_w.to_path_buf()]));
        assert!(is_desktop_owner_watch_relevant(rt_w, st_w, &[rt_w.to_path_buf(), rt_w.join("qa.owner.json")]));
        assert!(is_desktop_owner_watch_relevant(rt_w, st_w, &[st_w.to_path_buf(), st_w.join("qa")]));

        // Finding 27: SSH eligibility matches Rex's own `RexEndpoint::parse`!
        assert!(is_ssh_endpoint("ssh://alice@dev-box:2222"));
        assert!(is_ssh_endpoint("dev-box"));
        assert!(is_ssh_endpoint("user@dev-box"));
        assert!(!is_ssh_endpoint("SSH://dev-box"));
        assert!(!is_ssh_endpoint("https://dev-box.example"));
        assert!(!is_ssh_endpoint("unix:///var/run/rex.sock"));

        // Findings 29 & 30: symlinked state_root/NAME and FIFO lock files refused without hanging!
        let dir = scratch_dir("f29-30");
        let _cleanup = ScratchCleanup(dir.clone());
        let rt_dir = dir.join("rt");
        let state_root = dir.join("state");
        let escape = dir.join("escape");
        std::fs::create_dir_all(&rt_dir).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        std::fs::create_dir_all(&escape).unwrap();
        std::os::unix::fs::symlink(&escape, state_root.join("sym-qa")).unwrap();
        assert!(record_desktop_owner_in(&rt_dir, &state_root, "sym-qa", "dev", "s1", "b1", true, None).is_err());
        assert!(!escape.join("owner.json").exists());
        assert!(!escape.join(".owner.lock").exists());

        // Finding 30: FIFO planted at `state_root/fifo-qa/.owner.lock` is rejected immediately (`O_NONBLOCK` + `S_ISREG`).
        let fifo_dir = state_root.join("fifo-qa");
        std::fs::create_dir_all(&fifo_dir).unwrap();
        let fifo_path = CString::new(fifo_dir.join(".owner.lock").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
        assert!(record_desktop_owner_in(&rt_dir, &state_root, "fifo-qa", "dev", "s1", "b1", true, None).is_err());

        // Real screen generation guard (Finding 29):
        let my_pid = std::process::id() as i32;
        std::fs::write(rt_dir.join("qa.json"), format!(r#"{{"name":"qa","pid":{my_pid},"started":2000}}"#)).unwrap();
        std::fs::create_dir_all(state_root.join("qa")).unwrap();
        std::fs::write(state_root.join("qa/screen.json"), "{}").unwrap();
        // Pane B owns generation `started=2000`:
        let owned_b = record_desktop_owner_in(&rt_dir, &state_root, "qa", "host-b", "sb", "bb", true, Some(DesktopScreenGen { pid: my_pid, started: 2000, start_ticks: None })).unwrap().unwrap();
        assert_eq!(owned_b.block_id, "bb");
        // Late mutation from Pane A for older generation `started=1000` MUST NOT overwrite B's owner!
        assert!(record_desktop_owner_in(&rt_dir, &state_root, "qa", "host-a", "sa", "ba", false, Some(DesktopScreenGen { pid: my_pid, started: 1000, start_ticks: None })).unwrap().is_none());
        // Late stop from Pane A for `started=1000` MUST NOT unlink B's runtime sidecar!
        assert!(!unlink_desktop_runtime_owner_in(&rt_dir, &state_root, "qa", Some(DesktopScreenGen { pid: my_pid, started: 1000, start_ticks: None })));
        let loaded = load_desktop_owners_in(&rt_dir, &state_root);
        assert_eq!(loaded.get("qa").map(|o| o.block_id.as_str()), Some("bb"));

        // Finding 32: remote owner naming `host_key: Some("local")` cannot spoof a local tab!
        let state = Arc::new(Mutex::new(HubState::test_empty()));
        {
            let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
            let sub_local: crate::subagents::NativeSubagent = serde_json::from_value(serde_json::json!({
                "id": "sa-local", "host_key": "local", "session_id": "s-1", "window_id": "w-1", "block_id": "b-secret",
                "parent_window_id": "w-0", "parent_block_id": "b-0", "cli": "claude", "model": "opus", "effort": "max", "title": "Local Secret"
            }))
            .unwrap();
            let sub_dev: crate::subagents::NativeSubagent = serde_json::from_value(serde_json::json!({
                "id": "sa-dev", "host_key": "dev-box", "session_id": "s-1", "window_id": "w-2", "block_id": "b-dev",
                "parent_window_id": "w-0", "parent_block_id": "b-0", "cli": "claude", "model": "opus", "effort": "max", "title": "Dev Builder"
            }))
            .unwrap();
            st.subagents.entries.push(sub_local);
            st.subagents.entries.push(sub_dev);
        }
        let spoofed = OwnerPane { host_key: Some("local".into()), session_id: "s-1".into(), block_id: "b-secret".into() };
        assert_eq!(resolve_controlled_by_in_hub(&state, "dev-box", &spoofed), None);
        let legit = OwnerPane { host_key: None, session_id: "s-1".into(), block_id: "b-dev".into() };
        assert_eq!(resolve_controlled_by_in_hub(&state, "dev-box", &legit).as_deref(), Some("Dev Builder"));

        // Finding 26: reload cache restores matching `(host_key, endpoint_raw)` and ignores mismatched endpoint!
        let svc = ScreensService::new(dir.join("r.sock"), dir.join("r.json"), 1, DoubleOpener::new(), Arc::new(|_, _| None), None, None);
        let id_keep = next_host_instance_id();
        let id_changed = next_host_instance_id();
        svc.set_initial_hosts(vec![("keep".into(), id_keep, "Keep".into(), RexHostConn::Disconnected), ("changed".into(), id_changed, "Changed".into(), RexHostConn::Disconnected)]);
        let fake_host = |key: &str| RexHost {
            key: key.into(),
            label: key.into(),
            state: RexHostConn::Disconnected,
            error: None,
            disconnected_at: Some(777_000),
            installed: Some("2026.11.27".into()),
            not_set_up_reason: None,
            committed: None,
            default_on_host: true,
            can_control: None,
            control_disabled_reason: None,
            capabilities: vec![],
            screens: vec![RexScreen::from(sample_host_screen("saved-qa", HostScreenState::Stopped, None, Some(500)))],
        };
        let endpoints = HashMap::from([("keep".into(), "ssh://a@1".into()), ("changed".into(), "ssh://b@2".into())]);
        let seeded = svc.restore_cached_hosts(
            vec![
                SavedHostScreens { host_key: "keep".into(), endpoint_raw: "ssh://a@1".into(), host: fake_host("keep") },
                SavedHostScreens { host_key: "changed".into(), endpoint_raw: "ssh://OLD@9".into(), host: fake_host("changed") },
            ],
            &endpoints,
        );
        // Finding 26: restored entries are returned for immediate HubState::host_screens seeding even for let-go hosts!
        assert_eq!(seeded.len(), 1);
        assert_eq!(seeded[0].0, "keep");
        assert_eq!(seeded[0].1.host_instance_id, id_keep);
        assert_eq!(seeded[0].1.screens.len(), 1);
        assert_eq!(seeded[0].1.status.as_ref().and_then(|s| s.installed.as_deref()), Some("2026.11.27"));
        let snap = svc.snapshot();
        assert_eq!(snap.hosts[0].screens.len(), 1);
        assert_eq!(snap.hosts[0].screens[0].name, "saved-qa");
        assert!(snap.hosts[1].screens.is_empty(), "host whose endpoint changed must NOT inherit old machine's cached screens");
    }

    // -----------------------------------------------------------------------
    // Finding 10: Checked-in wire contract parity + peer UID + oversized/bad JSON
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn round3_f10_wire_contract_shapes_peer_uid_and_malformed_requests() {
        // Peer UID check (fail closed).
        assert!(rex_sock::verify_peer_uid_match(1000, 1000).is_ok());
        assert_eq!(rex_sock::verify_peer_uid_match(1001, 1000).unwrap_err().kind(), io::ErrorKind::PermissionDenied);

        let dir = scratch_dir("wire-shapes");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let opener = DoubleOpener::new();
        let svc = ScreensService::new(sock.clone(), state_path.clone(), std::process::id(), opener.clone(), Arc::new(|_, _| None), None, None);
        let id_dev = next_host_instance_id();
        svc.set_initial_hosts(vec![("dev-box".into(), id_dev, "dev-box".into(), RexHostConn::Disconnected)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();
        let (_cd, _dh) =
            connect_and_seed(&svc, &opener, &sock, "dev-box", id_dev, sample_status(Some("2026.11.27"), true, None), vec![sample_host_screen("qa", HostScreenState::Running, Some(1000), Some(5000))])
                .await;

        async fn ask_raw(sock: &Path, raw: &[u8]) -> serde_json::Value {
            let mut s = UnixStream::connect(sock).await.unwrap();
            s.write_all(raw).await.unwrap();
            s.flush().await.unwrap();
            let (r, _w) = s.into_split();
            let mut br = BufReader::new(r);
            let line = tokio::time::timeout(Duration::from_secs(5), read_bounded_line_async(&mut br, MAX_RESPONSE_LINE_BYTES)).await.unwrap().unwrap().unwrap();
            serde_json::from_slice(&line).unwrap()
        }

        // Exact checked-in JSON wire shapes (never silently skipped on a clean checkout!).
        let list_v = ask_raw(&sock, b"{\"op\":\"list\"}\n").await;
        assert_eq!(list_v["kind"], "snapshot");
        assert_eq!(list_v["state"]["version"], STATE_VERSION);
        let follow_v = ask_raw(&sock, b"{\"op\":\"follow\"}\n").await;
        assert_eq!(follow_v["event"], "snapshot");
        let proto_v = ask_raw(&sock, b"{\"op\":\"protocol\",\"host\":\"dev-box\"}\n").await;
        assert_eq!(proto_v["kind"], "protocol");
        assert_eq!(proto_v["proto"], 1);
        let unk_v = ask_raw(&sock, b"{\"op\":\"protocol\",\"host\":\"unknown\"}\n").await;
        assert_eq!(unk_v["kind"], "error");

        // Finding 37: malformed & bad-name requests against CONNECTED host trigger ZERO dispatches!
        let cmds_before = opener.recorded_commands().len();
        let streams_before = opener.recorded_streams().len();
        let bad_name_v = ask_raw(&sock, b"{\"op\":\"stop\",\"host\":\"dev-box\",\"name\":\"../escape\"}\n").await;
        assert_eq!(bad_name_v["kind"], "error");
        let bad_json_v = ask_raw(&sock, b"{not json\n").await;
        assert_eq!(bad_json_v["kind"], "error");
        let mut huge = vec![b'a'; MAX_REQUEST_LINE_BYTES + 16];
        huge.push(b'\n');
        let huge_v = ask_raw(&sock, &huge).await;
        assert_eq!(huge_v["kind"], "error");
        assert_eq!(opener.recorded_commands().len(), cmds_before);
        assert_eq!(opener.recorded_streams().len(), streams_before);

        // Transport's shared `screen_command` probes `--protocol` and exits 69 if absent (Finding 18).
        let sh_cmd = screen_command(Some("abc123"), &["--protocol".into()]);
        assert!(sh_cmd.contains("--protocol"));
        assert!(sh_cmd.contains("exit 69"));
        assert!(sh_cmd.contains("olo-rex-link-abc123"));

        svc.shutdown();
    }

    // -----------------------------------------------------------------------
    // Real `olo_screen_pane::RexSockClient` + `RexSockOpener` end-to-end test
    // -----------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pane_rex_sock_client_and_opener_work_end_to_end() {
        use olo_screen_pane::{RexSockClient, RexSockOpener, StreamOpener as _};
        use std::io::{Read as _, Write as _};

        let dir = scratch_dir("pane-client");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex-pane.sock");
        let state_path = dir.join("state-pane.json");
        let opener = DoubleOpener::new();
        let svc = ScreensService::new(sock.clone(), state_path.clone(), std::process::id(), opener.clone(), Arc::new(|_, _| None), None, None);
        let id_dev = next_host_instance_id();
        svc.set_initial_hosts(vec![("dev-box".into(), id_dev, "dev-box".into(), RexHostConn::Disconnected)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();
        let (_cd, _dh) = connect_and_seed(
            &svc,
            &opener,
            &sock,
            "dev-box",
            id_dev,
            sample_status(Some("2026.11.27"), true, None),
            vec![sample_host_screen("review", HostScreenState::Running, Some(1000), Some(5000))],
        )
        .await;
        let _ = rpc_once(&sock, &RexSockRequest::Protocol { host: "dev-box".into() }).await;

        // Stream open failure returns clean error to `RexSockOpener::open_live`.
        opener.push_stream_err("dev-box", "stream", "remote helper refused stream");
        let sock_err = sock.clone();
        let err = tokio::task::spawn_blocking(move || {
            let client = RexSockClient::with_socket(sock_err);
            let pane_opener = RexSockOpener::new(client, "dev-box");
            pane_opener.open_live("review", Arc::new(AtomicBool::new(false))).err()
        })
        .await
        .unwrap()
        .expect("expected error");
        assert!(err.contains("remote helper refused stream"), "got: {err}");

        // Live stream relay through `RexSockOpener::open_live` + cancel tears down remote end!
        let (srv_stream, mut remote_end) = tokio::io::duplex(16 * 1024);
        opener.push_stream("dev-box", "stream", Box::pin(srv_stream));
        let remote_task = tokio::spawn(async move {
            remote_end.write_all(b"HELLO-BIN").await.unwrap();
            let mut ping = [0u8; 4];
            remote_end.read_exact(&mut ping).await.unwrap();
            assert_eq!(&ping, b"PING");
            remote_end.write_all(b"PONG").await.unwrap();
            let mut eof = [0u8; 8];
            let n = tokio::time::timeout(Duration::from_secs(2), remote_end.read(&mut eof)).await.expect("remote did not see EOF on RexSockOpener cancel").unwrap();
            assert_eq!(n, 0);
        });

        let sock_ok = sock.clone();
        tokio::task::spawn_blocking(move || {
            let client = RexSockClient::with_socket(&sock_ok);
            let snap = client.list().unwrap();
            assert_eq!(snap.hosts[0].screens[0].name, "review");
            let (proto, cmds, caps) = client.protocol("dev-box").unwrap();
            assert_eq!(proto, 1);
            assert!(cmds.iter().any(|c| c == "history-stream"));
            assert!(caps.iter().any(|c| c == "control-lock"));
            client.stop_screen("dev-box", "review", Some(1000), Some(5000)).unwrap();
            client.clear_history("dev-box", "review", Some(0), Some(5000)).unwrap();
            client.delete("dev-box", "review", Some(0), Some(5000)).unwrap();

            let pane_opener = RexSockOpener::new(client, "dev-box");
            let mut live = pane_opener.open_live("review", Arc::new(AtomicBool::new(false))).unwrap();
            let mut hello = [0u8; 9];
            live.reader.read_exact(&mut hello).unwrap();
            assert_eq!(&hello, b"HELLO-BIN");
            live.writer.write_all(b"PING").unwrap();
            let mut pong = [0u8; 4];
            live.reader.read_exact(&mut pong).unwrap();
            assert_eq!(&pong, b"PONG");
            if let Some(cancel) = live.cancel.take() {
                cancel();
            }
        })
        .await
        .unwrap();

        remote_task.await.unwrap();
        svc.shutdown();
    }

    // -----------------------------------------------------------------------
    // Finding 32: `refresh_owner_labels` publishes newly known subagents
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn round3_f32_refresh_owner_labels_republishes_when_subagent_arrives() {
        let dir = scratch_dir("f32-refresh");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let opener = DoubleOpener::new();
        let hub_state = Arc::new(Mutex::new(HubState::test_empty()));
        let hs = hub_state.clone();
        let svc = ScreensService::new(sock.clone(), state_path.clone(), std::process::id(), opener.clone(), Arc::new(move |hk, pane| resolve_controlled_by_in_hub(&hs, hk, pane)), None, None);
        let id_dev = next_host_instance_id();
        svc.set_initial_hosts(vec![("dev-box".into(), id_dev, "Dev".into(), RexHostConn::Disconnected)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();

        let mut sc = sample_host_screen("qa", HostScreenState::Running, Some(1000), Some(2000));
        sc.owner = Some(OwnerPane { host_key: None, session_id: "s-1".into(), block_id: "b-sub".into() });
        let (_cd, _dh) = connect_and_seed(&svc, &opener, &sock, "dev-box", id_dev, sample_status(Some("2026.11.27"), true, None), vec![sc]).await;
        assert_eq!(svc.snapshot().hosts[0].screens[0].controlled_by, None);

        let (mut fr, _fw, _) = open_follow_client(&sock).await;
        // Now subagent arrives in HubState -> refresh_owner_labels pushes updated label to follower!
        {
            let mut st = hub_state.lock().unwrap_or_else(|e| e.into_inner());
            let sub: crate::subagents::NativeSubagent = serde_json::from_value(serde_json::json!({
                "id": "sa-1", "host_key": "dev-box", "session_id": "s-1", "window_id": "w-1", "block_id": "b-sub",
                "parent_window_id": "w-0", "parent_block_id": "b-0", "cli": "claude", "model": "opus", "effort": "max", "title": "QA Builder"
            }))
            .unwrap();
            st.subagents.entries.push(sub);
        }
        svc.refresh_owner_labels(Some("dev-box"));
        let label = wait_follow_until(&mut fr, |line| match line {
            RexFollowLine::HostUpdated { host, .. } if host.key == "dev-box" => host.screens.first().and_then(|s| s.controlled_by.clone()),
            _ => None,
        })
        .await;
        assert_eq!(label, "QA Builder");
        svc.shutdown();
    }

    // -----------------------------------------------------------------------
    // Finding 10: Opt-in external stand-in subprocess parity (bounded + reaped)
    // -----------------------------------------------------------------------

    /// Read one bounded line from an async subprocess stdout pipe within
    /// `timeout` (Finding 10): dropping the async `ChildStdout` on timeout
    /// immediately closes the pipe with ZERO leaked blocking worker thread even
    /// if a descendant process kept stdout open.
    async fn read_bounded_process_line(stdout: tokio::process::ChildStdout, max_bytes: usize, timeout: Duration) -> Result<String, String> {
        let mut reader = BufReader::new(stdout);
        let raw = tokio::time::timeout(timeout, read_bounded_line_async(&mut reader, max_bytes))
            .await
            .map_err(|_| "timed out waiting for subprocess readiness line".to_owned())?
            .map_err(|e| format!("read readiness line: {e}"))?
            .ok_or_else(|| "subprocess exited before printing readiness line".to_owned())?;
        Ok(String::from_utf8_lossy(&raw).into_owned())
    }

    #[tokio::test]
    #[ignore = "opt-in external stand-in check; set OLO_REX_STANDIN_PATH and run with --ignored"]
    async fn standin_subprocess_parity_when_configured() {
        let Some(standin_script) = std::env::var_os("OLO_REX_STANDIN_PATH").filter(|v| !v.is_empty()).map(PathBuf::from) else {
            panic!("set OLO_REX_STANDIN_PATH to run stand-in parity test");
        };
        let dir = scratch_dir("standin");
        let _cleanup = ScratchCleanup(dir.clone());
        let standin_sock = dir.join("standin.sock");
        let standin_state = dir.join("standin-state.json");
        let our_sock = dir.join("our.sock");
        let our_state = dir.join("our-state.json");

        let standin_cfg = dir.join("standin-config.json");
        std::fs::write(
            &standin_cfg,
            serde_json::to_vec(&serde_json::json!({
                "hosts": [{
                    "key": "dev-box",
                    "label": "dev-box",
                    "state": "connected",
                    "installed": "2026.11.27",
                    "default_on_host": true,
                    "can_control": true,
                    "capabilities": ["control-ack", "control-lock", "stop-expected", "stop-expected-started", "clear-expected", "delete-expected"],
                    "screens": [{
                        "name": "qa",
                        "state": "running",
                        "width": 1280,
                        "height": 800,
                        "scale": 1.0,
                        "sound": false,
                        "pid": 1234,
                        "started": 1000,
                        "last_used": 5000
                    }]
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        struct AsyncChildGuard(tokio::process::Child);
        impl Drop for AsyncChildGuard {
            fn drop(&mut self) {
                let _ = self.0.start_kill();
                let _ = self.0.try_wait();
            }
        }
        let child = tokio::process::Command::new("/usr/bin/python3")
            .arg(&standin_script)
            .arg("--config")
            .arg(&standin_cfg)
            .arg("--socket")
            .arg(&standin_sock)
            .arg("--state")
            .arg(&standin_state)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn rex_standin.py");
        let mut guard = AsyncChildGuard(child);
        let stdout = guard.0.stdout.take().unwrap();
        let ready = read_bounded_process_line(stdout, 4096, Duration::from_secs(5)).await.expect("stand-in readiness");
        assert!(!ready.trim().is_empty(), "stand-in printed no readiness line");

        let opener = DoubleOpener::new();
        let svc = ScreensService::new(our_sock.clone(), our_state.clone(), std::process::id(), opener.clone(), Arc::new(|_, _| None), None, None);
        let id_dev = next_host_instance_id();
        svc.set_initial_hosts(vec![("dev-box".into(), id_dev, "dev-box".into(), RexHostConn::Disconnected)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();
        let (_cd, _dh) = connect_and_seed(
            &svc,
            &opener,
            &our_sock,
            "dev-box",
            id_dev,
            sample_status(Some("2026.11.27"), true, None),
            vec![sample_host_screen("qa", HostScreenState::Running, Some(1000), Some(5000))],
        )
        .await;

        async fn ask_raw_line(sock_path: &Path, raw_req: &str) -> serde_json::Value {
            let mut s = UnixStream::connect(sock_path).await.unwrap();
            s.write_all(raw_req.as_bytes()).await.unwrap();
            s.write_all(b"\n").await.unwrap();
            s.flush().await.unwrap();
            let (r, _w) = s.into_split();
            let mut reader = BufReader::new(r);
            let raw = tokio::time::timeout(Duration::from_secs(5), read_bounded_line_async(&mut reader, MAX_RESPONSE_LINE_BYTES)).await.unwrap().unwrap().unwrap();
            serde_json::from_slice(&raw).unwrap()
        }

        let s_list = ask_raw_line(&standin_sock, r#"{"op":"list"}"#).await;
        let o_list = ask_raw_line(&our_sock, r#"{"op":"list"}"#).await;
        assert_eq!(s_list["kind"], o_list["kind"]);
        let s_follow = ask_raw_line(&standin_sock, r#"{"op":"follow"}"#).await;
        let o_follow = ask_raw_line(&our_sock, r#"{"op":"follow"}"#).await;
        assert_eq!(s_follow["event"], o_follow["event"]);
        let s_unk = ask_raw_line(&standin_sock, r#"{"op":"protocol","host":"unknown-host"}"#).await;
        let o_unk = ask_raw_line(&our_sock, r#"{"op":"protocol","host":"unknown-host"}"#).await;
        assert_eq!(s_unk["kind"], "error");
        assert_eq!(o_unk["kind"], "error");

        svc.shutdown();
    }

    // -----------------------------------------------------------------------
    // Final pass tests (each verified to fail before its fix!)
    // -----------------------------------------------------------------------

    #[test]
    fn final_pass_f27_ssh_endpoint_matches_rex_parser() {
        assert!(is_ssh_endpoint("dev-box"), "bare hostname must be SSH");
        assert!(is_ssh_endpoint("user@dev-box"), "user@host must be SSH");
        assert!(is_ssh_endpoint("user@dev-box.tail.ts.net"), "user@fqdn must be SSH");
        assert!(is_ssh_endpoint("ssh://alice@dev-box:2222"), "ssh:// must be SSH");
        assert!(!is_ssh_endpoint("SSH://dev-box"), "uppercase SSH:// is rejected by RexEndpoint::parse");
        assert!(!is_ssh_endpoint("https://dev-box.example"));
        assert!(!is_ssh_endpoint("dev-box.tail.ts.net"));
        assert!(!is_ssh_endpoint("unix:///var/run/rex.sock"));
        assert!(!is_ssh_endpoint("local"));
    }

    #[tokio::test]
    async fn final_pass_f1_f2_refuse_missing_expected_and_old_stop_cap() {
        let dir = scratch_dir("fp-f1-f2");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let opener = DoubleOpener::new();
        // Old helper advertises `stop-expected-started` (released non-atomic cap)
        // WITHOUT `stop-expected`, while advertising `clear-expected` & `delete-expected`.
        opener.set_command_handler(|_target, args, _timeout| {
            let args = args.to_vec();
            Box::pin(async move {
                if args.first().map(String::as_str) == Some("--protocol") {
                    return Ok(CommandExecOutput {
                        status: Some(0),
                        stdout: r#"{"proto":1,"commands":["follow","stream"],"capabilities":["control-ack","stop-expected-started","clear-expected","delete-expected"]}"#.into(),
                        stderr: String::new(),
                        truncated: false,
                    });
                }
                Ok(CommandExecOutput { status: Some(0), stdout: String::new(), stderr: String::new(), truncated: false })
            })
        });
        let svc = ScreensService::new(sock.clone(), state_path.clone(), std::process::id(), opener.clone(), Arc::new(|_, _| None), None, None);
        let id_old = next_host_instance_id();
        svc.set_initial_hosts(vec![("old-box".into(), id_old, "Old Box".into(), RexHostConn::Disconnected)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();
        let (_c, _h) =
            connect_and_seed(&svc, &opener, &sock, "old-box", id_old, sample_status(Some("2026.11.27"), true, None), vec![sample_host_screen("qa", HostScreenState::Running, Some(1000), Some(5000))])
                .await;
        let _ = opener.take_commands();

        // 1. Missing expected values on Stop, ClearHistory, Delete -> refused ("Update Olo on this computer...") with ZERO commands!
        let r_stop_none = rpc_once(&sock, &RexSockRequest::Stop { host: "old-box".into(), name: "qa".into(), expected_started: None, expected_last_used: None }).await;
        assert!(matches!(r_stop_none, RexSockResponse::Error { ref message } if message.contains("this computer")), "got: {r_stop_none:?}");
        let r_clear_none = rpc_once(&sock, &RexSockRequest::ClearHistory { host: "old-box".into(), name: "qa".into(), expected_started: None, expected_last_used: None }).await;
        assert!(matches!(r_clear_none, RexSockResponse::Error { ref message } if message.contains("this computer")), "got: {r_clear_none:?}");
        let r_del_none = rpc_once(&sock, &RexSockRequest::Delete { host: "old-box".into(), name: "qa".into(), expected_started: None, expected_last_used: None }).await;
        assert!(matches!(r_del_none, RexSockResponse::Error { ref message } if message.contains("this computer")), "got: {r_del_none:?}");

        // 2. Old helper advertising ONLY `stop-expected-started` (not `stop-expected`) MUST refuse Stop!
        let r_stop_old = rpc_once(&sock, &RexSockRequest::Stop { host: "old-box".into(), name: "qa".into(), expected_started: Some(1000), expected_last_used: Some(5000) }).await;
        assert!(matches!(r_stop_old, RexSockResponse::Error { ref message } if message.contains("Update Olo on “old-box” to stop a screen from here.")), "got: {r_stop_old:?}");

        let cmds = opener.take_commands();
        assert!(!cmds.iter().any(|(_, a)| matches!(a.first().map(String::as_str), Some("stop" | "history" | "delete"))), "destructive command dispatched despite refusal: {cmds:?}");
        svc.shutdown();
    }

    #[tokio::test]
    async fn final_pass_f9_f33_delayed_start_never_resurrects_gone_screen() {
        let dir = scratch_dir("fp-f9-33");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let opener = DoubleOpener::new();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let e_cb = entered.clone();
        let r_cb = release.clone();
        opener.set_command_handler(move |_target, args, _timeout| {
            let args = args.to_vec();
            let e_cb = e_cb.clone();
            let r_cb = r_cb.clone();
            Box::pin(async move {
                if args.first().map(String::as_str) == Some("start") {
                    e_cb.notify_one();
                    r_cb.notified().await;
                    return Ok(CommandExecOutput {
                        status: Some(0),
                        stdout: r#"{"name":"ephemeral","width":1280,"height":720,"scale":1.0,"sound":false,"pid":999,"started":3000}"#.into(),
                        stderr: String::new(),
                        truncated: false,
                    });
                }
                Ok(CommandExecOutput { status: Some(0), stdout: r#"{"proto":1,"commands":["follow"],"capabilities":["stop-expected"]}"#.into(), stderr: String::new(), truncated: false })
            })
        });
        let svc = ScreensService::new(sock.clone(), state_path.clone(), std::process::id(), opener.clone(), Arc::new(|_, _| None), None, None);
        let id = next_host_instance_id();
        svc.set_initial_hosts(vec![("dev".into(), id, "Dev".into(), RexHostConn::Disconnected)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();
        let (_c, mut h) = connect_and_seed(&svc, &opener, &sock, "dev", id, sample_status(Some("2026.11.27"), true, None), vec![]).await;

        let sock_c = sock.clone();
        let start_task =
            tokio::spawn(async move { rpc_once(&sock_c, &RexSockRequest::Start { host: "dev".into(), name: Some("ephemeral".into()), keep: false, size: None, scale: None, sound: false }).await });
        entered.notified().await;
        // Authoritative replacement snapshot arrives with NO `ephemeral` (screen was already deleted).
        send_follow_event(&mut h, &FollowEvent::Snapshot { proto: 1, status: sample_status(Some("2026.11.27"), true, None), screens: vec![] }).await;
        let (fr, _fw, _) = open_follow_client(&sock).await;
        // Release delayed start reply.
        release.notify_one();
        let resp = start_task.await.unwrap();
        assert!(matches!(resp, RexSockResponse::Started { .. }), "got: {resp:?}");
        // Snapshot must NOT resurrect deleted `ephemeral`!
        let snap = svc.snapshot();
        assert!(snap.hosts[0].screens.is_empty(), "delayed Start reply resurrected deleted screen: {:?}", snap.hosts[0].screens);
        drop(fr);
        svc.shutdown();
    }

    #[tokio::test]
    async fn final_pass_f12_f26_dead_follower_invalidates_freshness() {
        let dir = scratch_dir("fp-f12-26");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let opener = DoubleOpener::new();
        let updates = Arc::new(Mutex::new(Vec::<HostScreensUpdate>::new()));
        let u_cb = updates.clone();
        let svc = ScreensService::new(
            sock.clone(),
            state_path.clone(),
            std::process::id(),
            opener.clone(),
            Arc::new(|_, _| None),
            Some(Arc::new(move |u| u_cb.lock().unwrap_or_else(|e| e.into_inner()).push(u))),
            None,
        );
        let id = next_host_instance_id();
        svc.set_initial_hosts(vec![("dev".into(), id, "Dev".into(), RexHostConn::Disconnected)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();
        let (_c, h) =
            connect_and_seed(&svc, &opener, &sock, "dev", id, sample_status(Some("2026.11.27"), true, None), vec![sample_host_screen("qa", HostScreenState::Running, Some(1000), Some(2000))]).await;
        let (mut fr, _fw, _) = open_follow_client(&sock).await;
        // Kill the follow stream while SSH stays connected!
        opener.push_stream_err("dev", "follow", "temporary channel drop");
        drop(h);
        // Must immediately lose freshness (`Connecting` + `has_follow_snapshot == false`) so Check Again wins!
        wait_follow_until(&mut fr, |line| matches!(line, RexFollowLine::HostUpdated { host, .. } if host.key == "dev" && host.state == RexHostConn::Connecting).then_some(())).await;
        let last_upd = updates.lock().unwrap_or_else(|e| e.into_inner()).last().cloned().unwrap();
        assert!(!last_upd.has_follow_snapshot, "dead follower must set has_follow_snapshot = false");
        svc.shutdown();
    }

    #[tokio::test]
    async fn final_pass_f14_early_disconnect_prevents_delayed_connect() {
        let dir = scratch_dir("fp-f14");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let opener = DoubleOpener::new();
        let svc = ScreensService::new(sock.clone(), state_path.clone(), std::process::id(), opener.clone(), Arc::new(|_, _| None), None, None);
        let id = next_host_instance_id();
        svc.set_initial_hosts(vec![("dev".into(), id, "Dev".into(), RexHostConn::Connecting)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();

        let cid = next_conn_id();
        // Disconnect arrives BEFORE host_connected(cid)!
        svc.host_state_changed("dev", id, Some(cid), 0, RexHostConn::Disconnected, None, None);
        // Delayed host_connected(cid) arrives after user let go!
        svc.host_connected(&rt, "dev", id, cid, 0, "Dev", None);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let snap = svc.snapshot();
        assert_eq!(snap.hosts[0].state, RexHostConn::Disconnected, "delayed connect resurrected let-go host as {:?}", snap.hosts[0].state);
        assert!(opener.recorded_streams().is_empty(), "follower must NOT start for disconnected conn_id: {:?}", opener.recorded_streams());
        svc.shutdown();
    }

    #[tokio::test]
    async fn final_pass_f17_f18_f20_check_again_wakes_parked_and_committed_reprobes() {
        let dir = scratch_dir("fp-f17-18-20");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let opener = DoubleOpener::new();
        let proto_calls = Arc::new(AtomicUsize::new(0));
        let pc = proto_calls.clone();
        opener.set_command_handler(move |_target, args, _timeout| {
            let args = args.to_vec();
            let pc = pc.clone();
            Box::pin(async move {
                if args.first().map(String::as_str) == Some("--protocol") {
                    let n = pc.fetch_add(1, Ordering::SeqCst);
                    let caps = if n == 0 { r#"["control-ack"]"# } else { r#"["control-ack","control-lock","stop-expected"]"# };
                    return Ok(CommandExecOutput {
                        status: Some(0),
                        stdout: format!(r#"{{"proto":1,"commands":["follow","stream"],"capabilities":{caps}}}"#),
                        stderr: String::new(),
                        truncated: false,
                    });
                }
                Ok(CommandExecOutput { status: Some(0), stdout: String::new(), stderr: String::new(), truncated: false })
            })
        });
        let svc = ScreensService::new(sock.clone(), state_path.clone(), std::process::id(), opener.clone(), Arc::new(|_, _| None), None, None);
        let id = next_host_instance_id();
        svc.set_initial_hosts(vec![("dev".into(), id, "Dev".into(), RexHostConn::Disconnected)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();
        let (mut fr, _fw, _) = open_follow_client(&sock).await;

        // 1. Follower parks incompatible.
        opener.push_stream_err("dev", "follow", "the host has no screens-capable Olo helper");
        let cid = next_conn_id();
        svc.host_connected(&rt, "dev", id, cid, 0, "Dev", None);
        wait_follow_until(&mut fr, |line| matches!(line, RexFollowLine::HostUpdated { host, .. } if host.not_set_up_reason.as_deref() == Some(OLD_HELPER_REASON)).then_some(())).await;

        // 2. External helper installed, Check Again reports `helper_has_screens = Some(true)` -> MUST wake parked follower!
        let (fc, mut fh) = tokio::io::duplex(16 * 1024);
        opener.push_stream("dev", "follow", Box::pin(fc));
        svc.set_host_lacking("dev", id, cid, None, Some(true));
        let mut st1 = sample_status(Some("2026.11.27"), true, None);
        st1.committed = Some(100);
        send_follow_event(&mut fh, &FollowEvent::Snapshot { proto: 1, status: st1, screens: vec![] }).await;
        wait_follow_until(&mut fr, |line| matches!(line, RexFollowLine::HostUpdated { host, .. } if host.installed.as_deref() == Some("2026.11.27")).then_some(())).await;
        // Wait for the single initial protocol probe triggered by Snapshot, and verify NO duplicate probe ran!
        let dl = Instant::now() + Duration::from_secs(3);
        while proto_calls.load(Ordering::SeqCst) == 0 && Instant::now() < dl {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(proto_calls.load(Ordering::SeqCst), 1, "expected exactly 1 protocol probe after Snapshot");

        // 3. Same version string `"2026.11.27"`, DIFFERENT `committed` (`100 -> 200`) -> MUST re-probe protocol ONCE and gain `control-lock`!
        let mut st2 = sample_status(Some("2026.11.27"), true, None);
        st2.committed = Some(200);
        send_follow_event(&mut fh, &FollowEvent::Status { status: st2 }).await;
        wait_follow_until(&mut fr, |line| matches!(line, RexFollowLine::HostUpdated { host, .. } if host.can_control == Some(true)).then_some(())).await;
        assert_eq!(proto_calls.load(Ordering::SeqCst), 2, "expected exactly 1 re-probe after committed changed");
        svc.shutdown();
        drop(fh);
    }

    #[tokio::test]
    async fn final_pass_host_window_stop_guarded_and_host_pinned() {
        let dir = scratch_dir("fp-host-stop");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let opener = DoubleOpener::new();
        opener.set_command_handler(|target, args, _timeout| {
            let host = target.host_key.clone();
            let args = args.to_vec();
            Box::pin(async move {
                if args.first().map(String::as_str) == Some("--protocol") {
                    let caps = if host == "old-host" { r#"["control-ack","stop-expected-started"]"# } else { r#"["control-ack","stop-expected","stop-expected-started"]"# };
                    return Ok(CommandExecOutput { status: Some(0), stdout: format!(r#"{{"proto":1,"commands":["follow"],"capabilities":{caps}}}"#), stderr: String::new(), truncated: false });
                }
                Ok(CommandExecOutput { status: Some(0), stdout: String::new(), stderr: String::new(), truncated: false })
            })
        });
        let svc = ScreensService::new(sock.clone(), state_path.clone(), std::process::id(), opener.clone(), Arc::new(|_, _| None), None, None);
        let id_old = next_host_instance_id();
        let id_new = next_host_instance_id();
        svc.set_initial_hosts(vec![("old-host".into(), id_old, "Old Host".into(), RexHostConn::Disconnected), ("new-host".into(), id_new, "New Host".into(), RexHostConn::Disconnected)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();
        let (_co, _oh) =
            connect_and_seed(&svc, &opener, &sock, "old-host", id_old, sample_status(Some("2026.11.27"), true, None), vec![sample_host_screen("qa", HostScreenState::Running, Some(777), None)]).await;
        let (_cn, _nh) =
            connect_and_seed(&svc, &opener, &sock, "new-host", id_new, sample_status(Some("2026.11.27"), true, None), vec![sample_host_screen("qa", HostScreenState::Running, Some(888), None)]).await;
        let _ = opener.take_commands();

        // 1. Stale host_instance_id (e.g. replaced host under same key) is rejected with zero dispatch!
        let err_inst = svc.stop_screen_for_host_instance("new-host", id_old, "qa", Some(888)).await.unwrap_err();
        assert!(err_inst.contains("changed"), "got: {err_inst}");

        // 2. Missing expected_started on Host window Stop (legacy remote screen
        //    descriptor lacking `started`) blames the remote host, NOT "this computer" (Agy Finding 9)!
        let err_none = svc.stop_screen_for_host_instance("new-host", id_new, "qa", None).await.unwrap_err();
        assert!(err_none.contains("Update Olo on “new-host” to stop a screen from here."), "got: {err_none}");
        assert!(!err_none.contains("this computer"), "must not blame this computer: {err_none}");

        // 3. Old host advertising only `stop-expected-started` (not `stop-expected`) is refused!
        let err_old = svc.stop_screen_for_host_instance("old-host", id_old, "qa", Some(777)).await.unwrap_err();
        assert!(err_old.contains("Update Olo on “old-host” to stop a screen from here."), "got: {err_old}");

        let cmds_before = opener.take_commands();
        assert!(!cmds_before.iter().any(|(_, a)| a.first().map(String::as_str) == Some("stop")), "unexpected stop dispatched: {cmds_before:?}");

        // 4. Capable host with matching instance & started forwards `--expected-started 888`!
        svc.stop_screen_for_host_instance("new-host", id_new, "qa", Some(888)).await.unwrap();
        let cmds_after = opener.take_commands();
        assert!(cmds_after.iter().any(|(h, a)| h == "new-host" && a == &["stop", "qa", "--expected-started", "888"]), "got: {cmds_after:?}");

        // 5. Switching Host window from host-a to host-b causes queued host-a identity check to reject!
        //    Serialized via `TEST_LIVE_LOCK` so parallel `host::tests` never race on global `LIVE` (Agy Finding 10).
        {
            let _live_guard = crate::host::TEST_LIVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            crate::host::open("old-host", "Old Host", id_old);
            assert_eq!(crate::host::current().map(|s| (s.key, s.host_instance_id)), Some(("old-host".into(), id_old)));
            crate::host::open("new-host", "New Host", id_new);
            let cur = crate::host::current().unwrap();
            assert_ne!((cur.key.as_str(), cur.host_instance_id), ("old-host", id_old));
            crate::host::close();
        }

        svc.shutdown();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn final_pass_f15_concurrent_producers_deliver_all_hosts_in_order() {
        let dir = scratch_dir("fp-f15");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let opener = DoubleOpener::new();
        let last_used_seen = Arc::new(Mutex::new(Vec::<LastUsedUpdate>::new()));
        let lu_cb = last_used_seen.clone();
        let svc = ScreensService::new(
            sock.clone(),
            state_path.clone(),
            std::process::id(),
            opener.clone(),
            Arc::new(|_, _| None),
            None,
            Some(Arc::new(move |u| lu_cb.lock().unwrap_or_else(|e| e.into_inner()).push(u))),
        );
        let id_a = next_host_instance_id();
        let id_b = next_host_instance_id();
        svc.set_initial_hosts(vec![("host-a".into(), id_a, "A0".into(), RexHostConn::Disconnected), ("host-b".into(), id_b, "B0".into(), RexHostConn::Disconnected)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();
        let (mut fr, _fw, snap0) = open_follow_client(&sock).await;
        let mut last_rev = snap0.rev;

        // Two concurrent producer tasks rename host-a and host-b rapidly in parallel!
        let svc_a = svc.clone();
        let svc_b = svc.clone();
        let ta = tokio::spawn(async move {
            for i in 1..=60 {
                svc_a.host_renamed("host-a", id_a, &format!("A-{i}"));
                tokio::task::yield_now().await;
            }
        });
        let tb = tokio::spawn(async move {
            for i in 1..=60 {
                svc_b.host_renamed("host-b", id_b, &format!("B-{i}"));
                tokio::task::yield_now().await;
            }
        });
        ta.await.unwrap();
        tb.await.unwrap();

        // Healthy Follow subscriber MUST observe strictly increasing rev AND BOTH final labels A-60 and B-60!
        let mut got_a = String::new();
        let mut got_b = String::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while got_a != "A-60" || got_b != "B-60" {
            let rem = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(!rem.is_zero(), "missed final update: got_a={got_a:?}, got_b={got_b:?}");
            let line = read_next_follow_line(&mut fr).await;
            if let RexFollowLine::HostUpdated { rev, host } = line {
                assert!(rev > last_rev, "out-of-order HostUpdated rev {rev} <= {last_rev}");
                last_rev = rev;
                if host.key == "host-a" {
                    got_a = host.label;
                } else if host.key == "host-b" {
                    got_b = host.label;
                }
            }
        }
        assert_eq!(got_a, "A-60");
        assert_eq!(got_b, "B-60");

        // Verify `LastUsedUpdate` carries authoritative `(host_instance_id, conn_id)`!
        let (ca, mut ha) =
            connect_and_seed(&svc, &opener, &sock, "host-a", id_a, sample_status(Some("2026.11.27"), true, None), vec![sample_host_screen("qa", HostScreenState::Running, Some(100), Some(200))]).await;
        send_follow_event(&mut ha, &FollowEvent::LastUsed { name: "qa".into(), last_used: 999, actions: Some(7) }).await;
        wait_follow_until(&mut fr, |l| matches!(l, RexFollowLine::ScreenLastUsed { host, name, last_used: 999, actions: Some(7) } if host == "host-a" && name == "qa").then_some(())).await;
        let seen = last_used_seen.lock().unwrap_or_else(|e| e.into_inner());
        let lu = seen.iter().find(|u| u.host_key == "host-a" && u.last_used == 999).expect("missing LastUsedUpdate");
        assert_eq!(lu.host_instance_id, id_a);
        assert_eq!(lu.conn_id, ca);
        assert_eq!(lu.actions, Some(7));

        svc.shutdown();
    }

    #[tokio::test]
    async fn final_pass_f9_f24_f35_publisher_runs_without_socket_and_coalesces_queue() {
        let dir = scratch_dir("fp-pub-bound");
        let _cleanup = ScratchCleanup(dir.clone());
        let opener = DoubleOpener::new();
        let delivered = Arc::new(Mutex::new(HashMap::<String, u64>::new()));
        let d_cb = delivered.clone();
        let svc = ScreensService::new(
            dir.join("never-bound.sock"),
            dir.join("never-written.json"),
            std::process::id(),
            opener.clone(),
            Arc::new(|_, _| None),
            Some(Arc::new(move |u| {
                d_cb.lock().unwrap_or_else(|e| e.into_inner()).insert(u.host_key, u.rev);
            })),
            None,
        );
        let id_a = next_host_instance_id();
        let id_b = next_host_instance_id();
        svc.set_initial_hosts(vec![("a".into(), id_a, "A0".into(), RexHostConn::Disconnected), ("b".into(), id_b, "B0".into(), RexHostConn::Disconnected)]);

        // Burst 100 updates BEFORE publisher starts: queue MUST coalesce to at most 2 entries (one per host)!
        for i in 1..=50 {
            svc.host_renamed("a", id_a, &format!("A{i}"));
            svc.host_renamed("b", id_b, &format!("B{i}"));
        }
        {
            let st = svc.inner.lock().unwrap_or_else(|e| e.into_inner());
            assert!(st.pending_ops.len() <= 2, "pending_ops grew unbounded without consumer: {}", st.pending_ops.len());
        }

        // Start ONLY the publisher (simulating failed socket bind): Host window callback MUST still receive updates,
        // and NO state file is created!
        let rt = tokio::runtime::Handle::current();
        svc.start_publisher(&rt);
        let dl = Instant::now() + Duration::from_secs(3);
        while delivered.lock().unwrap_or_else(|e| e.into_inner()).len() < 2 && Instant::now() < dl {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(delivered.lock().unwrap_or_else(|e| e.into_inner()).len(), 2, "publisher failed to deliver updates when socket is unbound");
        assert!(!dir.join("never-written.json").exists(), "unbound service must never write state file");
        svc.shutdown();
    }

    #[test]
    fn final_pass_f5_relay_watcher_fails_closed() {
        // Invalid fd MUST return Err (never silently degrade to `None` -> `pending()`!).
        assert!(make_relay_watchers(-1).is_err(), "invalid fd must fail closed instead of returning pending watcher");
    }

    #[tokio::test]
    async fn final_pass_f10_bounded_async_readiness_does_not_hang_on_descendant() {
        // Spawn a shell that forks a background `sleep 30` inheriting stdout and exits immediately!
        let mut child = tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("sleep 30 & exit 0")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn sh");
        let stdout = child.stdout.take().unwrap();
        let t0 = Instant::now();
        let res = read_bounded_process_line(stdout, 4096, Duration::from_millis(120)).await;
        assert!(res.is_err(), "expected readiness timeout when descendant holds stdout open, got: {res:?}");
        assert!(t0.elapsed() < Duration::from_secs(2), "bounded async readiness waited too long: {:?}", t0.elapsed());
        let _ = child.wait().await;
    }

    #[test]
    fn final_pass_f8_f18_ran_stderr_order_and_shared_screen_command() {
        // Finding 8: `CommandExecOutput` reads `Ran.stderr` directly (no line subtraction) preserving exact diagnostic order.
        let out = CommandExecOutput { status: Some(1), stdout: "X\n".into(), stderr: "X\nY\n".into(), truncated: false };
        assert_eq!(out.error_words("fallback"), "X\nY");
        // Finding 18: shared `olo_rex_client::ssh::screen_command` probes `--protocol` on the candidate helper.
        let cmd = screen_command(Some("h1"), &["stop".into(), "qa".into(), "--expected-started".into(), "100".into()]);
        assert!(cmd.contains("probe_proto"));
        assert!(cmd.contains("--expected-started"));
    }

    // -----------------------------------------------------------------------
    // Agy Neon 4 xhigh code-review regression tests (Findings 1–8)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn agy_f1_shutdown_races_start_server_and_wait_true_exits_after_shutdown() {
        let dir = scratch_dir("agy-f1");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let rt = tokio::runtime::Handle::current();

        // 1. `shutdown()` called BEFORE `start_server()` must prevent binding,
        //    state-file creation, and holding the exclusive flock!
        let svc1 = ScreensService::new(sock.clone(), state_path.clone(), 1001, DoubleOpener::new(), Arc::new(|_, _| None), None, None);
        svc1.shutdown();
        svc1.start_server(&rt).unwrap();
        assert!(!sock.exists(), "start_server after shutdown must not leave socket");
        assert!(!state_path.exists(), "start_server after shutdown must not leave state file");

        // A second Rex instance must immediately acquire the flock cleanly!
        let svc2 = ScreensService::new(sock.clone(), state_path.clone(), 1002, DoubleOpener::new(), Arc::new(|_, _| None), None, None);
        svc2.start_server(&rt).unwrap();
        assert!(sock.exists() && state_path.exists());

        // 2. `wait_true()` subscribed AFTER `send(true)` must resolve immediately,
        //    and `handle_follow` / `wait_fresh_snapshot` invoked after shutdown
        //    must exit cleanly instead of hanging forever!
        let (tx, _rx0) = watch::channel(false);
        let _ = tx.send_replace(true);
        let mut late_rx = tx.subscribe();
        tokio::time::timeout(Duration::from_millis(200), wait_true(&mut late_rx)).await.expect("wait_true hung on already-true watch");

        // `handle_follow` on a service already shut down must exit immediately!
        let sock_pair_path = dir.join("pair.sock");
        let pair_listener = UnixListener::bind(&sock_pair_path).unwrap();
        let (accepted, _client) = tokio::join!(pair_listener.accept(), UnixStream::connect(&sock_pair_path));
        let (r, w) = accepted.unwrap().0.into_split();
        svc2.shutdown();
        tokio::time::timeout(Duration::from_millis(500), svc2.handle_follow(r, w)).await.expect("handle_follow hung when started after shutdown");
    }

    #[tokio::test]
    async fn agy_f2_monotonic_state_seq_prevents_stale_state_overwrite_and_teardown() {
        let dir = scratch_dir("agy-f2");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let opener = DoubleOpener::new();
        let svc = ScreensService::new(sock.clone(), state_path.clone(), std::process::id(), opener.clone(), Arc::new(|_, _| None), None, None);
        let id = next_host_instance_id();
        svc.set_initial_hosts(vec![("dev".into(), id, "Dev".into(), RexHostConn::Connecting)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();

        // Scenario 1: User disconnects (state_seq = 2); preempted attempt failure
        // with older state_seq = 1 (`expected_conn_id: None`) MUST be dropped!
        svc.host_state_changed("dev", id, None, 2, RexHostConn::Disconnected, None, None);
        svc.host_state_changed("dev", id, None, 1, RexHostConn::Reconnecting, Some("stale fail".into()), None);
        assert_eq!(svc.snapshot().hosts[0].state, RexHostConn::Disconnected, "stale Reconnecting overwrote Disconnected");

        // Scenario 2: Replacement connection connects (conn_id, state_seq = 3);
        // delayed `detach_host` with `expected_conn_id: None` and older `state_seq = 2`
        // MUST NOT tear down the live connection or its follow stream!
        let (fc, mut fh) = tokio::io::duplex(16 * 1024);
        opener.push_stream("dev", "follow", Box::pin(fc));
        let cid = next_conn_id();
        let (mut fr, _fw, _) = open_follow_client(&sock).await;
        svc.host_connected(&rt, "dev", id, cid, 3, "Dev", None);
        send_follow_event(&mut fh, &FollowEvent::Snapshot { proto: 1, status: sample_status(Some("2026.11.27"), true, None), screens: vec![] }).await;
        wait_follow_until(&mut fr, |l| matches!(l, RexFollowLine::HostUpdated { host, .. } if host.key == "dev" && host.state == RexHostConn::Connected).then_some(())).await;

        svc.host_state_changed("dev", id, None, 2, RexHostConn::Disconnected, None, None);
        assert_eq!(svc.snapshot().hosts[0].state, RexHostConn::Connected, "stale state_seq tore down live connection");

        // Live follower MUST still be alive and deliver subsequent events!
        send_follow_event(&mut fh, &FollowEvent::Screen { screen: Box::new(sample_host_screen("qa", HostScreenState::Running, Some(100), None)) }).await;
        wait_follow_until(&mut fr, |l| matches!(l, RexFollowLine::HostUpdated { host, .. } if host.key == "dev" && host.screens.iter().any(|s| s.name == "qa")).then_some(())).await;

        svc.shutdown();
    }

    #[tokio::test]
    async fn agy_f3_real_kit_watch_sees_existing_subdir_owner_update_and_stops_on_drop() {
        let dir = scratch_dir("agy-f3");
        let _cleanup = ScratchCleanup(dir.clone());
        let rt_dir = dir.join("rt");
        let state_root = dir.join("state");
        std::fs::create_dir(&rt_dir).unwrap();
        std::fs::set_permissions(&rt_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(&state_root).unwrap();
        std::fs::set_permissions(&state_root, std::fs::Permissions::from_mode(0o700)).unwrap();

        // Pre-existing stopped screen directory `state_root/qa` WITHOUT `owner.json` yet!
        let qa_dir = state_root.join("qa");
        std::fs::create_dir(&qa_dir).unwrap();
        std::fs::write(qa_dir.join("actions.jsonl"), b"").unwrap();

        // Verify `.owner.lock` inside watched `state_root/qa` is ignored even when
        // `kit-watch` emits `[state_root/qa, state_root/qa/.owner.lock]`.
        assert!(!is_desktop_owner_watch_relevant(&rt_dir, &state_root, &[qa_dir.clone(), qa_dir.join(".owner.lock")]), "lockfile write inside watched screen subdir must not trigger rescan");
        assert!(is_desktop_owner_watch_relevant(&rt_dir, &state_root, &[qa_dir.clone(), qa_dir.join("owner.json")]), "owner.json write inside watched screen subdir must trigger rescan");

        let hub_state = Arc::new(Mutex::new(crate::hub::HubState::test_empty()));
        let notified = Arc::new(Notify::new());
        let n_cb = notified.clone();
        let watch = watch_desktop_screen_owners_in(tokio::runtime::Handle::current(), rt_dir.clone(), state_root.clone(), hub_state.clone(), Arc::new(move |_| n_cb.notify_waiters()));
        let mut init_done = watch.init_done.clone();
        let task_done = watch.task_done.clone();

        // Wait deterministically for the initial background scan & `Watch::set`
        // to complete (no wall-clock sleep, Round 2 Finding 4), then write
        // `owner.json` inside the PRE-EXISTING `state_root/qa/` directory!
        tokio::time::timeout(Duration::from_secs(3), wait_true(&mut init_done)).await.expect("initial desktop owner watch scan never completed");
        let owner = ScreenOwner {
            host_key: Some("dev".into()),
            session_id: "s1".into(),
            block_id: "b1".into(),
            started_by: None,
            pid: None,
            began: None,
            rex_socket: None,
            at: 1000,
            screen_pid: None,
            screen_started: None,
            screen_start_ticks: None,
        };
        std::fs::write(qa_dir.join("owner.json"), serde_json::to_vec(&owner).unwrap()).unwrap();

        let dl = Instant::now() + Duration::from_secs(3);
        while Instant::now() < dl {
            if hub_state.lock().unwrap_or_else(|e| e.into_inner()).desktop_screen_owners.get("qa").is_some_and(|o| o.block_id == "b1") {
                break;
            }
            let _ = tokio::time::timeout(Duration::from_millis(100), notified.notified()).await;
        }
        assert_eq!(
            hub_state.lock().unwrap_or_else(|e| e.into_inner()).desktop_screen_owners.get("qa").map(|o| o.block_id.as_str()),
            Some("b1"),
            "real olo_kit::watch missed owner.json update inside pre-existing state_root/qa subdir"
        );

        // Round 2 Finding 2: creating a BRAND-NEW `state_root/late-qa/` directory
        // after startup and writing `actions.jsonl` + `owner.json` into it MUST
        // also be observed because `Watch::set` runs BEFORE `load_desktop_owners_in`!
        let late_dir = state_root.join("late-qa");
        std::fs::create_dir(&late_dir).unwrap();
        std::fs::write(late_dir.join("actions.jsonl"), b"").unwrap();
        let mut late_owner = owner.clone();
        late_owner.block_id = "b-late".into();
        std::fs::write(late_dir.join("owner.json"), serde_json::to_vec(&late_owner).unwrap()).unwrap();
        let dl2 = Instant::now() + Duration::from_secs(3);
        while Instant::now() < dl2 {
            if hub_state.lock().unwrap_or_else(|e| e.into_inner()).desktop_screen_owners.get("late-qa").is_some_and(|o| o.block_id == "b-late") {
                break;
            }
            let _ = tokio::time::timeout(Duration::from_millis(100), notified.notified()).await;
        }
        assert_eq!(
            hub_state.lock().unwrap_or_else(|e| e.into_inner()).desktop_screen_owners.get("late-qa").map(|o| o.block_id.as_str()),
            Some("b-late"),
            "real olo_kit::watch missed owner.json written in newly created state_root/late-qa subdir"
        );

        // Dropping `DesktopOwnersWatch` MUST stop its background refresher task cleanly!
        let done_wait = task_done.notified();
        tokio::pin!(done_wait);
        done_wait.as_mut().enable();
        drop(watch);
        tokio::time::timeout(Duration::from_secs(2), done_wait).await.expect("desktop owner watch refresher task leaked after drop");
    }

    #[tokio::test]
    async fn r2_f1_set_host_lacking_false_never_wipes_live_follow_stream() {
        let dir = scratch_dir("r2-f1");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let opener = DoubleOpener::new();
        let svc = ScreensService::new(sock.clone(), state_path.clone(), std::process::id(), opener.clone(), Arc::new(|_, _| None), None, None);
        let id = next_host_instance_id();
        svc.set_initial_hosts(vec![("dev".into(), id, "Dev".into(), RexHostConn::Disconnected)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();

        let (cid, mut fh) =
            connect_and_seed(&svc, &opener, &sock, "dev", id, sample_status(Some("2026.11.27"), true, None), vec![sample_host_screen("qa", HostScreenState::Running, Some(1000), Some(2000))]).await;
        let (mut fr, _fw, _) = open_follow_client(&sock).await;

        // A stale `ask_what_the_host_has` probe (from before connect-time helper
        // auto-upgrade, or on a multi-helper host) reports `helper_has_screens = Some(false)`
        // on the same `conn_id`: MUST NOT wipe the active live follow stream!
        svc.set_host_lacking("dev", id, cid, None, Some(false));
        let snap = svc.snapshot();
        let dev = &snap.hosts[0];
        assert_eq!(dev.state, RexHostConn::Connected);
        assert_eq!(dev.installed.as_deref(), Some("2026.11.27"));
        assert_ne!(dev.not_set_up_reason.as_deref(), Some(OLD_HELPER_REASON), "stale Some(false) must not mark live host incompatible");
        assert_eq!(dev.screens.len(), 1, "stale Some(false) wiped live follow screens");

        // Subsequent incremental follow updates MUST continue working normally!
        send_follow_event(&mut fh, &FollowEvent::Screen { screen: Box::new(sample_host_screen("second", HostScreenState::Running, Some(1500), Some(2500))) }).await;
        wait_follow_until(&mut fr, |l| matches!(l, RexFollowLine::HostUpdated { host, .. } if host.key == "dev" && host.screens.len() == 2).then_some(())).await;
        svc.shutdown();
    }

    #[tokio::test]
    async fn agy_f4_legacy_helper_probe_does_not_oscillate_parked_follower() {
        let dir = scratch_dir("agy-f4");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let opener = DoubleOpener::new();
        let svc = ScreensService::new(sock.clone(), state_path.clone(), std::process::id(), opener.clone(), Arc::new(|_, _| None), None, None);
        let id = next_host_instance_id();
        svc.set_initial_hosts(vec![("legacy".into(), id, "Legacy".into(), RexHostConn::Disconnected)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();
        let (mut fr, _fw, _) = open_follow_client(&sock).await;

        opener.push_stream_err("legacy", "follow", "the host has no screens-capable Olo helper");
        let cid = next_conn_id();
        svc.host_connected(&rt, "legacy", id, cid, 1, "Legacy", None);
        wait_follow_until(&mut fr, |l| matches!(l, RexFollowLine::HostUpdated { host, .. } if host.key == "legacy" && host.not_set_up_reason.as_deref() == Some(OLD_HELPER_REASON)).then_some(()))
            .await;
        let opens_before = opener.recorded_streams().len();

        // Pre-Step-5 helper probe reports `helper_follow == false` -> `set_host_lacking(..., Some(false))`
        // MUST keep follower parked with zero extra follow attempts or state oscillation!
        svc.set_host_lacking("legacy", id, cid, None, Some(false));
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(opener.recorded_streams().len(), opens_before, "pre-Step-5 helper probe unparked follower");
        let h = &svc.snapshot().hosts[0];
        assert_eq!(h.state, RexHostConn::Connected);
        assert_eq!(h.not_set_up_reason.as_deref(), Some(OLD_HELPER_REASON));
        svc.shutdown();
    }

    #[test]
    fn agy_f5_desktop_owner_fails_closed_on_untrusted_dirs_and_prefers_state_when_stopped() {
        let dir = scratch_dir("agy-f5");
        let _cleanup = ScratchCleanup(dir.clone());
        let real_rt = dir.join("real-rt");
        let state_root = dir.join("state");
        std::fs::create_dir(&real_rt).unwrap();
        std::fs::set_permissions(&real_rt, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(&state_root).unwrap();
        std::fs::set_permissions(&state_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let qa_state = state_root.join("qa");
        std::fs::create_dir(&qa_state).unwrap();
        std::fs::write(qa_state.join("actions.jsonl"), b"").unwrap();

        // 1. Symlinked `rt_dir` MUST fail closed (Err on record_desktop_owner_in,
        //    false on unlink_desktop_runtime_owner_in) instead of bypassing expected_gen!
        let sym_rt = dir.join("sym-rt");
        std::os::unix::fs::symlink(&real_rt, &sym_rt).unwrap();
        let exp = DesktopScreenGen { pid: std::process::id() as i32, started: 1000, start_ticks: None };
        let res = record_desktop_owner_in(&sym_rt, &state_root, "qa", "dev", "s1", "b1", false, Some(exp));
        assert!(res.is_err(), "symlinked rt_dir must fail closed, got: {res:?}");
        assert!(!qa_state.join("owner.json").exists(), "failed verification must never write state owner.json");
        assert!(!unlink_desktop_runtime_owner_in(&sym_rt, &state_root, "qa", Some(exp)), "symlinked rt_dir must return false on unlink");

        // 2. For a stopped screen, persistent `state_root/qa/owner.json` MUST win
        //    over a stale runtime `rt_dir/qa.owner.json` left by an earlier crash!
        let stale_rt = ScreenOwner {
            host_key: Some("dev".into()),
            session_id: "s-old".into(),
            block_id: "pane-a".into(),
            started_by: None,
            pid: None,
            began: None,
            rex_socket: None,
            at: 100,
            screen_pid: Some(999_999),
            screen_started: Some(100),
            screen_start_ticks: None,
        };
        let fresh_st = ScreenOwner {
            host_key: Some("dev".into()),
            session_id: "s-new".into(),
            block_id: "pane-b".into(),
            started_by: None,
            pid: None,
            began: None,
            rex_socket: None,
            at: 200,
            screen_pid: None,
            screen_started: None,
            screen_start_ticks: None,
        };
        std::fs::write(real_rt.join("qa.owner.json"), serde_json::to_vec(&stale_rt).unwrap()).unwrap();
        std::fs::write(qa_state.join("owner.json"), serde_json::to_vec(&fresh_st).unwrap()).unwrap();
        let loaded = load_desktop_owners_in(&real_rt, &state_root);
        assert_eq!(loaded.get("qa").map(|o| o.block_id.as_str()), Some("pane-b"), "stopped screen must prefer persistent state owner over stale runtime owner");
    }

    #[tokio::test]
    async fn agy_f6_control_char_only_host_label_falls_back_to_key_and_validates() {
        let dir = scratch_dir("agy-f6");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let svc = ScreensService::new(sock.clone(), state_path.clone(), std::process::id(), DoubleOpener::new(), Arc::new(|_, _| None), None, None);
        let id = next_host_instance_id();
        svc.set_initial_hosts(vec![("ctrl-box".into(), id, "\x1b\x07".into(), RexHostConn::Disconnected)]);
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).expect("control-char-only label must fall back to key so start_server validates");

        let snap = svc.snapshot();
        rex_sock::validate_state(&snap).expect("snapshot must validate");
        assert_eq!(snap.hosts[0].label, "ctrl-box");

        let (mut fr, _fw, follow_snap) = open_follow_client(&sock).await;
        assert_eq!(follow_snap.hosts[0].label, "ctrl-box");

        // Renaming to another control-char-only string must also fall back cleanly
        // without disconnecting active Follow subscribers!
        svc.host_renamed("ctrl-box", id, "Good Label");
        wait_follow_until(&mut fr, |l| matches!(l, RexFollowLine::HostUpdated { host, .. } if host.key == "ctrl-box" && host.label == "Good Label").then_some(())).await;
        svc.host_renamed("ctrl-box", id, "\x1b");
        wait_follow_until(&mut fr, |l| matches!(l, RexFollowLine::HostUpdated { host, .. } if host.key == "ctrl-box" && host.label == "ctrl-box").then_some(())).await;
        let list_resp = rpc_once(&sock, &RexSockRequest::List).await;
        assert!(matches!(list_resp, RexSockResponse::Snapshot { state } if state.hosts[0].label == "ctrl-box"));
        svc.shutdown();
    }

    #[tokio::test]
    async fn agy_f8_replaced_host_key_updates_screens_service_instance_id() {
        let dir = scratch_dir("agy-f8");
        let _cleanup = ScratchCleanup(dir.clone());
        let sock = dir.join("rex.sock");
        let state_path = dir.join("state.json");
        let opener = DoubleOpener::new();
        let svc = ScreensService::new(sock.clone(), state_path.clone(), std::process::id(), opener.clone(), Arc::new(|_, _| None), None, None);
        let id1 = next_host_instance_id();
        let id2 = next_host_instance_id();
        let rt = tokio::runtime::Handle::current();
        svc.start_server(&rt).unwrap();

        svc.host_added("dup", id1, "First", RexHostConn::Disconnected);
        // Mirror `add_host_as`: remove old instance before re-adding same key with `id2`.
        svc.host_removed("dup", id1);
        svc.host_added("dup", id2, "Second", RexHostConn::Disconnected);
        let (fc, mut fh) = tokio::io::duplex(16 * 1024);
        opener.push_stream("dup", "follow", Box::pin(fc));
        svc.host_connected(&rt, "dup", id2, next_conn_id(), 1, "Second", None);
        send_follow_event(&mut fh, &FollowEvent::Snapshot { proto: 1, status: sample_status(Some("2026.11.27"), true, None), screens: vec![] }).await;
        let (mut fr, _fw, snap) = open_follow_client(&sock).await;
        if !snap.hosts.iter().any(|h| h.key == "dup" && h.state == RexHostConn::Connected) {
            wait_follow_until(&mut fr, |l| matches!(l, RexFollowLine::HostUpdated { host, .. } if host.key == "dup" && host.state == RexHostConn::Connected).then_some(())).await;
        }
        assert_eq!(svc.snapshot().hosts[0].label, "Second");
        svc.shutdown();
    }
}
