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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::CString;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use olo_rex_link::screen_wire::{FollowEvent, HostScreen, HostScreenState, HostScreenStatus, MAX_JSON_LINE, PROTO, ScreenOwner};
use serde::Deserialize;

use crate::screens::{
    CancelFd, OwnedFd, PinResult, RuntimeScreen, default_now, exists_at, is_controller_pid_alive_with_ticks, open_regular_at, open_verified_subdir, pidfd_exited, pin_live_pid, prefix,
    read_bounded_regular, read_bounded_regular_at, read_control_meta, read_owner, read_owner_for_running, read_runtime_screen, screen_pid_running, screens_runtime_dir_candidates, screens_state_dir,
    set_nonblocking, timeout_ms_ceil, try_cleanup_stale_control, valid_name, write_all_cancellable,
};

/// Minimum interval between `LastUsed` lines for the same screen (at most 2/s).
const LAST_USED_INTERVAL: Duration = Duration::from_millis(500);

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
    /// `inotify_add_watch()`.
    fn reconcile(&mut self, runtime_dirs: &[PathBuf], state_root: &Path, prefix_dir: &Path, default_dir: &Path) -> io::Result<bool> {
        let mut changed = false;
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

            // Drop watches no longer desired (e.g. an ancestor we stepped down from
            // or a deleted screen folder).
            let stale: Vec<PathBuf> = self.by_path.keys().filter(|p| !desired.contains_key(*p)).cloned().collect();
            for path in stale {
                if let Some(entry) = self.by_path.remove(&path) {
                    changed = true;
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
                let want_mask = if roles.iter().any(|r| matches!(r, WatchRole::Target(_))) { DIR_MASK } else { ANCESTOR_MASK };
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
                    changed = true;
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

#[derive(Deserialize)]
struct ActionTime {
    at: u64,
    #[serde(default)]
    ms: u64,
}

/// Read the latest action end time (`at + ms`) from a screen's `actions.jsonl`
/// in O(1) via a verified screen-directory fd (never following symlinked screen
/// dirs or blocking on FIFOs).
fn read_last_used_at(dir_fd: &OwnedFd) -> Option<u64> {
    let (mut f, meta) = open_regular_at(dir_fd, "actions.jsonl").ok()?;
    let len = meta.len();
    if len == 0 {
        return None;
    }
    for cap in [16 * 1024u64, 256 * 1024u64] {
        let tail_len = len.min(cap);
        let from = len - tail_len;
        if f.seek(SeekFrom::Start(from)).is_err() {
            return None;
        }
        let mut raw = vec![0u8; tail_len as usize];
        if f.read_exact(&mut raw).is_err() {
            return None;
        }
        let slice = if from > 0 {
            match memchr::memchr(b'\n', &raw) {
                Some(nl) => &raw[nl + 1..],
                None => &raw[..],
            }
        } else {
            &raw[..]
        };
        let mut best: Option<u64> = None;
        for line in slice.split(|&b| b == b'\n') {
            let line = line.trim_ascii();
            if line.is_empty() {
                continue;
            }
            if let Ok(t) = serde_json::from_slice::<ActionTime>(line) {
                let end = t.at.saturating_add(t.ms);
                best = Some(best.map_or(end, |b| b.max(end)));
            }
        }
        if best.is_some() || from == 0 {
            return best;
        }
    }
    None
}

fn read_last_used(state_root: &Path, name: &str) -> Option<u64> {
    let dir_fd = open_verified_subdir(state_root, name).ok()?;
    read_last_used_at(&dir_fd)
}

fn read_status(prefix_dir: &Path) -> HostScreenStatus {
    let share = prefix_dir.join("share/olo");
    let has_tool = prefix_dir.join("bin/olo-screen").is_file();
    let installed = if has_tool { read_bounded_regular(&share.join("version"), 1024).and_then(|b| String::from_utf8(b).ok()).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()) } else { None };
    let committed = installed.as_ref().and_then(|_| {
        let bytes = read_bounded_regular(&share.join("release.json"), 64 * 1024)?;
        let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        v["committed"].as_u64()
    });
    HostScreenStatus { installed, committed, default_on_host: default_now() == "host" }
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
fn scan_screens(runtime_dirs: &[PathBuf], state_root: &Path, existing_pidfds: &mut HashMap<i32, OwnedFd>) -> (BTreeMap<String, HostScreen>, HashMap<i32, OwnedFd>) {
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

    let mut names: HashSet<String> = running.keys().cloned().collect();
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
    for name in names {
        let dir_fd = open_verified_subdir(state_root, &name).ok();
        let title = dir_fd.as_ref().and_then(|fd| read_bounded_regular_at(fd, "title", 4096)).and_then(|b| String::from_utf8(b).ok()).map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
        let last_used = dir_fd.as_ref().and_then(read_last_used_at);

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
                started: Some(s.started),
                last_used,
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
                controlled: false,
                owner,
                started_by,
            }
        };
        out.insert(name, screen);
    }

    (out, next_pidfds)
}

/// Compare two [`HostScreen`] snapshots ignoring `last_used`.
fn same_except_last_used(a: &HostScreen, b: &HostScreen) -> bool {
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
    let runtime_dirs = screens_runtime_dir_candidates();

    // Install watches BEFORE the initial scan so changes arriving during the
    // scan are queued in inotify and never missed.
    if let Err(e) = watches.reconcile(&runtime_dirs, &state_root, &prefix_dir, &default_dir) {
        eprintln!("olo screen follow: inotify watch failed: {e}");
        return 1;
    }

    let mut last_status = read_status(&prefix_dir);
    let mut pidfds: HashMap<i32, OwnedFd> = HashMap::new();
    let (mut known, next_pidfds) = scan_screens(&runtime_dirs, &state_root, &mut pidfds);
    pidfds = next_pidfds;

    let init_list: Vec<HostScreen> = known.values().cloned().collect();
    if !emit_snapshot(&last_status, &init_list, &cancels) {
        return 0;
    }

    let mut last_used_emit: HashMap<String, Instant> = HashMap::new();
    let mut pending_last_used: HashSet<String> = HashSet::new();
    let mut inotify_buf = vec![0u8; 32 * 1024];

    loop {
        // Compute earliest pending `last_used` debounce deadline (or -1 to sleep forever).
        let now = Instant::now();
        let mut timeout_ms: i32 = -1;
        for name in &pending_last_used {
            let rem = match last_used_emit.get(name) {
                Some(at) => LAST_USED_INTERVAL.saturating_sub(now.saturating_duration_since(*at)),
                None => Duration::ZERO,
            };
            let ms = timeout_ms_ceil(rem);
            timeout_ms = if timeout_ms < 0 { ms } else { timeout_ms.min(ms) };
        }

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
        let mut need_status = false;
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
                        need_reconcile = true;
                        need_status = true;
                        need_screens = true;
                        break;
                    }
                    if ev.mask & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_IGNORED) != 0 {
                        let already_ignored = ev.mask & libc::IN_IGNORED != 0;
                        if already_ignored && !watches.by_wd.contains_key(&ev.wd) {
                            continue;
                        }
                        watches.remove_and_forget_wd(ev.wd, already_ignored);
                        need_reconcile = true;
                        need_status = true;
                        need_screens = true;
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
                                        TargetKind::PrefixRoot | TargetKind::PrefixBin | TargetKind::PrefixShare | TargetKind::DefaultDir => {
                                            need_status = true;
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
                                        need_screens = true;
                                    } else {
                                        pending_last_used.insert(screen_name.clone());
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
                                    need_status = true;
                                }
                            }
                            WatchRole::Target(TargetKind::PrefixBin) => {
                                if name == "olo-screen" {
                                    need_status = true;
                                }
                            }
                            WatchRole::Target(TargetKind::PrefixShare) => {
                                if matches!(name, "version" | "release.json") {
                                    need_status = true;
                                }
                            }
                            WatchRole::Target(TargetKind::DefaultDir) => {
                                if name == "screens-on" {
                                    need_status = true;
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
        //    directories don't miss follow-up writes.
        if need_reconcile {
            match watches.reconcile(&runtime_dirs, &state_root, &prefix_dir, &default_dir) {
                Ok(true) => {
                    need_status = true;
                    need_screens = true;
                }
                Ok(false) => {}
                Err(e) => {
                    eprintln!("olo screen follow: inotify watch failed: {e}");
                    return 1;
                }
            }
        }

        // 6. Status update (Olo install/version/default).
        if need_status {
            let status = read_status(&prefix_dir);
            if status != last_status {
                if !emit(&FollowEvent::Status { status: status.clone() }, &cancels) {
                    return 0;
                }
                last_status = status;
            }
        }

        // 7. Full screen rescan when structure/state/metadata changed.
        if need_screens {
            let (now_screens, next_pidfds) = scan_screens(&runtime_dirs, &state_root, &mut pidfds);
            pidfds = next_pidfds;

            // Screens removed completely.
            let gone: Vec<String> = known.keys().filter(|k| !now_screens.contains_key(*k)).cloned().collect();
            for name in gone {
                known.remove(&name);
                pending_last_used.remove(&name);
                last_used_emit.remove(&name);
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
                            last_used_emit.insert(name.clone(), now_inst);
                        }
                        pending_last_used.remove(&name);
                        known.insert(name, new_s);
                    }
                    Some(old_s) if old_s == &new_s => {}
                    Some(old_s) if same_except_last_used(old_s, &new_s) => {
                        // Only `last_used` changed: route through the 500 ms debounce.
                        if new_s.last_used.is_some() {
                            pending_last_used.insert(name.clone());
                        } else {
                            // History cleared: emit full screen update so `last_used: None` is shown.
                            if !emit(&FollowEvent::Screen { screen: Box::new(new_s.clone()) }, &cancels) {
                                return 0;
                            }
                            pending_last_used.remove(&name);
                            known.insert(name, new_s);
                        }
                    }
                    Some(_) => {
                        if !emit(&FollowEvent::Screen { screen: Box::new(new_s.clone()) }, &cancels) {
                            return 0;
                        }
                        if new_s.last_used.is_some() {
                            last_used_emit.insert(name.clone(), now_inst);
                        }
                        pending_last_used.remove(&name);
                        known.insert(name, new_s);
                    }
                }
            }
        }

        // 8. Flush any debounced `last_used` updates that are now due (at most 2/s per screen).
        if !pending_last_used.is_empty() {
            let now_inst = Instant::now();
            let due: Vec<String> = pending_last_used.iter().filter(|name| last_used_emit.get(*name).is_none_or(|at| now_inst.saturating_duration_since(*at) >= LAST_USED_INTERVAL)).cloned().collect();
            for name in due {
                pending_last_used.remove(&name);
                let Some(cur) = known.get_mut(&name) else { continue };
                let latest = read_last_used(&state_root, &name);
                if latest != cur.last_used {
                    cur.last_used = latest;
                    if let Some(ts) = latest {
                        if !emit(&FollowEvent::LastUsed { name: name.clone(), last_used: ts }, &cancels) {
                            return 0;
                        }
                        last_used_emit.insert(name, now_inst);
                    } else if !emit(&FollowEvent::Screen { screen: Box::new(cur.clone()) }, &cancels) {
                        return 0;
                    }
                }
            }
        }
    }
}
