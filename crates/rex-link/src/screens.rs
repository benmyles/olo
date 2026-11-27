//! `olo-rex-link screen …`: Agent Screens of this host's own. An agent on
//! a Rex host has had a screen on the person's desktop, asked through
//! Rex (`olo screen …`, the bridge): the desktop had to be connected and
//! awake, and the screen drew there. This is the same thing here (Ben,
//! 2026-11-26: "there should be an option for agents on a rex host to use
//! olo agent screens on the host itself … the host's olo agent screens
//! should use software rendering and provide a full olo desktop for the
//! agent and it should also have chrome installed and ready to use").
//!
//! A screen is what it is on the desktop: `olo-screen`'s, a headless Olo
//! session with Deskbar, Tracker's desktop and every app, drawn in
//! software where the host has no GPU (app_server takes Mesa's software
//! device by itself). So Olo itself is put on the host, under the account
//! and nowhere else, in a place of its own: `~/.local/share/olo/desktop`
//! (`bin`, `share/olo`), from Olo's release on GitHub, checked, without
//! its wallpapers (1.6 GB the screens do without). Its programs find
//! their data beside them (`olo_xdg::data_dirs`). Chrome is put in the
//! home folder by Olo's own `olo-chrome` (Google's package unpacked
//! there: no root, nothing outside the home). And a font with emoji,
//! where the host has none (a server seldom has; every emoji is a box
//! then, in Olo's windows and on a page): Noto Color Emoji, from its
//! makers' repository at one commit, checked against the SHA-256 written
//! here, into the account's fonts (`~/.local/share/fonts/olo-screens`),
//! where fontconfig finds it for Olo's programs and Chrome alike.
//!
//! ```text
//! olo-rex-link screen install [--tag TAG | --from ARCHIVE] [--again] [--no-chrome] [--no-emoji-font]
//!     Olo put here (the newest release; the one tagged TAG; ARCHIVE, a
//!     file or a URL with its .sha256 beside it), or brought up to it
//!     (nothing fetched where that release is here already, unless
//!     `--again`; and not while screens run on the Olo that is here,
//!     which would each reload under whoever uses it, unless
//!     `--under-screens`: status 4, with which run),
//!     and Chrome: a line of JSON for each step as it goes, then what
//!     `status` says (status 3: it is put, and this host lacks something
//!     its screens need, which `missing` and `how` say)
//! olo-rex-link screen status
//!     what is here, as JSON: {"installed": VERSION|null, "prefix",
//!     "bytes" (Olo, Chrome and the font together), "chrome":
//!     VERSION|null, "missing": [libraries its
//!     programs need that this host hasn't], "older": {library:
//!     [versions of its symbols they need that this host's hasn't]},
//!     "software_drawing": true|false, "emoji_font": true|false (none:
//!     every emoji is a box, on Olo's windows and on a page), "default":
//!     "host"|"desktop", "committed": when the release here was made
//!     (seconds; by it a desktop knows its own is newer), "running":
//!     [the screens that run here, by name]}
//! olo-rex-link screen default [host|desktop]
//!     where `olo screen` goes on this host when it isn't said
//!     (`--on host`, `--on desktop`, `OLO_SCREENS_ON`); none: says it
//! olo-rex-link screen remove
//!     Olo taken off this host again (Chrome and the screens' own
//!     folders stay)
//! olo-rex-link screen COMMAND…
//!     `olo-screen COMMAND…` of the Olo here: start, run, look, tree,
//!     click, type, key, stop and the rest, as on the desktop
//! olo-rex-link screen start [NAME] [--keep] …
//!     the same, and the screen ends with the Rex tab it was started
//!     from (below); `--keep`: it stays until it is stopped
//! ```
//!
//! **A screen ends with the tab that started it.** On the desktop a
//! screen left running is seen by the person, in Agent Screens; on a
//! host nobody sees it, and one left by each agent that ever wanted a
//! page is a quarter of a gigabyte each, more with Chrome, for ever. So
//! a screen started from a Rex tab is stopped (kept, as a stopped one
//! is: its history, its Chrome profile) when that tab's own program
//! ends: the agent, or the shell. The tab is known by `REX_BLOCK`, and
//! its program is the furthest parent of ours started with the same one
//! (`/proc/PID/environ`). A keeper waits for that program's end, on a
//! file descriptor of the process itself (`pidfd_open`: no polling, and
//! no other process ever taken for it), and for the screen's own
//! session's the same way: a screen stopped first, by hand, leaves its
//! keeper nothing to do, and it ends. Outside a Rex tab there is no
//! such program to go by, and the screen stays until stopped, as with
//! `--keep`.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read as _, Write as _};
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::{DirBuilderExt as _, FileExt as _, MetadataExt as _, OpenOptionsExt as _};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use olo_rex_link::screen_wire::{EXIT_STALE, ExpectedScreen, GuardedAction, GuardedVerb, LifecycleCliAction, OwnerPane, PROTO, ScreenOwner, classify_lifecycle_cli_args, valid_screen_name};
use serde::Deserialize;
use serde_json::json;

const USAGE: &str = "usage: olo screen install [--tag TAG | --from ARCHIVE] [--again] [--under-screens] [--no-chrome] [--no-emoji-font]
                                            Olo and Chrome put on this host for screens here (and a font with emoji, if it has none)
       olo screen status                    what is here, as JSON
       olo screen default [host|desktop]    where `olo screen` goes on this host when it isn't said
       olo screen remove                    Olo taken off this host again
       olo screen follow                    follow screens and host status as NDJSON (zero polling)
       olo screen stream NAME               stream live frames of screen NAME (binary tile deltas)
       olo screen history-stream NAME       follow history & frame index, serve frame_at on stdin (binary wire)
       olo screen export NAME [OPTIONS]     export screen history MP4 with progress and streamed bytes
       olo screen --on host COMMAND…        olo-screen's own commands, on this host (start, run, look, tree, click, …)";

/// Commands that act on a screen and update `NAME.owner.json` when the
/// controlling Rex pane (`REX_BLOCK`) changes (matches `apps/screen/src/history.rs`).
const OWNER_ACTIONS: &[&str] = &[
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

/// Owned raw file descriptor closed automatically on drop.
pub(crate) struct OwnedFd(i32);

impl OwnedFd {
    pub(crate) fn new(fd: i32) -> Self {
        Self(fd)
    }

    pub(crate) fn raw(&self) -> i32 {
        self.0
    }

    pub(crate) fn into_raw(mut self) -> i32 {
        let fd = self.0;
        self.0 = -1;
        fd
    }

    pub(crate) fn try_dup(&self) -> Option<Self> {
        if self.0 < 0 {
            return None;
        }
        // SAFETY: duplicates our owned fd with close-on-exec set atomically.
        let dup = unsafe { libc::fcntl(self.0, libc::F_DUPFD_CLOEXEC, 0) };
        (dup >= 0).then(|| Self(dup))
    }
}

impl Drop for OwnedFd {
    fn drop(&mut self) {
        if self.0 >= 0 {
            // SAFETY: exclusively owned file descriptor.
            unsafe {
                libc::close(self.0);
            }
        }
    }
}

/// Runtime screen descriptor written by `olo-screen start` in
/// `$XDG_RUNTIME_DIR/olo-screens/NAME.json`.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RuntimeScreen {
    pub name: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub wayland_display: String,
    #[serde(alias = "tabula_socket")]
    pub olo_socket: PathBuf,
    pub pid: i32,
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_one")]
    pub scale: f64,
    #[serde(default)]
    pub sound: bool,
    /// Seconds since the Unix epoch when started (`None` on legacy descriptors
    /// lacking `started`; never conflated with `0`, Finding 25).
    #[serde(default)]
    pub started: Option<u64>,
}

fn default_one() -> f64 {
    1.0
}

/// Noto Color Emoji (SIL Open Font License 1.1), as its makers' own
/// repository has it at one commit (the release "unicode 18.0" of
/// 2026-09-24): 10.7 MB. What is fetched is refused unless it is this.
const EMOJI_FONT: &str = "NotoColorEmoji.ttf";
const EMOJI_FONT_URL: &str = "https://raw.githubusercontent.com/googlefonts/noto-emoji/e20cbc2bbec1926686be9f9bee7d1d2cfa1fea0e/2D/fonts/NotoColorEmoji.ttf";
const EMOJI_FONT_SHA256: &str = "15671215ab769fdc7162a045d56fd7d7e477c51b04e6b3c761d914d8fdd6cc44";

/// The account's own fonts (fontconfig's `<dir prefix="xdg">fonts</dir>`).
fn account_fonts() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home().join(".local/share")).join("fonts")
}

/// Where a font put here for the screens goes: a folder of its own
/// among the account's fonts, so `remove` knows what is Olo's.
fn screens_fonts() -> PathBuf {
    account_fonts().join("olo-screens")
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Where Olo is on a host, for its screens (`OLO_HOST_DESKTOP`: another
/// place, for Olo's own tests).
pub fn prefix() -> PathBuf {
    std::env::var_os("OLO_HOST_DESKTOP").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home().join(".local/share/olo/desktop"))
}

/// Where this host's default is kept: one word, `host` or `desktop`.
pub(crate) fn default_file() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home().join(".config")).join("olo/screens-on")
}

pub(crate) fn default_now() -> &'static str {
    let Some(bytes) = read_bounded_regular(&default_file(), 64) else {
        return "desktop";
    };
    match std::str::from_utf8(&bytes).unwrap_or_default().trim() {
        "host" => "host",
        _ => "desktop",
    }
}

/// Valid screen name (delegates to `screen_wire::valid_screen_name`, rejecting
/// option-shaped names beginning with `-`).
pub(crate) fn valid_name(name: &str) -> bool {
    valid_screen_name(name)
}

/// Round any positive sub-millisecond remainder up to 1 ms so `poll(2)` never
/// spins with `timeout_ms == 0` during the final fraction of a millisecond.
pub(crate) fn timeout_ms_ceil(rem: Duration) -> i32 {
    if rem.is_zero() {
        0
    } else {
        let ms = rem.as_millis().saturating_add(u128::from(rem.subsec_nanos() % 1_000_000 != 0));
        ms.min(i32::MAX as u128) as i32
    }
}

pub(crate) fn set_nonblocking(fd: i32) {
    // SAFETY: standard fcntl on valid fd.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

/// What fd condition aborts a blocked non-blocking stdout write.
#[derive(Clone, Copy)]
pub(crate) enum CancelFd {
    /// Any readability cancels (broadcast stop pipe or `signalfd`).
    Readable(i32),
    /// Stdin peer closure cancels without consuming buffered request bytes.
    StdinEof(i32),
}

/// True when `fd` is a socket (`S_IFSOCK`), so `poll` can request `POLLRDHUP`
/// instead of waking on unread input bytes (Finding 27).
fn fd_is_socket(fd: i32) -> bool {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `fstat` on a valid open descriptor.
    unsafe { libc::fstat(fd, &mut st) == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFSOCK }
}

/// True when `fd` (stdin pipe or socket) has reached peer EOF/hangup without
/// consuming any unread request bytes (Finding 27).
fn stdin_is_eof(_fd: i32, revents: i16) -> bool {
    revents & (libc::POLLRDHUP | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
}

/// Write all of `bytes` to non-blocking `fd`, aborting immediately with
/// `BrokenPipe` if any cancel fd triggers while waiting for `POLLOUT`.
///
/// For `CancelFd::StdinEof(f)` we poll with `events: 0` on pipes (Linux always
/// delivers `POLLHUP|POLLERR|POLLNVAL` on peer close without waking on unread
/// request bytes) and `events: POLLRDHUP` on sockets (waking only on peer write
/// shutdown, never on unread request bytes) so unread requests on stdin never
/// cause a 100% CPU busy-spin while stdout is backpressured (Finding 27).
pub(crate) fn write_all_cancellable(fd: i32, bytes: &[u8], cancels: &[CancelFd]) -> io::Result<()> {
    let mut written = 0usize;
    let cancel_specs: Vec<(i32, i16)> = cancels
        .iter()
        .map(|c| match *c {
            CancelFd::Readable(f) => (f, libc::POLLIN),
            CancelFd::StdinEof(f) => (f, if fd_is_socket(f) { libc::POLLRDHUP } else { 0 }),
        })
        .collect();
    let mut pfds = Vec::with_capacity(1 + cancels.len());
    while written < bytes.len() {
        // SAFETY: non-blocking write of valid slice to owned fd.
        let n = unsafe { libc::write(fd, bytes[written..].as_ptr().cast(), bytes.len() - written) };
        if n > 0 {
            written += n as usize;
            continue;
        }
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "write returned zero"));
        }
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if err.kind() != io::ErrorKind::WouldBlock {
            return Err(err);
        }

        pfds.clear();
        pfds.push(libc::pollfd { fd, events: libc::POLLOUT, revents: 0 });
        for &(cfd, events) in &cancel_specs {
            pfds.push(libc::pollfd { fd: cfd, events, revents: 0 });
        }
        // SAFETY: `pfds` holds valid open fds for the duration of `poll`.
        let r = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, -1) };
        if r < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(io::Error::last_os_error());
        }
        if pfds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "stdout closed"));
        }
        for (idx, c) in cancels.iter().enumerate() {
            let rev = pfds[1 + idx].revents;
            if rev == 0 {
                continue;
            }
            let cancelled = match *c {
                CancelFd::Readable(_) => true,
                CancelFd::StdinEof(f) => stdin_is_eof(f, rev),
            };
            if cancelled {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "cancelled while writing stdout"));
            }
        }
    }
    Ok(())
}

/// Open a regular file without following symlinks and without ever blocking on
/// a FIFO, verifying `is_file()` and capping reads at `max_bytes`.
pub(crate) fn read_bounded_regular(path: &Path, max_bytes: usize) -> Option<Vec<u8>> {
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW).open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.file_type().is_file() || meta.len() > max_bytes as u64 {
        return None;
    }
    let mut out = Vec::with_capacity((meta.len() as usize).min(max_bytes));
    let mut limited = file.take((max_bytes as u64).saturating_add(1));
    limited.read_to_end(&mut out).ok()?;
    (out.len() <= max_bytes).then_some(out)
}

/// Open a regular file safely (`O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC` + `is_file()`).
pub(crate) fn open_regular_nonblock(path: &Path) -> io::Result<(std::fs::File, std::fs::Metadata)> {
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path)?;
    let meta = file.metadata()?;
    if !meta.file_type().is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"));
    }
    Ok((file, meta))
}

/// Open `root/name` as a verified real directory (`O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC`),
/// atomically rejecting symlink screen directories and invalid names (Finding 6).
pub(crate) fn open_verified_subdir(root: &Path, name: &str) -> io::Result<OwnedFd> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;
    if !valid_name(name) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("invalid screen name {name:?}")));
    }
    let root_c = CString::new(root.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in root path"))?;
    let name_c = CString::new(name).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in name"))?;
    // SAFETY: `root_c` is NUL-terminated; opens existing root directory.
    let rfd = unsafe { libc::open(root_c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if rfd < 0 {
        return Err(io::Error::last_os_error());
    }
    let root_fd = OwnedFd::new(rfd);
    // SAFETY: `root_fd` is a valid directory fd, `name_c` is a validated single component.
    let dfd = unsafe { libc::openat(root_fd.raw(), name_c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if dfd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(OwnedFd::new(dfd))
}

/// Open a regular file `file_name` directly inside a verified directory fd
/// (`openat` with `O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC` + `is_file()`).
pub(crate) fn open_regular_at(dir_fd: &OwnedFd, file_name: &str) -> io::Result<(std::fs::File, std::fs::Metadata)> {
    use std::ffi::CString;
    use std::os::fd::FromRawFd as _;
    if file_name.is_empty() || file_name.contains('/') || file_name.contains('\0') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid file name"));
    }
    let c = CString::new(file_name).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in file name"))?;
    // SAFETY: valid directory fd and NUL-terminated single filename component.
    let fd = unsafe { libc::openat(dir_fd.raw(), c.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is freshly opened and exclusively owned.
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let meta = file.metadata()?;
    if !meta.file_type().is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"));
    }
    Ok((file, meta))
}

/// Read a bounded regular file directly inside a verified directory fd.
pub(crate) fn read_bounded_regular_at(dir_fd: &OwnedFd, file_name: &str, max_bytes: usize) -> Option<Vec<u8>> {
    let (file, meta) = open_regular_at(dir_fd, file_name).ok()?;
    if meta.len() > max_bytes as u64 {
        return None;
    }
    let mut out = Vec::with_capacity((meta.len() as usize).min(max_bytes));
    let mut limited = file.take((max_bytes as u64).saturating_add(1));
    limited.read_to_end(&mut out).ok()?;
    (out.len() <= max_bytes).then_some(out)
}

/// True when `file_name` exists directly inside a verified directory fd (without following symlinks).
pub(crate) fn exists_at(dir_fd: &OwnedFd, file_name: &str) -> bool {
    use std::ffi::CString;
    let Ok(c) = CString::new(file_name) else { return false };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: valid dirfd, NUL-terminated component, valid stat out-pointer.
    unsafe { libc::fstatat(dir_fd.raw(), c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) == 0 }
}

/// True when `file_name` inside `dir_fd` is a regular file (NOT a directory,
/// symlink, dangling symlink, FIFO, or device — Item 8 / Finding 14).
pub(crate) fn is_regular_at(dir_fd: &OwnedFd, file_name: &str) -> bool {
    use std::ffi::CString;
    let Ok(c) = CString::new(file_name) else { return false };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: valid dirfd, NUL-terminated component, valid stat out-pointer.
    unsafe { libc::fstatat(dir_fd.raw(), c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFREG }
}

/// True when `dir_fd` carries a genuine regular-file screen marker (`screen.json`
/// or `actions.jsonl`), matching `screen_follow.rs` discovery (Item 8 / Finding 14).
fn has_screen_marker_at(dir_fd: &OwnedFd) -> bool {
    is_regular_at(dir_fd, "screen.json") || is_regular_at(dir_fd, "actions.jsonl")
}

/// Read a bounded regular file following symlinks (used for system `/etc/ld.so.conf.d/*.conf`)
/// without ever blocking on a FIFO (`O_NONBLOCK | O_CLOEXEC` + `is_file()`).
fn read_bounded_regular_follow(path: &Path, max_bytes: usize) -> Option<Vec<u8>> {
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.file_type().is_file() || meta.len() > max_bytes as u64 {
        return None;
    }
    let mut out = Vec::with_capacity((meta.len() as usize).min(max_bytes));
    let mut limited = file.take((max_bytes as u64).saturating_add(1));
    limited.read_to_end(&mut out).ok()?;
    (out.len() <= max_bytes).then_some(out)
}

/// True when `dir` is an existing real directory (not a symlink) owned by our
/// effective user ID. (Per-operation scratch directories additionally enforce
/// mode `0700` via [`TrustedTempDir`].)
pub(crate) fn verify_owned_dir(dir: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(dir) else { return false };
    // SAFETY: `geteuid()` has no preconditions.
    let uid = unsafe { libc::geteuid() };
    meta.file_type().is_dir() && meta.uid() == uid
}

/// Return true when `gid` is a standard Linux User Private Group (UPG) solely
/// belonging to `uid`: `gid == pw_gid`, `gr_name == pw_name`, and `gr_mem`
/// lists no other username (Finding 1 of Round 1 agy-review).
fn is_private_user_group(uid: libc::uid_t, gid: libc::gid_t) -> bool {
    use std::ffi::CStr;

    let mut pw_buf: Vec<libc::c_char> = vec![0; 1024];
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut pw_res: *mut libc::passwd = std::ptr::null_mut();
    loop {
        // SAFETY: `getpwuid_r` writes into our owned `pw` and `pw_buf`.
        let rc = unsafe { libc::getpwuid_r(uid, &mut pw, pw_buf.as_mut_ptr(), pw_buf.len(), &mut pw_res) };
        if rc == libc::ERANGE && pw_buf.len() < 64 * 1024 {
            pw_buf.resize(pw_buf.len() * 2, 0);
            continue;
        }
        if rc != 0 || pw_res.is_null() {
            return false;
        }
        break;
    }
    if pw.pw_gid != gid || pw.pw_name.is_null() {
        return false;
    }
    // SAFETY: `pw.pw_name` is a NUL-terminated string inside `pw_buf`.
    let pw_name = unsafe { CStr::from_ptr(pw.pw_name) }.to_bytes().to_vec();

    let mut gr_buf: Vec<libc::c_char> = vec![0; 1024];
    let mut gr: libc::group = unsafe { std::mem::zeroed() };
    let mut gr_res: *mut libc::group = std::ptr::null_mut();
    loop {
        // SAFETY: `getgrgid_r` writes into our owned `gr` and `gr_buf`.
        let rc = unsafe { libc::getgrgid_r(gid, &mut gr, gr_buf.as_mut_ptr(), gr_buf.len(), &mut gr_res) };
        if rc == libc::ERANGE && gr_buf.len() < 64 * 1024 {
            gr_buf.resize(gr_buf.len() * 2, 0);
            continue;
        }
        if rc != 0 || gr_res.is_null() {
            return false;
        }
        break;
    }
    if gr.gr_name.is_null() {
        return false;
    }
    // SAFETY: `gr.gr_name` is a NUL-terminated string inside `gr_buf`.
    let gr_name = unsafe { CStr::from_ptr(gr.gr_name) }.to_bytes();
    if gr_name != pw_name.as_slice() {
        return false;
    }
    if !gr.gr_mem.is_null() {
        let mut p = gr.gr_mem;
        loop {
            // SAFETY: `gr_mem` is a NULL-terminated array of NUL-terminated C strings inside `gr_buf`.
            let mem = unsafe { *p };
            if mem.is_null() {
                break;
            }
            let mem_bytes = unsafe { CStr::from_ptr(mem) }.to_bytes();
            if mem_bytes != pw_name.as_slice() {
                return false;
            }
            // SAFETY: advancing to next pointer in NULL-terminated `gr_mem`.
            p = unsafe { p.add(1) };
        }
    }
    true
}

/// True when a directory owned by `uid` has safe write permissions: NEVER
/// world-writable (`0o002`), and group-writable (`0o020`) ONLY when already
/// enclosed inside a `0700` `uid`-owned ancestor or owned by `uid`'s verified
/// single-user User Private Group (`umask 0002` UPG `0775`).
fn uid_owned_dir_mode_safe(uid: libc::uid_t, st_uid: libc::uid_t, st_gid: libc::gid_t, st_mode: libc::mode_t, inside_private_ancestor: bool) -> bool {
    st_uid == uid && (st_mode & 0o002) == 0 && ((st_mode & 0o020) == 0 || inside_private_ancestor || is_private_user_group(uid, st_gid))
}

/// Walk existing non-root ancestors of `dir` top-down, failing closed if any
/// ancestor is world-writable without sticky bit, owned by another non-root
/// user, or writable by a non-private shared group outside a `0700` `uid`-owned
/// ancestor. Returns `Ok(true)` when `dir` is already enclosed by a `0700`
/// `uid`-owned ancestor (such as `$XDG_RUNTIME_DIR`).
fn verify_ancestors_safe(dir: &Path, uid: libc::uid_t) -> io::Result<bool> {
    let mut ancestors = Vec::new();
    let mut anc = dir.parent();
    while let Some(p) = anc {
        if p.as_os_str().is_empty() || p == Path::new("/") {
            break;
        }
        ancestors.push(p);
        anc = p.parent();
    }
    ancestors.reverse();
    let mut inside_private_ancestor = false;
    for p in ancestors {
        if let Ok(m) = std::fs::metadata(p) {
            let mode = m.mode() as libc::mode_t;
            let auid = m.uid();
            let agid = m.gid();
            let safe_root = auid == 0 && ((mode & 0o022) == 0 || (mode & libc::S_ISVTX) != 0);
            let safe_self = uid_owned_dir_mode_safe(uid, auid, agid, mode, inside_private_ancestor);
            if !safe_root && !safe_self {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("unsafe ancestor directory {}", p.display())));
            }
            if auid == uid && (mode & 0o077) == 0 {
                inside_private_ancestor = true;
            }
        }
    }
    Ok(inside_private_ancestor)
}

/// Open or create `dir` as a real directory (`O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC`)
/// owned by `geteuid()`, verifying ancestors (accepting standard UPG `0775`
/// user-owned dirs while rejecting world-writable or shared-group-writable ones)
/// and tightening the leaf directory itself to `0700` via `fchmod`.
fn open_or_create_private_dir_fd(dir: &Path) -> io::Result<OwnedFd> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;
    // SAFETY: `geteuid()` has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let _inside_private = verify_ancestors_safe(dir, uid)?;
    if std::fs::symlink_metadata(dir).is_err() {
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true).mode(0o700);
        b.create(dir)?;
    }
    let c = CString::new(dir.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in directory path"))?;
    // SAFETY: NUL-terminated path opened with O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC.
    let raw = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = OwnedFd::new(raw);
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.raw(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if (st.st_mode & libc::S_IFMT) != libc::S_IFDIR || st.st_uid != uid {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("unsafe scratch directory {}", dir.display())));
    }
    // Item 8 / Finding 9 & 14: enforce private `mode & 0o077 == 0` on the opened fd.
    if (st.st_mode & 0o077) != 0 {
        if unsafe { libc::fchmod(fd.raw(), 0o700) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::fstat(fd.raw(), &mut st) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if (st.st_mode & 0o077) != 0 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("private directory {} remains accessible to others", dir.display())));
        }
    }
    Ok(fd)
}

/// Remove all entries inside `dir_fd` via `unlinkat` (never following symlinks
/// or traversing outside the pinned directory fd). Opens a fresh directory file
/// description via `openat(dir_fd, ".")` so filesystems like btrfs see entries
/// created after `dir_fd` was originally opened.
fn clear_dir_fd_contents(dir_fd: &OwnedFd) {
    use std::ffi::{CStr, CString};
    // SAFETY: `openat(dir_fd, ".")` opens a fresh fd on the same pinned inode.
    let fresh = unsafe { libc::openat(dir_fd.raw(), c".".as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if fresh < 0 {
        return;
    }
    // SAFETY: `fresh` is a valid directory fd now owned by `dp`.
    let dp = unsafe { libc::fdopendir(fresh) };
    if dp.is_null() {
        unsafe { libc::close(fresh) };
        return;
    }
    let mut names: Vec<CString> = Vec::new();
    loop {
        // SAFETY: `dp` is a valid open DIR*.
        let ent = unsafe { libc::readdir(dp) };
        if ent.is_null() {
            break;
        }
        // SAFETY: `d_name` is NUL-terminated.
        let name = unsafe { CStr::from_ptr((*ent).d_name.as_ptr()) };
        let bytes = name.to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        names.push(name.to_owned());
    }
    // SAFETY: closes `dp` and `fresh`.
    unsafe { libc::closedir(dp) };
    for name in &names {
        // Try unlinking as file/symlink first, then as empty subdir.
        // SAFETY: valid dirfd and NUL-terminated entry name.
        if unsafe { libc::unlinkat(dir_fd.raw(), name.as_ptr(), 0) } != 0 {
            unsafe { libc::unlinkat(dir_fd.raw(), name.as_ptr(), libc::AT_REMOVEDIR) };
        }
    }
}

/// Scan `base_fd` for stale operation directories matching `prefix` whose
/// `.owner.lock` is unlocked (meaning the owning helper crashed or was killed),
/// and reclaim them safely relative to `base_fd` (Finding 18).
fn reclaim_stale_temp_dirs(base_fd: &OwnedFd, prefix: &str) {
    use std::ffi::CStr;
    // SAFETY: fresh open file description on `base_fd` so btrfs sees all current entries.
    let fresh = unsafe { libc::openat(base_fd.raw(), c".".as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if fresh < 0 {
        return;
    }
    let dp = unsafe { libc::fdopendir(fresh) };
    if dp.is_null() {
        unsafe { libc::close(fresh) };
        return;
    }
    let prefix_bytes = prefix.as_bytes();
    loop {
        let ent = unsafe { libc::readdir(dp) };
        if ent.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*ent).d_name.as_ptr()) };
        let bytes = name.to_bytes();
        if !bytes.starts_with(prefix_bytes) {
            continue;
        }
        // SAFETY: open candidate child relative to `base_fd` without following symlinks.
        let cfd = unsafe { libc::openat(base_fd.raw(), name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if cfd < 0 {
            continue;
        }
        let child_fd = OwnedFd::new(cfd);
        let lock_c = std::ffi::CString::new(".owner.lock").unwrap();
        let lfd = unsafe { libc::openat(child_fd.raw(), lock_c.as_ptr(), libc::O_RDWR | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC) };
        if lfd < 0 {
            continue;
        }
        let lock_fd = OwnedFd::new(lfd);
        // If we obtain LOCK_EX | LOCK_NB, the previous helper is gone!
        if unsafe { libc::flock(lock_fd.raw(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            clear_dir_fd_contents(&child_fd);
            unsafe {
                libc::unlinkat(base_fd.raw(), name.as_ptr(), libc::AT_REMOVEDIR);
                libc::flock(lock_fd.raw(), libc::LOCK_UN);
            }
        }
    }
    unsafe { libc::closedir(dp) };
}

/// Hardened per-operation private `0700` directory pinned by parent & child
/// directory descriptors and guarded by an exclusive `.owner.lock` flock so
/// crashes are reclaimed cleanly and cleanup never traverses mutable paths
/// (Findings 14 & 18).
pub(crate) struct TrustedTempDir {
    base_fd: OwnedFd,
    child_name: std::ffi::CString,
    child_fd: OwnedFd,
    lock_fd: OwnedFd,
    path: PathBuf,
}

impl TrustedTempDir {
    /// Exclusively allocate a private `0700` directory under `base` (tightening
    /// `base` and verifying its ownership/mode via `fstat`/`fchmod`), reclaim
    /// any crashed siblings with prefix `reclaim_prefix`, and lock `.owner.lock`.
    pub(crate) fn create(base: &Path, label: &str, reclaim_prefix: &str) -> io::Result<Self> {
        use std::ffi::CString;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // SAFETY: `geteuid()` has no preconditions.
        let uid = unsafe { libc::geteuid() };
        let base_fd = open_or_create_private_dir_fd(base)?;
        reclaim_stale_temp_dirs(&base_fd, reclaim_prefix);

        let pid = std::process::id();
        for _ in 0..32 {
            let seq = NEXT.fetch_add(1, Ordering::Relaxed);
            let ns = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
            let name_str = format!("{label}-{pid}-{seq}-{ns:09}");
            let child_name = CString::new(name_str.as_str()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in temp dir label"))?;
            // SAFETY: create exclusively relative to verified `base_fd`.
            if unsafe { libc::mkdirat(base_fd.raw(), child_name.as_ptr(), 0o700) } != 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::AlreadyExists {
                    continue;
                }
                return Err(err);
            }
            let cfd = unsafe { libc::openat(base_fd.raw(), child_name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
            if cfd < 0 {
                let err = io::Error::last_os_error();
                unsafe { libc::unlinkat(base_fd.raw(), child_name.as_ptr(), libc::AT_REMOVEDIR) };
                return Err(err);
            }
            let child_fd = OwnedFd::new(cfd);
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            if unsafe { libc::fchmod(child_fd.raw(), 0o700) } != 0
                || unsafe { libc::fstat(child_fd.raw(), &mut st) } != 0
                || (st.st_mode & libc::S_IFMT) != libc::S_IFDIR
                || st.st_uid != uid
                || (st.st_mode & 0o777) != 0o700
            {
                unsafe { libc::unlinkat(base_fd.raw(), child_name.as_ptr(), libc::AT_REMOVEDIR) };
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "private temp dir failed mode/ownership check"));
            }
            let lock_c = CString::new(".owner.lock").unwrap();
            let lfd = unsafe { libc::openat(child_fd.raw(), lock_c.as_ptr(), libc::O_CREAT | libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o600 as libc::c_uint) };
            if lfd < 0 {
                let err = io::Error::last_os_error();
                unsafe { libc::unlinkat(base_fd.raw(), child_name.as_ptr(), libc::AT_REMOVEDIR) };
                return Err(err);
            }
            let lock_fd = OwnedFd::new(lfd);
            if unsafe { libc::flock(lock_fd.raw(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                clear_dir_fd_contents(&child_fd);
                unsafe { libc::unlinkat(base_fd.raw(), child_name.as_ptr(), libc::AT_REMOVEDIR) };
                return Err(io::Error::new(io::ErrorKind::WouldBlock, "couldn't lock temp dir owner file"));
            }
            return Ok(Self { base_fd, child_name, child_fd, lock_fd, path: base.join(name_str) });
        }
        Err(io::Error::new(io::ErrorKind::AlreadyExists, "couldn't allocate unique private temp directory"))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn dir_fd(&self) -> &OwnedFd {
        &self.child_fd
    }

    /// Free bytes available to this user on the scratch directory's filesystem.
    pub(crate) fn available_bytes(&self) -> Option<u64> {
        let mut vfs: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `fstatvfs` on our pinned directory fd.
        if unsafe { libc::fstatvfs(self.child_fd.raw(), &mut vfs) } == 0 { Some((vfs.f_bavail as u64).saturating_mul(vfs.f_frsize as u64)) } else { None }
    }
}

impl Drop for TrustedTempDir {
    fn drop(&mut self) {
        clear_dir_fd_contents(&self.child_fd);
        // SAFETY: remove exclusively relative to pinned `base_fd` before releasing lock.
        unsafe {
            libc::unlinkat(self.base_fd.raw(), self.child_name.as_ptr(), libc::AT_REMOVEDIR);
            libc::flock(self.lock_fd.raw(), libc::LOCK_UN);
        }
    }
}

/// Private per-uid base directory on disk (`/var/tmp/olo-screens-UID`) for
/// fallback scratch and large export files so they never share a public temp
/// tree or exhaust tmpfs runtime memory.
pub(crate) fn disk_scratch_base() -> PathBuf {
    // SAFETY: `geteuid()` has no preconditions.
    let uid = unsafe { libc::geteuid() };
    PathBuf::from(format!("/var/tmp/olo-screens-{uid}"))
}

/// Unified private fallback runtime directory when `XDG_RUNTIME_DIR` is unset
/// (Item 8 / Finding 9 & 14): prefers a real non-symlink `/run/user/<uid>`
/// owned by `<uid>` with private permissions `(mode & 0o077) == 0`, else private
/// `0700` `/var/tmp/olo-runtime-<uid>` (never `/tmp`), propagating errors when
/// the private directory cannot be verified or created.
fn fallback_runtime_dir_in(uid: libc::uid_t, std_run: &Path, var_tmp_dir: &Path) -> io::Result<PathBuf> {
    if std::fs::symlink_metadata(std_run).is_ok_and(|m| m.file_type().is_dir() && m.uid() == uid && (m.mode() & 0o077) == 0) {
        return Ok(std_run.to_path_buf());
    }
    let fallback = var_tmp_dir.join(format!("olo-runtime-{uid}"));
    let _fd = open_or_create_private_dir_fd(&fallback)?;
    Ok(fallback)
}

pub(crate) fn fallback_runtime_dir_checked() -> io::Result<PathBuf> {
    // SAFETY: `geteuid()` has no preconditions.
    let uid = unsafe { libc::geteuid() };
    fallback_runtime_dir_in(uid, Path::new(&format!("/run/user/{uid}")), Path::new("/var/tmp"))
}

/// Single fallible runtime-base resolver shared across `follow`, `stream`,
/// owner recording, and guarded lifecycle mutations (Item 8 / Findings 9 & 14).
pub(crate) fn runtime_dir_checked() -> io::Result<PathBuf> {
    if let Some(v) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(v));
    }
    fallback_runtime_dir_checked()
}

/// Where running screens keep their runtime descriptors (`NAME.json`,
/// `NAME.control`, `NAME.owner.json`), matching `apps/screen/src/screens.rs::dir`.
fn screens_runtime_dir_checked() -> io::Result<PathBuf> {
    let rt = runtime_dir_checked()?;
    let new = rt.join("olo-screens");
    if std::fs::symlink_metadata(&new).is_ok() {
        return Ok(new);
    }
    let old = rt.join("tabula-screens");
    if std::fs::symlink_metadata(&old).is_ok() {
        return Ok(old);
    }
    Ok(new)
}

/// Both runtime directory names `follow`, `stream`, and guarded mutations inspect
/// from startup so an older host creating `tabula-screens` is still noticed,
/// failing closed if the fallback runtime directory is unsafe (Item 8).
pub(crate) fn screens_runtime_dir_candidates_checked() -> io::Result<Vec<PathBuf>> {
    let rt = runtime_dir_checked()?;
    Ok(vec![rt.join("olo-screens"), rt.join("tabula-screens")])
}

/// Non-blocking `AF_UNIX` connect bounded by absolute `deadline` and interruptible
/// via `stop_rd`, handling both `EINPROGRESS` and Linux's full-backlog `EAGAIN`
/// without ever blocking indefinitely in `unix_wait_for_peer` (Item 6 / Finding 25).
pub(crate) fn connect_unix_nonblocking_deadline(path: &Path, deadline: Instant, stop_rd: Option<i32>) -> io::Result<std::os::unix::net::UnixStream> {
    use std::os::fd::FromRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;
    let path_bytes = path.as_os_str().as_bytes();
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if path_bytes.is_empty() || path_bytes.len() >= addr.sun_path.len() || path_bytes.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid unix socket path"));
    }
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    // SAFETY: bounds checked above (`path_bytes.len() < addr.sun_path.len()`), remaining bytes are NUL.
    unsafe {
        std::ptr::copy_nonoverlapping(path_bytes.as_ptr(), addr.sun_path.as_mut_ptr().cast(), path_bytes.len());
    }
    let addrlen = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + path_bytes.len() + 1) as libc::socklen_t;

    let connected_fd = loop {
        if let Some(sfd) = stop_rd {
            let mut p = libc::pollfd { fd: sfd, events: libc::POLLIN, revents: 0 };
            // SAFETY: non-blocking poll on valid stop pipe descriptor.
            if unsafe { libc::poll(&mut p, 1, 0) } > 0 && p.revents != 0 {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "stream stopped during connect"));
            }
        }
        // SAFETY: standard AF_UNIX non-blocking close-on-exec socket.
        let raw_fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC, 0) };
        if raw_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let sock = OwnedFd::new(raw_fd);
        // SAFETY: `sock` is valid and `addr`/`addrlen` describe a valid `sockaddr_un`.
        let r = unsafe { libc::connect(sock.raw(), (&addr as *const libc::sockaddr_un).cast(), addrlen) };
        if r == 0 {
            break sock;
        }
        let err = io::Error::last_os_error();
        let errno = err.raw_os_error().unwrap_or(0);
        if errno == libc::EINPROGRESS {
            loop {
                let rem = deadline.saturating_duration_since(Instant::now());
                if rem.is_zero() {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "unix socket connect timed out"));
                }
                let mut pfds = [libc::pollfd { fd: sock.raw(), events: libc::POLLOUT, revents: 0 }, libc::pollfd { fd: stop_rd.unwrap_or(-1), events: libc::POLLIN, revents: 0 }];
                let nfds: libc::nfds_t = if stop_rd.is_some() { 2 } else { 1 };
                // SAFETY: `pfds[..nfds]` holds valid open fds.
                let pr = unsafe { libc::poll(pfds.as_mut_ptr(), nfds, timeout_ms_ceil(rem)) };
                if pr < 0 {
                    if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(io::Error::last_os_error());
                }
                if pr == 0 {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "unix socket connect timed out"));
                }
                if stop_rd.is_some() && pfds[1].revents != 0 {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "stream stopped during connect"));
                }
                if pfds[0].revents != 0 {
                    let mut so_err: libc::c_int = 0;
                    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
                    // SAFETY: querying `SO_ERROR` on our valid socket.
                    if unsafe { libc::getsockopt(sock.raw(), libc::SOL_SOCKET, libc::SO_ERROR, (&raw mut so_err).cast(), &mut len) } != 0 {
                        return Err(io::Error::last_os_error());
                    }
                    if so_err == 0 {
                        break;
                    }
                    return Err(io::Error::from_raw_os_error(so_err));
                }
            }
            break sock;
        }
        if errno == libc::EAGAIN || errno == libc::EWOULDBLOCK || errno == libc::EINTR {
            let rem = deadline.saturating_duration_since(Instant::now());
            if rem.is_zero() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "unix socket connect timed out (peer backlog full)"));
            }
            let step = rem.min(Duration::from_millis(10));
            if let Some(sfd) = stop_rd {
                let mut p = libc::pollfd { fd: sfd, events: libc::POLLIN, revents: 0 };
                // SAFETY: waiting on stop pipe with bounded timeout.
                let pr = unsafe { libc::poll(&mut p, 1, timeout_ms_ceil(step)) };
                if pr > 0 && p.revents != 0 {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "stream stopped during connect"));
                }
            } else {
                std::thread::sleep(step);
            }
            continue;
        }
        return Err(err);
    };

    // Switch back to blocking mode with remaining deadline as read/write timeout.
    // SAFETY: valid connected socket fd.
    unsafe {
        let flags = libc::fcntl(connected_fd.raw(), libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(connected_fd.raw(), libc::F_SETFL, flags & !libc::O_NONBLOCK);
        }
    }
    // SAFETY: transferring exclusive ownership of connected AF_UNIX fd to `UnixStream`.
    let stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(connected_fd.into_raw()) };
    let io_timeout = deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(50));
    stream.set_read_timeout(Some(io_timeout))?;
    stream.set_write_timeout(Some(io_timeout))?;
    Ok(stream)
}

pub(crate) fn connect_unix_nonblocking(path: &Path, timeout: Duration, stop_rd: Option<i32>) -> io::Result<std::os::unix::net::UnixStream> {
    connect_unix_nonblocking_deadline(path, Instant::now() + timeout, stop_rd)
}

/// Write `data` to `stream` bounded strictly by a single absolute `deadline` (Item 6).
fn write_all_unix_deadline(stream: &std::os::unix::net::UnixStream, data: &[u8], deadline: Instant) -> io::Result<()> {
    let fd = stream.as_raw_fd();
    let mut written = 0usize;
    while written < data.len() {
        let rem = deadline.saturating_duration_since(Instant::now());
        if rem.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "unix socket write timed out"));
        }
        let mut pfd = libc::pollfd { fd, events: libc::POLLOUT, revents: 0 };
        // SAFETY: polling our valid socket fd.
        let pr = unsafe { libc::poll(&mut pfd, 1, timeout_ms_ceil(rem)) };
        if pr < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(io::Error::last_os_error());
        }
        if pr == 0 {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "unix socket write timed out"));
        }
        // SAFETY: nonblocking `send(MSG_DONTWAIT | MSG_NOSIGNAL)` on valid socket fd.
        let n = unsafe { libc::send(fd, data[written..].as_ptr().cast(), data.len() - written, libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL) };
        if n > 0 {
            written += n as usize;
        } else if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "unix socket write returned 0"));
        } else {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock || err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
    }
    Ok(())
}

/// Read one `\n`-terminated line from `stream` capped at BOTH `max_bytes` AND
/// one absolute `deadline` (Item 6: an unterminated or trickle reply can never
/// grow memory or overrun the deadline).
fn read_line_unix_deadline(stream: &std::os::unix::net::UnixStream, max_bytes: usize, deadline: Instant) -> io::Result<String> {
    let fd = stream.as_raw_fd();
    let mut raw: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let rem = deadline.saturating_duration_since(Instant::now());
        if rem.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "unix socket read timed out"));
        }
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: polling our valid socket fd.
        let pr = unsafe { libc::poll(&mut pfd, 1, timeout_ms_ceil(rem)) };
        if pr < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(io::Error::last_os_error());
        }
        if pr == 0 {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "unix socket read timed out"));
        }
        // Read in small chunks via MSG_PEEK first so we never consume past `\n` when reading multiple lines!
        let mut peek = [0u8; 1024];
        let cap = peek.len().min(max_bytes.saturating_sub(raw.len()).saturating_add(1));
        // SAFETY: nonblocking `recv(MSG_PEEK | MSG_DONTWAIT)` on valid socket fd.
        let pn = unsafe { libc::recv(fd, peek.as_mut_ptr().cast(), cap, libc::MSG_PEEK | libc::MSG_DONTWAIT) };
        if pn < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock || err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if pn == 0 {
            if raw.is_empty() {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "unix socket closed"));
            }
            break;
        }
        let pn = pn as usize;
        let (take, done) = match memchr::memchr(b'\n', &peek[..pn]) {
            Some(pos) => (pos + 1, true),
            None => (pn, false),
        };
        if raw.len() + take > max_bytes {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("unix reply exceeded {max_bytes} bytes")));
        }
        // Consume exact `take` bytes.
        let mut got = 0usize;
        while got < take {
            let tmp = if take - got == 1 { &mut byte[..] } else { &mut peek[got..take] };
            // SAFETY: nonblocking `recv` consuming already-peeked bytes.
            let n = unsafe { libc::recv(fd, tmp.as_mut_ptr().cast(), tmp.len(), libc::MSG_DONTWAIT) };
            if n <= 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short recv after peek"));
            }
            raw.extend_from_slice(&tmp[..n as usize]);
            got += n as usize;
        }
        if done {
            break;
        }
    }
    String::from_utf8(raw).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Where screens keep persistent state (`$XDG_STATE_HOME/olo/screens`),
/// matching `olo_xdg::state_dir().join("screens")`.
pub(crate) fn screens_state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(".local/state")).join("olo/screens")
}

/// Bounded read of a runtime screen descriptor (`NAME.json`, max 64 KiB).
pub(crate) fn read_runtime_screen(path: &Path) -> Option<RuntimeScreen> {
    let bytes = read_bounded_regular(path, 64 * 1024)?;
    serde_json::from_slice(&bytes).ok()
}

/// Whether `/proc/{pid}/cmdline` identifies an `olo-session` or `tabula-session`
/// executable (matching `crates/screen-pane/src/screens.rs::is_session_of`).
pub(crate) fn is_session_cmdline(cmdline: &[u8]) -> bool {
    let first = cmdline.split(|b| *b == 0).next().unwrap_or_default();
    let base = first.rsplit(|b| *b == b'/').next().unwrap_or(first);
    base == b"olo-session" || base == b"tabula-session"
}

/// Whether process `pid` is alive (not zombie), runs `olo-session` /
/// `tabula-session`, and carries `OLO_SCREEN={name}` in its environment.
pub(crate) fn screen_pid_running(name: &str, pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { return false };
    let Some(state) = stat.rsplit_once(')').and_then(|(_, rest)| rest.trim_start().as_bytes().first().copied()) else {
        return false;
    };
    if matches!(state, b'Z' | b'X' | b'x') {
        return false;
    }
    let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) else { return false };
    if !is_session_cmdline(&cmdline) {
        return false;
    }
    let Ok(environ) = std::fs::read(format!("/proc/{pid}/environ")) else { return false };
    let wanted = format!("OLO_SCREEN={name}");
    environ.split(|b| *b == 0).any(|v| v == wanted.as_bytes())
}

/// Raw `pidfd_open(pid, 0)` returning the OS error so callers can distinguish
/// `ESRCH` (already exited) from invalid PIDs (`EINVAL`, Item 1) and
/// resource/kernel failures (`EMFILE`, `ENOSYS`).
pub(crate) fn pidfd_open_result(pid: i32) -> io::Result<OwnedFd> {
    if pid <= 0 {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    // SAFETY: Linux `pidfd_open(pid, 0)` returns a new fd or -1.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0 as libc::c_uint) as i32 };
    if fd >= 0 { Ok(OwnedFd::new(fd)) } else { Err(io::Error::last_os_error()) }
}

/// Open a `pidfd` for `pid`, or `None` if it has already exited.
pub(crate) fn pidfd_open(pid: i32) -> Option<OwnedFd> {
    pidfd_open_result(pid).ok()
}

/// True when `pidfd` has become readable (meaning the pinned process exited).
pub(crate) fn pidfd_exited(fd: &OwnedFd) -> bool {
    let mut pfd = libc::pollfd { fd: fd.raw(), events: libc::POLLIN, revents: 0 };
    // SAFETY: valid owned pidfd polled with timeout 0.
    let r = unsafe { libc::poll(&mut pfd, 1, 0) };
    r > 0 && pfd.revents != 0
}

pub(crate) enum PinResult {
    Alive(OwnedFd),
    Exited,
    Failed(io::Error),
}

/// Pin `pid` via `pidfd_open` BEFORE validating `check(pid)`, then verify the
/// pinned process is still alive after validation (eliminating exit/PID-reuse
/// TOCTOU races).
pub(crate) fn pin_live_pid(pid: i32, check: impl FnOnce(i32) -> bool) -> PinResult {
    match pidfd_open_result(pid) {
        Ok(fd) => {
            if check(pid) && !pidfd_exited(&fd) {
                PinResult::Alive(fd)
            } else {
                PinResult::Exited
            }
        }
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => PinResult::Exited,
        Err(e) => PinResult::Failed(e),
    }
}

/// Find a currently running screen `name`, its runtime directory, and a pinned
/// `pidfd` for its original session process within `rt_dirs`.
pub(crate) fn find_running_screen_pinned_in(rt_dirs: &[PathBuf], name: &str) -> Option<(PathBuf, RuntimeScreen, OwnedFd)> {
    for rt in rt_dirs {
        let path = rt.join(format!("{name}.json"));
        if let Some(screen) = read_runtime_screen(&path)
            && screen.name == name
            && screen.pid > 0
            && let PinResult::Alive(pidfd) = pin_live_pid(screen.pid, |p| screen_pid_running(name, p))
        {
            return Some((rt.clone(), screen, pidfd));
        }
    }
    None
}

/// Find a currently running screen `name` using the single fallible runtime
/// resolver (Item 8: propagates unsafe fallback errors instead of silently
/// falling back to a symlink path).
pub(crate) fn find_running_screen_pinned_checked(name: &str) -> io::Result<Option<(PathBuf, RuntimeScreen, OwnedFd)>> {
    let candidates = screens_runtime_dir_candidates_checked()?;
    Ok(find_running_screen_pinned_in(&candidates, name))
}

/// Whether `exe` (resolved from `/proc/PID/exe`) is an Olo viewer or Rex
/// helper permitted to hold a screen's `.control` lock (matches
/// `crates/screen-pane/src/screens.rs::is_controller_exe`, excluding unrelated
/// binaries such as `olo-screenshot`).
pub(crate) fn is_controller_exe(exe: &Path) -> bool {
    let Some(raw) = exe.file_name().and_then(|n| n.to_str()) else { return false };
    let clean = raw.trim_end_matches(" (deleted)");
    clean == "olo-screen" || clean == "tabula-screen" || clean == "olo-rex" || clean.starts_with("olo-rex-link")
}

/// Parse `"<pid>"` or `"<pid> <start_ticks>"` from a `.control` or
/// `.control.lock` sidecar (matching `crates/screen-pane/src/screens.rs`).
pub(crate) fn parse_control_lock_meta(bytes: &[u8]) -> Option<(i32, Option<u64>)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut parts = text.split_whitespace();
    let pid = parts.next()?.parse::<i32>().ok().filter(|&p| p > 0)?;
    let ticks = parts.next().and_then(|s| s.parse::<u64>().ok());
    Some((pid, ticks))
}

/// Process start ticks since boot (`/proc/{pid}/stat` field 22), matching
/// `crates/screen-pane/src/screens.rs::proc_start_ticks`.
pub(crate) fn proc_start_ticks(pid: i32) -> Option<u64> {
    parent_and_start(pid).map(|(_, ticks)| ticks)
}

/// Open the stable per-screen `NAME.control.lock` file (`O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC`,
/// mode `0600`), verifying regular-file type and ownership by `geteuid()`.
pub(crate) fn open_stable_control_lock(path: &Path) -> io::Result<std::fs::File> {
    // SAFETY: `geteuid()` has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC).open(path)?;
    let meta = file.metadata()?;
    if !meta.file_type().is_file() || meta.uid() != uid {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("unsafe control lock {}", path.display())));
    }
    Ok(file)
}

/// Whether `pid` is a live (non-zombie) controller process, optionally verified
/// against its expected `/proc/{pid}/stat` start ticks to reject PID reuse (Finding 8).
pub(crate) fn is_controller_pid_alive_with_ticks(pid: i32, expected_ticks: Option<u64>) -> bool {
    if pid <= 0 {
        return false;
    }
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { return false };
    let Some((_, rest)) = stat.rsplit_once(')') else { return false };
    let mut fields = rest.split_whitespace();
    let Some(state) = fields.next().and_then(|s| s.as_bytes().first().copied()) else {
        return false;
    };
    if matches!(state, b'Z' | b'X' | b'x') {
        return false;
    }
    if let Some(want) = expected_ticks {
        let Some(actual) = fields.nth(18).and_then(|s| s.parse::<u64>().ok()) else { return false };
        if actual != want {
            return false;
        }
    }
    std::fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|exe| is_controller_exe(&exe))
}

/// Read the active controller `(pid, expected_ticks)` from `NAME.control` and
/// optional `NAME.control.lock` sidecar.
pub(crate) fn read_control_meta(rt: &Path, name: &str) -> Option<(i32, Option<u64>)> {
    let bytes = read_bounded_regular(&rt.join(format!("{name}.control")), 64)?;
    let (pid, inline_ticks) = parse_control_lock_meta(&bytes)?;
    let expected_ticks =
        inline_ticks.or_else(|| read_bounded_regular(&rt.join(format!("{name}.control.lock")), 64).and_then(|b| parse_control_lock_meta(&b)).filter(|(p, _)| *p == pid).and_then(|(_, t)| t));
    Some((pid, expected_ticks))
}

/// If `NAME.control` is stale (dead or PID-reused) and unlocked, clean it up
/// safely under `NAME.control.lock` + `NAME.control` `LOCK_EX` (Finding 8).
pub(crate) fn try_cleanup_stale_control(rt: &Path, name: &str) {
    let lock_path = rt.join(format!("{name}.control.lock"));
    let ctl_path = rt.join(format!("{name}.control"));
    let Ok(stable) = open_stable_control_lock(&lock_path) else { return };
    // SAFETY: valid owned fd.
    if unsafe { libc::flock(stable.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return;
    }
    if let Ok(file) = std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC).open(&ctl_path)
        && let Ok(fm) = file.metadata()
        && fm.file_type().is_file()
        && unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0
    {
        // Re-check liveness while holding BOTH exclusive locks.
        let still_live = read_control_meta(rt, name).is_some_and(|(pid, ticks)| is_controller_pid_alive_with_ticks(pid, ticks));
        if !still_live && std::fs::symlink_metadata(&ctl_path).is_ok_and(|pm| pm.file_type().is_file() && pm.dev() == fm.dev() && pm.ino() == fm.ino()) {
            let _ = std::fs::remove_file(&ctl_path);
            let _ = stable.set_len(0);
        }
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    }
    unsafe { libc::flock(stable.as_raw_fd(), libc::LOCK_UN) };
}

/// Scan `file` ranges for every needle in `needles` without loading whole large binaries.
fn file_contains_all_in_ranges(file: &std::fs::File, ranges: &[(u64, u64)], needles: &[&[u8]]) -> bool {
    let overlap = needles.iter().map(|n| n.len()).max().unwrap_or(0);
    let mut found = vec![false; needles.len()];
    let mut buf = vec![0u8; 64 * 1024 + overlap];
    for &(start, len) in ranges {
        let mut pos = 0u64;
        let mut carry = 0usize;
        while pos < len {
            let chunk = ((len - pos) as usize).min(64 * 1024);
            let Ok(got) = file.read_at(&mut buf[carry..carry + chunk], start + pos) else { break };
            if got == 0 {
                break;
            }
            let hay = &buf[..carry + got];
            for (i, needle) in needles.iter().enumerate() {
                if !found[i] && hay.windows(needle.len()).any(|w| w == *needle) {
                    found[i] = true;
                }
            }
            if found.iter().all(|&f| f) {
                return true;
            }
            let keep = (carry + got).min(overlap);
            buf.copy_within(carry + got - keep..carry + got, 0);
            carry = keep;
            pos += got as u64;
        }
    }
    found.iter().all(|&f| f)
}

/// True ONLY in unit tests (`cfg!(test)`) or when an integration test explicitly
/// opts in via `OLO_ALLOW_TEST_SCREEN_CAPS=1`. Never enabled in production so
/// non-ELF scripts cannot spoof capabilities and absent `bin/olo-screen` fails
/// closed (Item 10 / Findings 6 & 26).
fn allow_test_screen_caps() -> bool {
    cfg!(test) || std::env::var_os("OLO_ALLOW_TEST_SCREEN_CAPS").is_some_and(|v| v == "1")
}

/// Inspect `program` (executable ELF64 `.rodata`/`.data` sections, or an explicit
/// `OLO_TEST_SCREEN_CAPS=` declaration in a non-ELF test stand-in script ONLY
/// when `allow_test_screen_caps()` is enabled) for required markers without
/// spawning the binary. Requires an executable regular file (`(mode & 0o111) != 0`)
/// and ignores `#` comments (Item 10 / Findings 6 & 26).
fn program_contains_markers(program: &Path, elf_needles: &[&[u8]], script_cap: &str, allow_script_code_needle: Option<&[u8]>) -> io::Result<bool> {
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open(program)?;
    let meta = file.metadata()?;
    if !meta.file_type().is_file() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "host olo-screen binary is not a regular file"));
    }
    if (meta.mode() & 0o111) == 0 {
        return Ok(false);
    }
    let file_len = meta.len();
    let mut head = [0u8; 64];
    let is_elf64 = file.read_exact_at(&mut head, 0).is_ok() && head[..4] == *b"\x7fELF" && head[4] == 2 && head[5] == 1;
    if is_elf64 {
        let shoff = u64_of(&head, 0x28);
        let shentsize = u16_of(&head, 0x3a);
        let shnum = u16_of(&head, 0x3c);
        let mut ranges = Vec::new();
        if shentsize >= 64 && shnum > 0 && shnum <= 4096 {
            for i in 0..shnum {
                let mut sh = [0u8; 64];
                if file.read_exact_at(&mut sh, shoff.saturating_add(i * shentsize)).is_err() {
                    break;
                }
                let sh_type = u32_of(&sh, 4);
                let sh_flags = u64_of(&sh, 8);
                let sh_offset = u64_of(&sh, 24);
                let sh_size = u64_of(&sh, 32);
                // SHT_PROGBITS (1), SHF_ALLOC (2), NOT SHF_EXECINSTR (4) -> .rodata / .data.
                if sh_type == 1 && (sh_flags & 0x2) != 0 && (sh_flags & 0x4) == 0 && sh_size > 0 && sh_offset < file_len {
                    ranges.push((sh_offset, sh_size.min(file_len - sh_offset)));
                }
            }
        }
        if ranges.is_empty() {
            ranges.push((0, file_len.min(64 * 1024 * 1024)));
        }
        return Ok(file_contains_all_in_ranges(&file, &ranges, elf_needles));
    }
    // Non-ELF script stand-ins are honored ONLY in tests (`allow_test_screen_caps()`).
    if !allow_test_screen_caps() {
        return Ok(false);
    }
    let cap = file_len.min(64 * 1024) as usize;
    let mut buf = vec![0u8; cap];
    let got = file.read_at(&mut buf, 0)?;
    let text = String::from_utf8_lossy(&buf[..got]);
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("OLO_TEST_SCREEN_CAPS=") {
            let unquoted = rest.trim_matches(|c| c == '"' || c == '\'');
            if unquoted.split(',').any(|tok| tok.trim() == script_cap) {
                return Ok(true);
            }
        }
        if let Some(needle) = allow_script_code_needle
            && trimmed.as_bytes().windows(needle.len()).any(|w| w == needle)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Probe installed `prefix()/bin/olo-screen` for a capability. Returns `Ok(false)`
/// if `bin/olo-screen` is absent (`ENOENT`) or non-executable, `Err(...)` if
/// unreadable/unsafe, and `Ok(true)` ONLY for an executable installed binary
/// (or test stand-in under `allow_test_screen_caps()`) with positive markers
/// (Item 10 / Findings 6 & 26).
fn probe_installed_cli_cap(elf_needles: &[&[u8]], script_cap: &str, allow_script_code_needle: Option<&[u8]>) -> Result<bool, String> {
    let program = prefix().join("bin/olo-screen");
    match program_contains_markers(&program, elf_needles, script_cap, allow_script_code_needle) {
        Ok(yes) => Ok(yes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("couldn't inspect host olo-screen: {e}")),
    }
}

/// Strict installed-CLI check for `screen --protocol`'s `control-lock` capability
/// (Finding 26: never returns `true` on empty or unreadable prefix).
pub(crate) fn host_cli_honors_helper_control_installed() -> Result<bool, String> {
    probe_installed_cli_cap(&[b"olo-rex-link", b".control.lock"], "control-lock", None)
}

/// Strict installed-CLI check for `NAME.lifecycle.lock` participation (Finding 6:
/// gates advertising AND execution of guarded `stop`/`delete`/`history --clear`).
pub(crate) fn host_cli_honors_lifecycle_lock() -> Result<bool, String> {
    probe_installed_cli_cap(&[b".lifecycle.lock"], "lifecycle-lock", None)
}

/// Verify whether the host's installed `bin/olo-screen` honors `olo-rex-link`
/// helper PIDs and `NAME.control.lock` leases (used inside `screen stream`).
/// Fails closed if `bin/olo-screen` is absent in production; allows an absent
/// binary ONLY in isolated test rigs opting in via `OLO_ALLOW_TEST_SCREEN_CAPS=1`
/// (Item 10 / Findings 6 & 26).
pub(crate) fn host_cli_honors_helper_control() -> Result<(), String> {
    let program = prefix().join("bin/olo-screen");
    match program_contains_markers(&program, &[b"olo-rex-link", b".control.lock"], "control-lock", None) {
        Ok(true) => Ok(()),
        Ok(false) => Err("This host's installed olo-screen is too old to honor remote Take Control; run `olo screen install` to upgrade.".into()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if allow_test_screen_caps() {
                Ok(())
            } else {
                Err("Olo isn't installed on this host yet; run `olo screen install` before taking control.".into())
            }
        }
        Err(e) => Err(format!("couldn't inspect host olo-screen: {e}")),
    }
}

/// True when the installed host `olo-screen` binary/script supports `--part`
/// for discontiguous history video exports (Finding 44).
pub(crate) fn host_cli_supports_export_part(program: &Path) -> bool {
    program_contains_markers(program, &[b"--part"], "discontiguous_part", Some(b"--part")).unwrap_or(false)
}

fn read_owner_file(path: &Path) -> Option<ScreenOwner> {
    let bytes = read_bounded_regular(path, 64 * 1024)?;
    let owner = serde_json::from_slice::<ScreenOwner>(&bytes).ok()?;
    (!owner.block_id.is_empty()).then_some(owner)
}

/// True when `owner` is unbound (legacy record) or matches the running screen
/// session's `(pid, started, start_ticks)` identity (Finding 54).
pub(crate) fn owner_matches_running(owner: &ScreenOwner, s: &RuntimeScreen) -> bool {
    if owner.screen_pid.is_some_and(|p| p != s.pid) {
        return false;
    }
    if let Some(st) = owner.screen_started
        && s.started != Some(st)
    {
        return false;
    }
    if let Some(expected_ticks) = owner.screen_start_ticks
        && proc_start_ticks(s.pid).is_some_and(|actual| actual != expected_ticks)
    {
        return false;
    }
    true
}

/// Read `NAME.owner.json` for a RUNNING screen `s`, discarding stale records
/// bound to a different session generation (Finding 54).
pub(crate) fn read_owner_for_running(rt: &Path, state_root: &Path, name: &str, s: &RuntimeScreen) -> Option<ScreenOwner> {
    for path in [rt.join(format!("{name}.owner.json")), state_root.join(name).join("owner.json")] {
        if let Some(owner) = read_owner_file(&path)
            && owner_matches_running(&owner, s)
        {
            return Some(owner);
        }
    }
    None
}

/// Read `NAME.owner.json` from the runtime directory, falling back to the
/// persistent copy in `$XDG_STATE_HOME/olo/screens/NAME/owner.json`.
pub(crate) fn read_owner(rt: &Path, state_root: &Path, name: &str) -> Option<ScreenOwner> {
    for path in [rt.join(format!("{name}.owner.json")), state_root.join(name).join("owner.json")] {
        if let Some(owner) = read_owner_file(&path) {
            return Some(owner);
        }
    }
    None
}

/// Remove any runtime `NAME.owner.json` for `name` across runtime dirs ONLY
/// when `name` is not currently running (verified under a nonblocking
/// `NAME.lifecycle.lock`, Finding 10: a replacement session started right after
/// an unguarded stop/delete can never lose its new `owner.json`).
fn unlink_stale_runtime_owner(name: &str) {
    if !valid_name(name) {
        return;
    }
    let Ok(candidates) = screens_runtime_dir_candidates_checked() else {
        return;
    };
    let Ok(locked) = acquire_runtime_locks(&candidates, name, Duration::from_millis(50), false) else {
        return;
    };
    if matches!(probe_running_in_locked_rts(&locked, name), RunningProbe::Stopped) {
        for rt in &locked {
            unlink_at_ignore_enoent(&rt.dir_fd, &format!("{name}.owner.json"));
        }
    }
}

/// Unconditionally remove `NAME.owner.json` (used ONLY when `start` runs
/// outside Rex and immediately replaces any prior session's runtime owner).
fn unlink_runtime_owner(name: &str) {
    if !valid_name(name) {
        return;
    }
    let Ok(candidates) = screens_runtime_dir_candidates_checked() else {
        return;
    };
    for rt in candidates {
        let _ = std::fs::remove_file(rt.join(format!("{name}.owner.json")));
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn write_atomic(path: &Path, bytes: &[u8]) {
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("owner.json");
    let tmp = parent.join(format!(".{fname}.{}.tmp", std::process::id()));
    if std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(&tmp).and_then(|mut f| f.write_all(bytes)).is_ok() {
        if std::fs::rename(&tmp, path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Record which Rex pane controls screen `name` (`NAME.owner.json` beside
/// `NAME.json` and `owner.json` in the screen's state folder), serialized per
/// screen with `flock`, writing ONLY when the pane changed or either copy is
/// missing or diverged (review1 change 8 + Astra finding 27).
fn record_owner(name: &str, is_start: bool) {
    if !valid_name(name) {
        return;
    }
    let Some(block_id) = std::env::var("REX_BLOCK").ok().filter(|b| !b.is_empty()) else { return };
    let session_id = std::env::var("REX_SESSION").unwrap_or_default();
    let Ok(rt) = screens_runtime_dir_checked() else { return };
    let state_root = screens_state_dir();
    let state_dir = state_root.join(name);
    let runtime_screen_exists = rt.join(format!("{name}.json")).is_file();
    let state_exists = state_dir.is_dir();
    if !is_start && !runtime_screen_exists && !state_exists {
        return;
    }

    // Serialize per-screen owner updates so concurrent actions cannot leave
    // runtime and persistent copies disagreeing.
    let lock_path = if is_start || state_exists {
        let _ = std::fs::create_dir_all(&state_dir);
        state_dir.join(".owner.lock")
    } else {
        let _ = std::fs::create_dir_all(&rt);
        rt.join(format!(".{name}.owner.lock"))
    };
    let _owner_lock = std::fs::OpenOptions::new().create(true).write(true).truncate(false).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(&lock_path).ok().inspect(|f| {
        // SAFETY: blocking flock on exclusively opened lock file.
        unsafe {
            libc::flock(f.as_raw_fd(), libc::LOCK_EX);
        }
    });

    let runtime_owner = rt.join(format!("{name}.owner.json"));
    let state_owner = state_dir.join("owner.json");
    let rt_existing = read_owner_file(&runtime_owner);
    let st_existing = read_owner_file(&state_owner);
    let existing = rt_existing.as_ref().or(st_existing.as_ref());

    // Finding 54: bind owner record to the active session's `(pid, started, start_ticks)`.
    let rt_screen = read_runtime_screen(&rt.join(format!("{name}.json"))).filter(|s| s.name == name);
    let screen_pid = rt_screen.as_ref().map(|s| s.pid);
    let screen_started = rt_screen.as_ref().and_then(|s| s.started);
    let screen_start_ticks = screen_pid.and_then(proc_start_ticks);

    let started_by = if is_start { Some(OwnerPane { host_key: None, session_id: session_id.clone(), block_id: block_id.clone() }) } else { existing.and_then(|o| o.started_by.clone()) };

    let need_runtime = is_start || runtime_screen_exists;
    let need_state = is_start || state_exists;
    let matches_target = |o: &ScreenOwner| {
        o.is_same_pane(&session_id, &block_id)
            && (!is_start || o.started_by == started_by)
            && (screen_pid.is_none() || o.screen_pid == screen_pid)
            && (screen_started.is_none() || o.screen_started == screen_started)
    };
    let rt_ok = !need_runtime || rt_existing.as_ref().is_some_and(matches_target);
    let st_ok = !need_state || st_existing.as_ref().is_some_and(matches_target);
    let copies_agree = !need_runtime || !need_state || rt_existing == st_existing;
    if rt_ok && st_ok && copies_agree {
        return;
    }

    // If one authoritative copy already matches this pane and the other merely
    // diverged or went missing, repair the missing/stale copy from it.
    if let Some(auth) = rt_existing.as_ref().filter(|o| matches_target(o)).or_else(|| st_existing.as_ref().filter(|o| matches_target(o)))
        && let Ok(bytes) = serde_json::to_vec_pretty(auth)
    {
        if need_runtime && rt_existing.as_ref() != Some(auth) {
            write_atomic(&runtime_owner, &bytes);
        }
        if need_state && st_existing.as_ref() != Some(auth) {
            write_atomic(&state_owner, &bytes);
        }
        return;
    }

    let (pid, began) = tab_program().map_or((None, None), |(p, b)| (Some(p), Some(b)));
    let rex_socket = std::env::var("REX_SERVER_SOCKET").ok().filter(|s| !s.is_empty()).map(|s| s.strip_prefix("unix://").unwrap_or(&s).to_owned());

    let owner = ScreenOwner { host_key: None, session_id, block_id, started_by, pid, began, rex_socket, at: now_ms(), screen_pid, screen_started, screen_start_ticks };
    let Ok(bytes) = serde_json::to_vec_pretty(&owner) else { return };
    if need_runtime {
        write_atomic(&runtime_owner, &bytes);
    }
    if need_state {
        write_atomic(&state_owner, &bytes);
    }
}

fn say(v: serde_json::Value) {
    use std::io::Write as _;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

fn fail(why: impl std::fmt::Display) -> ! {
    eprintln!("olo screen: {why}");
    std::process::exit(1);
}

pub fn run(args: Vec<String>) -> ! {
    match args.first().map(String::as_str) {
        None | Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            std::process::exit(0);
        }
        Some("--protocol") => {
            let mut caps = vec!["control-ack", "warning-status", "history-frame-error", "history-snapshot-done", "export-part-ranges"];
            let prog = prefix().join("bin/olo-screen");
            if prog.is_file() && host_cli_supports_export_part(&prog) {
                caps.push("discontiguous_part");
            }
            if host_cli_honors_helper_control_installed().is_ok_and(|yes| yes) {
                caps.push("control-lock");
            }
            if host_cli_honors_lifecycle_lock().is_ok_and(|yes| yes) {
                caps.push("stop-expected");
                caps.push("stop-expected-started");
                caps.push("delete-expected");
                caps.push("clear-expected");
            }
            say(json!({
                "proto": PROTO,
                "commands": ["follow", "stream", "history-stream", "export"],
                "capabilities": caps,
            }));
            std::process::exit(0);
        }
        Some("install") => install(&args[1..]),
        Some("status") => {
            say(status());
            std::process::exit(0);
        }
        Some("default") => default(&args[1..]),
        Some("remove") => remove(),
        Some("follow") => crate::screen_follow::run(&args[1..]),
        Some("stream") => crate::screen_stream::run(&args[1..]),
        // Distinct binary wire verb (`history-stream NAME`, or `history --wire NAME`)
        // so human/agent `olo screen --on host history NAME` keeps forwarding to
        // host `olo-screen history NAME` unchanged (Astra finding 12).
        Some("history-stream") => crate::screen_history::run(&args[1..]),
        Some("history") if args.len() == 3 && ((args[1] == "--wire" && valid_name(&args[2])) || (valid_name(&args[1]) && args[2] == "--wire")) => {
            let name = if args[1] == "--wire" { args[2].clone() } else { args[1].clone() };
            crate::screen_history::run(&[name]);
        }
        Some("history") => history_cmd(&args),
        Some("export") => crate::screen_export::run(&args[1..]),
        // (The window that shows screens is a window: the desktop's.)
        Some("watch") => fail("watching a screen is done from the Olo desktop; here, `olo screen --on host look NAME` is a picture of it"),
        Some("start") => start(&args),
        Some("stop") => stop_cmd(&args),
        Some("delete") => delete_cmd(&args),
        // (Its own: what waits for a tab's program to end.)
        Some("keep-until") => keep_until(&args[1..]),
        Some(_) => exec(&args),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum GuardedError {
    /// Expected `started` or `last_used` no longer matches live state (exit 75).
    Stale(String),
    /// Invalid CLI options/arguments (exit 2).
    Usage(String),
    /// Operational error (exit 1).
    Other(String),
}

fn exit_guarded_error(err: GuardedError) -> ! {
    match err {
        GuardedError::Stale(msg) => {
            eprintln!("{msg}");
            std::process::exit(EXIT_STALE);
        }
        GuardedError::Usage(msg) => {
            eprintln!("olo screen: {msg}");
            std::process::exit(2);
        }
        GuardedError::Other(msg) => fail(msg),
    }
}

fn lifecycle_lock_timeout() -> Duration {
    std::env::var("OLO_SCREEN_LOCK_TIMEOUT_MS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|&ms| ms > 0).map(Duration::from_millis).unwrap_or(Duration::from_secs(10))
}

/// Pinned runtime directory descriptor + exclusive `NAME.lifecycle.lock` handle.
struct LockedRuntime {
    path: PathBuf,
    dir_fd: OwnedFd,
    _lock: std::fs::File,
}

fn unlink_at_ignore_enoent(dir_fd: &OwnedFd, file_name: &str) {
    let Ok(c) = std::ffi::CString::new(file_name) else { return };
    // SAFETY: `dir_fd` is a valid directory descriptor and `c` is a single component.
    unsafe {
        libc::unlinkat(dir_fd.raw(), c.as_ptr(), 0);
    }
}

fn open_verified_owned_dir(dir: &Path, create_private: bool) -> io::Result<OwnedFd> {
    if create_private {
        return open_or_create_private_dir_fd(dir);
    }
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;
    // SAFETY: `geteuid()` has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let inside_private = verify_ancestors_safe(dir, uid)?;
    let c = CString::new(dir.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in directory path"))?;
    // SAFETY: NUL-terminated path opened with O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC.
    let raw = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = OwnedFd::new(raw);
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.raw(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if (st.st_mode & libc::S_IFMT) != libc::S_IFDIR || !uid_owned_dir_mode_safe(uid, st.st_uid, st.st_gid, st.st_mode, inside_private) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("unsafe directory {}", dir.display())));
    }
    Ok(fd)
}

/// Open `<dir_fd>/<name>.lifecycle.lock` (`O_CREAT | O_RDWR | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC`,
/// mode `0600`, regular file owned by `geteuid()`) and acquire `flock(LOCK_EX)`
/// within the shared operation `deadline` (Item 6 / Findings 14 & 19).
fn lock_lifecycle_at_deadline(dir_fd: &OwnedFd, dir_path: &Path, name: &str, deadline: Instant, total_timeout: Duration) -> Result<std::fs::File, GuardedError> {
    use std::ffi::CString;
    use std::os::fd::FromRawFd as _;
    if !valid_name(name) {
        return Err(GuardedError::Usage(format!("invalid screen name {name:?}")));
    }
    let fname = format!("{name}.lifecycle.lock");
    let cname = CString::new(fname.as_str()).map_err(|_| GuardedError::Usage("invalid lock name".into()))?;
    // SAFETY: `geteuid()` has no preconditions.
    let uid = unsafe { libc::geteuid() };
    // SAFETY: `dir_fd` is our verified owned directory fd and `cname` is a validated single component.
    let fd = unsafe { libc::openat(dir_fd.raw(), cname.as_ptr(), libc::O_CREAT | libc::O_RDWR | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC, 0o600 as libc::c_uint) };
    if fd < 0 {
        return Err(GuardedError::Other(format!("couldn't open lock {}/{fname}: {}", dir_path.display(), io::Error::last_os_error())));
    }
    // SAFETY: `fd` is freshly opened and exclusively owned.
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let meta = file.metadata().map_err(|e| GuardedError::Other(format!("stat {}/{fname}: {e}", dir_path.display())))?;
    if !meta.file_type().is_file() || meta.uid() != uid {
        return Err(GuardedError::Other(format!("refusing unsafe lock file {}/{fname}", dir_path.display())));
    }
    let mut backoff = Duration::from_millis(2);
    loop {
        // SAFETY: nonblocking `flock` on our owned lock file descriptor.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(file);
        }
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if err.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(GuardedError::Other(format!("couldn't lock lifecycle for {name}: {err}")));
        }
        let rem = deadline.saturating_duration_since(Instant::now());
        if rem.is_zero() {
            let secs = total_timeout.as_secs().max(1);
            return Err(GuardedError::Other(format!("screen {name} is busy (another screen action didn't finish within {secs} s)")));
        }
        std::thread::sleep(backoff.min(rem));
        backoff = (backoff * 2).min(Duration::from_millis(25));
    }
}

/// Lock ALL existing candidate runtime directories in fixed canonical order
/// under ONE shared total deadline (not restarted per candidate dir — Item 6),
/// creating the primary directory only when `create_if_none` is true and none
/// exist, binding each lock to its verified directory descriptor (Findings 9, 14 & 19).
fn acquire_runtime_locks(candidates: &[PathBuf], name: &str, timeout: Duration, create_if_none: bool) -> Result<Vec<LockedRuntime>, GuardedError> {
    if !valid_name(name) {
        return Err(GuardedError::Usage(format!("invalid screen name {name:?}")));
    }
    let deadline = Instant::now() + timeout;
    let mut existing: Vec<PathBuf> = candidates.iter().filter(|d| std::fs::symlink_metadata(d).is_ok()).cloned().collect();
    if existing.is_empty() {
        if !create_if_none {
            return Ok(Vec::new());
        }
        if let Some(first) = candidates.first() {
            existing.push(first.clone());
        }
    }
    let mut locked = Vec::with_capacity(existing.len());
    for rt in existing {
        let dir_fd = open_verified_owned_dir(&rt, create_if_none).map_err(|e| GuardedError::Other(format!("unsafe runtime directory {}: {e}", rt.display())))?;
        let lock = lock_lifecycle_at_deadline(&dir_fd, &rt, name, deadline, timeout)?;
        locked.push(LockedRuntime { path: rt, dir_fd, _lock: lock });
    }
    Ok(locked)
}

/// Classification of one newline-terminated (or EOF) `actions.jsonl` line
/// produced by the shared schema-correct streaming decoder (Item 10 / Findings 5 & 20).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineAction {
    /// Empty or whitespace-only line.
    Empty,
    /// Valid `history::Action` JSON object with `at + ms`.
    Valid(u64),
    /// Valid index-preserving oversized placeholder (`kind == "oversized"`, `at == 0`, `ms == 0`, no args/result/error).
    OversizedPlaceholder,
    /// Malformed JSON, wrong schema/field type, duplicate field, or torn/incomplete record.
    Invalid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JsonContainer {
    /// Object state: 0=key or `}`, 1=`:`, 2=value, 3=`,` or `}`, 4=key after `,`.
    Obj(u8),
    /// Array state: 0=value or `]`, 1=`,` or `]`, 2=value after `,`.
    Arr(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JsonToken {
    None,
    /// Inside `"..."`:
    /// - `esc`: 0=normal, 1=after `\`, 2..=5=hex digits of `\uXXXX`.
    /// - `utf8_rem`: remaining UTF-8 continuation bytes expected (0..=3).
    /// - `utf8_min`/`utf8_max`: valid range for the FIRST continuation byte (rejects overlongs/surrogates).
    Str {
        esc: u8,
        hex_acc: u16,
        high_surrogate: u16,
        utf8_rem: u8,
        utf8_min: u8,
        utf8_max: u8,
        capture: bool,
        is_key: bool,
    },
    /// JSON number:
    /// `phase`: 0=after `-`, 1=after leading `0`, 2=int digits, 3=after `.`,
    /// 4=frac digits, 5=after `e`/`E`, 6=after `e+`/`e-`, 7=exp digits.
    Num {
        phase: u8,
        neg: bool,
        int_val: u64,
        int_overflow: bool,
    },
    /// Matching `true` (0), `false` (1), `null` (2) at byte index `pos`.
    Lit {
        kind: u8,
        pos: u8,
    },
}

/// Constant-memory, RFC 8259 & `history::Action` schema-correct streaming
/// decoder for newline-delimited `actions.jsonl` records (Item 10 / Findings 5, 15 & 20):
/// - Properly decodes JSON escapes including `\uXXXX` and surrogate pairs in
///   keys and `"kind"` (so `"\u0061t"` matches `"at"` identically to serde).
/// - Strictly validates JSON syntax (no missing values, no trailing commas,
///   no unescaped control chars, valid numbers/literals/UTF-8, nesting capped at 128).
/// - Strictly validates `history::Action` schema (`at: u64`, `ms: u64`,
///   `kind: String`, optional `args: Vec<String>`, `error: Option<String>`,
///   `result: Option<Value>`, no duplicate fields).
/// - Uses O(1) memory even for multi-MiB records and fast-skips plain ASCII
///   string bodies in a single linear scan.
#[derive(Clone, Debug)]
pub(crate) struct ActionLineScanner {
    started: bool,
    closed: bool,
    invalid: bool,
    saw_non_ws: bool,
    stack: [JsonContainer; 128],
    depth: usize,
    tok: JsonToken,
    /// Top-level field currently being parsed:
    /// 0=other, 1=at, 2=ms, 3=kind, 4=args, 5=result, 6=error.
    cur_field: u8,
    str_buf: [u8; 16],
    str_len: u8,
    str_overflow: bool,
    saw_at: bool,
    saw_ms: bool,
    saw_kind: bool,
    saw_args: bool,
    saw_result: bool,
    saw_error: bool,
    at_val: u64,
    ms_val: u64,
    is_oversized_kind: bool,
    args_non_empty: bool,
    result_is_some: bool,
    error_is_some: bool,
}

impl Default for ActionLineScanner {
    fn default() -> Self {
        Self {
            started: false,
            closed: false,
            invalid: false,
            saw_non_ws: false,
            stack: [JsonContainer::Obj(0); 128],
            depth: 0,
            tok: JsonToken::None,
            cur_field: 0,
            str_buf: [0u8; 16],
            str_len: 0,
            str_overflow: false,
            saw_at: false,
            saw_ms: false,
            saw_kind: false,
            saw_args: false,
            saw_result: false,
            saw_error: false,
            at_val: 0,
            ms_val: 0,
            is_oversized_kind: false,
            args_non_empty: false,
            result_is_some: false,
            error_is_some: false,
        }
    }
}

impl ActionLineScanner {
    fn reset_line(&mut self) {
        *self = Self::default();
    }

    fn push_captured_bytes(&mut self, bytes: &[u8]) {
        if self.str_overflow {
            return;
        }
        let len = self.str_len as usize;
        if len + bytes.len() > self.str_buf.len() {
            self.str_overflow = true;
        } else {
            self.str_buf[len..len + bytes.len()].copy_from_slice(bytes);
            self.str_len = (len + bytes.len()) as u8;
        }
    }

    fn push_captured_char(&mut self, ch: char) {
        let mut tmp = [0u8; 4];
        let s = ch.encode_utf8(&mut tmp);
        self.push_captured_bytes(s.as_bytes());
    }

    fn finish_value(&mut self) {
        if self.depth == 0 {
            self.closed = true;
            return;
        }
        match self.stack[self.depth - 1] {
            JsonContainer::Obj(2) => self.stack[self.depth - 1] = JsonContainer::Obj(3),
            JsonContainer::Arr(0 | 2) => self.stack[self.depth - 1] = JsonContainer::Arr(1),
            _ => self.invalid = true,
        }
    }

    fn finish_string(&mut self, is_key: bool) {
        self.tok = JsonToken::None;
        if is_key {
            if self.depth == 1 {
                let k = if self.str_overflow { b"" } else { &self.str_buf[..self.str_len as usize] };
                self.cur_field = match k {
                    b"at" => {
                        if self.saw_at {
                            self.invalid = true;
                        }
                        1
                    }
                    b"ms" => {
                        if self.saw_ms {
                            self.invalid = true;
                        }
                        2
                    }
                    b"kind" => {
                        if self.saw_kind {
                            self.invalid = true;
                        }
                        3
                    }
                    b"args" => {
                        if self.saw_args {
                            self.invalid = true;
                        }
                        4
                    }
                    b"result" => {
                        if self.saw_result {
                            self.invalid = true;
                        }
                        5
                    }
                    b"error" => {
                        if self.saw_error {
                            self.invalid = true;
                        }
                        6
                    }
                    _ => 0,
                };
            }
            if self.depth > 0 {
                self.stack[self.depth - 1] = JsonContainer::Obj(1);
            } else {
                self.invalid = true;
            }
        } else {
            if self.depth == 1 && self.cur_field == 3 {
                self.saw_kind = true;
                self.is_oversized_kind = !self.str_overflow && &self.str_buf[..self.str_len as usize] == b"oversized";
            }
            self.finish_value();
        }
    }

    fn finish_number(&mut self, phase: u8, neg: bool, int_val: u64, int_overflow: bool) {
        self.tok = JsonToken::None;
        if !matches!(phase, 1 | 2 | 4 | 7) {
            self.invalid = true;
            return;
        }
        if self.depth == 1 && matches!(self.cur_field, 1 | 2) {
            if neg || int_overflow || !matches!(phase, 1 | 2) {
                self.invalid = true;
                return;
            }
            if self.cur_field == 1 {
                self.at_val = int_val;
                self.saw_at = true;
            } else {
                self.ms_val = int_val;
                self.saw_ms = true;
            }
        }
        self.finish_value();
    }

    fn start_value(&mut self, b: u8) {
        // Enforce top-level `Action` field types & `args: Vec<String>`.
        if self.depth == 1 {
            match self.cur_field {
                1 | 2 => {
                    if !b.is_ascii_digit() {
                        self.invalid = true;
                        return;
                    }
                }
                3 => {
                    if b != b'"' {
                        self.invalid = true;
                        return;
                    }
                }
                4 => {
                    if b != b'[' {
                        self.invalid = true;
                        return;
                    }
                    self.saw_args = true;
                }
                5 => {
                    self.saw_result = true;
                    if b != b'n' {
                        self.result_is_some = true;
                    }
                }
                6 => {
                    self.saw_error = true;
                    if b == b'"' {
                        self.error_is_some = true;
                    } else if b != b'n' {
                        self.invalid = true;
                        return;
                    }
                }
                _ => {}
            }
        } else if self.depth == 2 && self.cur_field == 4 {
            // Every element of `args` must be a JSON string.
            if b != b'"' {
                self.invalid = true;
                return;
            }
            self.args_non_empty = true;
        }

        match b {
            b'"' => {
                let capture = self.depth == 1 && self.cur_field == 3;
                if capture {
                    self.str_len = 0;
                    self.str_overflow = false;
                }
                self.tok = JsonToken::Str { esc: 0, hex_acc: 0, high_surrogate: 0, utf8_rem: 0, utf8_min: 0x80, utf8_max: 0xbf, capture, is_key: false };
            }
            b'{' => {
                if self.depth >= self.stack.len() {
                    self.invalid = true;
                    return;
                }
                self.stack[self.depth] = JsonContainer::Obj(0);
                self.depth += 1;
            }
            b'[' => {
                if self.depth >= self.stack.len() {
                    self.invalid = true;
                    return;
                }
                self.stack[self.depth] = JsonContainer::Arr(0);
                self.depth += 1;
            }
            b'-' => {
                self.tok = JsonToken::Num { phase: 0, neg: true, int_val: 0, int_overflow: false };
            }
            b'0' => {
                self.tok = JsonToken::Num { phase: 1, neg: false, int_val: 0, int_overflow: false };
            }
            b'1'..=b'9' => {
                self.tok = JsonToken::Num { phase: 2, neg: false, int_val: (b - b'0') as u64, int_overflow: false };
            }
            b't' => self.tok = JsonToken::Lit { kind: 0, pos: 1 },
            b'f' => self.tok = JsonToken::Lit { kind: 1, pos: 1 },
            b'n' => self.tok = JsonToken::Lit { kind: 2, pos: 1 },
            _ => self.invalid = true,
        }
    }

    fn complete_action(&self) -> LineAction {
        if !self.saw_non_ws {
            return LineAction::Empty;
        }
        if self.started && self.closed && !self.invalid && matches!(self.tok, JsonToken::None) && self.depth == 0 && self.saw_at && self.saw_ms && self.saw_kind {
            if self.is_oversized_kind && self.at_val == 0 && self.ms_val == 0 && !self.args_non_empty && !self.result_is_some && !self.error_is_some {
                LineAction::OversizedPlaceholder
            } else {
                LineAction::Valid(self.at_val.saturating_add(self.ms_val))
            }
        } else {
            LineAction::Invalid
        }
    }

    /// Inspect current unfinished-at-EOF line state without mutating `self`.
    pub(crate) fn peek_eof_action(&self) -> LineAction {
        self.complete_action()
    }

    /// Inspect current unfinished-at-EOF line timestamp if it is a complete valid action.
    pub(crate) fn peek_complete_eof(&self) -> Option<u64> {
        match self.complete_action() {
            LineAction::Valid(ts) => Some(ts),
            _ => None,
        }
    }

    /// Feed a slice of bytes, invoking `on_line(LineAction)` for every
    /// `\n`-terminated line (including `Invalid` and `Empty` so guards can
    /// reject invalid/torn final rows without losing track of line boundaries).
    pub(crate) fn feed_lines(&mut self, bytes: &[u8], mut on_line: impl FnMut(LineAction)) {
        let mut i = 0usize;
        while i < bytes.len() {
            // Fast path: skip plain ASCII string body bytes in one linear scan when
            // not capturing, stopping at any control byte (< 0x20, including `\n`),
            // UTF-8/non-ASCII byte (>= 0x80), quote (`"`), or backslash (`\`).
            if !self.invalid
                && let JsonToken::Str { esc: 0, high_surrogate: 0, utf8_rem: 0, capture: false, .. } = self.tok
            {
                let adv = bytes[i..].iter().position(|&c| !(0x20..0x80).contains(&c) || c == b'"' || c == b'\\').unwrap_or(bytes.len() - i);
                i += adv;
                if i >= bytes.len() {
                    break;
                }
            }
            // Fast path: skip rest of an already-invalid line directly to `\n`.
            if self.invalid {
                if let Some(nl) = memchr::memchr(b'\n', &bytes[i..]) {
                    on_line(LineAction::Invalid);
                    self.reset_line();
                    i += nl + 1;
                    continue;
                } else {
                    break;
                }
            }

            let b = bytes[i];
            i += 1;

            if b == b'\n' {
                // Note: if inside a number at EOL, finish it first? No — a JSON object must have closed `}` before `\n`.
                let res = self.complete_action();
                on_line(res);
                self.reset_line();
                continue;
            }
            if !matches!(b, b' ' | b'\t' | b'\r') {
                self.saw_non_ws = true;
            }
            if !self.started {
                if matches!(b, b' ' | b'\t' | b'\r') {
                    continue;
                }
                if b == b'{' {
                    self.started = true;
                    self.stack[0] = JsonContainer::Obj(0);
                    self.depth = 1;
                } else {
                    self.invalid = true;
                }
                continue;
            }
            if self.closed {
                if !matches!(b, b' ' | b'\t' | b'\r') {
                    self.invalid = true;
                }
                continue;
            }

            // Active scalar token?
            match self.tok {
                JsonToken::Str { mut esc, mut hex_acc, mut high_surrogate, mut utf8_rem, mut utf8_min, mut utf8_max, capture, is_key } => {
                    if utf8_rem > 0 {
                        if b < utf8_min || b > utf8_max {
                            self.invalid = true;
                            continue;
                        }
                        if capture {
                            self.push_captured_bytes(&[b]);
                        }
                        utf8_rem -= 1;
                        utf8_min = 0x80;
                        utf8_max = 0xbf;
                        self.tok = JsonToken::Str { esc, hex_acc, high_surrogate, utf8_rem, utf8_min, utf8_max, capture, is_key };
                        continue;
                    }
                    if esc == 1 {
                        if high_surrogate != 0 && b != b'u' {
                            self.invalid = true;
                            continue;
                        }
                        let ch = match b {
                            b'"' => Some(b'"'),
                            b'\\' => Some(b'\\'),
                            b'/' => Some(b'/'),
                            b'b' => Some(0x08),
                            b'f' => Some(0x0c),
                            b'n' => Some(b'\n'),
                            b'r' => Some(b'\r'),
                            b't' => Some(b'\t'),
                            b'u' => {
                                esc = 2;
                                hex_acc = 0;
                                self.tok = JsonToken::Str { esc, hex_acc, high_surrogate, utf8_rem, utf8_min, utf8_max, capture, is_key };
                                continue;
                            }
                            _ => {
                                self.invalid = true;
                                continue;
                            }
                        };
                        if let Some(c) = ch
                            && capture
                        {
                            self.push_captured_bytes(&[c]);
                        }
                        esc = 0;
                        self.tok = JsonToken::Str { esc, hex_acc, high_surrogate, utf8_rem, utf8_min, utf8_max, capture, is_key };
                        continue;
                    }
                    if esc >= 2 {
                        let digit = match b {
                            b'0'..=b'9' => (b - b'0') as u16,
                            b'a'..=b'f' => (b - b'a' + 10) as u16,
                            b'A'..=b'F' => (b - b'A' + 10) as u16,
                            _ => {
                                self.invalid = true;
                                continue;
                            }
                        };
                        hex_acc = (hex_acc << 4) | digit;
                        if esc < 5 {
                            esc += 1;
                            self.tok = JsonToken::Str { esc, hex_acc, high_surrogate, utf8_rem, utf8_min, utf8_max, capture, is_key };
                        } else {
                            let code = hex_acc;
                            esc = 0;
                            if high_surrogate != 0 {
                                if !(0xDC00..=0xDFFF).contains(&code) {
                                    self.invalid = true;
                                    continue;
                                }
                                let cp = 0x10000 + (((high_surrogate - 0xD800) as u32) << 10) + ((code - 0xDC00) as u32);
                                high_surrogate = 0;
                                if capture && let Some(ch) = char::from_u32(cp) {
                                    self.push_captured_char(ch);
                                }
                            } else if (0xD800..=0xDBFF).contains(&code) {
                                high_surrogate = code;
                            } else if (0xDC00..=0xDFFF).contains(&code) {
                                self.invalid = true;
                                continue;
                            } else if capture && let Some(ch) = char::from_u32(code as u32) {
                                self.push_captured_char(ch);
                            }
                            self.tok = JsonToken::Str { esc, hex_acc: 0, high_surrogate, utf8_rem, utf8_min, utf8_max, capture, is_key };
                        }
                        continue;
                    }
                    // `esc == 0`
                    if high_surrogate != 0 && b != b'\\' {
                        self.invalid = true;
                        continue;
                    }
                    if b == b'\\' {
                        self.tok = JsonToken::Str { esc: 1, hex_acc: 0, high_surrogate, utf8_rem: 0, utf8_min: 0x80, utf8_max: 0xbf, capture, is_key };
                        continue;
                    }
                    if b == b'"' {
                        self.finish_string(is_key);
                        continue;
                    }
                    if b < 0x20 {
                        self.invalid = true;
                        continue;
                    }
                    if b < 0x80 {
                        if capture {
                            self.push_captured_bytes(&[b]);
                        }
                        continue;
                    }
                    // Multi-byte UTF-8 lead byte validation.
                    match b {
                        0xc2..=0xdf => {
                            utf8_rem = 1;
                            utf8_min = 0x80;
                            utf8_max = 0xbf;
                        }
                        0xe0 => {
                            utf8_rem = 2;
                            utf8_min = 0xa0;
                            utf8_max = 0xbf;
                        }
                        0xe1..=0xec | 0xee..=0xef => {
                            utf8_rem = 2;
                            utf8_min = 0x80;
                            utf8_max = 0xbf;
                        }
                        0xed => {
                            utf8_rem = 2;
                            utf8_min = 0x80;
                            utf8_max = 0x9f;
                        }
                        0xf0 => {
                            utf8_rem = 3;
                            utf8_min = 0x90;
                            utf8_max = 0xbf;
                        }
                        0xf1..=0xf3 => {
                            utf8_rem = 3;
                            utf8_min = 0x80;
                            utf8_max = 0xbf;
                        }
                        0xf4 => {
                            utf8_rem = 3;
                            utf8_min = 0x80;
                            utf8_max = 0x8f;
                        }
                        _ => {
                            self.invalid = true;
                            continue;
                        }
                    }
                    if capture {
                        self.push_captured_bytes(&[b]);
                    }
                    self.tok = JsonToken::Str { esc: 0, hex_acc: 0, high_surrogate, utf8_rem, utf8_min, utf8_max, capture, is_key };
                    continue;
                }
                JsonToken::Lit { kind, pos } => {
                    let word: &[u8] = match kind {
                        0 => b"true",
                        1 => b"false",
                        _ => b"null",
                    };
                    if word.get(pos as usize).copied() != Some(b) {
                        self.invalid = true;
                        continue;
                    }
                    let next = pos + 1;
                    if next as usize == word.len() {
                        self.tok = JsonToken::None;
                        self.finish_value();
                    } else {
                        self.tok = JsonToken::Lit { kind, pos: next };
                    }
                    continue;
                }
                JsonToken::Num { mut phase, neg, mut int_val, mut int_overflow } => {
                    let consumed = match (phase, b) {
                        (0, b'0') => {
                            phase = 1;
                            true
                        }
                        (0, b'1'..=b'9') => {
                            phase = 2;
                            int_val = (b - b'0') as u64;
                            true
                        }
                        (2, b'0'..=b'9') => {
                            match int_val.checked_mul(10).and_then(|v| v.checked_add((b - b'0') as u64)) {
                                Some(v) => int_val = v,
                                None => int_overflow = true,
                            }
                            true
                        }
                        (1 | 2, b'.') => {
                            phase = 3;
                            true
                        }
                        (3 | 4, b'0'..=b'9') => {
                            phase = 4;
                            true
                        }
                        (1 | 2 | 4, b'e' | b'E') => {
                            phase = 5;
                            true
                        }
                        (5, b'+' | b'-') => {
                            phase = 6;
                            true
                        }
                        (5..=7, b'0'..=b'9') => {
                            phase = 7;
                            true
                        }
                        _ => false,
                    };
                    if consumed {
                        self.tok = JsonToken::Num { phase, neg, int_val, int_overflow };
                        continue;
                    }
                    self.finish_number(phase, neg, int_val, int_overflow);
                    if self.invalid {
                        continue;
                    }
                    // Fall through to process delimiter byte `b` in container state!
                }
                JsonToken::None => {}
            }

            // Outside scalar tokens: skip JSON whitespace.
            if matches!(b, b' ' | b'\t' | b'\r') {
                continue;
            }
            if self.depth == 0 {
                self.invalid = true;
                continue;
            }
            match self.stack[self.depth - 1] {
                JsonContainer::Obj(0) => match b {
                    b'}' => {
                        self.depth -= 1;
                        self.finish_value();
                    }
                    b'"' => {
                        let capture = self.depth == 1;
                        if capture {
                            self.str_len = 0;
                            self.str_overflow = false;
                        }
                        self.tok = JsonToken::Str { esc: 0, hex_acc: 0, high_surrogate: 0, utf8_rem: 0, utf8_min: 0x80, utf8_max: 0xbf, capture, is_key: true };
                    }
                    _ => self.invalid = true,
                },
                JsonContainer::Obj(4) => match b {
                    b'"' => {
                        let capture = self.depth == 1;
                        if capture {
                            self.str_len = 0;
                            self.str_overflow = false;
                        }
                        self.tok = JsonToken::Str { esc: 0, hex_acc: 0, high_surrogate: 0, utf8_rem: 0, utf8_min: 0x80, utf8_max: 0xbf, capture, is_key: true };
                    }
                    _ => self.invalid = true,
                },
                JsonContainer::Obj(1) => {
                    if b == b':' {
                        self.stack[self.depth - 1] = JsonContainer::Obj(2);
                    } else {
                        self.invalid = true;
                    }
                }
                JsonContainer::Obj(2) => self.start_value(b),
                JsonContainer::Obj(3) => match b {
                    b',' => self.stack[self.depth - 1] = JsonContainer::Obj(4),
                    b'}' => {
                        self.depth -= 1;
                        self.finish_value();
                    }
                    _ => self.invalid = true,
                },
                JsonContainer::Arr(0) => match b {
                    b']' => {
                        self.depth -= 1;
                        self.finish_value();
                    }
                    _ => self.start_value(b),
                },
                JsonContainer::Arr(1) => match b {
                    b',' => self.stack[self.depth - 1] = JsonContainer::Arr(2),
                    b']' => {
                        self.depth -= 1;
                        self.finish_value();
                    }
                    _ => self.invalid = true,
                },
                JsonContainer::Arr(2) => self.start_value(b),
                _ => self.invalid = true,
            }
        }
    }

    /// Feed a slice of bytes, invoking `on_record(at + ms)` for every valid
    /// complete `\n`-terminated `history::Action` record.
    pub(crate) fn feed(&mut self, bytes: &[u8], mut on_record: impl FnMut(u64)) {
        self.feed_lines(bytes, |res| {
            if let LineAction::Valid(ts) = res {
                on_record(ts);
            }
        });
    }
}

/// Snapshot of `actions.jsonl` inode metadata `(dev, ino, len, mtime_sec, mtime_nsec)`.
type ActionsMetaSnap = (u64, u64, u64, i64, i64);

/// Tri-state result of probing `actions.jsonl` for the latest action timestamp
/// (`at + ms`), distinguishing verified absence from read/parse errors (Finding 5).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LastUsedProbe {
    Value { last_used: u64, meta: ActionsMetaSnap },
    Absent { meta: Option<ActionsMetaSnap> },
    Unreadable(String),
}

/// Result of a budgeted tail probe used by `screen_follow.rs` (Item 10 / Findings 20 & 24).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BudgetedTailProbe {
    Value(u64),
    Absent,
    Inconclusive,
    Unreadable(String),
}

/// Budgeted O(1) EOF tail probe charging every byte read against `*budget_bytes`
/// and `*bytes_read_out` (Item 10 / Findings 20 & 24). Never reads past `*budget_bytes`,
/// and refuses a torn or invalid final record.
pub(crate) fn probe_last_used_from_file_budgeted(f: &mut std::fs::File, len: u64, budget_bytes: &mut u64, bytes_read_out: &mut u64) -> BudgetedTailProbe {
    use std::io::Seek as _;
    if len == 0 {
        return BudgetedTailProbe::Absent;
    }
    if *budget_bytes == 0 {
        return BudgetedTailProbe::Inconclusive;
    }
    let mut prev_need = 0u64;
    for window in [8 * 1024u64, 64 * 1024, 512 * 1024, 4 * 1024 * 1024] {
        let target = len.min(window);
        let clamped = target > *budget_bytes;
        // Clamp one final attempt to remaining `*budget_bytes` if it covers more
        // than the previous window so large records within remaining budget succeed!
        let need = if clamped { len.min(*budget_bytes) } else { target };
        if need <= prev_need {
            return BudgetedTailProbe::Inconclusive;
        }
        prev_need = need;
        let from = len - need;
        if let Err(e) = f.seek(io::SeekFrom::Start(from)) {
            return BudgetedTailProbe::Unreadable(format!("seek actions.jsonl: {e}"));
        }
        let mut scanner = ActionLineScanner::default();
        let mut last_valid: Option<u64> = None;
        let mut last_was_invalid = false;
        let mut skipped_partial = from == 0;
        let mut buf = [0u8; 32 * 1024];
        let mut rem = need;
        while rem > 0 {
            let want = (buf.len() as u64).min(rem).min(*budget_bytes) as usize;
            if want == 0 {
                return BudgetedTailProbe::Inconclusive;
            }
            let n = match f.read(&mut buf[..want]) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return BudgetedTailProbe::Unreadable(format!("read actions.jsonl: {e}")),
            };
            rem -= n as u64;
            *budget_bytes = budget_bytes.saturating_sub(n as u64);
            *bytes_read_out = bytes_read_out.saturating_add(n as u64);
            let slice = if !skipped_partial {
                match memchr::memchr(b'\n', &buf[..n]) {
                    Some(nl) => {
                        skipped_partial = true;
                        &buf[nl + 1..n]
                    }
                    None => continue,
                }
            } else {
                &buf[..n]
            };
            scanner.feed_lines(slice, |res| match res {
                LineAction::Empty => {}
                LineAction::Valid(ts) => {
                    last_valid = Some(ts);
                    last_was_invalid = false;
                }
                LineAction::OversizedPlaceholder => {
                    last_was_invalid = false;
                }
                LineAction::Invalid => {
                    last_was_invalid = true;
                }
            });
        }
        match scanner.peek_eof_action() {
            LineAction::Empty => {}
            LineAction::Valid(ts) => {
                last_valid = Some(ts);
                last_was_invalid = false;
            }
            LineAction::OversizedPlaceholder => {
                last_was_invalid = false;
            }
            LineAction::Invalid => {
                last_was_invalid = true;
            }
        }
        if last_was_invalid {
            return BudgetedTailProbe::Unreadable("actions.jsonl has an invalid or incomplete final action record".into());
        }
        if let Some(ts) = last_valid {
            return BudgetedTailProbe::Value(ts);
        }
        if from == 0 {
            return BudgetedTailProbe::Absent;
        }
        if clamped {
            return BudgetedTailProbe::Inconclusive;
        }
    }
    BudgetedTailProbe::Inconclusive
}

/// Read the latest recorded action timestamp (`at + ms` of the LAST valid action
/// line, matching `crates/screen-pane/src/history.rs::last_used_fast_in`) from
/// the END of `actions.jsonl` without any 16 MiB prefix cap. Fails closed
/// (`Unreadable`) whenever the final non-empty record is invalid JSON or torn
/// rather than falling back to an earlier row (Item 10 / Findings 5, 15, 20).
pub(crate) fn probe_last_used_at(dir_fd: &OwnedFd) -> LastUsedProbe {
    use std::io::Seek as _;
    let (mut f, meta) = match open_regular_at(dir_fd, "actions.jsonl") {
        Ok(v) => v,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return LastUsedProbe::Absent { meta: None },
        Err(e) => return LastUsedProbe::Unreadable(format!("couldn't read actions.jsonl: {e}")),
    };
    let snap: ActionsMetaSnap = (meta.dev(), meta.ino(), meta.len(), meta.mtime(), meta.mtime_nsec());
    let len = meta.len();
    if len == 0 {
        return LastUsedProbe::Absent { meta: Some(snap) };
    }
    // Fast O(1) tail scan expanding backwards from EOF (64 KiB -> 512 KiB -> 4 MiB -> 16 MiB),
    // falling back to a full streaming scan from offset 0 if a giant tail still has no complete line.
    for window in [64 * 1024u64, 512 * 1024, 4 * 1024 * 1024, 16 * 1024 * 1024] {
        let from = len.saturating_sub(window);
        if let Err(e) = f.seek(io::SeekFrom::Start(from)) {
            return LastUsedProbe::Unreadable(format!("seek actions.jsonl: {e}"));
        }
        let mut scanner = ActionLineScanner::default();
        let mut last_valid: Option<u64> = None;
        let mut last_was_invalid = false;
        let mut skipped_partial = from == 0;
        let mut buf = [0u8; 64 * 1024];
        let mut rem = len - from;
        while rem > 0 {
            let want = (buf.len() as u64).min(rem) as usize;
            let n = match f.read(&mut buf[..want]) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return LastUsedProbe::Unreadable(format!("read actions.jsonl: {e}")),
            };
            rem -= n as u64;
            let slice = if !skipped_partial {
                match memchr::memchr(b'\n', &buf[..n]) {
                    Some(nl) => {
                        skipped_partial = true;
                        &buf[nl + 1..n]
                    }
                    None => continue,
                }
            } else {
                &buf[..n]
            };
            scanner.feed_lines(slice, |res| match res {
                LineAction::Empty => {}
                LineAction::Valid(ts) => {
                    last_valid = Some(ts);
                    last_was_invalid = false;
                }
                LineAction::OversizedPlaceholder => {
                    last_was_invalid = false;
                }
                LineAction::Invalid => {
                    last_was_invalid = true;
                }
            });
        }
        match scanner.peek_eof_action() {
            LineAction::Empty => {}
            LineAction::Valid(ts) => {
                last_valid = Some(ts);
                last_was_invalid = false;
            }
            LineAction::OversizedPlaceholder => {
                last_was_invalid = false;
            }
            LineAction::Invalid => {
                last_was_invalid = true;
            }
        }
        if last_was_invalid {
            return LastUsedProbe::Unreadable("actions.jsonl has an invalid or incomplete final action record".into());
        }
        if let Some(ts) = last_valid {
            return LastUsedProbe::Value { last_used: ts, meta: snap };
        }
        if from == 0 {
            return LastUsedProbe::Absent { meta: Some(snap) };
        }
    }
    // File > 16 MiB and final 16 MiB contained no valid/invalid record: full streaming scan from 0 (no cap).
    if let Err(e) = f.seek(io::SeekFrom::Start(0)) {
        return LastUsedProbe::Unreadable(format!("seek actions.jsonl: {e}"));
    }
    let mut scanner = ActionLineScanner::default();
    let mut last_valid: Option<u64> = None;
    let mut last_was_invalid = false;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return LastUsedProbe::Unreadable(format!("read actions.jsonl: {e}")),
        };
        scanner.feed_lines(&buf[..n], |res| match res {
            LineAction::Empty => {}
            LineAction::Valid(ts) => {
                last_valid = Some(ts);
                last_was_invalid = false;
            }
            LineAction::OversizedPlaceholder => {
                last_was_invalid = false;
            }
            LineAction::Invalid => {
                last_was_invalid = true;
            }
        });
    }
    match scanner.peek_eof_action() {
        LineAction::Empty => {}
        LineAction::Valid(ts) => {
            last_valid = Some(ts);
            last_was_invalid = false;
        }
        LineAction::OversizedPlaceholder => {
            last_was_invalid = false;
        }
        LineAction::Invalid => {
            last_was_invalid = true;
        }
    }
    if last_was_invalid {
        LastUsedProbe::Unreadable("actions.jsonl has an invalid or incomplete final action record".into())
    } else if let Some(ts) = last_valid {
        LastUsedProbe::Value { last_used: ts, meta: snap }
    } else {
        LastUsedProbe::Absent { meta: Some(snap) }
    }
}

/// Tri-state session liveness verification for a pinned PID (Item 1 / Findings 5, 9, 11, 12).
enum PidVerify {
    Alive,
    DeadOrUnrelated,
    Unverifiable(String),
}

fn fstat_dev_ino(fd: &OwnedFd) -> io::Result<(u64, u64)> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `fstat` on a valid open directory fd.
    if unsafe { libc::fstat(fd.raw(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((st.st_dev as u64, st.st_ino as u64))
}

fn verify_session_process(name: &str, rt_dir_fd: &OwnedFd, rt_path: &Path, pid: i32) -> PidVerify {
    if pid <= 0 {
        return PidVerify::Unverifiable(format!("descriptor for {name} has invalid session pid {pid}"));
    }
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => s,
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound) || e.raw_os_error() == Some(libc::ESRCH) => return PidVerify::DeadOrUnrelated,
        Err(e) => return PidVerify::Unverifiable(format!("couldn't inspect session pid {pid} stat: {e}")),
    };
    let Some(state) = stat.rsplit_once(')').and_then(|(_, rest)| rest.trim_start().as_bytes().first().copied()) else {
        return PidVerify::Unverifiable(format!("malformed /proc/{pid}/stat"));
    };
    if matches!(state, b'Z' | b'X' | b'x') {
        return PidVerify::DeadOrUnrelated;
    }
    let cmdline = match std::fs::read(format!("/proc/{pid}/cmdline")) {
        Ok(c) => c,
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound) || e.raw_os_error() == Some(libc::ESRCH) => return PidVerify::DeadOrUnrelated,
        Err(e) => return PidVerify::Unverifiable(format!("couldn't inspect session pid {pid} cmdline: {e}")),
    };
    if !is_session_cmdline(&cmdline) {
        return PidVerify::DeadOrUnrelated;
    }
    let environ = match std::fs::read(format!("/proc/{pid}/environ")) {
        Ok(e) => e,
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound) || e.raw_os_error() == Some(libc::ESRCH) => return PidVerify::DeadOrUnrelated,
        Err(e) => return PidVerify::Unverifiable(format!("couldn't inspect session pid {pid} environ: {e}")),
    };
    let wanted = format!("OLO_SCREEN={name}");
    if !environ.split(|b| *b == 0).any(|v| v == wanted.as_bytes()) {
        return PidVerify::DeadOrUnrelated;
    }
    // Item 1 (Findings 5, 9, 11, 12): compare runtime/settings directory identity
    // via `(dev, ino)` rather than raw path strings so symlinked spellings like
    // `/var/run/user/UID` vs `/run/user/UID` match. Treat any unexplained identity
    // mismatch on a live `olo-session` for `name` as an observation error (fail
    // closed, NEVER classify as stopped!).
    if let Some(settings_val) = environ.split(|b| *b == 0).find_map(|v| v.strip_prefix(b"OLO_APP_SERVER_SETTINGS="))
        && !settings_val.is_empty()
    {
        use std::os::unix::ffi::OsStrExt as _;
        let settings_path = Path::new(std::ffi::OsStr::from_bytes(settings_val));
        let Ok((rt_dev, rt_ino)) = fstat_dev_ino(rt_dir_fd) else {
            return PidVerify::Unverifiable(format!("couldn't stat runtime directory {}", rt_path.display()));
        };
        let Some(parent) = settings_path.parent() else {
            return PidVerify::Unverifiable(format!("session pid {pid} has invalid OLO_APP_SERVER_SETTINGS path"));
        };
        let parent_meta = match std::fs::metadata(parent) {
            Ok(m) if m.is_dir() => m,
            Ok(_) => {
                return PidVerify::Unverifiable(format!("session pid {pid} settings parent {} is not a directory", parent.display()));
            }
            Err(e) => {
                return PidVerify::Unverifiable(format!("couldn't verify session pid {pid} settings directory {}: {e}", parent.display()));
            }
        };
        if (parent_meta.dev(), parent_meta.ino()) != (rt_dev, rt_ino) {
            return PidVerify::Unverifiable(format!("session pid {pid} for {name} belongs to runtime directory {} rather than {}", parent.display(), rt_path.display()));
        }
    }
    PidVerify::Alive
}

/// Tri-state result of probing locked runtime dirs for a running session (Finding 5).
enum RunningProbe {
    Running { rt_idx: usize, screen: RuntimeScreen, pidfd: OwnedFd },
    Stopped,
    Unverifiable(String),
}

fn probe_running_in_locked_rts(locked: &[LockedRuntime], name: &str) -> RunningProbe {
    let fname = format!("{name}.json");
    let mut found: Option<(usize, RuntimeScreen, OwnedFd)> = None;
    for (idx, rt) in locked.iter().enumerate() {
        let (file, meta) = match open_regular_at(&rt.dir_fd, &fname) {
            Ok(v) => v,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return RunningProbe::Unverifiable(format!("couldn't read {}/{fname}: {e}", rt.path.display())),
        };
        if meta.len() > 64 * 1024 {
            return RunningProbe::Unverifiable(format!("{}/{fname} is too large", rt.path.display()));
        }
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        if let Err(e) = file.take(64 * 1024 + 1).read_to_end(&mut bytes) {
            return RunningProbe::Unverifiable(format!("couldn't read {}/{fname}: {e}", rt.path.display()));
        }
        let screen: RuntimeScreen = match serde_json::from_slice(&bytes) {
            Ok(s) => s,
            Err(e) => return RunningProbe::Unverifiable(format!("corrupt descriptor {}/{fname}: {e}", rt.path.display())),
        };
        if screen.name != name {
            return RunningProbe::Unverifiable(format!("descriptor {}/{fname} names {:?}, not {:?}", rt.path.display(), screen.name, name));
        }
        if screen.pid <= 0 {
            return RunningProbe::Unverifiable(format!("descriptor {}/{fname} has invalid pid {}", rt.path.display(), screen.pid));
        }
        let pidfd = match pidfd_open_result(screen.pid) {
            Ok(fd) => fd,
            Err(e) if e.raw_os_error() == Some(libc::ESRCH) => continue,
            Err(e) => return RunningProbe::Unverifiable(format!("couldn't verify session pid {} via pidfd_open: {e}", screen.pid)),
        };
        match verify_session_process(name, &rt.dir_fd, &rt.path, screen.pid) {
            PidVerify::Alive => {
                if pidfd_exited(&pidfd) {
                    continue;
                }
                if found.is_some() {
                    return RunningProbe::Unverifiable(format!("screen {name} appears running in multiple runtime directories"));
                }
                found = Some((idx, screen, pidfd));
            }
            PidVerify::DeadOrUnrelated => continue,
            PidVerify::Unverifiable(why) => return RunningProbe::Unverifiable(why),
        }
    }
    match found {
        Some((rt_idx, screen, pidfd)) => RunningProbe::Running { rt_idx, screen, pidfd },
        None => RunningProbe::Stopped,
    }
}

/// Phase 1 expectation check (Item 9 / Finding 25): check running-state and
/// `--expected-started` mismatches IMMEDIATELY BEFORE reading `actions.jsonl`,
/// so a proved state change (e.g. live screen against `--expected-started 0`)
/// returns exit `75` even if `actions.jsonl` is unreadable.
fn check_expected_started_state(name: &str, verb: GuardedVerb, expected: ExpectedScreen, running_started: Option<Option<u64>>) -> Result<(), GuardedError> {
    match verb {
        GuardedVerb::Stop => {
            let Some(live_started) = running_started else {
                return Err(GuardedError::Stale(format!("{name} isn't running any more")));
            };
            if let Some(exp_started) = expected.expected_started
                && (exp_started == 0 || live_started != Some(exp_started))
            {
                return Err(GuardedError::Stale(format!("{name} was started again since you asked")));
            }
        }
        GuardedVerb::Delete => {
            // Finding 12: guarded Delete is stopped-only (matches desktop & Rex contract).
            if running_started.is_some() {
                return Err(GuardedError::Stale(format!("{name} was started again since you asked")));
            }
            if let Some(exp_started) = expected.expected_started
                && exp_started > 0
            {
                return Err(GuardedError::Stale(format!("{name} isn't running any more")));
            }
        }
        GuardedVerb::ClearHistory => match (running_started, expected.expected_started) {
            (Some(_), None | Some(0)) => {
                return Err(GuardedError::Stale(format!("{name} was started again since you asked")));
            }
            (Some(live_started), Some(exp_started)) if live_started != Some(exp_started) => {
                return Err(GuardedError::Stale(format!("{name} was started again since you asked")));
            }
            (None, Some(exp_started)) if exp_started > 0 => {
                return Err(GuardedError::Stale(format!("{name} isn't running any more")));
            }
            _ => {}
        },
    }
    Ok(())
}

/// Phase 2 expectation check: compare observed `actual_last_used` against
/// `--expected-last-used` (`Some(0)` matches observed empty history).
fn check_expected_last_used(name: &str, expected_lu: Option<u64>, actual: Option<u64>) -> Result<(), GuardedError> {
    if let Some(exp_lu) = expected_lu {
        let actual = actual.unwrap_or(0);
        if actual > exp_lu {
            return Err(GuardedError::Stale(format!("{name} has newer activity since you asked")));
        }
        if actual < exp_lu {
            return Err(GuardedError::Stale(format!("{name}'s activity changed since you asked")));
        }
    }
    Ok(())
}

fn wait_pidfd(pidfd: &OwnedFd, max: Duration) -> bool {
    let deadline = Instant::now() + max;
    loop {
        if pidfd_exited(pidfd) {
            return true;
        }
        let rem = deadline.saturating_duration_since(Instant::now());
        if rem.is_zero() {
            return pidfd_exited(pidfd);
        }
        let mut pfd = libc::pollfd { fd: pidfd.raw(), events: libc::POLLIN, revents: 0 };
        // SAFETY: valid owned pidfd.
        let r = unsafe { libc::poll(&mut pfd, 1, timeout_ms_ceil(rem)) };
        if r > 0 && pfd.revents != 0 {
            return true;
        }
        if r < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return pidfd_exited(pidfd);
        }
    }
}

/// Wait until EVERY pidfd in `fds` has exited or `deadline` expires.
fn wait_all_pidfds(fds: &[&OwnedFd], deadline: Instant) -> bool {
    loop {
        let mut pfds: Vec<libc::pollfd> = Vec::with_capacity(fds.len());
        for fd in fds {
            if !pidfd_exited(fd) {
                pfds.push(libc::pollfd { fd: fd.raw(), events: libc::POLLIN, revents: 0 });
            }
        }
        if pfds.is_empty() {
            return true;
        }
        let rem = deadline.saturating_duration_since(Instant::now());
        if rem.is_zero() {
            return fds.iter().all(|fd| pidfd_exited(fd));
        }
        // SAFETY: `pfds` holds valid owned pidfds.
        let r = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, timeout_ms_ceil(rem)) };
        if r < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return fds.iter().all(|fd| pidfd_exited(fd));
        }
    }
}

/// Signal ONLY the verified `pidfd` (never a numeric PID or process group,
/// Findings 1 & 11), propagating non-`ESRCH` kernel errors.
fn signal_pidfd(pidfd: &OwnedFd, sig: i32) -> io::Result<()> {
    // SAFETY: `pidfd` pins the exact session process.
    let rc = unsafe { libc::syscall(libc::SYS_pidfd_send_signal, pidfd.raw(), sig, std::ptr::null::<libc::siginfo_t>(), 0 as libc::c_uint) };
    if rc == 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ESRCH) { Ok(()) } else { Err(err) }
}

/// Acquire and RETAIN the exclusive `NAME.control.lock` inside `rt.dir_fd`,
/// failing closed on any open/metadata/`flock` error and verifying no live
/// legacy `NAME.control` viewer holds control while the lock is held
/// (Item 5 / Findings 4, 13, 17).
fn acquire_control_lock_in_rt(rt: &LockedRuntime, name: &str) -> Result<std::fs::File, GuardedError> {
    use std::ffi::CString;
    use std::os::fd::FromRawFd as _;

    // SAFETY: `geteuid()` has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let lock_name = format!("{name}.control.lock");
    let clock = CString::new(lock_name.as_str()).map_err(|_| GuardedError::Usage("invalid control lock name".into()))?;
    // SAFETY: `rt.dir_fd` is our verified owned runtime dir fd; open non-blocking without following symlinks.
    let raw = unsafe { libc::openat(rt.dir_fd.raw(), clock.as_ptr(), libc::O_CREAT | libc::O_RDWR | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC, 0o600 as libc::c_uint) };
    if raw < 0 {
        return Err(GuardedError::Other(format!("couldn't open control lock {}/{lock_name}: {}", rt.path.display(), io::Error::last_os_error())));
    }
    // SAFETY: freshly opened owned file descriptor.
    let file = unsafe { std::fs::File::from_raw_fd(raw) };
    let meta = file.metadata().map_err(|e| GuardedError::Other(format!("couldn't stat control lock {}/{lock_name}: {e}", rt.path.display())))?;
    if !meta.file_type().is_file() || meta.uid() != uid {
        return Err(GuardedError::Other(format!("refusing unsafe control lock {}/{lock_name}", rt.path.display())));
    }
    loop {
        // SAFETY: non-blocking `flock` strictly ordered after `NAME.lifecycle.lock`.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            break;
        }
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) || err.raw_os_error() == Some(libc::EAGAIN) {
            return Err(GuardedError::Other(format!("screen {name} is being controlled or acted on right now; try again in a moment")));
        }
        return Err(GuardedError::Other(format!("couldn't lock control for {name}: {err}")));
    }

    // While holding `NAME.control.lock`, inspect any legacy `NAME.control` marker inside `rt.dir_fd`.
    let ctl_name = format!("{name}.control");
    match open_regular_at(&rt.dir_fd, &ctl_name) {
        Ok((ctl_file, ctl_meta)) => {
            if ctl_meta.len() > 64 {
                return Err(GuardedError::Other(format!("refusing oversized control marker {}/{ctl_name}", rt.path.display())));
            }
            let mut bytes = Vec::new();
            ctl_file.take(65).read_to_end(&mut bytes).map_err(|e| GuardedError::Other(format!("couldn't read control marker {}/{ctl_name}: {e}", rt.path.display())))?;
            if let Some((pid, inline_ticks)) = parse_control_lock_meta(&bytes) {
                let mut sidecar = [0u8; 64];
                let side_ticks = file.read_at(&mut sidecar, 0).ok().and_then(|n| parse_control_lock_meta(&sidecar[..n])).filter(|(p, _)| *p == pid).and_then(|(_, t)| t);
                let expected_ticks = inline_ticks.or(side_ticks);
                if is_controller_pid_alive_with_ticks(pid, expected_ticks) {
                    return Err(GuardedError::Other(format!("screen {name} is being controlled right now; hand it back first")));
                }
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(GuardedError::Other(format!("couldn't inspect control marker {}/{ctl_name}: {e}", rt.path.display())));
        }
    }
    Ok(file)
}

/// Acquire and retain exclusive control locks across all locked runtime dirs
/// in canonical order (Item 5).
fn acquire_control_locks_in_rts(locked: &[LockedRuntime], name: &str) -> Result<Vec<std::fs::File>, GuardedError> {
    let mut guards = Vec::with_capacity(locked.len());
    for rt in locked {
        guards.push(acquire_control_lock_in_rt(rt, name)?);
    }
    Ok(guards)
}

/// Clean up a stale `NAME.control` marker while ALREADY holding the exclusive
/// `NAME.control.lock` fd (`stable_lock`), avoiding self-contention on `flock` (Item 5).
fn cleanup_stale_control_with_lock(rt: &LockedRuntime, name: &str, stable_lock: &std::fs::File) {
    use std::ffi::CString;
    use std::os::fd::FromRawFd as _;

    let ctl_name = format!("{name}.control");
    let Ok(cctl) = CString::new(ctl_name.as_str()) else { return };
    // SAFETY: valid owned runtime dir fd and validated component.
    let raw = unsafe { libc::openat(rt.dir_fd.raw(), cctl.as_ptr(), libc::O_RDWR | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC) };
    if raw < 0 {
        return;
    }
    let file = unsafe { std::fs::File::from_raw_fd(raw) };
    let Ok(fm) = file.metadata() else { return };
    if !fm.file_type().is_file() {
        return;
    }
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return;
    }
    let mut bytes = Vec::new();
    let live = file
        .try_clone()
        .ok()
        .and_then(|f| {
            let _ = f.take(65).read_to_end(&mut bytes);
            parse_control_lock_meta(&bytes)
        })
        .is_some_and(|(pid, inline_ticks)| {
            let mut sidecar = [0u8; 64];
            let side_ticks = stable_lock.read_at(&mut sidecar, 0).ok().and_then(|n| parse_control_lock_meta(&sidecar[..n])).filter(|(p, _)| *p == pid).and_then(|(_, t)| t);
            is_controller_pid_alive_with_ticks(pid, inline_ticks.or(side_ticks))
        });
    if !live && stat_at_nofollow(&rt.dir_fd, &ctl_name).is_ok_and(|id| id == (fm.dev(), fm.ino())) {
        unlink_at_ignore_enoent(&rt.dir_fd, &ctl_name);
        let _ = stable_lock.set_len(0);
    }
    unsafe {
        libc::flock(file.as_raw_fd(), libc::LOCK_UN);
    }
}

/// Query a screen compositor's window PIDs speaking the REAL newline-delimited
/// `olo_protocol` JSON wire format (`{"type":"list_windows"}\n` -> top-level
/// `"windows"` array, skipping any interleaved `{"type":"event",...}` lines),
/// bounded strictly by a single absolute `deadline` and capped reply bytes
/// (Items 3 & 6 / Finding 13).
fn query_compositor_window_pids_deadline(olo_socket: &Path, deadline: Instant) -> Option<Vec<u32>> {
    if Instant::now() >= deadline {
        return None;
    }
    let stream = connect_unix_nonblocking_deadline(olo_socket, deadline, None).ok()?;
    write_all_unix_deadline(&stream, b"{\"type\":\"list_windows\"}\n", deadline).ok()?;
    for _ in 0..64 {
        let line = read_line_unix_deadline(&stream, 2 * 1024 * 1024, deadline).ok()?;
        let reply: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        if reply.get("type").and_then(|t| t.as_str()) == Some("event") {
            continue;
        }
        let windows = reply.get("windows").and_then(|w| w.as_array())?;
        let mut pids: Vec<u32> = windows.iter().filter_map(|w| w.get("pid").and_then(|p| p.as_i64())).filter(|&p| p > 0).map(|p| p as u32).collect();
        pids.sort_unstable();
        pids.dedup();
        return Some(pids);
    }
    None
}

/// Mirror `crates/screen-pane/src/moving.rs::send_home` (Items 3 & 6 / Finding 13):
/// - Uses ONE absolute `deadline` across ALL sockets, non-blocking `connect`s,
///   writes, bounded reply reads, and arrival polling.
/// - Sends `home <proxied>\n` to live `olo-wlproxy-*.sock`.
/// - Queries `s.olo_socket` with newline-delimited `{"type":"list_windows"}\n`
///   reading top-level `"windows"`, reads each PID's `OLO_HOME` JSON `MoveTo`,
///   sends `{"move": <MoveTo>}\n` to `olo-menus-PID.sock`, and waits up to
///   `deadline` for acknowledged windows to leave the screen.
fn send_home_best_effort_until(rt_parent: &Path, s: &RuntimeScreen, deadline: Instant) {
    if Instant::now() >= deadline {
        return;
    }
    let display_leaf = Path::new(&s.wayland_display).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !display_leaf.is_empty()
        && let Ok(rd) = std::fs::read_dir(rt_parent)
    {
        let proxied_new = rt_parent.join(format!("olo-{display_leaf}-proxied"));
        let proxied_old = rt_parent.join(format!("tabula-{display_leaf}-proxied"));
        let proxied = if proxied_new.exists() || !proxied_old.exists() { proxied_new } else { proxied_old };
        let cmd = format!("home {}\n", proxied.display());
        let mut socks: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.strip_prefix("olo-wlproxy-").and_then(|n| n.strip_suffix(".sock")).and_then(|p| p.parse::<u32>().ok()).is_some_and(|pid| Path::new(&format!("/proc/{pid}")).exists())
            })
            .map(|e| e.path())
            .collect();
        socks.sort();
        for sock in socks {
            if Instant::now() >= deadline {
                return;
            }
            if let Ok(stream) = connect_unix_nonblocking_deadline(&sock, deadline, None)
                && write_all_unix_deadline(&stream, cmd.as_bytes(), deadline).is_ok()
            {
                let _ = read_line_unix_deadline(&stream, 4096, deadline);
            }
        }
    }

    if Instant::now() >= deadline {
        return;
    }
    let Some(pids) = query_compositor_window_pids_deadline(&s.olo_socket, deadline) else {
        return;
    };
    let mut moved_pids: Vec<u32> = Vec::new();
    for pid in pids {
        if Instant::now() >= deadline {
            return;
        }
        let Ok(env_bytes) = std::fs::read(format!("/proc/{pid}/environ")) else { continue };
        let Some(home_raw) = env_bytes.split(|b| *b == 0).find_map(|kv| kv.strip_prefix(b"OLO_HOME=")) else {
            continue;
        };
        let Ok(home_val) = serde_json::from_slice::<serde_json::Value>(home_raw) else {
            continue;
        };
        if !home_val.is_object() {
            continue;
        }
        let Ok(mut payload) = serde_json::to_vec(&json!({ "move": home_val })) else {
            continue;
        };
        payload.push(b'\n');
        let menus_sock = rt_parent.join(format!("olo-menus-{pid}.sock"));
        if let Ok(stream) = connect_unix_nonblocking_deadline(&menus_sock, deadline, None)
            && write_all_unix_deadline(&stream, &payload, deadline).is_ok()
            && let Ok(reply_line) = read_line_unix_deadline(&stream, 16 * 1024, deadline)
            && serde_json::from_str::<serde_json::Value>(reply_line.trim()).is_ok_and(|v| v.get("ok").and_then(|o| o.as_bool()) == Some(true))
        {
            moved_pids.push(pid);
        }
    }

    // Allow acknowledged kit moves to finish leaving the screen before signaling,
    // strictly within the same overall `deadline`.
    while !moved_pids.is_empty() && Instant::now() < deadline {
        let Some(left) = query_compositor_window_pids_deadline(&s.olo_socket, deadline) else {
            break;
        };
        if !left.iter().any(|p| moved_pids.contains(p)) {
            break;
        }
        let rem = deadline.saturating_duration_since(Instant::now());
        if rem.is_zero() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50).min(rem));
    }
}

fn send_home_best_effort(rt_parent: &Path, s: &RuntimeScreen) {
    send_home_best_effort_until(rt_parent, s, Instant::now() + Duration::from_secs(3));
}

/// Parse `(state, ppid, start_ticks)` from `/proc/<pid>/stat`.
fn proc_state_ppid_start(pid: i32) -> Option<(u8, i32, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, rest) = stat.rsplit_once(')')?;
    let mut fields = rest.split_whitespace();
    let state = fields.next()?.as_bytes().first().copied()?;
    let ppid = fields.next()?.parse::<i32>().ok()?;
    let start_ticks = fields.nth(17)?.parse::<u64>().ok()?;
    Some((state, ppid, start_ticks))
}

/// Snapshot and pin via `pidfd_open` every live verified descendant process of
/// `session_pid` (whose start ticks match `session_start_ticks`) BEFORE sending
/// `SIGKILL` to a wedged/suspended session supervisor (Item 4: prevents forced
/// Stop from leaving a live compositor behind and trashing an active profile).
fn collect_verified_session_descendants(session_pid: i32, session_start_ticks: u64, name: &str, rt_dir_fd: &OwnedFd, session_pidfd: &OwnedFd) -> Result<Vec<(i32, OwnedFd)>, String> {
    let rd = std::fs::read_dir("/proc").map_err(|e| format!("couldn't scan /proc to verify session children: {e}"))?;
    let mut proc_map: std::collections::HashMap<i32, (u8, i32, u64)> = std::collections::HashMap::new();
    for entry in rd.flatten() {
        let Ok(s) = entry.file_name().into_string() else { continue };
        let Ok(pid) = s.parse::<i32>() else { continue };
        if pid <= 1 {
            continue;
        }
        if let Some(info) = proc_state_ppid_start(pid) {
            proc_map.insert(pid, info);
        }
    }
    if pidfd_exited(session_pidfd) {
        return Ok(Vec::new());
    }
    if proc_map.get(&session_pid).is_none_or(|&(_, _, st)| st != session_start_ticks) {
        return Err(format!("session pid {session_pid} identity changed while inspecting children"));
    }

    let is_descendant = |pid: i32, map: &std::collections::HashMap<i32, (u8, i32, u64)>| -> bool {
        let mut cur = pid;
        for _ in 0..64 {
            let Some(&(_, ppid, start)) = map.get(&cur) else { return false };
            if ppid == session_pid {
                return start >= session_start_ticks;
            }
            if ppid <= 1 || ppid == cur {
                return false;
            }
            cur = ppid;
        }
        false
    };

    let rt_id = fstat_dev_ino(rt_dir_fd).ok();
    let mut pinned = Vec::new();
    for (&cpid, &(state, _, start_ticks)) in &proc_map {
        if cpid == session_pid || matches!(state, b'Z' | b'X' | b'x') || !is_descendant(cpid, &proc_map) {
            continue;
        }
        let cfd = match pidfd_open_result(cpid) {
            Ok(fd) => fd,
            Err(e) if e.raw_os_error() == Some(libc::ESRCH) => continue,
            Err(e) => return Err(format!("couldn't pin session child pid {cpid}: {e}")),
        };
        // Re-verify child identity AFTER pinning via pidfd (no PID reuse).
        let Some((state2, _, start2)) = proc_state_ppid_start(cpid) else {
            continue;
        };
        if matches!(state2, b'Z' | b'X' | b'x') || start2 != start_ticks || pidfd_exited(&cfd) {
            continue;
        }
        if let Ok(env_bytes) = std::fs::read(format!("/proc/{cpid}/environ")) {
            if let Some(scr) = env_bytes.split(|b| *b == 0).find_map(|v| v.strip_prefix(b"OLO_SCREEN="))
                && scr != name.as_bytes()
            {
                continue;
            }
            if let Some(rt_dev_ino) = rt_id
                && let Some(sv) = env_bytes.split(|b| *b == 0).find_map(|v| v.strip_prefix(b"OLO_APP_SERVER_SETTINGS="))
                && !sv.is_empty()
            {
                use std::os::unix::ffi::OsStrExt as _;
                let sp = Path::new(std::ffi::OsStr::from_bytes(sv));
                if sp.parent().and_then(|p| std::fs::metadata(p).ok()).is_some_and(|m| (m.dev(), m.ino()) != rt_dev_ino) {
                    continue;
                }
            }
        }
        if pidfd_exited(session_pidfd) {
            break;
        }
        pinned.push((cpid, cfd));
    }
    Ok(pinned)
}

/// Unlink `<NAME>-settings.json` inside `rt.dir_fd` ONLY after verifying it is
/// a genuine settings file and NOT another screen's runtime descriptor (such as
/// screen `qa-settings`'s descriptor `qa-settings.json` carrying `olo_socket` /
/// `pid` — Item 2).
fn unlink_screen_settings_if_not_descriptor(rt: &LockedRuntime, name: &str) {
    use std::ffi::CString;
    use std::os::fd::FromRawFd as _;

    let settings_name = format!("{name}-settings.json");
    // If `<NAME>-settings.lifecycle.lock` already exists, take a non-blocking
    // exclusive probe so we never race with an active Start of `<NAME>-settings`.
    let other_lock_name = format!("{name}-settings.lifecycle.lock");
    let _other_lock: Option<std::fs::File> = if let Ok(c) = CString::new(other_lock_name.as_str()) {
        let raw = unsafe { libc::openat(rt.dir_fd.raw(), c.as_ptr(), libc::O_RDWR | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC) };
        if raw >= 0 {
            let f = unsafe { std::fs::File::from_raw_fd(raw) };
            if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                // `<NAME>-settings` is actively mutating: preserve `<NAME>-settings.json`!
                return;
            }
            Some(f)
        } else if io::Error::last_os_error().kind() == io::ErrorKind::NotFound {
            None
        } else {
            return;
        }
    } else {
        return;
    };

    let (file, meta) = match open_regular_at(&rt.dir_fd, &settings_name) {
        Ok(v) => v,
        Err(_) => return,
    };
    if meta.len() > 256 * 1024 {
        return;
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    if file.take(256 * 1024 + 1).read_to_end(&mut bytes).is_err() {
        return;
    }
    let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return;
    };
    let Some(obj) = val.as_object() else {
        return;
    };
    // Any runtime screen descriptor field marks this file as belonging to screen
    // `<NAME>-settings` rather than `<NAME>`'s app_server settings: preserve it!
    if obj.contains_key("olo_socket") || obj.contains_key("tabula_socket") || obj.contains_key("pid") || obj.contains_key("wayland_display") || obj.contains_key("name") {
        return;
    }
    if stat_at_nofollow(&rt.dir_fd, &settings_name).is_ok_and(|id| id == (meta.dev(), meta.ino())) {
        unlink_at_ignore_enoent(&rt.dir_fd, &settings_name);
    }
}

/// Record a successful guarded `stop` in `actions.jsonl` while still holding
/// `NAME.lifecycle.lock`, using the ALREADY-PINNED `pinned_state_dir` fd (never
/// reopening by name after signaling — Item 7), opening `actions.jsonl`
/// non-blocking and checking `is_file()` (FIFO-safe — Item 6), and prepending
/// `\n` when an existing non-empty file lacks a trailing newline (Item 7).
/// Returns any recording failure explicitly so the caller can surface it
/// separately from the completed process stop (Item 6).
fn record_stop_action_locked(pinned_state_dir: Option<&OwnedFd>, state_root_fd: Option<&OwnedFd>, state_root: &Path, name: &str, at_ms: u64, elapsed_ms: u64) -> Result<(), String> {
    use std::ffi::CString;
    use std::os::fd::FromRawFd as _;

    let owned_dir_fd: Option<OwnedFd>;
    let dir_fd: &OwnedFd = if let Some(dfd) = pinned_state_dir {
        dfd
    } else {
        let owned_root_fd: Option<OwnedFd>;
        let rfd: &OwnedFd = if let Some(r) = state_root_fd {
            r
        } else {
            std::fs::create_dir_all(state_root).map_err(|e| format!("couldn't create screens state folder: {e}"))?;
            owned_root_fd = Some(open_verified_owned_dir(state_root, false).map_err(|e| format!("couldn't open screens state folder: {e}"))?);
            owned_root_fd.as_ref().unwrap()
        };
        let cname = CString::new(name).map_err(|_| format!("invalid screen name {name:?}"))?;
        // SAFETY: `rfd` is our verified owned state root fd.
        unsafe {
            libc::mkdirat(rfd.raw(), cname.as_ptr(), 0o755);
        }
        let raw = unsafe { libc::openat(rfd.raw(), cname.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if raw < 0 {
            return Err(format!("couldn't open screen folder {name:?}: {}", io::Error::last_os_error()));
        }
        owned_dir_fd = Some(OwnedFd::new(raw));
        owned_dir_fd.as_ref().unwrap()
    };

    let cact = c"actions.jsonl";
    // Item 6: open `O_RDWR | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC` so a FIFO never blocks
    // under `NAME.lifecycle.lock`, and we can check the trailing byte for `\n` (Item 7).
    // SAFETY: `dir_fd` is our verified non-symlink screen directory fd.
    let raw = unsafe { libc::openat(dir_fd.raw(), cact.as_ptr(), libc::O_RDWR | libc::O_CREAT | libc::O_APPEND | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC, 0o644 as libc::c_uint) };
    if raw < 0 {
        return Err(format!("couldn't open actions.jsonl: {}", io::Error::last_os_error()));
    }
    // SAFETY: freshly opened owned fd.
    let mut f = unsafe { std::fs::File::from_raw_fd(raw) };
    let meta = f.metadata().map_err(|e| format!("couldn't stat actions.jsonl: {e}"))?;
    if !meta.file_type().is_file() {
        return Err("actions.jsonl is not a regular file".into());
    }
    let mut needs_leading_nl = false;
    let len = meta.len();
    if len > 0 {
        let mut last = [0u8; 1];
        f.read_exact_at(&mut last, len - 1).map_err(|e| format!("couldn't inspect actions.jsonl tail: {e}"))?;
        needs_leading_nl = last[0] != b'\n';
    }
    let line = json!({ "at": at_ms, "ms": elapsed_ms, "kind": "stop", "args": [] });
    let mut payload = Vec::with_capacity(96);
    if needs_leading_nl {
        payload.push(b'\n');
    }
    serde_json::to_writer(&mut payload, &line).map_err(|e| format!("couldn't encode stop action: {e}"))?;
    payload.push(b'\n');
    f.write_all(&payload).map_err(|e| format!("couldn't append stop action: {e}"))?;
    Ok(())
}

fn stop_term_wait() -> Duration {
    if allow_test_screen_caps()
        && let Some(ms) = std::env::var("OLO_SCREEN_STOP_TERM_MS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|&ms| ms >= 50)
    {
        return Duration::from_millis(ms);
    }
    Duration::from_secs(5)
}

struct StopStateTarget<'a> {
    pinned_dir: Option<&'a OwnedFd>,
    root_fd: Option<&'a OwnedFd>,
    root_path: &'a Path,
}

/// Escalate a wedged/suspended session supervisor after `SIGTERM` times out:
/// inspect `/proc/<pid>/stat` BEFORE sending `SIGSTOP` (to preserve prior `T`/`t`
/// state and recover `session_start_ticks` if missing), fail closed if start
/// ticks remain unavailable while the supervisor is still alive, send `SIGCONT`
/// on descendant-scan error if WE stopped the supervisor, then terminate both
/// supervisor and verified descendants via pidfd (Item 4 + agy-review F5).
fn force_stop_supervisor_and_descendants(
    name: &str,
    session_pid: i32,
    mut session_start_ticks: Option<u64>,
    pidfd: &OwnedFd,
    read_stat: impl FnOnce(i32) -> Option<(u8, i32, u64)>,
    mut collect_descendants: impl FnMut(u64) -> Result<Vec<(i32, OwnedFd)>, String>,
) -> Result<(), GuardedError> {
    let pre_stat = read_stat(session_pid);
    if session_start_ticks.is_none() {
        session_start_ticks = pre_stat.map(|(_, _, ticks)| ticks);
    }
    if pidfd_exited(pidfd) {
        return Ok(());
    }
    let Some(ticks) = session_start_ticks else {
        return Err(GuardedError::Other(format!("screen {name} forced stop incomplete (couldn't read session start time to verify children)")));
    };
    let was_stopped = pre_stat.is_some_and(|(st, _, _)| matches!(st, b'T' | b't'));
    let sent_sigstop = !was_stopped && signal_pidfd(pidfd, libc::SIGSTOP).is_ok();
    let descendants = match collect_descendants(ticks) {
        Ok(list) => list,
        Err(why) => {
            if sent_sigstop {
                let _ = signal_pidfd(pidfd, libc::SIGCONT);
            }
            return Err(GuardedError::Other(format!("screen {name} forced stop incomplete ({why})")));
        }
    };
    signal_pidfd(pidfd, libc::SIGKILL).map_err(|e| GuardedError::Other(format!("couldn't end screen {name}: {e}")))?;
    for (_, cfd) in &descendants {
        let _ = signal_pidfd(cfd, libc::SIGTERM);
    }
    let child_refs: Vec<&OwnedFd> = descendants.iter().map(|(_, fd)| fd).collect();
    if !child_refs.is_empty() && !wait_all_pidfds(&child_refs, Instant::now() + Duration::from_millis(400)) {
        for (_, cfd) in &descendants {
            if !pidfd_exited(cfd) {
                signal_pidfd(cfd, libc::SIGKILL).map_err(|e| GuardedError::Other(format!("couldn't end screen {name} child process: {e}")))?;
            }
        }
    }
    let mut all_fds: Vec<&OwnedFd> = Vec::with_capacity(1 + child_refs.len());
    all_fds.push(pidfd);
    all_fds.extend(child_refs);
    let _ = wait_all_pidfds(&all_fds, Instant::now() + Duration::from_secs(2));
    if !pidfd_exited(pidfd) || descendants.iter().any(|(_, fd)| !pidfd_exited(fd)) {
        return Err(GuardedError::Other(format!("screen {name} is still ending (session or child processes remain alive)")));
    }
    Ok(())
}

fn stop_screen_in_process(rt: &LockedRuntime, state: StopStateTarget<'_>, name: &str, s: &RuntimeScreen, pidfd: &OwnedFd, control_lock: &std::fs::File) -> Result<Option<String>, GuardedError> {
    let began_ms = now_ms();
    let session_start_ticks = proc_start_ticks(s.pid);

    // Items 3, 5 & 6: return moved windows home while RETAINING `control_lock`
    // and under ONE single absolute deadline (non-blocking connect + bounded reads).
    if let Some(rt_parent) = rt.path.parent() {
        send_home_best_effort(rt_parent, s);
    }

    // Findings 1 & 11: signal ONLY the pinned session pidfd first (clean shutdown).
    signal_pidfd(pidfd, libc::SIGTERM).map_err(|e| GuardedError::Other(format!("couldn't signal screen {name}: {e}")))?;
    if !wait_pidfd(pidfd, stop_term_wait()) {
        force_stop_supervisor_and_descendants(name, s.pid, session_start_ticks, pidfd, proc_state_ppid_start, |ticks| collect_verified_session_descendants(s.pid, ticks, name, &rt.dir_fd, pidfd))?;
    }
    if !pidfd_exited(pidfd) {
        return Err(GuardedError::Other(format!("screen {name} is still ending")));
    }

    // Items 2, 5, 9 & 14: unlink ONLY inside the pinned runtime directory fd,
    // preserving `<NAME>-settings.json` when it is another screen's descriptor.
    unlink_at_ignore_enoent(&rt.dir_fd, &format!("{name}.json"));
    unlink_screen_settings_if_not_descriptor(rt, name);
    unlink_at_ignore_enoent(&rt.dir_fd, &format!("{name}.owner.json"));
    if let Ok(views_fd) = open_verified_subdir(&rt.path, "views") {
        unlink_at_ignore_enoent(&views_fd, &format!("record-{name}.rgba"));
    }
    cleanup_stale_control_with_lock(rt, name, control_lock);

    // Items 6 & 7: record `"stop"` in History/Stats via pinned state dir fd while
    // still holding `NAME.lifecycle.lock`, surfacing any recording error separately.
    let rec_err = record_stop_action_locked(state.pinned_dir, state.root_fd, state.root_path, name, began_ms, now_ms().saturating_sub(began_ms)).err();
    Ok(rec_err)
}

fn xdg_trash_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(".local/share")).join("Trash")
}

fn percent_encode_trash_path(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt as _;
    let mut out = String::new();
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Local-time Freedesktop Trash `DeletionDate` (`YYYY-MM-DDTHH:MM:SS`),
/// matching `crates/xdg/src/trash.rs::format_iso_now` without writing the
/// musl-deprecated `libc::time_t` type alias (Finding 27).
fn deletion_date_now() -> String {
    // SAFETY: `libc::time(null)` returns current epoch seconds in the platform's
    // native time integer type; `localtime_r` writes into our stack `tm`.
    let (ok, tm) = unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        let ok = !libc::localtime_r(&t, &mut tm).is_null();
        (ok, tm)
    };
    if ok { format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec) } else { "1970-01-01T00:00:00".to_owned() }
}

/// Linux `renameat2` `RENAME_NOREPLACE` flag (kernel UAPI constant = 1).
const RENAME_NOREPLACE: libc::c_uint = 1;

fn stat_at_nofollow(dir_fd: &OwnedFd, name: &str) -> io::Result<(u64, u64)> {
    let c = std::ffi::CString::new(name).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in name"))?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: valid dirfd, NUL-terminated component, valid stat out-pointer.
    if unsafe { libc::fstatat(dir_fd.raw(), c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((st.st_dev as u64, st.st_ino as u64))
}

fn renameat2_noreplace(old_dir: &OwnedFd, old_name: &str, new_dir: &OwnedFd, new_name: &str) -> io::Result<()> {
    let cold = std::ffi::CString::new(old_name).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in old name"))?;
    let cnew = std::ffi::CString::new(new_name).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in new name"))?;
    // SAFETY: valid pinned directory fds and NUL-terminated single filename components.
    let rc = unsafe { libc::syscall(libc::SYS_renameat2, old_dir.raw(), cold.as_ptr(), new_dir.raw(), cnew.as_ptr(), RENAME_NOREPLACE) };
    if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Unlink `info_name` inside `info_fd` ONLY if it still has our expected inode
/// AND exact `expected_body` (avoiding clobbering a `.trashinfo` concurrently
/// overwritten by another writer).
fn unlink_own_trashinfo(info_fd: &OwnedFd, info_name: &str, expected_id: (u64, u64), expected_body: &[u8]) {
    let Ok((f, m)) = open_regular_at(info_fd, info_name) else { return };
    if (m.dev(), m.ino()) != expected_id || m.len() != expected_body.len() as u64 {
        return;
    }
    let mut cur = Vec::with_capacity(expected_body.len());
    if f.take(expected_body.len() as u64 + 1).read_to_end(&mut cur).is_ok() && cur == expected_body {
        unlink_at_ignore_enoent(info_fd, info_name);
    }
}

/// Move verified screen directory `state_root_fd/name` (pinned at `expected_dev_ino`)
/// into Freedesktop Trash via two-step staging + same-volume `renameat2(RENAME_NOREPLACE)`
/// (Item 8 / Findings 2, 3, 14 & 27):
/// 1. Atomically move `state_root_fd/name` into a private staging name inside the
///    same pinned `state_root_fd` and verify the STAGED inode matches `expected_dev_ino`.
///    On mismatch, restore via `RENAME_NOREPLACE` (retaining both entries if `name`
///    is already occupied) and return `Stale` without trashing the replacement!
/// 2. Publish `stage_name` -> `Trash/files/<stem>` via `RENAME_NOREPLACE` only after
///    identity is proved. On `EXDEV` or fatal error, restore `stage_name` -> `name`
///    via `RENAME_NOREPLACE`, unlink only our unmodified `.trashinfo`, and return error.
fn trash_screen_pinned_with_hooks(
    state_root: &Path,
    state_root_fd: &OwnedFd,
    expected_dev_ino: (u64, u64),
    trash_root: &Path,
    name: &str,
    mut before_stage: impl FnMut(),
    mut before_publish: impl FnMut(&str) -> Option<io::Error>,
) -> Result<(), GuardedError> {
    use std::ffi::CString;
    use std::os::fd::FromRawFd as _;
    static STAGE_SEQ: AtomicU64 = AtomicU64::new(0);

    let screen_dir = state_root.join(name);
    let files_dir = trash_root.join("files");
    let info_dir = trash_root.join("info");
    let files_fd = open_verified_owned_dir(&files_dir, true).map_err(|e| GuardedError::Other(format!("{}: {e}", files_dir.display())))?;
    let info_fd = open_verified_owned_dir(&info_dir, true).map_err(|e| GuardedError::Other(format!("{}: {e}", info_dir.display())))?;

    // Step 1: Move `state_root_fd/name` into a private staging name in the same
    // pinned parent directory, then verify the STAGED inode matches `expected_dev_ino`.
    before_stage();
    let pid = std::process::id();
    let stage_name = loop {
        let seq = STAGE_SEQ.fetch_add(1, Ordering::Relaxed);
        let cand = format!(".delete-stage-{name}-{pid}-{seq}");
        match renameat2_noreplace(state_root_fd, name, state_root_fd, &cand) {
            Ok(()) => break cand,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists || e.raw_os_error() == Some(libc::EEXIST) || e.raw_os_error() == Some(libc::ENOTEMPTY) => continue,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(GuardedError::Stale(format!("{name} was removed since you asked")));
            }
            Err(e) => {
                return Err(GuardedError::Other(format!("couldn't stage {name:?} for Trash: {e}")));
            }
        }
    };

    // Helper: restore staged dir back to `name` via `RENAME_NOREPLACE` (retaining
    // both if another entry already occupies `name`).
    let restore_staged = || {
        let _ = renameat2_noreplace(state_root_fd, &stage_name, state_root_fd, name);
    };

    match stat_at_nofollow(state_root_fd, &stage_name) {
        Ok(staged_id) if staged_id == expected_dev_ino => {}
        _ => {
            restore_staged();
            return Err(GuardedError::Stale(format!("{name} was started again since you asked")));
        }
    }

    // Step 2: Now `stage_name` is proved to be the exact pinned inode and hidden
    // from other callers under `name`; reserve `.trashinfo` and publish to Trash.
    let encoded_path = percent_encode_trash_path(&screen_dir);
    let date = deletion_date_now();
    let info_body = format!("[Trash Info]\nPath={encoded_path}\nDeletionDate={date}\n");

    for n in 1..=10_000u32 {
        let stem = if n == 1 { name.to_owned() } else { format!("{name}.{n}") };
        let info_name = format!("{stem}.trashinfo");
        if exists_at(&files_fd, &stem) {
            continue;
        }
        let cinfo = CString::new(info_name.as_str()).map_err(|_| {
            restore_staged();
            GuardedError::Other("invalid trashinfo name".into())
        })?;
        // SAFETY: `info_fd` is our verified owned directory fd; `O_CREAT | O_EXCL | O_NOFOLLOW` reserves exclusively.
        let raw = unsafe { libc::openat(info_fd.raw(), cinfo.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o600 as libc::c_uint) };
        if raw < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::AlreadyExists {
                continue;
            }
            restore_staged();
            return Err(GuardedError::Other(format!("{}/{info_name}: {err}", info_dir.display())));
        }
        // SAFETY: freshly created exclusively owned file descriptor.
        let mut info_file = unsafe { std::fs::File::from_raw_fd(raw) };
        let info_id = info_file.metadata().map(|m| (m.dev(), m.ino())).unwrap_or((0, 0));
        if let Err(e) = info_file.write_all(info_body.as_bytes()).and_then(|()| info_file.sync_all()) {
            drop(info_file);
            unlink_at_ignore_enoent(&info_fd, &info_name);
            restore_staged();
            return Err(GuardedError::Other(format!("{}/{info_name}: {e}", info_dir.display())));
        }
        drop(info_file);

        let rename_res = match before_publish(&stem) {
            Some(injected_err) => Err(injected_err),
            None => renameat2_noreplace(state_root_fd, &stage_name, &files_fd, &stem),
        };
        match rename_res {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists || e.raw_os_error() == Some(libc::EEXIST) || e.raw_os_error() == Some(libc::ENOTEMPTY) => {
                // Remove ONLY our own unmodified `.trashinfo` (never touch `Trash/files/<stem>` or another writer's info) and try next slot.
                unlink_own_trashinfo(&info_fd, &info_name, info_id, info_body.as_bytes());
                continue;
            }
            Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
                unlink_own_trashinfo(&info_fd, &info_name, info_id, info_body.as_bytes());
                restore_staged();
                return Err(GuardedError::Other(format!("couldn't move {name:?} to Trash across filesystems (source left intact)")));
            }
            Err(e) => {
                unlink_own_trashinfo(&info_fd, &info_name, info_id, info_body.as_bytes());
                restore_staged();
                return Err(GuardedError::Other(format!("couldn't move {name:?} to Trash: {e}")));
            }
        }
    }
    restore_staged();
    Err(GuardedError::Other("couldn't reserve a Trash entry name".into()))
}

fn actions_meta_at(dir_fd: &OwnedFd, file_name: &str) -> Result<Option<ActionsMetaSnap>, GuardedError> {
    match open_regular_at(dir_fd, file_name) {
        Ok((_f, m)) => Ok(Some((m.dev(), m.ino(), m.len(), m.mtime(), m.mtime_nsec()))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(GuardedError::Other(format!("couldn't verify {file_name}: {e}"))),
    }
}

/// Stage `frames-*.olof` and `actions.jsonl` into a private temporary subdirectory
/// inside `state_dir_fd` BEFORE unlinking anything, rolling back EVERY staged
/// file on any error or `actions.jsonl` mismatch so Clear NEVER returns exit `75`
/// or exit `1` after already deleting recordings (Item 5 / Findings 4, 14 & 17).
fn clear_history_pinned_with_hook(state_dir_fd: &OwnedFd, name: &str, expected_actions_meta: Option<Option<ActionsMetaSnap>>, before_commit: impl FnOnce()) -> Result<(), GuardedError> {
    use std::ffi::{CStr, CString};
    static CLEAR_STAGE_SEQ: AtomicU64 = AtomicU64::new(0);
    const BATCH_LIMIT: usize = 256;

    let pid = std::process::id();
    let (stage_name, stage_cname, stage_fd) = loop {
        let seq = CLEAR_STAGE_SEQ.fetch_add(1, Ordering::Relaxed);
        let cand = format!(".clear-stage-{pid}-{seq}");
        let ccand = CString::new(cand.as_str()).map_err(|_| GuardedError::Other("invalid clear stage name".into()))?;
        // SAFETY: create private staging dir exclusively inside pinned `state_dir_fd`.
        if unsafe { libc::mkdirat(state_dir_fd.raw(), ccand.as_ptr(), 0o700) } != 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::AlreadyExists {
                continue;
            }
            return Err(GuardedError::Other(format!("couldn't stage history clear for {name:?}: {err}")));
        }
        let raw = unsafe { libc::openat(state_dir_fd.raw(), ccand.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if raw < 0 {
            let err = io::Error::last_os_error();
            unsafe { libc::unlinkat(state_dir_fd.raw(), ccand.as_ptr(), libc::AT_REMOVEDIR) };
            return Err(GuardedError::Other(format!("couldn't open clear stage dir for {name:?}: {err}")));
        }
        break (cand, ccand, OwnedFd::new(raw));
    };

    let mut staged_frames: Vec<String> = Vec::new();
    let mut staged_actions = false;

    let rollback = |frames: &[String], actions_moved: bool| {
        if actions_moved {
            let _ = renameat2_noreplace(&stage_fd, "actions.jsonl", state_dir_fd, "actions.jsonl");
        }
        for fname in frames {
            let _ = renameat2_noreplace(&stage_fd, fname, state_dir_fd, fname);
        }
        unsafe {
            libc::unlinkat(state_dir_fd.raw(), stage_cname.as_ptr(), libc::AT_REMOVEDIR);
        }
    };

    // Phase 1: Move all `frames-*.olof` into `stage_fd` (never deleting yet!).
    loop {
        // SAFETY: fresh open file description on pinned `state_dir_fd` so btrfs sees all current entries.
        let fresh = unsafe { libc::openat(state_dir_fd.raw(), c".".as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
        if fresh < 0 {
            let err = io::Error::last_os_error();
            rollback(&staged_frames, false);
            return Err(GuardedError::Other(format!("couldn't list screen {name:?}: {err}")));
        }
        // SAFETY: `fresh` is a valid directory fd now owned by `dp`.
        let dp = unsafe { libc::fdopendir(fresh) };
        if dp.is_null() {
            let err = io::Error::last_os_error();
            unsafe { libc::close(fresh) };
            rollback(&staged_frames, false);
            return Err(GuardedError::Other(format!("couldn't list screen {name:?}: {err}")));
        }
        let mut batch: Vec<String> = Vec::new();
        let mut hit_limit = false;
        let mut readdir_err: Option<io::Error> = None;
        loop {
            // SAFETY: clear thread-local `errno` before `readdir` to distinguish errors from EOF (Finding 17).
            unsafe {
                *libc::__errno_location() = 0;
            }
            // SAFETY: `dp` is a valid open `DIR*`.
            let ent = unsafe { libc::readdir(dp) };
            if ent.is_null() {
                let err_code = unsafe { *libc::__errno_location() };
                if err_code != 0 {
                    readdir_err = Some(io::Error::from_raw_os_error(err_code));
                }
                break;
            }
            // SAFETY: `d_name` is NUL-terminated.
            let cname = unsafe { CStr::from_ptr((*ent).d_name.as_ptr()) };
            let b = cname.to_bytes();
            if b.starts_with(b"frames-") && b.ends_with(b".olof") {
                let Ok(s) = std::str::from_utf8(b) else {
                    readdir_err = Some(io::Error::new(io::ErrorKind::InvalidData, "non-UTF8 frame filename"));
                    break;
                };
                batch.push(s.to_owned());
                if batch.len() >= BATCH_LIMIT {
                    hit_limit = true;
                    break;
                }
            }
        }
        // SAFETY: closes `dp` and `fresh`.
        unsafe { libc::closedir(dp) };
        if let Some(err) = readdir_err {
            rollback(&staged_frames, false);
            return Err(GuardedError::Other(format!("couldn't list screen {name:?}: {err}")));
        }
        if batch.is_empty() {
            break;
        }
        for entry in batch {
            // Reject subdirectories pretending to be `frames-*.olof` (preserves EISDIR behavior & rolls back!).
            let Ok(centry) = CString::new(entry.as_str()) else {
                rollback(&staged_frames, false);
                return Err(GuardedError::Other(format!("invalid frame entry {entry:?}")));
            };
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            if unsafe { libc::fstatat(state_dir_fd.raw(), centry.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::NotFound {
                    continue;
                }
                rollback(&staged_frames, false);
                return Err(GuardedError::Other(format!("couldn't clear {entry}: {err}")));
            }
            if (st.st_mode & libc::S_IFMT) == libc::S_IFDIR {
                rollback(&staged_frames, false);
                return Err(GuardedError::Other(format!("couldn't clear {entry}: Is a directory")));
            }
            match renameat2_noreplace(state_dir_fd, &entry, &stage_fd, &entry) {
                Ok(()) => staged_frames.push(entry),
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => {
                    rollback(&staged_frames, false);
                    return Err(GuardedError::Other(format!("couldn't stage {entry} into {stage_name}: {e}")));
                }
            }
        }
        if !hit_limit {
            break;
        }
    }

    before_commit();

    // Phase 2: Verify `actions.jsonl` has not changed BEFORE moving it, then move
    // `actions.jsonl` into `stage_fd` and re-verify it AFTER moving it; roll back
    // ALL staged frames (and `actions.jsonl`) on any mismatch so Clear NEVER exits
    // 75 after deleting recordings!
    if let Some(want_meta) = expected_actions_meta {
        match actions_meta_at(state_dir_fd, "actions.jsonl") {
            Ok(cur) if cur == want_meta => {}
            Ok(_) => {
                rollback(&staged_frames, false);
                return Err(GuardedError::Stale(format!("{name} has newer activity since you asked")));
            }
            Err(e) => {
                rollback(&staged_frames, false);
                return Err(e);
            }
        }
    }

    match renameat2_noreplace(state_dir_fd, "actions.jsonl", &stage_fd, "actions.jsonl") {
        Ok(()) => staged_actions = true,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            rollback(&staged_frames, false);
            return Err(GuardedError::Other(format!("couldn't stage actions.jsonl: {e}")));
        }
    }

    if let Some(want_meta) = expected_actions_meta {
        let cur_staged = if staged_actions {
            match actions_meta_at(&stage_fd, "actions.jsonl") {
                Ok(m) => m,
                Err(e) => {
                    rollback(&staged_frames, true);
                    return Err(e);
                }
            }
        } else {
            None
        };
        if cur_staged != want_meta {
            rollback(&staged_frames, staged_actions);
            return Err(GuardedError::Stale(format!("{name} has newer activity since you asked")));
        }
    }

    // Phase 3: Commit — unlink everything inside `stage_fd` and remove `stage_fd`.
    if staged_actions {
        unlink_at_ignore_enoent(&stage_fd, "actions.jsonl");
    }
    for fname in &staged_frames {
        unlink_at_ignore_enoent(&stage_fd, fname);
    }
    unsafe {
        libc::unlinkat(state_dir_fd.raw(), stage_cname.as_ptr(), libc::AT_REMOVEDIR);
    }
    Ok(())
}

struct GuardedPaths<'a> {
    rt_dirs: &'a [PathBuf],
    state_root: &'a Path,
    trash_root: &'a Path,
}

/// Core atomic check + act under canonical `NAME.lifecycle.lock`(s) and retained
/// `NAME.control.lock`(s) with pinned runtime & state directory descriptors
/// (Items 1–9). Returns `Ok(Some(warning))` if a completed Stop failed to record
/// its history entry so the caller surfaces the recording error separately.
fn run_guarded_action_with_hook(paths: &GuardedPaths<'_>, verb: GuardedVerb, name: &str, expected: ExpectedScreen, after_check: impl FnOnce()) -> Result<Option<String>, GuardedError> {
    let locked_rts = acquire_runtime_locks(paths.rt_dirs, name, lifecycle_lock_timeout(), true)?;
    let running = match probe_running_in_locked_rts(&locked_rts, name) {
        RunningProbe::Running { rt_idx, screen, pidfd } => Some((rt_idx, screen, pidfd)),
        RunningProbe::Stopped => None,
        RunningProbe::Unverifiable(why) => return Err(GuardedError::Other(why)),
    };

    // Item 9 (Finding 25): check already-proved state / `expected_started`
    // mismatches BEFORE opening `actions.jsonl`!
    check_expected_started_state(name, verb, expected, running.as_ref().map(|(_, s, _)| s.started))?;

    // Pin `state_root` and `state_root/name` ONCE BEFORE checking activity so
    // the look and act operate on the exact same directory inode (Items 7 & 8).
    let state_root_fd = match open_verified_owned_dir(paths.state_root, false) {
        Ok(fd) => Some(fd),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(GuardedError::Other(format!("couldn't open screens state folder: {e}"))),
    };
    let state_dir_pinned: Option<(OwnedFd, (u64, u64))> = match state_root_fd.as_ref() {
        Some(rfd) => {
            let cname = std::ffi::CString::new(name).map_err(|_| GuardedError::Usage(format!("invalid screen name {name:?}")))?;
            // SAFETY: valid verified directory fd and validated screen name component.
            let raw = unsafe { libc::openat(rfd.raw(), cname.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
            if raw < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::NotFound {
                    None
                } else {
                    return Err(GuardedError::Other(format!("refusing to open screen folder {name:?}: {err}")));
                }
            } else {
                let dfd = OwnedFd::new(raw);
                let mut st: libc::stat = unsafe { std::mem::zeroed() };
                if unsafe { libc::fstat(dfd.raw(), &mut st) } != 0 {
                    return Err(GuardedError::Other(format!("stat screen folder {name:?}: {}", io::Error::last_os_error())));
                }
                Some((dfd, (st.st_dev as u64, st.st_ino as u64)))
            }
        }
        None => None,
    };

    // Item 9 (Finding 25): if a stopped confirmed Delete/Clear target vanished
    // during confirmation, return exit 75 (`Stale`) rather than exit 1!
    if matches!(verb, GuardedVerb::Delete | GuardedVerb::ClearHistory) && running.is_none() && state_dir_pinned.is_none() {
        return Err(GuardedError::Stale(format!("{name} was removed since you asked")));
    }

    // Finding 5: read activity ONLY when requested (`expected_last_used.is_some()`),
    // and propagate unreadable errors when actually needed to decide!
    let (actual_last_used, actions_meta_snapshot) = if expected.expected_last_used.is_some() {
        match state_dir_pinned.as_ref() {
            Some((dfd, _)) => match probe_last_used_at(dfd) {
                LastUsedProbe::Value { last_used, meta } => (Some(last_used), Some(Some(meta))),
                LastUsedProbe::Absent { meta } => (None, Some(meta)),
                LastUsedProbe::Unreadable(why) => return Err(GuardedError::Other(why)),
            },
            None => (None, Some(None)),
        }
    } else {
        (None, None)
    };

    check_expected_last_used(name, expected.expected_last_used, actual_last_used)?;

    // Verb-specific rules AFTER all expectations have been checked (Finding 25).
    if verb == GuardedVerb::ClearHistory && running.is_some() {
        return Err(GuardedError::Other(format!("screen {name} is running: stop it first")));
    }

    // Item 8: require a regular-file screen marker (`screen.json` or `actions.jsonl`)
    // in BOTH Delete and ClearHistory (matching `screen_follow.rs`).
    if matches!(verb, GuardedVerb::Delete | GuardedVerb::ClearHistory) {
        let Some((ref dfd, _)) = state_dir_pinned else {
            return Err(GuardedError::Stale(format!("{name} was removed since you asked")));
        };
        if !has_screen_marker_at(dfd) {
            let act_word = if verb == GuardedVerb::Delete { "delete" } else { "clear" };
            return Err(GuardedError::Other(format!("refusing to {act_word} {name:?}: not a screen folder")));
        }
    }

    // Item 5: acquire and RETAIN exclusive `NAME.control.lock` across the entire
    // mutation (including `send_home` + Stop, Delete, and Clear), failing closed
    // on lock/marker errors.
    let control_locks = match verb {
        GuardedVerb::Stop => {
            let Some((rt_idx, _, _)) = running.as_ref() else {
                return Err(GuardedError::Stale(format!("{name} isn't running any more")));
            };
            vec![acquire_control_lock_in_rt(&locked_rts[*rt_idx], name)?]
        }
        GuardedVerb::Delete | GuardedVerb::ClearHistory => acquire_control_locks_in_rts(&locked_rts, name)?,
    };

    after_check();

    match verb {
        GuardedVerb::Stop => {
            let Some((rt_idx, s, pidfd)) = running else {
                return Err(GuardedError::Stale(format!("{name} isn't running any more")));
            };
            stop_screen_in_process(
                &locked_rts[rt_idx],
                StopStateTarget { pinned_dir: state_dir_pinned.as_ref().map(|(fd, _)| fd), root_fd: state_root_fd.as_ref(), root_path: paths.state_root },
                name,
                &s,
                &pidfd,
                &control_locks[0],
            )
        }
        GuardedVerb::Delete => {
            let Some(ref rfd) = state_root_fd else {
                return Err(GuardedError::Stale(format!("{name} was removed since you asked")));
            };
            let Some((_, expected_dev_ino)) = state_dir_pinned else {
                return Err(GuardedError::Stale(format!("{name} was removed since you asked")));
            };
            // Move to Trash FIRST (via staging rename); unlink stale owner markers only after move succeeds.
            trash_screen_pinned_with_hooks(paths.state_root, rfd, expected_dev_ino, paths.trash_root, name, || {}, |_| None)?;
            for rt in &locked_rts {
                unlink_at_ignore_enoent(&rt.dir_fd, &format!("{name}.owner.json"));
            }
            drop(control_locks);
            Ok(None)
        }
        GuardedVerb::ClearHistory => {
            let Some((ref dfd, _)) = state_dir_pinned else {
                return Err(GuardedError::Stale(format!("{name} was removed since you asked")));
            };
            clear_history_pinned_with_hook(dfd, name, actions_meta_snapshot, || {})?;
            drop(control_locks);
            Ok(None)
        }
    }
}

fn run_guarded_action_cli(parsed: GuardedAction) -> ! {
    // Finding 6: require installed executable host `olo-screen` to honor `NAME.lifecycle.lock`!
    match host_cli_honors_lifecycle_lock() {
        Ok(true) => {}
        Ok(false) => exit_guarded_error(GuardedError::Other("This host's installed olo-screen is too old for guarded screen actions; run `olo screen install` to upgrade.".into())),
        Err(e) => exit_guarded_error(GuardedError::Other(e)),
    }
    let rt_base = match runtime_dir_checked() {
        Ok(r) => r,
        Err(e) => exit_guarded_error(GuardedError::Other(format!("unsafe fallback runtime directory: {e}"))),
    };
    let rt_dirs = vec![rt_base.join("olo-screens"), rt_base.join("tabula-screens")];
    let state_root = screens_state_dir();
    let trash_root = xdg_trash_dir();
    let paths = GuardedPaths { rt_dirs: &rt_dirs, state_root: &state_root, trash_root: &trash_root };
    match run_guarded_action_with_hook(&paths, parsed.verb, &parsed.name, parsed.expected, || {}) {
        Ok(Some(rec_err)) => {
            // Item 6: surface history recording error separately from completed stop.
            eprintln!("olo screen: warning: stopped {}, but couldn't record stop in history: {rec_err}", parsed.name);
            std::process::exit(0);
        }
        Ok(None) => std::process::exit(0),
        Err(e) => exit_guarded_error(e),
    }
}

fn print_lifecycle_help(cmd: &str) -> ! {
    match cmd {
        "stop" => println!("usage: olo screen stop NAME [--quick] [--expected-started SECONDS] [--expected-last-used MILLIS]"),
        "delete" => println!("usage: olo screen delete NAME [--expected-started SECONDS] [--expected-last-used MILLIS]"),
        "history" => println!(
            "usage: olo screen history NAME [--clear [--expected-started SECONDS] [--expected-last-used MILLIS]] [--kind KIND] [--search TEXT] [--last N] [--json] [--look N [OUT]] [--export OUT …]"
        ),
        _ => println!("{USAGE}"),
    }
    std::process::exit(0);
}

/// `stop NAME [--quick] [--expected-started S] [--expected-last-used M]` (Item 9 / Finding 16).
fn stop_cmd(args: &[String]) -> ! {
    match classify_lifecycle_cli_args(args) {
        Ok(LifecycleCliAction::Help) => print_lifecycle_help("stop"),
        Ok(LifecycleCliAction::Guarded(parsed)) => run_guarded_action_cli(parsed),
        Ok(LifecycleCliAction::Forward) => {
            let name = args[1..].iter().find(|a| !a.starts_with('-')).cloned();
            let mut tool = screen_tool(args);
            let status = match tool.status() {
                Ok(s) => s,
                Err(e) => fail(format_args!("{}: {e}", tool.get_program().to_string_lossy())),
            };
            if status.success()
                && let Some(ref n) = name
            {
                unlink_stale_runtime_owner(n);
            }
            std::process::exit(status.code().unwrap_or(1));
        }
        Err(msg) => exit_guarded_error(GuardedError::Usage(msg)),
    }
}

/// `delete NAME [--expected-started S] [--expected-last-used M]` (Item 9 / Finding 16).
fn delete_cmd(args: &[String]) -> ! {
    match classify_lifecycle_cli_args(args) {
        Ok(LifecycleCliAction::Help) => print_lifecycle_help("delete"),
        Ok(LifecycleCliAction::Guarded(parsed)) => run_guarded_action_cli(parsed),
        Ok(LifecycleCliAction::Forward) => exec(args),
        Err(msg) => exit_guarded_error(GuardedError::Usage(msg)),
    }
}

/// `history NAME [--clear [--expected-started S] [--expected-last-used M]] …` (Item 9 / Finding 16).
fn history_cmd(args: &[String]) -> ! {
    match classify_lifecycle_cli_args(args) {
        Ok(LifecycleCliAction::Help) => print_lifecycle_help("history"),
        Ok(LifecycleCliAction::Guarded(parsed)) => run_guarded_action_cli(parsed),
        Ok(LifecycleCliAction::Forward) => exec(args),
        Err(msg) => exit_guarded_error(GuardedError::Usage(msg)),
    }
}

/// The Olo here's `olo-screen`, to be run with `args`: its own programs
/// first on the path (`olo screen run NAME olo-chrome …`).
fn screen_tool(args: &[String]) -> Command {
    let prefix = prefix();
    let program = prefix.join("bin/olo-screen");
    if !program.is_file() {
        fail(format_args!("Olo isn't on this host yet: `olo screen install` puts it here, under {} (about 600 MB to download; nothing needs root)", prefix.display()));
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(std::iter::once(prefix.join("bin")).chain(std::env::split_paths(&path))).unwrap_or(path);
    let mut tool = Command::new(&program);
    tool.args(args).env("PATH", path);
    tool
}

/// `olo-screen ARGS…` of the Olo here. When run from a Rex pane on an
/// ownership-updating command, records `owner.json` ONLY after `olo-screen`
/// succeeds (so a rejected action never overwrites ownership, and `take --new`
/// attributes to the screen that was actually created).
fn exec(args: &[String]) -> ! {
    let mut tool = screen_tool(args);
    let cmd = args.first().map(String::as_str);
    let has_block = std::env::var_os("REX_BLOCK").is_some_and(|b| !b.is_empty());
    if has_block
        && let Some(cmd) = cmd
        && OWNER_ACTIONS.contains(&cmd)
    {
        if cmd == "take" {
            let is_new = args[1..].iter().any(|a| a == "--new");
            let out = match tool.stdin(std::process::Stdio::inherit()).stderr(std::process::Stdio::inherit()).output() {
                Ok(out) => out,
                Err(e) => fail(format_args!("olo-screen: {e}")),
            };
            {
                let mut stdout = io::stdout().lock();
                let _ = stdout.write_all(&out.stdout);
                let _ = stdout.flush();
            }
            if out.status.success() {
                let target = serde_json::from_slice::<serde_json::Value>(&out.stdout)
                    .ok()
                    .and_then(|v| v["name"].as_str().map(str::to_owned))
                    .or_else(|| (!is_new).then(|| args[1..].iter().find(|a| !a.starts_with("--")).cloned()).flatten());
                if let Some(name) = target {
                    record_owner(&name, is_new);
                }
            }
            std::process::exit(out.status.code().unwrap_or(1));
        }
        let status = match tool.status() {
            Ok(s) => s,
            Err(e) => fail(format_args!("{}: {e}", tool.get_program().to_string_lossy())),
        };
        if status.success()
            && let Some(name) = args[1..].iter().find(|a| a.as_str() != "--quick" && !a.starts_with("--"))
        {
            record_owner(name, false);
        }
        std::process::exit(status.code().unwrap_or(1));
    }
    if cmd == Some("delete") {
        let status = match tool.status() {
            Ok(s) => s,
            Err(e) => fail(format_args!("{}: {e}", tool.get_program().to_string_lossy())),
        };
        if status.success()
            && let Some(name) = args[1..].iter().find(|a| !a.starts_with("--"))
        {
            unlink_stale_runtime_owner(name);
        }
        std::process::exit(status.code().unwrap_or(1));
    }
    let e = tool.exec();
    fail(format_args!("{}: {e}", tool.get_program().to_string_lossy()));
}

/// `start`: the screen started, and (from a Rex tab, unless `--keep`)
/// a keeper left that stops it when the tab's program ends.
fn start(args: &[String]) -> ! {
    let keep = args.iter().any(|a| a == "--keep");
    let passed: Vec<String> = args.iter().filter(|a| *a != "--keep").cloned().collect();
    let out = match screen_tool(&passed).stdin(std::process::Stdio::inherit()).stderr(std::process::Stdio::inherit()).output() {
        Ok(out) => out,
        Err(e) => fail(format_args!("olo-screen: {e}")),
    };
    {
        use std::io::Write as _;
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(&out.stdout);
        let _ = stdout.flush();
    }
    if out.status.success()
        && let Ok(screen) = serde_json::from_slice::<serde_json::Value>(&out.stdout)
        && let Some(name) = screen["name"].as_str()
    {
        if std::env::var_os("REX_BLOCK").is_some_and(|b| !b.is_empty()) {
            record_owner(name, true);
        } else {
            // Finding 54: a non-Rex start must not inherit a prior Rex pane's runtime owner.
            unlink_runtime_owner(name);
        }
        if !keep
            && let Some(session) = screen["pid"].as_i64()
            && let Some((owner, began)) = tab_program()
            && let Some((_, session_began)) = parent_and_start(session as i32)
        {
            // (In the screen's process group, so that it is seen with it.
            // Execute our own running image via `/proc/self/exe` on Linux so
            // replacing or pruning the helper binary never breaks spawning.)
            let me = if Path::new("/proc/self/exe").exists() { PathBuf::from("/proc/self/exe") } else { std::env::current_exe().unwrap_or_else(|_| "olo-rex-link".into()) };
            let kept = Command::new(me)
                .args(["screen", "keep-until", name, &owner.to_string(), &began.to_string(), &session.to_string(), &session_began.to_string()])
                .process_group(session as i32)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            if let Err(e) = kept {
                eprintln!("olo screen: {name} will stay until it is stopped (what would stop it with this tab couldn't start: {e})");
            }
        }
    }
    std::process::exit(out.status.code().unwrap_or(1));
}

/// A process's parent and when it began (in the kernel's ticks since
/// boot: with its number, what it is and no other), from
/// `/proc/PID/stat`.
fn parent_and_start(pid: i32) -> Option<(i32, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // (After its name, which may hold anything: the last `)`.)
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    Some((fields.get(1)?.parse().ok()?, fields.get(19)?.parse().ok()?))
}

/// What a process was started with for `REX_BLOCK`: the Rex tab's pane
/// it is in.
fn block_of(pid: i32) -> Option<Vec<u8>> {
    let environ = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    environ.split(|b| *b == 0).find_map(|var| var.strip_prefix(b"REX_BLOCK=")).filter(|v| !v.is_empty()).map(<[u8]>::to_vec)
}

/// The program of the Rex tab this was run from: the furthest of our
/// parents started in the same pane as we were (`REX_BLOCK`), with when
/// it began. None outside a Rex tab.
fn tab_program() -> Option<(i32, u64)> {
    let block = std::env::var_os("REX_BLOCK").filter(|b| !b.is_empty())?;
    let block = std::os::unix::ffi::OsStrExt::as_bytes(block.as_os_str());
    let mut found = None;
    let mut pid = unsafe { libc::getppid() };
    // (A chain of parents is short; a hundred is a loop that isn't one.)
    for _ in 0..100 {
        if pid <= 1 || block_of(pid).as_deref() != Some(block) {
            break;
        }
        let (parent, began) = parent_and_start(pid)?;
        found = Some((pid, began));
        pid = parent;
    }
    found
}

/// A process held: a descriptor of the process itself (the one that
/// began at `began`), readable when it has ended. Its number given to
/// another after it ends is never taken for it. None where it has ended
/// already.
fn held(pid: i32, began: u64) -> Option<i32> {
    // SAFETY: a system call with two integers.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
    if fd < 0 {
        return None;
    }
    if parent_and_start(pid).is_some_and(|(_, b)| b == began) {
        return Some(fd);
    }
    // SAFETY: ours, just opened.
    unsafe { libc::close(fd) };
    None
}

/// `keep-until NAME PID BEGAN SESSION SESSION_BEGAN`: wait for the
/// process PID (the tab's program) to end, then stop screen NAME; or
/// for the screen's own session to end, and then there is nothing to
/// stop. (The session too: a screen stopped by hand ends its session,
/// not its process group, and its keeper, left waiting for the tab,
/// would have stopped whatever screen had that name by then: seen on
/// the dev server's own account, 2026-11-26.)
fn keep_until(args: &[String]) -> ! {
    let [name, owner, began, session, session_began] = args else { std::process::exit(2) };
    let (Ok(owner), Ok(began), Ok(session), Ok(session_began)) = (owner.parse::<i32>(), began.parse::<u64>(), session.parse::<i32>(), session_began.parse::<u64>()) else { std::process::exit(2) };
    // The screen it was left for has ended already: nothing to keep.
    let Some(session) = held(session, session_began) else { std::process::exit(0) };
    if let Some(owner) = held(owner, began) {
        let mut waiting = [libc::pollfd { fd: owner, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: session, events: libc::POLLIN, revents: 0 }];
        // (Each readable when its process has ended. Interrupted: wait again.)
        // SAFETY: two descriptors of ours, in an array of two.
        while unsafe { libc::poll(waiting.as_mut_ptr(), 2, -1) } < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {}
        // The screen ended first, stopped by hand or by itself.
        if waiting[1].revents != 0 {
            std::process::exit(0);
        }
    }
    // The tab's program has ended and the screen still runs: stopped.
    // (In a group of its own: `stop` may end the screen's, which is ours.)
    let _ = screen_tool(&["stop".to_owned(), name.clone()]).process_group(0).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    std::process::exit(0);
}

/// Says how far a step has got: when the step changes, when it ends,
/// and each quarter second between.
struct Told {
    doing: &'static str,
    last: Option<Instant>,
}

impl Told {
    fn tell(&mut self, doing: &'static str, done: u64, total: u64) {
        let now = Instant::now();
        if self.doing == doing && done < total && self.last.is_some_and(|t| now.duration_since(t) < Duration::from_millis(250)) {
            return;
        }
        (self.doing, self.last) = (doing, Some(now));
        let fraction = if total > 0 { (done as f64 / total as f64).min(1.0) } else { 0.0 };
        say(json!({ "doing": doing, "fraction": (fraction * 1000.0).round() / 1000.0 }));
    }
}

fn option(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn install(args: &[String]) -> ! {
    let prefix = prefix();
    if olo_release::arch() != "x86_64" && option(args, "--from").is_none() {
        fail(format_args!("Olo's releases are for x86_64; this host is {}", olo_release::arch()));
    }
    let ask = olo_release::Install { from: option(args, "--from"), tag: option(args, "--tag"), retire: Vec::new(), without: vec!["olo-wallpapers-".into()] };
    let mut told = Told { doing: "", last: None };
    // The release asked for is the one here already: nothing is fetched
    // (600 MB, to find every program the same). `--again` fetches it
    // all the same; an archive given is always put.
    let (here, _) = read_installed_meta(&prefix);
    let wanted = match (&ask.from, &ask.tag, args.iter().any(|a| a == "--again")) {
        (None, Some(tag), false) => olo_release::by_tag(tag).ok().map(|r| r.version),
        (None, None, false) => olo_release::latest().ok().map(|r| r.version),
        _ => None,
    };
    // Olo isn't changed under screens that run: each follows the Olo it
    // runs from (its app_server reloads in place a moment after its
    // programs change, its apps start again), which is nothing to do to
    // another agent in the middle of a page. Said, with which run;
    // `--under-screens` goes ahead. (Chrome and the font are put all
    // the same: a running Chrome keeps the one it was started from.)
    let running = if here.is_some() && !args.iter().any(|a| a == "--under-screens") { running_screens(&prefix) } else { Vec::new() };
    let mut held = false;
    if here.is_some() && here == wanted {
        say(json!({ "installed": here, "changed": 0, "was": true }));
    } else if !running.is_empty() {
        held = true;
        say(
            json!({ "installed": here, "changed": 0, "held": running, "how": "screens run on the Olo that is here, and each would reload a moment after it changed: `olo screen install` again once they are stopped, or `olo screen install --under-screens`" }),
        );
    } else {
        let (changed, version) = match olo_release::install(&prefix, &ask, &AtomicBool::new(false), &mut |doing, done, all| told.tell(doing, done, all)) {
            Ok(done) => done,
            Err(e) => {
                say(json!({ "error": e }));
                std::process::exit(1);
            }
        };
        say(json!({ "installed": version, "changed": changed.len() }));
    }
    // Chrome, by Olo's own launcher: Google's package, unpacked in the
    // home folder. What it says is passed on as it comes.
    if !args.iter().any(|a| a == "--no-chrome") {
        say(json!({ "doing": "chrome" }));
        match Command::new(prefix.join("bin/olo-chrome")).arg("--olo-install").stdin(std::process::Stdio::null()).status() {
            Ok(s) if s.success() => {}
            Ok(s) => say(json!({ "error": format!("Chrome wasn't put here (olo-chrome --olo-install: {s}); Olo is, and `olo screen install` again tries Chrome again") })),
            Err(e) => say(json!({ "error": format!("Chrome wasn't put here (olo-chrome: {e})") })),
        }
    }
    // A font with emoji, where this host has none.
    if !args.iter().any(|a| a == "--no-emoji-font") && !emoji_font() {
        let put = (|| {
            let fetched = olo_release::fetch_file(EMOJI_FONT, EMOJI_FONT_URL, EMOJI_FONT_SHA256, &olo_release::cache_dir(), &AtomicBool::new(false), &mut |got, of| told.tell("emoji font", got, of))?;
            let dir = screens_fonts();
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let (tmp, to) = (dir.join(format!(".{EMOJI_FONT}.new")), dir.join(EMOJI_FONT));
            std::fs::copy(&fetched, &tmp).and_then(|_| std::fs::rename(&tmp, &to)).map_err(|e| format!("{}: {e}", to.display()))?;
            let _ = std::fs::remove_file(&fetched);
            Ok::<_, String>(to)
        })();
        match put {
            Ok(to) => say(json!({ "emoji_font": to })),
            Err(e) => say(json!({ "error": format!("a font with emoji wasn't put here ({e}); emoji are boxes until one is: `olo screen install` again tries again") })),
        }
    }
    let now = status();
    let ready = now["missing"].as_array().is_some_and(Vec::is_empty) && now["older"].as_object().is_some_and(|o| o.is_empty()) && now["software_drawing"] == true;
    say(now);
    // (3: this host lacks something its screens need. 4: Olo was left
    // as it is, for the screens that run.)
    std::process::exit(match (ready, held) {
        (false, _) => 3,
        (true, true) => 4,
        (true, false) => 0,
    });
}

fn default(args: &[String]) -> ! {
    match args.first().map(String::as_str) {
        None => println!("{}", default_now()),
        Some(to @ ("host" | "desktop")) => {
            let file = default_file();
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(&file, format!("{to}\n")) {
                fail(format_args!("{}: {e}", file.display()));
            }
            println!("{to}");
        }
        Some(other) => fail(format_args!("`{other}` is neither `host` nor `desktop`")),
    }
    std::process::exit(0);
}

/// The screens that run on the Olo under `prefix`, by name.
fn running_screens(prefix: &Path) -> Vec<String> {
    let listed = Command::new(prefix.join("bin/olo-screen")).arg("list").output().ok().and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok()).unwrap_or_default();
    listed.as_array().into_iter().flatten().filter(|s| s["state"] == "running").filter_map(|s| s["name"].as_str().map(str::to_owned)).collect()
}

fn remove() -> ! {
    let prefix = prefix();
    if !prefix.join("bin/olo-screen").is_file() {
        println!("Olo isn't on this host: nothing to take off");
        std::process::exit(0);
    }
    // (Never under a screen that runs: its programs are these files.)
    let running = running_screens(&prefix);
    if !running.is_empty() {
        fail(format_args!("{} here ({}): `olo screen --on host stop NAME` first", if running.len() == 1 { "a screen still runs" } else { "screens still run" }, running.join(", ")));
    }
    if let Err(e) = std::fs::remove_dir_all(&prefix) {
        fail(format_args!("{}: {e}", prefix.display()));
    }
    // (And the font put here with it, in its folder of its own.)
    let _ = std::fs::remove_dir_all(screens_fonts());
    println!("Olo is off this host ({})", prefix.display());
    std::process::exit(0);
}

/// Shared host library / Mesa readiness check used by both `screen status` and
/// `screen follow`'s `HostScreenStatus::not_set_up_reason`.
pub(crate) struct HostReadiness {
    pub(crate) programs: usize,
    pub(crate) missing: Vec<String>,
    pub(crate) older: BTreeMap<String, BTreeSet<String>>,
    pub(crate) software_drawing: bool,
    pub(crate) not_set_up_reason: Option<String>,
}

pub(crate) const NOT_INSTALLED_REASON: &str = "Olo isn't installed on this host yet (`olo screen install`).";

#[cfg(test)]
thread_local! {
    pub(crate) static HOST_READINESS_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(crate) fn host_readiness(prefix_dir: &Path, installed: Option<&str>) -> HostReadiness {
    #[cfg(test)]
    HOST_READINESS_CALLS.with(|c| c.set(c.get() + 1));
    let dirs = library_dirs();
    let has = |name: &str| dirs.iter().any(|d| d.join(name).exists());
    let mut missing: Vec<String> = Vec::new();
    // A library here but older than the programs were linked against:
    // the versions of its symbols they need that it hasn't.
    let mut older: BTreeMap<String, BTreeSet<String>> = Default::default();
    let mut defined: std::collections::HashMap<String, Option<Vec<String>>> = Default::default();
    let mut programs = 0;
    for entry in std::fs::read_dir(prefix_dir.join("bin")).into_iter().flatten().flatten() {
        let Some(program) = Linked::open(&entry.path()) else { continue };
        programs += 1;
        for library in program.needed() {
            if !has(&library) && !missing.contains(&library) {
                missing.push(library);
            }
        }
        lacking(&program.versions_needed(), &mut older, &mut |library| {
            defined.entry(library.to_owned()).or_insert_with(|| dirs.iter().map(|d| d.join(library)).find(|p| p.exists()).and_then(|p| Linked::open(&p)).map(|l| l.versions_defined())).clone()
        });
    }
    missing.sort();
    // Drawn in software: Mesa's EGL, and its software renderer (one
    // library since Mesa 24.2, `libgallium`; a driver of its own before).
    let gallium = dirs.iter().any(|d| std::fs::read_dir(d).into_iter().flatten().flatten().any(|e| e.file_name().to_string_lossy().starts_with("libgallium")) || d.join("dri/swrast_dri.so").exists());
    let software_drawing = has("libEGL_mesa.so.0") && gallium;
    let not_set_up_reason = if installed.is_none() {
        Some(NOT_INSTALLED_REASON.to_owned())
    } else if !older.is_empty() {
        Some("this host's libraries are older than Olo's programs are linked against (Fedora 44's): its screens can't run here until the system is a newer one".to_owned())
    } else if !missing.is_empty() {
        Some(format!("this host is missing libraries Olo's programs need: {}", missing.join(", ")))
    } else if !software_drawing {
        Some(
            "Mesa's EGL and software renderer aren't here: on Fedora, `sudo dnf install mesa-libEGL mesa-dri-drivers`; on Debian or Ubuntu, `sudo apt install libegl-mesa0 libgl1-mesa-dri`".to_owned(),
        )
    } else {
        None
    };
    HostReadiness { programs, missing, older, software_drawing, not_set_up_reason }
}

/// Non-blocking, FIFO-safe read of installed Olo version (`share/olo/version`)
/// and commit timestamp (`share/olo/release.json`) under `prefix_dir` (Item 10 / Finding 23).
pub(crate) fn read_installed_meta(prefix_dir: &Path) -> (Option<String>, Option<u64>) {
    let share = prefix_dir.join("share/olo");
    let has_tool = prefix_dir.join("bin/olo-screen").is_file();
    let installed = if has_tool { read_bounded_regular(&share.join("version"), 1024).and_then(|b| String::from_utf8(b).ok()).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()) } else { None };
    let committed = installed.as_ref().and_then(|_| {
        let bytes = read_bounded_regular(&share.join("release.json"), 64 * 1024)?;
        let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        v["committed"].as_u64()
    });
    (installed, committed)
}

/// What is here.
fn status() -> serde_json::Value {
    let prefix = prefix();
    let (installed, committed) = read_installed_meta(&prefix);
    let readiness = host_readiness(&prefix, installed.as_deref());
    let chrome = std::fs::read_link(home().join(".local/share/olo/chrome/current")).ok().map(|p| p.to_string_lossy().into_owned());
    let mut v = json!({
        "installed": installed,
        "prefix": prefix,
        "programs": readiness.programs,
        // (All of it: Olo, the Chrome it runs and the font put with it.
        // Olo's alone, 1.8 GB, stood beside "2.3 GB on the host" in the
        // window that had both.)
        "bytes": if installed.is_some() { bytes_under(&prefix) + bytes_under(&home().join(".local/share/olo/chrome")) + bytes_under(&screens_fonts()) } else { 0 },
        "chrome": chrome,
        "missing": readiness.missing,
        "older": readiness.older,
        "software_drawing": readiness.software_drawing,
        "emoji_font": emoji_font(),
        "committed": committed,
        "running": if installed.is_some() { running_screens(&prefix) } else { Vec::new() },
        "default": default_now(),
    });
    if installed.is_none() {
        v["how"] = json!("olo screen install");
    } else if let Some(reason) = readiness.not_set_up_reason {
        v["how"] = json!(reason);
    }
    v
}

/// Whether this host has a font with emoji, as far as a file's name
/// says (Noto Color Emoji, Twemoji, Apple Color Emoji…), among the
/// system's fonts and the account's. A server often has none, and then
/// every emoji is a box, in Olo's windows and on a page in Chrome alike.
fn emoji_font() -> bool {
    let mut todo: Vec<(PathBuf, u32)> = [PathBuf::from("/usr/share/fonts"), PathBuf::from("/usr/local/share/fonts"), account_fonts(), home().join(".fonts")].into_iter().map(|d| (d, 0)).collect();
    while let Some((dir, depth)) = todo.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                if depth < 4 {
                    todo.push((entry.path(), depth + 1));
                }
            } else if name.contains("emoji") && [".ttf", ".otf", ".ttc"].iter().any(|x| name.ends_with(x)) {
                return true;
            }
        }
    }
    false
}

/// Of the versions a program `needs` of each library's symbols, those
/// the library here hasn't (`defined`: what it has; None where it isn't
/// here at all, which `missing` says), added to `older`.
fn lacking(needs: &[(String, Vec<String>)], older: &mut std::collections::BTreeMap<String, std::collections::BTreeSet<String>>, defined: &mut dyn FnMut(&str) -> Option<Vec<String>>) {
    for (library, versions) in needs {
        let Some(here) = defined(library) else { continue };
        for version in versions {
            if !here.contains(version) {
                older.entry(library.clone()).or_default().insert(version.clone());
            }
        }
    }
}

/// What `dir` holds on disk, in bytes (as it is stored, not its length).
fn bytes_under(dir: &Path) -> u64 {
    let mut total = 0;
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                todo.push(entry.path());
            } else {
                total += meta.blocks() * 512;
            }
        }
    }
    total
}

/// Where this host's loader looks for a library: its usual folders for
/// this machine, and those `conf_file` / `conf_d/*.conf` name (using
/// non-blocking regular-file reads so a FIFO `.conf` never hangs — Item 10 / Finding 23).
fn library_dirs_from(conf_file: &Path, conf_d: &Path) -> Vec<PathBuf> {
    let machine = std::env::consts::ARCH;
    let mut dirs: Vec<PathBuf> = ["/lib64", "/usr/lib64", "/usr/local/lib64", "/lib", "/usr/lib", "/usr/local/lib"].iter().map(PathBuf::from).collect();
    dirs.push(format!("/lib/{machine}-linux-gnu").into());
    dirs.push(format!("/usr/lib/{machine}-linux-gnu").into());
    let mut confs = vec![conf_file.to_path_buf()];
    confs.extend(std::fs::read_dir(conf_d).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "conf")));
    for conf in confs {
        let Some(bytes) = read_bounded_regular_follow(&conf, 256 * 1024) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.starts_with('/') {
                dirs.push(line.into());
            }
        }
    }
    dirs.retain(|d| d.is_dir());
    dirs.dedup();
    dirs
}

fn library_dirs() -> Vec<PathBuf> {
    library_dirs_from(Path::new("/etc/ld.so.conf"), Path::new("/etc/ld.so.conf.d"))
}

/// A program or a library as its loader reads it: a 64-bit ELF of this
/// machine's byte order with a dynamic section, read from the file's own
/// headers (nothing of it is run). None for anything else (a script, a
/// static program).
pub struct Linked {
    file: std::fs::File,
    /// Its dynamic section: each entry's tag and value.
    dynamic: Vec<(u64, u64)>,
    /// Where its strings are, in the file.
    strings: u64,
    /// What is loaded where: (address, place in the file, length).
    loads: Vec<(u64, u64, u64)>,
}

fn u16_of(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as u64)
}

fn u32_of(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u64)
}

fn u64_of(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 8).and_then(|b| b.try_into().ok()).map_or(0, u64::from_le_bytes)
}

impl Linked {
    pub fn open(path: &Path) -> Option<Linked> {
        // Finding 23: non-blocking open + `is_file()` check (allowing standard
        // shared-library SONAME symlinks like `libgcc_s.so.1 -> ...`) so a FIFO
        // in `prefix/bin` can never hang `screen follow` or `screen status`.
        let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open(path).ok()?;
        let meta = file.metadata().ok()?;
        if !meta.file_type().is_file() {
            return None;
        }
        let read = |at: u64, len: usize| {
            let mut buf = vec![0u8; len];
            file.read_exact_at(&mut buf, at).ok().map(|()| buf)
        };
        let head = read(0, 64)?;
        // (0x7f "ELF", 64-bit, little-endian.)
        if head[..4] != *b"\x7fELF" || head[4] != 2 || head[5] != 1 {
            return None;
        }
        let (phoff, phentsize, phnum) = (u64_of(&head, 0x20), u16_of(&head, 0x36), u16_of(&head, 0x38));
        if phentsize < 56 || phnum > 512 {
            return None;
        }
        // Its segments: what is loaded where (to find a table by its
        // address), and the dynamic one.
        let (mut loads, mut dynamic) = (Vec::new(), None);
        for i in 0..phnum {
            let ph = read(phoff + i * phentsize, 56)?;
            let (kind, offset, vaddr, filesz) = (u32_of(&ph, 0), u64_of(&ph, 8), u64_of(&ph, 16), u64_of(&ph, 32));
            match kind {
                1 => loads.push((vaddr, offset, filesz)),
                2 => dynamic = Some((offset, filesz)),
                _ => {}
            }
        }
        let (at, len) = dynamic?;
        if len > 1 << 20 {
            return None;
        }
        let table = read(at, len as usize)?;
        let dynamic: Vec<(u64, u64)> = table.as_chunks::<16>().0.iter().map(|e| (u64_of(e, 0), u64_of(e, 8))).take_while(|(tag, _)| *tag != 0).collect();
        let mut linked = Linked { file, dynamic, strings: 0, loads };
        // (DT_STRTAB.)
        linked.strings = linked.at(linked.tag(5)?)?;
        Some(linked)
    }

    fn tag(&self, tag: u64) -> Option<u64> {
        self.dynamic.iter().find(|(t, _)| *t == tag).map(|(_, v)| *v)
    }

    /// Where an address is in the file.
    fn at(&self, address: u64) -> Option<u64> {
        self.loads.iter().find(|(vaddr, _, filesz)| address >= *vaddr && address < vaddr + filesz).map(|(vaddr, offset, _)| offset + (address - vaddr))
    }

    fn read(&self, at: u64, len: usize) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; len];
        self.file.read_exact_at(&mut buf, at).ok().map(|()| buf)
    }

    /// The string at `name` in its strings. (A library's or a version's
    /// name is short; the file may end sooner than 256.)
    fn string(&self, name: u64) -> Option<String> {
        let mut buf = vec![0u8; 256];
        let got = self.file.read_at(&mut buf, self.strings + name).ok()?;
        let end = buf[..got].iter().position(|b| *b == 0)?;
        Some(String::from_utf8_lossy(&buf[..end]).into_owned())
    }

    /// The libraries it asks its loader for (`DT_NEEDED`).
    pub fn needed(&self) -> Vec<String> {
        self.dynamic.iter().filter(|(tag, _)| *tag == 1).filter_map(|(_, name)| self.string(*name)).collect()
    }

    /// The versions it needs of each library's symbols (`DT_VERNEED`:
    /// `libc.so.6` at `GLIBC_2.38`): a library older than the one it
    /// was linked against hasn't them, and the loader refuses the
    /// program ("version `GLIBC_2.43' not found").
    pub fn versions_needed(&self) -> Vec<(String, Vec<String>)> {
        let (Some(table), Some(count)) = (self.tag(0x6fff_fffe).and_then(|a| self.at(a)), self.tag(0x6fff_ffff)) else { return Vec::new() };
        let mut out = Vec::new();
        let mut at = table;
        for _ in 0..count.min(256) {
            // (vn_version, vn_cnt, vn_file, vn_aux, vn_next.)
            let Some(need) = self.read(at, 16) else { break };
            let (cnt, file, aux, next) = (u16_of(&need, 2), u32_of(&need, 4), u32_of(&need, 8), u32_of(&need, 12));
            let mut versions = Vec::new();
            let mut aux_at = at + aux;
            for _ in 0..cnt.min(256) {
                // (vna_hash, vna_flags, vna_other, vna_name, vna_next.)
                let Some(entry) = self.read(aux_at, 16) else { break };
                versions.extend(self.string(u32_of(&entry, 8)));
                match u32_of(&entry, 12) {
                    0 => break,
                    n => aux_at += n,
                }
            }
            if let Some(file) = self.string(file) {
                out.push((file, versions));
            }
            match next {
                0 => break,
                n => at += n,
            }
        }
        out
    }

    /// The versions of its symbols a library has (`DT_VERDEF`).
    pub fn versions_defined(&self) -> Vec<String> {
        let (Some(table), Some(count)) = (self.tag(0x6fff_fffc).and_then(|a| self.at(a)), self.tag(0x6fff_fffd)) else { return Vec::new() };
        let mut out = Vec::new();
        let mut at = table;
        for _ in 0..count.min(4096) {
            // (vd_version, vd_flags, vd_ndx, vd_cnt, vd_hash, vd_aux,
            // vd_next; its first aux entry is its own name.)
            let Some(def) = self.read(at, 20) else { break };
            let (aux, next) = (u32_of(&def, 12), u32_of(&def, 16));
            if let Some(entry) = self.read(at + aux, 8) {
                out.extend(self.string(u32_of(&entry, 0)));
            }
            match next {
                0 => break,
                n => at += n,
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A C library older than a program was linked against is said by
    /// the versions it lacks; one that isn't here at all is `missing`'s
    /// to say, not this.
    #[test]
    fn an_older_library_is_said_by_the_versions_it_lacks() {
        let needs = vec![
            ("libc.so.6".to_owned(), vec!["GLIBC_2.34".to_owned(), "GLIBC_2.43".to_owned(), "GLIBC_2.42".to_owned()]),
            ("libm.so.6".to_owned(), vec!["GLIBC_2.29".to_owned()]),
            ("libabsent.so.1".to_owned(), vec!["ABSENT_1".to_owned()]),
        ];
        let mut older = Default::default();
        lacking(&needs, &mut older, &mut |library| match library {
            "libc.so.6" => Some(vec!["GLIBC_2.2.5".into(), "GLIBC_2.34".into(), "GLIBC_2.39".into()]),
            "libm.so.6" => Some(vec!["GLIBC_2.29".into()]),
            _ => None,
        });
        assert_eq!(serde_json::to_string(&older).unwrap(), r#"{"libc.so.6":["GLIBC_2.42","GLIBC_2.43"]}"#);
    }

    /// This test's own program asks for the C library, at versions of
    /// its symbols this host's has (it runs); a script asks for nothing.
    #[test]
    fn a_programs_libraries_are_read_from_its_headers() {
        let me = Linked::open(&std::env::current_exe().unwrap()).expect("a test is a dynamic program");
        let libraries = me.needed();
        assert!(libraries.iter().any(|l| l.starts_with("libc.so")), "{libraries:?}");
        let script = std::env::temp_dir().join(format!("olo-screens-needed-{}", std::process::id()));
        std::fs::write(&script, "#!/bin/sh\necho hello\n").unwrap();
        assert!(Linked::open(&script).is_none());
        let _ = std::fs::remove_file(&script);
        // And this host has what it asks for, each at the versions it needs.
        let dirs = library_dirs();
        for library in &libraries {
            assert!(dirs.iter().any(|d| d.join(library).exists()), "{library} in none of {dirs:?}");
        }
        let versions = me.versions_needed();
        let (_, of_c) = versions.iter().find(|(library, _)| library.starts_with("libc.so")).expect("versions of the C library's symbols");
        assert!(of_c.iter().any(|v| v.starts_with("GLIBC_2.")), "{of_c:?}");
        for (library, needs) in &versions {
            let here = dirs.iter().map(|d| d.join(library)).find(|p| p.exists()).and_then(|p| Linked::open(&p)).map(|l| l.versions_defined()).unwrap_or_default();
            assert!(here.len() >= needs.len(), "{library} defines {here:?}");
            for version in needs {
                assert!(here.contains(version), "{library} has no {version}: {here:?}");
            }
        }
    }

    #[test]
    fn review_2_25_33_controller_exe_session_cmdline_and_timeout_ceil() {
        // Findings 2 & 25: `olo-screenshot` is NOT a controller; `olo-rex` and `olo-rex-link*` ARE.
        assert!(!is_controller_exe(Path::new("/usr/bin/olo-screenshot")));
        assert!(!is_controller_exe(Path::new("/usr/bin/sleep")));
        assert!(is_controller_exe(Path::new("/usr/bin/olo-screen")));
        assert!(is_controller_exe(Path::new("/usr/bin/tabula-screen")));
        assert!(is_controller_exe(Path::new("/usr/bin/olo-rex")));
        assert!(is_controller_exe(Path::new("/tmp/olo-rex-link-abc123-x86_64 (deleted)")));

        // Finding 25: only `olo-session` / `tabula-session` basenames pass session identity.
        assert!(is_session_cmdline(b"/usr/bin/olo-session\0--headless\0"));
        assert!(is_session_cmdline(b"olo-session\0"));
        assert!(is_session_cmdline(b"/opt/bin/tabula-session\0"));
        assert!(!is_session_cmdline(b"/bin/sleep\x0060\0"));
        assert!(!is_session_cmdline(b"/usr/bin/not-olo-session\0"));

        // Finding 33: positive sub-ms remainders round UP to 1 ms so poll never spins.
        assert_eq!(timeout_ms_ceil(Duration::ZERO), 0);
        assert_eq!(timeout_ms_ceil(Duration::from_nanos(1)), 1);
        assert_eq!(timeout_ms_ceil(Duration::from_micros(999)), 1);
        assert_eq!(timeout_ms_ceil(Duration::from_millis(1)), 1);
        assert_eq!(timeout_ms_ceil(Duration::from_micros(1_001)), 2);
    }

    fn thread_cpu_ns() -> u64 {
        let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
        (ts.tv_sec.max(0) as u64).saturating_mul(1_000_000_000).saturating_add(ts.tv_nsec.max(0) as u64)
    }

    /// Finding 27: `write_all_cancellable` MUST NOT spin at 100% CPU when
    /// stdout is full and unread request bytes sit on stdin (both pipe and
    /// Unix stream socket), and MUST still cancel immediately when stdin closes.
    #[test]
    fn review2_w2_write_all_cancellable_no_cpu_spin_with_unread_stdin() {
        use std::os::unix::net::UnixStream;

        // (A) Pipe stdin with unread data + full stdout pipe.
        let mut out_pipe = [-1i32; 2];
        let mut in_pipe = [-1i32; 2];
        assert_eq!(unsafe { libc::pipe2(out_pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) }, 0);
        assert_eq!(unsafe { libc::pipe2(in_pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) }, 0);
        let out_rd = OwnedFd::new(out_pipe[0]);
        let out_wr = OwnedFd::new(out_pipe[1]);
        let in_rd = OwnedFd::new(in_pipe[0]);
        let in_wr = OwnedFd::new(in_pipe[1]);
        let unread = b"{\"op\":\"frame_at\",\"req\":1,\"at\":100}\n";
        assert!(unsafe { libc::write(in_wr.raw(), unread.as_ptr().cast(), unread.len()) } > 0);

        let wr_fd = out_wr.raw();
        let cancel_fd = in_rd.raw();
        let t = std::thread::spawn(move || {
            let big = vec![b'x'; 512 * 1024];
            let cpu0 = thread_cpu_ns();
            let res = write_all_cancellable(wr_fd, &big, &[CancelFd::StdinEof(cancel_fd)]);
            let cpu1 = thread_cpu_ns();
            (res, cpu1.saturating_sub(cpu0))
        });
        std::thread::sleep(Duration::from_millis(120));
        drop(in_wr);
        let (res, cpu_ns) = t.join().unwrap();
        assert!(res.is_err(), "closing stdin must cancel blocked write_all_cancellable");
        assert!(cpu_ns < 25_000_000, "Finding 27: pipe write_all_cancellable spun CPU ({:.1} ms CPU in 120 ms wall) with unread stdin!", cpu_ns as f64 / 1e6);
        drop(out_rd);
        drop(out_wr);
        drop(in_rd);

        // (B) Unix socket stdin with unread data + full stdout pipe.
        let mut out_pipe2 = [-1i32; 2];
        assert_eq!(unsafe { libc::pipe2(out_pipe2.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) }, 0);
        let out_rd2 = OwnedFd::new(out_pipe2[0]);
        let out_wr2 = OwnedFd::new(out_pipe2[1]);
        let (sock_rd, mut sock_wr) = UnixStream::pair().unwrap();
        sock_rd.set_nonblocking(true).unwrap();
        sock_wr.write_all(unread).unwrap();
        let wr_fd2 = out_wr2.raw();
        let cancel_fd2 = sock_rd.as_raw_fd();
        let t2 = std::thread::spawn(move || {
            let big = vec![b'y'; 512 * 1024];
            let cpu0 = thread_cpu_ns();
            let res = write_all_cancellable(wr_fd2, &big, &[CancelFd::StdinEof(cancel_fd2)]);
            let cpu1 = thread_cpu_ns();
            (res, cpu1.saturating_sub(cpu0))
        });
        std::thread::sleep(Duration::from_millis(120));
        drop(sock_wr);
        let (res2, cpu_ns2) = t2.join().unwrap();
        assert!(res2.is_err(), "closing Unix socket stdin must cancel blocked write_all_cancellable");
        assert!(cpu_ns2 < 25_000_000, "Finding 27: socket write_all_cancellable spun CPU ({:.1} ms CPU in 120 ms wall) with unread stdin!", cpu_ns2 as f64 / 1e6);
        drop(out_rd2);
        drop(out_wr2);
        drop(sock_rd);
    }

    /// Finding 43: `fallback_runtime_dir_in` prefers a private owned `/run/user/<uid>`,
    /// rejects world-readable or symlinked dirs, and falls back to `0700` under
    /// `/var/tmp` (never `/tmp`).
    #[test]
    fn review2_w3_runtime_dir_fallback_prefers_private_run_user_and_never_uses_tmp() {
        use std::os::unix::fs::PermissionsExt as _;

        let uid = unsafe { libc::geteuid() };
        let base = PathBuf::from(format!("/var/tmp/olo-r2-w3-rt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let good_run = base.join("good-run");
        let wide_run = base.join("wide-run");
        let missing_run = base.join("missing-run");
        let var_tmp = base.join("vartmp");
        std::fs::create_dir_all(&good_run).unwrap();
        std::fs::set_permissions(&good_run, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir_all(&wide_run).unwrap();
        std::fs::set_permissions(&wide_run, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir_all(&var_tmp).unwrap();

        assert_eq!(fallback_runtime_dir_in(uid, &good_run, &var_tmp).unwrap(), good_run);
        let fb1 = fallback_runtime_dir_in(uid, &wide_run, &var_tmp).unwrap();
        assert_eq!(fb1, var_tmp.join(format!("olo-runtime-{uid}")));
        assert_eq!(std::fs::symlink_metadata(&fb1).unwrap().mode() & 0o777, 0o700);
        let fb2 = fallback_runtime_dir_in(uid, &missing_run, &var_tmp).unwrap();
        assert_eq!(fb2, fb1);

        let real_fb = fallback_runtime_dir_checked().unwrap();
        assert!(!real_fb.starts_with("/tmp"), "fallback runtime dir must never be under /tmp: {}", real_fb.display());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Round 3 Req 1 (hardened in Round 3b): deterministic zero-window proof
    /// using bounded channel rendezvous (zero sleeps, parent sender dropped so a
    /// panicking contender never hangs).
    #[test]
    fn round3_req1_zero_window_lifecycle_lock_hook() {
        use std::sync::mpsc;

        let base = PathBuf::from(format!("/var/tmp/olo-r3-hook-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let rt_dir = base.join("run/olo-screens");
        let state_root = base.join("state/olo/screens");
        let trash_root = base.join("data/Trash");
        std::fs::create_dir_all(&rt_dir).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        std::fs::create_dir_all(&trash_root).unwrap();

        let spawn_session = |name: &str| {
            Command::new("/bin/sleep")
                .arg0("olo-session")
                .arg("60")
                .env("OLO_SCREEN", name)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap()
        };
        let write_rt = |rt: &Path, name: &str, pid: i32, started: u64| {
            std::fs::write(
                rt.join(format!("{name}.json")),
                serde_json::to_vec(&json!({
                    "name": name,
                    "wayland_display": format!("w-{name}"),
                    "olo_socket": rt.join(format!("{name}.sock")),
                    "pid": pid,
                    "width": 800,
                    "height": 600,
                    "scale": 1.0,
                    "sound": false,
                    "started": started
                }))
                .unwrap(),
            )
            .unwrap();
        };

        let paths = GuardedPaths { rt_dirs: std::slice::from_ref(&rt_dir), state_root: &state_root, trash_root: &trash_root };

        // --- 1. Guarded STOP holds NAME.lifecycle.lock across look -> act ---
        let mut s_old = spawn_session("s1");
        write_rt(&rt_dir, "s1", s_old.id() as i32, 1000);
        let rt_for_thread = rt_dir.clone();
        let (nb_tx, nb_rx) = mpsc::channel::<bool>();
        let mut nb_tx = Some(nb_tx);
        let mut contender: Option<std::thread::JoinHandle<std::process::Child>> = None;

        let res = run_guarded_action_with_hook(&paths, GuardedVerb::Stop, "s1", ExpectedScreen { expected_started: Some(1000), expected_last_used: None }, || {
            let rt_c = rt_for_thread.clone();
            let tx = nb_tx.take().unwrap();
            contender = Some(std::thread::spawn(move || {
                let f = std::fs::OpenOptions::new().read(true).write(true).open(rt_c.join("s1.lifecycle.lock")).unwrap();
                let nb_rc = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                let blocked = nb_rc != 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK);
                drop(f);
                tx.send(blocked).unwrap();
                let _guard = acquire_runtime_locks(std::slice::from_ref(&rt_c), "s1", Duration::from_secs(5), true).unwrap();
                let s_new = Command::new("/bin/sleep")
                    .arg0("olo-session")
                    .arg("60")
                    .env("OLO_SCREEN", "s1")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .unwrap();
                std::fs::write(
                    rt_c.join("s1.json"),
                    serde_json::to_vec(&json!({
                        "name": "s1",
                        "wayland_display": "w-s1",
                        "olo_socket": rt_c.join("s1.sock"),
                        "pid": s_new.id() as i32,
                        "width": 800,
                        "height": 600,
                        "scale": 1.0,
                        "sound": false,
                        "started": 2000
                    }))
                    .unwrap(),
                )
                .unwrap();
                s_new
            }));
            assert!(nb_rx.recv_timeout(Duration::from_secs(5)).unwrap(), "NAME.lifecycle.lock must be held between look and act");
        });
        assert_eq!(res, Ok(None));
        let _ = s_old.wait();
        let mut s_new = contender.unwrap().join().unwrap();
        let cur = read_runtime_screen(&rt_dir.join("s1.json")).expect("replacement s1.json must survive");
        assert_eq!(cur.started, Some(2000));
        assert!(screen_pid_running("s1", s_new.id() as i32));
        let stale =
            run_guarded_action_with_hook(&paths, GuardedVerb::Stop, "s1", ExpectedScreen { expected_started: Some(1000), expected_last_used: None }, || panic!("hook must not run when check fails"));
        assert_eq!(stale, Err(GuardedError::Stale("s1 was started again since you asked".into())));
        let _ = s_new.kill();
        let _ = s_new.wait();
        let _ = std::fs::remove_file(rt_dir.join("s1.json"));

        // --- 2. Guarded CLEAR HISTORY holds NAME.lifecycle.lock across look -> act ---
        let s2_dir = state_root.join("s2");
        std::fs::create_dir_all(&s2_dir).unwrap();
        std::fs::write(s2_dir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        std::fs::write(s2_dir.join("actions.jsonl"), "{\"at\":5000,\"ms\":10,\"kind\":\"click\"}\n").unwrap();
        let (nb_tx2, nb_rx2) = mpsc::channel::<bool>();
        let mut nb_tx2 = Some(nb_tx2);
        let mut contender2: Option<std::thread::JoinHandle<()>> = None;
        let rt_c2 = rt_dir.clone();
        let st_c2 = s2_dir.clone();
        let res2 = run_guarded_action_with_hook(&paths, GuardedVerb::ClearHistory, "s2", ExpectedScreen { expected_started: Some(0), expected_last_used: Some(5010) }, || {
            let tx = nb_tx2.take().unwrap();
            contender2 = Some(std::thread::spawn(move || {
                let f = std::fs::OpenOptions::new().read(true).write(true).open(rt_c2.join("s2.lifecycle.lock")).unwrap();
                let blocked = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK);
                drop(f);
                tx.send(blocked).unwrap();
                let _guard = acquire_runtime_locks(std::slice::from_ref(&rt_c2), "s2", Duration::from_secs(5), true).unwrap();
                std::fs::write(st_c2.join("actions.jsonl"), "{\"at\":9999,\"ms\":1,\"kind\":\"start\"}\n").unwrap();
            }));
            assert!(nb_rx2.recv_timeout(Duration::from_secs(5)).unwrap(), "clear must hold NAME.lifecycle.lock across look -> act");
        });
        assert_eq!(res2, Ok(None));
        contender2.unwrap().join().unwrap();
        let dfd2 = open_verified_subdir(&state_root, "s2").unwrap();
        assert!(matches!(probe_last_used_at(&dfd2), LastUsedProbe::Value { last_used: 10_000, .. }));

        // --- 3. Guarded DELETE holds NAME.lifecycle.lock across look -> act ---
        let (nb_tx3, nb_rx3) = mpsc::channel::<bool>();
        let mut nb_tx3 = Some(nb_tx3);
        let mut contender3: Option<std::thread::JoinHandle<()>> = None;
        let rt_c3 = rt_dir.clone();
        let st_c3 = s2_dir.clone();
        let res3 = run_guarded_action_with_hook(&paths, GuardedVerb::Delete, "s2", ExpectedScreen { expected_started: Some(0), expected_last_used: Some(10_000) }, || {
            let tx = nb_tx3.take().unwrap();
            contender3 = Some(std::thread::spawn(move || {
                let f = std::fs::OpenOptions::new().read(true).write(true).open(rt_c3.join("s2.lifecycle.lock")).unwrap();
                let blocked = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK);
                drop(f);
                tx.send(blocked).unwrap();
                let _guard = acquire_runtime_locks(std::slice::from_ref(&rt_c3), "s2", Duration::from_secs(5), true).unwrap();
                std::fs::create_dir_all(&st_c3).unwrap();
                std::fs::write(st_c3.join("title"), "Reborn\n").unwrap();
            }));
            assert!(nb_rx3.recv_timeout(Duration::from_secs(5)).unwrap(), "delete must hold NAME.lifecycle.lock across look -> act");
        });
        assert_eq!(res3, Ok(None));
        contender3.unwrap().join().unwrap();
        assert!(trash_root.join("files/s2").is_dir());
        assert_eq!(std::fs::read_to_string(s2_dir.join("title")).unwrap().trim(), "Reborn");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Round 3b Findings 2, 3, 9, 11, 14 (hardened for Final Items 1, 8, T1):
    /// - F2: Cross-device (`EXDEV`) Trash move fails with source 100% intact and removes ONLY our `.trashinfo`.
    /// - F3: Concurrent Tracker placement at `Trash/files/<stem>` hits `EEXIST` on `renameat2(RENAME_NOREPLACE)`,
    ///   never deletes Tracker's folder, unlinks only our unmodified `.trashinfo`, and retries at `<stem>.2`.
    /// - F9: Dual runtime dirs (`olo-screens` + `tabula-screens`) lock BOTH in canonical order under ONE shared
    ///   total deadline and remove ONLY the target runtime's `owner.json`.
    /// - F11 / Item 1: Session with `OLO_APP_SERVER_SETTINGS` in `rt_old` referenced from `rt_new/mig.json`
    ///   returns `RunningProbe::Unverifiable` (fail closed, NEVER aliases `s_mig` and NEVER classifies as stopped!).
    /// - F14 / Item 8 / T1: Swapping `state/NAME` at the EXACT rename boundary (`before_stage`) detects the
    ///   staged inode mismatch and restores the replacement via `RENAME_NOREPLACE` (retaining both if occupied!).
    #[test]
    fn round3b_f2_f3_f9_f11_f14_trash_exdev_tracker_race_dual_rt_and_dir_swap() {
        let base = PathBuf::from(format!("/var/tmp/olo-r3b-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let rt_new = base.join("run/olo-screens");
        let rt_old = base.join("run/tabula-screens");
        let state_root = base.join("state/olo/screens");
        let trash_root = base.join("data/Trash");
        std::fs::create_dir_all(&rt_new).unwrap();
        std::fs::create_dir_all(&rt_old).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        std::fs::create_dir_all(&trash_root).unwrap();

        // --- F2: EXDEV preserves source 100% and leaves no orphan .trashinfo ---
        let exdev_dir = state_root.join("exdev_s");
        std::fs::create_dir_all(&exdev_dir).unwrap();
        std::fs::write(exdev_dir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        std::fs::write(exdev_dir.join("important.dat"), b"keep-me").unwrap();
        let rfd = open_verified_owned_dir(&state_root, false).unwrap();
        let dev_ino = stat_at_nofollow(&rfd, "exdev_s").unwrap();
        let err = trash_screen_pinned_with_hooks(&state_root, &rfd, dev_ino, &trash_root, "exdev_s", || {}, |_| Some(io::Error::from_raw_os_error(libc::EXDEV)));
        assert!(err.is_err(), "F2: EXDEV must return error");
        assert_eq!(std::fs::read(exdev_dir.join("important.dat")).unwrap(), b"keep-me", "F2: source must remain intact");
        assert!(!trash_root.join("info/exdev_s.trashinfo").exists(), "F2: our .trashinfo must be cleaned up");

        // --- F3: Tracker places a non-empty folder at `Trash/files/race_s` right before `renameat2`! ---
        let race_dir = state_root.join("race_s");
        std::fs::create_dir_all(&race_dir).unwrap();
        std::fs::write(race_dir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        std::fs::write(race_dir.join("ours.txt"), b"screen-data").unwrap();
        let race_id = stat_at_nofollow(&rfd, "race_s").unwrap();
        let tracker_dir = trash_root.join("files/race_s");
        let mut injected = false;
        trash_screen_pinned_with_hooks(
            &state_root,
            &rfd,
            race_id,
            &trash_root,
            "race_s",
            || {},
            |stem| {
                if stem == "race_s" && !injected {
                    injected = true;
                    std::fs::create_dir_all(&tracker_dir).unwrap();
                    std::fs::write(tracker_dir.join("tracker_precious.txt"), b"do-not-delete").unwrap();
                }
                None
            },
        )
        .expect("F3: renameat2(RENAME_NOREPLACE) must retry at race_s.2");
        assert_eq!(std::fs::read(tracker_dir.join("tracker_precious.txt")).unwrap(), b"do-not-delete", "F3: Tracker's folder must NEVER be deleted!");
        assert!(!trash_root.join("info/race_s.trashinfo").exists(), "F3: orphaned slot-1 trashinfo must be unlinked");
        assert_eq!(std::fs::read(trash_root.join("files/race_s.2/ours.txt")).unwrap(), b"screen-data", "F3: our screen must land in race_s.2");
        assert!(trash_root.join("info/race_s.2.trashinfo").is_file());

        // --- F9 & F11 / Item 1: Dual runtime lock ordering + runtime-bound session identity ---
        let mut s_mig = Command::new("/bin/sleep")
            .arg0("olo-session")
            .arg("60")
            .env("OLO_SCREEN", "mig")
            .env("OLO_APP_SERVER_SETTINGS", rt_old.join("mig-settings.json"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        // Item 1 / F11: Putting `s_mig`'s PID into `rt_new/mig.json` (WRONG runtime dir!) MUST fail closed
        // with `RunningProbe::Unverifiable` (NEVER aliasing `s_mig` as `Running` AND NEVER classifying as `Stopped`!).
        let desc = json!({
            "name": "mig", "wayland_display": "w-mig", "olo_socket": rt_old.join("mig.sock"),
            "pid": s_mig.id() as i32, "width": 800, "height": 600, "scale": 1.0, "sound": false, "started": 7000
        });
        std::fs::write(rt_new.join("mig.json"), serde_json::to_vec(&desc).unwrap()).unwrap();
        let locked_new_only = acquire_runtime_locks(std::slice::from_ref(&rt_new), "mig", Duration::from_secs(2), false).unwrap();
        assert!(
            matches!(probe_running_in_locked_rts(&locked_new_only, "mig"), RunningProbe::Unverifiable(_)),
            "Item 1 / F11: wrong-runtime descriptor for live session must fail closed as Unverifiable"
        );
        drop(locked_new_only);
        std::fs::remove_file(rt_new.join("mig.json")).unwrap();

        // F9: Put descriptor in `rt_old` while BOTH `rt_new` and `rt_old` exist + owner markers in both.
        std::fs::write(rt_old.join("mig.json"), serde_json::to_vec(&desc).unwrap()).unwrap();
        std::fs::write(rt_old.join("mig.owner.json"), b"{}").unwrap();
        std::fs::write(rt_new.join("mig.owner.json"), b"{}").unwrap();
        // Hold `rt_old/mig.lifecycle.lock` -> dual-runtime guarded stop MUST time out busy within ONE shared deadline.
        let old_lock_f = std::fs::OpenOptions::new().create(true).read(true).write(true).truncate(false).open(rt_old.join("mig.lifecycle.lock")).unwrap();
        assert_eq!(unsafe { libc::flock(old_lock_f.as_raw_fd(), libc::LOCK_EX) }, 0);
        let dual_rts = vec![rt_new.clone(), rt_old.clone()];
        let t_lock = Instant::now();
        let busy = acquire_runtime_locks(&dual_rts, "mig", Duration::from_millis(80), true);
        assert!(matches!(busy, Err(GuardedError::Other(ref m)) if m.contains("busy")), "F9: must contend on tabula-screens lock too");
        assert!(t_lock.elapsed() < Duration::from_millis(500), "Item 6: dual runtime lock must share one deadline");
        drop(old_lock_f);
        // Now stop `mig`: removes `rt_old/mig.owner.json`, leaves `rt_new/mig.owner.json` untouched!
        let paths_dual = GuardedPaths { rt_dirs: &dual_rts, state_root: &state_root, trash_root: &trash_root };
        run_guarded_action_with_hook(&paths_dual, GuardedVerb::Stop, "mig", ExpectedScreen { expected_started: Some(7000), expected_last_used: None }, || {}).unwrap();
        let _ = s_mig.wait();
        assert!(!rt_old.join("mig.owner.json").exists(), "F9: target runtime owner must be removed");
        assert!(rt_new.join("mig.owner.json").exists(), "F9: other runtime's owner.json must NOT be touched");

        // --- F14 / Item 8 / T1: Swapped screen directory at the EXACT rename boundary ---
        // (A) Clear operates ONLY on originally pinned `dir_fd`, leaving swapped replacement untouched!
        let swap_a = state_root.join("swap_clr");
        let swap_aside = base.join("swap_clr_old");
        std::fs::create_dir_all(&swap_a).unwrap();
        std::fs::write(swap_a.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        std::fs::write(swap_a.join("actions.jsonl"), "{\"at\":4000,\"ms\":10,\"kind\":\"click\"}\n").unwrap();
        let paths_single = GuardedPaths { rt_dirs: std::slice::from_ref(&rt_new), state_root: &state_root, trash_root: &trash_root };
        run_guarded_action_with_hook(&paths_single, GuardedVerb::ClearHistory, "swap_clr", ExpectedScreen { expected_started: Some(0), expected_last_used: Some(4010) }, || {
            std::fs::rename(&swap_a, &swap_aside).unwrap();
            std::fs::create_dir_all(&swap_a).unwrap();
            std::fs::write(swap_a.join("actions.jsonl"), "{\"at\":99999,\"ms\":1,\"kind\":\"click\"}\n").unwrap();
        })
        .unwrap();
        assert!(!swap_aside.join("actions.jsonl").exists(), "F14: originally pinned dir should be cleared");
        assert!(swap_a.join("actions.jsonl").exists(), "F14: swapped replacement dir's actions.jsonl must NOT be cleared!");

        // (B) Delete staging rename: swap `state/swap_del` RIGHT AT `before_stage` (the rename boundary!),
        // verifying staged inode mismatch restores replacement B intact without trashing B!
        let swap_d = state_root.join("swap_del");
        let swap_d_old = base.join("swap_del_old");
        std::fs::create_dir_all(&swap_d).unwrap();
        std::fs::write(swap_d.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        let orig_id = stat_at_nofollow(&rfd, "swap_del").unwrap();
        let del_res = trash_screen_pinned_with_hooks(
            &state_root,
            &rfd,
            orig_id,
            &trash_root,
            "swap_del",
            || {
                std::fs::rename(&swap_d, &swap_d_old).unwrap();
                std::fs::create_dir_all(&swap_d).unwrap();
                std::fs::write(swap_d.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
                std::fs::write(swap_d.join("precious.txt"), b"replacement").unwrap();
            },
            |_| None,
        );
        assert!(matches!(del_res, Err(GuardedError::Stale(_))), "Item 8 / T1: rename-boundary swap must fail Delete with Stale, got {del_res:?}");
        assert_eq!(std::fs::read(swap_d.join("precious.txt")).unwrap(), b"replacement", "Item 8 / T1: replacement directory must be restored intact!");
        assert!(!trash_root.join("files/swap_del").exists(), "Item 8 / T1: replacement directory must NOT appear in Trash!");

        // (C) Delete EXDEV rollback when `name` is ALREADY occupied by a new dir C:
        //     `RENAME_NOREPLACE` restore MUST retain BOTH C at `name` AND our original dir at `.delete-stage-*`!
        let occ_d = state_root.join("occ_del");
        std::fs::create_dir_all(&occ_d).unwrap();
        std::fs::write(occ_d.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        std::fs::write(occ_d.join("orig.txt"), b"original-data").unwrap();
        let occ_id = stat_at_nofollow(&rfd, "occ_del").unwrap();
        let occ_res = trash_screen_pinned_with_hooks(
            &state_root,
            &rfd,
            occ_id,
            &trash_root,
            "occ_del",
            || {},
            |_| {
                std::fs::create_dir_all(&occ_d).unwrap();
                std::fs::write(occ_d.join("new_c.txt"), b"occupant-c").unwrap();
                Some(io::Error::from_raw_os_error(libc::EXDEV))
            },
        );
        assert!(occ_res.is_err(), "Item 8 / T1: EXDEV must return error");
        assert_eq!(std::fs::read(occ_d.join("new_c.txt")).unwrap(), b"occupant-c", "Item 8 / T1: occupant C must remain intact!");
        let kept_stage = std::fs::read_dir(&state_root)
            .unwrap()
            .flatten()
            .find(|e| e.file_name().to_string_lossy().starts_with(".delete-stage-occ_del-"))
            .expect("Item 8 / T1: original staged dir must be retained when name is occupied!");
        assert_eq!(std::fs::read(kept_stage.path().join("orig.txt")).unwrap(), b"original-data");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Final Items 5, 6, 7, 8 & 10 unit suite:
    /// - Item 5: control lock retained across mutation; Clear staging rolls back ALL frames and exits Stale (75)
    ///   when an external writer appends to `actions.jsonl` before Clear commits.
    /// - Item 6: `send_home_best_effort_until` respects ONE absolute deadline on a slow/unterminated socket;
    ///   `library_dirs_from` ignores a FIFO `.conf` file without hanging.
    /// - Item 7: `stop_screen_in_process` writes `"stop"` into the originally pinned state dir fd even if
    ///   `state_root/NAME` was swapped aside during shutdown.
    /// - Item 8: `open_or_create_private_dir_fd` enforces `(mode & 0o077) == 0` (tightens `0755` to `0700`)
    ///   and rejects a group-writable ancestor (`0775`).
    /// - Item 10: `ActionLineScanner` matches serde semantics (`\u0061t`, surrogate pairs, rejects invalid
    ///   `{"at":100,"ms":0,"kind":}`, missing `kind`/`ms`, trailing comma, duplicate key, non-string `args`).
    #[test]
    fn final_item8_item10_private_mode_077_and_schema_scanner() {
        use std::os::unix::fs::PermissionsExt as _;
        use std::os::unix::net::UnixListener;

        let base = PathBuf::from(format!("/var/tmp/olo-final-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        // 1. Item 8 + agy-r1 F1: pre-existing 0755 fallback dir tightened to 0700;
        //    world-writable `0777` ancestor and non-UPG shared-group `0775` rejected.
        let fb = base.join("preexisting-0755");
        std::fs::create_dir_all(&fb).unwrap();
        std::fs::set_permissions(&fb, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _fd = open_or_create_private_dir_fd(&fb).unwrap();
        let mode = std::fs::symlink_metadata(&fb).unwrap().mode() & 0o777;
        assert_eq!(mode & 0o077, 0, "Item 8: pre-existing 0755 fallback dir was not tightened to 0700 (mode={mode:#o})!");

        let ww_anc = base.join("ww-anc");
        let ww_child = ww_anc.join("rt");
        std::fs::create_dir_all(&ww_anc).unwrap();
        std::fs::set_permissions(&ww_anc, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(open_or_create_private_dir_fd(&ww_child).is_err(), "Item 8: world-writable ancestor must be rejected");
        std::fs::set_permissions(&ww_anc, std::fs::Permissions::from_mode(0o755)).unwrap();
        let uid = unsafe { libc::geteuid() };
        if uid != 0 {
            assert!(!uid_owned_dir_mode_safe(uid, uid, 0, 0o775, false), "agy-r1 F1: shared root-group 0775 outside 0700 ancestor must be rejected");
        }

        // 2. Item 10: schema-correct `ActionLineScanner` vs serde semantics.
        let scan_one = |line: &[u8]| -> LineAction {
            let mut sc = ActionLineScanner::default();
            let mut res = LineAction::Empty;
            sc.feed_lines(line, |r| res = r);
            res
        };
        assert_eq!(scan_one(b"{\"\\u0061t\":200,\"\\u006ds\":5,\"\\u006bind\":\"click\"}\n"), LineAction::Valid(205));
        assert_eq!(scan_one(b"{\"at\":100,\"ms\":10,\"kind\":\"type\",\"args\":[\"hi \\uD83D\\uDE00\"],\"result\":{\"ok\":true},\"error\":null}\n"), LineAction::Valid(110));
        assert_eq!(scan_one(b"{\"at\":0,\"ms\":0,\"kind\":\"oversized\",\"args\":[]}\n"), LineAction::OversizedPlaceholder);
        assert_eq!(scan_one(b"   \t\r\n"), LineAction::Empty);
        for invalid in [
            &b"{\"at\":100,\"ms\":0,\"kind\":}\n"[..],
            &b"{\"at\":100,\"ms\":0,\"kind\":\"click\",}\n"[..],
            &b"{\"at\":100,\"ms\":0}\n"[..],
            &b"{\"at\":100,\"kind\":\"click\"}\n"[..],
            &b"{\"at\":-1,\"ms\":0,\"kind\":\"click\"}\n"[..],
            &b"{\"at\":100.5,\"ms\":0,\"kind\":\"click\"}\n"[..],
            &b"{\"at\":100,\"at\":200,\"ms\":0,\"kind\":\"click\"}\n"[..],
            &b"{\"at\":100,\"ms\":0,\"kind\":\"click\",\"args\":[123]}\n"[..],
            &b"{\"at\":100,\"ms\":0,\"kind\":\"click\",\"error\":42}\n"[..],
            &b"{\"at\":100,\"ms\":0,\"kind\":\"click\"} junk\n"[..],
            &b"{\"at\":100,\"ms\":0,\"kind\":\"\\uD800\"}\n"[..],
        ] {
            assert_eq!(scan_one(invalid), LineAction::Invalid, "Item 10: scanner must reject invalid JSON {:?}", String::from_utf8_lossy(invalid));
        }

        // 3. Item 5: Clear staging rolls back ALL frames and returns Stale (75) if actions.jsonl changes before commit!
        let state_root = base.join("state/olo/screens");
        let cdir = state_root.join("clr_race");
        std::fs::create_dir_all(&cdir).unwrap();
        std::fs::write(cdir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        std::fs::write(cdir.join("actions.jsonl"), "{\"at\":5000,\"ms\":10,\"kind\":\"click\"}\n").unwrap();
        std::fs::write(cdir.join("frames-0000000000000001.olof"), b"FRAME1").unwrap();
        std::fs::write(cdir.join("frames-0000000000000002.olof"), b"FRAME2").unwrap();
        let dfd = open_verified_subdir(&state_root, "clr_race").unwrap();
        let LastUsedProbe::Value { meta: snap, .. } = probe_last_used_at(&dfd) else { panic!("expected Value") };
        let cdir_hook = cdir.clone();
        let res_clr = clear_history_pinned_with_hook(&dfd, "clr_race", Some(Some(snap)), || {
            let mut f = std::fs::OpenOptions::new().append(true).open(cdir_hook.join("actions.jsonl")).unwrap();
            writeln!(f, "{{\"at\":6000,\"ms\":10,\"kind\":\"click\"}}").unwrap();
        });
        assert!(matches!(res_clr, Err(GuardedError::Stale(_))), "Item 5: concurrent append during Clear must return Stale, got {res_clr:?}");
        assert_eq!(std::fs::read(cdir.join("frames-0000000000000001.olof")).unwrap(), b"FRAME1", "Item 5: frame 1 must be rolled back!");
        assert_eq!(std::fs::read(cdir.join("frames-0000000000000002.olof")).unwrap(), b"FRAME2", "Item 5: frame 2 must be rolled back!");
        assert!(cdir.join("actions.jsonl").exists(), "Item 5: actions.jsonl must survive rollback!");

        // 4. Item 5 & 7: Control lock is RETAINED across Stop (`after_check` proves EWOULDBLOCK on control.lock!),
        //    and swapping `state/pin_stop` aside during `after_check` still writes `"stop"` into the pinned original dir!
        let rt_dir = base.join("run/olo-screens");
        let trash_root = base.join("data/Trash");
        std::fs::create_dir_all(&rt_dir).unwrap();
        std::fs::create_dir_all(&trash_root).unwrap();
        let pdir = state_root.join("pin_stop");
        let pdir_aside = base.join("pin_stop_aside");
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::write(pdir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
        std::fs::write(pdir.join("actions.jsonl"), r#"{"at":3000,"ms":5,"kind":"click"}"#).unwrap();
        let mut s_pin = Command::new("/bin/sleep")
            .arg0("olo-session")
            .arg("60")
            .env("OLO_SCREEN", "pin_stop")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::fs::write(
            rt_dir.join("pin_stop.json"),
            serde_json::to_vec(&json!({
                "name": "pin_stop", "wayland_display": "w-pin", "olo_socket": base.join("run/pin.sock"),
                "pid": s_pin.id() as i32, "width": 800, "height": 600, "scale": 1.0, "sound": false, "started": 7777
            }))
            .unwrap(),
        )
        .unwrap();
        let paths_p = GuardedPaths { rt_dirs: std::slice::from_ref(&rt_dir), state_root: &state_root, trash_root: &trash_root };
        let rt_chk = rt_dir.clone();
        let pdir_c = pdir.clone();
        let pdir_aside_c = pdir_aside.clone();
        let mut ctl_was_locked = false;
        let stop_res = run_guarded_action_with_hook(&paths_p, GuardedVerb::Stop, "pin_stop", ExpectedScreen { expected_started: Some(7777), expected_last_used: Some(3005) }, || {
            // Item 5: control lock MUST STILL be held here (before send_home & Stop)!
            let cf = std::fs::OpenOptions::new().read(true).write(true).open(rt_chk.join("pin_stop.control.lock")).unwrap();
            let rc = unsafe { libc::flock(cf.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            ctl_was_locked = rc != 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK);
            // Item 7: move original folder aside and put a replacement directory at `state/pin_stop`!
            std::fs::rename(&pdir_c, &pdir_aside_c).unwrap();
            std::fs::create_dir_all(&pdir_c).unwrap();
            std::fs::write(pdir_c.join("actions.jsonl"), "{\"at\":99999,\"ms\":1,\"kind\":\"start\"}\n").unwrap();
        });
        let _ = s_pin.wait();
        assert_eq!(stop_res, Ok(None));
        assert!(ctl_was_locked, "Item 5: control.lock must remain held through mutation!");
        let orig_log = std::fs::read_to_string(pdir_aside.join("actions.jsonl")).unwrap();
        assert_eq!(orig_log.lines().count(), 2, "Item 7: stop row must land in originally pinned dir with leading newline: {orig_log:?}");
        assert_eq!(std::fs::read_to_string(pdir.join("actions.jsonl")).unwrap().lines().count(), 1, "Item 7: replacement dir must NOT receive stop row!");

        // 5. Item 6: `send_home_best_effort_until` bounds an unterminated trickling reply to ONE absolute deadline.
        let trickle_sock = base.join("run/trickle.sock");
        let trickle_listener = UnixListener::bind(&trickle_sock).unwrap();
        let trickle_stop = std::sync::Arc::new(AtomicBool::new(false));
        let trickle_stop_c = trickle_stop.clone();
        let t_trickle = std::thread::spawn(move || {
            if let Ok((mut s, _)) = trickle_listener.accept() {
                let mut req = [0u8; 64];
                let _ = s.read(&mut req);
                while !trickle_stop_c.load(Ordering::Relaxed) {
                    if s.write_all(b"x").is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        });
        let fake_screen =
            RuntimeScreen { name: "trickle".into(), wayland_display: "w-trickle".into(), olo_socket: trickle_sock, pid: 1, width: 800, height: 600, scale: 1.0, sound: false, started: Some(1) };
        let t0 = Instant::now();
        send_home_best_effort_until(&base.join("run"), &fake_screen, Instant::now() + Duration::from_millis(200));
        assert!(t0.elapsed() < Duration::from_secs(1), "Item 6: send_home overran its single deadline on trickle socket: {:?}", t0.elapsed());
        trickle_stop.store(true, Ordering::Relaxed);
        let _ = t_trickle.join();

        // 6. Item 10 (F23): `library_dirs_from` must NOT block on a FIFO `.conf` inside `ld.so.conf.d`!
        let conf_d = base.join("ld.so.conf.d");
        std::fs::create_dir_all(&conf_d).unwrap();
        let fifo_conf = conf_d.join("evil.conf");
        let cfifo = std::ffi::CString::new(fifo_conf.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cfifo.as_ptr(), 0o600) }, 0);
        let t_ld = Instant::now();
        let _dirs = library_dirs_from(&base.join("ld.so.conf"), &conf_d);
        assert!(t_ld.elapsed() < Duration::from_secs(1), "Item 10: library_dirs_from blocked on FIFO .conf!");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Round 1 agy-review Finding 1 + Round 2 confirming Finding 3: UPG `0775`
    /// user-owned directories outside a `0700` ancestor are accepted iff the
    /// directory's actual group is a single-user UPG for `uid`, `0775` inside a
    /// `0700` uid-owned ancestor is accepted unconditionally on all accounts
    /// (with private leaves still tightened to `0700`), and `0777` world-writable
    /// or non-UPG shared-group-writable dirs outside `0700` are rejected.
    #[test]
    fn agy_r1_f1_upg_0775_allowed_world_and_shared_group_rejected() {
        use std::os::unix::fs::PermissionsExt as _;

        let base = PathBuf::from(format!("/var/tmp/olo-agy-r1-f1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();

        let uid = unsafe { libc::geteuid() };
        let primary_gid = unsafe { libc::getegid() };

        // Standard Linux UPG (`umask 0002`): `0775` user-owned dir outside any
        // `0700` ancestor is accepted iff its actual group is a single-user UPG.
        let upg = base.join("upg-0775");
        std::fs::create_dir_all(&upg).unwrap();
        std::fs::set_permissions(&upg, std::fs::Permissions::from_mode(0o775)).unwrap();
        let upg_gid = std::fs::symlink_metadata(&upg).unwrap().gid();
        let is_upg = is_private_user_group(uid, upg_gid);

        assert_eq!(open_verified_owned_dir(&upg, false).is_ok(), is_upg, "agy-r1 F1: 0775 dir outside 0700 ancestor acceptance must match is_private_user_group(uid={uid}, gid={upg_gid})={is_upg}");
        let leaf = upg.join("private-leaf");
        assert_eq!(open_or_create_private_dir_fd(&leaf).is_ok(), is_upg, "agy-r1 F1: 0775 ancestor acceptance in open_or_create_private_dir_fd must match is_upg={is_upg}");
        if is_upg {
            let leaf_mode = std::fs::symlink_metadata(&leaf).unwrap().mode() & 0o777;
            assert_eq!(leaf_mode & 0o077, 0, "agy-r1 F1: private leaf must still be tightened to 0700 (got {leaf_mode:#o})");
        }

        // Unconditionally test `0700` private-ancestor enclosure (e.g. `$XDG_RUNTIME_DIR`):
        // a `0775` child inside a `0700` uid-owned ancestor MUST be accepted on both
        // UPG and non-UPG accounts, while private leaves inside it are tightened to `0700`.
        let priv_parent = base.join("priv-0700");
        let enclosed_default = priv_parent.join("inner-0775");
        std::fs::create_dir_all(&enclosed_default).unwrap();
        std::fs::set_permissions(&priv_parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&enclosed_default, std::fs::Permissions::from_mode(0o775)).unwrap();
        assert!(open_verified_owned_dir(&enclosed_default, false).is_ok(), "agy-r1 F1: 0775 inside 0700 uid-owned ancestor must be accepted unconditionally");
        let enclosed_leaf = enclosed_default.join("views");
        assert!(open_or_create_private_dir_fd(&enclosed_leaf).is_ok(), "agy-r1 F1: leaf inside 0700 -> 0775 must be accepted unconditionally");
        let enclosed_leaf_mode = std::fs::symlink_metadata(&enclosed_leaf).unwrap().mode() & 0o777;
        assert_eq!(enclosed_leaf_mode & 0o077, 0, "agy-r1 F1: private leaf inside 0700 -> 0775 must still be tightened to 0700 (got {enclosed_leaf_mode:#o})");

        // World-writable (`0777`) directory and ancestor MUST be rejected!
        let world = base.join("world-0777");
        std::fs::create_dir_all(&world).unwrap();
        std::fs::set_permissions(&world, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(open_verified_owned_dir(&world, false).is_err(), "agy-r1 F1: 0777 dir must be rejected");
        assert!(open_or_create_private_dir_fd(&world.join("child")).is_err(), "agy-r1 F1: 0777 ancestor must be rejected");

        // Non-private shared group (`0775` chgrp'd to a supplementary group if present on this host):
        // rejected outside a `0700` uid-owned ancestor, accepted inside a `0700` uid-owned ancestor!
        let mut groups = [0 as libc::gid_t; 64];
        let ng = unsafe { libc::getgroups(groups.len() as libc::c_int, groups.as_mut_ptr()) };
        if ng > 0
            && let Some(&shared_gid) = groups[..ng as usize].iter().find(|&&g| g != primary_gid && !is_private_user_group(uid, g))
        {
            let shared = base.join("shared-0775");
            std::fs::create_dir_all(&shared).unwrap();
            let cpath = std::ffi::CString::new(shared.as_os_str().as_encoded_bytes()).unwrap();
            assert_eq!(unsafe { libc::chown(cpath.as_ptr(), uid, shared_gid) }, 0);
            std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o775)).unwrap();
            assert!(open_verified_owned_dir(&shared, false).is_err(), "agy-r1 F1: shared-group 0775 dir must be rejected");
            assert!(open_or_create_private_dir_fd(&shared.join("child")).is_err(), "agy-r1 F1: shared-group 0775 ancestor must be rejected");

            // Inside a `0700` uid-owned ancestor (e.g. `$XDG_RUNTIME_DIR`), a `0775` shared-group child IS enclosed & safe!
            let enclosed_shared = priv_parent.join("inner-shared-0775");
            std::fs::create_dir_all(&enclosed_shared).unwrap();
            let cenc = std::ffi::CString::new(enclosed_shared.as_os_str().as_encoded_bytes()).unwrap();
            assert_eq!(unsafe { libc::chown(cenc.as_ptr(), uid, shared_gid) }, 0);
            std::fs::set_permissions(&enclosed_shared, std::fs::Permissions::from_mode(0o775)).unwrap();
            assert!(open_verified_owned_dir(&enclosed_shared, false).is_ok(), "agy-r1 F1: shared-group 0775 inside 0700 uid-owned ancestor must be accepted");
            assert!(open_or_create_private_dir_fd(&enclosed_shared.join("views")).is_ok(), "agy-r1 F1: leaf inside 0700 -> shared-group 0775 must be accepted");
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Round 1 agy-review Finding 2 + Round 2 confirming Finding 1:
    /// `ActionLineScanner::feed_lines` must scan strings with a trailing UTF-8
    /// character OR repeated multi-byte UTF-8 characters in strictly linear O(N) time.
    #[test]
    fn agy_r1_f2_non_ascii_string_scan_is_linear() {
        let cases: [(&str, Vec<u8>); 3] = [
            ("192 KiB ASCII + trailing UTF-8", {
                let mut v = Vec::with_capacity(200 * 1024);
                v.extend_from_slice(b"{\"args\":[\"");
                v.resize(v.len() + 192 * 1024, b'a');
                v.extend_from_slice("→end\"],\"at\":12345,\"ms\":5,\"kind\":\"type\"}\n".as_bytes());
                v
            }),
            ("repeated ASCII+UTF-8 (a→ * 48 Ki)", {
                let body = "a→".repeat(48 * 1024);
                format!("{{\"args\":[\"{body}\"],\"at\":12345,\"ms\":5,\"kind\":\"type\"}}\n").into_bytes()
            }),
            ("repeated multi-byte UTF-8 (→ * 64 Ki)", {
                let body = "→".repeat(64 * 1024);
                format!("{{\"args\":[\"{body}\"],\"at\":12345,\"ms\":5,\"kind\":\"type\"}}\n").into_bytes()
            }),
        ];

        for (label, line) in cases {
            let t0 = Instant::now();
            let mut sc = ActionLineScanner::default();
            let mut got = LineAction::Empty;
            sc.feed_lines(&line, |r| got = r);
            let elapsed = t0.elapsed();
            assert_eq!(got, LineAction::Valid(12350), "agy-r1/r2 F1: UTF-8 record ({label}) must parse validly");
            assert!(elapsed < Duration::from_millis(200), "agy-r1/r2 F1: quadratic non-ASCII string scan ({label}) took {elapsed:?} for {} bytes!", line.len());
        }
    }

    /// Round 1 agy-review Finding 3: `probe_last_used_from_file_budgeted` must
    /// reach an 80 KiB final record inside a 512 KiB budget without wasting the
    /// remaining budget after the 8 KiB & 64 KiB windows.
    #[test]
    fn agy_r1_f3_budgeted_tail_probe_reaches_large_record_within_512k_budget() {
        let base = PathBuf::from(format!("/var/tmp/olo-agy-r1-f3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let act = base.join("actions.jsonl");
        {
            let mut f = std::fs::File::create(&act).unwrap();
            let filler = "{\"at\":1000,\"ms\":1,\"kind\":\"click\",\"args\":[\"padding-0123456789abcdef\"]}\n".repeat(16 * 1024);
            f.write_all(filler.as_bytes()).unwrap(); // > 1 MiB
            let big_arg = "x".repeat(80 * 1024);
            writeln!(f, "{{\"at\":88888,\"ms\":12,\"kind\":\"type\",\"args\":[\"{big_arg}\"]}}").unwrap();
        }
        let mut f = std::fs::File::open(&act).unwrap();
        let len = f.metadata().unwrap().len();
        let mut budget = 512 * 1024u64;
        let mut read_bytes = 0u64;
        let res = probe_last_used_from_file_budgeted(&mut f, len, &mut budget, &mut read_bytes);
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(res, BudgetedTailProbe::Value(88900), "agy-r1 F3: 80 KiB tail record must succeed within 512 KiB budget");
        assert!(read_bytes <= 512 * 1024, "agy-r1 F3: exceeded 512 KiB budget (read {read_bytes})");
    }

    /// Round 1 agy-review Finding 4: `LineAction::OversizedPlaceholder` must NOT
    /// be misclassified as `Unreadable` or abort window expansion in either
    /// `probe_last_used_at` or `probe_last_used_from_file_budgeted`.
    #[test]
    fn agy_r1_f4_oversized_placeholder_absent_and_expands_window() {
        let base = PathBuf::from(format!("/var/tmp/olo-agy-r1-f4-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let sdir = base.join("s_over");
        std::fs::create_dir_all(&sdir).unwrap();
        let act = sdir.join("actions.jsonl");

        // Case 1: Only `OversizedPlaceholder` -> Absent (NOT Unreadable!).
        std::fs::write(&act, "{\"at\":0,\"ms\":0,\"kind\":\"oversized\",\"args\":[]}\n").unwrap();
        let dfd = open_verified_subdir(&base, "s_over").unwrap();
        assert!(matches!(probe_last_used_at(&dfd), LastUsedProbe::Absent { meta: Some(_) }), "agy-r1 F4: placeholder-only file must be Absent, got {:?}", probe_last_used_at(&dfd));
        {
            let mut f = std::fs::File::open(&act).unwrap();
            let len = f.metadata().unwrap().len();
            let mut budget = 64 * 1024;
            let mut read_b = 0;
            assert_eq!(probe_last_used_from_file_budgeted(&mut f, len, &mut budget, &mut read_b), BudgetedTailProbe::Absent, "agy-r1 F4: budgeted placeholder-only file must be Absent");
        }

        // Case 2: Valid row (1005), then >64 KiB of placeholders -> MUST expand past 64 KiB window and return 1005!
        {
            let mut f = std::fs::File::create(&act).unwrap();
            writeln!(f, "{{\"at\":1000,\"ms\":5,\"kind\":\"click\"}}").unwrap();
            let ph = "{\"at\":0,\"ms\":0,\"kind\":\"oversized\",\"args\":[]}\n";
            for _ in 0..1600 {
                f.write_all(ph.as_bytes()).unwrap(); // ~75 KiB of placeholders
            }
        }
        assert!(
            matches!(probe_last_used_at(&dfd), LastUsedProbe::Value { last_used: 1005, .. }),
            "agy-r1 F4: trailing placeholders >64 KiB must expand window to find 1005, got {:?}",
            probe_last_used_at(&dfd)
        );
        {
            let mut f = std::fs::File::open(&act).unwrap();
            let len = f.metadata().unwrap().len();
            let mut budget = 512 * 1024;
            let mut read_b = 0;
            assert_eq!(
                probe_last_used_from_file_budgeted(&mut f, len, &mut budget, &mut read_b),
                BudgetedTailProbe::Value(1005),
                "agy-r1 F4: budgeted trailing placeholders >64 KiB must expand window to find 1005"
            );
        }

        // Case 3: Truly invalid trailing line still fails closed (`Unreadable`).
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&act).unwrap();
            f.write_all(b"{\"at\":2000,\"ms\":0,\"kind\":}\n").unwrap();
        }
        assert!(matches!(probe_last_used_at(&dfd), LastUsedProbe::Unreadable(_)));

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Round 1 agy-review Finding 5: forced stop MUST resume (`SIGCONT`) a
    /// previously-running supervisor if descendant inspection fails, AND MUST
    /// fail closed (never skipping descendant cleanup) if `session_start_ticks`
    /// is unavailable while the supervisor is still alive.
    #[test]
    fn agy_r1_f5_forced_stop_resumes_sigstop_on_error_and_fails_closed_without_start_ticks() {
        // Part A: Running child -> descendant scan error -> MUST NOT stay SIGSTOP'd (`T`)!
        let mut child_a = Command::new("/bin/sleep").arg("60").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let pid_a = child_a.id() as i32;
        let pidfd_a = pidfd_open_result(pid_a).unwrap();
        let ticks_a = proc_start_ticks(pid_a).unwrap();

        let err_a = force_stop_supervisor_and_descendants("f5a", pid_a, Some(ticks_a), &pidfd_a, proc_state_ppid_start, |_| Err("injected EMFILE".into()));
        assert!(err_a.is_err());

        // Poll briefly for state to settle away from `T`/`t` after SIGCONT.
        let until = Instant::now() + Duration::from_millis(500);
        let mut final_state = b'T';
        while Instant::now() < until {
            if let Some((st, _, _)) = proc_state_ppid_start(pid_a) {
                final_state = st;
                if !matches!(st, b'T' | b't') {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Clean up child_a before asserting.
        let _ = signal_pidfd(&pidfd_a, libc::SIGKILL);
        let _ = child_a.wait();
        assert!(!matches!(final_state, b'T' | b't'), "agy-r1 F5: supervisor was left permanently SIGSTOP'd (state={}) after descendant-scan error!", final_state as char);

        // Part B: Already-`SIGSTOP`'d supervisor -> descendant-scan error MUST preserve `T` state!
        let mut child_s = Command::new("/bin/sleep").arg("60").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let pid_s = child_s.id() as i32;
        let pidfd_s = pidfd_open_result(pid_s).unwrap();
        let ticks_s = proc_start_ticks(pid_s).unwrap();
        signal_pidfd(&pidfd_s, libc::SIGSTOP).unwrap();
        let wait_t = Instant::now() + Duration::from_millis(500);
        while Instant::now() < wait_t {
            if proc_state_ppid_start(pid_s).is_some_and(|(st, _, _)| matches!(st, b'T' | b't')) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let err_s = force_stop_supervisor_and_descendants("f5s", pid_s, Some(ticks_s), &pidfd_s, proc_state_ppid_start, |_| Err("injected EMFILE".into()));
        assert!(err_s.is_err());
        let stayed_stopped = proc_state_ppid_start(pid_s).is_some_and(|(st, _, _)| matches!(st, b'T' | b't'));
        let _ = signal_pidfd(&pidfd_s, libc::SIGKILL);
        let _ = child_s.wait();
        assert!(stayed_stopped, "agy-r1 F5: already-stopped supervisor must remain stopped on error");

        // Part C: Missing `session_start_ticks` (and unavailable `/proc` stat) -> MUST fail closed
        // and MUST NOT `SIGKILL` the supervisor without scanning its descendants!
        let mut child_b = Command::new("/bin/sleep").arg("60").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let pid_b = child_b.id() as i32;
        let pidfd_b = pidfd_open_result(pid_b).unwrap();

        let err_b = force_stop_supervisor_and_descendants("f5b", pid_b, None, &pidfd_b, |_| None, |_| Ok(Vec::new()));
        let still_alive = !pidfd_exited(&pidfd_b);
        let _ = signal_pidfd(&pidfd_b, libc::SIGKILL);
        let _ = child_b.wait();
        assert!(err_b.is_err(), "agy-r1 F5: missing session_start_ticks must fail closed, got {err_b:?}");
        assert!(still_alive, "agy-r1 F5: missing session_start_ticks silently SIGKILL'd supervisor without scanning descendants!");
    }
}
