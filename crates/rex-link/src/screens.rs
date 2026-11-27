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

use std::io::{self, Read as _, Write as _};
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::{DirBuilderExt as _, FileExt as _, MetadataExt as _, OpenOptionsExt as _};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use olo_rex_link::screen_wire::{OwnerPane, PROTO, ScreenOwner, valid_screen_name};
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
    #[serde(default)]
    pub started: u64,
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

/// True when `dir` is an existing real directory (not a symlink) owned by our
/// effective user ID. (Per-operation scratch directories additionally enforce
/// mode `0700` via [`TrustedTempDir`].)
pub(crate) fn verify_owned_dir(dir: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(dir) else { return false };
    // SAFETY: `geteuid()` has no preconditions.
    let uid = unsafe { libc::geteuid() };
    meta.file_type().is_dir() && meta.uid() == uid
}

/// Open or create `dir` as a real directory (`O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC`)
/// owned by `geteuid()`, tightening permissions to `0700` via `fchmod` if needed
/// and verifying no group/other write bits remain (Finding 14).
fn open_or_create_private_dir_fd(dir: &Path) -> io::Result<OwnedFd> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;
    // SAFETY: `geteuid()` has no preconditions.
    let uid = unsafe { libc::geteuid() };
    // Verify existing non-root ancestors are not world-writable without sticky bit
    // or owned by another non-root user.
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
    if (st.st_mode & 0o022) != 0 {
        if unsafe { libc::fchmod(fd.raw(), 0o700) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::fstat(fd.raw(), &mut st) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if (st.st_mode & 0o022) != 0 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("scratch directory {} remains writable by others", dir.display())));
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
/// (matching `crates/screen-pane/src/screens.rs::fallback_runtime_dir`, Finding 43):
/// prefers a real non-symlink `/run/user/<uid>` owned by `<uid>` with private
/// permissions `(mode & 0o077) == 0`, else private `0700` `/var/tmp/olo-runtime-<uid>`
/// (never `/tmp`).
fn fallback_runtime_dir_in(uid: libc::uid_t, std_run: &Path, var_tmp_dir: &Path) -> PathBuf {
    if std::fs::symlink_metadata(std_run).is_ok_and(|m| m.file_type().is_dir() && m.uid() == uid && (m.mode() & 0o077) == 0) {
        return std_run.to_path_buf();
    }
    let fallback = var_tmp_dir.join(format!("olo-runtime-{uid}"));
    let _ = open_or_create_private_dir_fd(&fallback);
    fallback
}

pub(crate) fn fallback_runtime_dir() -> PathBuf {
    // SAFETY: `geteuid()` has no preconditions.
    let uid = unsafe { libc::geteuid() };
    fallback_runtime_dir_in(uid, Path::new(&format!("/run/user/{uid}")), Path::new("/var/tmp"))
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(fallback_runtime_dir)
}

/// Where running screens keep their runtime descriptors (`NAME.json`,
/// `NAME.control`, `NAME.owner.json`), matching `apps/screen/src/screens.rs::dir`.
pub(crate) fn screens_runtime_dir() -> PathBuf {
    let rt = runtime_dir();
    let new = rt.join("olo-screens");
    if std::fs::symlink_metadata(&new).is_ok() {
        return new;
    }
    let old = rt.join("tabula-screens");
    if std::fs::symlink_metadata(&old).is_ok() {
        return old;
    }
    new
}

/// Both runtime directory names `follow` watches from startup so an older host
/// creating `tabula-screens` after `follow` began is still noticed.
pub(crate) fn screens_runtime_dir_candidates() -> Vec<PathBuf> {
    let rt = runtime_dir();
    vec![rt.join("olo-screens"), rt.join("tabula-screens")]
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
/// `ESRCH` (already exited) from resource/kernel failures (`EMFILE`, `ENOSYS`).
pub(crate) fn pidfd_open_result(pid: i32) -> io::Result<OwnedFd> {
    if pid <= 0 {
        return Err(io::Error::from_raw_os_error(libc::ESRCH));
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
/// `pidfd` for its original session process.
pub(crate) fn find_running_screen_pinned(name: &str) -> Option<(PathBuf, RuntimeScreen, OwnedFd)> {
    for rt in screens_runtime_dir_candidates() {
        let path = rt.join(format!("{name}.json"));
        if let Some(screen) = read_runtime_screen(&path)
            && screen.name == name
            && let PinResult::Alive(pidfd) = pin_live_pid(screen.pid, |p| screen_pid_running(name, p))
        {
            return Some((rt, screen, pidfd));
        }
    }
    None
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

/// Inspect `program` (ELF64 `.rodata`/`.data` sections, or non-ELF script up to
/// 64 KiB) for all literal markers in `needles` without spawning the binary.
fn program_contains_markers(program: &Path, needles: &[&[u8]]) -> io::Result<bool> {
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open(program)?;
    let meta = file.metadata()?;
    if !meta.file_type().is_file() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "host olo-screen binary is not a regular file"));
    }
    let file_len = meta.len();
    let mut head = [0u8; 64];
    let is_elf64 = file.read_exact_at(&mut head, 0).is_ok() && head[..4] == *b"\x7fELF" && head[4] == 2 && head[5] == 1;
    let ranges = if is_elf64 {
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
        ranges
    } else {
        vec![(0, file_len.min(64 * 1024))]
    };
    Ok(file_contains_all_in_ranges(&file, &ranges, needles))
}

/// Verify whether the host's installed `bin/olo-screen` (if present under
/// `prefix()`) honors `olo-rex-link` helper PIDs and `NAME.control.lock`
/// leases (Finding 10).
pub(crate) fn host_cli_honors_helper_control() -> Result<(), String> {
    let program = prefix().join("bin/olo-screen");
    if std::fs::metadata(&program).is_err() {
        // No host `olo-screen` installed under `prefix()` (e.g. isolated test rigs).
        return Ok(());
    }
    let supported = program_contains_markers(&program, &[b"olo-rex-link", b".control.lock"]).map_err(|e| format!("couldn't inspect host olo-screen: {e}"))?;
    if supported { Ok(()) } else { Err("This host's installed olo-screen is too old to honor remote Take Control; run `olo screen install` to upgrade.".into()) }
}

/// True when the installed host `olo-screen` binary/script supports `--part`
/// for discontiguous history video exports (Finding 44).
pub(crate) fn host_cli_supports_export_part(program: &Path) -> bool {
    program_contains_markers(program, &[b"--part"]).unwrap_or(false)
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
    if owner.screen_started.is_some_and(|st| st != s.started) {
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

/// Remove any runtime `NAME.owner.json` for `name` across runtime dirs (Finding 54).
fn unlink_runtime_owner(name: &str) {
    if !valid_name(name) {
        return;
    }
    for rt in screens_runtime_dir_candidates() {
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
    let rt = screens_runtime_dir();
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
    let screen_started = rt_screen.as_ref().map(|s| s.started);
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
            let mut caps = vec!["control-ack", "warning-status", "history-frame-error", "history-snapshot-done", "export-part-ranges", "stop-expected-started"];
            let prog = prefix().join("bin/olo-screen");
            if !prog.is_file() || host_cli_supports_export_part(&prog) {
                caps.push("discontiguous_part");
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
        Some("export") => crate::screen_export::run(&args[1..]),
        // (The window that shows screens is a window: the desktop's.)
        Some("watch") => fail("watching a screen is done from the Olo desktop; here, `olo screen --on host look NAME` is a picture of it"),
        Some("start") => start(&args),
        Some("stop") => stop_cmd(&args),
        // (Its own: what waits for a tab's program to end.)
        Some("keep-until") => keep_until(&args[1..]),
        Some(_) => exec(&args),
    }
}

/// `stop NAME [--expected-started N]`: validates session generation under
/// `NAME.lifecycle.lock` before forwarding `stop NAME` to host `olo-screen`,
/// and unlinks `NAME.owner.json` on success (Findings 3 & 54).
fn stop_cmd(args: &[String]) -> ! {
    let mut forward: Vec<String> = vec!["stop".to_owned()];
    let mut name: Option<String> = None;
    let mut expected_started: Option<u64> = None;
    let mut i = 1usize;
    while i < args.len() {
        if args[i] == "--expected-started" {
            let Some(v) = args.get(i + 1) else {
                fail("--expected-started needs a session timestamp");
            };
            let parsed = v.parse::<u64>().unwrap_or_else(|_| fail("--expected-started must be an integer"));
            expected_started = Some(parsed);
            i += 2;
        } else {
            if name.is_none() && !args[i].starts_with("--") {
                name = Some(args[i].clone());
            }
            forward.push(args[i].clone());
            i += 1;
        }
    }
    if let Some(expected) = expected_started {
        let Some(ref n) = name else {
            fail("stop needs a screen's name");
        };
        if !valid_name(n) {
            fail(format_args!("invalid screen name {n:?}"));
        }
        // Check under `NAME.lifecycle.lock` and release BEFORE spawning host
        // `olo-screen stop` so `olo-screen` can take the same lock without deadlock.
        let rt = screens_runtime_dir();
        let _ = std::fs::create_dir_all(&rt);
        let lock_path = rt.join(format!("{n}.lifecycle.lock"));
        let life_lock = std::fs::OpenOptions::new().create(true).read(true).write(true).truncate(false).mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(&lock_path).ok();
        if let Some(ref f) = life_lock {
            // SAFETY: blocking flock on exclusively opened lifecycle lock file.
            unsafe {
                libc::flock(f.as_raw_fd(), libc::LOCK_EX);
            }
        }
        let running = screens_runtime_dir_candidates().into_iter().find_map(|d| read_runtime_screen(&d.join(format!("{n}.json"))).filter(|s| s.name == *n && screen_pid_running(n, s.pid)));
        drop(life_lock);
        match running {
            None => fail(format_args!("screen {n} is not running")),
            Some(s) if s.started != expected => fail(format_args!("screen {n} was replaced by a newer session; refusing to stop it")),
            Some(_) => {}
        }
    }
    let mut tool = screen_tool(&forward);
    let status = match tool.status() {
        Ok(s) => s,
        Err(e) => fail(format_args!("{}: {e}", tool.get_program().to_string_lossy())),
    };
    if status.success()
        && let Some(ref n) = name
    {
        unlink_runtime_owner(n);
    }
    std::process::exit(status.code().unwrap_or(1));
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
            unlink_runtime_owner(name);
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
    let here = olo_release::installed_in(&prefix).version.filter(|_| prefix.join("bin/olo-screen").is_file());
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

/// What is here.
fn status() -> serde_json::Value {
    let prefix = prefix();
    let here = olo_release::installed_in(&prefix);
    let installed = here.version.filter(|_| prefix.join("bin/olo-screen").is_file());
    let dirs = library_dirs();
    let has = |name: &str| dirs.iter().any(|d| d.join(name).exists());
    let mut missing: Vec<String> = Vec::new();
    // A library here but older than the programs were linked against:
    // the versions of its symbols they need that it hasn't.
    let mut older: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> = Default::default();
    let mut defined: std::collections::HashMap<String, Option<Vec<String>>> = Default::default();
    let mut programs = 0;
    for entry in std::fs::read_dir(prefix.join("bin")).into_iter().flatten().flatten() {
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
    let software = has("libEGL_mesa.so.0") && gallium;
    let chrome = std::fs::read_link(home().join(".local/share/olo/chrome/current")).ok().map(|p| p.to_string_lossy().into_owned());
    let mut v = json!({
        "installed": installed,
        "prefix": prefix,
        "programs": programs,
        // (All of it: Olo, the Chrome it runs and the font put with it.
        // Olo's alone, 1.8 GB, stood beside "2.3 GB on the host" in the
        // window that had both.)
        "bytes": if installed.is_some() { bytes_under(&prefix) + bytes_under(&home().join(".local/share/olo/chrome")) + bytes_under(&screens_fonts()) } else { 0 },
        "chrome": chrome,
        "missing": missing,
        "older": older,
        "software_drawing": software,
        "emoji_font": emoji_font(),
        "committed": here.committed.filter(|_| installed.is_some()),
        "running": if installed.is_some() { running_screens(&prefix) } else { Vec::new() },
        "default": default_now(),
    });
    if installed.is_none() {
        v["how"] = json!("olo screen install");
    } else if !older.is_empty() {
        v["how"] = json!("this host's libraries are older than Olo's programs are linked against (Fedora 44's): its screens can't run here until the system is a newer one");
    } else if !software {
        v["how"] =
            json!("Mesa's EGL and software renderer aren't here: on Fedora, `sudo dnf install mesa-libEGL mesa-dri-drivers`; on Debian or Ubuntu, `sudo apt install libegl-mesa0 libgl1-mesa-dri`");
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
/// this machine, and those `/etc/ld.so.conf` names.
fn library_dirs() -> Vec<PathBuf> {
    let machine = std::env::consts::ARCH;
    let mut dirs: Vec<PathBuf> = ["/lib64", "/usr/lib64", "/usr/local/lib64", "/lib", "/usr/lib", "/usr/local/lib"].iter().map(PathBuf::from).collect();
    dirs.push(format!("/lib/{machine}-linux-gnu").into());
    dirs.push(format!("/usr/lib/{machine}-linux-gnu").into());
    let mut confs = vec![PathBuf::from("/etc/ld.so.conf")];
    confs.extend(std::fs::read_dir("/etc/ld.so.conf.d").into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "conf")));
    for conf in confs {
        for line in std::fs::read_to_string(conf).unwrap_or_default().lines() {
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
        let file = std::fs::File::open(path).ok()?;
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
        let dynamic: Vec<(u64, u64)> = table.chunks_exact(16).map(|e| (u64_of(e, 0), u64_of(e, 8))).take_while(|(tag, _)| *tag != 0).collect();
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
        assert!(!is_session_cmdline(b"/bin/sleep\060\0"));
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

        assert_eq!(fallback_runtime_dir_in(uid, &good_run, &var_tmp), good_run);
        let fb1 = fallback_runtime_dir_in(uid, &wide_run, &var_tmp);
        assert_eq!(fb1, var_tmp.join(format!("olo-runtime-{uid}")));
        assert_eq!(std::fs::symlink_metadata(&fb1).unwrap().mode() & 0o777, 0o700);
        let fb2 = fallback_runtime_dir_in(uid, &missing_run, &var_tmp);
        assert_eq!(fb2, fb1);

        let real_fb = fallback_runtime_dir();
        assert!(!real_fb.starts_with("/tmp"), "fallback runtime dir must never be under /tmp: {}", real_fb.display());
        let _ = std::fs::remove_dir_all(&base);
    }
}
