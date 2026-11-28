//! What was done on each screen: every action `olo-screen` takes on it
//! (as it was asked, what it matched or where it went, how it went and how
//! long it took), one JSON object a line in
//! `$XDG_STATE_HOME/olo/screens/NAME/actions.jsonl`, appended as each one
//! ends. It outlives the screen until cleared (`olo-screen history NAME
//! --clear`), and is kept under `MAX_BYTES` by dropping its oldest half.

use std::{
    collections::BTreeMap,
    ffi::CString,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::ffi::OsStrExt as _,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

/// The commands that act on a screen (its history). Reads (`tree`,
/// `look`, `windows`, `menus`, `wait`) change nothing and are left out.
pub const ACTIONS: &[&str] = &[
    "start",
    "stop",
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
    "resize",
    "place",
];

/// The most a screen's history takes on disk.
const MAX_BYTES: u64 = 8 << 20;

/// Stretches without an action longer than this don't count as the
/// screen being in use.
pub const IDLE_MS: u64 = 60_000;

/// One action on a screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    /// When it began (milliseconds since the epoch), and how long it took.
    pub at: u64,
    pub ms: u64,
    pub kind: String,
    /// Its arguments after the screen's name, as given.
    #[serde(default)]
    pub args: Vec<String>,
    /// Its answer: what it matched, where it clicked, the focus after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Action {
    /// True when this entry is an index-preserving placeholder emitted by the
    /// host helper (`olo-rex-link screen history-stream`) for a recorded line
    /// that exceeded wire record limits (`{"at":0,"ms":0,"kind":"oversized","args":[]}`).
    /// Its 0-based index stays aligned with `--part`, while display and timing
    /// treat it as an action that cannot be shown.
    pub fn is_oversized(&self) -> bool {
        self.kind == "oversized" && self.at == 0 && self.ms == 0 && self.args.is_empty() && self.result.is_none() && self.error.is_none()
    }

    /// A line for people: the time, what was done, and what it found.
    pub fn line(&self) -> String {
        if self.is_oversized() {
            return "—  Action too large to show".to_owned();
        }
        let time = clock(self.at);
        let args = self.args.iter().map(|a| if a.contains(' ') || a.is_empty() { format!("{a:?}") } else { a.clone() }).collect::<Vec<_>>().join(" ");
        let what = if args.is_empty() { self.kind.clone() } else { format!("{} {args}", self.kind) };
        let outcome = match (&self.error, self.matched()) {
            (Some(e), _) => format!("  ✗ {e}"),
            (None, Some(m)) => format!("  → {m}"),
            (None, None) => String::new(),
        };
        format!("{time}  {what}{outcome}  ({} ms)", self.ms)
    }

    /// What it found to act on, as `Role "name" at X,Y`.
    pub fn matched(&self) -> Option<String> {
        let m = self.result.as_ref()?.get("matched")?;
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("");
        let name = m.get("name").and_then(|r| r.as_str()).unwrap_or("");
        let at = m.get("center").and_then(|c| Some((c.get(0)?.as_f64()?, c.get(1)?.as_f64()?)));
        Some(match at {
            Some((x, y)) => format!("{role} {name:?} at {x:.0},{y:.0}"),
            None => format!("{role} {name:?}"),
        })
    }

    /// Whether `text` is in it anywhere (its kind, arguments, what it
    /// matched, its error), regardless of case.
    pub fn mentions(&self, text: &str) -> bool {
        let t = text.to_lowercase();
        self.kind.to_lowercase().contains(&t)
            || self.args.iter().any(|a| a.to_lowercase().contains(&t))
            || self.matched().is_some_and(|m| m.to_lowercase().contains(&t))
            || self.error.as_ref().is_some_and(|e| e.to_lowercase().contains(&t))
    }
}

/// Root folder holding all local screens' histories (`$XDG_STATE_HOME/olo/screens`).
pub fn root() -> PathBuf {
    root_dir()
}

/// Root folder holding all local screens' histories (`$XDG_STATE_HOME/olo/screens`).
pub(crate) fn root_dir() -> PathBuf {
    olo_xdg::state_dir().join("screens")
}

/// Open the configured screens `root` as a directory fd (`O_DIRECTORY | O_CLOEXEC`).
/// Every per-screen entry below `root` (`screens/NAME` and files inside it) is
/// opened via `openat(root_fd, ..., O_NOFOLLOW)` so no screen name can ever
/// follow a symlink or escape the configured root.
fn open_root_dir(root: &Path) -> Result<OwnedFd, std::io::Error> {
    let c = CString::new(root.as_os_str().as_bytes()).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid root path"))?;
    // SAFETY: `c` is a valid NUL-terminated path; returned fd is wrapped in `OwnedFd`.
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if fd < 0 { Err(std::io::Error::last_os_error()) } else { Ok(unsafe { OwnedFd::from_raw_fd(fd) }) }
}

/// Open screen `name`'s existing directory under `root` via `openat` with
/// `O_DIRECTORY | O_NOFOLLOW` so a symlink at `root/name` is rejected atomically.
pub(crate) fn open_screen_dir_fd(root: &Path, name: &str) -> Result<OwnedFd, String> {
    crate::screens::require_valid_name(name)?;
    let root_fd = open_root_dir(root).map_err(|e| format!("couldn't open screens folder: {e}"))?;
    let cname = CString::new(name).map_err(|_| format!("{name:?} can't name a screen"))?;
    // SAFETY: `root_fd` and `cname` are valid; `O_NOFOLLOW | O_DIRECTORY` refuses symlinks and non-dirs.
    let fd = unsafe { libc::openat(root_fd.as_raw_fd(), cname.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if fd < 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::ELOOP) || e.raw_os_error() == Some(libc::ENOTDIR) {
            return Err(format!("refusing to follow symlink or non-directory for screen {name:?}"));
        }
        return Err(format!("couldn't open screen folder {name:?}: {e}"));
    }
    // SAFETY: `fd >= 0` is a freshly opened owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Create (if absent) and open screen `name`'s directory under `root` without
/// following symlinks (`mkdirat` + `openat(O_DIRECTORY | O_NOFOLLOW)`).
fn ensure_screen_dir_fd(root: &Path, name: &str) -> Result<OwnedFd, String> {
    crate::screens::require_valid_name(name)?;
    if let Ok(m) = std::fs::metadata(root)
        && !m.file_type().is_dir()
    {
        return Err("screens history root is not a directory".into());
    }
    std::fs::create_dir_all(root).map_err(|e| format!("couldn't create screens folder: {e}"))?;
    let root_fd = open_root_dir(root).map_err(|e| format!("couldn't open screens folder: {e}"))?;
    let cname = CString::new(name).map_err(|_| format!("{name:?} can't name a screen"))?;
    // SAFETY: `root_fd` and `cname` are valid.
    let rc = unsafe { libc::mkdirat(root_fd.as_raw_fd(), cname.as_ptr(), 0o755) };
    if rc != 0 {
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(format!("couldn't create folder for {name}: {e}"));
        }
    }
    // SAFETY: `openat` with `O_NOFOLLOW | O_DIRECTORY` atomically rejects a symlink.
    let fd = unsafe { libc::openat(root_fd.as_raw_fd(), cname.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if fd < 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::ELOOP) || e.raw_os_error() == Some(libc::ENOTDIR) {
            return Err(format!("refusing to follow symlink or non-directory for screen {name:?}"));
        }
        return Err(format!("couldn't open screen folder {name:?}: {e}"));
    }
    // SAFETY: `fd >= 0` is freshly opened.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Open a regular file relative to a verified screen-directory fd without
/// following symlinks or blocking on FIFOs.
fn openat_regular(dir_fd: &OwnedFd, file: &str, flags: i32, mode: libc::mode_t) -> std::io::Result<File> {
    let cfile = CString::new(file).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid filename"))?;
    // SAFETY: `dir_fd` and `cfile` are valid; `O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC` prevents symlink follow and FIFO hang.
    let fd = unsafe { libc::openat(dir_fd.as_raw_fd(), cfile.as_ptr(), flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC, mode) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fd >= 0` is freshly opened.
    let f = unsafe { File::from_raw_fd(fd) };
    let meta = f.metadata()?;
    if !meta.file_type().is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{file} is not a regular file")));
    }
    Ok(f)
}

fn read_regular_at(dir_fd: &OwnedFd, file: &str, max_bytes: usize) -> Option<Vec<u8>> {
    let f = openat_regular(dir_fd, file, libc::O_RDONLY, 0).ok()?;
    let len = f.metadata().ok()?.len();
    if len > max_bytes as u64 {
        return None;
    }
    let mut buf = Vec::with_capacity((len as usize).min(64 << 10));
    f.take(max_bytes as u64 + 1).read_to_end(&mut buf).ok()?;
    (buf.len() <= max_bytes).then_some(buf)
}

fn is_regular_at(dir_fd: &OwnedFd, file: &str) -> bool {
    let Ok(cfile) = CString::new(file) else { return false };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `dir_fd`, `cfile`, and `&mut st` are valid; `AT_SYMLINK_NOFOLLOW` never follows symlinks.
    unsafe { libc::fstatat(dir_fd.as_raw_fd(), cfile.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFREG }
}

/// Validated folder for screen `name` inside `root`: refuses empty, long,
/// path-shaped names, and symlinks before any filesystem operation touches disk.
pub(crate) fn screen_dir_in(root: &Path, name: &str) -> Result<PathBuf, String> {
    crate::screens::require_valid_name(name)?;
    let path = root.join(name);
    if let Ok(m) = std::fs::symlink_metadata(&path)
        && !m.file_type().is_dir()
    {
        return Err(format!("refusing to follow symlink or non-directory for screen {name:?}"));
    }
    Ok(path)
}

/// Validated screen history folder under the default state root (rejects
/// invalid names and symlink directories).
pub fn dir_checked(name: &str) -> Result<PathBuf, String> {
    screen_dir_in(&root_dir(), name)
}

/// Where screen `name`'s history is kept (callers handling untrusted input
/// should prefer [`dir_checked`]).
pub fn dir(name: &str) -> PathBuf {
    if crate::screens::valid_name(name) { root_dir().join(name) } else { root_dir().join("__invalid__") }
}

/// Screen `name` was archived (stopped, out of the list until View ▸
/// Show Archived): a mark beside its history.
pub fn archived(name: &str) -> bool {
    archived_in(&root_dir(), name)
}

pub(crate) fn archived_in(root: &Path, name: &str) -> bool {
    open_screen_dir_fd(root, name).is_ok_and(|dfd| is_regular_at(&dfd, "archived"))
}

/// Archive screen `name`, or take it out of the archive.
pub fn set_archived(name: &str, on: bool) -> Result<(), String> {
    set_archived_in(&root_dir(), name, on)
}

pub(crate) fn set_archived_in(root: &Path, name: &str, on: bool) -> Result<(), String> {
    let failed = |e: std::io::Error| format!("couldn't {} {name}: {e}", if on { "archive" } else { "unarchive" });
    if on {
        let dfd = ensure_screen_dir_fd(root, name)?;
        let _ = openat_regular(&dfd, "archived", libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC, 0o644).map_err(failed)?;
        Ok(())
    } else {
        let Ok(dfd) = open_screen_dir_fd(root, name) else {
            // Re-run name/symlink check so invalid names or symlinks still error.
            let _ = screen_dir_in(root, name)?;
            return Ok(());
        };
        let cname = CString::new("archived").unwrap();
        // SAFETY: `dfd` and `cname` are valid; `unlinkat` removes only inside the verified directory fd.
        if unsafe { libc::unlinkat(dfd.as_raw_fd(), cname.as_ptr(), 0) } != 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(failed(e));
            }
        }
        Ok(())
    }
}

/// Screen `name`'s history, its recordings, stats and title, to the Trash
/// (where it can be put back from), as Delete does.
pub fn trash(name: &str) -> Result<(), String> {
    trash_in(&root_dir(), name)
}

pub(crate) fn trash_in(root: &Path, name: &str) -> Result<(), String> {
    let dir = screen_dir_in(root, name)?;
    match olo_xdg::trash::move_to_trash(&dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("couldn't move {name}'s history to the Trash: {e}")),
        _ => Ok(()),
    }
}

/// The name the person gave screen `name` (Rename…), if any: kept with
/// its history, and gone with it.
pub fn title(name: &str) -> Option<String> {
    title_in(&root_dir(), name)
}

pub(crate) fn title_in(root: &Path, name: &str) -> Option<String> {
    let dfd = open_screen_dir_fd(root, name).ok()?;
    let bytes = read_regular_at(&dfd, "title", 4096)?;
    String::from_utf8(bytes).ok().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty())
}

/// Give screen `name` the name `title` to show by (None or blank: its
/// own again).
pub fn set_title(name: &str, title: Option<&str>) -> Result<(), String> {
    set_title_in(&root_dir(), name, title)
}

pub(crate) fn set_title_in(root: &Path, name: &str, title: Option<&str>) -> Result<(), String> {
    let failed = |e: std::io::Error| format!("couldn't rename {name}: {e}");
    match title.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => {
            let dfd = ensure_screen_dir_fd(root, name)?;
            let tmp_name = format!(".title.{}.tmp", std::process::id());
            let mut f = openat_regular(&dfd, &tmp_name, libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC, 0o644).map_err(failed)?;
            f.write_all(t.as_bytes()).and_then(|()| f.flush()).map_err(failed)?;
            drop(f);
            let ctmp = CString::new(tmp_name).unwrap();
            let cdst = CString::new("title").unwrap();
            // SAFETY: both descriptors are our verified `dfd`.
            if unsafe { libc::renameat(dfd.as_raw_fd(), ctmp.as_ptr(), dfd.as_raw_fd(), cdst.as_ptr()) } != 0 {
                let e = std::io::Error::last_os_error();
                unsafe {
                    libc::unlinkat(dfd.as_raw_fd(), ctmp.as_ptr(), 0);
                }
                return Err(failed(e));
            }
            Ok(())
        }
        None => {
            let Ok(dfd) = open_screen_dir_fd(root, name) else {
                let _ = screen_dir_in(root, name)?;
                return Ok(());
            };
            let cname = CString::new("title").unwrap();
            if unsafe { libc::unlinkat(dfd.as_raw_fd(), cname.as_ptr(), 0) } != 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() != std::io::ErrorKind::NotFound {
                    return Err(failed(e));
                }
            }
            Ok(())
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Add `action` to screen `name`'s history (quietly giving up if the disk
/// won't have it: the action itself was done).
pub fn record(name: &str, action: &Action) {
    record_in(&root_dir(), name, action);
}

pub(crate) fn record_in(root: &Path, name: &str, action: &Action) {
    let Ok(dfd) = ensure_screen_dir_fd(root, name) else { return };
    if let Ok(f) = openat_regular(&dfd, "actions.jsonl", libc::O_RDONLY, 0)
        && f.metadata().is_ok_and(|m| m.len() > MAX_BYTES)
    {
        let all = read_in(root, name);
        let keep = &all[all.len() / 2..];
        let text: String = keep.iter().filter_map(|a| serde_json::to_string(a).ok()).map(|l| l + "\n").collect();
        let tmp_name = format!(".actions.{}.tmp", std::process::id());
        if let Ok(mut tf) = openat_regular(&dfd, &tmp_name, libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC, 0o644)
            && tf.write_all(text.as_bytes()).is_ok()
            && tf.flush().is_ok()
        {
            drop(tf);
            let ctmp = CString::new(tmp_name).unwrap();
            let cdst = CString::new("actions.jsonl").unwrap();
            // SAFETY: `dfd` is verified non-symlink.
            unsafe {
                if libc::renameat(dfd.as_raw_fd(), ctmp.as_ptr(), dfd.as_raw_fd(), cdst.as_ptr()) != 0 {
                    libc::unlinkat(dfd.as_raw_fd(), ctmp.as_ptr(), 0);
                }
            }
        }
    }
    if let Ok(mut f) = openat_regular(&dfd, "actions.jsonl", libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND, 0o644)
        && let Ok(line) = serde_json::to_string(action)
    {
        let _ = writeln!(f, "{line}");
    }
}

/// Screen `name`'s history, oldest first (lines that don't read are
/// skipped).
pub fn read(name: &str) -> Vec<Action> {
    read_in(&root_dir(), name)
}

pub(crate) fn read_in(root: &Path, name: &str) -> Vec<Action> {
    let Ok(dfd) = open_screen_dir_fd(root, name) else { return Vec::new() };
    let Some(bytes) = read_regular_at(&dfd, "actions.jsonl", (MAX_BYTES as usize) * 2) else { return Vec::new() };
    String::from_utf8_lossy(&bytes).lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Fast tail-only lookup of a screen's latest recorded action timestamp
/// (`at + ms`, milliseconds since epoch) reading bounded chunks from the end of
/// `actions.jsonl` instead of deserializing up to 16 MiB of history.
pub fn last_used_fast(name: &str) -> Option<u64> {
    last_used_fast_in(&root_dir(), name)
}

pub(crate) fn last_used_fast_in(root: &Path, name: &str) -> Option<u64> {
    last_used_fast_checked_in(root, name).ok().flatten()
}

/// Extract `(at, ms)` from a single JSON action line without allocating or
/// deserializing large `result` / `args` payloads (Astra #23: even an action
/// record longer than 64 KiB can yield its timestamp without parsing a whole
/// 16 MiB history).
fn parse_action_timestamp_line(line: &str) -> Option<Result<Option<u64>, ()>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(a) = serde_json::from_str::<Action>(trimmed) {
        return Some(Ok(if a.is_oversized() { None } else { Some(a.at.saturating_add(a.ms)) }));
    }
    // Lightweight fallback for very large lines that only carry header fields:
    #[derive(Deserialize)]
    struct Head {
        at: u64,
        ms: u64,
        #[serde(default)]
        kind: String,
    }
    match serde_json::from_str::<Head>(trimmed) {
        Ok(h) => Some(Ok(if h.kind == "oversized" { None } else { Some(h.at.saturating_add(h.ms)) })),
        Err(_) => Some(Err(())),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum OversizedPrefix {
    Timestamp(u64),
    OversizedPlaceholder,
    Invalid,
}

fn skip_json_ws(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n') {
        i += 1;
    }
    i
}

/// Decode one JSON string literal starting at `bytes[start] == b'"'`, returning
/// `(decoded, next_byte_index)` or `None` if malformed or truncated.
fn parse_json_string_prefix(bytes: &[u8], start: usize) -> Option<(String, usize)> {
    if bytes.get(start) != Some(&b'"') {
        return None;
    }
    let mut out = String::new();
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => return Some((out, i + 1)),
            b'\\' => {
                i += 1;
                let esc = *bytes.get(i)?;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000C}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let hex = std::str::from_utf8(bytes.get(i + 1..i + 5)?).ok()?;
                        let code = u32::from_str_radix(hex, 16).ok()?;
                        out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                        i += 4;
                    }
                    _ => return None,
                }
                i += 1;
            }
            b if b < 0x20 => return None,
            _ => {
                let rest = std::str::from_utf8(&bytes[i..]).ok()?;
                let ch = rest.chars().next()?;
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    None
}

/// Skip one complete JSON value starting at `bytes[start]` within the prefix.
fn skip_json_value_prefix(bytes: &[u8], start: usize) -> Option<usize> {
    let i = skip_json_ws(bytes, start);
    let b = *bytes.get(i)?;
    match b {
        b'"' => parse_json_string_prefix(bytes, i).map(|(_, next)| next),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut pos = i;
            while pos < bytes.len() {
                match bytes[pos] {
                    b'"' => {
                        let (_, next) = parse_json_string_prefix(bytes, pos)?;
                        pos = next;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth = depth.checked_sub(1)?;
                        if depth == 0 {
                            return Some(pos + 1);
                        }
                    }
                    _ => {}
                }
                pos += 1;
            }
            None
        }
        _ => {
            let mut pos = i;
            while pos < bytes.len() && !matches!(bytes[pos], b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n') {
                pos += 1;
            }
            (pos > i && pos < bytes.len()).then_some(pos)
        }
    }
}

/// Strictly parse top-level `"at"`, `"ms"`, and `"kind"` directly from the
/// prefix of an oversized action line (> 64 KiB) without scanning into string
/// payloads such as `"args"` (Agy-review R1 #6).
fn parse_oversized_line_prefix(prefix: &str) -> OversizedPrefix {
    let bytes = prefix.as_bytes();
    let mut i = skip_json_ws(bytes, 0);
    if bytes.get(i) != Some(&b'{') {
        return OversizedPrefix::Invalid;
    }
    i += 1;
    let mut at: Option<u64> = None;
    let mut ms: Option<u64> = None;
    let mut kind: Option<String> = None;
    loop {
        i = skip_json_ws(bytes, i);
        if bytes.get(i) == Some(&b'}') {
            break;
        }
        let Some((key, next)) = parse_json_string_prefix(bytes, i) else {
            break;
        };
        i = skip_json_ws(bytes, next);
        if bytes.get(i) != Some(&b':') {
            return OversizedPrefix::Invalid;
        }
        i = skip_json_ws(bytes, i + 1);
        match key.as_str() {
            "at" | "ms" => {
                let num_start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i == num_start || i >= bytes.len() || !matches!(bytes[i], b',' | b'}' | b' ' | b'\t' | b'\r' | b'\n') {
                    return OversizedPrefix::Invalid;
                }
                let Ok(val) = std::str::from_utf8(&bytes[num_start..i]).unwrap_or("").parse::<u64>() else {
                    return OversizedPrefix::Invalid;
                };
                if key == "at" {
                    at = Some(val);
                } else {
                    ms = Some(val);
                }
            }
            "kind" => {
                let Some((val, next)) = parse_json_string_prefix(bytes, i) else {
                    return OversizedPrefix::Invalid;
                };
                kind = Some(val);
                i = next;
            }
            _ => {
                let Some(next) = skip_json_value_prefix(bytes, i) else {
                    break;
                };
                i = next;
            }
        }
        if let (Some(a), Some(m), Some(k)) = (at, ms, kind.as_deref()) {
            return if k == "oversized" { OversizedPrefix::OversizedPlaceholder } else { OversizedPrefix::Timestamp(a.saturating_add(m)) };
        }
        i = skip_json_ws(bytes, i);
        match bytes.get(i) {
            Some(&b',') => i += 1,
            Some(&b'}') => break,
            _ => break,
        }
    }
    match (at, ms, kind.as_deref()) {
        (_, _, Some("oversized")) => OversizedPrefix::OversizedPlaceholder,
        (Some(a), Some(m), Some(_)) => OversizedPrefix::Timestamp(a.saturating_add(m)),
        _ => OversizedPrefix::Invalid,
    }
}

/// Checked tail-only lookup of a screen's latest recorded action timestamp
/// (`at + ms`, milliseconds since epoch) for guarded Delete / Clear / Stop
/// (Astra #2–#3, #23; Agy-review R1 #6):
/// - Missing screen directory or missing/empty `actions.jsonl` -> `Ok(None)`.
/// - Symlinked/unreadable directory, non-regular/unreadable `actions.jsonl`, or
///   a trailing corrupt/truncated line -> `Err(...)` immediately (NEVER falls
///   back past a corrupt newest line to an earlier valid record).
/// - If the final line is longer than 64 KiB or the tail holds only valid
///   `"oversized"` placeholder lines, scans backwards in bounded 64 KiB chunks
///   (up to 4 MiB) inspecting each record's top-level JSON header.
pub fn last_used_fast_checked_in(root: &Path, name: &str) -> Result<Option<u64>, String> {
    crate::screens::require_valid_name(name)?;
    let root_fd = match open_root_dir(root) {
        Ok(fd) => fd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("couldn't inspect screens folder: {e}")),
    };
    let cname = CString::new(name).map_err(|_| format!("{name:?} can't name a screen"))?;
    // SAFETY: valid root dir fd and NUL-terminated single-component name.
    let raw_dfd = unsafe { libc::openat(root_fd.as_raw_fd(), cname.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    let dfd = if raw_dfd < 0 {
        let e = std::io::Error::last_os_error();
        if e.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(format!("couldn't inspect screen {name:?}'s history folder: {e}"));
    } else {
        // SAFETY: `raw_dfd >= 0` is a freshly opened directory descriptor.
        unsafe { OwnedFd::from_raw_fd(raw_dfd) }
    };
    let mut f = match openat_regular(&dfd, "actions.jsonl", libc::O_RDONLY, 0) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("couldn't read {name:?}'s action log: {e}")),
    };
    let meta = f.metadata().map_err(|e| format!("couldn't stat {name:?}'s action log: {e}"))?;
    if !meta.file_type().is_file() {
        return Err(format!("{name:?}'s action log is not a regular file"));
    }
    let len = meta.len();
    last_used_fast_checked_from_file(&mut f, len, name)
}

fn last_used_fast_checked_from_file(f: &mut std::fs::File, len: u64, name: &str) -> Result<Option<u64>, String> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    const TAIL_BYTES: u64 = 64 * 1024;
    const MAX_BACKSCAN_BYTES: u64 = 4 * 1024 * 1024;
    if len == 0 {
        return Ok(None);
    }
    let start = len.saturating_sub(TAIL_BYTES);
    if start > 0 {
        f.seek(SeekFrom::Start(start)).map_err(|e| format!("couldn't seek {name:?}'s action log: {e}"))?;
    }
    let cap = len - start;
    let mut buf = Vec::with_capacity(cap as usize);
    // Cap the tail read strictly to the snapshotted byte range `len - start` so
    // a concurrent append from `history::record_in` can never slice mid-line
    // past `len` and trigger a false corrupt-log error (Agy-review R2 #1).
    std::io::Read::by_ref(f).take(cap).read_to_end(&mut buf).map_err(|e| format!("couldn't read {name:?}'s action log: {e}"))?;

    // Check complete lines within the tail chunk first. Any trailing corrupt or
    // truncated line MUST fail closed immediately rather than falling back to
    // an older valid timestamp.
    let (first_nl, slice) = if start > 0 {
        match buf.iter().position(|&b| b == b'\n') {
            Some(pos) => (Some(pos), &buf[pos + 1..]),
            None => (None, &[][..]),
        }
    } else {
        (Some(0), &buf[..])
    };
    let mut saw_any_nonempty = false;
    let mut saw_valid_oversized = false;
    if first_nl.is_some() {
        let text = String::from_utf8_lossy(slice);
        for line in text.lines().rev() {
            if let Some(res) = parse_action_timestamp_line(line) {
                saw_any_nonempty = true;
                match res {
                    Ok(Some(ts)) => return Ok(Some(ts)),
                    Ok(None) => saw_valid_oversized = true,
                    Err(()) => return Err(format!("{name:?}'s action log is unreadable or corrupt")),
                }
            }
        }
    }

    // If earlier records exist before `start` (e.g. a final action line > 64 KiB,
    // or a tail filled entirely with valid `"oversized"` placeholder lines),
    // scan lines backwards up to `MAX_BACKSCAN_BYTES`.
    if start > 0 {
        let floor = len.saturating_sub(MAX_BACKSCAN_BYTES);
        let mut line_end = match first_nl {
            Some(pos) => start + pos as u64,
            None => len,
        };
        let mut reached_bof = false;
        while line_end > floor {
            // Find the previous '\n' strictly before `line_end`.
            let mut cursor = line_end;
            let mut found_nl: Option<u64> = None;
            while cursor > floor {
                let chunk_start = cursor.saturating_sub(TAIL_BYTES).max(floor);
                let chunk_len = (cursor - chunk_start) as usize;
                f.seek(SeekFrom::Start(chunk_start)).map_err(|e| format!("couldn't seek {name:?}'s action log: {e}"))?;
                let mut chunk = vec![0u8; chunk_len];
                f.read_exact(&mut chunk).map_err(|e| format!("couldn't read {name:?}'s action log: {e}"))?;
                if let Some(pos) = chunk.iter().rposition(|&b| b == b'\n') {
                    found_nl = Some(chunk_start + pos as u64);
                    break;
                }
                cursor = chunk_start;
            }
            let line_start = match found_nl {
                Some(nl) => nl + 1,
                None if floor == 0 => 0,
                None => return Err(format!("couldn't determine latest activity from {name:?}'s action log")),
            };
            let line_len = line_end.saturating_sub(line_start);
            if line_len > 0 {
                if line_len <= 4096 {
                    f.seek(SeekFrom::Start(line_start)).map_err(|e| format!("couldn't seek {name:?}'s action log: {e}"))?;
                    let mut raw = vec![0u8; line_len as usize];
                    f.read_exact(&mut raw).map_err(|e| format!("couldn't read {name:?}'s action log: {e}"))?;
                    let s = String::from_utf8_lossy(&raw);
                    if let Some(res) = parse_action_timestamp_line(&s) {
                        saw_any_nonempty = true;
                        match res {
                            Ok(Some(ts)) => return Ok(Some(ts)),
                            Ok(None) => saw_valid_oversized = true,
                            Err(()) => return Err(format!("{name:?}'s action log is unreadable or corrupt")),
                        }
                    }
                } else {
                    // Verify the oversized line closes with `}` (not crash-truncated).
                    let tail_probe_start = line_end.saturating_sub(64).max(line_start);
                    let tail_probe_len = (line_end - tail_probe_start) as usize;
                    f.seek(SeekFrom::Start(tail_probe_start)).map_err(|e| format!("couldn't seek {name:?}'s action log: {e}"))?;
                    let mut tail_probe = vec![0u8; tail_probe_len];
                    f.read_exact(&mut tail_probe).map_err(|e| format!("couldn't read {name:?}'s action log: {e}"))?;
                    let last_non_ws = tail_probe.iter().rev().find(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n'));
                    if last_non_ws != Some(&b'}') {
                        return Err(format!("{name:?}'s action log is unreadable or corrupt"));
                    }
                    f.seek(SeekFrom::Start(line_start)).map_err(|e| format!("couldn't seek {name:?}'s action log: {e}"))?;
                    let mut head = Vec::with_capacity(512);
                    std::io::Read::by_ref(f).take(512).read_to_end(&mut head).map_err(|e| format!("couldn't read {name:?}'s action log: {e}"))?;
                    let head_str = String::from_utf8_lossy(&head);
                    let first_line = head_str.lines().next().unwrap_or("");
                    match parse_oversized_line_prefix(first_line) {
                        OversizedPrefix::Timestamp(ts) => return Ok(Some(ts)),
                        OversizedPrefix::OversizedPlaceholder => {
                            saw_any_nonempty = true;
                            saw_valid_oversized = true;
                        }
                        OversizedPrefix::Invalid => return Err(format!("couldn't determine latest activity from {name:?}'s action log")),
                    }
                }
            }
            match found_nl {
                Some(nl) => line_end = nl,
                None => {
                    reached_bof = floor == 0;
                    break;
                }
            }
        }
        if line_end == 0 && floor == 0 {
            reached_bof = true;
        }
        if reached_bof && (!saw_any_nonempty || saw_valid_oversized) {
            return Ok(None);
        }
        return Err(format!("couldn't determine latest activity from {name:?}'s action log"));
    }

    if !saw_any_nonempty || saw_valid_oversized { Ok(None) } else { Err(format!("{name:?}'s action log is unreadable or corrupt")) }
}

/// Verify that `root/name` is a real screen directory containing at least one
/// screen marker (`screen.json` or `actions.jsonl`) via verified directory fds
/// (Astra #2–#3: guarded Delete must never trash an unrelated directory).
pub(crate) fn has_screen_marker_in(root: &Path, name: &str) -> Result<bool, String> {
    let dfd = match open_screen_dir_fd(root, name) {
        Ok(fd) => fd,
        Err(e) if e.contains("No such file") => return Ok(false),
        Err(e) => return Err(e),
    };
    for marker in ["screen.json", "actions.jsonl"] {
        match openat_regular(&dfd, marker, libc::O_RDONLY, 0) {
            Ok(_) => return Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("couldn't inspect {marker} for screen {name:?}: {e}")),
        }
    }
    Ok(false)
}

/// Reads a screen's history as it grows: what was added since the last
/// read, or all of it again when the file was cut down (its oldest half
/// dropped, pruned, cleared) or replaced.
#[derive(Debug, Default)]
pub struct Reader {
    /// The file (its `(dev, ino)`) and how far into it was read: up to the end
    /// of its last whole line.
    inode: Option<(u64, u64)>,
    offset: u64,
}

impl Reader {
    /// What is new in screen `name`'s history, and whether it is the
    /// whole history (start over with it) rather than more of it.
    pub fn more(&mut self, name: &str) -> (Vec<Action>, bool) {
        self.more_in(&root_dir(), name)
    }

    /// Checked variant of [`Self::more`] that distinguishes a missing/empty
    /// history (`Ok(([], reset))`) from an unreadable or corrupt `actions.jsonl`
    /// (`Err(reason)`) so local Stop fails closed before prompting (Astra #10 / Neon #7).
    pub fn more_checked(&mut self, name: &str) -> Result<(Vec<Action>, bool), String> {
        self.more_checked_in(&root_dir(), name)
    }

    pub(crate) fn more_in(&mut self, root: &Path, name: &str) -> (Vec<Action>, bool) {
        let Ok(dfd) = open_screen_dir_fd(root, name) else {
            let reset = self.inode.take().is_some();
            self.offset = 0;
            return (Vec::new(), reset);
        };
        let Ok(mut f) = openat_regular(&dfd, "actions.jsonl", libc::O_RDONLY, 0) else {
            let reset = self.inode.take().is_some();
            self.offset = 0;
            return (Vec::new(), reset);
        };
        self.more_file(&mut f)
    }

    pub(crate) fn more_checked_in(&mut self, root: &Path, name: &str) -> Result<(Vec<Action>, bool), String> {
        let dfd = match open_screen_dir_fd(root, name) {
            Ok(fd) => fd,
            Err(e) if e.contains("No such file") => {
                let reset = self.inode.take().is_some();
                self.offset = 0;
                return Ok((Vec::new(), reset));
            }
            Err(_) => {
                self.inode = None;
                self.offset = 0;
                return Err("Couldn’t read this screen’s history.".to_owned());
            }
        };
        let mut f = match openat_regular(&dfd, "actions.jsonl", libc::O_RDONLY, 0) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let reset = self.inode.take().is_some();
                self.offset = 0;
                return Ok((Vec::new(), reset));
            }
            Err(_) => {
                self.inode = None;
                self.offset = 0;
                return Err("Couldn’t read this screen’s history.".to_owned());
            }
        };
        self.more_file_checked(&mut f)
    }

    fn more_file_checked(&mut self, f: &mut File) -> Result<(Vec<Action>, bool), String> {
        let meta = f.metadata().map_err(|_| "Couldn’t read this screen’s history.".to_owned())?;
        if !meta.file_type().is_file() {
            return Err("Couldn’t read this screen’s history.".to_owned());
        }
        let id = (meta.dev(), meta.ino());
        let reset = self.inode != Some(id) || meta.len() < self.offset;
        if reset {
            self.inode = Some(id);
            self.offset = 0;
        }
        let mut text = Vec::new();
        let max_read = (MAX_BYTES * 2).saturating_sub(self.offset);
        if meta.len() > self.offset && max_read > 0 {
            f.seek(SeekFrom::Start(self.offset)).map_err(|_| "Couldn’t read this screen’s history.".to_owned())?;
            f.take(max_read).read_to_end(&mut text).map_err(|_| "Couldn’t read this screen’s history.".to_owned())?;
        }
        let whole = text.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        let slice = std::str::from_utf8(&text[..whole]).map_err(|_| "This screen’s history log is corrupt and couldn’t be read.".to_owned())?;
        let mut actions = Vec::new();
        for line in slice.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let act: Action = serde_json::from_str(line).map_err(|_| "This screen’s history log is corrupt and couldn’t be read.".to_owned())?;
            actions.push(act);
        }
        // If the entire file has non-whitespace content without a trailing newline,
        // verify it isn't a corrupt un-terminated record when nothing parsed yet.
        if whole == 0 && self.offset == 0 && !text.is_empty() && text.iter().any(|b| !b.is_ascii_whitespace()) {
            let tail_str = std::str::from_utf8(&text).map_err(|_| "This screen’s history log is corrupt and couldn’t be read.".to_owned())?;
            if serde_json::from_str::<Action>(tail_str.trim()).is_err() {
                return Err("This screen’s history log is corrupt and couldn’t be read.".to_owned());
            }
        }
        self.offset += whole as u64;
        Ok((actions, reset))
    }

    /// What is new in an already-verified regular `actions.jsonl` file.
    fn more_file(&mut self, f: &mut File) -> (Vec<Action>, bool) {
        let Ok(meta) = f.metadata() else { return (Vec::new(), false) };
        if !meta.file_type().is_file() {
            return (Vec::new(), false);
        }
        let id = (meta.dev(), meta.ino());
        let reset = self.inode != Some(id) || meta.len() < self.offset;
        if reset {
            self.inode = Some(id);
            self.offset = 0;
        }
        let mut text = Vec::new();
        let max_read = (MAX_BYTES * 2).saturating_sub(self.offset);
        if meta.len() > self.offset && max_read > 0 && f.seek(SeekFrom::Start(self.offset)).is_ok() {
            let _ = f.take(max_read).read_to_end(&mut text);
        }
        // Only whole lines: one being written is read the next time.
        let whole = text.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        self.offset += whole as u64;
        let actions = String::from_utf8_lossy(&text[..whole]).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        (actions, reset)
    }

    #[cfg(test)]
    fn more_from(&mut self, path: &Path) -> (Vec<Action>, bool) {
        use std::os::unix::fs::OpenOptionsExt as _;
        let Ok(mut f) = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path) else {
            let reset = self.inode.take().is_some();
            self.offset = 0;
            return (Vec::new(), reset);
        };
        self.more_file(&mut f)
    }
}

/// Forget screen `name`'s history.
pub fn clear(name: &str) -> Result<(), String> {
    clear_in(&root_dir(), name)
}

pub(crate) fn clear_in(root: &Path, name: &str) -> Result<(), String> {
    // Its actions and the frames kept with them, as Clear History says:
    // not the screen (its name, settings, archive mark) nor what its
    // programs keep there (Chrome's profile, the passwords shared with it).
    // Open via `O_DIRECTORY | O_NOFOLLOW` and unlink via `unlinkat(dfd, ...)`
    // so a symlinked screen directory can never clear another directory's files.
    let dfd = match open_screen_dir_fd(root, name) {
        Ok(d) => d,
        Err(_) => {
            // Propagate invalid name or symlink errors; ignore missing dir.
            let dir = screen_dir_in(root, name)?;
            if std::fs::symlink_metadata(&dir).is_err() {
                return Ok(());
            }
            return Err(format!("refusing to clear {name:?}: not a real screen directory"));
        }
    };
    let dir = root.join(name);
    let Ok(read) = std::fs::read_dir(&dir) else { return Ok(()) };
    for e in read.flatten() {
        let file = e.file_name().to_string_lossy().into_owned();
        if file == "actions.jsonl" || (file.starts_with("frames-") && file.ends_with(".olof")) {
            let Ok(cfile) = CString::new(file.as_bytes()) else { continue };
            // SAFETY: `dfd` is verified non-symlink; `unlinkat` unlinks strictly inside `dfd`.
            if unsafe { libc::unlinkat(dfd.as_raw_fd(), cfile.as_ptr(), 0) } != 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() != std::io::ErrorKind::NotFound {
                    return Err(format!("couldn't clear {name}'s history: {err}"));
                }
            }
        }
    }
    Ok(())
}

/// How much of each screen's history to keep (bytes) and for how long
/// (ms; None: for ever): Preferences ▸ AI as the person's session has it,
/// else the screen's own copy, else 512 MB for 30 days.
pub fn limits(screen: Option<&crate::screens::Screen>) -> (u64, Option<u64>) {
    use olo_protocol::{Client, Reply, Request};
    let ask = |c: Option<Client>| match c.map(|mut c| c.request(&Request::GetSettings)) {
        Some(Ok(Reply::Settings { settings })) => Some(settings.ai),
        _ => None,
    };
    let ai = ask(Client::connect().ok()).or_else(|| screen.and_then(|s| ask(Client::connect_to(&s.olo_socket).ok()))).unwrap_or_default();
    let days = u64::from(ai.screen_history_days);
    ((u64::from(ai.screen_history_mb.max(16))) << 20, (days > 0).then_some(days * 86_400_000))
}

/// Drop what is older than `max_age` from every screen's history, and
/// the histories left with nothing.
pub fn prune(max_age: Option<u64>) {
    let Some(age) = max_age else { return };
    let root = root_dir();
    let cutoff = now_ms().saturating_sub(age);
    for name in names_in(&root) {
        let Ok(dfd) = open_screen_dir_fd(&root, &name) else { continue };
        let Ok(sdir) = screen_dir_in(&root, &name) else { continue };
        let all = read_in(&root, &name);
        let keep: Vec<&Action> = all.iter().filter(|a| a.at >= cutoff).collect();
        if keep.len() != all.len() {
            let text: String = keep.iter().filter_map(|a| serde_json::to_string(a).ok()).map(|l| l + "\n").collect();
            let tmp_name = format!(".actions-prune.{}.tmp", std::process::id());
            if let Ok(mut tf) = openat_regular(&dfd, &tmp_name, libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC, 0o644)
                && tf.write_all(text.as_bytes()).is_ok()
                && tf.flush().is_ok()
            {
                drop(tf);
                let ctmp = CString::new(tmp_name).unwrap();
                let cdst = CString::new("actions.jsonl").unwrap();
                unsafe {
                    if libc::renameat(dfd.as_raw_fd(), ctmp.as_ptr(), dfd.as_raw_fd(), cdst.as_ptr()) != 0 {
                        libc::unlinkat(dfd.as_raw_fd(), ctmp.as_ptr(), 0);
                    }
                }
            }
        }
        olo_screen_codec::olof::prune(&sdir, cutoff);
        if keep.is_empty() {
            let _ = clear_in(&root, &name);
        }
    }
}

/// The screens with a history kept.
pub fn names() -> Vec<String> {
    names_in(&root_dir())
}

pub(crate) fn names_in(root: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(root)
        .map(|d| {
            d.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .filter_map(|e| e.file_name().to_str().map(str::to_owned))
                .filter(|name| crate::screens::valid_name(name))
                .filter(|name| open_screen_dir_fd(root, name).is_ok_and(|dfd| is_regular_at(&dfd, "actions.jsonl") || is_regular_at(&dfd, "screen.json")))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// A screen's use, from its history.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Stats {
    pub name: String,
    /// When it was first started and last acted on (ms since the epoch).
    pub started: Option<u64>,
    pub last_used: Option<u64>,
    /// How long it has run (from each start to its stop, or to now while
    /// it runs) and been in use (the stretches between actions shorter
    /// than `IDLE_MS`), in milliseconds.
    pub ran_ms: u64,
    pub active_ms: u64,
    /// Actions of each kind.
    pub actions: BTreeMap<String, usize>,
    /// The programs run on it, each with how many times.
    pub programs: BTreeMap<String, usize>,
    /// Actions that failed.
    pub failed: usize,
}

/// `history`'s stats, `running` saying whether the screen is up now.
pub fn stats(name: &str, history: &[Action], running: bool, now: u64) -> Stats {
    let mut s = Stats { name: name.to_owned(), ..Default::default() };
    let mut since: Option<u64> = None;
    let mut last_end: Option<u64> = None;
    for a in history {
        // (A new String only for a kind not seen yet: this runs over long
        // histories, in the window as they grow.)
        match s.actions.get_mut(&a.kind) {
            Some(n) => *n += 1,
            None => {
                s.actions.insert(a.kind.clone(), 1);
            }
        }
        s.failed += usize::from(a.error.is_some());
        if a.kind == "run"
            && let Some(p) = a.args.first()
        {
            let program = std::path::Path::new(p).file_name().and_then(|n| n.to_str()).unwrap_or(p).to_owned();
            *s.programs.entry(program).or_default() += 1;
        }
        match a.kind.as_str() {
            "start" if a.error.is_none() => {
                s.started.get_or_insert(a.at);
                since.get_or_insert(a.at);
            }
            "stop" => {
                if let Some(from) = since.take() {
                    s.ran_ms = s.ran_ms.saturating_add(a.at.saturating_sub(from));
                }
            }
            _ => {}
        }
        // Index-preserving oversized placeholders carry `at: 0, ms: 0` so skip
        // them in wall-clock duration / last-used / active-time calculations.
        if a.is_oversized() {
            continue;
        }
        // In use: an action's own time, and the gap before it when short.
        let end = a.at.saturating_add(a.ms);
        let gap = last_end.map(|e| a.at.saturating_sub(e)).filter(|g| *g < IDLE_MS).unwrap_or(0);
        s.active_ms = s.active_ms.saturating_add(a.ms.saturating_add(gap));
        last_end = Some(last_end.map_or(end, |e| e.max(end)));
        s.last_used = Some(s.last_used.map_or(end, |l| l.max(end)));
    }
    if running && let Some(from) = since {
        s.ran_ms = s.ran_ms.saturating_add(now.saturating_sub(from));
    }
    s
}

/// The local midnight that began the day of `ms` (since the epoch).
pub fn midnight(ms: u64) -> u64 {
    let secs = (ms / 1000) as libc::time_t;
    // SAFETY: localtime_r and mktime touch only `tm`, which outlives them.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    (tm.tm_hour, tm.tm_min, tm.tm_sec) = (0, 0, 0);
    let t = unsafe { libc::mktime(&mut tm) };
    (t.max(0) as u64) * 1000
}

/// `ms` (since the epoch) as the local time of day, to the millisecond.
pub fn clock(ms: u64) -> String {
    let secs = (ms / 1000) as libc::time_t;
    // SAFETY: localtime_r writes only into `tm`, which outlives the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec, ms % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(at: u64, ms: u64, kind: &str, args: &[&str], error: Option<&str>) -> Action {
        Action { at, ms, kind: kind.into(), args: args.iter().map(|a| (*a).to_owned()).collect(), result: None, error: error.map(str::to_owned) }
    }

    #[test]
    fn stats_from_a_history() {
        let h = vec![
            act(1_000, 900, "start", &[], None),
            act(5_000, 100, "run", &["/usr/bin/olo-styled-edit", "notes.txt"], None),
            act(6_000, 50, "click", &["--name", "Save"], None),
            act(7_000, 30, "click", &["--name", "Nope"], Some("nothing matches")),
            // Ten minutes later: the gap isn't use.
            act(607_000, 40, "key", &["Cmd+S"], None),
            act(608_000, 10, "stop", &[], None),
        ];
        let s = stats("agent", &h, false, 700_000);
        assert_eq!((s.started, s.last_used), (Some(1_000), Some(608_010)));
        assert_eq!(s.ran_ms, 607_000);
        assert_eq!(s.actions["click"], 2);
        assert_eq!(s.programs["olo-styled-edit"], 1);
        assert_eq!(s.failed, 1);
        // The actions' own time and the short gaps between them, not the
        // ten minutes.
        assert_eq!(s.active_ms, 900 + 100 + 50 + 30 + 40 + 10 + (5_000 - 1_900) + (6_000 - 5_100) + (7_000 - 6_050) + (608_000 - 607_040));
        // Still running: up to now.
        let running = stats("agent", &h[..5], true, 700_000);
        assert_eq!(running.ran_ms, 699_000);
    }

    #[test]
    fn reads_what_was_added() {
        let state = std::path::PathBuf::from(format!("/var/tmp/olo-screen-pane-history-{}", std::process::id()));
        let root = state.join("olo/screens");
        let _ = std::fs::remove_dir_all(&state);
        let name = "reader-test";
        let sdir = screen_dir_in(&root, name).unwrap();
        assert!(sdir.starts_with(&state), "test path escaped scratch folder: {}", sdir.display());
        let path = sdir.join("actions.jsonl");
        let mut r = Reader::default();
        assert_eq!(r.more_from(&path), (vec![], false));
        record_in(&root, name, &act(1, 1, "start", &[], None));
        record_in(&root, name, &act(2, 1, "click", &["--name", "Save"], None));
        let (got, reset) = r.more_from(&path);
        assert!(reset);
        assert_eq!(got.len(), 2);
        assert_eq!(r.more_from(&path), (vec![], false));
        // A line half written waits for its end.
        let line = serde_json::to_string(&act(3, 1, "key", &["Cmd+S"], None)).unwrap();
        let (head, tail) = line.split_at(10);
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(head.as_bytes()).unwrap();
        assert_eq!(r.more_from(&path), (vec![], false));
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(format!("{tail}\n").as_bytes()).unwrap();
        assert_eq!(r.more_from(&path), (vec![act(3, 1, "key", &["Cmd+S"], None)], false));
        // Cut down (or replaced): all of it again.
        std::fs::write(&path, serde_json::to_string(&act(9, 1, "stop", &[], None)).unwrap() + "\n").unwrap();
        let (got, reset) = r.more_from(&path);
        assert!(reset && got.len() == 1 && got[0].kind == "stop");
        // Clear History takes its actions and frames, not the screen nor
        // what its programs keep there.
        std::fs::write(sdir.join("frames-0000000000000001.olof"), b"x").unwrap();
        for kept in ["screen.json", "title", "passwords.vault"] {
            std::fs::write(sdir.join(kept), b"{}").unwrap();
        }
        std::fs::create_dir_all(sdir.join("google-chrome")).unwrap();
        clear_in(&root, name).unwrap();
        assert_eq!(r.more_from(&path), (vec![], true));
        let mut left: Vec<String> = std::fs::read_dir(&sdir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["google-chrome", "passwords.vault", "screen.json", "title"]);
        // Listed still, with no history: a screen made and stopped
        // before an agent did anything on it.
        assert!(names_in(&root).contains(&name.to_owned()), "{:?}", names_in(&root));
        std::fs::create_dir_all(root.join("not-a-screen")).unwrap();
        assert!(!names_in(&root).contains(&"not-a-screen".to_owned()));
        // What another program writes of a screen (Passwords: a passkey
        // used there without asking) is verified in an isolated child process
        // with XDG_STATE_HOME set at launch (zero env mutation in this process).
        let exe = std::env::current_exe().unwrap();
        let child = std::process::Command::new(exe)
            .args(["history::tests::reads_keychain_log_child", "--ignored", "--exact"])
            .env("XDG_STATE_HOME", &state)
            .env("OLO_HISTORY_CHILD_NAME", name)
            .status()
            .unwrap();
        assert!(child.success(), "child keychain log check failed");
        let (got, _) = r.more_from(&path);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].kind.as_str(), got[0].args.as_slice()), ("passkey", &["example.com".to_owned(), "Example (ann)".to_owned()][..]));
        assert!(got[0].line().contains("passkey example.com \"Example (ann)\""), "{}", got[0].line());
        assert!(got[0].mentions("example.com") && got[0].at > 1_700_000_000_000);
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    #[ignore]
    fn reads_keychain_log_child() {
        let state = std::env::var_os("XDG_STATE_HOME").expect("XDG_STATE_HOME must point to scratch dir");
        assert!(Path::new(&state).starts_with("/var/tmp"), "refusing to run outside /var/tmp: {:?}", state);
        let name = std::env::var("OLO_HISTORY_CHILD_NAME").unwrap_or_else(|_| "reader-test".into());
        olo_keychain::shared::log(&name, "passkey", &["example.com".into(), "Example (ann)".into()]);
    }

    #[test]
    fn astra_f24_history_ops_reject_path_escape_in_scratch() {
        let scratch = std::path::PathBuf::from(format!("/var/tmp/olo-screen-pane-f24-hist-{}", std::process::id()));
        let root = scratch.join("screens");
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&root).unwrap();

        for bad in ["../escape", "a/b", "", ".", ".."] {
            assert!(screen_dir_in(&root, bad).is_err(), "screen_dir_in must reject {bad:?}");
            assert!(set_title_in(&root, bad, Some("oops")).is_err(), "set_title_in must reject {bad:?}");
            assert!(set_archived_in(&root, bad, true).is_err(), "set_archived_in must reject {bad:?}");
            assert!(clear_in(&root, bad).is_err(), "clear_in must reject {bad:?}");
            assert!(trash_in(&root, bad).is_err(), "trash_in must reject {bad:?}");
        }
        // Guard: nothing escaped into `scratch` outside `screens/`.
        assert!(!scratch.join("escape").exists());
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn lines_say_what_was_done_and_found() {
        let mut a = act(0, 42, "click", &["--name", "Save As…"], None);
        a.result = Some(serde_json::json!({"matched": {"role": "Button", "name": "Save", "center": [640.0, 320.0]}}));
        assert!(a.line().ends_with("click --name \"Save As…\"  → Button \"Save\" at 640,320  (42 ms)"), "{}", a.line());
        assert!(a.mentions("button") && a.mentions("save as") && !a.mentions("tracker"));
        let failed = act(0, 5, "click", &["--name", "Nope"], Some("nothing matches"));
        assert!(failed.line().contains("✗ nothing matches"));
    }

    #[test]
    fn review2_f6_symlink_screen_dir_rejected_and_clear_leaves_target_intact() {
        let scratch = std::path::PathBuf::from(format!("/var/tmp/olo-screen-pane-r2-f6-{}", std::process::id()));
        let root = scratch.join("screens");
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&root).unwrap();

        // Real screen `victim` with actions and a recorded `.olof` file.
        record_in(&root, "victim", &act(1_000, 10, "click", &[], None));
        let victim_dir = root.join("victim");
        let victim_olof = victim_dir.join("frames-0000000000001000.olof");
        std::fs::write(&victim_olof, b"OLOFRM1\n").unwrap();

        // Symlink `screens/alias -> screens/victim`.
        std::os::unix::fs::symlink(&victim_dir, root.join("alias")).unwrap();

        assert!(!names_in(&root).contains(&"alias".to_owned()), "symlink dir must not be listed by names_in");
        assert!(names_in(&root).contains(&"victim".to_owned()));
        assert!(read_in(&root, "alias").is_empty(), "read_in must refuse symlink screen dir");
        assert!(clear_in(&root, "alias").is_err(), "clear_in must refuse symlink screen dir");
        assert!(set_title_in(&root, "alias", Some("Hijacked")).is_err(), "set_title_in must refuse symlink screen dir");
        assert!(set_archived_in(&root, "alias", true).is_err(), "set_archived_in must refuse symlink screen dir");

        // Victim's actions and recordings MUST remain completely intact!
        assert_eq!(read_in(&root, "victim").len(), 1);
        assert!(victim_olof.is_file(), "victim's .olof must not be deleted through symlink alias");
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn review2_f55_stats_saturates_overflowing_action_timestamps() {
        let h = vec![act(u64::MAX - 5, 100, "start", &[], None), act(u64::MAX - 2, u64::MAX, "click", &[], None)];
        let s = stats("overflow", &h, true, u64::MAX);
        assert_eq!(s.last_used, Some(u64::MAX));
        assert_eq!(s.active_ms, u64::MAX);
    }

    #[test]
    fn oversized_placeholder_detected_formatted_and_skipped_in_wall_clock_stats() {
        let oversized = Action { at: 0, ms: 0, kind: "oversized".into(), args: vec![], result: None, error: None };
        assert!(oversized.is_oversized());
        assert_eq!(oversized.line(), "—  Action too large to show");
        let normal = act(1_750_000_000_000, 25, "click", &["--name", "OK"], None);
        assert!(!normal.is_oversized());
        let h = vec![normal.clone(), oversized, act(1_750_000_005_000, 15, "type", &["hi"], None)];
        let s = stats("qa", &h, false, 1_750_000_010_000);
        assert_eq!(s.actions.values().sum::<usize>(), 3);
        assert_eq!(s.last_used, Some(1_750_000_005_015));
        assert_eq!(s.active_ms, 5_015);
    }

    #[test]
    fn round3_f20_f23_root_and_last_used_fast_tail() {
        // Finding 20: history::root() must sit directly under olo_xdg::state_dir()/screens,
        // never double-nesting `olo/olo/screens`.
        let r = root();
        assert_eq!(r, olo_xdg::state_dir().join("screens"));
        let s = r.to_string_lossy();
        assert!(!s.ends_with("/olo/olo/screens"), "unexpected double-nested history root: {s}");

        // Finding 23: last_used_fast reads only the <=64 KiB tail and matches full read_in
        // even when actions.jsonl is well over 64 KiB (and ignores trailing oversized placeholders).
        let scratch = PathBuf::from("/var/tmp").join(format!("olo-hist-r3-f23-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(scratch.join("big")).unwrap();
        let mut content = String::new();
        for i in 0..800u64 {
            let line = serde_json::to_string(&act(1_750_000_000_000 + i * 1000, 15, "type", &[&format!("padding-payload-{i:04}-{}", "x".repeat(80))], None)).unwrap();
            content.push_str(&line);
            content.push('\n');
        }
        assert!(content.len() > 80 * 1024, "fixture must exceed 64 KiB tail window");
        let last_ts = 1_750_000_000_000 + 799 * 1000 + 15;
        // Append an oversized placeholder after the final real action: must still find `last_ts`.
        content.push_str(&serde_json::to_string(&Action { at: 0, ms: 0, kind: "oversized".into(), args: vec![], result: None, error: None }).unwrap());
        content.push('\n');
        std::fs::write(scratch.join("big/actions.jsonl"), content).unwrap();

        assert_eq!(last_used_fast_in(&scratch, "big"), Some(last_ts));
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn final_pass_b11_f23_last_used_fast_handles_single_line_over_64kib_and_rejects_corrupt() {
        let scratch = PathBuf::from("/var/tmp").join(format!("olo-hist-b11-f23-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(scratch.join("huge")).unwrap();

        // Single action line whose payload is > 90 KiB (exceeds 64 KiB tail read):
        // bounded reverse backscan must still find its timestamp without parsing a whole 16 MiB file.
        let earlier = serde_json::to_string(&act(1_750_000_000_000, 10, "start", &[], None)).unwrap();
        let big_payload = "Z".repeat(96 * 1024);
        let huge_line = serde_json::to_string(&act(1_750_000_050_000, 42, "type", &[&big_payload], None)).unwrap();
        assert!(huge_line.len() > 90 * 1024, "test line must exceed 64 KiB");
        std::fs::write(scratch.join("huge/actions.jsonl"), format!("{earlier}\n{huge_line}\n")).unwrap();

        assert_eq!(last_used_fast_checked_in(&scratch, "huge").unwrap(), Some(1_750_000_050_042));

        // Corrupt non-empty file must return Err (never Ok(None)).
        std::fs::create_dir_all(scratch.join("corrupt")).unwrap();
        std::fs::write(scratch.join("corrupt/actions.jsonl"), "garbage-not-json\n").unwrap();
        assert!(last_used_fast_checked_in(&scratch, "corrupt").is_err());

        // Missing or empty file returns Ok(None).
        std::fs::create_dir_all(scratch.join("empty")).unwrap();
        std::fs::write(scratch.join("empty/actions.jsonl"), "").unwrap();
        assert_eq!(last_used_fast_checked_in(&scratch, "empty").unwrap(), None);
        assert_eq!(last_used_fast_checked_in(&scratch, "absent").unwrap(), None);

        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn agy_r1_f6_corrupt_trailing_line_and_oversized_kind_in_args_and_placeholder_tail() {
        let scratch = PathBuf::from("/var/tmp").join(format!("olo-hist-agyr1-f6-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);

        // 1. Valid line 1 followed by truncated/corrupt line 2 MUST fail closed (never return line 1's timestamp).
        std::fs::create_dir_all(scratch.join("trailing-corrupt")).unwrap();
        let valid = serde_json::to_string(&act(1_750_000_000_000, 10, "click", &[], None)).unwrap();
        std::fs::write(scratch.join("trailing-corrupt/actions.jsonl"), format!("{valid}\n{{\"at\":1750000010000,\"ms\":\n")).unwrap();
        assert!(last_used_fast_checked_in(&scratch, "trailing-corrupt").is_err(), "trailing corrupt line must fail closed");

        // 2. >64 KiB valid action whose `args` contain literal `"kind":"oversized"` must still return its timestamp!
        std::fs::create_dir_all(scratch.join("args-literal")).unwrap();
        let payload = format!(r#"prefix "kind":"oversized" {}"#, "A".repeat(80 * 1024));
        let huge_with_literal = serde_json::to_string(&act(1_750_000_100_000, 33, "type", &[&payload], None)).unwrap();
        assert!(huge_with_literal.len() > 70 * 1024);
        std::fs::write(scratch.join("args-literal/actions.jsonl"), format!("{valid}\n{huge_with_literal}\n")).unwrap();
        assert_eq!(last_used_fast_checked_in(&scratch, "args-literal").unwrap(), Some(1_750_000_100_033));

        // 3. >64 KiB file ending with 70 KiB of valid `"oversized"` placeholders after an earlier real action
        //    scans past placeholders to find the real timestamp; a >64 KiB file of ONLY placeholders returns Ok(None).
        let ph_line = serde_json::to_string(&Action { at: 0, ms: 0, kind: "oversized".into(), args: vec![], result: None, error: None }).unwrap();
        let repeat_n = (72 * 1024) / (ph_line.len() + 1) + 2;
        let ph_block = format!("{ph_line}\n").repeat(repeat_n);
        assert!(ph_block.len() > 70 * 1024);

        std::fs::create_dir_all(scratch.join("ph-after-real")).unwrap();
        std::fs::write(scratch.join("ph-after-real/actions.jsonl"), format!("{valid}\n{ph_block}")).unwrap();
        assert_eq!(last_used_fast_checked_in(&scratch, "ph-after-real").unwrap(), Some(1_750_000_000_010));

        std::fs::create_dir_all(scratch.join("ph-only")).unwrap();
        std::fs::write(scratch.join("ph-only/actions.jsonl"), &ph_block).unwrap();
        assert_eq!(last_used_fast_checked_in(&scratch, "ph-only").unwrap(), None);

        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn agy_r2_f1_tail_read_capped_at_snapshotted_len_survives_concurrent_append() {
        use std::io::Write as _;
        let scratch = PathBuf::from("/var/tmp").join(format!("olo-hist-agyr2-f1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).unwrap();

        // 1. > 64 KiB log: snapshot `len` BEFORE a concurrent append writes another JSON line.
        //    Reading `len - start` bytes (never `TAIL_BYTES + 1`) must NOT slice the appended `{`.
        let p_big = scratch.join("big-actions.jsonl");
        let mut content = String::new();
        for i in 0..700u64 {
            let line = serde_json::to_string(&act(1_750_000_000_000 + i * 1000, 15, "type", &[&format!("pad-{i:04}-{}", "x".repeat(80))], None)).unwrap();
            content.push_str(&line);
            content.push('\n');
        }
        assert!(content.len() > 64 * 1024);
        std::fs::write(&p_big, &content).unwrap();
        let snap_len = std::fs::metadata(&p_big).unwrap().len();
        let expected_ts = 1_750_000_000_000 + 699 * 1000 + 15;
        // Simulate concurrent `history::record_in` appending after `meta.len()` was snapshotted.
        let appended = serde_json::to_string(&act(1_750_009_999_000, 12, "click", &["10", "20"], None)).unwrap();
        let mut appender = std::fs::OpenOptions::new().append(true).open(&p_big).unwrap();
        writeln!(appender, "{appended}").unwrap();
        appender.flush().unwrap();
        let mut reader = std::fs::File::open(&p_big).unwrap();
        assert_eq!(last_used_fast_checked_from_file(&mut reader, snap_len, "qa").unwrap(), Some(expected_ts));

        // 2. < 64 KiB log whose snapshotted length sits near 64 KiB so a concurrent append crosses 65,537 bytes.
        let p_near = scratch.join("near-64k.jsonl");
        let filler = serde_json::to_string(&act(1_750_000_100_000, 20, "type", &[&"y".repeat(65_400)], None)).unwrap();
        let near_content = format!("{filler}\n");
        assert!(near_content.len() < 64 * 1024 && near_content.len() > 65_400);
        std::fs::write(&p_near, &near_content).unwrap();
        let near_snap_len = std::fs::metadata(&p_near).unwrap().len();
        let mut appender2 = std::fs::OpenOptions::new().append(true).open(&p_near).unwrap();
        writeln!(appender2, "{}", serde_json::to_string(&act(1_750_009_999_500, 5, "type", &[&"z".repeat(200)], None)).unwrap()).unwrap();
        appender2.flush().unwrap();
        let mut reader2 = std::fs::File::open(&p_near).unwrap();
        assert_eq!(last_used_fast_checked_from_file(&mut reader2, near_snap_len, "qa").unwrap(), Some(1_750_000_100_020));

        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn r3_polish_astra_f10_neon_f7_reader_more_checked_fails_closed_on_unreadable_and_corrupt() {
        use std::os::unix::fs::PermissionsExt as _;
        let scratch = PathBuf::from("/var/tmp").join(format!("olo-hist-r3-f10-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(scratch.join("qa")).unwrap();

        // 1. Absent or empty file -> Ok(([], _)) (verified empty).
        let mut reader = Reader::default();
        assert_eq!(reader.more_checked_in(&scratch, "qa").unwrap().0.len(), 0);
        let log_path = scratch.join("qa/actions.jsonl");
        std::fs::write(&log_path, "").unwrap();
        assert_eq!(reader.more_checked_in(&scratch, "qa").unwrap().0.len(), 0);

        // 2. Corrupt JSON line -> Err (never silent Ok([])).
        std::fs::write(&log_path, "not-valid-json\n").unwrap();
        let err = reader.more_checked_in(&scratch, "qa").unwrap_err();
        assert!(err.contains("corrupt") || err.contains("unreadable"), "expected corrupt error, got: {err}");

        // 3. Valid line followed by corrupt line -> Err (fails closed).
        let valid = serde_json::to_string(&act(1_750_000_000_000, 12, "click", &["10", "20"], None)).unwrap();
        std::fs::write(&log_path, format!("{valid}\n{{broken\n")).unwrap();
        assert!(reader.more_checked_in(&scratch, "qa").is_err(), "corrupt trailing line must return Err");

        // 4. Unreadable (chmod 000) -> Err when running as non-root.
        std::fs::write(&log_path, format!("{valid}\n")).unwrap();
        std::fs::set_permissions(&log_path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&log_path).is_err() {
            assert!(reader.more_checked_in(&scratch, "qa").is_err(), "unreadable actions.jsonl must return Err");
        }
        std::fs::set_permissions(&log_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        // 5. Once repaired, reader recovers cleanly with reset = true and returns the valid action.
        let (recovered, reset) = reader.more_checked_in(&scratch, "qa").expect("recover after repair");
        assert!(reset, "recovery after unreadability/corruption must emit reset = true");
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].kind, "click");

        let _ = std::fs::remove_dir_all(&scratch);
    }
}
