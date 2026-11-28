//! Local Unix socket server exposing Rex's connected SSH hosts to Olo Tracker
//! (`rex://HOST/PATH`).
//!
//! Tracker never opens its own SSH connections: all listing, stat, mkdir,
//! rename, copy, delete, file read/write, and `olo-rex-sync` transfers go
//! through Rex's existing `SshSessionPool` and `olo-rex-link` helper on each
//! host. Every connection is authenticated via `SO_PEERCRED` (same UID only)
//! and bound with mode `0600` at [`olo_kit_base::tracker::rex_files_socket`].

use std::{
    collections::HashMap,
    io,
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
        io::{AsRawFd, RawFd},
    },
    path::{Component, Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use base64::Engine as _;
use olo_rex_client::{
    RexEndpoint, SshAskpass, SshSessionPool, SshTarget,
    helper::{Asked, ask as helper_ask, file as helper_file, put as helper_put},
    provision::shell_word,
    ssh::Verified,
    sync::{HostFolder, Stop, receive_from, receive_paths, send_paths, send_to},
};
use olo_rex_sync::{index::Skipped, wire::LazyFolderSource};
use olo_sync::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{OwnedSemaphorePermit, Semaphore},
};

use crate::hub::{HostConnState, HostHelper, HubState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HelperPolicy {
    Allowed,
    DisabledGlobally,
    DisabledForHost,
}

struct ResolvedHost {
    key: String,
    label: String,
    target: SshTarget,
    pool: Arc<SshSessionPool>,
    askpass: Option<Arc<dyn SshAskpass>>,
    helper_ready: bool,
    helper_policy: HelperPolicy,
}

/// Maximum concurrent client tasks admitted on the Rex files Unix socket.
const MAX_CONCURRENT_CLIENTS: usize = 32;

/// Maximum inline JSON request line accepted on the local Rex files socket (32 MiB;
/// charged 2× against the 64 MiB inline budget to cover raw JSON + decoded payload).
const MAX_REQUEST_LINE_BYTES: usize = 32 * 1024 * 1024;

/// Granularity of the global in-flight inline-memory semaphore (64 KiB).
const INLINE_SLAB_BYTES: usize = 64 * 1024;

/// Total in-flight inline request/payload memory permitted across all
/// concurrent clients (64 MiB), preventing 32 clients from holding gigabytes.
const MAX_INFLIGHT_INLINE_BYTES: usize = 64 * 1024 * 1024;

/// Maximum concurrent large JSON replies (e.g. huge directory listings) buffered at once.
const MAX_CONCURRENT_LARGE_REPLIES: usize = 4;

/// Maximum JSON reply frame sent back to Tracker (32 MiB).
const MAX_REPLY_LINE_BYTES: usize = 32 * 1024 * 1024;

/// Maximum time a newly connected peer has to send its opening JSON request line.
const FIRST_LINE_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum time spent writing a single reply line back to a local peer.
const REPLY_WRITE_TIMEOUT: Duration = Duration::from_secs(15);

/// Maximum size of a follow-up control line (`{"op":"cancel"}`) during a transfer.
const MAX_CONTROL_LINE_BYTES: usize = 4096;

/// Maximum time allowed for helper version probing / installation before failing.
const HELPER_PREP_TIMEOUT: Duration = Duration::from_secs(45);

/// Total server deadline for short metadata/mutation requests (`list`, `stat`,
/// `mkdir`, `rename`, `remove`) including first-time helper preparation.
/// Tracker waits 75 s, leaving 15 s of response margin.
const OP_TIMEOUT: Duration = Duration::from_secs(60);

/// Total server deadline for recursive remote `copy_remote` (Tracker waits 315 s).
const COPY_REMOTE_TIMEOUT: Duration = Duration::from_secs(300);

/// Per-chunk inactivity deadline and total cap for `read_to_cache`.
const READ_CACHE_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const READ_CACHE_TOTAL_TIMEOUT: Duration = Duration::from_secs(600);

/// Auxiliary remote `fs` command timeout inside streaming transfers.
const REMOTE_CMD_TIMEOUT: Duration = Duration::from_secs(30);

/// Per-I/O inactivity deadline for `download`/`upload` sync streams (Tracker waits 180 s).
const TRANSFER_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Dedicated staging directory name under `tracker-rex`, completely separate
/// from published remote basenames under `files/`.
const CACHE_STAGING_DIR: &str = ".olo-staging";
const CACHE_FILES_DIR: &str = "files";

/// Cache retention policy: remove stale temp parts older than 1 hour, keep
/// published cache within 512 MiB / 256 buckets while protecting recently used
/// versions for 5 minutes (up to a 1 GiB hard ceiling).
const STALE_PART_AGE: Duration = Duration::from_secs(3600);
const CACHE_MAX_BYTES: u64 = 512 * 1024 * 1024;
const CACHE_HARD_MAX_BYTES: u64 = 1024 * 1024 * 1024;
const CACHE_MAX_ENTRIES: usize = 256;
const CACHE_MIN_EVICT_AGE: Duration = Duration::from_secs(300);
/// Minimum grace window protecting a just-published or just-hit cache entry
/// (even an oversized >1 GiB file) from immediate self-eviction before the
/// caller opens it.
const CACHE_HANDOFF_GRACE: Duration = Duration::from_secs(30);
const CACHE_MAINT_COOLDOWN_SECS: u64 = 120;

/// Typed client request (unknown JSON fields are ignored without allocating).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClientRequest {
    op: String,
    host: String,
    path: String,
    from: String,
    to: String,
    hidden: bool,
    unique: bool,
    base64: Option<String>,
    max_bytes: Option<u64>,
    remote_path: String,
    local_dest_dir: String,
    local_name: String,
    local_path: String,
    remote_dest_dir: String,
    remote_name: String,
    is_dir: bool,
    default_mode: bool,
    expect_endpoint: Option<String>,
    expect_dev: Option<u64>,
    expect_ino: Option<u64>,
    expect_size: Option<u64>,
    expect_mtime_secs: Option<i64>,
    expect_mtime_nanos: Option<u32>,
    expect_ctime_secs: Option<i64>,
    expect_ctime_nanos: Option<u32>,
}

const STATE_RUNNING: u8 = 0;
const STATE_CANCELLED: u8 = 1;
const STATE_COMMITTED: u8 = 2;

#[derive(Debug)]
enum TransferError {
    Cancelled,
    Failed(String),
}

/// Evaluate effective helper policy from both the global Preferences ▸ Rex
/// setting (`host_helper`) and the per-host override (`helper_hosts`).
fn helper_policy(host_helper: bool, per_host: Option<&str>) -> HelperPolicy {
    if !host_helper {
        HelperPolicy::DisabledGlobally
    } else if per_host == Some("off") {
        HelperPolicy::DisabledForHost
    } else {
        HelperPolicy::Allowed
    }
}

fn helper_policy_allows(host_helper: bool, per_host: Option<&str>) -> bool {
    helper_policy(host_helper, per_host) == HelperPolicy::Allowed
}

fn helper_policy_error(policy: HelperPolicy, host_label: &str) -> String {
    match policy {
        HelperPolicy::DisabledGlobally => "Olo’s helper is turned off in Preferences ▸ Rex. Turn it on to browse remote files.".to_owned(),
        HelperPolicy::DisabledForHost => format!("Olo’s helper is turned off for “{host_label}”. Turn it back on in Rex’s Host window to browse its files."),
        HelperPolicy::Allowed => unreachable!("helper_policy_error called for Allowed"),
    }
}

pub(crate) fn helper_host_lock(host_key: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let map = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    map.lock().unwrap_or_else(|e| e.into_inner()).entry(host_key.to_owned()).or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))).clone()
}

fn conn_state_str(s: &HostConnState) -> (&'static str, Option<String>) {
    match s {
        HostConnState::Connected => ("connected", None),
        HostConnState::Connecting => ("connecting", None),
        HostConnState::Reconnecting { last_error, .. } => ("reconnecting", (!last_error.is_empty()).then(|| last_error.clone())),
        HostConnState::Disconnected => ("disconnected", None),
        HostConnState::Error(e) => ("error", Some(e.clone())),
    }
}

fn find_host_key(st: &HubState, host: &str) -> Option<String> {
    if let Some(h) = st.hosts.values().find(|h| !h.is_local && h.label == host) {
        return Some(h.key.clone());
    }
    if let Some(h) = st.hosts.values().find(|h| !h.is_local && h.label.eq_ignore_ascii_case(host)) {
        return Some(h.key.clone());
    }
    st.hosts.get(host).filter(|h| !h.is_local).map(|h| h.key.clone())
}

/// A host's helper can run this build's `fs` subcommands (such as `--len` and
/// `--private`) without re-uploading only when the helper is installed *and*
/// known to be this build's current ID (`helper_current == Some(true)`).
fn helper_is_current(helper: &HostHelper, helper_current: Option<bool>) -> bool {
    matches!(helper, HostHelper::Used | HostHelper::Ready) && helper_current == Some(true)
}

/// Read one newline-delimited JSON request up to `max_bytes`.
/// Checks the byte limit BEFORE UTF-8 decoding so a multi-byte character
/// straddling `max_bytes + 1` still reports the size limit rather than a
/// spurious invalid-UTF-8 error.
async fn read_request_line<R: AsyncBufReadExt + Unpin>(reader: &mut R, max_bytes: usize) -> Result<Option<String>, String> {
    let (line, _permits) = read_request_line_budgeted(reader, max_bytes, None).await?;
    Ok(line)
}

/// Bounded newline-delimited reader that draws 64 KiB permits from `budget`
/// once a request exceeds 64 KiB so concurrent large inline writes stay within
/// [`MAX_INFLIGHT_INLINE_BYTES`] while tiny metadata ops never contend.
/// Acquires `gate` lazily only for requests > 64 KiB and drops it as soon as
/// the line finishes, eliminating hold-and-wait deadlock between concurrent
/// large partial readers. Charges 2× wire bytes while reading so raw JSON +
/// parsed/decoded payload stay within the memory budget.
async fn read_request_line_budgeted<R: AsyncBufReadExt + Unpin>(
    reader: &mut R,
    max_bytes: usize,
    budget: Option<(&Arc<Semaphore>, &Arc<tokio::sync::Mutex<()>>)>,
) -> Result<(Option<String>, Option<OwnedSemaphorePermit>), String> {
    let mut buf = Vec::new();
    let mut held: Option<OwnedSemaphorePermit> = None;
    let mut reserved_bytes = INLINE_SLAB_BYTES;
    let mut _large_gate = None;
    loop {
        let chunk = reader.fill_buf().await.map_err(|e| e.to_string())?;
        if chunk.is_empty() {
            if buf.is_empty() {
                return Ok((None, held));
            }
            break;
        }
        let (take, saw_nl) = match chunk.iter().position(|&b| b == b'\n') {
            Some(pos) => (pos + 1, true),
            None => (chunk.len(), false),
        };
        if buf.len().saturating_add(take) > max_bytes {
            return Err(format!("request exceeds {max_bytes} byte inline limit"));
        }
        let next_len = buf.len() + take;
        if let Some((sem, gate)) = budget
            && next_len > INLINE_SLAB_BYTES
        {
            if _large_gate.is_none() {
                _large_gate = Some(gate.lock().await);
            }
            // Account for raw line + parsed/decoded copy (2× wire bytes).
            let charged_bytes = next_len.saturating_mul(2);
            if charged_bytes > reserved_bytes {
                let extra = (charged_bytes - reserved_bytes).div_ceil(INLINE_SLAB_BYTES);
                let acquired = sem.clone().acquire_many_owned(extra as u32).await.map_err(|_| "server inline memory budget closed".to_owned())?;
                match held.as_mut() {
                    Some(existing) => existing.merge(acquired),
                    None => held = Some(acquired),
                }
                reserved_bytes = reserved_bytes.saturating_add(extra * INLINE_SLAB_BYTES);
            }
        }
        buf.extend_from_slice(&chunk[..take]);
        reader.consume(take);
        if saw_nl {
            break;
        }
    }
    let s = String::from_utf8(buf).map_err(|e| format!("request is not valid UTF-8: {e}"))?;
    Ok((Some(s), held))
}

/// Shrink a merged inline-budget permit down to exactly `keep_slabs` permits,
/// releasing excess permits back to the semaphore immediately.
fn shrink_inline_permit(permit: Option<OwnedSemaphorePermit>, keep_slabs: usize) -> Option<OwnedSemaphorePermit> {
    let mut p = permit?;
    if keep_slabs == 0 {
        return None;
    }
    if p.num_permits() <= keep_slabs {
        return Some(p);
    }
    p.split(keep_slabs)
}

/// Extract and base64-decode a required string `base64` field for `write_file`.
/// Missing or non-string fields are rejected instead of defaulting to an empty file.
fn decode_write_payload(raw_field: Option<&str>) -> Result<Vec<u8>, String> {
    let b64 = raw_field.ok_or_else(|| "missing string base64 payload".to_owned())?;
    base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()).map_err(|e| format!("invalid base64: {e}"))
}

fn resolve_host(state: &Arc<Mutex<HubState>>, host: &str) -> Result<ResolvedHost, String> {
    let st = state.lock().unwrap_or_else(|e| e.into_inner());
    let Some(key) = find_host_key(&st, host) else {
        return Err(format!("“{host}” isn’t one of Rex’s hosts."));
    };
    let h = &st.hosts[&key];
    match &h.conn_state {
        HostConnState::Connected => {}
        HostConnState::Connecting => return Err(format!("Connecting to “{}”…", h.label)),
        HostConnState::Reconnecting { .. } => return Err(format!("Reconnecting to “{}”…", h.label)),
        HostConnState::Disconnected => return Err(format!("“{}” isn’t connected in Rex.", h.label)),
        HostConnState::Error(e) => return Err(format!("“{}” isn’t connected: {e}", h.label)),
    }
    let Some(RexEndpoint::Ssh { target, pool, askpass }) = h.endpoint.clone() else {
        return Err(format!("“{}” isn’t reached over SSH.", h.label));
    };
    let helper_ready = helper_is_current(&h.helper, h.helper_current);
    let policy = helper_policy(st.settings.host_helper, st.settings.helper_hosts.get(&key).map(String::as_str));
    Ok(ResolvedHost { key, label: h.label.clone(), target, pool, askpass, helper_ready, helper_policy: policy })
}

fn host_same_endpoint(st: &HubState, host: &ResolvedHost) -> bool {
    st.hosts.get(&host.key).is_some_and(|hr| {
        matches!(
            &hr.endpoint,
            Some(RexEndpoint::Ssh { target, pool, .. }) if *target == host.target && Arc::ptr_eq(pool, &host.pool)
        )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HelperCheck {
    policy: HelperPolicy,
    same_endpoint: bool,
    ready: bool,
}

async fn ensure_helper(state: &Arc<Mutex<HubState>>, host: &ResolvedHost) -> Result<(), String> {
    ensure_helper_with_timeout(state, host, HELPER_PREP_TIMEOUT).await
}

async fn ensure_helper_with_timeout(state: &Arc<Mutex<HubState>>, host: &ResolvedHost, timeout: Duration) -> Result<(), String> {
    let check_state = || {
        let st = state.lock().unwrap_or_else(|e| e.into_inner());
        let policy = helper_policy(st.settings.host_helper, st.settings.helper_hosts.get(&host.key).map(String::as_str));
        let same_endpoint = host_same_endpoint(&st, host);
        let ready = st.hosts.get(&host.key).is_some_and(|hr| helper_is_current(&hr.helper, hr.helper_current));
        HelperCheck { policy, same_endpoint, ready }
    };
    let mark_ready = || {
        let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
        let live_policy = helper_policy(st.settings.host_helper, st.settings.helper_hosts.get(&host.key).map(String::as_str));
        let same = host_same_endpoint(&st, host);
        if live_policy == HelperPolicy::Allowed && same {
            if let Some(hr) = st.hosts.get_mut(&host.key) {
                hr.helper_current = Some(true);
                hr.helper_sync_err = None;
                if !matches!(hr.helper, HostHelper::Used) {
                    hr.helper = HostHelper::Ready;
                }
            }
            Ok(())
        } else if live_policy != HelperPolicy::Allowed {
            Err(helper_policy_error(live_policy, &host.label))
        } else {
            Err(format!("Connection to “{}” changed while preparing Olo’s helper.", host.label))
        }
    };
    ensure_helper_core(
        &host.key,
        &host.label,
        host.helper_policy,
        host.helper_ready,
        crate::link::carried() && crate::link::syncs(),
        timeout,
        check_state,
        mark_ready,
        || async {
            match helper_ask(&host.pool, &host.target, host.askpass.clone(), crate::link::ID, &crate::link::BUILDS).await {
                Ok(Ok(Asked::Here)) => Ok(None),
                Ok(Ok(Asked::Needs(build) | Asked::Outdated(build))) => Ok(Some(build)),
                Ok(Err(kind)) => Err(format!("No Olo helper build is available for “{}” ({kind}).", host.label)),
                Err(e) => Err(format!("Couldn’t reach “{}”: {}", host.label, e.why)),
            }
        },
        |build| async move {
            helper_put(&host.pool, &host.target, host.askpass.clone(), crate::link::ID, build).await.map(|_| ()).map_err(|e| format!("Couldn’t install Olo’s helper on “{}”: {}", host.label, e.why))
        },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn ensure_helper_core<B, CheckFn, MarkFn, AskFn, AskFut, PutFn, PutFut>(
    host_key: &str,
    host_label: &str,
    initial_policy: HelperPolicy,
    initial_ready: bool,
    carried_and_syncs: bool,
    timeout: Duration,
    check_state: CheckFn,
    mark_ready: MarkFn,
    ask: AskFn,
    put: PutFn,
) -> Result<(), String>
where
    CheckFn: Fn() -> HelperCheck,
    MarkFn: FnOnce() -> Result<(), String>,
    AskFn: FnOnce() -> AskFut,
    AskFut: std::future::Future<Output = Result<Option<B>, String>>,
    PutFn: FnOnce(B) -> PutFut,
    PutFut: std::future::Future<Output = Result<(), String>>,
{
    let started = Instant::now();
    let remaining = || timeout.saturating_sub(started.elapsed());
    // Always enforce policy BEFORE checking helper_ready so toggling
    // Preferences ▸ Rex ▸ host_helper off or Remove Helper takes effect immediately.
    if initial_policy != HelperPolicy::Allowed {
        return Err(helper_policy_error(initial_policy, host_label));
    }
    {
        let cur = check_state();
        if cur.policy != HelperPolicy::Allowed {
            return Err(helper_policy_error(cur.policy, host_label));
        }
        if !cur.same_endpoint {
            return Err(format!("Connection to “{host_label}” changed. Try again."));
        }
        if initial_ready && cur.ready {
            return Ok(());
        }
    }
    if !carried_and_syncs {
        return Err(format!("Olo’s helper isn’t built with remote file support for “{host_label}”."));
    }

    // Serialize helper probes/installs per host so concurrent requests do not
    // upload twice and policy/endpoint changes are checked cleanly. Count lock
    // wait time against the single absolute preparation deadline.
    let host_lock = helper_host_lock(host_key);
    let _guard = tokio::time::timeout(remaining(), host_lock.lock()).await.map_err(|_| format!("Timed out preparing Olo’s helper on “{host_label}”."))?;

    {
        let cur = check_state();
        if cur.policy != HelperPolicy::Allowed {
            return Err(helper_policy_error(cur.policy, host_label));
        }
        if !cur.same_endpoint {
            return Err(format!("Connection to “{host_label}” changed. Try again."));
        }
        if cur.ready {
            return Ok(());
        }
    }

    let rem = remaining();
    if rem.is_zero() {
        return Err(format!("Timed out preparing Olo’s helper on “{host_label}”."));
    }
    let maybe_build = tokio::time::timeout(rem, ask()).await.map_err(|_| format!("Timed out preparing Olo’s helper on “{host_label}”."))??;

    if let Some(build) = maybe_build {
        // Re-verify policy and endpoint after probing, before uploading anything.
        {
            let cur = check_state();
            if cur.policy != HelperPolicy::Allowed {
                return Err(helper_policy_error(cur.policy, host_label));
            }
            if !cur.same_endpoint {
                return Err(format!("Connection to “{host_label}” changed while preparing Olo’s helper."));
            }
        }
        let rem = remaining();
        if rem.is_zero() {
            return Err(format!("Timed out preparing Olo’s helper on “{host_label}”."));
        }
        tokio::time::timeout(rem, put(build)).await.map_err(|_| format!("Timed out preparing Olo’s helper on “{host_label}”."))??;
    }

    // On global opt-out or Remove Helper racing with prep, simply refuse to mark
    // ready and stop using the helper without shelling out to `rm`/`rmdir` or
    // deleting a binary that another workstation may share.
    mark_ready()
}

fn parse_helper_json(ran_output: &str, host_label: &str) -> Result<Value, String> {
    let trimmed = ran_output.trim();
    let line = trimmed.lines().rev().find(|l| l.trim().starts_with('{')).unwrap_or(trimmed);
    let val: Value = serde_json::from_str(line).map_err(|_| if trimmed.is_empty() { format!("“{host_label}” closed the connection before answering.") } else { trimmed.to_owned() })?;
    if val.get("ok").and_then(Value::as_bool) == Some(true) { Ok(val) } else { Err(val.get("error").and_then(Value::as_str).unwrap_or("remote filesystem operation failed").to_owned()) }
}

async fn run_fs_json_bounded_verified(host: &ResolvedHost, subcmd: &str, timeout: Duration) -> Result<(Value, Option<Verified>), String> {
    let cmd = format!("{} fs {subcmd}", helper_file(crate::link::ID));
    let (ran, verified) = tokio::time::timeout(timeout, host.pool.exec_bounded_verified(&host.target, host.askpass.clone(), &cmd, MAX_REPLY_LINE_BYTES, MAX_REPLY_LINE_BYTES, &mut |_| {}))
        .await
        .map_err(|_| format!("Timed out running remote filesystem operation on “{}”.", host.label))?
        .map_err(|e| e.to_string())?;
    if ran.truncated {
        return Err(format!("Remote response from “{}” exceeded the {} MiB limit.", host.label, MAX_REPLY_LINE_BYTES / (1024 * 1024)));
    }
    let val = parse_helper_json(&ran.output, &host.label)?;
    Ok((val, verified))
}

async fn run_fs_json_bounded(host: &ResolvedHost, subcmd: &str, timeout: Duration) -> Result<Value, String> {
    run_fs_json_bounded_verified(host, subcmd, timeout).await.map(|(v, _)| v)
}

async fn run_fs_json_expect_verified(host: &ResolvedHost, subcmd: &str, expected_ep: &str, timeout: Duration) -> Result<Value, String> {
    let cmd = format!("{} fs {subcmd}", helper_file(crate::link::ID));
    let target_c = host.target.clone();
    let expected_owned = expected_ep.to_owned();
    let label = host.label.clone();
    let ran = tokio::time::timeout(
        timeout,
        host.pool.exec_bounded_expect_verified(
            &host.target,
            host.askpass.clone(),
            &cmd,
            MAX_REPLY_LINE_BYTES,
            MAX_REPLY_LINE_BYTES,
            move |v| {
                let Some(v) = v else {
                    return Err(format!("Host “{label}” has no verified SSH identity; refusing source removal."));
                };
                let live_ep = format_verified_identity(&target_c, Some(v));
                if live_ep != expected_owned {
                    return Err(format!("Host “{label}” now points to a different SSH endpoint; refusing source removal."));
                }
                Ok(())
            },
            &mut |_| {},
        ),
    )
    .await
    .map_err(|_| format!("Timed out running remote filesystem operation on “{}”.", host.label))?
    .map_err(|e| e.to_string())?;
    if ran.truncated {
        return Err(format!("Remote response from “{}” exceeded the {} MiB limit.", host.label, MAX_REPLY_LINE_BYTES / (1024 * 1024)));
    }
    parse_helper_json(&ran.output, &host.label)
}

async fn run_fs_json_op_verified(
    state: &Arc<Mutex<HubState>>,
    host: &ResolvedHost,
    subcmd: &str,
    total_timeout: Duration,
    write_half: &tokio::net::unix::OwnedWriteHalf,
) -> Result<(Value, Option<Verified>), String> {
    let started = Instant::now();
    ensure_helper_with_timeout(state, host, total_timeout.min(HELPER_PREP_TIMEOUT)).await?;
    if peer_is_gone(write_half) {
        return Err("client disconnected".to_owned());
    }
    let rem = total_timeout.saturating_sub(started.elapsed());
    if rem.is_zero() {
        return Err(format!("Timed out running remote filesystem operation on “{}”.", host.label));
    }
    run_fs_json_bounded_verified(host, subcmd, rem).await
}

/// Verified endpoint & authenticated SSH account identity included in every
/// cache key and move-removal check so reusing a label or SSH alias for another
/// host or account can never return stale bytes or delete the wrong host's file.
fn format_verified_identity(target: &SshTarget, verified: Option<&Verified>) -> String {
    let raw_user = target.user.as_deref().unwrap_or("");
    let port = target.port.unwrap_or(22);
    let sock = target.socket_path.as_deref().unwrap_or("");
    match verified {
        Some(v) => format!("v2|au={}|u={raw_user}|h={}|p={port}|s={sock}|vh={}|vp={}|k={}", v.user, target.host, v.hostname, v.port, v.key),
        None => format!("v2|u={raw_user}|h={}|p={port}|s={sock}|unverified", target.host),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CacheVersion {
    size: u64,
    mtime: i64,
    mtime_ns: u64,
    ctime: i64,
    ctime_ns: u64,
    dev: u64,
    ino: u64,
}

/// Version-, cap-, inode-, and endpoint-specific cache path for a remote file.
/// Stored under `<cache_root>/files/…`, completely separate from `<cache_root>/.olo-staging`.
fn cache_entry_path_in(cache_root: &Path, host_identity: &str, host_label: &str, remote_path: &str, ver: CacheVersion, max_bytes: Option<u64>) -> PathBuf {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (host_identity, host_label, remote_path, ver, max_bytes).hash(&mut h);
    let host_dir: String = host_label.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_') { c } else { '_' }).collect();
    let host_dir = if host_dir.is_empty() || host_dir == "." || host_dir == ".." { "_host".to_owned() } else { host_dir };
    let name = remote_path.rsplit('/').find(|s| !s.is_empty() && *s != "." && *s != "..").unwrap_or("file");
    cache_root.join(CACHE_FILES_DIR).join(host_dir).join(format!("{:016x}", h.finish())).join(name)
}

fn cache_root_dir() -> PathBuf {
    olo_xdg::cache_dir().join("tracker-rex")
}

/// Dedicated Rex-owned staging path on the cache filesystem (`<cache_root>/.olo-staging/part-<uuid>`),
/// completely isolated from all remote filenames (even `.olo-part-notes`).
fn cache_part_in(cache_root: &Path) -> PathBuf {
    cache_root.join(CACHE_STAGING_DIR).join(format!("part-{}", uuid::Uuid::new_v4().simple()))
}

struct TempPartGuard(Option<PathBuf>);

impl TempPartGuard {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for TempPartGuard {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

fn ensure_private_dir_sync(dir: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(md) => {
            if md.file_type().is_symlink() || !md.is_dir() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} is not a real directory", dir.display())));
            }
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = dir.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut b = std::fs::DirBuilder::new();
            b.mode(0o700);
            match b.create(dir) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let md = std::fs::symlink_metadata(dir)?;
                    if md.file_type().is_symlink() || !md.is_dir() {
                        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} is not a real directory", dir.display())));
                    }
                    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        Err(e) => Err(e),
    }
}

/// Best-effort cleanup of abandoned staging files inside the dedicated
/// `<cache_root>/.olo-staging` directory ONLY, without following symlinks and
/// without ever touching published remote filenames under `files/`.
fn sweep_stale_cache_parts(cache_root: &Path, max_age: Duration) {
    let staging = cache_root.join(CACHE_STAGING_DIR);
    let Ok(md) = std::fs::symlink_metadata(&staging) else { return };
    if md.file_type().is_symlink() || !md.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(&staging) else { return };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(emd) = std::fs::symlink_metadata(&path) else { continue };
        if !emd.file_type().is_file() {
            continue;
        }
        let Ok(modified) = emd.modified() else { continue };
        if now.duration_since(modified).is_ok_and(|age| age >= max_age) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Process-wide lock synchronizing cache-hit `open(O_NOFOLLOW) + fstat + set_modified`
/// with `prune_cache_versions` per-bucket re-stat and unlink so a cache hit can
/// never hand out a path that is simultaneously being evicted.
fn cache_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Synchronized cache-hit check: opens the cached file with `O_NOFOLLOW`,
/// verifies it is a regular file of the exact expected size, and refreshes its
/// mtime under [`cache_lock`]. Returns `false` if any step fails (falling
/// through cleanly to the cache-miss download path).
fn try_touch_cached_file(path: &Path, expected: u64) -> bool {
    let _g = cache_lock().lock().unwrap_or_else(|e| e.into_inner());
    let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NOFOLLOW).open(path) else {
        return false;
    };
    let Ok(md) = f.metadata() else {
        return false;
    };
    if !md.file_type().is_file() || md.len() != expected {
        return false;
    }
    f.set_modified(SystemTime::now()).is_ok()
}

/// Bounded LRU eviction of published cache buckets under `<cache_root>/files`
/// without ever following symlinks. Synchronizes per-bucket re-stat and unlink
/// under [`cache_lock`]. Returns `Some(wait)` when the cache remains over budget
/// solely because candidate buckets are still within `CACHE_HANDOFF_GRACE`, so
/// the caller can schedule one deferred pass right after grace expiry.
fn prune_cache_versions(cache_root: &Path, max_bytes: u64, hard_max_bytes: u64, max_entries: usize, min_evict_age: Duration) -> Option<Duration> {
    let files_root = cache_root.join(CACHE_FILES_DIR);
    let Ok(root_md) = std::fs::symlink_metadata(&files_root) else { return None };
    if root_md.file_type().is_symlink() || !root_md.is_dir() {
        return None;
    }
    let mut buckets: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
    let mut total_bytes = 0u64;

    let Ok(hosts) = std::fs::read_dir(&files_root) else { return None };
    for host_entry in hosts.flatten() {
        let host_path = host_entry.path();
        let Ok(hmd) = std::fs::symlink_metadata(&host_path) else { continue };
        if hmd.file_type().is_symlink() || !hmd.is_dir() {
            continue;
        }
        let Ok(bdirs) = std::fs::read_dir(&host_path) else { continue };
        for bucket in bdirs.flatten() {
            let bpath = bucket.path();
            let Ok(bmd) = std::fs::symlink_metadata(&bpath) else { continue };
            if bmd.file_type().is_symlink() || !bmd.is_dir() {
                continue;
            }
            let mut bsize = 0u64;
            let mut newest: Option<SystemTime> = None;
            if let Ok(files) = std::fs::read_dir(&bpath) {
                for f in files.flatten() {
                    let fpath = f.path();
                    if let Ok(fmd) = std::fs::symlink_metadata(&fpath)
                        && fmd.file_type().is_file()
                    {
                        bsize = bsize.saturating_add(fmd.len());
                        if let Ok(m) = fmd.modified() {
                            newest = Some(newest.map_or(m, |prev| prev.max(m)));
                        }
                    }
                }
            }
            let newest = newest.unwrap_or_else(|| bmd.modified().unwrap_or(SystemTime::UNIX_EPOCH));
            total_bytes = total_bytes.saturating_add(bsize);
            buckets.push((bpath, bsize, newest));
        }
    }

    if total_bytes <= max_bytes && buckets.len() <= max_entries {
        return None;
    }
    // Evict oldest buckets first, never deleting entries within
    // `CACHE_HANDOFF_GRACE` (protects a just-hit file or a newly cached >1 GiB
    // movie from self-evicting before its caller opens it).
    buckets.sort_by_key(|(_, _, m)| *m);
    let mut remaining_entries = buckets.len();
    let mut soonest_grace_wait: Option<Duration> = None;
    for (bpath, bsize, _initial_mtime) in buckets {
        if total_bytes <= max_bytes && remaining_entries <= max_entries {
            break;
        }
        // Re-stat live mtime and unlink strictly under `cache_lock()` so a
        // concurrent cache hit that refreshed mtime can never be unlinked.
        let _g = cache_lock().lock().unwrap_or_else(|e| e.into_inner());
        let now = SystemTime::now();
        let Ok(bmd) = std::fs::symlink_metadata(&bpath) else {
            total_bytes = total_bytes.saturating_sub(bsize);
            remaining_entries = remaining_entries.saturating_sub(1);
            continue;
        };
        if bmd.file_type().is_symlink() || !bmd.is_dir() {
            continue;
        }
        let mut live_files: Vec<PathBuf> = Vec::new();
        let mut live_size = 0u64;
        let mut live_newest: Option<SystemTime> = None;
        if let Ok(files) = std::fs::read_dir(&bpath) {
            for f in files.flatten() {
                let fp = f.path();
                if let Ok(fmd) = std::fs::symlink_metadata(&fp)
                    && fmd.file_type().is_file()
                {
                    live_size = live_size.saturating_add(fmd.len());
                    if let Ok(m) = fmd.modified() {
                        live_newest = Some(live_newest.map_or(m, |prev| prev.max(m)));
                    }
                    live_files.push(fp);
                }
            }
        }
        let live_mtime = live_newest.unwrap_or_else(|| bmd.modified().unwrap_or(SystemTime::UNIX_EPOCH));
        let age = now.duration_since(live_mtime).unwrap_or(Duration::ZERO);
        if age < CACHE_HANDOFF_GRACE {
            let rem_grace = CACHE_HANDOFF_GRACE.saturating_sub(age);
            soonest_grace_wait = Some(soonest_grace_wait.map_or(rem_grace, |prev| prev.min(rem_grace)));
            continue;
        }
        if age < min_evict_age && total_bytes <= hard_max_bytes {
            continue;
        }
        for fp in live_files {
            let _ = std::fs::remove_file(&fp);
        }
        if std::fs::remove_dir(&bpath).is_ok() {
            total_bytes = total_bytes.saturating_sub(live_size.max(bsize));
            remaining_entries = remaining_entries.saturating_sub(1);
        }
    }
    if total_bytes > max_bytes || remaining_entries > max_entries { soonest_grace_wait } else { None }
}

/// Clean up recognized legacy `tracker-rex/<host>/<16-hex>` cache buckets from
/// before the `files/` + `.olo-staging/` split, without following symlinks.
fn sweep_legacy_cache_entries(cache_root: &Path, min_age: Duration) {
    let Ok(rd) = std::fs::read_dir(cache_root) else { return };
    let now = SystemTime::now();
    for entry in rd.flatten() {
        let name = entry.file_name();
        let name_s = name.to_string_lossy();
        if name_s == CACHE_FILES_DIR || name_s == CACHE_STAGING_DIR {
            continue;
        }
        let host_path = entry.path();
        let Ok(hmd) = std::fs::symlink_metadata(&host_path) else { continue };
        if hmd.file_type().is_symlink() || !hmd.is_dir() {
            continue;
        }
        let Ok(bdirs) = std::fs::read_dir(&host_path) else { continue };
        for b in bdirs.flatten() {
            let bname = b.file_name();
            let bname_s = bname.to_string_lossy();
            if bname_s.len() != 16 || !bname_s.bytes().all(|c| c.is_ascii_hexdigit()) {
                continue;
            }
            let bpath = b.path();
            let Ok(bmd) = std::fs::symlink_metadata(&bpath) else { continue };
            if bmd.file_type().is_symlink() || !bmd.is_dir() {
                continue;
            }
            let Ok(modified) = bmd.modified() else { continue };
            if now.duration_since(modified).is_ok_and(|age| age < min_age) {
                continue;
            }
            if let Ok(files) = std::fs::read_dir(&bpath) {
                for f in files.flatten() {
                    let fp = f.path();
                    if std::fs::symlink_metadata(&fp).is_ok_and(|m| m.file_type().is_file()) {
                        let _ = std::fs::remove_file(&fp);
                    }
                }
            }
            let _ = std::fs::remove_dir(&bpath);
        }
        let _ = std::fs::remove_dir(&host_path);
    }
}

fn maintain_cache(cache_root: &Path) -> Option<Duration> {
    sweep_stale_cache_parts(cache_root, STALE_PART_AGE);
    let grace_wait = prune_cache_versions(cache_root, CACHE_MAX_BYTES, CACHE_HARD_MAX_BYTES, CACHE_MAX_ENTRIES, CACHE_MIN_EVICT_AGE);
    sweep_legacy_cache_entries(cache_root, CACHE_MIN_EVICT_AGE);
    grace_wait
}

fn schedule_grace_followup(cache_root: PathBuf, wait: Duration) {
    static GRACE_DEFERRED: AtomicBool = AtomicBool::new(false);
    if GRACE_DEFERRED.compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
        let sleep_for = wait.saturating_add(Duration::from_secs(1));
        tokio::spawn(async move {
            tokio::time::sleep(sleep_for).await;
            GRACE_DEFERRED.store(false, Ordering::Release);
            let root = cache_root.clone();
            let _ = tokio::task::spawn_blocking(move || maintain_cache(&root)).await;
        });
    }
}

fn maybe_schedule_cache_maintenance(cache_root: PathBuf) {
    static LAST_MAINT_SECS: AtomicU64 = AtomicU64::new(0);
    static MAINT_DEFERRED: AtomicBool = AtomicBool::new(false);
    let now_secs = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let prev = LAST_MAINT_SECS.load(Ordering::Relaxed);
    let elapsed = now_secs.saturating_sub(prev);
    if elapsed < CACHE_MAINT_COOLDOWN_SECS {
        // Coalesce bursts during cooldown into ONE deferred follow-up pass so
        // retained disk usage cannot remain unboundedly above ceiling.
        if MAINT_DEFERRED.compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            let wait = Duration::from_secs(CACHE_MAINT_COOLDOWN_SECS - elapsed + 1);
            tokio::spawn(async move {
                tokio::time::sleep(wait).await;
                MAINT_DEFERRED.store(false, Ordering::Release);
                let run_secs = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                LAST_MAINT_SECS.store(run_secs, Ordering::Release);
                let root_c = cache_root.clone();
                if let Ok(Some(grace_wait)) = tokio::task::spawn_blocking(move || maintain_cache(&root_c)).await {
                    schedule_grace_followup(cache_root, grace_wait);
                }
            });
        }
        return;
    }
    if LAST_MAINT_SECS.compare_exchange(prev, now_secs, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
        tokio::spawn(async move {
            let root_c = cache_root.clone();
            if let Ok(Some(grace_wait)) = tokio::task::spawn_blocking(move || maintain_cache(&root_c)).await {
                schedule_grace_followup(cache_root, grace_wait);
            }
        });
    }
}

async fn write_line(w: &mut tokio::net::unix::OwnedWriteHalf, v: &Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(v).unwrap_or_else(|_| b"{\"ok\":false}".to_vec());
    if bytes.len() + 1 > MAX_REPLY_LINE_BYTES {
        bytes = serde_json::to_vec(&json!({ "ok": false, "error": format!("reply exceeds {} MiB limit", MAX_REPLY_LINE_BYTES / (1024 * 1024)) }))
            .unwrap_or_else(|_| b"{\"ok\":false,\"error\":\"reply too large\"}".to_vec());
    }
    bytes.push(b'\n');
    tokio::time::timeout(REPLY_WRITE_TIMEOUT, async {
        w.write_all(&bytes).await?;
        w.flush().await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "timed out writing reply to client"))?
}

fn parent_and_leaf(path: &str) -> Option<(String, String)> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return None;
    }
    match trimmed.rfind('/') {
        Some(0) => Some(("/".to_owned(), trimmed[1..].to_owned())),
        Some(i) => Some((trimmed[..i].to_owned(), trimmed[i + 1..].to_owned())),
        None => Some(("~".to_owned(), trimmed.to_owned())),
    }
}

fn validate_single_name(name: &str) -> Result<(), String> {
    let mut comps = Path::new(name).components();
    match (comps.next(), comps.next()) {
        (Some(Component::Normal(_)), None) if !name.contains('/') && !name.contains('\0') => Ok(()),
        _ => Err(format!("invalid destination filename “{name}”")),
    }
}

/// Atomically rename `src` to `dst` without replacing any existing file,
/// directory, or dangling symlink (`renameat2(RENAME_NOREPLACE)`). Fails closed
/// with `Unsupported` when neither native no-replace rename nor hard links are
/// supported (never copying bytes directly into the final pathname), and keeps
/// linked `dst` intact if removing `src` fails.
fn rename_noreplace(src: &Path, dst: &Path) -> io::Result<()> {
    rename_noreplace_inner(src, dst, false, false)
}

fn rename_noreplace_inner(src: &Path, dst: &Path, force_no_flag: bool, force_no_hardlink: bool) -> io::Result<()> {
    let err = if force_no_flag {
        io::Error::from_raw_os_error(libc::EINVAL)
    } else {
        let c_src = std::ffi::CString::new(src.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL byte"))?;
        let c_dst = std::ffi::CString::new(dst.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL byte"))?;
        // SAFETY: both pointers are valid NUL-terminated paths; AT_FDCWD is standard.
        let r = unsafe { libc::renameat2(libc::AT_FDCWD, c_src.as_ptr(), libc::AT_FDCWD, c_dst.as_ptr(), libc::RENAME_NOREPLACE) };
        if r == 0 {
            return Ok(());
        }
        io::Error::last_os_error()
    };
    if matches!(err.raw_os_error(), Some(libc::ENOSYS | libc::EINVAL)) {
        let lmeta = std::fs::symlink_metadata(src)?;
        if lmeta.file_type().is_file() {
            let link_res = if force_no_hardlink { Err(io::Error::from_raw_os_error(libc::EOPNOTSUPP)) } else { std::fs::hard_link(src, dst) };
            match link_res {
                Ok(()) => {
                    if let Err(e) = std::fs::remove_file(src) {
                        return Err(io::Error::new(e.kind(), format!("destination published at {}, but removing source failed: {e}", dst.display())));
                    }
                    return Ok(());
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(e),
                Err(e) if matches!(e.raw_os_error(), Some(libc::EPERM | libc::EOPNOTSUPP | libc::ENOSYS)) => {
                    return Err(io::Error::new(io::ErrorKind::Unsupported, "atomic no-replace rename is not supported on this filesystem"));
                }
                Err(e) => return Err(e),
            }
        }
        if lmeta.file_type().is_symlink() {
            let target = std::fs::read_link(src)?;
            std::os::unix::fs::symlink(&target, dst)?;
            if let Err(e) = std::fs::remove_file(src) {
                return Err(io::Error::new(e.kind(), format!("destination symlink published at {}, but removing source failed: {e}", dst.display())));
            }
            return Ok(());
        }
        return Err(io::Error::new(io::ErrorKind::Unsupported, "atomic no-replace rename is not supported for directories on this filesystem"));
    }
    Err(err)
}

struct StageDirGuard(*mut libc::DIR);
impl Drop for StageDirGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `self.0` was returned by `libc::fdopendir` and is closed once here.
            unsafe { libc::closedir(self.0) };
        }
    }
}

/// Remove an owned private staging directory, restoring owner `0700` access on
/// read-only subdirectories strictly via `openat(O_DIRECTORY | O_NOFOLLOW)` and
/// `fchmod` on opened directory descriptors (never path-following `chmod`).
fn remove_stage_dir(stage: &Path) -> io::Result<()> {
    use std::os::unix::io::{FromRawFd as _, OwnedFd};
    let c_dir = std::ffi::CString::new(stage.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL byte"))?;
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    // SAFETY: `c_dir` is NUL-terminated.
    let raw_fd = unsafe { libc::open(c_dir.as_ptr(), flags) };
    if raw_fd < 0 {
        let err = io::Error::last_os_error();
        return if err.kind() == io::ErrorKind::NotFound { Ok(()) } else { Err(err) };
    }
    // SAFETY: `raw_fd` is a freshly opened valid descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    empty_stage_dir_fd(fd)?;
    match std::fs::remove_dir(stage) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn empty_stage_dir_fd(fd: std::os::unix::io::OwnedFd) -> io::Result<()> {
    use std::os::unix::io::{FromRawFd as _, IntoRawFd as _, OwnedFd};
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `fd` is an open directory descriptor; `st` is valid output memory.
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if (st.st_mode & libc::S_IFMT) != libc::S_IFDIR {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "stage path is not a directory"));
    }
    // SAFETY: `geteuid` has no preconditions.
    let euid = unsafe { libc::geteuid() };
    if st.st_uid != euid {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "refusing to modify stage directory not owned by current user"));
    }
    if (st.st_mode & 0o700) != 0o700 {
        // SAFETY: `fd` is verified above to be an owned directory descriptor.
        if unsafe { libc::fchmod(fd.as_raw_fd(), (st.st_mode & 0o7777) | 0o700) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    let raw = fd.into_raw_fd();
    // SAFETY: `fdopendir` takes ownership of `raw` on success.
    let dirp = unsafe { libc::fdopendir(raw) };
    if dirp.is_null() {
        let err = io::Error::last_os_error();
        // SAFETY: `fdopendir` failed, so `raw` is still owned by us.
        let _ = unsafe { OwnedFd::from_raw_fd(raw) };
        return Err(err);
    }
    let guard = StageDirGuard(dirp);
    // SAFETY: `guard.0` is a live `DIR*`.
    let dir_fd = unsafe { libc::dirfd(guard.0) };
    let mut names: Vec<std::ffi::CString> = Vec::new();
    loop {
        // SAFETY: `errno` is cleared before `readdir` to distinguish EOF from error.
        unsafe { *libc::__errno_location() = 0 };
        // SAFETY: `guard.0` is exclusively owned by this frame.
        let ent = unsafe { libc::readdir(guard.0) };
        if ent.is_null() {
            let err_no = unsafe { *libc::__errno_location() };
            if err_no != 0 {
                return Err(io::Error::from_raw_os_error(err_no));
            }
            break;
        }
        // SAFETY: `d_name` is a NUL-terminated C string inside the dirent.
        let cname = unsafe { std::ffi::CStr::from_ptr((*ent).d_name.as_ptr()) };
        let bytes = cname.to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        names.push(cname.to_owned());
    }
    for name in names {
        // SAFETY: `dir_fd` is valid and `name` is a NUL-terminated single component.
        if unsafe { libc::unlinkat(dir_fd, name.as_ptr(), 0) } == 0 {
            continue;
        }
        let err = io::Error::last_os_error();
        if matches!(err.raw_os_error(), Some(libc::EISDIR | libc::EPERM)) {
            let open_flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
            // SAFETY: `dir_fd` is valid and `name` is a single component.
            let mut child_raw = unsafe { libc::openat(dir_fd, name.as_ptr(), open_flags) };
            if child_raw < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EACCES) {
                let mut cst: libc::stat = unsafe { std::mem::zeroed() };
                if unsafe { libc::fstatat(dir_fd, name.as_ptr(), &mut cst, libc::AT_SYMLINK_NOFOLLOW) } == 0 && (cst.st_mode & libc::S_IFMT) == libc::S_IFDIR && cst.st_uid == euid {
                    let _ = unsafe { libc::fchmodat(dir_fd, name.as_ptr(), (cst.st_mode & 0o7777) | 0o700, 0) };
                    child_raw = unsafe { libc::openat(dir_fd, name.as_ptr(), open_flags) };
                }
            }
            if child_raw < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: `child_raw` is a newly opened directory fd.
            let child_fd = unsafe { OwnedFd::from_raw_fd(child_raw) };
            empty_stage_dir_fd(child_fd)?;
            // SAFETY: `dir_fd` and `name` are valid.
            if unsafe { libc::unlinkat(dir_fd, name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
                return Err(io::Error::last_os_error());
            }
        } else if err.kind() != io::ErrorKind::NotFound {
            return Err(err);
        }
    }
    Ok(())
}

async fn remove_stage_dir_async(stage: PathBuf) {
    let p = stage.clone();
    if let Ok(Err(e)) = tokio::task::spawn_blocking(move || remove_stage_dir(&p)).await {
        tracing::warn!("Rex files_sock failed to clean stage {}: {e}", stage.display());
    }
}

/// Create a fresh private `0700` staging directory exclusively (`mkdir(2)`).
fn create_private_stage_dir(parent: &Path, prefix: &str) -> io::Result<PathBuf> {
    std::fs::create_dir_all(parent)?;
    let stage = parent.join(format!("{prefix}{}", uuid::Uuid::new_v4().simple()));
    let mut b = std::fs::DirBuilder::new();
    b.mode(0o700);
    b.create(&stage)?;
    std::fs::set_permissions(&stage, std::fs::Permissions::from_mode(0o700))?;
    Ok(stage)
}

fn describe_skipped(skipped: &Skipped) -> String {
    let mut parts = Vec::new();
    if skipped.non_utf8.count > 0 {
        parts.push(format!("{} non-UTF-8", skipped.non_utf8.count));
    }
    if skipped.unread.count > 0 {
        parts.push(format!("{} unreadable", skipped.unread.count));
    }
    if skipped.special.count > 0 {
        parts.push(format!("{} special", skipped.special.count));
    }
    if skipped.mounts.count > 0 {
        parts.push(format!("{} on another filesystem", skipped.mounts.count));
    }
    format!("transfer incomplete: host skipped {} entries ({})", skipped.total(), parts.join(", "))
}

/// Non-blocking check for full peer socket hangup (`POLLHUP`/`POLLERR`), which
/// stays false on a valid one-shot client half-close (`shutdown(SHUT_WR)`).
fn fd_peer_hung_up(fd: RawFd) -> bool {
    let mut pfd = libc::pollfd { fd, events: 0, revents: 0 };
    // SAFETY: `pfd` points to one valid pollfd struct; timeout 0 does not block.
    let rc = unsafe { libc::poll(&mut pfd, 1, 0) };
    rc > 0 && (pfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL)) != 0
}

fn peer_is_gone(write_half: &tokio::net::unix::OwnedWriteHalf) -> bool {
    fd_peer_hung_up(write_half.as_ref().as_raw_fd())
}

/// Wait asynchronously (zero polling) until the peer completely closes the
/// socket (`POLLHUP`/`POLLERR`). Does NOT fire when the peer only half-closes
/// its write half (`shutdown(SHUT_WR)`) while waiting for our reply.
async fn wait_for_hup(stream: &UnixStream) {
    use std::os::unix::io::{FromRawFd as _, OwnedFd};
    let sock_fd = stream.as_raw_fd();
    if fd_peer_hung_up(sock_fd) {
        return;
    }
    // Create a private epoll fd monitoring `sock_fd` with `events = 0` so ONLY
    // kernel-mandatory EPOLLHUP / EPOLLERR (never EPOLLIN / EPOLLRDHUP / EPOLLOUT)
    // make `epfd` readable in Tokio's outer epoll reactor.
    // SAFETY: `epoll_create1(EPOLL_CLOEXEC)` returns a fresh owned fd or -1.
    let raw_epfd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    if raw_epfd < 0 {
        tracing::warn!("Rex files_sock wait_for_hup epoll_create1 failed: {}; cancelling safely", io::Error::last_os_error());
        return;
    }
    // SAFETY: `raw_epfd >= 0` is a newly created fd exclusively owned here.
    let epfd = unsafe { OwnedFd::from_raw_fd(raw_epfd) };
    let mut ev = libc::epoll_event { events: 0, u64: 0 };
    // SAFETY: `epfd` is valid, `sock_fd` is borrowed from `stream` for the
    // entire duration of this function, and `ev` points to a valid epoll_event.
    if unsafe { libc::epoll_ctl(epfd.as_raw_fd(), libc::EPOLL_CTL_ADD, sock_fd, &mut ev) } != 0 {
        if !fd_peer_hung_up(sock_fd) {
            tracing::warn!("Rex files_sock wait_for_hup epoll_ctl failed: {}; cancelling safely", io::Error::last_os_error());
        }
        return;
    }
    // SAFETY: `epfd` is an exclusively owned valid `OwnedFd` whose lifetime
    // matches the returned `AsyncFd`.
    let Ok(async_epfd) = (unsafe { tokio::io::unix::AsyncFd::register_with_interest(epfd, tokio::io::Interest::READABLE) }) else {
        tracing::warn!("Rex files_sock wait_for_hup AsyncFd registration failed; cancelling safely");
        return;
    };
    loop {
        if fd_peer_hung_up(sock_fd) {
            return;
        }
        let Ok(mut guard) = async_epfd.readable().await else {
            return;
        };
        if fd_peer_hung_up(sock_fd) {
            return;
        }
        guard.clear_ready();
    }
}

/// Watch for either an explicit `{"op":"cancel"}` control line or a full peer
/// disconnect (`POLLHUP`/`POLLERR`). Preserves valid one-shot request half-close
/// (`shutdown(SHUT_WR)`) so slow uploads/ops still complete and deliver replies.
async fn wait_peer_cancel_or_disconnect(reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>) {
    loop {
        match read_request_line(reader, MAX_CONTROL_LINE_BYTES).await {
            Ok(Some(msg)) => {
                let is_cancel = serde_json::from_str::<Value>(msg.trim()).ok().and_then(|v| v.get("op").and_then(Value::as_str).map(|s| s == "cancel")).unwrap_or(false);
                if is_cancel {
                    return;
                }
            }
            Ok(None) => {
                // Write-half EOF: peer may still be reading our reply. Wait for
                // full hangup (POLLHUP/POLLERR) before treating it as cancelled.
                wait_for_hup(reader.get_ref().as_ref()).await;
                return;
            }
            Err(_) => return,
        }
    }
}

/// Single control-line / disconnect monitor spawned ONCE per request so a split
/// `{"op":"cancel"}\n` frame straddling phase boundaries (helper prep -> stat ->
/// mkdir -> transfer) is never dropped or lost across separate readers.
struct CancelMonitor {
    cancelled: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
    task: tokio::task::JoinHandle<()>,
}

impl CancelMonitor {
    fn spawn(mut reader: BufReader<tokio::net::unix::OwnedReadHalf>, stop: Option<Stop>, commit_state: Option<Arc<AtomicU8>>) -> Self {
        let cancelled = Arc::new(AtomicBool::new(false));
        let notify = Arc::new(tokio::sync::Notify::new());
        let cancelled_c = cancelled.clone();
        let notify_c = notify.clone();
        let task = tokio::spawn(async move {
            wait_peer_cancel_or_disconnect(&mut reader).await;
            cancelled_c.store(true, Ordering::Release);
            if let Some(cs) = commit_state {
                let _ = cs.compare_exchange(STATE_RUNNING, STATE_CANCELLED, Ordering::AcqRel, Ordering::Acquire);
            }
            if let Some(s) = stop {
                s.cancel();
            }
            notify_c.notify_waiters();
        });
        Self { cancelled, notify, task }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.cancelled.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}

impl Drop for CancelMonitor {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn handle_client(stream: UnixStream, state: Arc<Mutex<HubState>>, inline_budget: Arc<Semaphore>, large_reader_gate: Arc<tokio::sync::Mutex<()>>, reply_budget: Arc<Semaphore>) {
    // Fail closed: reject connections unless SO_PEERCRED confirms same UID.
    let Ok(cred) = stream.peer_cred() else {
        return;
    };
    // SAFETY: geteuid is a pure read of the current process's effective UID.
    let my_uid = unsafe { libc::geteuid() };
    if cred.uid() != my_uid {
        return;
    }
    let request_started = Instant::now();
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let (line, line_permits) = match tokio::time::timeout(FIRST_LINE_TIMEOUT, read_request_line_budgeted(&mut reader, MAX_REQUEST_LINE_BYTES, Some((&inline_budget, &large_reader_gate)))).await {
        Ok(Ok((Some(line), permits))) => (line, permits),
        Ok(Ok((None, _))) => return,
        Ok(Err(e)) => {
            let _ = write_line(&mut write_half, &json!({ "ok": false, "error": e })).await;
            return;
        }
        Err(_) => {
            let _ = write_line(&mut write_half, &json!({ "ok": false, "error": "timed out waiting for request" })).await;
            return;
        }
    };
    let req: ClientRequest = match serde_json::from_str(line.trim()) {
        Ok(v) => v,
        Err(e) => {
            drop(line);
            drop(line_permits);
            let _ = write_line(&mut write_half, &json!({ "ok": false, "error": format!("invalid request: {e}") })).await;
            return;
        }
    };
    // Release raw JSON line buffer immediately after parsing so it is never
    // held across remote SSH I/O.
    drop(line);

    match req.op.as_str() {
        "hosts" => {
            drop(req);
            drop(line_permits);
            let hosts: Vec<Value> = {
                let st = state.lock().unwrap_or_else(|e| e.into_inner());
                st.host_order
                    .iter()
                    .filter_map(|k| st.hosts.get(k))
                    .filter(|h| !h.is_local)
                    .map(|h| {
                        let (state_s, err) = conn_state_str(&h.conn_state);
                        json!({
                            "key": h.key,
                            "label": h.label,
                            "state": state_s,
                            "error": err,
                            "ssh": matches!(h.endpoint, Some(RexEndpoint::Ssh { .. })) || h.endpoint_raw.starts_with("ssh://"),
                            "helper": matches!(h.helper, HostHelper::Used | HostHelper::Ready),
                        })
                    })
                    .collect()
            };
            let _ = write_line(&mut write_half, &json!({ "ok": true, "hosts": hosts })).await;
        }
        "host_state" => {
            let host = req.host;
            drop(line_permits);
            let resp = {
                let st = state.lock().unwrap_or_else(|e| e.into_inner());
                match find_host_key(&st, &host).and_then(|k| st.hosts.get(&k)) {
                    Some(h) => {
                        let (state_s, err) = conn_state_str(&h.conn_state);
                        json!({ "ok": true, "found": true, "key": h.key, "label": h.label, "state": state_s, "error": err })
                    }
                    None => json!({ "ok": true, "found": false }),
                }
            };
            let _ = write_line(&mut write_half, &resp).await;
        }
        "list" => {
            let host_name = req.host;
            let path = req.path;
            let hidden = req.hidden;
            drop(line_permits);
            let cancel_mon = CancelMonitor::spawn(reader, None, None);
            let sub = if hidden { format!("list {} --hidden", shell_word(&path)) } else { format!("list {}", shell_word(&path)) };
            run_one_shot_fs_op(&state, &host_name, &sub, OP_TIMEOUT, true, Some(&reply_budget), None, &cancel_mon, &mut write_half).await;
        }
        "stat" => {
            let host_name = req.host;
            let path = req.path;
            drop(line_permits);
            let cancel_mon = CancelMonitor::spawn(reader, None, None);
            let sub = format!("stat {}", shell_word(&path));
            run_one_shot_fs_op(&state, &host_name, &sub, OP_TIMEOUT, true, None, None, &cancel_mon, &mut write_half).await;
        }
        "mkdir" => {
            let host_name = req.host;
            let path = req.path;
            let unique = req.unique;
            drop(line_permits);
            let cancel_mon = CancelMonitor::spawn(reader, None, None);
            let sub = if unique { format!("mkdir {} --unique", shell_word(&path)) } else { format!("mkdir {}", shell_word(&path)) };
            run_one_shot_fs_op(&state, &host_name, &sub, OP_TIMEOUT, true, None, None, &cancel_mon, &mut write_half).await;
        }
        "rename" => {
            let host_name = req.host;
            let from = req.from;
            let to = req.to;
            let unique = req.unique;
            drop(line_permits);
            let cancel_mon = CancelMonitor::spawn(reader, None, None);
            let sub = if unique { format!("rename {} {} --unique", shell_word(&from), shell_word(&to)) } else { format!("rename {} {}", shell_word(&from), shell_word(&to)) };
            run_one_shot_fs_op(&state, &host_name, &sub, OP_TIMEOUT, true, None, None, &cancel_mon, &mut write_half).await;
        }
        "copy_remote" => {
            let host_name = req.host;
            let from = req.from;
            let to = req.to;
            drop(line_permits);
            let cancel_mon = CancelMonitor::spawn(reader, None, None);
            let sub = format!("copy {} {}", shell_word(&from), shell_word(&to));
            run_one_shot_fs_op(&state, &host_name, &sub, COPY_REMOTE_TIMEOUT, true, None, None, &cancel_mon, &mut write_half).await;
        }
        "remove" => {
            let host_name = req.host;
            let path = req.path;
            drop(line_permits);
            let cancel_mon = CancelMonitor::spawn(reader, None, None);
            let sub = format!("remove {}", shell_word(&path));
            run_one_shot_fs_op(&state, &host_name, &sub, OP_TIMEOUT, false, None, None, &cancel_mon, &mut write_half).await;
        }
        "verified_remove" => {
            let host_name = req.host;
            let path = req.path;
            let is_dir = req.is_dir;
            drop(line_permits);
            let cancel_mon = CancelMonitor::spawn(reader, None, None);
            let Some(expect_endpoint) = req.expect_endpoint.filter(|ep| !ep.is_empty() && !ep.contains("|unverified")) else {
                let _ = write_line(&mut write_half, &json!({ "ok": false, "error": "verified_remove requires a verified endpoint_id" })).await;
                return;
            };
            let (Some(d), Some(i), Some(ms), Some(mn), Some(cs), Some(cn)) =
                (req.expect_dev, req.expect_ino, req.expect_mtime_secs, req.expect_mtime_nanos, req.expect_ctime_secs, req.expect_ctime_nanos)
            else {
                let _ = write_line(&mut write_half, &json!({ "ok": false, "error": "verified_remove requires complete lstat identity fields" })).await;
                return;
            };
            let mut sub =
                format!("remove {} --expect-dev {d} --expect-ino {i} --expect-mtime-secs {ms} --expect-mtime-nanos {mn} --expect-ctime-secs {cs} --expect-ctime-nanos {cn}", shell_word(&path));
            if !is_dir {
                let Some(s) = req.expect_size else {
                    let _ = write_line(&mut write_half, &json!({ "ok": false, "error": "verified_remove requires expect_size for non-directories" })).await;
                    return;
                };
                sub.push_str(&format!(" --expect-size {s}"));
            }
            run_one_shot_fs_op(&state, &host_name, &sub, OP_TIMEOUT, false, None, Some(expect_endpoint.as_str()), &cancel_mon, &mut write_half).await;
        }
        "write_file" => {
            let host_name = req.host;
            let path = req.path;
            let unique = req.unique;
            let bytes = match decode_write_payload(req.base64.as_deref()) {
                Ok(b) => b,
                Err(e) => {
                    drop(req.base64);
                    drop(line_permits);
                    let _ = write_line(&mut write_half, &json!({ "ok": false, "error": e })).await;
                    return;
                }
            };
            // Drop the parsed base64 String immediately and shrink held permits
            // down to cover ONLY the live decoded buffer during SSH streaming.
            drop(req.base64);
            let keep_slabs = bytes.len().saturating_sub(INLINE_SLAB_BYTES).div_ceil(INLINE_SLAB_BYTES);
            let live_permits = shrink_inline_permit(line_permits, keep_slabs);
            let cancel_mon = CancelMonitor::spawn(reader, None, None);
            let resp = match resolve_host(&state, &host_name) {
                Ok(host) => {
                    let flag = if unique { " --unique" } else { "" };
                    let cmd = format!("{} fs write {}{flag} --len {}", helper_file(crate::link::ID), shell_word(&path), bytes.len());
                    let total_budget = Duration::from_secs(120 + bytes.len() as u64 / 20_000);
                    let within = total_budget.saturating_sub(request_started.elapsed()).max(Duration::from_secs(15));
                    // Set ONLY by `SshSessionPool::put_stream_tracked` after `tx.eof().await`
                    // succeeds on the SSH channel (never when the local feeder finishes queueing).
                    let eof_sent = Arc::new(AtomicBool::new(false));
                    let eof_sent_c = eof_sent.clone();
                    let work = async {
                        let prep_started = Instant::now();
                        ensure_helper_with_timeout(&state, &host, within.min(HELPER_PREP_TIMEOUT)).await?;
                        if peer_is_gone(&write_half) {
                            return Err("client disconnected".to_owned());
                        }
                        let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
                        let feeder = tokio::spawn(async move {
                            let _permits = live_permits;
                            for chunk in bytes.chunks(64 * 1024) {
                                if tx.send(chunk.to_vec()).await.is_err() {
                                    return;
                                }
                            }
                        });
                        let rem = within.saturating_sub(prep_started.elapsed()).max(Duration::from_secs(5));
                        let res = tokio::time::timeout(rem, host.pool.put_stream_tracked(&host.target, host.askpass.clone(), &cmd, rx, Some(eof_sent_c)))
                            .await
                            .map_err(|_| "timed out writing file to host".to_owned())?;
                        feeder.abort();
                        match res {
                            Ok(ran) => parse_helper_json(&ran.output, &host.label).map(|mut v| {
                                v["host"] = json!(host.label);
                                v
                            }),
                            Err(e) => Err(e.to_string()),
                        }
                    };
                    tokio::pin!(work);
                    tokio::select! {
                        out = &mut work => match out {
                            Ok(v) => v,
                            Err(e) => json!({ "ok": false, "error": e }),
                        },
                        () = cancel_mon.wait() => {
                            if eof_sent.load(Ordering::Acquire) {
                                // Channel EOF already reached the SSH transport, so the
                                // remote helper may have committed the file. Drain to the
                                // operation deadline and NEVER report a false cancelled:true.
                                let drain_rem = within.saturating_sub(request_started.elapsed()).max(Duration::from_secs(5));
                                match tokio::time::timeout(drain_rem, &mut work).await {
                                    Ok(Ok(v)) => v,
                                    Ok(Err(e)) => json!({ "ok": false, "error": e }),
                                    Err(_) => json!({
                                        "ok": false,
                                        "error": "transfer outcome unknown after upload reached host; check destination before retrying",
                                    }),
                                }
                            } else {
                                json!({ "ok": false, "cancelled": true, "error": "cancelled" })
                            }
                        }
                    }
                }
                Err(e) => {
                    drop(live_permits);
                    json!({ "ok": false, "error": e })
                }
            };
            let _ = write_line(&mut write_half, &resp).await;
        }
        "read_to_cache" => {
            let host_name = req.host;
            let path = req.path;
            let max_bytes = req.max_bytes;
            drop(line_permits);
            let cancel_mon = CancelMonitor::spawn(reader, None, None);
            let resp = tokio::select! {
                out = tokio::time::timeout(READ_CACHE_TOTAL_TIMEOUT, handle_read_to_cache(&state, &host_name, &path, max_bytes, &write_half)) => match out {
                    Ok(v) => v,
                    Err(_) => json!({ "ok": false, "error": "timed out reading file from host" }),
                },
                () = cancel_mon.wait() => json!({ "ok": false, "cancelled": true, "error": "cancelled" }),
            };
            let _ = write_line(&mut write_half, &resp).await;
        }
        "download" => {
            let host_name = req.host;
            let remote_path = req.remote_path;
            let local_dest_dir = PathBuf::from(req.local_dest_dir);
            let local_name = req.local_name;
            let is_dir = req.is_dir;
            drop(line_permits);
            handle_download(&state, &host_name, &remote_path, local_dest_dir, local_name, is_dir, reader, &mut write_half).await;
        }
        "upload" => {
            let host_name = req.host;
            let local_path = PathBuf::from(req.local_path);
            let remote_dest_dir = req.remote_dest_dir;
            let remote_name = req.remote_name;
            let is_dir = req.is_dir;
            let default_mode = req.default_mode;
            drop(line_permits);
            handle_upload(&state, &host_name, local_path, remote_dest_dir, remote_name, is_dir, default_mode, reader, &mut write_half).await;
        }
        other => {
            let other = other.to_owned();
            drop(line_permits);
            let _ = write_line(&mut write_half, &json!({ "ok": false, "error": format!("unknown op '{other}'") })).await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_one_shot_fs_op(
    state: &Arc<Mutex<HubState>>,
    host_name: &str,
    subcmd: &str,
    timeout: Duration,
    attach_host: bool,
    reply_budget: Option<&Arc<Semaphore>>,
    expect_endpoint: Option<&str>,
    cancel_mon: &CancelMonitor,
    write_half: &mut tokio::net::unix::OwnedWriteHalf,
) {
    let started = Instant::now();
    // Hold the reply-budget permit through `write_line` so large JSON replies
    // remain bounded while serializing and flushing to the Unix socket.
    let mut held_reply_permit: Option<OwnedSemaphorePermit> = None;
    let resp = match resolve_host(state, host_name) {
        Ok(host) => {
            if let Some(sem) = reply_budget {
                let rem = timeout.saturating_sub(started.elapsed());
                let acq = tokio::select! {
                    r = tokio::time::timeout(rem, sem.clone().acquire_owned()) => Some(r),
                    () = cancel_mon.wait() => None,
                };
                match acq {
                    None => {
                        let _ = write_line(write_half, &json!({ "ok": false, "cancelled": true, "error": "cancelled" })).await;
                        return;
                    }
                    Some(Ok(Ok(p))) => held_reply_permit = Some(p),
                    Some(Ok(Err(_))) => {
                        let _ = write_line(write_half, &json!({ "ok": false, "error": "server reply budget closed" })).await;
                        return;
                    }
                    Some(Err(_)) => {
                        let _ = write_line(write_half, &json!({ "ok": false, "error": format!("Timed out waiting for reply budget on “{}”.", host.label) })).await;
                        return;
                    }
                }
            }
            let work = async {
                let rem = timeout.saturating_sub(started.elapsed());
                if rem.is_zero() {
                    return Err(format!("Timed out running remote filesystem operation on “{}”.", host.label));
                }
                if let Some(expected_ep) = expect_endpoint {
                    ensure_helper_with_timeout(state, &host, rem.min(HELPER_PREP_TIMEOUT)).await?;
                    if peer_is_gone(write_half) {
                        return Err("client disconnected".to_owned());
                    }
                    let rem2 = timeout.saturating_sub(started.elapsed());
                    if rem2.is_zero() {
                        return Err(format!("Timed out running remote filesystem operation on “{}”.", host.label));
                    }
                    // Verify the exact live channel's authenticated SSH identity
                    // BEFORE sending `exec` on the wire (establishing the session first
                    // if needed so pooled reconnects never fail spuriously).
                    let val = run_fs_json_expect_verified(&host, subcmd, expected_ep, rem2).await?;
                    return Ok((val, None));
                }
                run_fs_json_op_verified(state, &host, subcmd, rem, write_half).await
            };
            tokio::select! {
                res = work => match res {
                    Ok((mut v, verified)) => {
                        if attach_host {
                            v["host"] = json!(host.label);
                            v["endpoint_id"] = json!(format_verified_identity(&host.target, verified.as_ref()));
                        }
                        v
                    }
                    Err(e) => json!({ "ok": false, "error": e }),
                },
                () = cancel_mon.wait() => json!({ "ok": false, "cancelled": true, "error": "cancelled" }),
            }
        }
        Err(e) => json!({ "ok": false, "error": e }),
    };
    let _ = write_line(write_half, &resp).await;
    drop(held_reply_permit);
}

trait ChunkSource {
    type Chunk: AsRef<[u8]>;
    async fn next_chunk(&mut self) -> Result<Option<Self::Chunk>, String>;
}

impl ChunkSource for olo_rex_client::ssh::ExecStdout {
    type Chunk = Vec<u8>;
    async fn next_chunk(&mut self) -> Result<Option<Self::Chunk>, String> {
        self.next_chunk().await.map(|opt| opt.map(|b| b.to_vec())).map_err(|e| e.to_string())
    }
}

/// Write a stream of chunks from a remote `fs read` into a private staging
/// temp file, enforcing `expected` bytes strictly during streaming (catching
/// file growth and misbehaving caps immediately) and accepting empty / `--max 0`
/// files cleanly when the stream finishes with exit status 0.
async fn stream_into_cache_part<S: ChunkSource>(part: &Path, expected: u64, idle_timeout: Duration, source: &mut S) -> Result<u64, String> {
    let part_owned = part.to_path_buf();
    let std_file =
        tokio::task::spawn_blocking(move || std::fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(&part_owned)).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    let mut file = tokio::fs::File::from_std(std_file);
    let mut total = 0u64;
    loop {
        let chunk = tokio::time::timeout(idle_timeout, source.next_chunk()).await.map_err(|_| "timed out reading file from host".to_owned())??;
        let Some(data) = chunk else {
            break;
        };
        let slice = data.as_ref();
        if total.saturating_add(slice.len() as u64) > expected {
            return Err(format!("remote read exceeded expected size ({expected} bytes)"));
        }
        file.write_all(slice).await.map_err(|e| e.to_string())?;
        total += slice.len() as u64;
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    if total != expected {
        return Err(format!("remote read ended early ({total} of {expected} bytes)"));
    }
    Ok(total)
}

async fn handle_read_to_cache(state: &Arc<Mutex<HubState>>, host_name: &str, path: &str, max_bytes: Option<u64>, write_half: &tokio::net::unix::OwnedWriteHalf) -> Value {
    let host = match resolve_host(state, host_name) {
        Ok(h) => h,
        Err(e) => return json!({ "ok": false, "error": e }),
    };
    let (stat, stat_verified) = match run_fs_json_op_verified(state, &host, &format!("stat {}", shell_word(path)), OP_TIMEOUT, write_half).await {
        Ok(v) => v,
        Err(e) => return json!({ "ok": false, "error": e }),
    };
    let resolved = stat.get("resolved").and_then(Value::as_str).unwrap_or(path).to_owned();
    let Some(entry) = stat.get("entry").filter(|e| !e.is_null()) else {
        return json!({ "ok": false, "error": format!("“{path}” doesn’t exist on {}.", host.label) });
    };
    if entry.get("is_dir").and_then(Value::as_bool).unwrap_or(false) {
        return json!({ "ok": false, "error": format!("“{path}” is a folder.") });
    }
    // Reject non-regular special files (FIFOs, sockets, devices) that have no
    // bounded regular-file length and can block open/read indefinitely.
    let mode = entry.get("mode").and_then(Value::as_u64).unwrap_or(0) as u32;
    let file_type = mode & 0o170000;
    if file_type != 0 && file_type != 0o100000 {
        return json!({ "ok": false, "error": format!("“{path}” is a special file and can’t be cached.") });
    }
    let size = entry.get("size").and_then(Value::as_u64).unwrap_or(0);
    let mtime = entry.get("mtime_secs").and_then(Value::as_i64).unwrap_or(0);
    let mtime_ns = entry.get("mtime_nanos").and_then(Value::as_u64).unwrap_or(0);
    let ctime = entry.get("ctime_secs").and_then(Value::as_i64).unwrap_or(0);
    let ctime_ns = entry.get("ctime_nanos").and_then(Value::as_u64).unwrap_or(0);
    let dev = entry.get("dev").and_then(Value::as_u64).unwrap_or(0);
    let ino = entry.get("ino").and_then(Value::as_u64).unwrap_or(0);
    let ver = CacheVersion { size, mtime, mtime_ns, ctime, ctime_ns, dev, ino };
    let expected = max_bytes.map_or(size, |cap| size.min(cap));

    let cache_root = cache_root_dir();
    let host_identity = format_verified_identity(&host.target, stat_verified.as_ref());
    let local = cache_entry_path_in(&cache_root, &host_identity, &host.label, &resolved, ver, max_bytes);
    let touch_path = local.clone();
    let cache_hit = tokio::task::spawn_blocking(move || try_touch_cached_file(&touch_path, expected)).await.unwrap_or(false);
    if cache_hit {
        return json!({ "ok": true, "local": local.to_string_lossy(), "size": expected, "cached": true });
    }

    let staging_dir = cache_root.join(CACHE_STAGING_DIR);
    if let Err(e) = tokio::task::spawn_blocking(move || ensure_private_dir_sync(&staging_dir)).await.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string())) {
        return json!({ "ok": false, "error": e });
    }

    let mut cmd = format!("{} fs read {}", helper_file(crate::link::ID), shell_word(&resolved));
    if let Some(cap) = max_bytes {
        cmd.push_str(&format!(" --max {cap}"));
    }
    if ino != 0 {
        cmd.push_str(&format!(
            " --expect-dev {dev} --expect-ino {ino} --expect-size {size} --expect-mtime-secs {mtime} --expect-mtime-nanos {mtime_ns} --expect-ctime-secs {ctime} --expect-ctime-nanos {ctime_ns}"
        ));
    }
    let mut stream = match tokio::time::timeout(READ_CACHE_IDLE_TIMEOUT, host.pool.exec_stdout(&host.target, host.askpass.clone(), &cmd)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return json!({ "ok": false, "error": e.to_string() }),
        Err(_) => return json!({ "ok": false, "error": format!("Timed out reading “{path}” from {}.", host.label) }),
    };

    let part = cache_part_in(&cache_root);
    let mut guard = TempPartGuard(Some(part.clone()));
    let total = match stream_into_cache_part(&part, expected, READ_CACHE_IDLE_TIMEOUT, &mut stream).await {
        Ok(n) => n,
        Err(e) => return json!({ "ok": false, "error": e }),
    };
    drop(stream);

    // Re-stat before publishing (checking size, mtime, dev, ino, and ctime) so
    // a same-size, preserved-mtime atomic replacement cannot poison the cache.
    let verify = match run_fs_json_bounded(&host, &format!("stat {}", shell_word(&resolved)), REMOTE_CMD_TIMEOUT).await {
        Ok(v) => v,
        Err(e) => return json!({ "ok": false, "error": e }),
    };
    let same_version = verify.get("entry").filter(|e| !e.is_null()).is_some_and(|e| {
        e.get("size").and_then(Value::as_u64) == Some(size)
            && e.get("mtime_secs").and_then(Value::as_i64) == Some(mtime)
            && e.get("mtime_nanos").and_then(Value::as_u64) == Some(mtime_ns)
            && e.get("ctime_secs").and_then(Value::as_i64).unwrap_or(0) == ctime
            && e.get("ctime_nanos").and_then(Value::as_u64).unwrap_or(0) == ctime_ns
            && e.get("dev").and_then(Value::as_u64).unwrap_or(0) == dev
            && e.get("ino").and_then(Value::as_u64).unwrap_or(0) == ino
    });
    if !same_version {
        return json!({ "ok": false, "error": format!("“{path}” changed on {} while reading.", host.label) });
    }

    // Defer creating the bucket parent until right before rename (retrying once
    // on ENOENT) so concurrent pruning cannot delete an empty in-flight bucket.
    if let Some(parent) = local.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            return json!({ "ok": false, "error": e.to_string() });
        }
        match tokio::fs::rename(&part, &local).await {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if let Err(ce) = tokio::fs::create_dir_all(parent).await {
                    return json!({ "ok": false, "error": ce.to_string() });
                }
                if let Err(re) = tokio::fs::rename(&part, &local).await {
                    return json!({ "ok": false, "error": re.to_string() });
                }
            }
            Err(e) => return json!({ "ok": false, "error": e.to_string() }),
        }
    } else if let Err(e) = tokio::fs::rename(&part, &local).await {
        return json!({ "ok": false, "error": e.to_string() });
    }
    guard.disarm();
    maybe_schedule_cache_maintenance(cache_root);
    json!({ "ok": true, "local": local.to_string_lossy(), "size": total, "cached": false })
}

/// Shared atomic publish step for `handle_download`: checks cancellation via
/// `commit_state`, sets and verifies `dir_mode` via `fchmod` on the staged
/// directory child INSIDE the private `0700` outer wrapper BEFORE renaming out
/// (so final permissions are applied before visibility and any `fchmod` failure
/// leaves nothing published), and cleans up the outer wrapper best-effort.
fn publish_staged_download(stage_wrapper: &Path, staged_entry: &Path, target_local: &Path, dir_mode: Option<u32>, commit_state: &AtomicU8) -> Result<(), TransferError> {
    use std::os::unix::io::{FromRawFd as _, OwnedFd};
    if commit_state.compare_exchange(STATE_RUNNING, STATE_COMMITTED, Ordering::AcqRel, Ordering::Acquire).is_err() {
        let _ = remove_stage_dir(stage_wrapper);
        return Err(TransferError::Cancelled);
    }
    if let Some(mode) = dir_mode {
        let c_staged = std::ffi::CString::new(staged_entry.as_os_str().as_bytes()).map_err(|_| TransferError::Failed("invalid stage path".into()))?;
        // SAFETY: c_staged is valid NUL-terminated; O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC opens only this directory inode.
        let fd = unsafe { libc::open(c_staged.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if fd < 0 {
            let err = io::Error::last_os_error();
            let _ = remove_stage_dir(stage_wrapper);
            return Err(TransferError::Failed(err.to_string()));
        }
        // SAFETY: fd >= 0 is newly opened and exclusively owned here.
        let dir_fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // Apply and verify final directory mode INSIDE the private 0700 wrapper
        // BEFORE publishing via rename_noreplace.
        if unsafe { libc::fchmod(dir_fd.as_raw_fd(), (mode & 0o3777) as libc::mode_t) } != 0 {
            let err = io::Error::last_os_error();
            drop(dir_fd);
            let _ = remove_stage_dir(stage_wrapper);
            return Err(TransferError::Failed(format!("failed to set folder permissions before publish: {err}")));
        }
    }

    match rename_noreplace(staged_entry, target_local) {
        Ok(()) => {
            if let Err(e) = remove_stage_dir(stage_wrapper) {
                tracing::warn!("Rex files_sock failed to clean outer download stage {}: {e}", stage_wrapper.display());
            }
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let _ = remove_stage_dir(stage_wrapper);
            Err(TransferError::Failed(format!("destination “{}” already exists", target_local.display())))
        }
        Err(e) => {
            let _ = remove_stage_dir(stage_wrapper);
            Err(TransferError::Failed(e.to_string()))
        }
    }
}

/// Drive progress notifications for a streaming transfer with ZERO idle
/// wakeups (waking only when `progress_notify` fires: throttled to at most one
/// JSON progress event per 120 ms when bytes advance, and at most once per 5 s
/// as an indexing keepalive while `Phase::Indexing` reports activity).
async fn pump_transfer_progress<T>(
    work: &mut tokio::task::JoinHandle<Result<T, TransferError>>,
    counter: Arc<AtomicU64>,
    progress_notify: Arc<tokio::sync::Notify>,
    stop: &Stop,
    commit_state: &AtomicU8,
    write_half: &mut tokio::net::unix::OwnedWriteHalf,
) -> Result<Result<T, TransferError>, tokio::task::JoinError> {
    const MIN_PROGRESS_GAP: Duration = Duration::from_millis(120);
    const INDEX_HEARTBEAT_GAP: Duration = Duration::from_secs(5);
    let mut last_bytes = 0u64;
    let mut last_sent = Instant::now().checked_sub(MIN_PROGRESS_GAP).unwrap_or_else(Instant::now);
    loop {
        let notified = progress_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let cur = counter.load(Ordering::Relaxed);
        if cur != last_bytes {
            let elapsed = last_sent.elapsed();
            if elapsed < MIN_PROGRESS_GAP {
                let wait = tokio::time::sleep(MIN_PROGRESS_GAP - elapsed);
                tokio::select! {
                    res = &mut *work => return res,
                    () = wait => {}
                }
            }
            let latest = counter.load(Ordering::Relaxed);
            if latest != last_bytes {
                last_bytes = latest;
                last_sent = Instant::now();
                if write_line(write_half, &json!({ "event": "progress", "done_bytes": latest })).await.is_err() {
                    let _ = commit_state.compare_exchange(STATE_RUNNING, STATE_CANCELLED, Ordering::AcqRel, Ordering::Acquire);
                    stop.cancel();
                }
            }
            continue;
        }
        tokio::select! {
            res = &mut *work => return res,
            () = &mut notified => {
                // `Phase::Indexing` notifies progress even before payload bytes
                // move; emit a throttled keepalive progress frame so Tracker's
                // idle timer does not trip on huge folder scans.
                let latest = counter.load(Ordering::Relaxed);
                if latest == last_bytes && last_sent.elapsed() >= INDEX_HEARTBEAT_GAP {
                    last_sent = Instant::now();
                    if write_line(write_half, &json!({ "event": "progress", "done_bytes": latest })).await.is_err() {
                        let _ = commit_state.compare_exchange(STATE_RUNNING, STATE_CANCELLED, Ordering::AcqRel, Ordering::Acquire);
                        stop.cancel();
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_download(
    state: &Arc<Mutex<HubState>>,
    host_name: &str,
    remote_path: &str,
    local_dest_dir: PathBuf,
    local_name: String,
    is_dir: bool,
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    write_half: &mut tokio::net::unix::OwnedWriteHalf,
) {
    if let Err(e) = validate_single_name(&local_name) {
        let _ = write_line(write_half, &json!({ "ok": false, "error": e })).await;
        return;
    }
    let host = match resolve_host(state, host_name) {
        Ok(h) => h,
        Err(e) => {
            let _ = write_line(write_half, &json!({ "ok": false, "error": e })).await;
            return;
        }
    };
    let stop = Stop::default().with_idle_timeout(TRANSFER_IDLE_TIMEOUT);
    let commit_state = Arc::new(AtomicU8::new(STATE_RUNNING));
    // Spawn ONE control monitor for the entire request so split cancel frames
    // across prep -> stat -> transfer are never dropped.
    let cancel_mon = CancelMonitor::spawn(reader, Some(stop.clone()), Some(commit_state.clone()));

    let prep_res = tokio::select! {
        res = ensure_helper(state, &host) => Some(res),
        () = cancel_mon.wait() => None,
    };
    match prep_res {
        None => {
            let _ = write_line(write_half, &json!({ "ok": false, "cancelled": true, "error": "cancelled" })).await;
            return;
        }
        Some(Err(e)) => {
            let _ = write_line(write_half, &json!({ "ok": false, "error": e })).await;
            return;
        }
        Some(Ok(())) => {}
    }

    let (source_dir_mode, stat_verified) = if is_dir {
        let stat_cmd = format!("stat {}", shell_word(remote_path));
        let stat_res = tokio::select! {
            res = run_fs_json_bounded_verified(&host, &stat_cmd, REMOTE_CMD_TIMEOUT) => Some(res),
            () = cancel_mon.wait() => None,
        };
        match stat_res {
            None => {
                let _ = write_line(write_half, &json!({ "ok": false, "cancelled": true, "error": "cancelled" })).await;
                return;
            }
            Some(Ok((v, ver))) => {
                if v.get("exists").and_then(Value::as_bool) != Some(true) {
                    let _ = write_line(write_half, &json!({ "ok": false, "error": format!("“{remote_path}” doesn’t exist on {}.", host.label) })).await;
                    return;
                }
                let Some(mode) = v.get("entry").and_then(|e| e.get("mode")).and_then(Value::as_u64).map(|m| (m as u32) & 0o3777) else {
                    let _ = write_line(write_half, &json!({ "ok": false, "error": format!("remote stat for “{remote_path}” omitted folder permissions") })).await;
                    return;
                };
                (Some(mode), ver)
            }
            Some(Err(e)) => {
                let _ = write_line(write_half, &json!({ "ok": false, "error": e })).await;
                return;
            }
        }
    } else {
        (None, host.pool.verified(&host.target))
    };
    let endpoint_id = format_verified_identity(&host.target, stat_verified.as_ref());

    let rx_counter = stop.received.clone();
    let progress_notify = stop.progress_notify();
    let target_local = local_dest_dir.join(&local_name);
    let pool = host.pool.clone();
    let ssh_target = host.target.clone();
    let askpass = host.askpass.clone();
    let remote_path_owned = remote_path.to_owned();
    let stop_work = stop.clone();
    let commit_work = commit_state.clone();

    let mut work = tokio::spawn(async move {
        if is_dir {
            let dest_parent = local_dest_dir.clone();
            // Stage the downloaded folder as a child inside a private 0700 outer
            // wrapper so `fchmod` sets final permissions before rename out.
            let (stage_outer, stage_inner) = tokio::task::spawn_blocking(move || {
                let outer = create_private_stage_dir(&dest_parent, ".olo-rx-")?;
                let inner = outer.join("dir");
                let mut b = std::fs::DirBuilder::new();
                b.mode(0o700);
                if let Err(e) = b.create(&inner) {
                    let _ = remove_stage_dir(&outer);
                    return Err(e);
                }
                Ok::<_, io::Error>((outer, inner))
            })
            .await
            .map_err(|e| TransferError::Failed(e.to_string()))?
            .map_err(|e| TransferError::Failed(e.to_string()))?;
            let folder = HostFolder { path: remote_path_owned, ..Default::default() };
            let res = receive_from(&pool, &ssh_target, askpass, crate::link::ID, &folder, stage_inner.clone(), None, stop_work.clone()).await;
            let report = match res {
                Ok(r) => r,
                Err(e) => {
                    remove_stage_dir_async(stage_outer).await;
                    return if stop_work.is_stopped() { Err(TransferError::Cancelled) } else { Err(TransferError::Failed(e.to_string())) };
                }
            };
            if !report.sender_confirmed {
                remove_stage_dir_async(stage_outer).await;
                return Err(TransferError::Failed("host helper didn’t confirm a complete transfer; update Olo’s helper on the host".into()));
            }
            if let Some(err) = report.errors.first() {
                remove_stage_dir_async(stage_outer).await;
                return Err(TransferError::Failed(format!("{}: {}", err.path, err.reason)));
            }
            if !report.skipped.is_empty() {
                remove_stage_dir_async(stage_outer).await;
                return Err(TransferError::Failed(describe_skipped(&report.skipped)));
            }
            let target_pub = target_local.clone();
            tokio::task::spawn_blocking(move || publish_staged_download(&stage_outer, &stage_inner, &target_pub, source_dir_mode, &commit_work))
                .await
                .map_err(|e| TransferError::Failed(e.to_string()))??;
            Ok((target_local, report.bytes_received))
        } else {
            let Some((remote_parent, remote_leaf)) = parent_and_leaf(&remote_path_owned) else {
                return Err(TransferError::Failed(format!("invalid remote path: {remote_path_owned}")));
            };
            let dest_parent = local_dest_dir.clone();
            let stage = tokio::task::spawn_blocking(move || create_private_stage_dir(&dest_parent, ".olo-rx-"))
                .await
                .map_err(|e| TransferError::Failed(e.to_string()))?
                .map_err(|e| TransferError::Failed(e.to_string()))?;
            let res = receive_paths(&pool, &ssh_target, askpass, crate::link::ID, &remote_parent, stage.clone(), std::slice::from_ref(&remote_leaf), stop_work.clone()).await;
            let report = match res {
                Ok(r) => r,
                Err(e) => {
                    remove_stage_dir_async(stage).await;
                    return if stop_work.is_stopped() { Err(TransferError::Cancelled) } else { Err(TransferError::Failed(e.to_string())) };
                }
            };
            if !report.sender_confirmed {
                remove_stage_dir_async(stage).await;
                return Err(TransferError::Failed("host helper didn’t confirm a complete transfer; update Olo’s helper on the host".into()));
            }
            if let Some(err) = report.errors.first() {
                remove_stage_dir_async(stage).await;
                return Err(TransferError::Failed(format!("{}: {}", err.path, err.reason)));
            }
            if !report.skipped.is_empty() {
                remove_stage_dir_async(stage).await;
                return Err(TransferError::Failed(describe_skipped(&report.skipped)));
            }
            let staged = stage.join(&remote_leaf);
            let target_pub = target_local.clone();
            let stage_pub = stage.clone();
            tokio::task::spawn_blocking(move || publish_staged_download(&stage_pub, &staged, &target_pub, None, &commit_work)).await.map_err(|e| TransferError::Failed(e.to_string()))??;
            Ok((target_local, report.bytes_received))
        }
    });

    let final_res = pump_transfer_progress(&mut work, rx_counter, progress_notify, &stop, &commit_state, write_half).await;
    drop(cancel_mon);

    // Report strictly what actually happened: if publication won before Stop
    // arrived, return ok:true with the committed path instead of cancelled:true.
    let resp = match final_res {
        Ok(Ok((path, bytes))) => json!({ "ok": true, "complete": true, "endpoint_id": endpoint_id, "path": path.to_string_lossy(), "bytes": bytes }),
        Ok(Err(TransferError::Cancelled)) => json!({ "ok": false, "cancelled": true, "error": "cancelled" }),
        Ok(Err(TransferError::Failed(e))) => json!({ "ok": false, "error": e }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    };
    let _ = write_line(write_half, &resp).await;
}

#[allow(clippy::too_many_arguments)]
async fn handle_upload(
    state: &Arc<Mutex<HubState>>,
    host_name: &str,
    local_path: PathBuf,
    remote_dest_dir: String,
    remote_name: String,
    is_dir: bool,
    default_mode: bool,
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    write_half: &mut tokio::net::unix::OwnedWriteHalf,
) {
    if let Err(e) = validate_single_name(&remote_name) {
        let _ = write_line(write_half, &json!({ "ok": false, "error": e })).await;
        return;
    }
    // Validate `local_path` locally BEFORE touching the remote host or creating
    // a remote staging directory so bad local inputs never leak empty stages.
    let validated_local = if is_dir {
        match std::fs::symlink_metadata(&local_path) {
            Ok(m) if m.is_dir() => Ok((None::<(PathBuf, String)>, Some(m.permissions().mode() & 0o3777))),
            Ok(_) => Err(format!("“{}” is not a directory", local_path.display())),
            Err(e) => Err(format!("cannot read “{}”: {e}", local_path.display())),
        }
    } else {
        let parent = local_path.parent().map(Path::to_path_buf);
        let leaf = local_path.file_name().and_then(|n| n.to_str()).map(str::to_owned);
        match (parent, leaf) {
            (Some(p), Some(l)) => match std::fs::symlink_metadata(&local_path) {
                Ok(_) => Ok((Some((p, l)), None)),
                Err(e) => Err(format!("cannot read “{}”: {e}", local_path.display())),
            },
            (None, _) => Err("invalid local path".to_owned()),
            (_, None) => Err("local filename is not valid UTF-8".to_owned()),
        }
    };
    let (local_parent_leaf, src_dir_mode) = match validated_local {
        Ok(v) => v,
        Err(e) => {
            let _ = write_line(write_half, &json!({ "ok": false, "error": e })).await;
            return;
        }
    };

    let host = match resolve_host(state, host_name) {
        Ok(h) => h,
        Err(e) => {
            let _ = write_line(write_half, &json!({ "ok": false, "error": e })).await;
            return;
        }
    };
    let stop = Stop::default().with_idle_timeout(TRANSFER_IDLE_TIMEOUT);
    let commit_state = Arc::new(AtomicU8::new(STATE_RUNNING));
    // Spawn ONE control monitor for the entire request across prep -> stat -> mkdir -> transfer!
    let cancel_mon = CancelMonitor::spawn(reader, Some(stop.clone()), Some(commit_state.clone()));

    let prep_res = tokio::select! {
        res = ensure_helper(state, &host) => Some(res),
        () = cancel_mon.wait() => None,
    };
    match prep_res {
        None => {
            let _ = write_line(write_half, &json!({ "ok": false, "cancelled": true, "error": "cancelled" })).await;
            return;
        }
        Some(Err(e)) => {
            let _ = write_line(write_half, &json!({ "ok": false, "error": e })).await;
            return;
        }
        Some(Ok(())) => {}
    }

    let remote_target = if remote_dest_dir == "/" { format!("/{remote_name}") } else { format!("{}/{remote_name}", remote_dest_dir.trim_end_matches('/')) };
    // Resolve `~` / relative paths on the host via `fs stat` (cancellable!) so
    // private staging, sync, atomic rename, and cleanup use the exact same absolute directory.
    let stat_cmd = format!("stat {}", shell_word(&remote_target));
    let stat_res = tokio::select! {
        res = run_fs_json_bounded_verified(&host, &stat_cmd, REMOTE_CMD_TIMEOUT) => Some(res),
        () = cancel_mon.wait() => None,
    };
    let (stat_val, stat_verified) = match stat_res {
        None => {
            let _ = write_line(write_half, &json!({ "ok": false, "cancelled": true, "error": "cancelled" })).await;
            return;
        }
        Some(Ok(v)) => v,
        Some(Err(e)) => {
            let _ = write_line(write_half, &json!({ "ok": false, "error": e })).await;
            return;
        }
    };
    let endpoint_id = format_verified_identity(&host.target, stat_verified.as_ref());
    let Some(resolved_target) = stat_val.get("resolved").and_then(Value::as_str).map(str::to_owned) else {
        let _ = write_line(write_half, &json!({ "ok": false, "error": "remote stat didn’t return a resolved path" })).await;
        return;
    };
    if !resolved_target.starts_with('/') {
        let _ = write_line(write_half, &json!({ "ok": false, "error": format!("remote stat returned non-absolute path: {resolved_target}") })).await;
        return;
    }
    let Some((resolved_dir, _)) = parent_and_leaf(&resolved_target) else {
        let _ = write_line(write_half, &json!({ "ok": false, "error": format!("invalid resolved remote path: {resolved_target}") })).await;
        return;
    };
    if cancel_mon.is_cancelled() {
        let _ = write_line(write_half, &json!({ "ok": false, "cancelled": true, "error": "cancelled" })).await;
        return;
    }

    // Construct the private `0700` outer stage path locally and require the
    // helper's `mkdir` reply to match it verbatim so a misrouted reply can
    // never redirect subsequent sync or recursive `fs remove --stage` cleanup.
    let stage_req =
        if resolved_dir == "/" { format!("/.olo-tx-{}", uuid::Uuid::new_v4().simple()) } else { format!("{}/.olo-tx-{}", resolved_dir.trim_end_matches('/'), uuid::Uuid::new_v4().simple()) };
    let mk_cmd = format!("mkdir {} --private", shell_word(&stage_req));
    let mk_res = tokio::select! {
        res = run_fs_json_bounded(&host, &mk_cmd, REMOTE_CMD_TIMEOUT) => Some(res),
        () = cancel_mon.wait() => None,
    };
    match mk_res {
        None => {
            // Best-effort cleanup in case mkdir already ran on the host before cancel.
            let _ = run_fs_json_bounded(&host, &format!("remove {} --stage", shell_word(&stage_req)), Duration::from_secs(10)).await;
            let _ = write_line(write_half, &json!({ "ok": false, "cancelled": true, "error": "cancelled" })).await;
            return;
        }
        Some(Ok(mk_val)) => {
            let mk_path = mk_val.get("path").and_then(Value::as_str).unwrap_or("");
            if mk_path != stage_req {
                let _ = write_line(write_half, &json!({ "ok": false, "error": format!("remote stage path mismatch (expected {stage_req}, got {mk_path})") })).await;
                return;
            }
        }
        Some(Err(e)) => {
            let _ = write_line(write_half, &json!({ "ok": false, "error": e })).await;
            return;
        }
    }

    // Report logical uncompressed payload bytes (matching Tracker's total_bytes).
    let tx_counter = stop.logical_bytes.clone();
    let progress_notify = stop.progress_notify();
    let pool = host.pool.clone();
    let ssh_target = host.target.clone();
    let askpass = host.askpass.clone();
    let host_label = host.label.clone();
    let stop_work = stop.clone();
    let commit_work = commit_state.clone();

    let mut work = tokio::spawn(async move {
        let helper_exec = |subcmd: String| {
            let pool = pool.clone();
            let ssh_target = ssh_target.clone();
            let askpass = askpass.clone();
            let host_label = host_label.clone();
            async move {
                let cmd = format!("{} fs {subcmd}", helper_file(crate::link::ID));
                let ran = tokio::time::timeout(REMOTE_CMD_TIMEOUT, pool.exec_bounded(&ssh_target, askpass, &cmd, MAX_REPLY_LINE_BYTES, MAX_REPLY_LINE_BYTES, &mut |_| {}))
                    .await
                    .map_err(|_| format!("Timed out running remote command on “{host_label}”."))?
                    .map_err(|e| e.to_string())?;
                parse_helper_json(&ran.output, &host_label)
            }
        };
        let cleanup_stage = |stage: &str| {
            let stage_owned = stage.to_owned();
            let fut = helper_exec(format!("remove {} --stage", shell_word(stage)));
            async move {
                if let Err(e) = fut.await {
                    tracing::warn!("Rex files_sock failed to clean remote stage {stage_owned}: {e}");
                }
            }
        };

        if stop_work.is_stopped() {
            cleanup_stage(&stage_req).await;
            return Err(TransferError::Cancelled);
        }

        if is_dir {
            let src_mode = src_dir_mode.unwrap_or(0o755);
            // Stage directory as a child inside the private `0700` outer wrapper
            // so `fs rename --mode` sets final mode inside `0700` before publish.
            let stage_inner = format!("{stage_req}/dir");
            if let Err(e) = helper_exec(format!("mkdir {} --private", shell_word(&stage_inner))).await {
                cleanup_stage(&stage_req).await;
                return Err(TransferError::Failed(e));
            }
            let folder = HostFolder { path: stage_inner.clone(), no_cache: true, ..Default::default() };
            let source = Box::new(LazyFolderSource::new(local_path));
            let report = match send_to(&pool, &ssh_target, askpass.clone(), crate::link::ID, source, &folder, stop_work.clone()).await {
                Ok(r) => r,
                Err(e) => {
                    cleanup_stage(&stage_req).await;
                    return if stop_work.is_stopped() { Err(TransferError::Cancelled) } else { Err(TransferError::Failed(e.to_string())) };
                }
            };
            if let Some(err) = report.errors.first() {
                cleanup_stage(&stage_req).await;
                return Err(TransferError::Failed(format!("{}: {}", err.path, err.reason)));
            }
            if !report.skipped.is_empty() {
                cleanup_stage(&stage_req).await;
                return Err(TransferError::Failed(describe_skipped(&report.skipped)));
            }
            if commit_work.compare_exchange(STATE_RUNNING, STATE_COMMITTED, Ordering::AcqRel, Ordering::Acquire).is_err() {
                cleanup_stage(&stage_req).await;
                return Err(TransferError::Cancelled);
            }
            let ren_res = helper_exec(format!("rename {} {} --unique --mode {:04o}", shell_word(&stage_inner), shell_word(&resolved_target), src_mode)).await;
            cleanup_stage(&stage_req).await;
            let ren_val = ren_res.map_err(TransferError::Failed)?;
            let final_remote = ren_val.get("path").and_then(Value::as_str).unwrap_or(&resolved_target).to_owned();
            Ok((final_remote, report.bytes_sent))
        } else {
            let (local_parent, local_leaf) = local_parent_leaf.expect("validated above");
            let res = send_paths(&pool, &ssh_target, askpass.clone(), crate::link::ID, &stage_req, local_parent, vec![local_leaf.clone()], stop_work.clone()).await;
            let report = match res {
                Ok(r) => r,
                Err(e) => {
                    cleanup_stage(&stage_req).await;
                    return if stop_work.is_stopped() { Err(TransferError::Cancelled) } else { Err(TransferError::Failed(e.to_string())) };
                }
            };
            if let Some(err) = report.errors.first() {
                cleanup_stage(&stage_req).await;
                return Err(TransferError::Failed(format!("{}: {}", err.path, err.reason)));
            }
            if !report.skipped.is_empty() {
                cleanup_stage(&stage_req).await;
                return Err(TransferError::Failed(describe_skipped(&report.skipped)));
            }
            if commit_work.compare_exchange(STATE_RUNNING, STATE_COMMITTED, Ordering::AcqRel, Ordering::Acquire).is_err() {
                cleanup_stage(&stage_req).await;
                return Err(TransferError::Cancelled);
            }
            let staged_file = format!("{stage_req}/{local_leaf}");
            let mode_flag = if default_mode { " --default-mode" } else { "" };
            let ren_res = helper_exec(format!("rename {} {} --unique{mode_flag}", shell_word(&staged_file), shell_word(&resolved_target))).await;
            cleanup_stage(&stage_req).await;
            let ren_val = ren_res.map_err(TransferError::Failed)?;
            let final_remote = ren_val.get("path").and_then(Value::as_str).unwrap_or(&resolved_target).to_owned();
            Ok((final_remote, report.bytes_sent))
        }
    });

    let final_res = pump_transfer_progress(&mut work, tx_counter, progress_notify, &stop, &commit_state, write_half).await;
    drop(cancel_mon);

    let resp = match final_res {
        Ok(Ok((path, bytes))) => json!({ "ok": true, "complete": true, "endpoint_id": endpoint_id, "path": path, "host": host.label, "bytes": bytes }),
        Ok(Err(TransferError::Cancelled)) => json!({ "ok": false, "cancelled": true, "error": "cancelled" }),
        Ok(Err(TransferError::Failed(e))) => json!({ "ok": false, "error": e }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    };
    let _ = write_line(write_half, &resp).await;
}

/// Start listening on the Rex files Unix socket for Tracker (`rex://HOST/PATH`).
pub fn spawn(rt: &tokio::runtime::Handle, state: Arc<Mutex<HubState>>) {
    let sock = olo_kit_base::tracker::rex_files_socket();
    rt.spawn(async move {
        if let Some(parent) = sock.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::remove_file(&sock);
        let listener = match UnixListener::bind(&sock) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("Rex files socket {} bind failed: {e}", sock.display());
                return;
            }
        };
        // Fail closed: never expose the socket if mode 0600 cannot be set.
        if let Err(e) = std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600)) {
            tracing::error!("Rex files socket {} chmod 0600 failed: {e}; aborting listener", sock.display());
            let _ = std::fs::remove_file(&sock);
            return;
        }
        tracing::info!("Rex files socket listening at {}", sock.display());

        // Bind and expose the socket FIRST so Tracker never sees "Rex isn't
        // running" while startup cache maintenance runs in the background.
        let cache_root = cache_root_dir();
        tokio::task::spawn_blocking(move || maintain_cache(&cache_root));

        let limiter = Arc::new(Semaphore::new(MAX_CONCURRENT_CLIENTS));
        let inline_budget = Arc::new(Semaphore::new(MAX_INFLIGHT_INLINE_BYTES / INLINE_SLAB_BYTES));
        let large_reader_gate = Arc::new(tokio::sync::Mutex::new(()));
        let reply_budget = Arc::new(Semaphore::new(MAX_CONCURRENT_LARGE_REPLIES));
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    // Shed excess connections promptly rather than leaving
                    // Tracker stalled in the kernel listen backlog with no timer.
                    let Ok(permit) = limiter.clone().try_acquire_owned() else {
                        tokio::spawn(async move {
                            let (_, mut wh) = stream.into_split();
                            let _ = write_line(&mut wh, &json!({ "ok": false, "error": "Rex files server is busy; try again" })).await;
                        });
                        continue;
                    };
                    let st = state.clone();
                    let budget = inline_budget.clone();
                    let gate = large_reader_gate.clone();
                    let rbudget = reply_budget.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        handle_client(stream, st, budget, gate, rbudget).await;
                    });
                }
                Err(e) => {
                    tracing::warn!("Rex files socket accept error: {e}");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_ready_requires_current_build() {
        assert!(!helper_is_current(&HostHelper::Used, None));
        assert!(!helper_is_current(&HostHelper::Used, Some(false)));
        assert!(!helper_is_current(&HostHelper::Ready, None));
        assert!(!helper_is_current(&HostHelper::Ready, Some(false)));
        assert!(!helper_is_current(&HostHelper::None, Some(true)));
        assert!(helper_is_current(&HostHelper::Used, Some(true)));
        assert!(helper_is_current(&HostHelper::Ready, Some(true)));
    }

    #[test]
    fn helper_policy_respects_global_and_per_host_settings() {
        assert_eq!(helper_policy(false, None), HelperPolicy::DisabledGlobally);
        assert_eq!(helper_policy(false, Some("yes")), HelperPolicy::DisabledGlobally);
        assert_eq!(helper_policy(false, Some("off")), HelperPolicy::DisabledGlobally);
        assert_eq!(helper_policy(true, Some("off")), HelperPolicy::DisabledForHost);
        assert_eq!(helper_policy(true, None), HelperPolicy::Allowed);
        assert_eq!(helper_policy(true, Some("yes")), HelperPolicy::Allowed);
        assert!(!helper_policy_allows(false, None));
        assert!(!helper_policy_allows(true, Some("off")));
        assert!(helper_policy_allows(true, None));
    }

    #[test]
    fn write_file_requires_string_base64_payload() {
        assert!(decode_write_payload(None).is_err());
        assert!(decode_write_payload(Some("!!!not-base64!!!")).is_err());
        assert_eq!(decode_write_payload(Some("")).unwrap(), Vec::<u8>::new());
        assert_eq!(decode_write_payload(Some("aGVsbG8=")).unwrap(), b"hello");
        // Non-string JSON types for `base64` must be rejected during streaming
        // deserialization (before constructing any Value tree).
        assert!(serde_json::from_str::<ClientRequest>(r#"{"op":"write_file","base64":123}"#).is_err());
        assert!(serde_json::from_str::<ClientRequest>(r#"{"op":"write_file","base64":[1,2,3]}"#).is_err());
        let null_req: ClientRequest = serde_json::from_str(r#"{"op":"write_file","base64":null}"#).unwrap();
        assert!(decode_write_payload(null_req.base64.as_deref()).is_err());
    }

    #[test]
    fn cache_paths_isolate_endpoints_versions_inodes_caps_and_staging() {
        let root = PathBuf::from("/var/tmp/olo-files-sock-cache-key-test");
        let long_name = format!("{}.png", "a".repeat(251));
        assert_eq!(long_name.len(), 255);
        let v1 = CacheVersion { size: 100, mtime: 1700, mtime_ns: 5, ctime: 1700, ctime_ns: 5, dev: 1, ino: 42 };
        let v_newer = CacheVersion { mtime: 1701, ..v1 };
        let v_diff_ino = CacheVersion { ino: 43, ..v1 };
        let full = cache_entry_path_in(&root, "v2|au=alice|h=a.example|k=key1", "dev-box", &format!("/home/dev/{long_name}"), v1, None);
        let cap0 = cache_entry_path_in(&root, "v2|au=alice|h=a.example|k=key1", "dev-box", &format!("/home/dev/{long_name}"), v1, Some(0));
        let cap64 = cache_entry_path_in(&root, "v2|au=alice|h=a.example|k=key1", "dev-box", &format!("/home/dev/{long_name}"), v1, Some(64));
        let newer = cache_entry_path_in(&root, "v2|au=alice|h=a.example|k=key1", "dev-box", &format!("/home/dev/{long_name}"), v_newer, None);
        let diff_ino = cache_entry_path_in(&root, "v2|au=alice|h=a.example|k=key1", "dev-box", &format!("/home/dev/{long_name}"), v_diff_ino, None);
        let other_user = cache_entry_path_in(&root, "v2|au=bob|h=a.example|k=key1", "dev-box", &format!("/home/dev/{long_name}"), v1, None);
        let other_host = cache_entry_path_in(&root, "v2|au=alice|h=b.example|k=key2", "dev-box", &format!("/home/dev/{long_name}"), v1, None);
        assert_ne!(full, cap0, "None and Some(0) caps must never collide");
        assert_ne!(full, cap64);
        assert_ne!(full, newer);
        assert_ne!(full, diff_ino, "same size & mtime with different inode must never share a cache entry");
        assert_ne!(full, other_user, "different authenticated SSH account must never share a cache entry");
        assert_ne!(full, other_host, "same label pointing at another endpoint must never share a cache entry");

        let part = cache_part_in(&root);
        assert_eq!(part.parent().unwrap(), root.join(CACHE_STAGING_DIR));
        assert!(full.starts_with(root.join(CACHE_FILES_DIR)));
    }

    #[test]
    fn sweep_staging_never_touches_remote_dot_olo_part_files_or_follows_symlinks() {
        let dir = PathBuf::from(format!("/var/tmp/olo-files-sock-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let staging = dir.join(CACHE_STAGING_DIR);
        let v = CacheVersion { size: 12, mtime: 100, mtime_ns: 0, ctime: 100, ctime_ns: 0, dev: 1, ino: 1 };
        let published = cache_entry_path_in(&dir, "id1", "dev-box", "/home/dev/.olo-part-notes", v, None);
        let outside = dir.join("outside-target");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(published.parent().unwrap()).unwrap();
        std::fs::write(&published, b"legit notes!").unwrap();
        std::fs::write(&outside, b"must stay").unwrap();
        let stale_part = staging.join("part-deadbeef");
        std::fs::write(&stale_part, b"partial").unwrap();
        std::os::unix::fs::symlink(&outside, staging.join("part-link")).unwrap();

        sweep_stale_cache_parts(&dir, Duration::ZERO);
        assert!(!stale_part.exists(), "stale staging part must be removed");
        assert!(published.exists(), "published remote file named .olo-part-notes must never be deleted");
        assert!(outside.exists(), "symlink target outside cache must never be deleted");
        assert!(std::fs::symlink_metadata(staging.join("part-link")).is_ok(), "symlinks in staging must not be followed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_cache_versions_enforces_size_and_entry_bounds_and_protects_fresh_oversized() {
        let dir = PathBuf::from(format!("/var/tmp/olo-files-sock-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for i in 0..5u64 {
            let v = CacheVersion { size: 1024, mtime: 1000 + i as i64, mtime_ns: 0, ctime: 1000, ctime_ns: 0, dev: 1, ino: i + 1 };
            let p = cache_entry_path_in(&dir, "id1", "dev-box", "/home/dev/big.bin", v, None);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, vec![b'x'; 1024]).unwrap();
            let old = SystemTime::now() - Duration::from_secs(600 + (5 - i) * 60);
            let _ = std::fs::File::options().write(true).open(&p).unwrap().set_modified(old);
        }
        let grace1 = prune_cache_versions(&dir, 2048, 4096, 2, Duration::from_secs(60));
        assert!(grace1.is_none(), "after pruning to budget no grace follow-up is needed");
        let remaining = std::fs::read_dir(dir.join(CACHE_FILES_DIR).join("dev-box")).unwrap().count();
        assert_eq!(remaining, 2, "must prune oldest versions down to budget");

        // A freshly cached single file larger than `hard_max_bytes` MUST NOT
        // immediately self-evict within CACHE_HANDOFF_GRACE, AND prune must
        // return `Some(wait)` so a deferred follow-up pass reclaims it later!
        let v_big = CacheVersion { size: 8192, mtime: 2000, mtime_ns: 0, ctime: 2000, ctime_ns: 0, dev: 1, ino: 999 };
        let p_big = cache_entry_path_in(&dir, "id1", "dev-box", "/home/dev/movie.mp4", v_big, None);
        std::fs::create_dir_all(p_big.parent().unwrap()).unwrap();
        std::fs::write(&p_big, vec![b'm'; 8192]).unwrap();
        let grace2 = prune_cache_versions(&dir, 2048, 4096, 2, Duration::from_secs(60));
        assert!(p_big.exists(), "freshly published oversized file must not self-evict within handoff grace");
        assert!(grace2.is_some(), "oversized file held by grace must report remaining grace for deferred prune");

        // Synchronized cache-hit touch verifies O_NOFOLLOW + size and refreshes mtime,
        // while missing/wrong-size files return false so caller falls through to miss.
        assert!(try_touch_cached_file(&p_big, 8192));
        assert!(!try_touch_cached_file(&p_big, 4096));
        assert!(!try_touch_cached_file(&dir.join("missing"), 10));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn stream_into_cache_part_handles_empty_zero_cap_and_rejects_growth() {
        struct MockChunks(std::collections::VecDeque<Result<Vec<u8>, String>>);
        impl ChunkSource for MockChunks {
            type Chunk = Vec<u8>;
            async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, String> {
                match self.0.pop_front() {
                    Some(Ok(c)) => Ok(Some(c)),
                    Some(Err(e)) => Err(e),
                    None => Ok(None),
                }
            }
        }

        let dir = PathBuf::from(format!("/var/tmp/olo-files-sock-stream-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 1. Empty file / `--max 0`: 0 bytes + Ok(None) succeeds cleanly.
        let p_empty = dir.join("part-empty");
        let mut src_empty = MockChunks(std::collections::VecDeque::new());
        let n = stream_into_cache_part(&p_empty, 0, Duration::from_secs(2), &mut src_empty).await.unwrap();
        assert_eq!(n, 0);
        assert_eq!(std::fs::metadata(&p_empty).unwrap().len(), 0);

        // 2. Growing file (120 bytes when 100 expected) is rejected while streaming.
        let p_grow = dir.join("part-grow");
        let mut src_grow = MockChunks(std::collections::VecDeque::from([Ok(vec![b'a'; 120])]));
        let err = stream_into_cache_part(&p_grow, 100, Duration::from_secs(2), &mut src_grow).await.unwrap_err();
        assert!(err.contains("exceeded expected size"), "{err}");

        // 3. Short read (80 bytes when 100 expected) is rejected.
        let p_short = dir.join("part-short");
        let mut src_short = MockChunks(std::collections::VecDeque::from([Ok(vec![b'a'; 80])]));
        let err = stream_into_cache_part(&p_short, 100, Duration::from_secs(2), &mut src_short).await.unwrap_err();
        assert!(err.contains("ended early"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_stage_0700_atomic_noreplace_and_cancel_vs_commit() {
        let dir = PathBuf::from(format!("/var/tmp/olo-files-sock-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 1. Private staging directory is strictly 0700.
        let stage = create_private_stage_dir(&dir, ".olo-rx-").unwrap();
        assert_eq!(std::fs::metadata(&stage).unwrap().permissions().mode() & 0o777, 0o700);
        let staged_file = stage.join("report.pdf");
        std::fs::write(&staged_file, b"new-pdf").unwrap();

        // 2. Broken symlink at destination must NOT be overwritten.
        let dst = dir.join("report.pdf");
        std::os::unix::fs::symlink(dir.join("nowhere"), &dst).unwrap();
        let state = AtomicU8::new(STATE_RUNNING);
        let err = publish_staged_download(&stage, &staged_file, &dst, None, &state);
        assert!(matches!(err, Err(TransferError::Failed(ref s)) if s.contains("already exists")));
        assert!(std::fs::symlink_metadata(&dst).unwrap().file_type().is_symlink(), "broken symlink must stay intact");
        assert!(!stage.exists(), "stage must be cleaned up on collision");

        // 3. Cancellation winning before publish discards stage and never touches dst.
        std::fs::remove_file(&dst).unwrap();
        let stage2 = create_private_stage_dir(&dir, ".olo-rx-").unwrap();
        let sf2 = stage2.join("report.pdf");
        std::fs::write(&sf2, b"cancelled-pdf").unwrap();
        let state2 = AtomicU8::new(STATE_CANCELLED);
        let res2 = publish_staged_download(&stage2, &sf2, &dst, None, &state2);
        assert!(matches!(res2, Err(TransferError::Cancelled)));
        assert!(!dst.exists(), "cancelled download must not publish destination");
        assert!(!stage2.exists(), "cancelled download must delete stage");

        // 4. Two concurrent publishers racing for the exact same destination:
        // exactly one succeeds, one fails with EEXIST, and zero data is clobbered.
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let mut handles = Vec::new();
        for i in 0..2 {
            let b = barrier.clone();
            let dir_c = dir.clone();
            let dst_c = dst.clone();
            handles.push(std::thread::spawn(move || {
                let stg = create_private_stage_dir(&dir_c, ".olo-rx-").unwrap();
                let sf = stg.join("report.pdf");
                std::fs::write(&sf, format!("host-{i}")).unwrap();
                let cs = AtomicU8::new(STATE_RUNNING);
                b.wait();
                publish_staged_download(&stg, &sf, &dst_c, None, &cs).is_ok()
            }));
        }
        let outcomes: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(outcomes.iter().filter(|&&ok| ok).count(), 1, "exactly one concurrent publisher may win: {outcomes:?}");
        let won = std::fs::read_to_string(&dst).unwrap();
        assert!(won == "host-0" || won == "host-1", "{won}");

        // 5. Directory publication sets & verifies source root permissions via
        //    fchmod INSIDE the private 0700 outer wrapper BEFORE rename out
        //    (preserving sticky bit 0o1750) and removes the outer wrapper cleanly.
        let stage_outer = create_private_stage_dir(&dir, ".olo-rx-").unwrap();
        let stage_inner = stage_outer.join("dir");
        std::fs::create_dir(&stage_inner).unwrap();
        std::fs::write(stage_inner.join("a.txt"), b"ok").unwrap();
        let dst_dir = dir.join("folder");
        let cs_dir = AtomicU8::new(STATE_RUNNING);
        publish_staged_download(&stage_outer, &stage_inner, &dst_dir, Some(0o1750), &cs_dir).unwrap();
        assert_eq!(std::fs::metadata(&dst_dir).unwrap().permissions().mode() & 0o3777, 0o1750);
        assert!(!stage_outer.exists(), "outer 0700 wrapper must be cleaned up after publish");
        // Late cancel after commit cannot flip STATE_COMMITTED.
        assert!(cs_dir.compare_exchange(STATE_RUNNING, STATE_CANCELLED, Ordering::AcqRel, Ordering::Acquire).is_err());

        // 6. Discarding a stage with a 0555 read-only subdir cleans up completely via dir-fd fchmod.
        let stage_ro = create_private_stage_dir(&dir, ".olo-rx-").unwrap();
        let ro_sub = stage_ro.join("pkg/mod");
        std::fs::create_dir_all(&ro_sub).unwrap();
        std::fs::write(ro_sub.join("file.go"), b"package mod").unwrap();
        std::fs::set_permissions(&ro_sub, std::fs::Permissions::from_mode(0o555)).unwrap();
        let cs_ro = AtomicU8::new(STATE_CANCELLED);
        let res_ro = publish_staged_download(&stage_ro, &stage_ro, &dir.join("ro-dst"), Some(0o755), &cs_ro);
        assert!(matches!(res_ro, Err(TransferError::Cancelled)));
        assert!(!stage_ro.exists(), "0555 stage subdir must be cleaned up on cancel");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn half_close_does_not_cancel_while_full_disconnect_and_cancel_frame_do() {
        // 1. Keep ONE single watcher future alive across half-close AND full close!
        let (client, server) = UnixStream::pair().unwrap();
        let (mut cr, mut cw) = client.into_split();
        cw.write_all(b"{\"op\":\"write_file\"}\n").await.unwrap();
        cw.shutdown().await.unwrap();
        let (sr, mut sw) = server.into_split();
        let mut reader = BufReader::new(sr);
        let req = read_request_line(&mut reader, 1024).await.unwrap().unwrap();
        assert!(req.contains("write_file"));

        let mut watcher = Box::pin(wait_peer_cancel_or_disconnect(&mut reader));
        let half_res = tokio::select! {
            () = &mut watcher => true,
            () = tokio::time::sleep(Duration::from_millis(80)) => false,
        };
        assert!(!half_res, "valid request-side half-close must NOT cancel in-flight operation");

        // Reply still reaches the half-closed client cleanly while watcher stays armed.
        write_line(&mut sw, &json!({ "ok": true })).await.unwrap();
        let mut reply = String::new();
        BufReader::new(&mut cr).read_line(&mut reply).await.unwrap();
        assert!(reply.contains("\"ok\":true"), "{reply}");

        // 2. Full disconnect wakes THAT SAME registered watcher immediately!
        drop(cw);
        drop(cr);
        tokio::time::timeout(Duration::from_secs(2), &mut watcher).await.expect("full peer disconnect must wake the SAME registered watcher immediately");

        // 3. Explicit cancel frame split across phase boundaries wakes CancelMonitor cleanly!
        let (mut client2, server2) = UnixStream::pair().unwrap();
        let (sr2, _sw2) = server2.into_split();
        let mon = CancelMonitor::spawn(BufReader::new(sr2), None, None);
        client2.write_all(b"{\"op\":").await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!mon.is_cancelled());
        client2.write_all(b"\"cancel\"}\n").await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), mon.wait()).await.expect("split cancel frame must wake CancelMonitor across phases");
        assert!(mon.is_cancelled());
    }

    #[tokio::test]
    async fn ensure_helper_enforces_global_off_and_stops_on_policy_race() {
        #[derive(Clone)]
        struct FakeHostState {
            host_helper: bool,
            per_host: Option<String>,
            same_endpoint: bool,
            ready: bool,
        }

        // 1. Global host_helper == false rejects even when helper_ready == true.
        let fake = Arc::new(Mutex::new(FakeHostState { host_helper: false, per_host: None, same_endpoint: true, ready: true }));
        let fake_c = fake.clone();
        let err = ensure_helper_core(
            "test-global-off",
            "dev-box",
            helper_policy(false, None),
            true,
            true,
            Duration::from_secs(5),
            move || {
                let g = fake_c.lock().unwrap();
                HelperCheck { policy: helper_policy(g.host_helper, g.per_host.as_deref()), same_endpoint: g.same_endpoint, ready: g.ready }
            },
            || Ok(()),
            || async { Ok(None::<()>) },
            |()| async { Ok(()) },
        )
        .await
        .unwrap_err();
        assert!(err.contains("Preferences"), "{err}");

        // 2. Per-host "off" rejects even when helper_ready == true.
        let fake_off = Arc::new(Mutex::new(FakeHostState { host_helper: true, per_host: Some("off".into()), same_endpoint: true, ready: true }));
        let fake_off_c = fake_off.clone();
        let err_off = ensure_helper_core(
            "test-host-off",
            "dev-box",
            helper_policy(true, Some("off")),
            true,
            true,
            Duration::from_secs(5),
            move || {
                let g = fake_off_c.lock().unwrap();
                HelperCheck { policy: helper_policy(g.host_helper, g.per_host.as_deref()), same_endpoint: g.same_endpoint, ready: g.ready }
            },
            || Ok(()),
            || async { Ok(None::<()>) },
            |()| async { Ok(()) },
        )
        .await
        .unwrap_err();
        assert!(err_off.contains("Host window"), "{err_off}");

        // 3. Opt-out during install suppresses readiness and stops using the helper
        //    without running any shell rm/rmdir rollback command.
        let race_state = Arc::new(Mutex::new(FakeHostState { host_helper: true, per_host: None, same_endpoint: true, ready: false }));
        let rs_check = race_state.clone();
        let rs_mark = race_state.clone();
        let rs_put = race_state.clone();
        let err_race = ensure_helper_core(
            "test-remove-race",
            "dev-box",
            HelperPolicy::Allowed,
            false,
            true,
            Duration::from_secs(5),
            move || {
                let g = rs_check.lock().unwrap();
                HelperCheck { policy: helper_policy(g.host_helper, g.per_host.as_deref()), same_endpoint: g.same_endpoint, ready: g.ready }
            },
            move || {
                let mut g = rs_mark.lock().unwrap();
                let pol = helper_policy(g.host_helper, g.per_host.as_deref());
                if pol == HelperPolicy::Allowed && g.same_endpoint {
                    g.ready = true;
                    Ok(())
                } else if pol != HelperPolicy::Allowed {
                    Err(helper_policy_error(pol, "dev-box"))
                } else {
                    Err("endpoint changed".into())
                }
            },
            || async { Ok(Some(())) },
            move |()| async move {
                rs_put.lock().unwrap().per_host = Some("off".into());
                Ok(())
            },
        )
        .await
        .unwrap_err();
        assert!(err_race.contains("turned off"), "{err_race}");
        assert!(!race_state.lock().unwrap().ready, "must not mark host ready after opt-out");

        // 4. Endpoint changed while probe was paused: suppresses readiness and fails.
        let ep_state = Arc::new(Mutex::new(FakeHostState { host_helper: true, per_host: None, same_endpoint: true, ready: false }));
        let ep_check = ep_state.clone();
        let ep_ask = ep_state.clone();
        let err_ep = ensure_helper_core(
            "test-endpoint-race",
            "dev-box",
            HelperPolicy::Allowed,
            false,
            true,
            Duration::from_secs(5),
            move || {
                let g = ep_check.lock().unwrap();
                HelperCheck { policy: helper_policy(g.host_helper, g.per_host.as_deref()), same_endpoint: g.same_endpoint, ready: g.ready }
            },
            || Ok(()),
            move || async move {
                ep_ask.lock().unwrap().same_endpoint = false;
                Ok(Some(()))
            },
            |()| async { panic!("put must not run after endpoint changed during ask") },
        )
        .await
        .unwrap_err();
        assert!(err_ep.contains("changed"), "{err_ep}");

        // 5. Lock wait counts against the single absolute timeout (no reset after lock):
        //    100 ms lock hold + 180 ms ask > 200 ms total timeout -> must time out (never succeed),
        //    with wide margin (< 500 ms) so scheduling jitter cannot cause flakes.
        let lock = helper_host_lock("test-single-deadline");
        let held = lock.lock_owned().await;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            drop(held);
        });
        let started = Instant::now();
        let err_to = ensure_helper_core(
            "test-single-deadline",
            "dev-box",
            HelperPolicy::Allowed,
            false,
            true,
            Duration::from_millis(200),
            || HelperCheck { policy: HelperPolicy::Allowed, same_endpoint: true, ready: false },
            || Ok(()),
            || async {
                tokio::time::sleep(Duration::from_millis(180)).await;
                Ok(None::<()>)
            },
            |()| async { Ok(()) },
        )
        .await
        .unwrap_err();
        assert!(err_to.contains("Timed out"), "{err_to}");
        assert!(started.elapsed() < Duration::from_millis(500), "lock wait must not grant a fresh timeout: {:?}", started.elapsed());
    }

    #[tokio::test]
    async fn inline_memory_budget_bounds_large_requests_without_deadlock_or_blocking_small() {
        // 5 slabs = 320 KiB global budget.
        let budget = Arc::new(Semaphore::new(5));
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        // 1. Two concurrent large partial readers whose combined 2× budget
        //    (2 × 180 KiB × 2 => 2 × 5 = 10 slabs > 5 slabs) exceeds capacity
        //    must serialize cleanly without hold-and-wait deadlock!
        let (mut w1, r1) = tokio::io::duplex(16 * 1024);
        let (mut w2, r2) = tokio::io::duplex(16 * 1024);
        let payload = format!("{}\n", "a".repeat(180 * 1024));
        let p1 = payload.clone();
        let p2 = payload.clone();
        let send1 = tokio::spawn(async move {
            for c in p1.as_bytes().chunks(8 * 1024) {
                w1.write_all(c).await.unwrap();
                tokio::task::yield_now().await;
            }
        });
        let send2 = tokio::spawn(async move {
            for c in p2.as_bytes().chunks(8 * 1024) {
                w2.write_all(c).await.unwrap();
                tokio::task::yield_now().await;
            }
        });
        let b1 = budget.clone();
        let g1 = gate.clone();
        let t1 = tokio::spawn(async move {
            let mut br = BufReader::new(r1);
            let (line, permits) = read_request_line_budgeted(&mut br, 256 * 1024, Some((&b1, &g1))).await.unwrap();
            let len = line.unwrap().len();
            drop(permits);
            len
        });
        let b2 = budget.clone();
        let g2 = gate.clone();
        let t2 = tokio::spawn(async move {
            let mut br = BufReader::new(r2);
            let (line, permits) = read_request_line_budgeted(&mut br, 256 * 1024, Some((&b2, &g2))).await.unwrap();
            let len = line.unwrap().len();
            drop(permits);
            len
        });
        let (l1, l2) = tokio::time::timeout(Duration::from_secs(3), async { (t1.await.unwrap(), t2.await.unwrap()) }).await.expect("concurrent large readers must never deadlock");
        send1.await.unwrap();
        send2.await.unwrap();
        assert_eq!(l1, payload.len());
        assert_eq!(l2, payload.len());
        assert_eq!(budget.available_permits(), 5);

        // 2. While one large request holds all 5 permits, a small metadata
        //    request (<= 64 KiB) STILL succeeds immediately without contending!
        let mut br = BufReader::new(payload.as_bytes());
        let (line, permits) = read_request_line_budgeted(&mut br, 256 * 1024, Some((&budget, &gate))).await.unwrap();
        assert_eq!(line.unwrap().len(), payload.len());
        assert_eq!(budget.available_permits(), 0);
        // Downsizing a merged permit to 2 slabs releases exactly 3 permits immediately!
        let shrunk = shrink_inline_permit(permits, 2);
        assert_eq!(budget.available_permits(), 3, "shrink_inline_permit must release exact excess permits");
        let small = b"{\"op\":\"hosts\"}\n";
        let mut sbr = BufReader::new(&small[..]);
        let (sline, _) = tokio::time::timeout(Duration::from_secs(1), read_request_line_budgeted(&mut sbr, 256 * 1024, Some((&budget, &gate)))).await.unwrap().unwrap();
        assert!(sline.unwrap().contains("hosts"));
        drop(shrunk);
        assert_eq!(budget.available_permits(), 5);
    }

    #[tokio::test]
    async fn request_line_is_bounded_including_utf8_boundary_and_open_writer() {
        let mut small = BufReader::new(&b"{\"op\":\"hosts\"}\n"[..]);
        assert_eq!(read_request_line(&mut small, 64).await.unwrap().as_deref(), Some("{\"op\":\"hosts\"}\n"));
        assert_eq!(read_request_line(&mut small, 64).await.unwrap(), None);

        let exact = format!("{}\n", "a".repeat(63));
        let mut ex = BufReader::new(exact.as_bytes());
        assert_eq!(read_request_line(&mut ex, 64).await.unwrap().as_deref(), Some(exact.as_str()));

        let utf8_over = format!("{}é\n", "a".repeat(64));
        let mut u8r = BufReader::new(utf8_over.as_bytes());
        let err = read_request_line(&mut u8r, 64).await.unwrap_err();
        assert!(err.contains("exceeds"), "expected size limit error, got: {err}");

        let (mut writer, reader) = tokio::io::duplex(256);
        let mut br = BufReader::new(reader);
        let write_task = tokio::spawn(async move {
            writer.write_all(&[b'x'; 65]).await.unwrap();
            std::future::pending::<()>().await;
        });
        let err = tokio::time::timeout(Duration::from_secs(2), read_request_line(&mut br, 64)).await.expect("must reject oversize stream before EOF").unwrap_err();
        assert!(err.contains("exceeds"), "{err}");
        write_task.abort();
    }
}
