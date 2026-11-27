//! `olo-rex-link screen follow`: zero-polling host-side discovery of Agent
//! Screens over `inotify` + `pidfd_open` + `signalfd` + stdin EOF in a single
//! `poll`.
//!
//! Runs on every connected Rex host (whether or not Olo for screens is
//! installed yet): watches the deepest existing ancestor of each screens
//! folder and the host Olo prefix until they appear, stepping watches down as
//! directories are created. Sleeps indefinitely in `poll(..., -1)` while
//! nothing changes (0 wakeups/s, 0% CPU, 0 bytes/s). Bursts of writes to
//! `actions.jsonl` are debounced to at most two lightweight `last_used` lines
//! per second per screen. Exits immediately on stdin EOF, broken stdout, or
//! termination signal.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::ffi::CString;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{FileExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use olo_rex_link::screen_wire::{FollowEvent, HostScreen, HostScreenState, HostScreenStatus, MAX_JSON_LINE, PROTO, ScreenOwner};
use serde::Deserialize;

use crate::screens::{
    ActionLineScanner, BudgetedTailProbe, CancelFd, NOT_INSTALLED_REASON, OwnedFd, PinResult, RuntimeScreen, default_now, exists_at, host_readiness, is_controller_pid_alive_with_ticks,
    open_regular_at, open_verified_subdir, pidfd_exited, pin_live_pid, prefix, probe_last_used_from_file_budgeted, read_bounded_regular_at, read_control_meta, read_installed_meta, read_owner,
    read_owner_for_running, read_runtime_screen, screen_pid_running, screens_runtime_dir_candidates_checked, screens_state_dir, set_nonblocking, timeout_ms_ceil, try_cleanup_stale_control,
    valid_name, write_all_cancellable,
};

/// Minimum interval between `LastUsed` lines for the same screen (at most 2/s).
const LAST_USED_INTERVAL: Duration = Duration::from_millis(500);
/// Maximum bytes scanned synchronously during initial `Snapshot` construction so
/// huge or numerous histories never delay the initial snapshot (Finding 24).
const INITIAL_SCAN_BUDGET_BYTES: u64 = 2 * 1024 * 1024;
/// Maximum bytes scanned per event-loop slice for deferred large-history counts.
const SLICE_SCAN_BUDGET_BYTES: u64 = 2 * 1024 * 1024;

const DIR_MASK: u32 = libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_MODIFY
    | libc::IN_CLOSE_WRITE
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_ATTRIB
    | libc::IN_DELETE_SELF
    | libc::IN_MOVE_SELF
    | libc::IN_ONLYDIR;

const ANCESTOR_MASK: u32 = libc::IN_CREATE | libc::IN_MOVED_TO | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_ONLYDIR;

#[derive(Clone, Debug, PartialEq, Eq)]
enum TargetKind {
    RuntimeDir,
    StateRoot,
    StateScreen(String),
    PrefixRoot,
    PrefixBin,
    PrefixShare,
    DefaultDir,
    SystemLdCache,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ReconcileChanged {
    screens: bool,
    prefix: bool,
    default_dir: bool,
}

impl ReconcileChanged {
    fn note_roles(&mut self, roles: &[WatchRole]) {
        for r in roles {
            let kind = match r {
                WatchRole::Target(k) | WatchRole::Ancestor { kind: k, .. } => k,
            };
            match kind {
                TargetKind::RuntimeDir | TargetKind::StateRoot | TargetKind::StateScreen(_) => self.screens = true,
                TargetKind::PrefixRoot | TargetKind::PrefixBin | TargetKind::PrefixShare | TargetKind::SystemLdCache => self.prefix = true,
                TargetKind::DefaultDir => self.default_dir = true,
            }
        }
    }

    fn merge(&mut self, other: Self) {
        self.screens |= other.screens;
        self.prefix |= other.prefix;
        self.default_dir |= other.default_dir;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WatchRole {
    Target(TargetKind),
    Ancestor { want_child: String, kind: TargetKind },
}

#[derive(Default)]
struct WatchEntry {
    wd: i32,
    mask: u32,
    dev_ino: (u64, u64),
    roles: Vec<WatchRole>,
}

struct FollowWatches {
    ifd: OwnedFd,
    by_path: HashMap<PathBuf, WatchEntry>,
    by_wd: HashMap<i32, PathBuf>,
}

fn dir_dev_ino(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    meta.is_dir().then_some((meta.dev(), meta.ino()))
}

fn is_permanent_watch_err(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(libc::ENOSPC | libc::EMFILE | libc::ENFILE | libc::ENOMEM | libc::EACCES | libc::EPERM))
}

impl FollowWatches {
    fn new() -> io::Result<Self> {
        // SAFETY: creates a non-blocking, close-on-exec inotify descriptor.
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { ifd: OwnedFd::new(fd), by_path: HashMap::new(), by_wd: HashMap::new() })
    }

    /// Rebuild the inotify instance from scratch (used on `IN_Q_OVERFLOW` so no
    /// stale or replaced-inode watch can survive an event drop).
    fn rebuild(&mut self) -> io::Result<()> {
        let fresh = Self::new()?;
        self.ifd = fresh.ifd;
        self.by_path.clear();
        self.by_wd.clear();
        Ok(())
    }

    fn fd(&self) -> i32 {
        self.ifd.raw()
    }

    /// Reconcile watches against all logical targets (stepping down from
    /// ancestors as directories appear, and falling back to ancestors if
    /// directories are removed). Loops to a fixed point so rapid `mkdir -p`
    /// bursts cannot slip a new child directory between `is_dir()` and
    /// `inotify_add_watch()`. Returns which category of watches changed so
    /// adding a screen directory never re-runs the ELF readiness probe (Finding 23).
    fn reconcile(&mut self, runtime_dirs: &[PathBuf], state_root: &Path, prefix_dir: &Path, default_dir: &Path, watch_ld_cache: bool) -> io::Result<ReconcileChanged> {
        let mut changed = ReconcileChanged::default();
        let mut permanent_err: Option<io::Error> = None;
        for _ in 0..16 {
            let mut desired: HashMap<PathBuf, Vec<WatchRole>> = HashMap::new();
            let mut add = |target: PathBuf, kind: TargetKind| {
                let (watch_path, role) = deepest_existing_watch(&target, kind);
                let roles = desired.entry(watch_path).or_default();
                if !roles.contains(&role) {
                    roles.push(role);
                }
            };

            for rt in runtime_dirs {
                add(rt.clone(), TargetKind::RuntimeDir);
            }
            add(state_root.to_path_buf(), TargetKind::StateRoot);
            if state_root.is_dir()
                && let Ok(rd) = std::fs::read_dir(state_root)
            {
                for entry in rd.flatten() {
                    if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                        continue;
                    }
                    let Ok(name) = entry.file_name().into_string() else { continue };
                    if valid_name(&name) {
                        add(entry.path(), TargetKind::StateScreen(name));
                    }
                }
            }
            add(prefix_dir.to_path_buf(), TargetKind::PrefixRoot);
            add(prefix_dir.join("bin"), TargetKind::PrefixBin);
            add(prefix_dir.join("share/olo"), TargetKind::PrefixShare);
            add(default_dir.to_path_buf(), TargetKind::DefaultDir);
            if watch_ld_cache && Path::new("/etc").is_dir() {
                add(PathBuf::from("/etc"), TargetKind::SystemLdCache);
            }

            // Drop watches no longer desired (e.g. an ancestor we stepped down from
            // or a deleted screen folder).
            let stale: Vec<PathBuf> = self.by_path.keys().filter(|p| !desired.contains_key(*p)).cloned().collect();
            for path in stale {
                if let Some(entry) = self.by_path.remove(&path) {
                    changed.note_roles(&entry.roles);
                    self.by_wd.remove(&entry.wd);
                    // SAFETY: `self.ifd` is valid and `entry.wd` was returned by `inotify_add_watch`.
                    unsafe {
                        libc::inotify_rm_watch(self.ifd.raw(), entry.wd);
                    }
                }
            }

            // Add or update desired watches (re-adding if the underlying directory inode changed).
            let mut retry = false;
            for (path, roles) in desired {
                let only_ld_cache = roles.iter().all(|r| matches!(r, WatchRole::Target(TargetKind::SystemLdCache)));
                let want_mask = if only_ld_cache {
                    libc::IN_CREATE | libc::IN_MOVED_TO | libc::IN_CLOSE_WRITE | libc::IN_DELETE | libc::IN_MOVED_FROM | libc::IN_ONLYDIR
                } else if roles.iter().any(|r| matches!(r, WatchRole::Target(_))) {
                    DIR_MASK
                } else {
                    ANCESTOR_MASK
                };
                let Some(cur_id) = dir_dev_ino(&path) else {
                    retry = true;
                    continue;
                };
                if let Some(existing) = self.by_path.get_mut(&path)
                    && existing.mask == want_mask
                    && existing.dev_ino == cur_id
                {
                    existing.roles = roles;
                    continue;
                }
                if let Some(old) = self.by_path.remove(&path) {
                    self.by_wd.remove(&old.wd);
                    // SAFETY: remove stale watch for old inode before installing replacement.
                    unsafe {
                        libc::inotify_rm_watch(self.ifd.raw(), old.wd);
                    }
                }
                let is_new_state_root = roles.iter().any(|r| matches!(r, WatchRole::Target(TargetKind::StateRoot)));
                let Ok(c) = CString::new(path.as_os_str().as_bytes()) else { continue };
                // SAFETY: `self.ifd` is valid and `c` is NUL-terminated.
                let wd = unsafe { libc::inotify_add_watch(self.ifd.raw(), c.as_ptr(), want_mask) };
                if wd >= 0 {
                    changed.note_roles(&roles);
                    let after_id = dir_dev_ino(&path).unwrap_or(cur_id);
                    self.by_path.insert(path.clone(), WatchEntry { wd, mask: want_mask, dev_ino: after_id, roles });
                    self.by_wd.insert(wd, path);
                    if is_new_state_root || after_id != cur_id {
                        // A screen subdirectory may have appeared in `state_root`
                        // between `read_dir` and `inotify_add_watch`; do one more
                        // pass now that `state_root` itself is watched.
                        retry = true;
                    }
                } else {
                    let err = io::Error::last_os_error();
                    if is_permanent_watch_err(&err) {
                        permanent_err = Some(err);
                    } else {
                        // Target may have disappeared between `is_dir()` and
                        // `inotify_add_watch`; retry to fall back to its ancestor.
                        retry = true;
                    }
                }
            }

            // Close the TOCTOU window where an ancestor's awaited child appeared
            // between `is_dir()` and `inotify_add_watch()`: if any watched
            // ancestor's child already exists now, step down immediately.
            if !retry {
                'check: for (path, entry) in &self.by_path {
                    for role in &entry.roles {
                        if let WatchRole::Ancestor { want_child, .. } = role
                            && !want_child.is_empty()
                            && path.join(want_child).is_dir()
                        {
                            retry = true;
                            break 'check;
                        }
                    }
                }
            }
            if !retry {
                break;
            }
        }
        if let Some(err) = permanent_err {
            return Err(err);
        }
        Ok(changed)
    }

    /// Remove a watch descriptor from the kernel (unless `IN_IGNORED` already
    /// removed it) and forget it from our maps.
    fn remove_and_forget_wd(&mut self, wd: i32, already_ignored: bool) {
        if !already_ignored {
            // SAFETY: explicitly drop kernel watch on `IN_MOVE_SELF` so moved
            // directories never leak watches.
            unsafe {
                libc::inotify_rm_watch(self.ifd.raw(), wd);
            }
        }
        if let Some(path) = self.by_wd.remove(&wd)
            && self.by_path.get(&path).is_some_and(|e| e.wd == wd)
        {
            self.by_path.remove(&path);
        }
    }
}

/// Find the deepest existing directory along `target` (or `target` itself if it
/// exists), returning `(dir_to_watch, role)`.
fn deepest_existing_watch(target: &Path, kind: TargetKind) -> (PathBuf, WatchRole) {
    if target.is_dir() {
        return (target.to_path_buf(), WatchRole::Target(kind));
    }
    let mut cur = target;
    while let Some(parent) = cur.parent() {
        if parent.as_os_str().is_empty() {
            break;
        }
        if parent.is_dir() {
            let want_child = cur.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            return (parent.to_path_buf(), WatchRole::Ancestor { want_child, kind });
        }
        cur = parent;
    }
    (PathBuf::from("/"), WatchRole::Ancestor { want_child: String::new(), kind })
}

#[derive(Deserialize)]
struct KeptOptions {
    #[serde(default = "default_width")]
    width: u32,
    #[serde(default = "default_height")]
    height: u32,
    #[serde(default = "default_scale")]
    scale: f64,
    #[serde(default)]
    sound: bool,
}

fn default_width() -> u32 {
    1440
}

fn default_height() -> u32 {
    900
}

fn default_scale() -> f64 {
    1.0
}

impl Default for KeptOptions {
    fn default() -> Self {
        Self { width: default_width(), height: default_height(), scale: default_scale(), sound: false }
    }
}

/// Incremental streaming cache of `(last_used, actions)` for one screen's
/// `actions.jsonl`:
/// - Uses constant-memory schema-correct [`ActionLineScanner`] with a byte
///   `cursor` that advances even across unfinished partial lines (zero repeated
///   rereads, Finding 20).
/// - Tracks `(dev, ino, mtime)` plus 64-byte header and cursor-boundary
///   fingerprints; always rescans on ambiguous same-size writes (`len == cursor`
///   with `dirty` or changed `mtime`) so middle-only in-place edits are never
///   missed (Item 10 / Finding 21).
/// - Charges EVERY read (fingerprints, budgeted tail probes, and forward scans)
///   to the shared scan budget, remembers inconclusive tail probes until the
///   file changes, and never requeues on `EIO`/zero-progress reads (Item 10 /
///   Findings 20 & 24).
#[derive(Clone, Debug)]
struct ActionMeta {
    dev: u64,
    ino: u64,
    mtime_sec: i64,
    mtime_nsec: i64,
    cursor: u64,
    head_fp: [u8; 64],
    head_len: usize,
    tail_fp: [u8; 64],
    tail_len: usize,
    scanner: ActionLineScanner,
    count: usize,
    last_used: Option<u64>,
    /// Authoritative EOF `last_used` from budgeted O(1) tail probe, returned while
    /// a large file's full count is still being sliced (`!self.complete`).
    eof_last_used: Option<u64>,
    /// True once we have already attempted the budgeted tail probe for the
    /// current `(dev, ino, len, mtime)` so an inconclusive probe is remembered
    /// until the file changes instead of repeating on every slice (Item 10 / F20/F24).
    eof_probe_done: bool,
    eof_probe_len: u64,
    complete: bool,
    /// Set when an I/O or open error prevented reading; cleared only by a
    /// relevant filesystem event (`dirty`) or inode/mtime change so EIO never
    /// spins at rest (Item 10 / F24).
    read_failed: bool,
    /// Set by `inotify` on `IN_MODIFY`/`IN_CLOSE_WRITE` so same-tick Linux coarse
    /// mtime collisions still detect in-place rewrites (Finding 21).
    dirty: bool,
    #[cfg(test)]
    bytes_read_total: u64,
}

impl Default for ActionMeta {
    fn default() -> Self {
        Self {
            dev: 0,
            ino: 0,
            mtime_sec: 0,
            mtime_nsec: 0,
            cursor: 0,
            head_fp: [0u8; 64],
            head_len: 0,
            tail_fp: [0u8; 64],
            tail_len: 0,
            scanner: ActionLineScanner::default(),
            count: 0,
            last_used: None,
            eof_last_used: None,
            eof_probe_done: false,
            eof_probe_len: 0,
            complete: false,
            read_failed: false,
            dirty: false,
            #[cfg(test)]
            bytes_read_total: 0,
        }
    }
}

impl ActionMeta {
    fn reset_for(&mut self, dev: u64, ino: u64, mtime_sec: i64, mtime_nsec: i64) {
        self.dev = dev;
        self.ino = ino;
        self.mtime_sec = mtime_sec;
        self.mtime_nsec = mtime_nsec;
        self.cursor = 0;
        self.head_len = 0;
        self.tail_len = 0;
        self.scanner = ActionLineScanner::default();
        self.count = 0;
        self.last_used = None;
        self.eof_last_used = None;
        self.eof_probe_done = false;
        self.eof_probe_len = 0;
        self.complete = false;
        self.read_failed = false;
        self.dirty = false;
    }

    fn effective_summary(&self) -> (Option<u64>, Option<usize>) {
        let eof_ts = self.scanner.peek_complete_eof();
        let last_used = eof_ts.or(self.last_used);
        let actions = self.complete.then(|| self.count.saturating_add(usize::from(eof_ts.is_some())));
        (last_used, actions)
    }

    /// Verify head and cursor-boundary fingerprints, charging every read byte to
    /// `*budget_bytes` (and `bytes_read_total` in tests — Item 10 / T3).
    /// Returns `Ok(true)` if fingerprints match, `Ok(false)` on mismatch or
    /// insufficient budget, or `Err(io::Error)` on read error (`EIO`).
    fn fingerprint_matches(&mut self, f: &std::fs::File, budget_bytes: &mut u64) -> io::Result<bool> {
        let mut read_exact_metered = |buf: &mut [u8], at: u64| -> io::Result<bool> {
            let need = buf.len() as u64;
            if *budget_bytes < need {
                return Ok(false);
            }
            let mut off = 0usize;
            while off < buf.len() {
                match f.read_at(&mut buf[off..], at + off as u64) {
                    Ok(0) => return Ok(false),
                    Ok(n) => {
                        off += n;
                        *budget_bytes = budget_bytes.saturating_sub(n as u64);
                        #[cfg(test)]
                        {
                            self.bytes_read_total = self.bytes_read_total.saturating_add(n as u64);
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
            Ok(true)
        };

        if self.head_len > 0 {
            let mut h = [0u8; 64];
            if !read_exact_metered(&mut h[..self.head_len], 0)? || h[..self.head_len] != self.head_fp[..self.head_len] {
                return Ok(false);
            }
        }
        if self.tail_len > 0 {
            let at = self.cursor.saturating_sub(self.tail_len as u64);
            let mut t = [0u8; 64];
            if !read_exact_metered(&mut t[..self.tail_len], at)? || t[..self.tail_len] != self.tail_fp[..self.tail_len] {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn update_fingerprints(&mut self, chunk_start: u64, data: &[u8]) {
        if chunk_start < 64 {
            let off = chunk_start as usize;
            let take = (64 - off).min(data.len());
            self.head_fp[off..off + take].copy_from_slice(&data[..take]);
            self.head_len = self.head_len.max(off + take);
        }
        if data.len() >= 64 {
            self.tail_fp.copy_from_slice(&data[data.len() - 64..]);
            self.tail_len = 64;
        } else if !data.is_empty() {
            let keep = (64 - data.len()).min(self.tail_len);
            let start = self.tail_len - keep;
            self.tail_fp.copy_within(start..self.tail_len, 0);
            self.tail_fp[keep..keep + data.len()].copy_from_slice(data);
            self.tail_len = keep + data.len();
        }
    }
}

/// Outcome of advancing one screen's `actions.jsonl` cache under a byte budget.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RefreshOutcome {
    last_used: Option<u64>,
    actions: Option<usize>,
    /// True only when more bytes remain AND no I/O error occurred.
    needs_more: bool,
    /// True when the forward scan actually advanced `cursor` (required to
    /// immediately requeue on the deferred loop so EIO / zero-progress never
    /// spins with 0 ms poll timeout — Item 10 / Finding 24).
    progressed: bool,
}

/// Advance `cache` against `actions.jsonl` inside `dir_fd` using at most
/// `*budget_bytes` (including fingerprints AND tail probes!).
/// - `dir_fd == None` with `dir_missing == true` -> `(None, Some(0), false)`.
/// - `open_regular_at` `NotFound` -> `(None, Some(0), false)`.
/// - Any other open/read error (`EACCES`, `ELOOP`, FIFO, `EIO`) ->
///   `actions: None`, `needs_more: false` (NEVER `Some(0)` and NEVER spins!).
fn refresh_action_meta_budgeted(dir_fd: Option<&OwnedFd>, dir_missing: bool, cache: &mut ActionMeta, budget_bytes: &mut u64) -> RefreshOutcome {
    let Some(dir_fd) = dir_fd else {
        if dir_missing {
            *cache = ActionMeta { complete: true, ..ActionMeta::default() };
            return RefreshOutcome { last_used: None, actions: Some(0), needs_more: false, progressed: false };
        }
        cache.complete = false;
        cache.read_failed = true;
        return RefreshOutcome { last_used: None, actions: None, needs_more: false, progressed: false };
    };
    let (mut f, meta) = match open_regular_at(dir_fd, "actions.jsonl") {
        Ok(v) => v,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            *cache = ActionMeta { complete: true, ..ActionMeta::default() };
            return RefreshOutcome { last_used: None, actions: Some(0), needs_more: false, progressed: false };
        }
        Err(_) => {
            // Item 10 (F5): publish `actions: None` (NEVER `0`) on open errors (EACCES, FIFO, symlink).
            cache.complete = false;
            cache.read_failed = true;
            return RefreshOutcome { last_used: None, actions: None, needs_more: false, progressed: false };
        }
    };
    let (dev, ino, len, mtime_sec, mtime_nsec) = (meta.dev(), meta.ino(), meta.len(), meta.mtime(), meta.mtime_nsec());
    if len == 0 {
        cache.reset_for(dev, ino, mtime_sec, mtime_nsec);
        cache.complete = true;
        return RefreshOutcome { last_used: None, actions: Some(0), needs_more: false, progressed: false };
    }

    let same_inode = cache.dev == dev && cache.ino == ino;
    let same_mtime = cache.mtime_sec == mtime_sec && cache.mtime_nsec == mtime_nsec;
    let was_dirty = std::mem::take(&mut cache.dirty);

    // If a prior EIO failed on this exact unchanged file and no filesystem event
    // touched it, remain quiet until an event arrives (zero spin at rest!).
    if cache.read_failed && !was_dirty && same_inode && same_mtime && len == cache.eof_probe_len {
        return RefreshOutcome { last_used: None, actions: None, needs_more: false, progressed: false };
    }

    // Fast path: identical inode, size, complete, mtime, and no inotify write hint -> 0 bytes read.
    if !was_dirty && !cache.read_failed && same_inode && cache.complete && len == cache.cursor && same_mtime {
        let (lu, cnt) = cache.effective_summary();
        return RefreshOutcome { last_used: lu, actions: cnt, needs_more: false, progressed: false };
    }

    // Detect in-place rewrites on the same inode (Item 10 / Finding 21):
    // - Different inode, prior read failure, truncation (`len < cursor`), cursor == 0,
    //   or ambiguous same-size write (`len == cursor` when `was_dirty || !same_mtime`):
    //   ALWAYS reset & rescan from 0 so even a middle-only edit preserving first & last 64B is caught!
    // - Growth (`len > cursor`): verify head & cursor-boundary fingerprints (metered against budget!).
    let need_reset = if !same_inode || cache.read_failed || len <= cache.cursor || cache.cursor == 0 {
        true
    } else {
        match cache.fingerprint_matches(&f, budget_bytes) {
            Ok(true) => false,
            Ok(false) => true,
            Err(_) => {
                cache.complete = false;
                cache.read_failed = true;
                cache.eof_probe_len = len;
                cache.mtime_sec = mtime_sec;
                cache.mtime_nsec = mtime_nsec;
                return RefreshOutcome { last_used: None, actions: None, needs_more: false, progressed: false };
            }
        }
    };

    if need_reset {
        cache.reset_for(dev, ino, mtime_sec, mtime_nsec);
    }

    if cache.cursor >= len {
        cache.complete = true;
        cache.eof_last_used = None;
        cache.mtime_sec = mtime_sec;
        cache.mtime_nsec = mtime_nsec;
        let (lu, cnt) = cache.effective_summary();
        return RefreshOutcome { last_used: lu, actions: cnt, needs_more: false, progressed: false };
    }

    // If the file won't finish within the remaining budget, attempt ONE budgeted
    // O(1) tail probe for this `(len, mtime)` (charged directly to `*budget_bytes`!),
    // remembering inconclusive probes so they never repeat on subsequent slices (Item 10 / F20/F24).
    let file_changed_since_probe = !cache.eof_probe_done || cache.eof_probe_len != len || !same_mtime;
    let will_slice = (len - cache.cursor) > *budget_bytes;
    if will_slice && file_changed_since_probe && *budget_bytes > 0 {
        // Reserve at least half of `*budget_bytes` for forward scan progress while
        // allowing up to `584 KiB` (`8 + 64 + 512 KiB`) on a 2 MiB slice so records
        // up to 512 KiB finish on the initial O(1) tail probe without starving forward scan.
        let mut tail_budget = (*budget_bytes / 2).min(584 * 1024);
        let tail_before = tail_budget;
        let mut tail_read = 0u64;
        let probe = probe_last_used_from_file_budgeted(&mut f, len, &mut tail_budget, &mut tail_read);
        *budget_bytes = budget_bytes.saturating_sub(tail_before - tail_budget);
        #[cfg(test)]
        {
            cache.bytes_read_total = cache.bytes_read_total.saturating_add(tail_read);
        }
        cache.eof_probe_done = true;
        cache.eof_probe_len = len;
        match probe {
            BudgetedTailProbe::Value(ts) => cache.eof_last_used = Some(ts),
            BudgetedTailProbe::Absent | BudgetedTailProbe::Inconclusive | BudgetedTailProbe::Unreadable(_) => {
                cache.eof_last_used = None;
            }
        }
    }

    let to_read = (len - cache.cursor).min(*budget_bytes);
    if to_read == 0 {
        cache.complete = false;
        cache.mtime_sec = mtime_sec;
        cache.mtime_nsec = mtime_nsec;
        return RefreshOutcome { last_used: cache.eof_last_used.or(cache.last_used), actions: None, needs_more: true, progressed: false };
    }
    if f.seek(SeekFrom::Start(cache.cursor)).is_err() {
        cache.complete = false;
        cache.read_failed = true;
        cache.eof_probe_len = len;
        return RefreshOutcome { last_used: None, actions: None, needs_more: false, progressed: false };
    }

    let mut buf = [0u8; 64 * 1024];
    let mut rem = to_read;
    let mut bytes_advanced = 0u64;
    let mut read_err = false;
    while rem > 0 {
        let want = (buf.len() as u64).min(rem) as usize;
        let n = match f.read(&mut buf[..want]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                read_err = true;
                break;
            }
        };
        let chunk_start = cache.cursor;
        cache.update_fingerprints(chunk_start, &buf[..n]);
        cache.scanner.feed(&buf[..n], |ts| {
            cache.count = cache.count.saturating_add(1);
            cache.last_used = Some(ts);
        });
        cache.cursor = cache.cursor.saturating_add(n as u64);
        bytes_advanced = bytes_advanced.saturating_add(n as u64);
        rem -= n as u64;
        *budget_bytes = budget_bytes.saturating_sub(n as u64);
        #[cfg(test)]
        {
            cache.bytes_read_total = cache.bytes_read_total.saturating_add(n as u64);
        }
    }
    cache.mtime_sec = mtime_sec;
    cache.mtime_nsec = mtime_nsec;

    if read_err {
        // Item 10 (F24): DO NOT requeue with 0 ms poll timeout on `EIO`!
        cache.complete = false;
        cache.read_failed = true;
        cache.eof_probe_len = len;
        return RefreshOutcome { last_used: None, actions: None, needs_more: false, progressed: false };
    }

    // Re-check file length in case more bytes landed while we read.
    let cur_len = f.metadata().map_or(len, |m| m.len());
    if cache.cursor >= cur_len {
        cache.complete = true;
        cache.eof_last_used = None;
        let (lu, cnt) = cache.effective_summary();
        RefreshOutcome { last_used: lu, actions: cnt, needs_more: false, progressed: bytes_advanced > 0 }
    } else {
        cache.complete = false;
        // If `bytes_advanced == 0` despite `to_read > 0` (e.g. unexpected early EOF),
        // do NOT requeue into an infinite spin loop!
        RefreshOutcome { last_used: cache.eof_last_used.or(cache.last_used), actions: None, needs_more: bytes_advanced > 0, progressed: bytes_advanced > 0 }
    }
}

fn read_action_meta_budgeted(state_root: &Path, name: &str, cache: &mut ActionMeta, budget_bytes: &mut u64) -> RefreshOutcome {
    match open_verified_subdir(state_root, name) {
        Ok(dfd) => refresh_action_meta_budgeted(Some(&dfd), false, cache, budget_bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => refresh_action_meta_budgeted(None, true, cache, budget_bytes),
        Err(_) => refresh_action_meta_budgeted(None, false, cache, budget_bytes),
    }
}

fn read_status_with_meta(prefix_dir: &Path, installed: Option<String>, committed: Option<u64>) -> HostScreenStatus {
    // Round 1 agy-review F6: short-circuit when Olo isn't installed so we never
    // run the full ELF + `/usr/lib64` scan only to discard it.
    let not_set_up_reason = if installed.is_none() { Some(NOT_INSTALLED_REASON.to_owned()) } else { host_readiness(prefix_dir, installed.as_deref()).not_set_up_reason };
    HostScreenStatus { installed, committed, default_on_host: default_now() == "host", not_set_up_reason }
}

/// Reconcile the `/etc` (`ld.so.cache`) readiness watch with `installed.is_some()`
/// BEFORE probing `host_readiness`, avoiding watch thrash and ELF/`/usr/lib64`
/// scans whenever Olo is not installed (Round 1 agy-review F6 + Finding 23).
/// Returns the aggregated `ReconcileChanged` across every `watches.reconcile`
/// call so callers never miss screen directories created during an install or
/// uninstall transition (Round 2 confirming agy-review F2).
fn sync_readiness_watch_and_status(
    watches: &mut FollowWatches,
    runtime_dirs: &[PathBuf],
    state_root: &Path,
    prefix_dir: &Path,
    default_dir: &Path,
    watching_ld_cache: &mut bool,
    force_reconcile: bool,
) -> io::Result<(HostScreenStatus, ReconcileChanged)> {
    let (mut installed, mut committed) = read_installed_meta(prefix_dir);
    let mut want_ld = installed.is_some();
    let mut changed = ReconcileChanged::default();
    if force_reconcile || want_ld != *watching_ld_cache {
        *watching_ld_cache = want_ld;
        let rc = watches.reconcile(runtime_dirs, state_root, prefix_dir, default_dir, *watching_ld_cache)?;
        changed.merge(rc);
        if force_reconcile || rc.prefix {
            // Re-read metadata AFTER watches are in place (or stepped down onto
            // `prefix/share/olo`) so `version` or `release.json` written during
            // reconcile is never missed.
            (installed, committed) = read_installed_meta(prefix_dir);
            want_ld = installed.is_some();
            if want_ld != *watching_ld_cache {
                *watching_ld_cache = want_ld;
                let rc2 = watches.reconcile(runtime_dirs, state_root, prefix_dir, default_dir, *watching_ld_cache)?;
                changed.merge(rc2);
            }
        }
    }
    Ok((read_status_with_meta(prefix_dir, installed, committed), changed))
}

/// Deterministic `last_used` debouncer (at most 2/s per screen) factored out
/// with explicit `now: Instant` so tests can verify debouncing with a controlled
/// clock and zero sleeps (Item 11 / T3).
#[derive(Default)]
struct LastUsedDebouncer {
    last_emit: HashMap<String, Instant>,
    pending: HashSet<String>,
}

impl LastUsedDebouncer {
    fn mark_pending(&mut self, name: String) {
        self.pending.insert(name);
    }

    fn note_emitted(&mut self, name: String, now: Instant) {
        self.last_emit.insert(name.clone(), now);
        self.pending.remove(&name);
    }

    fn clear_pending(&mut self, name: &str) {
        self.pending.remove(name);
    }

    fn remove(&mut self, name: &str) {
        self.pending.remove(name);
        self.last_emit.remove(name);
    }

    fn next_timeout_ms(&self, now: Instant) -> i32 {
        let mut timeout_ms = -1i32;
        for name in &self.pending {
            let rem = match self.last_emit.get(name) {
                Some(at) => LAST_USED_INTERVAL.saturating_sub(now.saturating_duration_since(*at)),
                None => Duration::ZERO,
            };
            let ms = timeout_ms_ceil(rem);
            timeout_ms = if timeout_ms < 0 { ms } else { timeout_ms.min(ms) };
        }
        timeout_ms
    }

    fn take_due(&mut self, now: Instant) -> Vec<String> {
        let due: Vec<String> = self.pending.iter().filter(|name| self.last_emit.get(*name).is_none_or(|at| now.saturating_duration_since(*at) >= LAST_USED_INTERVAL)).cloned().collect();
        for name in &due {
            self.pending.remove(name);
        }
        due
    }
}

/// Pin `pid` (reusing an existing still-alive `pidfd` when available) BEFORE
/// verifying `check(pid)`, eliminating exit/PID-reuse races between discovery
/// and `pidfd_open`.
fn ensure_pinned(pid: i32, existing: &mut HashMap<i32, OwnedFd>, next: &mut HashMap<i32, OwnedFd>, check: impl FnOnce(i32) -> bool) -> bool {
    if next.contains_key(&pid) {
        return check(pid) && next.get(&pid).is_some_and(|fd| !pidfd_exited(fd));
    }
    if let Some(fd) = existing.remove(&pid) {
        if !pidfd_exited(&fd) && check(pid) && !pidfd_exited(&fd) {
            next.insert(pid, fd);
            return true;
        }
        return false;
    }
    match pin_live_pid(pid, check) {
        PinResult::Alive(fd) => {
            next.insert(pid, fd);
            true
        }
        PinResult::Exited => false,
        PinResult::Failed(e) => {
            eprintln!("olo screen follow: pidfd_open({pid}): {e}");
            false
        }
    }
}

/// Scan all running, stopped, and archived screens while pinning every live
/// session and controller PID via `pidfd_open` BEFORE accepting its row.
/// Uses `scan_budget` for counting large histories, queuing any screen that
/// needs additional slices onto `deferred_counts` so initial snapshots stay fast (Finding 24).
fn scan_screens(
    runtime_dirs: &[PathBuf],
    state_root: &Path,
    existing_pidfds: &mut HashMap<i32, OwnedFd>,
    action_cache: &mut HashMap<String, ActionMeta>,
    deferred_counts: &mut VecDeque<String>,
    mut scan_budget: u64,
) -> (BTreeMap<String, HostScreen>, HashMap<i32, OwnedFd>) {
    let mut running: HashMap<String, (PathBuf, RuntimeScreen)> = HashMap::new();
    let mut next_pidfds: HashMap<i32, OwnedFd> = HashMap::new();

    for rt in runtime_dirs {
        let Ok(rd) = std::fs::read_dir(rt) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            let Some(fname) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if !fname.ends_with(".json") || fname.ends_with("-settings.json") || fname.ends_with(".owner.json") {
                continue;
            }
            let Some(rt_screen) = read_runtime_screen(&path) else { continue };
            if !valid_name(&rt_screen.name) {
                continue;
            }
            if running.contains_key(&rt_screen.name) {
                continue;
            }
            let sname = rt_screen.name.clone();
            if ensure_pinned(rt_screen.pid, existing_pidfds, &mut next_pidfds, |p| screen_pid_running(&sname, p)) {
                running.insert(sname, (rt.clone(), rt_screen));
            }
        }
    }

    let mut names: std::collections::BTreeSet<String> = running.keys().cloned().collect();
    if let Ok(rd) = std::fs::read_dir(state_root) {
        for entry in rd.flatten() {
            // Finding 6: reject symlink screen directories atomically via
            // `file_type()` + `open_verified_subdir` (never follow symlinks!).
            if !entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                continue;
            }
            let Ok(name) = entry.file_name().into_string() else { continue };
            if !valid_name(&name) {
                continue;
            }
            if let Ok(dir_fd) = open_verified_subdir(state_root, &name)
                && (open_regular_at(&dir_fd, "actions.jsonl").is_ok() || open_regular_at(&dir_fd, "screen.json").is_ok())
            {
                names.insert(name);
            }
        }
    }

    let mut out = BTreeMap::new();
    let total_screens = names.len();
    for (idx, name) in names.into_iter().enumerate() {
        let (dir_fd, dir_missing) = match open_verified_subdir(state_root, &name) {
            Ok(fd) => (Some(fd), false),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (None, true),
            Err(_) => (None, false),
        };
        let title = dir_fd.as_ref().and_then(|fd| read_bounded_regular_at(fd, "title", 4096)).and_then(|b| String::from_utf8(b).ok()).map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
        // Fair-share the total scan budget across remaining screens so one huge
        // multi-MiB log never starves smaller screens out of the initial Snapshot,
        // while strictly capping ALL reads (tail probes, fingerprints, and forward
        // scans) at `scan_budget` (Item 10 / Findings 20 & 24).
        let rem_screens = (total_screens - idx) as u64;
        let mut per_screen = if scan_budget == 0 { 0 } else { (scan_budget / rem_screens).max(1).min(scan_budget) };
        let before = per_screen;
        let res = refresh_action_meta_budgeted(dir_fd.as_ref(), dir_missing, action_cache.entry(name.clone()).or_default(), &mut per_screen);
        scan_budget = scan_budget.saturating_sub(before - per_screen);
        if res.needs_more && !deferred_counts.iter().any(|n| n == &name) {
            deferred_counts.push_back(name.clone());
        }
        let (last_used, actions) = (res.last_used, res.actions);

        let rt_entry = running.get(&name);
        // Finding 54: for a running screen, validate owner against `(pid, started, start_ticks)`
        // so stale owner records from a previous session generation are discarded.
        let owner_record: Option<ScreenOwner> = match rt_entry {
            Some((rt, s)) => read_owner_for_running(rt, state_root, &name, s),
            None => runtime_dirs.iter().find_map(|rt| read_owner(rt, state_root, &name)),
        };
        let (owner, started_by) = match owner_record {
            Some(o) => (Some(o.pane()), o.started_by),
            None => (None, None),
        };

        let screen = if let Some((rt, s)) = rt_entry {
            let controlled = match read_control_meta(rt, &name) {
                Some((pid, expected_ticks)) => {
                    let active = ensure_pinned(pid, existing_pidfds, &mut next_pidfds, |p| is_controller_pid_alive_with_ticks(p, expected_ticks));
                    if !active {
                        try_cleanup_stale_control(rt, &name);
                    }
                    active
                }
                None => false,
            };
            HostScreen {
                name: name.clone(),
                title,
                state: HostScreenState::Running,
                width: s.width,
                height: s.height,
                scale: s.scale,
                sound: s.sound,
                pid: Some(s.pid),
                started: s.started,
                last_used,
                actions,
                controlled,
                owner,
                started_by,
            }
        } else {
            let kept: KeptOptions = dir_fd.as_ref().and_then(|fd| read_bounded_regular_at(fd, "screen.json", 16 * 1024)).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
            let state = if dir_fd.as_ref().is_some_and(|fd| exists_at(fd, "archived")) { HostScreenState::Archived } else { HostScreenState::Stopped };
            HostScreen {
                name: name.clone(),
                title,
                state,
                width: kept.width,
                height: kept.height,
                scale: kept.scale,
                sound: kept.sound,
                pid: None,
                started: None,
                last_used,
                actions,
                controlled: false,
                owner,
                started_by,
            }
        };
        out.insert(name, screen);
    }

    (out, next_pidfds)
}

/// Compare two [`HostScreen`] snapshots ignoring activity fields (`last_used`, `actions`).
fn same_except_activity(a: &HostScreen, b: &HostScreen) -> bool {
    a.name == b.name
        && a.title == b.title
        && a.state == b.state
        && a.width == b.width
        && a.height == b.height
        && a.scale == b.scale
        && a.sound == b.sound
        && a.pid == b.pid
        && a.started == b.started
        && a.controlled == b.controlled
        && a.owner == b.owner
        && a.started_by == b.started_by
}

/// Block SIGTERM/SIGINT/SIGHUP and return a non-blocking `signalfd` for clean
/// multiplexing inside `poll`.
fn setup_signals() -> Option<OwnedFd> {
    // SAFETY: standard POSIX signal setup before entering the single-threaded poll loop.
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

pub fn run(args: &[String]) -> ! {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("usage: olo-rex-link screen follow");
        std::process::exit(0);
    }
    if !args.is_empty() {
        eprintln!("olo screen follow: unexpected argument {:?}", args[0]);
        std::process::exit(2);
    }
    std::process::exit(follow_loop())
}

fn emit(ev: &FollowEvent, cancels: &[CancelFd]) -> bool {
    let Ok(mut line) = serde_json::to_vec(ev) else { return false };
    if line.len() + 1 > MAX_JSON_LINE {
        return false;
    }
    line.push(b'\n');
    write_all_cancellable(1, &line, cancels).is_ok()
}

/// Emit the initial `Snapshot`, keeping the serialized line strictly within
/// `MAX_JSON_LINE` by overflowing excess screens into subsequent `Screen` lines
/// if an unusually large host would otherwise exceed the line budget.
fn emit_snapshot(status: &HostScreenStatus, screens: &[HostScreen], cancels: &[CancelFd]) -> bool {
    let mut head: Vec<HostScreen> = Vec::new();
    let mut overflow: Vec<HostScreen> = Vec::new();
    let mut est_bytes = 256usize;
    for s in screens {
        let item_bytes = serde_json::to_vec(s).map_or(0, |b| b.len() + 2);
        if overflow.is_empty() && est_bytes + item_bytes <= MAX_JSON_LINE - 1024 {
            est_bytes += item_bytes;
            head.push(s.clone());
        } else {
            overflow.push(s.clone());
        }
    }
    if !emit(&FollowEvent::Snapshot { proto: PROTO, status: status.clone(), screens: head }, cancels) {
        return false;
    }
    for s in overflow {
        if !emit(&FollowEvent::Screen { screen: Box::new(s) }, cancels) {
            return false;
        }
    }
    true
}

fn follow_loop() -> i32 {
    let sigfd = setup_signals();
    set_nonblocking(0);
    set_nonblocking(1);
    let mut cancels = vec![CancelFd::StdinEof(0)];
    if let Some(s) = &sigfd {
        cancels.push(CancelFd::Readable(s.raw()));
    }

    let mut watches = match FollowWatches::new() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("olo screen follow: inotify: {e}");
            return 1;
        }
    };

    let prefix_dir = prefix();
    let default_dir = crate::screens::default_file().parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/"));
    let state_root = screens_state_dir();
    let runtime_dirs = match screens_runtime_dir_candidates_checked() {
        Ok(dirs) => dirs,
        Err(e) => {
            eprintln!("olo screen follow: unsafe runtime directory: {e}");
            return 1;
        }
    };

    // Item 10 (Finding 23) + agy-review F6: install all base watches and only
    // add `/etc` (and run `host_readiness`) when Olo is installed — adding `/etc`
    // BEFORE probing so a repair arriving mid-probe is never missed, while
    // skipping `/etc` watch thrash and ELF/`/usr/lib64` scans on uninstalled hosts.
    let mut watching_ld_cache = false;
    let (mut last_status, _init_rc) = match sync_readiness_watch_and_status(&mut watches, &runtime_dirs, &state_root, &prefix_dir, &default_dir, &mut watching_ld_cache, true) {
        Ok(res) => res,
        Err(e) => {
            eprintln!("olo screen follow: inotify watch failed: {e}");
            return 1;
        }
    };
    let mut pidfds: HashMap<i32, OwnedFd> = HashMap::new();
    let mut action_cache: HashMap<String, ActionMeta> = HashMap::new();
    let mut deferred_counts: VecDeque<String> = VecDeque::new();
    let (mut known, next_pidfds) = scan_screens(&runtime_dirs, &state_root, &mut pidfds, &mut action_cache, &mut deferred_counts, INITIAL_SCAN_BUDGET_BYTES);
    pidfds = next_pidfds;

    let init_list: Vec<HostScreen> = known.values().cloned().collect();
    if !emit_snapshot(&last_status, &init_list, &cancels) {
        return 0;
    }

    let mut debouncer = LastUsedDebouncer::default();
    let mut inotify_buf = vec![0u8; 32 * 1024];

    loop {
        // Compute earliest pending `last_used` debounce deadline (0 while slicing
        // deferred large histories, or -1 to sleep forever when idle).
        let now = Instant::now();
        let timeout_ms: i32 = if !deferred_counts.is_empty() { 0 } else { debouncer.next_timeout_ms(now) };

        let mut pfds = Vec::with_capacity(4 + pidfds.len());
        pfds.push(libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 });
        // Finding 35: poll stdout with `events: 0` so closing the output reader
        // wakes the idle loop immediately via `POLLERR | POLLHUP`.
        pfds.push(libc::pollfd { fd: 1, events: 0, revents: 0 });
        pfds.push(libc::pollfd { fd: watches.fd(), events: libc::POLLIN, revents: 0 });
        let sig_idx = sigfd.as_ref().map(|s| {
            let idx = pfds.len();
            pfds.push(libc::pollfd { fd: s.raw(), events: libc::POLLIN, revents: 0 });
            idx
        });
        let tracked_pids: Vec<i32> = pidfds.keys().copied().collect();
        let pid_base = pfds.len();
        for pid in &tracked_pids {
            if let Some(fd) = pidfds.get(pid) {
                pfds.push(libc::pollfd { fd: fd.raw(), events: libc::POLLIN, revents: 0 });
            }
        }

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

        // 2. Stdin EOF / hangup: exit immediately.
        if pfds[0].revents != 0 {
            let mut sink = [0u8; 256];
            loop {
                // SAFETY: non-blocking read from stdin into `sink`.
                let got = unsafe { libc::read(0, sink.as_mut_ptr().cast(), sink.len()) };
                if got == 0 {
                    return 0;
                }
                if got < 0 {
                    let err = io::Error::last_os_error();
                    if err.kind() == io::ErrorKind::WouldBlock || err.kind() == io::ErrorKind::Interrupted {
                        break;
                    }
                    return 0;
                }
            }
            if pfds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                return 0;
            }
        }

        let mut need_reconcile = false;
        let mut need_readiness = false;
        let mut need_default = false;
        let mut need_screens = false;

        // 3. Drain inotify events and classify what actually changed.
        if pfds[2].revents != 0 {
            loop {
                // SAFETY: `watches.fd()` is non-blocking and `inotify_buf` is valid.
                let got = unsafe { libc::read(watches.fd(), inotify_buf.as_mut_ptr().cast(), inotify_buf.len()) };
                if got <= 0 {
                    break;
                }
                let mut at = 0usize;
                while at + std::mem::size_of::<libc::inotify_event>() <= got as usize {
                    // SAFETY: kernel writes whole `inotify_event` structs into `inotify_buf`.
                    let ev = unsafe { std::ptr::read_unaligned(inotify_buf.as_ptr().add(at).cast::<libc::inotify_event>()) };
                    let name_at = at + std::mem::size_of::<libc::inotify_event>();
                    let name_end = (name_at + ev.len as usize).min(got as usize);
                    let name = std::ffi::CStr::from_bytes_until_nul(&inotify_buf[name_at..name_end]).ok().and_then(|c| c.to_str().ok()).unwrap_or("");
                    at = name_end;

                    if ev.mask & libc::IN_Q_OVERFLOW != 0 {
                        if let Err(e) = watches.rebuild() {
                            eprintln!("olo screen follow: inotify rebuild after overflow: {e}");
                            return 1;
                        }
                        // Finding 21: invalidate all action caches on overflow.
                        action_cache.clear();
                        deferred_counts.clear();
                        need_reconcile = true;
                        need_readiness = true;
                        need_default = true;
                        need_screens = true;
                        break;
                    }
                    if ev.mask & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_IGNORED) != 0 {
                        let already_ignored = ev.mask & libc::IN_IGNORED != 0;
                        if already_ignored && !watches.by_wd.contains_key(&ev.wd) {
                            continue;
                        }
                        if let Some(path) = watches.by_wd.get(&ev.wd).cloned()
                            && let Some(entry) = watches.by_path.get(&path)
                        {
                            let mut rc = ReconcileChanged::default();
                            rc.note_roles(&entry.roles);
                            need_screens |= rc.screens;
                            need_readiness |= rc.prefix;
                            need_default |= rc.default_dir;
                        }
                        watches.remove_and_forget_wd(ev.wd, already_ignored);
                        need_reconcile = true;
                        continue;
                    }
                    let Some(path) = watches.by_wd.get(&ev.wd).cloned() else { continue };
                    let Some(entry) = watches.by_path.get(&path) else { continue };
                    for role in &entry.roles {
                        match role {
                            WatchRole::Ancestor { want_child, kind } => {
                                if want_child.is_empty() || name == want_child {
                                    need_reconcile = true;
                                    match kind {
                                        TargetKind::PrefixRoot | TargetKind::PrefixBin | TargetKind::PrefixShare | TargetKind::SystemLdCache => {
                                            need_readiness = true;
                                        }
                                        TargetKind::DefaultDir => {
                                            need_default = true;
                                        }
                                        TargetKind::RuntimeDir | TargetKind::StateRoot | TargetKind::StateScreen(_) => {
                                            need_screens = true;
                                        }
                                    }
                                }
                            }
                            WatchRole::Target(TargetKind::RuntimeDir) => {
                                if matches!(name, "" | "logs" | "views" | "no-sound") || name.ends_with(".tmp") || name.ends_with("-settings.json") || name.ends_with(".bus") {
                                    continue;
                                }
                                if name.ends_with(".json") || name.ends_with(".control") {
                                    need_screens = true;
                                }
                            }
                            WatchRole::Target(TargetKind::StateRoot) => {
                                if !name.is_empty() {
                                    need_reconcile = true;
                                    need_screens = true;
                                }
                            }
                            WatchRole::Target(TargetKind::StateScreen(screen_name)) => match name {
                                "actions.jsonl" => {
                                    if ev.mask & (libc::IN_DELETE | libc::IN_MOVED_FROM | libc::IN_CREATE | libc::IN_MOVED_TO) != 0 || !known.contains_key(screen_name) {
                                        // Finding 21: invalidate cache on deletion/replacement/recreate.
                                        action_cache.remove(screen_name);
                                        need_screens = true;
                                    } else {
                                        if let Some(meta) = action_cache.get_mut(screen_name) {
                                            meta.dirty = true;
                                        }
                                        debouncer.mark_pending(screen_name.clone());
                                    }
                                }
                                "title" | "archived" | "screen.json" | "owner.json" => {
                                    need_screens = true;
                                }
                                _ => {}
                            },
                            WatchRole::Target(TargetKind::PrefixRoot) => {
                                if matches!(name, "bin" | "share") {
                                    need_reconcile = true;
                                    need_readiness = true;
                                }
                            }
                            WatchRole::Target(TargetKind::PrefixBin) => {
                                if !name.is_empty() {
                                    need_readiness = true;
                                }
                            }
                            WatchRole::Target(TargetKind::PrefixShare) => {
                                if matches!(name, "version" | "release.json") {
                                    need_readiness = true;
                                }
                            }
                            WatchRole::Target(TargetKind::DefaultDir) => {
                                if name == "screens-on" {
                                    need_default = true;
                                }
                            }
                            WatchRole::Target(TargetKind::SystemLdCache) => {
                                if matches!(name, "ld.so.cache" | "ld.so.conf") {
                                    need_readiness = true;
                                }
                            }
                        }
                    }
                }
            }
        }

        // 4. Exited screen session or controller PIDs (instant wake via pidfd).
        for (i, pid) in tracked_pids.iter().enumerate() {
            if pfds[pid_base + i].revents != 0 {
                pidfds.remove(pid);
                need_screens = true;
            }
        }

        // 5. Reconcile directory watches before rescanning so newly created
        //    directories don't miss follow-up writes (Finding 23: only trigger
        //    readiness probe when prefix/library watches changed!).
        if need_reconcile {
            match watches.reconcile(&runtime_dirs, &state_root, &prefix_dir, &default_dir, watching_ld_cache) {
                Ok(rc) => {
                    need_screens |= rc.screens;
                    need_readiness |= rc.prefix;
                    need_default |= rc.default_dir;
                }
                Err(e) => {
                    eprintln!("olo screen follow: inotify watch failed: {e}");
                    return 1;
                }
            }
        }

        // 6. Status update: only re-run ELF/library scan on prefix/ld.so.cache events
        //    when Olo is installed; never thrash `/etc` watch or scan `/usr/lib64`
        //    while uninstalled or unpacking binaries (Item 10 / Finding 23 + agy-review F6).
        if need_readiness || need_default {
            let status = if need_readiness {
                match sync_readiness_watch_and_status(&mut watches, &runtime_dirs, &state_root, &prefix_dir, &default_dir, &mut watching_ld_cache, false) {
                    Ok((s, rc)) => {
                        need_screens |= rc.screens;
                        s
                    }
                    Err(e) => {
                        eprintln!("olo screen follow: inotify watch failed: {e}");
                        return 1;
                    }
                }
            } else {
                let mut s = last_status.clone();
                s.default_on_host = default_now() == "host";
                s
            };
            if status != last_status {
                if !emit(&FollowEvent::Status { status: status.clone() }, &cancels) {
                    return 0;
                }
                last_status = status;
            }
        }

        // 7. Full screen rescan when structure/state/metadata changed.
        if need_screens {
            let (now_screens, next_pidfds) = scan_screens(&runtime_dirs, &state_root, &mut pidfds, &mut action_cache, &mut deferred_counts, INITIAL_SCAN_BUDGET_BYTES);
            pidfds = next_pidfds;

            // Screens removed completely.
            let gone: Vec<String> = known.keys().filter(|k| !now_screens.contains_key(*k)).cloned().collect();
            for name in gone {
                known.remove(&name);
                action_cache.remove(&name);
                debouncer.remove(&name);
                deferred_counts.retain(|n| n != &name);
                if !emit(&FollowEvent::Gone { name }, &cancels) {
                    return 0;
                }
            }

            // Screens added or changed.
            let now_inst = Instant::now();
            for (name, new_s) in now_screens {
                match known.get(&name) {
                    None => {
                        if !emit(&FollowEvent::Screen { screen: Box::new(new_s.clone()) }, &cancels) {
                            return 0;
                        }
                        if new_s.last_used.is_some() {
                            debouncer.note_emitted(name.clone(), now_inst);
                        } else {
                            debouncer.clear_pending(&name);
                        }
                        known.insert(name, new_s);
                    }
                    Some(old_s) if old_s == &new_s => {}
                    Some(old_s) if same_except_activity(old_s, &new_s) => {
                        let monotonic = new_s.last_used.is_some()
                            && new_s.actions.is_some()
                            && old_s.actions.is_none_or(|prev| new_s.actions.is_some_and(|c| c >= prev))
                            && old_s.last_used.is_none_or(|prev| new_s.last_used.is_some_and(|cur| cur >= prev));
                        if monotonic {
                            debouncer.mark_pending(name.clone());
                        } else {
                            if !emit(&FollowEvent::Screen { screen: Box::new(new_s.clone()) }, &cancels) {
                                return 0;
                            }
                            debouncer.clear_pending(&name);
                            known.insert(name, new_s);
                        }
                    }
                    Some(_) => {
                        if !emit(&FollowEvent::Screen { screen: Box::new(new_s.clone()) }, &cancels) {
                            return 0;
                        }
                        if new_s.last_used.is_some() {
                            debouncer.note_emitted(name.clone(), now_inst);
                        } else {
                            debouncer.clear_pending(&name);
                        }
                        known.insert(name, new_s);
                    }
                }
            }
        }

        // 8. Drain one bounded slice of any deferred large-history count (Item 10 / Findings 8, 21, 24).
        //    Require actual progress (`res.progressed`) to requeue so EIO never spins at rest,
        //    and emit a full `Screen` event whenever `last_used` decreases or `actions` shrinks/becomes None!
        if let Some(name) = deferred_counts.pop_front()
            && let Some(cur) = known.get_mut(&name)
        {
            let mut budget = SLICE_SCAN_BUDGET_BYTES;
            let res = read_action_meta_budgeted(&state_root, &name, action_cache.entry(name.clone()).or_default(), &mut budget);
            if res.needs_more && res.progressed {
                deferred_counts.push_back(name);
            } else if res.last_used != cur.last_used || res.actions != cur.actions {
                let decreased = res.last_used.is_none()
                    || cur.last_used.is_some_and(|prev| res.last_used.is_some_and(|l| l < prev))
                    || res.actions.is_none()
                    || cur.actions.is_some_and(|prev| res.actions.is_some_and(|c| c < prev));
                cur.last_used = res.last_used;
                cur.actions = res.actions;
                if !decreased && let (Some(ts), Some(cnt)) = (res.last_used, res.actions) {
                    if !emit(&FollowEvent::LastUsed { name: name.clone(), last_used: ts, actions: Some(cnt) }, &cancels) {
                        return 0;
                    }
                    debouncer.note_emitted(name, Instant::now());
                } else if !emit(&FollowEvent::Screen { screen: Box::new(cur.clone()) }, &cancels) {
                    return 0;
                }
            }
        }

        // 9. Flush any debounced activity (`last_used` + `actions`) updates that are now due (at most 2/s per screen).
        let now_inst = Instant::now();
        for name in debouncer.take_due(now_inst) {
            let Some(cur) = known.get_mut(&name) else { continue };
            let mut budget = SLICE_SCAN_BUDGET_BYTES;
            let res = read_action_meta_budgeted(&state_root, &name, action_cache.entry(name.clone()).or_default(), &mut budget);
            if res.needs_more {
                if res.progressed && !deferred_counts.iter().any(|n| n == &name) {
                    deferred_counts.push_back(name.clone());
                }
                // Item 10 (8, 21, 24): publish `actions: None` while rescanning and emit a full
                // `Screen` if `last_used` decreased or `actions` changed to `None`!
                if res.last_used != cur.last_used || cur.actions.is_some() {
                    let decreased = res.last_used.is_none() || cur.last_used.is_some_and(|prev| res.last_used.is_some_and(|l| l < prev)) || cur.actions.is_some();
                    cur.last_used = res.last_used;
                    cur.actions = None;
                    if decreased {
                        if !emit(&FollowEvent::Screen { screen: Box::new(cur.clone()) }, &cancels) {
                            return 0;
                        }
                        debouncer.note_emitted(name, now_inst);
                    } else if let Some(ts) = res.last_used {
                        if !emit(&FollowEvent::LastUsed { name: name.clone(), last_used: ts, actions: None }, &cancels) {
                            return 0;
                        }
                        debouncer.note_emitted(name, now_inst);
                    }
                }
                continue;
            }
            if res.last_used != cur.last_used || res.actions != cur.actions {
                let shrank = res.last_used.is_none()
                    || cur.last_used.is_some_and(|prev| res.last_used.is_some_and(|l| l < prev))
                    || res.actions.is_none()
                    || cur.actions.is_some_and(|prev| res.actions.is_some_and(|c| c < prev));
                cur.last_used = res.last_used;
                cur.actions = res.actions;
                if !shrank && let Some(ts) = res.last_used {
                    if !emit(&FollowEvent::LastUsed { name: name.clone(), last_used: ts, actions: res.actions }, &cancels) {
                        return 0;
                    }
                    debouncer.note_emitted(name, now_inst);
                } else if !emit(&FollowEvent::Screen { screen: Box::new(cur.clone()) }, &cancels) {
                    return 0;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// Round 3b Findings 20, 21, 23:
    /// - 2.5 MiB single record with `"at"` after 2.25 MiB payload parses accurately.
    /// - Unfinished partial line split across 4 writes + final `\n` reads EXACTLY
    ///   the file's byte length total (zero repeated rereads!).
    /// - Same-inode same-size and same-inode larger in-place rewrites are detected.
    /// - Creating a new screen directory sets `ReconcileChanged.screens = true, prefix = false`
    ///   (never triggering a readiness ELF rescan!).
    #[test]
    fn round3b_f20_f21_f23_streaming_zero_reread_rewrite_and_reconcile_isolation() {
        let base = PathBuf::from(format!("/var/tmp/olo-r3b-follow-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let state_root = base.join("state/olo/screens");
        let sdir = state_root.join("stream_u");
        std::fs::create_dir_all(&sdir).unwrap();
        let act_path = sdir.join("actions.jsonl");
        let dfd = open_verified_subdir(&state_root, "stream_u").unwrap();

        // 1. Finding 20: 512 KiB unfinished line split across 4 writes -> zero repeated rereads!
        let mut cache = ActionMeta::default();
        let part1 = format!("{{\"args\":[\"{}", "a".repeat(128 * 1024));
        let part2 = "b".repeat(128 * 1024);
        let part3 = "c".repeat(128 * 1024);
        let part4 = format!("{}\"],\"at\":55555,\"ms\":45,\"kind\":\"type\"}}\n", "d".repeat(128 * 1024));
        let total_len = (part1.len() + part2.len() + part3.len() + part4.len()) as u64;
        for (idx, chunk) in [&part1, &part2, &part3, &part4].iter().enumerate() {
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&act_path).unwrap();
            f.write_all(chunk.as_bytes()).unwrap();
            let mut budget = 2 * 1024 * 1024;
            let res = refresh_action_meta_budgeted(Some(&dfd), false, &mut cache, &mut budget);
            assert!(!res.needs_more);
            if idx < 3 {
                assert_eq!(res.actions, Some(0), "incomplete line chunk {idx} must not count yet");
            } else {
                assert_eq!(res.actions, Some(1), "completed 512 KiB line must count as 1");
                assert_eq!(res.last_used, Some(55_600));
            }
        }
        // T3: ALL reads including the 3×128-byte head+tail fingerprint checks are metered!
        assert_eq!(cache.bytes_read_total, total_len + 3 * 128, "F20/T3: partial writes reread bytes ({} != {})", cache.bytes_read_total, total_len + 3 * 128);

        // 2. Finding 21: same-inode same-size in-place rewrite & larger in-place rewrite.
        let two = "{\"at\":1000,\"ms\":10,\"kind\":\"click\"}\n{\"at\":2000,\"ms\":10,\"kind\":\"click\"}\n";
        std::fs::write(&act_path, two).unwrap();
        let mut rw_cache = ActionMeta::default();
        let mut budget = 1024 * 1024;
        let r0 = refresh_action_meta_budgeted(Some(&dfd), false, &mut rw_cache, &mut budget);
        assert_eq!((r0.last_used, r0.actions, r0.needs_more), (Some(2010), Some(2), false));

        // Same-size rewrite on same inode (bump mtime via truncate+write).
        let pfx = "{\"at\":9000,\"ms\":5,\"kind\":\"click\",\"args\":[\"";
        let sfx = "\"]}\n";
        let same_size = format!("{pfx}{}{sfx}", "x".repeat(two.len() - pfx.len() - sfx.len()));
        assert_eq!(same_size.len(), two.len());
        {
            let mut f = std::fs::OpenOptions::new().write(true).truncate(true).open(&act_path).unwrap();
            f.write_all(same_size.as_bytes()).unwrap();
        }
        // Force a distinct mtime so coarse filesystem clocks never hide the in-place write.
        rw_cache.mtime_sec -= 1;
        let mut budget = 1024 * 1024;
        let r1 = refresh_action_meta_budgeted(Some(&dfd), false, &mut rw_cache, &mut budget);
        assert_eq!((r1.last_used, r1.actions, r1.needs_more), (Some(9005), Some(1), false));

        // Larger in-place rewrite on same inode (header changes from at:9000 -> at:8000, larger file).
        let larger = format!("{{\"at\":8000,\"ms\":1,\"kind\":\"type\",\"args\":[\"{}\"]}}\n", "z".repeat(200));
        assert!(larger.len() > same_size.len());
        {
            let mut f = std::fs::OpenOptions::new().write(true).truncate(true).open(&act_path).unwrap();
            f.write_all(larger.as_bytes()).unwrap();
        }
        let mut budget = 1024 * 1024;
        let r2 = refresh_action_meta_budgeted(Some(&dfd), false, &mut rw_cache, &mut budget);
        assert_eq!((r2.last_used, r2.actions, r2.needs_more), (Some(8001), Some(1), false));

        // 3. Finding 23: creating a new screen dir changes ONLY `screens`, NOT `prefix`!
        let rt_dir = base.join("run/olo-screens");
        let prefix_dir = base.join("desktop");
        let default_dir = base.join("config/olo");
        std::fs::create_dir_all(&rt_dir).unwrap();
        std::fs::create_dir_all(prefix_dir.join("bin")).unwrap();
        std::fs::create_dir_all(prefix_dir.join("share/olo")).unwrap();
        std::fs::create_dir_all(&default_dir).unwrap();
        let mut watches = FollowWatches::new().unwrap();
        let _ = watches.reconcile(std::slice::from_ref(&rt_dir), &state_root, &prefix_dir, &default_dir, false).unwrap();
        std::fs::create_dir_all(state_root.join("brand_new_screen")).unwrap();
        let rc = watches.reconcile(std::slice::from_ref(&rt_dir), &state_root, &prefix_dir, &default_dir, false).unwrap();
        assert!(rc.screens, "F23: new screen dir must mark screens changed");
        assert!(!rc.prefix, "F23: new screen dir must NOT trigger prefix readiness rescan!");
        assert!(!rc.default_dir);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Final Item 10 & 11 (20, 21, 24, T3):
    /// - 12 KiB middle-only same-size in-place edit preserving first 64B and last 64B MUST be detected.
    /// - Initial 2 MiB budget MUST meter and cap tail probes & fingerprints AND remember inconclusive probes.
    /// - Open/read errors publish `actions: None` (never `0`) and `needs_more == false` (no spin!).
    /// - Controlled-clock `LastUsedDebouncer` enforces 2/s cap deterministically with zero sleeps.
    #[test]
    fn final_item10_follow_middle_edit_and_strict_budget_metering() {
        let base = PathBuf::from(format!("/var/tmp/olo-final-follow-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let state_root = base.join("state/olo/screens");
        let sdir = state_root.join("mid_u");
        std::fs::create_dir_all(&sdir).unwrap();
        let act_path = sdir.join("actions.jsonl");
        let dfd = open_verified_subdir(&state_root, "mid_u").unwrap();

        // 1. 12 KiB middle-only in-place rewrite preserving first 64B and last 64B.
        let mut prefix_rows = String::new();
        for i in 0..40 {
            prefix_rows.push_str(&format!("{{\"at\":{},\"ms\":10,\"kind\":\"click\",\"args\":[\"row-{i:04}-padding-0123456789abcdef\"]}}\n", 1000 + i));
        }
        let tail_pad = "Z".repeat(10 * 1024);
        let v1 = format!("{prefix_rows}{{\"at\":5000,\"ms\":10,\"kind\":\"click\",\"args\":[\"{tail_pad}\"]}}\n");
        let v2 = format!("{prefix_rows}{{\"at\":9999,\"ms\":10,\"kind\":\"click\",\"args\":[\"{tail_pad}\"]}}\n");
        assert_eq!(v1.len(), v2.len());
        assert!(v1.len() > 12 * 1024);
        assert_eq!(&v1.as_bytes()[..64], &v2.as_bytes()[..64]);
        assert_eq!(&v1.as_bytes()[v1.len() - 64..], &v2.as_bytes()[v2.len() - 64..]);

        std::fs::write(&act_path, &v1).unwrap();
        let mut cache = ActionMeta::default();
        let mut budget = 1024 * 1024;
        let res1 = refresh_action_meta_budgeted(Some(&dfd), false, &mut cache, &mut budget);
        assert_eq!((res1.last_used, res1.actions, res1.needs_more), (Some(5010), Some(41), false));

        // Pure in-place middle-only edit (same inode, same size, same head 64B & tail 64B), marked dirty!
        {
            let mut f = std::fs::OpenOptions::new().write(true).open(&act_path).unwrap();
            f.write_all(v2.as_bytes()).unwrap();
        }
        cache.dirty = true;
        let mut budget = 1024 * 1024;
        let res2 = refresh_action_meta_budgeted(Some(&dfd), false, &mut cache, &mut budget);
        assert!(!res2.needs_more);
        assert_eq!(res2.actions, Some(41));
        assert_eq!(res2.last_used, Some(10_009), "Item 10 (F21): 12 KiB middle-only in-place rewrite preserving head/tail 64B was missed!");

        // 2. Tight budget caps ALL reads (tail probe + fingerprints + forward scan) AND remembers inconclusive probe!
        let huge_dir = state_root.join("huge_tail");
        std::fs::create_dir_all(&huge_dir).unwrap();
        let huge_act = huge_dir.join("actions.jsonl");
        {
            let mut f = std::fs::File::create(&huge_act).unwrap();
            f.write_all(b"{\"args\":[\"").unwrap();
            let blk = vec![b'Q'; 64 * 1024];
            for _ in 0..48 {
                f.write_all(&blk).unwrap(); // 3 MiB single record
            }
            f.write_all(b"\"],\"at\":70000,\"ms\":5,\"kind\":\"type\"}\n").unwrap();
        }
        let hdfd = open_verified_subdir(&state_root, "huge_tail").unwrap();
        let mut hcache = ActionMeta::default();
        let mut tight_budget: u64 = 128 * 1024;
        let h1 = refresh_action_meta_budgeted(Some(&hdfd), false, &mut hcache, &mut tight_budget);
        assert!(h1.needs_more && h1.progressed);
        assert_eq!(h1.actions, None);
        assert!(hcache.eof_probe_done, "Item 10 (F20/F24): inconclusive tail probe must be remembered");
        assert!(hcache.bytes_read_total <= 128 * 1024, "Item 10 (F20/F24): tail probe + forward scan read {} bytes exceeding 128 KiB budget!", hcache.bytes_read_total);
        assert_eq!(tight_budget, 0, "Item 10 (F20/F24): tight budget must be fully accounted for");

        // Second slice with 128 KiB budget MUST NOT re-run the inconclusive tail probe!
        let read_before = hcache.bytes_read_total;
        let cur_before = hcache.cursor;
        let mut slice2_budget: u64 = 128 * 1024;
        let h2 = refresh_action_meta_budgeted(Some(&hdfd), false, &mut hcache, &mut slice2_budget);
        assert!(h2.needs_more && h2.progressed);
        let delta_read = hcache.bytes_read_total - read_before;
        let delta_cursor = hcache.cursor - cur_before;
        assert!(delta_read <= delta_cursor + 128, "Item 10 (F20/F24): second slice re-ran tail probe! delta_read={delta_read}, delta_cursor={delta_cursor}");

        // 3. Long invalid tail (200 KiB of invalid non-JSON): completes cleanly with 0 valid actions and NEVER spins.
        let inv_dir = state_root.join("long_inv");
        std::fs::create_dir_all(&inv_dir).unwrap();
        std::fs::write(inv_dir.join("actions.jsonl"), format!("{{\"at\":100,\"ms\":5,\"kind\":\"click\"}}\n{}", "not-json".repeat(26 * 1024))).unwrap();
        let idfd = open_verified_subdir(&state_root, "long_inv").unwrap();
        let mut icache = ActionMeta::default();
        let mut ibudget: u64 = 64 * 1024;
        let i1 = refresh_action_meta_budgeted(Some(&idfd), false, &mut icache, &mut ibudget);
        assert!(icache.eof_probe_done);
        assert!(icache.bytes_read_total <= 64 * 1024);
        assert!(i1.needs_more && i1.progressed);
        let mut ibudget2: u64 = 512 * 1024;
        let i2 = refresh_action_meta_budgeted(Some(&idfd), false, &mut icache, &mut ibudget2);
        assert!(!i2.needs_more);
        assert_eq!(i2.actions, Some(1));

        // 4. Open error (FIFO / unreadable) publishes `actions: None` (NEVER `0`) and `needs_more: false` (no spin!).
        let err_dir = state_root.join("fifo_act");
        std::fs::create_dir_all(&err_dir).unwrap();
        let fifo_p = err_dir.join("actions.jsonl");
        let cfifo = CString::new(fifo_p.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cfifo.as_ptr(), 0o600) }, 0);
        let edfd = open_verified_subdir(&state_root, "fifo_act").unwrap();
        let mut ecache = ActionMeta::default();
        let mut ebudget = 64 * 1024;
        let er = refresh_action_meta_budgeted(Some(&edfd), false, &mut ecache, &mut ebudget);
        assert_eq!(er.actions, None, "Item 10 (F5): open error must publish actions: None, not 0");
        assert!(!er.needs_more && !er.progressed, "Item 10 (F24): error must never requeue into spin loop");

        // 5. Controlled-clock `LastUsedDebouncer` test (T3: deterministic, zero sleeps!).
        let mut deb = LastUsedDebouncer::default();
        let t0 = Instant::now();
        assert_eq!(deb.next_timeout_ms(t0), -1);
        deb.mark_pending("qa".into());
        assert_eq!(deb.next_timeout_ms(t0), 0);
        assert_eq!(deb.take_due(t0), vec!["qa".to_owned()]);
        deb.note_emitted("qa".into(), t0);
        deb.mark_pending("qa".into());
        assert_eq!(deb.next_timeout_ms(t0 + Duration::from_millis(100)), 400);
        assert!(deb.take_due(t0 + Duration::from_millis(499)).is_empty());
        assert_eq!(deb.next_timeout_ms(t0 + Duration::from_millis(499)), 1);
        assert_eq!(deb.take_due(t0 + Duration::from_millis(500)), vec!["qa".to_owned()]);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Round 1 agy-review Finding 6: uninstalled host (`share/olo/version` absent)
    /// MUST NOT run the full ELF + `/usr/lib64` `host_readiness` scan or add/thrash
    /// an `/etc` inotify watch (neither at startup nor while unpacking binaries).
    #[test]
    fn agy_r1_f6_uninstalled_follow_skips_host_readiness_and_etc_watch() {
        let base = PathBuf::from(format!("/var/tmp/olo-agy-r1-f6-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let prefix = base.join("desktop");
        let rt_dir = base.join("run/olo-screens");
        let state_root = base.join("state/olo/screens");
        let default_dir = base.join("config/olo");
        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        std::fs::create_dir_all(&rt_dir).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        std::fs::create_dir_all(&default_dir).unwrap();
        // Unpack `bin/olo-screen` BEFORE `share/olo/version` exists (mid-install state!).
        std::fs::write(prefix.join("bin/olo-screen"), b"not-yet-installed").unwrap();

        let mut watches = FollowWatches::new().unwrap();
        let mut watching_ld_cache = false;
        let rts = vec![rt_dir];
        let calls = || crate::screens::HOST_READINESS_CALLS.with(std::cell::Cell::get);

        // 1. Startup on uninstalled host: 0 readiness scans, NO `/etc` watch installed!
        let before = calls();
        let (st1, _rc1) = sync_readiness_watch_and_status(&mut watches, &rts, &state_root, &prefix, &default_dir, &mut watching_ld_cache, true).unwrap();
        let after1 = calls();
        assert_eq!(st1.installed, None);
        assert_eq!(st1.not_set_up_reason.as_deref(), Some(NOT_INSTALLED_REASON));
        assert_eq!(after1 - before, 0, "agy-r1 F6: startup ran host_readiness on uninstalled host!");
        assert!(!watching_ld_cache, "agy-r1 F6: watching_ld_cache must stay false while uninstalled");
        assert!(!watches.by_path.contains_key(Path::new("/etc")), "agy-r1 F6: /etc watch must NOT be added while uninstalled");

        // 2. Unpacking additional binaries while still uninstalled: still 0 scans & NO `/etc` watch!
        std::fs::write(prefix.join("bin/app_server"), b"unpacking").unwrap();
        let (st2, rc2) = sync_readiness_watch_and_status(&mut watches, &rts, &state_root, &prefix, &default_dir, &mut watching_ld_cache, false).unwrap();
        let after2 = calls();
        assert_eq!(st2.installed, None);
        assert_eq!(rc2, ReconcileChanged::default());
        assert_eq!(after2 - before, 0, "agy-r1 F6: unpacking binaries while uninstalled ran host_readiness!");
        assert!(!watching_ld_cache && !watches.by_path.contains_key(Path::new("/etc")));

        // 3. Install completes (`share/olo/version` appears): `/etc` watch IS added and `host_readiness` runs ONCE!
        std::fs::create_dir_all(prefix.join("share/olo")).unwrap();
        std::fs::write(prefix.join("share/olo/version"), "2026.11.27-test\n").unwrap();
        let (st3, rc3) = sync_readiness_watch_and_status(&mut watches, &rts, &state_root, &prefix, &default_dir, &mut watching_ld_cache, false).unwrap();
        let after3 = calls();
        assert_eq!(st3.installed.as_deref(), Some("2026.11.27-test"));
        assert!(rc3.prefix, "agy-r2 F2: install transition must report prefix watch change");
        assert_eq!(after3 - before, 1, "agy-r1 F6: installed host should run host_readiness once");
        assert!(watching_ld_cache && watches.by_path.contains_key(Path::new("/etc")), "agy-r1 F6: /etc watch must be installed when Olo is installed");

        // 4. Uninstall (`share/olo/version` removed): `/etc` watch removed cleanly with 0 extra scans!
        std::fs::remove_file(prefix.join("share/olo/version")).unwrap();
        let (st4, rc4) = sync_readiness_watch_and_status(&mut watches, &rts, &state_root, &prefix, &default_dir, &mut watching_ld_cache, false).unwrap();
        let after4 = calls();
        assert_eq!(st4.installed, None);
        assert!(rc4.prefix, "agy-r2 F2: uninstall transition must report prefix watch change");
        assert_eq!(after4 - after3, 0, "agy-r1 F6: uninstall must not run host_readiness");
        assert!(!watching_ld_cache && !watches.by_path.contains_key(Path::new("/etc")), "agy-r1 F6: /etc watch must be removed on uninstall");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Round 2 confirming agy-review Finding 2: `sync_readiness_watch_and_status`
    /// MUST return aggregated `ReconcileChanged` across every `watches.reconcile`
    /// call so screen directories created during an install/uninstall transition
    /// propagate `rc.screens == true` into step 7's `need_screens` rescan (even
    /// after ancestor watches step down onto the new child directories).
    #[test]
    fn agy_r2_f2_sync_readiness_propagates_reconcile_changed_on_install_and_uninstall() {
        let base = PathBuf::from(format!("/var/tmp/olo-agy-r2-f2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let run_root = base.join("run");
        let rt_dir = run_root.join("olo-screens");
        let tabula_rt_dir = run_root.join("tabula-screens");
        let state_home = base.join("state");
        let state_root = state_home.join("olo/screens");
        let prefix = base.join("desktop");
        let default_dir = base.join("config/olo");
        // Fresh host: parent dirs exist, but neither `olo-screens`, `state/olo/screens`,
        // nor `prefix/share/olo` exists yet.
        std::fs::create_dir_all(&run_root).unwrap();
        std::fs::create_dir_all(&state_home).unwrap();
        std::fs::create_dir_all(&prefix).unwrap();
        std::fs::create_dir_all(&default_dir).unwrap();

        let mut watches = FollowWatches::new().unwrap();
        let mut watching_ld_cache = false;
        let rts = vec![rt_dir.clone(), tabula_rt_dir];

        // Startup reconcile places ancestor watches on `run_root`, `state_home`, and `prefix`.
        let (st0, rc0) = sync_readiness_watch_and_status(&mut watches, &rts, &state_root, &prefix, &default_dir, &mut watching_ld_cache, true).unwrap();
        assert_eq!(st0.installed, None);
        assert!(!watching_ld_cache);
        assert!(rc0.screens && rc0.prefix && rc0.default_dir);
        assert!(watches.by_path.contains_key(&run_root));
        assert!(watches.by_path.contains_key(&state_home));

        // Simulate install completing AND a screen (`qa`) appearing before step 6 runs.
        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        std::fs::write(prefix.join("bin/olo-screen"), "#!/bin/sh\n").unwrap();
        std::fs::create_dir_all(prefix.join("share/olo")).unwrap();
        std::fs::write(prefix.join("share/olo/version"), "2026.11.27-r2\n").unwrap();
        std::fs::write(prefix.join("share/olo/release.json"), r#"{"committed":1700000123}"#).unwrap();
        std::fs::create_dir_all(&rt_dir).unwrap();
        std::fs::write(rt_dir.join("qa.json"), "{}").unwrap();
        std::fs::create_dir_all(state_root.join("qa")).unwrap();
        std::fs::write(state_root.join("qa/screen.json"), "{}").unwrap();

        // Step 6 runs with `force_reconcile = false`: `want_ld` flips false -> true,
        // triggering `watches.reconcile` which steps down onto `olo-screens` and
        // `state/olo/screens/qa` (removing `state_home`'s ancestor watch).
        let (st1, rc1) = sync_readiness_watch_and_status(&mut watches, &rts, &state_root, &prefix, &default_dir, &mut watching_ld_cache, false).unwrap();
        assert_eq!(st1.installed.as_deref(), Some("2026.11.27-r2"));
        assert_eq!(st1.committed, Some(1_700_000_123));
        assert!(watching_ld_cache);
        assert!(rc1.screens, "agy-r2 F2: install-triggered reconcile MUST propagate rc.screens=true so step 7 rescans `qa`!");
        assert!(rc1.prefix, "agy-r2 F2: install-triggered reconcile MUST propagate rc.prefix=true");
        assert!(watches.by_path.contains_key(&rt_dir), "watch must have stepped down onto olo-screens");
        assert!(watches.by_path.contains_key(&state_root.join("qa")), "watch must have stepped down onto state/olo/screens/qa");
        assert!(!watches.by_path.contains_key(&state_home), "ancestor state_home watch stepped down and was removed");

        // Steady-state call with no transition performs no reconcile and reports no changes.
        let (_st_steady, rc_steady) = sync_readiness_watch_and_status(&mut watches, &rts, &state_root, &prefix, &default_dir, &mut watching_ld_cache, false).unwrap();
        assert_eq!(rc_steady, ReconcileChanged::default());

        // Uninstall transition coinciding with another new screen directory `qa2`
        // must also propagate `rc.screens == true`.
        std::fs::remove_file(prefix.join("share/olo/version")).unwrap();
        std::fs::create_dir_all(state_root.join("qa2")).unwrap();
        let (st2, rc2) = sync_readiness_watch_and_status(&mut watches, &rts, &state_root, &prefix, &default_dir, &mut watching_ld_cache, false).unwrap();
        assert_eq!(st2.installed, None);
        assert!(!watching_ld_cache);
        assert!(rc2.screens, "agy-r2 F2: uninstall-triggered reconcile MUST propagate rc.screens=true for new screen `qa2`!");
        assert!(rc2.prefix);
        assert!(watches.by_path.contains_key(&state_root.join("qa2")));

        let _ = std::fs::remove_dir_all(&base);
    }
}
