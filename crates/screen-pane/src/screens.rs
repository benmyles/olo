//! Agent screens: each one a headless Olo session of its own (its own
//! app_server, Deskbar and desktop, windows, pointer and keyboard focus),
//! so what an agent does there never moves the person's pointer or takes
//! their focus. The person watches (and steps in) through the viewer.
//!
//! A screen is `olo-session --headless WxH`, started in its own process
//! group with settings of its own (`OLO_APP_SERVER_SETTINGS`: the
//! person's, at 1× unless asked, never locking, blanking or sleeping) and
//! silent unless asked. It is listed in `$XDG_RUNTIME_DIR/olo-screens/
//! NAME.json`; everything else talks to its app_server's IPC socket.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use olo_protocol::{Chord, Client, ImageFormat, Reply, Request, Settings, SyntheticEvent, WindowInfo};
use serde::{Deserialize, Serialize};

/// evdev button codes.
pub const LEFT: u32 = 0x110;
pub const RIGHT: u32 = 0x111;
pub const MIDDLE: u32 = 0x112;

/// One agent screen, as listed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Screen {
    pub name: String,
    pub wayland_display: String,
    #[serde(alias = "tabula_socket")]
    pub olo_socket: PathBuf,
    /// olo-session's pid (the leader of the screen's process group).
    pub pid: i32,
    /// Logical size.
    pub width: i32,
    pub height: i32,
    pub scale: f64,
    pub sound: bool,
    /// Seconds since the epoch.
    pub started: u64,
    /// Its own session bus (`olo-bus`, in its session), and the person's
    /// accessibility bus its programs use; None for a screen from before
    /// (on the person's bus).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bus: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a11y: Option<String>,
}

/// Where Chrome keeps its profile on screen `name` (`CHROME_CONFIG_HOME`,
/// which Chrome, Chromium and olo-chrome all follow): beside the screen's
/// history. With the person's own, a Chrome started there was handed to
/// the person's running Chrome, which opened the window on their display.
pub fn chrome_home(name: &str) -> PathBuf {
    crate::history::dir(name)
}

/// Private fallback runtime directory when `XDG_RUNTIME_DIR` is unset:
/// prefers the standard owned `/run/user/<uid>` if present and owned by this
/// account, else private `0700` `/var/tmp/olo-runtime-<uid>` (never `/tmp`).
pub(crate) fn fallback_runtime_dir() -> PathBuf {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
    // SAFETY: `geteuid()` has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let std_run = PathBuf::from(format!("/run/user/{uid}"));
    if std::fs::symlink_metadata(&std_run).is_ok_and(|m| m.file_type().is_dir() && m.uid() == uid) {
        return std_run;
    }
    let fallback = PathBuf::from(format!("/var/tmp/olo-runtime-{uid}"));
    if std::fs::symlink_metadata(&fallback).is_err() {
        let mut b = std::fs::DirBuilder::new();
        b.mode(0o700);
        let _ = b.create(&fallback);
    }
    fallback
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(fallback_runtime_dir)
}

/// Where screens are listed (and their logs and settings kept).
pub fn dir() -> PathBuf {
    olo_xdg::legacy::runtime_file(&runtime_dir(), "olo-screens")
}

/// Read at most `max_bytes` from a verified regular file without following
/// symlinks or blocking on a FIFO.
pub(crate) fn read_bounded_regular(path: &Path, max_bytes: usize) -> Option<Vec<u8>> {
    use std::io::Read as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let f = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path).ok()?;
    let meta = f.metadata().ok()?;
    if !meta.file_type().is_file() || meta.len() > max_bytes as u64 {
        return None;
    }
    let mut buf = Vec::with_capacity((meta.len() as usize).min(64 << 10));
    f.take(max_bytes as u64 + 1).read_to_end(&mut buf).ok()?;
    (buf.len() <= max_bytes).then_some(buf)
}

/// Where screens' logs go: a folder of their own in `dir` (made as it's
/// asked for), not among the screens listed, which Deskbar follows (each
/// screen's session writes its log a few times a second: each a wake for
/// it, 2026-11-15).
fn logs_dir(dir: &Path) -> PathBuf {
    let logs = dir.join("logs");
    let _ = std::fs::create_dir_all(&logs);
    logs
}

fn entry(name: &str) -> PathBuf {
    dir().join(format!("{name}.json"))
}

/// Marks that the person has screen `name`'s pointer and keys (Agent
/// Screens' Take Control), in `dir`: the viewer's pid, while they have
/// them.
fn control_file(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.control"))
}

/// Stable per-screen flock inode serializing Take Control (`LOCK_EX`),
/// in-flight agent CLI actions (`LOCK_SH`), and `.control` marker cleanup
/// (never unlinked while screens exist, so releasing an old control lock can
/// never remove a replacement controller's `.control` marker).
fn control_lock_file(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.control.lock"))
}

/// Stable per-screen flock inode serializing lifecycle transitions (`start`,
/// `stop`, `clear`, `archive`, `delete`) so generation checks are atomic.
fn lifecycle_lock_file(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.lifecycle.lock"))
}

fn open_stable_lock(path: &Path) -> Result<std::fs::File, String> {
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
    // SAFETY: `geteuid()` has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| format!("couldn't open lock {}: {e}", path.display()))?;
    let meta = file.metadata().map_err(|e| format!("stat {}: {e}", path.display()))?;
    if !meta.file_type().is_file() || meta.uid() != uid {
        return Err(format!("refusing unsafe lock file {}", path.display()));
    }
    Ok(file)
}

fn lock_lifecycle_in(dir: &Path, name: &str) -> Result<std::fs::File, String> {
    use std::os::fd::AsRawFd as _;
    require_valid_name(name)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("couldn't create runtime folder: {e}"))?;
    let file = open_stable_lock(&lifecycle_lock_file(dir, name))?;
    // SAFETY: valid owned fd.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(format!("couldn't lock lifecycle for {name}: {}", std::io::Error::last_os_error()));
    }
    Ok(file)
}

/// Read `/proc/{pid}/stat`'s 22nd field (process start time in clock ticks) to
/// detect PID reuse when validating `.control` or session identity.
pub(crate) fn proc_start_ticks(pid: i32) -> Option<u64> {
    started_ticks(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

fn parse_control_lock_meta(bytes: &[u8]) -> Option<(i32, Option<u64>)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut parts = text.split_whitespace();
    let pid = parts.next()?.parse::<i32>().ok().filter(|&p| p > 0)?;
    let ticks = parts.next().and_then(|t| t.parse::<u64>().ok());
    Some((pid, ticks))
}

fn is_controller_alive(pid: i32, expected_ticks: Option<u64>) -> bool {
    if pid <= 0 {
        return false;
    }
    if let Some(exp) = expected_ticks
        && proc_start_ticks(pid) != Some(exp)
    {
        return false;
    }
    std::fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|exe| is_controller_exe(&exe))
}

/// Exclusive advisory lock (`flock`) on a running screen's Take Control lease.
///
/// Holds `LOCK_EX` on both the stable `NAME.control.lock` (serializing against
/// new viewers, in-flight agent CLI actions, and `screens::stop`) and on
/// `NAME.control` (compatible with legacy/current `olo-rex-link`), checks any
/// pre-existing non-flocking viewer PID before truncating, records `(dev, ino)`
/// so releasing an old handle can never unlink a replacement controller's
/// `.control` marker, and keeps `NAME.control` containing the plain decimal PID.
pub struct ControlLock {
    marker_path: PathBuf,
    marker_id: (u64, u64),
    stable_lock: Option<std::fs::File>,
    marker_file: Option<std::fs::File>,
}

impl ControlLock {
    pub fn release(&mut self) {
        use std::os::fd::AsRawFd as _;
        use std::os::unix::fs::MetadataExt as _;
        let marker = self.marker_file.take();
        let stable = self.stable_lock.take();
        if let Some(file) = marker {
            // Unlink ONLY while still holding both locks AND only if the path
            // on disk still points to the exact `(dev, ino)` we created.
            if let Ok(pm) = std::fs::symlink_metadata(&self.marker_path)
                && pm.file_type().is_file()
                && (pm.dev(), pm.ino()) == self.marker_id
            {
                let _ = std::fs::remove_file(&self.marker_path);
            }
            // Explicit `LOCK_UN` before `close` ensures a concurrent `fork()`
            // in another thread (prior to `O_CLOEXEC`) cannot keep the flock alive.
            // SAFETY: valid owned fd.
            unsafe {
                libc::flock(file.as_raw_fd(), libc::LOCK_UN);
            }
            drop(file);
        }
        if let Some(s) = stable {
            let _ = s.set_len(0);
            // SAFETY: valid owned fd.
            unsafe {
                libc::flock(s.as_raw_fd(), libc::LOCK_UN);
            }
            drop(s);
        }
    }
}

impl Drop for ControlLock {
    fn drop(&mut self) {
        self.release();
    }
}

/// Shared lease held by an in-flight agent CLI mutating action on `NAME` so
/// Take Control cannot interleave mid-drag (and an action cannot begin while
/// Take Control holds `LOCK_EX`).
pub struct ActionLease {
    stable_lock: Option<std::fs::File>,
}

impl Drop for ActionLease {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd as _;
        if let Some(s) = self.stable_lock.take() {
            // SAFETY: valid owned fd; unlock before close so a concurrent fork
            // cannot briefly keep LOCK_SH held.
            unsafe {
                libc::flock(s.as_raw_fd(), libc::LOCK_UN);
            }
            drop(s);
        }
    }
}

/// Acquire a shared action lease for screen `name` (waiting while the person
/// controls it, matching `controlled(name)` semantics).
pub fn acquire_action_lease(name: &str) -> Result<ActionLease, String> {
    acquire_action_lease_in(&dir(), name)
}

pub(crate) fn acquire_action_lease_in(dir: &Path, name: &str) -> Result<ActionLease, String> {
    use std::os::fd::AsRawFd as _;
    require_valid_name(name)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("couldn't create runtime folder: {e}"))?;
    let stable = open_stable_lock(&control_lock_file(dir, name))?;
    // First try non-blocking so we can check if someone else holds Take Control.
    // SAFETY: valid owned fd.
    if unsafe { libc::flock(stable.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } != 0 {
        return Err(format!("someone is controlling screen {name} right now"));
    }
    // Also honor an unchanged `olo-rex-link` or legacy viewer that only holds
    // `NAME.control` (without `NAME.control.lock`).
    if controlled_in(dir, name) {
        unsafe {
            libc::flock(stable.as_raw_fd(), libc::LOCK_UN);
        }
        return Err(format!("someone is controlling screen {name} right now"));
    }
    Ok(ActionLease { stable_lock: Some(stable) })
}

/// Acquire exclusive Take Control lock for screen `name`, failing without
/// modifying `.control` if another viewer or an in-flight agent action holds it.
pub fn try_acquire_control(name: &str) -> Result<ControlLock, String> {
    try_acquire_control_in(&dir(), name)
}

pub(crate) fn try_acquire_control_in(dir: &Path, name: &str) -> Result<ControlLock, String> {
    use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};

    require_valid_name(name)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("couldn't create control folder: {e}"))?;
    // SAFETY: `geteuid()` has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let my_pid = std::process::id() as i32;
    let my_ticks = proc_start_ticks(my_pid);

    let mut stable = open_stable_lock(&control_lock_file(dir, name))?;
    // SAFETY: valid owned fd.
    if unsafe { libc::flock(stable.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("Someone else is controlling or acting on this screen.".into());
    }

    let path = control_file(dir, name);
    for _ in 0..4 {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&path)
            .map_err(|e| format!("couldn't open control lock for {name}: {e}"))?;
        let fm = file.metadata().map_err(|e| format!("stat control lock for {name}: {e}"))?;
        if !fm.file_type().is_file() || fm.uid() != uid {
            return Err(format!("refusing unsafe control marker for {name}"));
        }
        // SAFETY: `file` is a valid owned file descriptor.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("Someone else is controlling this screen.".into());
        }
        let same_inode = std::fs::symlink_metadata(&path).is_ok_and(|pm| pm.file_type().is_file() && fm.dev() == pm.dev() && fm.ino() == pm.ino());
        if !same_inode {
            continue;
        }
        // Guard against a live legacy non-flocking viewer BEFORE truncating.
        if fm.len() > 0 && fm.len() <= 64 {
            let mut existing = Vec::new();
            if file.seek(SeekFrom::Start(0)).is_ok()
                && std::io::Read::by_ref(&mut file).take(65).read_to_end(&mut existing).is_ok()
                && let Some((other_pid, _)) = parse_control_lock_meta(&existing)
                && other_pid != my_pid
                && is_controller_alive(other_pid, None)
            {
                return Err("Someone else is controlling this screen.".into());
            }
        }
        // Record `(pid, start_ticks)` in the stable lock file for PID-reuse
        // validation, and keep plain decimal `{pid}` in `.control` for full
        // compatibility with legacy/current `olo-rex-link` readers.
        let _ = stable.set_len(0);
        let _ = stable.seek(SeekFrom::Start(0));
        if let Some(t) = my_ticks {
            let _ = write!(stable, "{my_pid} {t}");
        } else {
            let _ = write!(stable, "{my_pid}");
        }
        let _ = stable.flush();

        if let Err(e) = file.set_len(0).and_then(|()| file.seek(SeekFrom::Start(0))).and_then(|_| write!(file, "{my_pid}")).and_then(|()| file.flush()) {
            if std::fs::symlink_metadata(&path).is_ok_and(|pm| fm.dev() == pm.dev() && fm.ino() == pm.ino()) {
                let _ = std::fs::remove_file(&path);
            }
            return Err(format!("couldn't write control lock for {name}: {e}"));
        }
        return Ok(ControlLock { marker_path: path, marker_id: (fm.dev(), fm.ino()), stable_lock: Some(stable), marker_file: Some(file) });
    }
    Err("Someone else is controlling this screen.".into())
}

/// Whether `exe` (resolved from `/proc/PID/exe`) is an Olo viewer or Rex
/// helper permitted to hold a screen's `.control` lock (`olo-screen`,
/// `olo-rex`, `olo-rex-link*`, or the current test executable).
fn is_controller_exe(exe: &Path) -> bool {
    let Some(raw) = exe.file_name().and_then(|n| n.to_str()) else { return false };
    let clean = raw.trim_end_matches(" (deleted)");
    if clean == "olo-screen" || clean == "olo-rex" || clean.starts_with("olo-rex-link") {
        return true;
    }
    std::env::current_exe().ok().and_then(|me| me.file_name().map(|m| m == raw || m == clean)).unwrap_or(false)
}

/// Whether the person is controlling screen `name` now (a viewer that
/// took it over and has since gone doesn't count).
pub fn controlled(name: &str) -> bool {
    controlled_in(&dir(), name)
}

fn controlled_in(dir: &Path, name: &str) -> bool {
    if !valid_name(name) {
        return false;
    }
    let Some(bytes) = read_bounded_regular(&control_file(dir, name), 64) else { return false };
    let Some((pid, inline_ticks)) = parse_control_lock_meta(&bytes) else { return false };
    // Check optional `(pid, start_ticks)` recorded in the stable lock sidecar
    // so a recycled PID from a crashed helper is not mistaken for the owner.
    let expected_ticks = inline_ticks.or_else(|| read_bounded_regular(&control_lock_file(dir, name), 64).and_then(|b| parse_control_lock_meta(&b)).filter(|(p, _)| *p == pid).and_then(|(_, t)| t));
    is_controller_alive(pid, expected_ticks)
}

/// Forget the screens this process said the person controls (a viewer
/// starting: an upgrade re-executes it with the same pid).
pub fn clear_controlled_by_me() {
    clear_controlled_in(&dir());
}

fn clear_controlled_in(dir: &Path) {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
    let me = std::process::id() as i32;
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|e| e == "control") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|s| s.to_str()).filter(|n| valid_name(n)) else { continue };
        let Some(bytes) = read_bounded_regular(&path, 64) else { continue };
        if !parse_control_lock_meta(&bytes).is_some_and(|(p, _)| p == me) {
            continue;
        }
        // Serialize with `NAME.control.lock` and verify `(dev, ino)` before unlinking.
        let Ok(stable) = open_stable_lock(&control_lock_file(dir, name)) else { continue };
        if unsafe { libc::flock(stable.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            continue;
        }
        let Ok(file) = std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC).open(&path) else {
            continue;
        };
        let Ok(fm) = file.metadata() else { continue };
        if !fm.file_type().is_file() {
            continue;
        }
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0
            && std::fs::symlink_metadata(&path).is_ok_and(|pm| pm.file_type().is_file() && pm.dev() == fm.dev() && pm.ino() == fm.ino())
        {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// A name a screen can have: starts with a letter or digit, followed by
/// letters, digits, `-` or `_` (never option-shaped `-…` or path-shaped).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 40 && name.as_bytes()[0].is_ascii_alphanumeric() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Single validation gate for every operation taking a screen name.
pub(crate) fn require_valid_name(name: &str) -> Result<(), String> {
    if valid_name(name) { Ok(()) } else { Err(format!("{name:?} can't name a screen (letters, digits, - and _)")) }
}

/// Whether a process with this command line and environment is screen
/// `name`'s session: an olo-session (`tabula-session` before the rename)
/// started for it, as its environment says (`OLO_SCREEN`, given at
/// `start` and kept across an upgrade's re-execution). Its command line
/// isn't: a session re-executed itself bare after an upgrade, so a
/// screen known by `--headless` there looked ended, `list` dropped it,
/// and it ran on unknown (ten on 2026-11-07, each with its own display
/// server).
fn is_session_of(cmdline: &[u8], environ: &[u8], name: &str) -> bool {
    let first = cmdline.split(|b| *b == 0).next().unwrap_or_default();
    let session = first.ends_with(b"olo-session") || first.ends_with(b"tabula-session");
    let wanted = format!("OLO_SCREEN={name}");
    session && environ.split(|b| *b == 0).any(|v| v == wanted.as_bytes())
}

/// Whether a session with this environment is one of the screens listed
/// in `here` (this olo-screen's runtime folder): it was started under
/// that folder (`XDG_RUNTIME_DIR`, which a screen's session takes from
/// the olo-screen that starts it and keeps). One started under another
/// folder is listed there, and nothing here can call it a stray: a tool
/// that keeps its screens in a runtime folder of its own
/// (tools/dev/readme-shots) saw every other running screen as one, and
/// each `start` and `stop` it made ended them all, another agent's
/// screen in the middle of its work among them (2026-11-16).
fn listed_in(environ: &[u8], here: &Path) -> bool {
    let theirs = environ
        .split(|b| *b == 0)
        .find_map(|v| v.strip_prefix(b"XDG_RUNTIME_DIR="))
        .filter(|d| !d.is_empty())
        .map(|d| PathBuf::from(String::from_utf8_lossy(d).into_owned()))
        .unwrap_or_else(fallback_runtime_dir);
    let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_owned());
    real(&theirs) == real(here)
}

/// The screen a process is the session of, if it's a screen's session
/// (`OLO_SCREEN`'s name).
fn screen_of(pid: i32) -> Option<String> {
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let environ = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    let name = environ.split(|b| *b == 0).find_map(|v| v.strip_prefix(b"OLO_SCREEN=")).map(|n| String::from_utf8_lossy(n).into_owned())?;
    is_session_of(&cmdline, &environ, &name).then_some(name)
}

/// When a process started, in clock ticks since boot (`/proc/PID/stat`'s
/// 22nd field, after its name in parentheses, which may hold anything).
fn started_ticks(stat: &str) -> Option<u64> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()
}

/// How long ago process `pid` started.
fn age(pid: i32) -> Option<Duration> {
    let ticks = started_ticks(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)?;
    let uptime: f64 = std::fs::read_to_string("/proc/uptime").ok()?.split_whitespace().next()?.parse().ok()?;
    // SAFETY: sysconf has no preconditions.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
    Some(Duration::from_secs_f64((uptime - ticks as f64 / hz).max(0.0)))
}

fn parent(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
}

/// Screens' sessions no listed screen is (`(pid, screen)`): forgotten
/// by an olo-screen from before (every upgrade did), or left by one
/// that ended while starting a screen. Those of screen `name` whatever
/// their age; others once two minutes old, so a screen another agent is
/// starting now isn't one. Never a session inside a screen's (a test
/// session run there inherits its `OLO_SCREEN`), and never one started
/// under another runtime folder (`listed_in`: it is listed there).
fn strays(name: Option<&str>) -> Vec<(i32, String)> {
    let listed: Vec<i32> = list().iter().map(|s| s.pid).collect();
    let here = runtime_dir();
    let in_a_screen = |pid: i32| {
        let mut p = parent(pid);
        for _ in 0..64 {
            match p {
                Some(q) if q > 1 => {
                    if screen_of(q).is_some() {
                        return true;
                    }
                    p = parent(q);
                }
                _ => return false,
            }
        }
        false
    };
    let mut out = Vec::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else { continue };
        if listed.contains(&pid) || std::fs::read_to_string(format!("/proc/{pid}/comm")).map_or(true, |c| !c.trim().ends_with("-session")) {
            continue;
        }
        let Some(screen) = screen_of(pid) else { continue };
        if !std::fs::read(format!("/proc/{pid}/environ")).is_ok_and(|e| listed_in(&e, &here)) {
            continue;
        }
        let old = age(pid).is_some_and(|a| a > Duration::from_secs(120));
        if (Some(screen.as_str()) == name || old) && !in_a_screen(pid) {
            out.push((pid, screen));
        }
    }
    out
}

/// End sessions `pids` (each a screen's, in a process group of its own):
/// asked first, then, any still there after 5 s, their whole process
/// group ended (app_server and what the session started with it), so
/// none is left running unknown.
fn end_sessions(pids: &[i32]) {
    for &pid in pids {
        stop_pid(pid);
    }
    let gone = |pid: i32| !std::path::Path::new(&format!("/proc/{pid}")).exists() || screen_of(pid).is_none();
    let until = Instant::now() + Duration::from_secs(5);
    while pids.iter().any(|&p| !gone(p)) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    for &pid in pids.iter().filter(|&&p| !gone(p)) {
        tracing::warn!(pid, "a screen's session didn't end when asked; ending its process group");
        // SAFETY: plain kill(2), on the process group `start` made for it.
        unsafe { libc::kill(-pid, libc::SIGKILL) };
    }
    let until = Instant::now() + Duration::from_secs(2);
    while pids.iter().any(|&p| !gone(p)) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// End the screens' sessions nobody lists any more (`strays`).
fn end_strays(name: Option<&str>) {
    let found = strays(name);
    for (pid, screen) in &found {
        tracing::info!(pid, screen, "a screen's session nothing lists; ending it");
    }
    end_sessions(&found.iter().map(|(p, _)| *p).collect::<Vec<_>>());
}

impl Screen {
    /// Its session is still running: the process listed is this
    /// screen's session (`is_session_of`), whatever its command line
    /// says now.
    pub fn running(&self) -> bool {
        screen_of(self.pid).as_deref() == Some(self.name.as_str())
    }

    /// Still running and reachable: its session alive and its socket
    /// there.
    pub fn alive(&self) -> bool {
        self.running() && self.olo_socket.exists()
    }

    fn client(&self) -> Result<Client, String> {
        Client::connect_to(&self.olo_socket).map_err(|e| format!("screen {} isn't answering: {e}", self.name))
    }

    pub fn request(&self, request: &Request) -> Result<Reply, String> {
        self.client()?.request(request).map_err(|e| e.to_string())
    }

    /// Its windows, front ones with where they are.
    pub fn windows(&self) -> Result<Vec<WindowInfo>, String> {
        match self.request(&Request::ListWindows)? {
            Reply::Windows { windows } => Ok(windows),
            other => Err(format!("unexpected reply {other:?}")),
        }
    }

    pub fn input(&self, events: Vec<SyntheticEvent>) -> Result<(), String> {
        self.request(&Request::Synthetic { events }).map(drop)
    }

    /// Type `text`: what a US keyboard types as keys, anything else (é,
    /// emoji, CJK) put in whole as an input method would
    /// (`Request::InsertText`), in order.
    pub fn type_text(&self, text: &str) -> Result<(), String> {
        for (keys, run) in runs(text) {
            if keys {
                self.input(type_text(run)?)?;
            } else {
                if let Reply::Error { message } = self.request(&Request::InsertText { text: run.to_owned() })? {
                    return Err(format!("{run:?} couldn't be put in: {message}"));
                }
            }
        }
        Ok(())
    }

    /// Send each batch of events in turn, waiting after each as long as
    /// it says: as a hand would, so what reacts to movement over time (a
    /// drag, a touchpad's lift) sees it as from a person.
    pub fn input_paced(&self, steps: Vec<(Vec<SyntheticEvent>, Duration)>) -> Result<(), String> {
        let mut client = self.client()?;
        for (events, wait) in steps {
            client.request(&Request::Synthetic { events }).map_err(|e| e.to_string())?;
            std::thread::sleep(wait);
        }
        Ok(())
    }

    /// A picture of the whole screen (or one window) at `path`, once it
    /// is written.
    pub fn picture(&self, path: &Path, window: Option<u64>, pointer: bool, format: ImageFormat) -> Result<(), String> {
        let request = Request::Screenshot { path: path.display().to_string(), output: None, window, border: true, pointer, format, as_shown: false };
        self.request(&request).map(drop)
    }

    /// The environment a program on this screen gets: its display and
    /// app_server, no X display (X programs would show on the person's
    /// screen), Chrome's profile of its own (`chrome_home`), and silence
    /// unless the screen has sound.
    pub fn env(&self) -> Vec<(String, String)> {
        // (The kit's, which moves to a screen use too.)
        let mut env = olo_kit::moving::screen_env(&self.name, &self.wayland_display, &self.olo_socket, self.pid, self.sound);
        env.extend(olo_kit::moving::bus_env(self.bus.as_deref(), self.a11y.as_deref()));
        env
    }

    /// Start `argv` on this screen (a bare name is looked for as Olo
    /// finds programs); its pid.
    pub fn run(&self, argv: &[String]) -> Result<u32, String> {
        let program = argv.first().ok_or("no program to run")?;
        let path = if program.contains('/') { PathBuf::from(program) } else { olo_xdg::find_program(program).ok_or_else(|| format!("{program} isn't installed"))? };
        let mut cmd = Command::new(&path);
        cmd.args(&argv[1..]).env_remove("DISPLAY").stdin(Stdio::null());
        for (k, v) in self.env() {
            cmd.env(k, v);
        }
        // An Electron app's single-instance lock is in its config: the
        // screen's own, or it hands itself to the person's copy.
        if olo_xdg::electron::is_electron(path.as_os_str()) {
            let config = crate::history::dir(&self.name).join("config");
            let _ = std::fs::create_dir_all(&config);
            cmd.env("XDG_CONFIG_HOME", config);
        }
        let log = std::fs::File::create(logs_dir(&dir()).join(format!("{}-{}.log", self.name, sanitize(program))));
        if let Ok(log) = log {
            cmd.stdout(log.try_clone().map(Stdio::from).unwrap_or_else(|_| Stdio::null())).stderr(Stdio::from(log));
        }
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        let mut child = cmd.spawn().map_err(|e| format!("{program} couldn't be started: {e}"))?;
        let pid = child.id();
        // Reaped when it ends (a CLI that exits first leaves that to init).
        std::thread::spawn(move || child.wait());
        Ok(pid)
    }
}

fn sanitize(program: &str) -> String {
    Path::new(program).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The running screens, by name. Entries whose session is gone are
/// removed.
pub fn list() -> Vec<Screen> {
    list_in(&dir())
}

pub(crate) fn list_in(runtime_screens_dir: &Path) -> Vec<Screen> {
    let mut screens = Vec::new();
    let Ok(read) = std::fs::read_dir(runtime_screens_dir) else { return screens };
    for e in read.flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if fname.ends_with("-settings.json") || fname.ends_with(".owner.json") {
            continue;
        }
        let Some(screen) = read_bounded_regular(&path, 64 * 1024).and_then(|b| serde_json::from_slice::<Screen>(&b).ok()) else { continue };
        // (Forgotten only once its session has ended: a socket missing a
        // moment, or a command line that changed, isn't that.)
        if screen.running() {
            screens.push(screen);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
    screens.sort_by(|a, b| a.name.cmp(&b.name));
    screens
}

/// Look up a single running screen directly from its bounded `NAME.json`
/// descriptor (no full directory scan, no unbounded read).
pub fn get(name: &str) -> Result<Screen, String> {
    get_in(&dir(), name)
}

pub(crate) fn get_in(runtime_screens_dir: &Path, name: &str) -> Result<Screen, String> {
    require_valid_name(name)?;
    let path = runtime_screens_dir.join(format!("{name}.json"));
    let Some(screen) = read_bounded_regular(&path, 64 * 1024).and_then(|b| serde_json::from_slice::<Screen>(&b).ok()) else {
        return Err(format!("there's no screen {name:?} (olo-screen list)"));
    };
    if screen.name == name && screen.running() {
        Ok(screen)
    } else {
        let _ = std::fs::remove_file(&path);
        Err(format!("there's no screen {name:?} (olo-screen list)"))
    }
}

/// The first of "agent", "agent-2", … that no running screen has: what
/// `olo-screen start` with no name starts (a stopped screen of that name
/// starts again, as it was: an agent gets its "agent" back).
pub fn free_name() -> String {
    let taken: Vec<String> = list().into_iter().map(|s| s.name).collect();
    first_free(&taken)
}

/// A name for a screen that is new: the first of "agent", "agent-2", …
/// that no screen there is has, running, stopped or archived, and that
/// nothing is kept under (Agent Screens' New Screen: with `free_name` it
/// started the stopped or archived "agent" again, its history and its
/// Chrome profile with it. Ben, 2026-11-17: "it seems to bring back an
/// old archived/deleted screen instead of creating an actual new
/// screen").
pub fn new_name() -> String {
    new_name_among(all().into_iter().map(|(n, _)| n).collect(), &olo_xdg::state_dir().join("screens"))
}

/// [`new_name`], with the screens there are and where screens keep
/// their things.
fn new_name_among(mut taken: Vec<String>, kept: &std::path::Path) -> String {
    // (A folder left with neither history nor settings in it: its
    // recordings or its browser's profile would be the new screen's.)
    if let Ok(kept) = std::fs::read_dir(kept) {
        taken.extend(kept.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()));
    }
    first_free(&taken)
}

/// The first of "agent", "agent-2", … not among `taken`.
fn first_free(taken: &[String]) -> String {
    (1..).map(|n| if n == 1 { "agent".to_owned() } else { format!("agent-{n}") }).find(|n| !taken.contains(n)).unwrap_or_default()
}

/// What to start a screen with (kept beside its history: `kept`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Options {
    pub width: i32,
    pub height: i32,
    pub scale: f64,
    pub sound: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options { width: 1440, height: 900, scale: 1.0, sound: false }
    }
}

/// What screen `name` was started with, if it ever was. Validates `name` and
/// reads only a bounded non-symlink regular file.
pub fn kept(name: &str) -> Option<Options> {
    let dir = crate::history::dir_checked(name).ok()?;
    read_bounded_regular(&dir.join("screen.json"), 16 * 1024).and_then(|b| serde_json::from_slice(&b).ok())
}

fn keep(name: &str, opts: &Options) {
    let Ok(dir) = crate::history::dir_checked(name) else { return };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("screen.json");
    if let Ok(json) = serde_json::to_vec_pretty(opts) {
        let _ = std::fs::write(path, json);
    }
}

/// How a screen is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Running,
    Stopped,
    Archived,
}

/// Every screen there is (running, or stopped or archived with what it
/// kept), by name, and how it is.
pub fn all() -> Vec<(String, State)> {
    let running: Vec<String> = list().into_iter().map(|s| s.name).collect();
    let mut all: Vec<(String, State)> = running.iter().map(|n| (n.clone(), State::Running)).collect();
    for n in crate::history::names() {
        if !running.contains(&n) {
            let state = if crate::history::archived(&n) { State::Archived } else { State::Stopped };
            all.push((n, state));
        }
    }
    all.sort_by(|a, b| a.0.cmp(&b.0));
    all
}

/// Stop screen `name` if it runs, archive it.
pub fn archive(name: &str, on: bool) -> Result<(), String> {
    let _life = lock_lifecycle_in(&dir(), name)?;
    if !all().iter().any(|(n, _)| n == name) {
        return Err(format!("there's no screen {name:?} (olo-screen list)"));
    }
    if on && get(name).is_ok() {
        return Err(format!("screen {name} is running: stop it first (olo-screen stop {name})"));
    }
    crate::history::set_archived(name, on)
}

/// Delete screen `name` and everything it kept (its history,
/// recordings, stats and Chrome profile), to the Trash; stopped first if
/// it runs.
pub fn delete(name: &str) -> Result<(), String> {
    let _life = lock_lifecycle_in(&dir(), name)?;
    if !all().iter().any(|(n, _)| n == name) {
        return Err(format!("there's no screen {name:?} (olo-screen list)"));
    }
    if get(name).is_ok() {
        stop_locked(&dir(), name, None)?;
    }
    let _ = std::fs::remove_file(dir().join(format!("{name}.owner.json")));
    crate::history::trash(name)
}

/// Delete screen `name` ONLY if it is currently stopped (refusing if a new
/// session started while the confirmation dialog was open).
pub fn delete_if_stopped(name: &str) -> Result<(), String> {
    delete_if_stopped_in(&dir(), &crate::history::root_dir(), name)
}

pub(crate) fn delete_if_stopped_in(rt_dir: &Path, state_root: &Path, name: &str) -> Result<(), String> {
    let _life = lock_lifecycle_in(rt_dir, name)?;
    if get_in(rt_dir, name).is_ok() {
        return Err(format!("screen {name} is running; stop it first before deleting"));
    }
    let _ = std::fs::remove_file(rt_dir.join(format!("{name}.owner.json")));
    crate::history::trash_in(state_root, name)
}

/// Clear screen `name`'s history ONLY if it is currently stopped (refusing if
/// the screen started while the confirmation dialog was open).
pub fn clear_history_if_stopped(name: &str) -> Result<(), String> {
    clear_history_if_stopped_in(&dir(), &crate::history::root_dir(), name)
}

pub(crate) fn clear_history_if_stopped_in(rt_dir: &Path, state_root: &Path, name: &str) -> Result<(), String> {
    let _life = lock_lifecycle_in(rt_dir, name)?;
    if get_in(rt_dir, name).is_ok() {
        return Err(format!("screen {name} is running; stop it first before clearing its history"));
    }
    crate::history::clear_in(state_root, name)
}

fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))).unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("olo").join("app_server.json")
}

/// The person's settings, made right for a screen nobody sits at: its
/// own scale, one display, and nothing that locks, blanks or sleeps.
pub fn screen_settings(person: Option<Settings>, scale: f64) -> Settings {
    let mut s = person.unwrap_or_default();
    s.display = olo_protocol::DisplaySettings { scale, ..Default::default() };
    s.power.set_idle_minutes(false, false, 0);
    s.power.set_idle_minutes(false, true, 0);
    s.power.set_idle_minutes(true, false, 0);
    s.power.set_idle_minutes(true, true, 0);
    s.power.lock_on_display_off = false;
    s.screen_saver.enabled = false;
    s.screen_saver.lock = false;
    s
}

/// Start a screen called `name`, and wait until it answers.
pub fn start(name: &str, opts: &Options) -> Result<Screen, String> {
    if !valid_name(name) {
        return Err(format!("{name:?} can't name a screen (letters, digits, - and _)"));
    }
    if list().iter().any(|s| s.name == name) {
        return Err(format!("there's already a screen {name:?}"));
    }
    // (One of this name nothing lists would be a second, unreachable.)
    end_strays(Some(name));
    sweep_recorder_frames();
    if !(320..=7680).contains(&opts.width) || !(240..=4320).contains(&opts.height) {
        return Err(format!("{}x{} isn't a size a screen can have", opts.width, opts.height));
    }
    if !(1.0..=3.0).contains(&opts.scale) {
        return Err(format!("{} isn't a scale a screen can have (1 to 3)", opts.scale));
    }
    let dir = dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let _life = lock_lifecycle_in(&dir, name)?;
    if get_in(&dir, name).is_ok() {
        return Err(format!("there's already a screen {name:?}"));
    }
    let _ = std::fs::File::create(dir.join("no-sound"));
    let person: Option<Settings> = std::fs::read(config_path()).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let settings_path = dir.join(format!("{name}-settings.json"));
    let settings = serde_json::to_vec_pretty(&screen_settings(person, opts.scale)).map_err(|e| e.to_string())?;
    std::fs::write(&settings_path, settings).map_err(|e| e.to_string())?;

    let session = olo_xdg::find_program("olo-session").ok_or("olo-session isn't installed")?;
    let log_path = logs_dir(&dir).join(format!("{name}.log"));
    let log = std::fs::File::create(&log_path).map_err(|e| e.to_string())?;
    // Pixels, as the backend takes them.
    let (pw, ph) = ((opts.width as f64 * opts.scale).round() as i32, (opts.height as f64 * opts.scale).round() as i32);
    // A session bus of its own, so a program there never finds the
    // person's copy of itself (GTK's), and dialogs it asks for show there;
    // the person's accessibility bus still, for `tree`.
    let bus = format!("unix:path={}", dir.join(format!("{name}.bus")).display());
    let a11y = person_a11y_bus();
    let mut cmd = Command::new(session);
    cmd.arg("--headless")
        .arg(format!("{pw}x{ph}"))
        .arg("--own-bus")
        .arg(&bus)
        .env("OLO_APP_SERVER_SETTINGS", &settings_path)
        .env("OLO_SCREEN", name)
        .env("CHROME_CONFIG_HOME", chrome_home(name))
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("OLO_SOCKET")
        .env_remove("OLO_SESSION")
        .env_remove("OLO_SESSION_PID")
        .env_remove("OLO_REAL_SESSION")
        .env_remove("DISPLAY")
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    if !opts.sound {
        cmd.env("PULSE_SERVER", format!("unix:{}", dir.join("no-sound").display()));
    }
    match &a11y {
        Some(a) => cmd.env("OLO_A11Y_BUS", a),
        None => cmd.env_remove("OLO_A11Y_BUS"),
    };
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|e| format!("the screen couldn't be started: {e}"))?;
    let pid = child.id() as i32;
    std::thread::spawn(move || child.wait());

    // The session names its sockets on its first line, then app_server
    // answers on the IPC socket once it is up.
    let until = Instant::now() + Duration::from_secs(20);
    let (display, socket) = loop {
        let text = std::fs::read_to_string(&log_path).unwrap_or_default();
        if let Some(found) = text.lines().find_map(sockets) {
            break found;
        }
        if Instant::now() > until || !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            return Err(format!("the screen didn't start; see {}", log_path.display()));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let screen = Screen {
        name: name.to_owned(),
        wayland_display: display,
        olo_socket: socket,
        pid,
        width: opts.width,
        height: opts.height,
        scale: opts.scale,
        sound: opts.sound,
        started: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        // (Its session says if it couldn't have its own.)
        bus: (!std::fs::read_to_string(&log_path).unwrap_or_default().contains("shares the person's")).then_some(bus),
        a11y,
    };
    loop {
        if matches!(screen.request(&Request::Ping), Ok(Reply::Pong { .. })) {
            break;
        }
        if Instant::now() > until {
            stop_pid(pid);
            return Err(format!("the screen started but its app_server doesn't answer; see {}", log_path.display()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    fastest(&screen);
    // Kept, to be listed once stopped and started again as it was; out
    // of the archive if it was there.
    keep(name, opts);
    let _ = crate::history::set_archived(name, false);
    let tmp = entry(name).with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&screen).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, entry(name)).map_err(|e| e.to_string())?;
    start_recorder(name, pid);
    Ok(screen)
}

fn is_executable_file(path: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;
    let Ok(m) = std::fs::metadata(path) else { return false };
    if !m.is_file() {
        return false;
    }
    let Ok(cpath) = CString::new(path.as_os_str().as_bytes()) else { return false };
    // `access(X_OK)` checks whether *this account* can actually execute the
    // file (rejecting foreign owner-only executables and `noexec` mounts).
    // SAFETY: `cpath` is a valid NUL-terminated path.
    unsafe { libc::access(cpath.as_ptr(), libc::X_OK) == 0 }
}

fn resolve_recorder_candidates_from(current_exe: Option<&Path>, path_env: Option<&std::ffi::OsStr>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut push_unique = |cand: PathBuf| {
        if is_executable_file(&cand) && !out.contains(&cand) {
            out.push(cand);
        }
    };
    if let Some(exe) = current_exe {
        // (This program's own file may have been replaced since it started.)
        let exe = PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
        if exe.file_name().and_then(|n| n.to_str()) == Some("olo-screen") {
            push_unique(exe.clone());
        }
        push_unique(exe.with_file_name("olo-screen"));
    }
    if let Some(paths) = path_env {
        for dir in std::env::split_paths(paths) {
            push_unique(dir.join("olo-screen"));
        }
    }
    if let Some(found) = olo_xdg::find_program("olo-screen") {
        push_unique(found);
    }
    out
}

/// Resolve all executable `olo-screen` candidates in priority order so a
/// failing candidate (e.g. `EACCES` or `noexec`) falls through to the next.
fn resolve_recorder_candidates() -> Vec<PathBuf> {
    let cur = std::env::current_exe().ok();
    let path_env = std::env::var_os("PATH");
    resolve_recorder_candidates_from(cur.as_deref(), path_env.as_deref())
}

fn spawn_recorder_from_candidates(candidates: &[PathBuf], name: &str, pid: i32, log_path: Option<&Path>) -> Result<(), String> {
    use std::os::unix::process::CommandExt as _;
    if candidates.is_empty() {
        return Err("olo-screen wasn't found".into());
    }
    let mut last_err = String::new();
    for exe in candidates {
        let mut rec = Command::new(exe);
        rec.args(["record", name]).process_group(pid).stdin(std::process::Stdio::null());
        if let Some(lp) = log_path
            && let Ok(log) = std::fs::OpenOptions::new().create(true).append(true).open(lp)
        {
            if let Ok(out) = log.try_clone() {
                rec.stdout(out);
            }
            rec.stderr(log);
        }
        match rec.spawn() {
            Ok(child) => {
                olo_xdg::reap_child(child);
                return Ok(());
            }
            Err(e) => {
                last_err = format!("{}: {e}", exe.display());
            }
        }
    }
    Err(last_err)
}

/// Start what keeps screen `name`'s frames beside its history (`record`)
/// for as long as it runs: in the screen's process group (its session's
/// pid), so it ends with it.
fn start_recorder(name: &str, pid: i32) {
    let candidates = resolve_recorder_candidates();
    let log_path = logs_dir(&dir()).join(format!("{name}-record.log"));
    if let Err(e) = spawn_recorder_from_candidates(&candidates, name, pid, Some(&log_path)) {
        tracing::warn!("screen {name}'s frames won't be kept: {e}");
    }
}

/// The file a screen's recorder holds locked for as long as it runs
/// (`flock`: let go by the kernel however it ends), so anyone can tell
/// whether its frames are being kept.
pub fn recorder_lock(name: &str) -> PathBuf {
    crate::history::dir(name).join("recorder.lock")
}

/// The lock on screen `name`'s recorder, if nobody has it: the recorder
/// takes it for its life (and is the only one); anyone else lets it go
/// at once, having learnt that none runs.
pub fn take_recorder_lock(name: &str) -> Option<std::fs::File> {
    use std::os::fd::AsRawFd as _;
    let path = recorder_lock(name);
    std::fs::create_dir_all(path.parent()?).ok()?;
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path).ok()?;
    // SAFETY: flock on a descriptor of ours, never waiting.
    (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(file)
}

/// A running screen with no recorder gets one (as an agent acts on it:
/// nothing looks otherwise). A recorder can end while its screen goes
/// on: stopped by hand, failed, or stopped as "still the old version"
/// by an upgrade from before it knew how to become the new one, which
/// left five running screens with no frames kept (2026-11-18).
pub fn ensure_recorder(name: &str) {
    let Ok(screen) = get(name) else { return };
    if !screen.alive() {
        return;
    }
    if let Some(lock) = take_recorder_lock(name) {
        drop(lock);
        tracing::info!("screen {name} had no recorder: starting one");
        start_recorder(name, screen.pid);
    }
}

/// The person's accessibility bus (at-spi's), asked of their session bus
/// (starting it if need be), for a screen's programs to use: where
/// `olo-screen tree` reads GTK and Qt programs.
fn person_a11y_bus() -> Option<String> {
    // (A bus that leaves a call to a name nobody has unanswered must not
    // hold the screen up.)
    let conn = zbus::blocking::connection::Builder::session().ok()?.method_timeout(Duration::from_secs(2)).build().ok()?;
    let reply = conn.call_method(Some("org.a11y.Bus"), "/org/a11y/bus", Some("org.a11y.Bus"), "GetAddress", &()).ok()?;
    reply.body().deserialize::<String>().ok().filter(|a| !a.is_empty())
}

/// "session: WAYLAND_DISPLAY=wayland-4 OLO_SOCKET=/run/…" → its parts.
fn sockets(line: &str) -> Option<(String, PathBuf)> {
    let rest = line.strip_prefix("session: WAYLAND_DISPLAY=")?;
    let (display, socket) = rest.split_once(" OLO_SOCKET=")?;
    Some((display.to_owned(), PathBuf::from(socket.trim())))
}

/// The screen at the fastest refresh its size has (120 Hz): watched live
/// (`olo_capture::live`), it moves as smoothly as the person's own
/// screen. At rest it draws nothing either way; busy, it costs about the
/// same (a window dragged for five seconds: 4.2% of a core at 60 Hz,
/// 4.4% at 120).
fn fastest(screen: &Screen) {
    let Ok(Reply::Outputs { outputs }) = screen.request(&Request::Outputs) else { return };
    for o in outputs {
        let Some(now) = o.current_mode else { continue };
        let best = o.modes.iter().filter(|m| (m.width, m.height) == (now.width, now.height)).max_by_key(|m| m.refresh);
        if let Some(best) = best.filter(|b| b.refresh > now.refresh) {
            let config = olo_protocol::OutputConfig { mode: Some(best.clone()), ..Default::default() };
            if let Err(e) = screen.request(&Request::ConfigureOutput { name: o.name.clone(), config }) {
                tracing::warn!("screen {}: its display stays at {} Hz: {e}", screen.name, now.refresh / 1000);
            }
        }
    }
}

fn stop_pid(pid: i32) {
    // The session ends app_server and the programs it started.
    // SAFETY: plain kill(2).
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
}

/// Where screen `name`'s recorder has app_server put each frame it reads
/// (`record::run`).
pub fn recorder_frame(name: &str) -> PathBuf {
    dir().join("views").join(format!("record-{name}.rgba"))
}

/// A file removed when this goes.
pub struct Removed(pub PathBuf);

impl Drop for Removed {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Remove the recorders' frames of screens not running: a recorder ended
/// with its screen's process group leaves its frame behind, and the
/// runtime folder is memory (six stopped screens' frames were there,
/// 31 MB, 2026-11-18).
fn sweep_recorder_frames() {
    let Ok(rd) = std::fs::read_dir(dir().join("views")) else { return };
    let running: Vec<String> = list().into_iter().map(|s| s.name).collect();
    for e in rd.flatten() {
        let file = e.file_name().to_string_lossy().into_owned();
        if let Some(screen) = file.strip_prefix("record-").and_then(|f| f.strip_suffix(".rgba"))
            && !running.iter().any(|r| r == screen)
        {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Remove `NAME.control` ONLY if nobody currently holds the exclusive lock on
/// `NAME.control.lock` and `NAME.control` (never unlinking a live controller's
/// replacement marker).
fn remove_unlocked_control_marker(rt_dir: &Path, name: &str) {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
    let Ok(stable) = open_stable_lock(&control_lock_file(rt_dir, name)) else { return };
    if unsafe { libc::flock(stable.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return;
    }
    let path = control_file(rt_dir, name);
    let Ok(file) = std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC).open(&path) else {
        return;
    };
    let Ok(fm) = file.metadata() else { return };
    if !fm.file_type().is_file() {
        return;
    }
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0
        && std::fs::symlink_metadata(&path).is_ok_and(|pm| pm.file_type().is_file() && pm.dev() == fm.dev() && pm.ino() == fm.ino())
    {
        let _ = std::fs::remove_file(&path);
    }
}

fn stop_locked(rt_dir: &Path, name: &str, expected_started: Option<u64>) -> Result<(), String> {
    let screen = get_in(rt_dir, name)?;
    if let Some(expected) = expected_started
        && screen.started != expected
    {
        return Err(format!("screen {name} was replaced by a newer session; refusing to stop it"));
    }
    // The person's programs on it back to their display first.
    crate::moving::send_home(&screen);
    end_sessions(&[screen.pid]);
    end_strays(Some(name));
    // Programs `run` started are in process groups of their own, tied to
    // the screen's display: they end when it goes (Wayland clients exit
    // when their compositor does).
    if screen.running() {
        // (Listed still, so it isn't forgotten while it runs: stopped again,
        // or by the next start of its name.)
        return Err(format!("screen {name} is still ending"));
    }
    let _ = std::fs::remove_file(rt_dir.join(format!("{name}.json")));
    let _ = std::fs::remove_file(rt_dir.join(format!("{name}-settings.json")));
    let _ = std::fs::remove_file(rt_dir.join(format!("{name}.owner.json")));
    remove_unlocked_control_marker(rt_dir, name);
    sweep_recorder_frames();
    Ok(())
}

/// End a screen and everything started on it.
pub fn stop(name: &str) -> Result<(), String> {
    stop_if_generation(name, None)
}

/// End screen `name`, verifying under its lifecycle lock that its session's
/// `started` generation still matches `expected_started` (when supplied).
pub fn stop_if_generation(name: &str, expected_started: Option<u64>) -> Result<(), String> {
    stop_if_generation_in(&dir(), name, expected_started)
}

pub(crate) fn stop_if_generation_in(rt_dir: &Path, name: &str, expected_started: Option<u64>) -> Result<(), String> {
    let _life = lock_lifecycle_in(rt_dir, name)?;
    stop_locked(rt_dir, name, expected_started)
}

// ---- Input -----------------------------------------------------------------

pub fn click(x: f64, y: f64, button: u32, count: u32) -> Vec<SyntheticEvent> {
    let mut events = vec![SyntheticEvent::Move { x, y }];
    for _ in 0..count.max(1) {
        events.push(SyntheticEvent::Button { button, pressed: true });
        events.push(SyntheticEvent::Button { button, pressed: false });
    }
    events
}

/// A drag from `from` to `to` in `steps` moves over `duration`, pausing
/// a little after the press and before the release, as a hand does (a
/// scroll bar's knob or a window's corner needs to see the press, then
/// movement over time).
pub fn drag(from: (f64, f64), to: (f64, f64), steps: u32, duration: Duration) -> Vec<(Vec<SyntheticEvent>, Duration)> {
    const SETTLE: Duration = Duration::from_millis(50);
    let steps = steps.max(1);
    let each = duration / steps;
    let mut out = vec![(vec![SyntheticEvent::Move { x: from.0, y: from.1 }, SyntheticEvent::Button { button: LEFT, pressed: true }], SETTLE)];
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        out.push((vec![SyntheticEvent::Move { x: from.0 + (to.0 - from.0) * t, y: from.1 + (to.1 - from.1) * t }], if i == steps { SETTLE } else { each }));
    }
    out.push((vec![SyntheticEvent::Button { button: LEFT, pressed: false }], Duration::ZERO));
    out
}

/// A touchpad scroll by (dx, dy) pixels: moves of at most 20 pixels every
/// 8 ms (as fingers report), then, unless `lift` is false, the fingers
/// resting a moment and lifting. Resting first means nothing glides on
/// (app_server glides a lift that comes while the fingers move fast), and
/// the lift ends the gesture, so a list pulled past its end springs back.
pub fn scroll(dx: f64, dy: f64, lift: bool) -> Vec<(Vec<SyntheticEvent>, Duration)> {
    const STEP: Duration = Duration::from_millis(8);
    let n = (dx.abs().max(dy.abs()) / 20.0).ceil().max(1.0) as u32;
    let mut out: Vec<_> = (0..n).map(|_| (vec![SyntheticEvent::Scroll { dx: dx / n as f64, dy: dy / n as f64, at_us: None }], STEP)).collect();
    if lift {
        if let Some(last) = out.last_mut() {
            last.1 = Duration::from_millis(80);
        }
        out.push((vec![SyntheticEvent::Scroll { dx: 0.0, dy: 0.0, at_us: None }], Duration::ZERO));
    }
    out
}

/// Mouse-wheel notches, one at a time (a toolkit may take only so much of
/// one event), a little apart.
pub fn wheel(dx: i32, dy: i32) -> Vec<(Vec<SyntheticEvent>, Duration)> {
    let n = dx.abs().max(dy.abs());
    (0..n)
        .map(|i| {
            let step = |d: i32| if i < d.abs() { d.signum() } else { 0 };
            (vec![SyntheticEvent::Wheel { dx: step(dx), dy: step(dy) }], Duration::from_millis(10))
        })
        .collect()
}

/// evdev codes of the modifiers.
const CTRL: u32 = 29;
const SHIFT: u32 = 42;
const ALT: u32 = 56;
const CMD: u32 = 125;

/// A chord as people write it: "Cmd+S", "ctrl+shift+t", "Enter", "Esc".
pub fn parse_chord(text: &str) -> Result<Chord, String> {
    let mut parts: Vec<String> = text.split('+').map(|p| p.trim().to_owned()).collect();
    if let Some(key) = parts.last_mut() {
        // As other tools name keys too: X11's (Page_Down, Prior, Next,
        // KP_Enter: xdotool's) and the web's (ArrowDown, PageDown), any
        // case, underscores or not (Page_Down was "unknown" nine times on
        // one screen).
        let lower = key.to_ascii_lowercase();
        let squashed = if lower.len() > 1 { lower.replace('_', "") } else { lower };
        let alias = match squashed.as_str() {
            "enter" | "return" => Some("Return"),
            "esc" => Some("Escape"),
            "del" => Some("Delete"),
            "pgup" | "pageup" | "prior" => Some("PageUp"),
            "pgdn" | "pgdown" | "pagedown" | "next" => Some("PageDown"),
            "arrowup" | "uparrow" => Some("Up"),
            "arrowdown" | "downarrow" => Some("Down"),
            "arrowleft" | "leftarrow" => Some("Left"),
            "arrowright" | "rightarrow" => Some("Right"),
            "kpenter" => Some("KeypadEnter"),
            "bksp" => Some("Backspace"),
            "space" | " " => Some("Space"),
            "-" => Some("Minus"),
            "=" => Some("Equal"),
            "," => Some("Comma"),
            "." => Some("Period"),
            "/" => Some("Slash"),
            ";" => Some("Semicolon"),
            "'" => Some("Apostrophe"),
            "`" => Some("Grave"),
            "[" => Some("BracketLeft"),
            "]" => Some("BracketRight"),
            "\\" => Some("Backslash"),
            _ => None,
        };
        if let Some(a) = alias {
            *key = a.to_owned();
        }
    }
    parts.join("+").parse()
}

/// Press a chord's modifiers, then its key, then let all go.
pub fn chord(c: &Chord) -> Vec<SyntheticEvent> {
    let mods: Vec<u32> = [(c.ctrl, CTRL), (c.alt, ALT), (c.shift, SHIFT), (c.cmd, CMD)].iter().filter(|(on, _)| *on).map(|(_, k)| *k).collect();
    let mut events: Vec<SyntheticEvent> = mods.iter().map(|&key| SyntheticEvent::Key { key, pressed: true }).collect();
    events.push(SyntheticEvent::Key { key: c.key, pressed: true });
    events.push(SyntheticEvent::Key { key: c.key, pressed: false });
    events.extend(mods.iter().rev().map(|&key| SyntheticEvent::Key { key, pressed: false }));
    events
}

/// The key (and whether Shift) that types `c` on a US keyboard.
pub fn key_for(c: char) -> Option<(u32, bool)> {
    const ROWS: [(&str, &str, u32); 4] = [("`1234567890-=", "~!@#$%^&*()_+", 41), ("qwertyuiop[]\\", "QWERTYUIOP{}|", 16), ("asdfghjkl;'", "ASDFGHJKL:\"", 30), ("zxcvbnm,./", "ZXCVBNM<>?", 44)];
    match c {
        ' ' => return Some((57, false)),
        '\n' => return Some((28, false)),
        '\t' => return Some((15, false)),
        _ => {}
    }
    for (plain, shifted, first) in ROWS {
        // The number row starts with ` (41), then 1 (2) onward.
        let code = |i: usize| {
            if first == 41 {
                if i == 0 { 41 } else { 1 + i as u32 }
            } else if first == 16 && i == 12 {
                43
            } else {
                first + i as u32
            }
        };
        if let Some(i) = plain.chars().position(|p| p == c) {
            return Some((code(i), false));
        }
        if let Some(i) = shifted.chars().position(|p| p == c) {
            return Some((code(i), true));
        }
    }
    None
}

/// `text` in runs: what a US keyboard types (true), and what it doesn't.
pub fn runs(text: &str) -> Vec<(bool, &str)> {
    let mut out: Vec<(bool, &str)> = Vec::new();
    let mut start = 0;
    let mut kind = None;
    for (i, c) in text.char_indices() {
        let keys = key_for(c).is_some();
        if kind.is_some_and(|k| k != keys) {
            out.push((kind.unwrap_or(true), &text[start..i]));
            start = i;
        }
        kind = Some(keys);
    }
    if let Some(k) = kind {
        out.push((k, &text[start..]));
    }
    out
}

/// Type `text` (what a US keyboard can type).
pub fn type_text(text: &str) -> Result<Vec<SyntheticEvent>, String> {
    let mut events = Vec::new();
    for c in text.chars() {
        let (key, shift) = key_for(c).ok_or_else(|| format!("{c:?} can't be typed (only what a US keyboard types)"))?;
        if shift {
            events.push(SyntheticEvent::Key { key: SHIFT, pressed: true });
        }
        events.push(SyntheticEvent::Key { key, pressed: true });
        events.push(SyntheticEvent::Key { key, pressed: false });
        if shift {
            events.push(SyntheticEvent::Key { key: SHIFT, pressed: false });
        }
    }
    Ok(events)
}

#[cfg(test)]
mod tests {

    /// A screen's session is known by the `OLO_SCREEN` it was started
    /// with, not its command line, which an upgrade's re-execution left
    /// bare (every screen then looked ended and was forgotten).
    /// A new screen's name is one no screen has: not a stopped or
    /// archived one's, though a running one alone leaves "agent" free
    /// for `olo-screen start`.
    #[test]
    fn a_new_screens_name() {
        let named = |names: &[&str]| first_free(&names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>());
        assert_eq!(named(&[]), "agent");
        assert_eq!(named(&["agent"]), "agent-2");
        assert_eq!(named(&["agent", "agent-2", "qa", "agent-4"]), "agent-3");
        assert_eq!(named(&["agent-2"]), "agent");
        // What's kept counts: a stopped screen's folder, an archived
        // one's, and one with only a browser's profile left in it.
        let dir = std::path::Path::new("/var/tmp").join(format!("olo-screen-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (name, file) in [("agent", "screen.json"), ("agent-2", "archived"), ("agent-3", "chrome/Default/Preferences")] {
            let path = dir.join(name).join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"{}").unwrap();
        }
        assert_eq!(new_name_among(Vec::new(), &dir), "agent-4");
        assert_eq!(new_name_among(vec!["agent-4".to_owned()], &dir), "agent-5");
        assert_eq!(new_name_among(Vec::new(), &dir.join("none")), "agent");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_screens_session_is_known_by_its_environment() {
        let env = b"HOME=/home/me\0OLO_SCREEN=claude-browser\0WAYLAND_DISPLAY=\0";
        // Started, and re-executed bare after an upgrade: both its.
        assert!(is_session_of(b"/home/me/.local/bin/olo-session\0--headless\x001440x900\0", env, "claude-browser"));
        assert!(is_session_of(b"/home/me/.local/bin/olo-session\0", env, "claude-browser"));
        // Another screen's, the person's own session, another program.
        assert!(!is_session_of(b"/home/me/.local/bin/olo-session\0", env, "claude"));
        assert!(!is_session_of(b"/home/me/.local/bin/olo-session\0", b"HOME=/home/me\0", "claude-browser"));
        assert!(!is_session_of(b"/usr/bin/bash\0", env, "claude-browser"));
    }

    #[test]
    fn a_session_under_another_runtime_folder_is_not_a_stray() {
        // (A stray is judged among the screens listed here: one started
        // under another folder is listed there.)
        let here = Path::new("/run/user/1000");
        assert!(listed_in(b"HOME=/home/me\0XDG_RUNTIME_DIR=/run/user/1000\0OLO_SCREEN=agent\0", here));
        assert!(listed_in(b"XDG_RUNTIME_DIR=/run/user/1000/\0OLO_SCREEN=agent\0", here) || !here.exists());
        assert!(!listed_in(b"HOME=/var/tmp/shots/home\0XDG_RUNTIME_DIR=/var/tmp/shots/run\0OLO_SCREEN=readme\0", here));
        assert!(!listed_in(b"XDG_RUNTIME_DIR=/run/user/1000\0OLO_SCREEN=qa\0", Path::new("/var/tmp/shots/run")));
        // No folder said: the one olo-screen itself falls back to.
        assert!(listed_in(b"OLO_SCREEN=agent\0", &fallback_runtime_dir()));
    }

    /// When a process started, past a name with spaces and parentheses.
    #[test]
    fn a_process_start_from_its_stat() {
        let stat = "4102 (olo (x) session) S 4061 4102 4102 0 -1 4194560 1 2 3 4 5 6 7 8 20 0 12 0 987654 1000 200 18446744073709551615";
        assert_eq!(started_ticks(stat), Some(987654));
    }

    use super::*;

    /// Chrome on a screen keeps a profile of the screen's own, so it
    /// runs there rather than handing its window to the person's Chrome.
    #[test]
    fn an_agent_waits_while_the_person_controls() {
        let dir = PathBuf::from("/var/tmp").join(format!("olo-screen-pane-control-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!controlled_in(&dir, "agent"));
        let mut lock = try_acquire_control_in(&dir, "agent").unwrap();
        assert!(controlled_in(&dir, "agent") && !controlled_in(&dir, "agent-2"));
        lock.release();
        assert!(!controlled_in(&dir, "agent"));
        // A viewer that has gone (another program on that pid, or none)
        // controls nothing.
        std::fs::write(control_file(&dir, "agent"), "1").unwrap();
        assert!(!controlled_in(&dir, "agent"));
        // Started again (an upgrade's exec: the same pid), it forgets
        // an unlocked leftover lock file with its own pid.
        std::fs::write(control_file(&dir, "agent"), std::process::id().to_string()).unwrap();
        clear_controlled_in(&dir);
        assert!(!controlled_in(&dir, "agent"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn astra_f2_control_lock_excludes_second_viewer_and_preserves_owner() {
        let dir = PathBuf::from("/var/tmp").join(format!("olo-screen-pane-f2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut first = try_acquire_control_in(&dir, "qa").expect("first viewer acquires lock");
        let ctrl_path = control_file(&dir, "qa");
        assert!(ctrl_path.starts_with(&dir));
        assert_eq!(std::fs::read_to_string(&ctrl_path).unwrap().trim(), std::process::id().to_string());

        // Second viewer fails without truncating or removing the first's lock.
        let second = try_acquire_control_in(&dir, "qa");
        assert!(second.is_err(), "second viewer must be refused while first holds lock");
        assert!(ctrl_path.is_file(), "refused acquisition must leave lock file intact");
        assert_eq!(std::fs::read_to_string(&ctrl_path).unwrap().trim(), std::process::id().to_string());

        // `clear_controlled_in` must not remove an actively flocked file either.
        clear_controlled_in(&dir);
        assert!(ctrl_path.is_file(), "active flocked control file must survive clear_controlled_in");

        // Releasing first removes the file and allows another viewer to acquire.
        first.release();
        assert!(!ctrl_path.exists());
        let third = try_acquire_control_in(&dir, "qa").expect("lock free after first releases");
        drop(third);
        assert!(!ctrl_path.exists());

        // Invalid/path-shaped screen name is refused before touching disk.
        assert!(try_acquire_control_in(&dir, "../escape").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn astra_f28_non_executable_sibling_does_not_mask_path_recorder() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = PathBuf::from("/var/tmp").join(format!("olo-screen-pane-f28-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sib_dir = dir.join("sib");
        let path_dir = dir.join("bin");
        std::fs::create_dir_all(&sib_dir).unwrap();
        std::fs::create_dir_all(&path_dir).unwrap();

        let fake_rex = sib_dir.join("olo-rex");
        let bad_sibling = sib_dir.join("olo-screen");
        let good_bin = path_dir.join("olo-screen");
        std::fs::write(&fake_rex, b"").unwrap();
        std::fs::write(&bad_sibling, b"not executable").unwrap();
        std::fs::set_permissions(&bad_sibling, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::write(&good_bin, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&good_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        let found = resolve_recorder_candidates_from(Some(&fake_rex), Some(path_dir.as_os_str()));
        assert_eq!(found.first().map(PathBuf::as_path), Some(good_bin.as_path()), "non-executable sibling must not mask executable on PATH");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn controller_exe_accepts_screen_rex_and_rex_link() {
        assert!(is_controller_exe(Path::new("/usr/local/bin/olo-screen")));
        assert!(is_controller_exe(Path::new("/usr/local/bin/olo-screen (deleted)")));
        assert!(is_controller_exe(Path::new("/usr/local/bin/olo-rex")));
        assert!(is_controller_exe(Path::new("/home/user/.cache/olo-rex-link-abc123def")));
        assert!(is_controller_exe(Path::new("/home/user/.cache/olo-rex-link-abc123def (deleted)")));
        assert!(!is_controller_exe(Path::new("/usr/lib/systemd/systemd")));
        assert!(!is_controller_exe(Path::new("/usr/bin/bash")));
    }

    #[test]
    fn chrome_has_a_profile_of_its_own() {
        let s = Screen { name: "work".into(), wayland_display: "wayland-9".into(), olo_socket: "/x".into(), pid: 1, width: 1, height: 1, scale: 1.0, sound: false, started: 0, bus: None, a11y: None };
        let env: std::collections::HashMap<String, String> = s.env().into_iter().collect();
        let home = &env["CHROME_CONFIG_HOME"];
        assert!(home.ends_with("/screens/work"), "{home}");
        assert_ne!(Some(home.as_str()), std::env::var("CHROME_CONFIG_HOME").ok().as_deref());
    }

    #[test]
    fn keys_for_text() {
        assert_eq!(key_for('a'), Some((30, false)));
        assert_eq!(key_for('A'), Some((30, true)));
        assert_eq!(key_for('1'), Some((2, false)));
        assert_eq!(key_for('0'), Some((11, false)));
        assert_eq!(key_for('='), Some((13, false)));
        assert_eq!(key_for('`'), Some((41, false)));
        assert_eq!(key_for('~'), Some((41, true)));
        assert_eq!(key_for('@'), Some((3, true)));
        assert_eq!(key_for('\\'), Some((43, false)));
        assert_eq!(key_for('|'), Some((43, true)));
        assert_eq!(key_for('/'), Some((53, false)));
        assert_eq!(key_for('"'), Some((40, true)));
        assert_eq!(key_for('é'), None);
        assert!(type_text("Hi!").unwrap().len() == 10);
        assert_eq!(runs("café 👍🏽 ok\n"), vec![(true, "caf"), (false, "é"), (true, " "), (false, "👍🏽"), (true, " ok\n")]);
        assert!(runs("").is_empty());
    }

    #[test]
    fn chords() {
        let c = parse_chord("cmd+shift+t").unwrap();
        assert!(c.cmd && c.shift && !c.ctrl && c.key == 20);
        assert_eq!(parse_chord("Enter").unwrap().key, 28);
        assert_eq!(parse_chord("esc").unwrap().key, 1);
        assert_eq!(parse_chord("ctrl+/").unwrap().key, 53);
        assert!(parse_chord("hyper+x").is_err());
        // Other tools' names: X11's (xdotool's) and the web's.
        let page_down = parse_chord("PageDown").unwrap().key;
        for name in ["Page_Down", "page_down", "pgdn", "Next"] {
            assert_eq!(parse_chord(name).unwrap().key, page_down, "{name}");
        }
        let page_up = parse_chord("PageUp").unwrap().key;
        for name in ["Page_Up", "Prior", "pgup"] {
            assert_eq!(parse_chord(name).unwrap().key, page_up, "{name}");
        }
        assert_eq!(parse_chord("ArrowDown").unwrap().key, parse_chord("Down").unwrap().key);
        assert_eq!(parse_chord("shift+Arrow_Left").unwrap().key, parse_chord("Left").unwrap().key);
        assert_eq!(parse_chord("KP_Enter").unwrap().key, parse_chord("KeypadEnter").unwrap().key);
        assert_eq!(parse_chord("ctrl+-").unwrap().key, parse_chord("Minus").unwrap().key);
        let events = chord(&parse_chord("Cmd+W").unwrap());
        assert_eq!(events.first(), Some(&SyntheticEvent::Key { key: CMD, pressed: true }));
        assert_eq!(events.last(), Some(&SyntheticEvent::Key { key: CMD, pressed: false }));
    }

    #[test]
    fn paced_input() {
        // A scroll in finger-sized steps, resting before the lift.
        let s = scroll(0.0, -300.0, true);
        assert_eq!(s.len(), 16);
        assert!(s[..15].iter().all(|(e, _)| e == &vec![SyntheticEvent::Scroll { dx: 0.0, dy: -20.0, at_us: None }]));
        assert_eq!(s[14].1, Duration::from_millis(80));
        assert_eq!(s[15].0, vec![SyntheticEvent::Scroll { dx: 0.0, dy: 0.0, at_us: None }]);
        assert_eq!(scroll(0.0, 30.0, false).len(), 2);
        // A notch at a time, both ways at once.
        let w = wheel(1, -3);
        assert_eq!(
            w.iter().map(|(e, _)| e[0].clone()).collect::<Vec<_>>(),
            vec![SyntheticEvent::Wheel { dx: 1, dy: -1 }, SyntheticEvent::Wheel { dx: 0, dy: -1 }, SyntheticEvent::Wheel { dx: 0, dy: -1 }]
        );
        // Press, the moves, ending where asked, then let go.
        let d = drag((0.0, 0.0), (100.0, 50.0), 4, Duration::from_millis(200));
        assert_eq!(d.len(), 6);
        assert_eq!(d[4].0, vec![SyntheticEvent::Move { x: 100.0, y: 50.0 }]);
        assert_eq!(d[5].0, vec![SyntheticEvent::Button { button: LEFT, pressed: false }]);
    }

    #[test]
    fn names_and_sockets() {
        assert!(valid_name("agent-2") && valid_name("claude_1"));
        assert!(!valid_name("") && !valid_name("a/b") && !valid_name("x y"));
        assert_eq!(sockets("session: WAYLAND_DISPLAY=wayland-4 OLO_SOCKET=/run/user/1000/olo-wayland-4.sock"), Some(("wayland-4".into(), "/run/user/1000/olo-wayland-4.sock".into())));
        assert_eq!(sockets("olo-session: hello"), None);
    }

    #[test]
    fn settings_never_lock_or_sleep() {
        let mut person = Settings::default();
        person.power.sleep_minutes = 15;
        person.screen_saver.enabled = true;
        person.display.scale = 2.0;
        let s = screen_settings(Some(person), 1.0);
        assert_eq!((s.power.sleep_minutes, s.power.display_off_minutes, s.screen_saver.enabled, s.display.scale), (0, 0, false, 1.0));
    }

    #[test]
    fn review2_f5_and_f10_control_lock_safety_and_fifo_symlink_rejection() {
        use std::os::unix::fs::MetadataExt as _;
        let dir = PathBuf::from("/var/tmp").join(format!("olo-screen-pane-r2-f5-f10-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 1. Finding #5: if a replacement `.control` inode appears while an old
        //    `ControlLock` still exists, dropping the old handle MUST NOT unlink
        //    the new controller's marker.
        let mut old = try_acquire_control_in(&dir, "qa").unwrap();
        let ctrl = control_file(&dir, "qa");
        // Simulate an external replacement moving the old path aside and creating
        // a brand-new `.control` inode.
        let moved = dir.join("qa.control.old");
        std::fs::rename(&ctrl, &moved).unwrap();
        std::fs::write(&ctrl, "999999").unwrap();
        let new_ino = std::fs::symlink_metadata(&ctrl).unwrap().ino();
        old.release();
        assert!(ctrl.is_file(), "old ControlLock drop must never unlink replacement .control");
        assert_eq!(std::fs::symlink_metadata(&ctrl).unwrap().ino(), new_ino);
        let _ = std::fs::remove_file(&ctrl);
        let _ = std::fs::remove_file(&moved);

        // 2. Finding #10: FIFO at `.control` must be rejected immediately (never hang)
        //    and a symlink at `.control` must be rejected (never truncate target).
        let fifo = control_file(&dir, "fifo");
        assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        let dir_c = dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(try_acquire_control_in(&dir_c, "fifo").is_err());
        });
        assert!(rx.recv_timeout(Duration::from_secs(2)).expect("FIFO .control must not hang"), "FIFO .control must be rejected");

        let victim = dir.join("victim.txt");
        std::fs::write(&victim, b"untouched").unwrap();
        std::os::unix::fs::symlink(&victim, control_file(&dir, "sym")).unwrap();
        assert!(try_acquire_control_in(&dir, "sym").is_err(), "symlink .control must be rejected");
        assert_eq!(std::fs::read(&victim).unwrap(), b"untouched", "symlink target must not be truncated");

        // 3. Finding #8: stale PID-reuse start_ticks mismatch is treated as NOT controlled.
        let stale = control_file(&dir, "stale");
        let stale_lock = control_lock_file(&dir, "stale");
        std::fs::write(&stale, std::process::id().to_string()).unwrap();
        std::fs::write(&stale_lock, format!("{} 1", std::process::id())).unwrap();
        assert!(!controlled_in(&dir, "stale"), "mismatched start_ticks in lock sidecar must not report controlled");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn review2_f36_action_lease_serializes_with_take_control() {
        let dir = PathBuf::from("/var/tmp").join(format!("olo-screen-pane-r2-f36-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // While an agent CLI action holds `ActionLease` (`LOCK_SH`), Take Control
        // (`LOCK_EX`) must be refused.
        let lease = acquire_action_lease_in(&dir, "qa").expect("action lease");
        assert!(try_acquire_control_in(&dir, "qa").is_err(), "Take Control must not preempt an in-flight agent action");
        drop(lease);

        // Once the action finishes, Take Control succeeds and blocks new actions.
        let ctrl = try_acquire_control_in(&dir, "qa").expect("Take Control after action ends");
        assert!(acquire_action_lease_in(&dir, "qa").is_err(), "agent action must not start while Take Control holds LOCK_EX");
        drop(ctrl);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn review2_f3_generation_guards_refuse_replaced_stop_and_running_clear_delete() {
        use std::os::unix::process::CommandExt as _;
        let dir = PathBuf::from("/var/tmp").join(format!("olo-screen-pane-r2-f3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let rt = dir.join("rt");
        let state = dir.join("state");
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::create_dir_all(state.join("qa")).unwrap();
        std::fs::write(state.join("qa/actions.jsonl"), "{}\n").unwrap();

        // Spawn a real dummy process whose argv[0] ends with `olo-session` and
        // `OLO_SCREEN=qa` so `Screen::running()` validates it identically to a
        // live session.
        let mut fake_session = std::process::Command::new("/usr/bin/sleep").arg0("olo-session").arg("30").env("OLO_SCREEN", "qa").spawn().expect("spawn dummy session");
        let sock = rt.join("olo-test.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let scr = Screen {
            name: "qa".into(),
            wayland_display: "wayland-test".into(),
            olo_socket: sock,
            pid: fake_session.id() as i32,
            width: 800,
            height: 600,
            scale: 1.0,
            sound: false,
            started: 2000,
            bus: None,
            a11y: None,
        };
        std::fs::write(rt.join("qa.json"), serde_json::to_vec(&scr).unwrap()).unwrap();

        // Stop with older generation 1000 must refuse to stop generation 2000!
        let err = stop_if_generation_in(&rt, "qa", Some(1000)).unwrap_err();
        assert!(err.contains("replaced"), "expected replaced-session refusal, got: {err}");
        // Clear & Delete while running must both refuse!
        assert!(clear_history_if_stopped_in(&rt, &state, "qa").is_err(), "clear must refuse running screen");
        assert!(delete_if_stopped_in(&rt, &state, "qa").is_err(), "delete must refuse running screen");
        assert!(state.join("qa/actions.jsonl").is_file());

        let _ = fake_session.kill();
        let _ = fake_session.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn review2_f42_and_f43_option_names_rejected_and_runtime_never_tmp() {
        assert!(!valid_name("-x"));
        assert!(!valid_name("--clear"));
        assert!(!valid_name("_leading_underscore"));
        assert!(valid_name("agent-1_ok"));
        let fb = fallback_runtime_dir();
        assert!(!fb.starts_with("/tmp"), "fallback runtime dir must not live in /tmp: {}", fb.display());
    }

    #[test]
    fn review2_f57_recorder_spawn_falls_through_failing_candidate() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = PathBuf::from("/var/tmp").join(format!("olo-screen-pane-r2-f57-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // First candidate has +x bit but invalid ELF/shebang (spawn returns ENOEXEC).
        let bad = dir.join("bad-screen");
        std::fs::write(&bad, b"#!/nonexistent/interpreter/path\n").unwrap();
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Second candidate is a real executable script that writes a marker file.
        let marker = dir.join("ran.txt");
        let good = dir.join("good-screen");
        std::fs::write(&good, format!("#!/bin/sh\necho ok > {}\n", marker.display())).unwrap();
        std::fs::set_permissions(&good, std::fs::Permissions::from_mode(0o755)).unwrap();

        spawn_recorder_from_candidates(&[bad, good], "qa", 0, None).expect("must fall through failing candidate to working one");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !marker.is_file() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.is_file(), "working fallback candidate must have executed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
