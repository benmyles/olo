//! `olo-rex-link fs <op> [ARGS…]`: native Rust filesystem helper on a Rex host
//! used by the Olo desktop's Tracker integration (`rex://HOST/PATH`) through
//! Rex's SSH connection pool. Never shells out to system utilities.
//!
//! Subcommands:
//!   list PATH [--hidden]                         list directory entries + filesystem free space (JSON)
//!   stat PATH                                    stat a single path, resolving `~` / relative paths (JSON)
//!   mkdir PATH [--unique] [--private]            create directory (`--unique`: append ` 2`, ` 3`…; `--private`: 0700)
//!   rename FROM TO [--unique] [--default-mode] [--mode OCTAL]
//!                                                rename or move within this host
//!   copy FROM TO                                 recursively copy file, symlink, or directory via private stage
//!   remove PATH [--stage] [--expect-dev D] [--expect-ino I] [--expect-size S]
//!               [--expect-mtime-secs S] [--expect-mtime-nanos N]
//!               [--expect-ctime-secs S] [--expect-ctime-nanos N]
//!                                                permanently remove file, symlink, or directory (refuses `/` and `$HOME`;
//!                                                verified `--expect-*` removes only matching files, symlinks, or empty dirs;
//!                                                `--stage` repairs read-only permissions inside owned private stage trees only)
//!   read PATH [--max N] [--expect-dev D --expect-ino I --expect-size S --expect-mtime-secs S --expect-mtime-nanos N --expect-ctime-secs S --expect-ctime-nanos N]
//!                                                stream file bytes directly to stdout (optional byte cap & inode check)
//!   write PATH [--unique] [--len N]              atomically write stdin bytes to PATH

use std::ffi::{CStr, CString};
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

const USAGE: &str = "\
usage: olo-rex-link fs list PATH [--hidden]
       olo-rex-link fs stat PATH
       olo-rex-link fs mkdir PATH [--unique] [--private]
       olo-rex-link fs rename FROM TO [--unique] [--default-mode] [--mode OCTAL]
       olo-rex-link fs copy FROM TO
       olo-rex-link fs remove PATH [--stage] [--expect-dev D] [--expect-ino I] [--expect-size S] [--expect-mtime-secs S] [--expect-mtime-nanos N] [--expect-ctime-secs S] [--expect-ctime-nanos N]
       olo-rex-link fs read PATH [--max N] [--expect-dev D] [--expect-ino I] [--expect-size S] [--expect-mtime-secs S] [--expect-mtime-nanos N] [--expect-ctime-secs S] [--expect-ctime-nanos N]
       olo-rex-link fs write PATH [--unique] [--len N]";

/// Maximum directory entries returned in a single JSON listing response.
const MAX_LIST_ENTRIES: usize = 100_000;

#[derive(Debug, Clone, Serialize)]
pub struct FsEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    pub mtime_secs: i64,
    pub mtime_nanos: u32,
    pub ctime_secs: i64,
    pub ctime_nanos: u32,
    pub dev: u64,
    pub ino: u64,
    pub mode: u32,
    /// `lstat` (non-following) identity fields so verified removal can check the
    /// directory entry itself (including valid and dangling symlinks) without
    /// conflating it with a symlink target's followed metadata.
    pub lstat_dev: u64,
    pub lstat_ino: u64,
    pub lstat_size: u64,
    pub lstat_mtime_secs: i64,
    pub lstat_mtime_nanos: u32,
    pub lstat_ctime_secs: i64,
    pub lstat_ctime_nanos: u32,
}

fn home_dir() -> io::Result<PathBuf> {
    match std::env::var_os("HOME") {
        Some(h) if !h.is_empty() => Ok(PathBuf::from(h)),
        _ => Err(io::Error::new(io::ErrorKind::NotFound, "HOME is not set")),
    }
}

/// Resolve `~`, `~/…`, or relative paths against `$HOME`, normalizing `.` / `..`.
/// Fails explicitly if a home-relative path is given while `$HOME` is unset.
pub fn try_resolve_path(raw: &str) -> io::Result<PathBuf> {
    let expanded = if raw == "~" {
        home_dir()?
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home_dir()?.join(rest)
    } else {
        let p = PathBuf::from(raw);
        if p.is_relative() { home_dir()?.join(p) } else { p }
    };
    Ok(normalize_path(&expanded))
}

#[cfg(test)]
pub fn resolve_path(raw: &str) -> PathBuf {
    try_resolve_path(raw).unwrap_or_else(|_| PathBuf::from(raw))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::RootDir => out.push("/"),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::Normal(seg) => out.push(seg),
            std::path::Component::Prefix(p) => out.push(p.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() { PathBuf::from("/") } else { out }
}

fn stat_entry(path: &Path) -> io::Result<FsEntry> {
    let lmeta = fs::symlink_metadata(path)?;
    let is_link = lmeta.file_type().is_symlink();
    let lstat_dev = lmeta.dev();
    let lstat_ino = lmeta.ino();
    let lstat_size = lmeta.size();
    let lstat_mtime_secs = lmeta.mtime();
    let lstat_mtime_nanos = lmeta.mtime_nsec().clamp(0, 999_999_999) as u32;
    let lstat_ctime_secs = lmeta.ctime();
    let lstat_ctime_nanos = lmeta.ctime_nsec().clamp(0, 999_999_999) as u32;
    let meta = if is_link { fs::metadata(path).unwrap_or(lmeta) } else { lmeta };
    let is_dir = meta.is_dir();
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into());
    Ok(FsEntry {
        name,
        is_dir,
        is_link,
        size: if is_dir { 0 } else { meta.size() },
        mtime_secs: meta.mtime(),
        mtime_nanos: meta.mtime_nsec().clamp(0, 999_999_999) as u32,
        ctime_secs: meta.ctime(),
        ctime_nanos: meta.ctime_nsec().clamp(0, 999_999_999) as u32,
        dev: meta.dev(),
        ino: meta.ino(),
        mode: meta.permissions().mode(),
        lstat_dev,
        lstat_ino,
        lstat_size,
        lstat_mtime_secs,
        lstat_mtime_nanos,
        lstat_ctime_secs,
        lstat_ctime_nanos,
    })
}

#[cfg(unix)]
fn fs_space(path: &Path) -> Option<(u64, u64)> {
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is NUL-terminated and `st` is valid for statvfs to fill.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let fr = st.f_frsize as u64;
    Some(((st.f_bavail as u64).saturating_mul(fr), (st.f_blocks as u64).saturating_mul(fr)))
}

fn unique_candidates(base: &Path) -> impl Iterator<Item = PathBuf> {
    let parent = base.parent().unwrap_or(Path::new("/")).to_path_buf();
    let name = base.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "New Folder".into());
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !e.is_empty() => (s.to_owned(), format!(".{e}")),
        _ => (name, String::new()),
    };
    let first = base.to_path_buf();
    std::iter::once(first).chain((2..=10_000u32).map(move |n| parent.join(format!("{stem} {n}{ext}"))))
}

#[cfg(test)]
fn unique_path(base: &Path) -> PathBuf {
    for cand in unique_candidates(base) {
        if fs::symlink_metadata(&cand).is_err() {
            return cand;
        }
    }
    base.to_path_buf()
}

/// Atomically rename `src` to `dst` without replacing any existing file,
/// directory, or dangling symlink at `dst` (`renameat2(RENAME_NOREPLACE)`).
/// When the filesystem lacks `RENAME_NOREPLACE`, uses atomic `hard_link` +
/// source unlink for regular files and atomic `symlink` + unlink for symlinks,
/// and fails closed with `Unsupported` when neither native no-replace rename
/// nor hard links are supported (never copying bytes directly into the final
/// pathname). If linking `dst` succeeds and unlinking `src` fails, `dst` is
/// left intact so a concurrent replacement is never unlinked.
fn rename_noreplace(src: &Path, dst: &Path) -> io::Result<()> {
    rename_noreplace_inner(src, dst, false, false)
}

fn rename_noreplace_inner(src: &Path, dst: &Path, force_no_flag: bool, force_no_hardlink: bool) -> io::Result<()> {
    let err = if force_no_flag {
        io::Error::from_raw_os_error(libc::EINVAL)
    } else {
        let c_src = CString::new(src.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL byte"))?;
        let c_dst = CString::new(dst.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL byte"))?;
        // SAFETY: both pointers are valid NUL-terminated paths; AT_FDCWD is standard.
        let r = unsafe { libc::renameat2(libc::AT_FDCWD, c_src.as_ptr(), libc::AT_FDCWD, c_dst.as_ptr(), libc::RENAME_NOREPLACE) };
        if r == 0 {
            return Ok(());
        }
        io::Error::last_os_error()
    };
    if err.raw_os_error() == Some(libc::EINVAL)
        && !force_no_flag
        && let (Ok(cs), Some(dp)) = (fs::canonicalize(src), dst.parent().and_then(|p| fs::canonicalize(p).ok()))
        && dp.starts_with(&cs)
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "cannot move a folder inside itself"));
    }
    if matches!(err.raw_os_error(), Some(libc::ENOSYS | libc::EINVAL | libc::EOPNOTSUPP)) {
        let lmeta = fs::symlink_metadata(src)?;
        if lmeta.file_type().is_file() {
            let link_res = if force_no_hardlink { Err(io::Error::from_raw_os_error(libc::EOPNOTSUPP)) } else { fs::hard_link(src, dst) };
            match link_res {
                Ok(()) => {
                    if let Err(e) = fs::remove_file(src) {
                        // Keep `dst` intact: it is already published and another
                        // caller may now own or have replaced it.
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
            let target = fs::read_link(src)?;
            symlink(&target, dst)?;
            if let Err(e) = fs::remove_file(src) {
                return Err(io::Error::new(e.kind(), format!("destination symlink published at {}, but removing source failed: {e}", dst.display())));
            }
            return Ok(());
        }
        // Directories have no race-free non-replacing rename fallback without
        // RENAME_NOREPLACE: fail closed rather than risking clobbering a peer.
        return Err(io::Error::new(io::ErrorKind::Unsupported, "atomic no-replace rename is not supported for directories on this filesystem"));
    }
    Err(err)
}

/// Atomically publish `src` at `base` or the first free `"{stem} N{ext}"`
/// sibling using no-replace renames, retrying on `EEXIST` collisions.
fn rename_unique(src: &Path, base: &Path) -> io::Result<PathBuf> {
    for cand in unique_candidates(base) {
        match rename_noreplace(src, &cand) {
            Ok(()) => return Ok(cand),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "could not find a free filename"))
}

/// Reads the current process umask on Linux without mutating global state
/// (defaults to `0o022` if unavailable).
fn process_umask() -> u32 {
    fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find_map(|l| l.strip_prefix("Umask:")).and_then(|v| u32::from_str_radix(v.trim(), 8).ok())).unwrap_or(0o022)
}

/// Ordinary new-file mode for this process (`0o666 & !umask`), matching what
/// `open(O_CREAT, 0666)` / `fs write` produces for fresh files.
fn default_file_mode() -> u32 {
    0o666 & !process_umask()
}

fn mkdir_one(path: &Path, private: bool) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    let mut b = fs::DirBuilder::new();
    if private {
        b.mode(0o700);
    }
    b.create(path)
}

fn mkdir_unique(base: &Path, private: bool) -> io::Result<PathBuf> {
    for cand in unique_candidates(base) {
        match mkdir_one(&cand, private) {
            Ok(()) => return Ok(cand),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "could not find a free folder name"))
}

struct DirGuard(*mut libc::DIR);
impl Drop for DirGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `self.0` was returned by `libc::fdopendir` and is closed once here.
            unsafe { libc::closedir(self.0) };
        }
    }
}

fn is_owned_stage_name(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(".olo-tx-") || n.starts_with(".olo-fs-copy-"))
}

/// Remove an owned private staging directory recursively, repairing read-only
/// subdirectory permissions strictly via `openat(O_DIRECTORY | O_NOFOLLOW)` and
/// `fchmod` on opened directory descriptors (never path-following `chmod`, and
/// never used on user-visible trees).
fn remove_stage_dir_fd(dir: &Path) -> io::Result<()> {
    let c_dir = CString::new(dir.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL byte"))?;
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    // SAFETY: `c_dir` is NUL-terminated.
    let raw_fd = unsafe { libc::open(c_dir.as_ptr(), flags) };
    if raw_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw_fd` is a freshly opened valid descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    empty_stage_dir_fd(fd)?;
    fs::remove_dir(dir)
}

fn empty_stage_dir_fd(fd: OwnedFd) -> io::Result<()> {
    use std::os::fd::IntoRawFd as _;
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
    let guard = DirGuard(dirp);
    // SAFETY: `guard.0` is a live `DIR*`.
    let dir_fd = unsafe { libc::dirfd(guard.0) };
    let mut names: Vec<CString> = Vec::new();
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
        let cname = unsafe { CStr::from_ptr((*ent).d_name.as_ptr()) };
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
            // SAFETY: `dir_fd` is valid and `name` is a single filename component.
            let mut child_raw = unsafe { libc::openat(dir_fd, name.as_ptr(), open_flags) };
            if child_raw < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EACCES) {
                // Handle a 0000 child directory inside our own private 0700 stage.
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

fn copy_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    let lmeta = fs::symlink_metadata(src)?;
    if lmeta.file_type().is_symlink() {
        let target = fs::read_link(src)?;
        symlink(&target, dst)?;
        return Ok(());
    }
    if lmeta.is_dir() {
        // Refuse copying a directory inside itself.
        if let (Ok(cs), Some(dp)) = (fs::canonicalize(src), dst.parent().and_then(|p| fs::canonicalize(p).ok()))
            && dp.starts_with(&cs)
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "cannot copy a folder inside itself"));
        }
        mkdir_one(dst, true)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dst.join(entry.file_name()))?;
        }
        fs::set_permissions(dst, fs::Permissions::from_mode(lmeta.permissions().mode() & 0o3777))?;
        return Ok(());
    }
    if !lmeta.file_type().is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "cannot copy special file (FIFO, socket, or device)"));
    }
    let mut reader = fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(src)?;
    if !reader.metadata()?.file_type().is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "cannot copy special file (FIFO, socket, or device)"));
    }
    let mut writer = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(dst)?;
    io::copy(&mut reader, &mut writer)?;
    writer.flush()?;
    drop(writer);
    fs::set_permissions(dst, fs::Permissions::from_mode(lmeta.permissions().mode() & 0o777))?;
    Ok(())
}

/// Copy `src` into a private `0700` sibling stage beside `base_to`, then
/// atomically publish the finished copy at `base_to` (or a free `"{stem} N{ext}"`
/// sibling). Leaves no partial destination behind on failure or disk-full.
fn copy_staged(src: &Path, base_to: &Path) -> io::Result<PathBuf> {
    static COPY_SEQ: AtomicU64 = AtomicU64::new(0);
    let parent = base_to.parent().unwrap_or(Path::new("/"));
    fs::create_dir_all(parent)?;
    // Refuse copying a directory inside itself before allocating a stage.
    let lmeta = fs::symlink_metadata(src)?;
    if lmeta.is_dir()
        && let (Ok(cs), Ok(dp)) = (fs::canonicalize(src), fs::canonicalize(parent))
        && dp.starts_with(&cs)
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "cannot copy a folder inside itself"));
    }
    let stage_dir = {
        let mut created = None;
        for _ in 0..128 {
            let seq = COPY_SEQ.fetch_add(1, Ordering::Relaxed);
            let cand = parent.join(format!(".olo-fs-copy-{:08x}-{:08x}", std::process::id(), seq));
            match mkdir_one(&cand, true) {
                Ok(()) => {
                    created = Some(cand);
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        created.ok_or_else(|| io::Error::new(io::ErrorKind::AlreadyExists, "could not create copy staging directory"))?
    };
    let stage_item = stage_dir.join("item");
    let res = (|| -> io::Result<PathBuf> {
        copy_recursive(src, &stage_item)?;
        rename_unique(&stage_item, base_to)
    })();
    let _ = remove_stage_dir_fd(&stage_dir);
    res
}

/// Copies `input` into `f`, bounding reads immediately when `whole = Some(expected)`:
/// returns `UnexpectedEof` if fewer than `expected` bytes arrive, or `InvalidData`
/// immediately upon seeing 1 excess byte without writing further bytes to disk.
fn copy_bounded(mut input: impl Read, f: &mut fs::File, whole: Option<u64>) -> io::Result<u64> {
    match whole {
        Some(expected) => {
            let n = io::copy(&mut input.by_ref().take(expected), f)?;
            if n < expected {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("{n} of {expected} bytes came: nothing was written")));
            }
            let mut extra = [0u8; 1];
            loop {
                match input.read(&mut extra) {
                    Ok(0) => break,
                    Ok(_) => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("more than {expected} bytes came: nothing was written"))),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
            Ok(n)
        }
        None => io::copy(&mut input, f),
    }
}

/// Writes `input` beside `base` in an exclusively created `0600` temp file and
/// atomically publishes it once all bytes arrive. `whole`: expected byte count
/// (`--len`): fewer bytes (`UnexpectedEof`) or more than that (`InvalidData`,
/// rejected immediately without writing further bytes to disk) leaves any
/// existing destination untouched and removes the temp file.
fn write_atomic(base: &Path, make_unique: bool, input: impl Read, whole: Option<u64>) -> io::Result<(PathBuf, u64)> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let parent = base.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("/"));
    fs::create_dir_all(parent)?;
    let existing_mode = if make_unique { None } else { fs::symlink_metadata(base).ok().filter(|m| m.is_file()).map(|m| m.permissions().mode() & 0o777) };
    let final_mode = existing_mode.unwrap_or_else(default_file_mode);

    // Exclusively create a private 0600 temp file, retrying on name collision.
    // Cleanup is armed ONLY AFTER open() succeeds so we can never delete a file
    // we did not create.
    let (tmp, mut f) = {
        let mut opened = None;
        for _ in 0..1_000 {
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let cand = parent.join(format!(".olo-fs-part-{:08x}{:08x}", std::process::id(), seq));
            match fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&cand) {
                Ok(file) => {
                    opened = Some((cand, file));
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        opened.ok_or_else(|| io::Error::new(io::ErrorKind::AlreadyExists, "could not create staging file"))?
    };

    let res = (|| -> io::Result<(PathBuf, u64)> {
        let n = copy_bounded(input, &mut f, whole)?;
        if final_mode != 0o600 {
            f.set_permissions(fs::Permissions::from_mode(final_mode))?;
        }
        f.sync_all()?;
        drop(f);
        if make_unique {
            let published = rename_unique(&tmp, base)?;
            Ok((published, n))
        } else {
            fs::rename(&tmp, base)?;
            Ok((base.to_path_buf(), n))
        }
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct IdentityExpect {
    dev: Option<u64>,
    ino: Option<u64>,
    size: Option<u64>,
    mtime_secs: Option<i64>,
    mtime_nanos: Option<u32>,
    ctime_secs: Option<i64>,
    ctime_nanos: Option<u32>,
}

impl IdentityExpect {
    fn any(&self) -> bool {
        self.dev.is_some() || self.ino.is_some() || self.size.is_some() || self.mtime_secs.is_some() || self.mtime_nanos.is_some() || self.ctime_secs.is_some() || self.ctime_nanos.is_some()
    }
}

fn parse_flag_val<T: std::str::FromStr>(flag: &str, it: &mut impl Iterator<Item = String>) -> Result<T, String> {
    let raw = it.next().ok_or_else(|| format!("{flag} requires a value"))?;
    raw.parse::<T>().map_err(|_| format!("invalid value for {flag}: {raw:?}"))
}

fn parse_remove_opts(mut it: impl Iterator<Item = String>) -> Result<(IdentityExpect, bool), String> {
    let mut exp = IdentityExpect::default();
    let mut stage = false;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--stage" => stage = true,
            "--expect-dev" => exp.dev = Some(parse_flag_val("--expect-dev", &mut it)?),
            "--expect-ino" => exp.ino = Some(parse_flag_val("--expect-ino", &mut it)?),
            "--expect-size" => exp.size = Some(parse_flag_val("--expect-size", &mut it)?),
            "--expect-mtime-secs" => exp.mtime_secs = Some(parse_flag_val("--expect-mtime-secs", &mut it)?),
            "--expect-mtime-nanos" => exp.mtime_nanos = Some(parse_flag_val("--expect-mtime-nanos", &mut it)?),
            "--expect-ctime-secs" => exp.ctime_secs = Some(parse_flag_val("--expect-ctime-secs", &mut it)?),
            "--expect-ctime-nanos" => exp.ctime_nanos = Some(parse_flag_val("--expect-ctime-nanos", &mut it)?),
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok((exp, stage))
}

fn parse_read_opts(mut it: impl Iterator<Item = String>) -> Result<(Option<u64>, IdentityExpect), String> {
    let mut max_bytes: Option<u64> = None;
    let mut exp = IdentityExpect::default();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--max" => max_bytes = Some(parse_flag_val("--max", &mut it)?),
            "--expect-dev" => exp.dev = Some(parse_flag_val("--expect-dev", &mut it)?),
            "--expect-ino" => exp.ino = Some(parse_flag_val("--expect-ino", &mut it)?),
            "--expect-size" => exp.size = Some(parse_flag_val("--expect-size", &mut it)?),
            "--expect-mtime-secs" => exp.mtime_secs = Some(parse_flag_val("--expect-mtime-secs", &mut it)?),
            "--expect-mtime-nanos" => exp.mtime_nanos = Some(parse_flag_val("--expect-mtime-nanos", &mut it)?),
            "--expect-ctime-secs" => exp.ctime_secs = Some(parse_flag_val("--expect-ctime-secs", &mut it)?),
            "--expect-ctime-nanos" => exp.ctime_nanos = Some(parse_flag_val("--expect-ctime-nanos", &mut it)?),
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok((max_bytes, exp))
}

fn do_rename(from: &Path, base_to: &Path, make_unique: bool, apply_default_mode: bool, explicit_mode: Option<u32>) -> io::Result<PathBuf> {
    let from_meta = fs::symlink_metadata(from)?;
    if let Some(m) = explicit_mode {
        if !from_meta.file_type().is_symlink() {
            fs::set_permissions(from, fs::Permissions::from_mode(m))?;
        }
    } else if apply_default_mode && from_meta.file_type().is_file() {
        fs::set_permissions(from, fs::Permissions::from_mode(default_file_mode()))?;
    }
    if from == base_to {
        // `from_meta` already verified existence (including dangling symlinks).
        Ok(base_to.to_path_buf())
    } else if make_unique {
        rename_unique(from, base_to)
    } else {
        match rename_noreplace(from, base_to) {
            Ok(()) => Ok(base_to.to_path_buf()),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(io::Error::new(io::ErrorKind::AlreadyExists, "a file with that name already exists")),
            Err(e) => Err(e),
        }
    }
}

fn do_remove(target: &Path, exp: &IdentityExpect, stage: bool) -> Result<(), String> {
    let home_norm = home_dir().ok().map(|h| normalize_path(&h));
    if target == Path::new("/") || home_norm.as_deref() == Some(target) {
        return Err("refusing to delete the root or home directory".into());
    }
    let meta = fs::symlink_metadata(target).map_err(|e| e.to_string())?;
    if let Some(d) = exp.dev
        && meta.dev() != d
    {
        return Err("source changed before removal (device mismatch)".into());
    }
    if let Some(i) = exp.ino
        && meta.ino() != i
    {
        return Err("source changed before removal (inode mismatch)".into());
    }
    if let Some(s) = exp.size
        && !meta.is_dir()
        && meta.size() != s
    {
        return Err("source changed before removal (size mismatch)".into());
    }
    if let Some(ms) = exp.mtime_secs
        && meta.mtime() != ms
    {
        return Err("source changed before removal (mtime mismatch)".into());
    }
    if let Some(mn) = exp.mtime_nanos
        && meta.mtime_nsec().clamp(0, 999_999_999) as u32 != mn
    {
        return Err("source changed before removal (mtime mismatch)".into());
    }
    if let Some(cs) = exp.ctime_secs
        && meta.ctime() != cs
    {
        return Err("source changed before removal (ctime mismatch)".into());
    }
    if let Some(cn) = exp.ctime_nanos
        && meta.ctime_nsec().clamp(0, 999_999_999) as u32 != cn
    {
        return Err("source changed before removal (ctime mismatch)".into());
    }
    if meta.is_dir() {
        if exp.any() {
            // Verified removal must NEVER recursively `remove_dir_all` a directory
            // from root-only metadata: only remove an already-empty directory.
            fs::remove_dir(target).map_err(|e| {
                if e.raw_os_error() == Some(libc::ENOTEMPTY) || e.kind() == io::ErrorKind::DirectoryNotEmpty {
                    "verified remove refuses recursive deletion of non-empty directory".to_string()
                } else {
                    e.to_string()
                }
            })
        } else if stage {
            if !is_owned_stage_name(target) {
                return Err("refusing --stage forced removal on non-stage path".into());
            }
            remove_stage_dir_fd(target).map_err(|e| e.to_string())
        } else {
            fs::remove_dir_all(target).map_err(|e| e.to_string())
        }
    } else {
        fs::remove_file(target).map_err(|e| e.to_string())
    }
}

fn open_verified_read(path: &Path, exp: &IdentityExpect) -> Result<fs::File, String> {
    // Open with O_NONBLOCK first so a path replaced by a FIFO never blocks open().
    let f = fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path).map_err(|e| e.to_string())?;
    // Bind the read strictly to the opened file descriptor's inode/stat.
    let fmeta = f.metadata().map_err(|e| e.to_string())?;
    if !fmeta.file_type().is_file() {
        return Err("not a regular file".into());
    }
    let mismatched = exp.dev.is_some_and(|v| fmeta.dev() != v)
        || exp.ino.is_some_and(|v| fmeta.ino() != v)
        || exp.size.is_some_and(|v| fmeta.size() != v)
        || exp.mtime_secs.is_some_and(|v| fmeta.mtime() != v)
        || exp.mtime_nanos.is_some_and(|v| fmeta.mtime_nsec().clamp(0, 999_999_999) as u32 != v)
        || exp.ctime_secs.is_some_and(|v| fmeta.ctime() != v)
        || exp.ctime_nanos.is_some_and(|v| fmeta.ctime_nsec().clamp(0, 999_999_999) as u32 != v);
    if mismatched {
        return Err("file changed before read".into());
    }
    // Clear O_NONBLOCK now that we know `f` is a regular file.
    let flags = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_GETFL) };
    if flags >= 0 {
        let _ = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK) };
    }
    Ok(f)
}

fn print_json(v: serde_json::Value) -> ! {
    let _ = writeln!(io::stdout().lock(), "{v}");
    std::process::exit(0);
}

fn fail_json(code: i32, msg: impl std::fmt::Display) -> ! {
    let _ = writeln!(io::stdout().lock(), "{}", serde_json::json!({ "ok": false, "error": msg.to_string() }));
    std::process::exit(code);
}

pub fn run(args: Vec<String>) -> ! {
    let mut it = args.into_iter();
    let Some(op) = it.next() else {
        eprintln!("olo-rex-link fs: missing operation\n{USAGE}");
        std::process::exit(2);
    };
    match op.as_str() {
        "list" => {
            let Some(raw) = it.next() else { fail_json(2, "list requires PATH") };
            let mut hidden = false;
            for a in it {
                match a.as_str() {
                    "--hidden" => hidden = true,
                    other => fail_json(2, format!("unknown option {other}")),
                }
            }
            let dir = match try_resolve_path(&raw) {
                Ok(p) => p,
                Err(e) => fail_json(1, e),
            };
            let rd = match fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(e) => fail_json(1, e),
            };
            let mut entries = Vec::new();
            for item in rd.flatten() {
                let name = item.file_name().to_string_lossy().into_owned();
                if !hidden && name.starts_with('.') {
                    continue;
                }
                if entries.len() >= MAX_LIST_ENTRIES {
                    fail_json(1, format!("folder has more than {MAX_LIST_ENTRIES} entries"));
                }
                if let Ok(en) = stat_entry(&item.path()) {
                    entries.push(en);
                }
            }
            let (free_bytes, total_bytes) = fs_space(&dir).map(|(f, t)| (Some(f), Some(t))).unwrap_or((None, None));
            print_json(serde_json::json!({
                "ok": true,
                "resolved": dir.to_string_lossy(),
                "free_bytes": free_bytes,
                "total_bytes": total_bytes,
                "entries": entries,
            }));
        }
        "stat" => {
            let Some(raw) = it.next() else { fail_json(2, "stat requires PATH") };
            let path = match try_resolve_path(&raw) {
                Ok(p) => p,
                Err(e) => fail_json(1, e),
            };
            match stat_entry(&path) {
                Ok(entry) => print_json(serde_json::json!({
                    "ok": true,
                    "resolved": path.to_string_lossy(),
                    "exists": true,
                    "entry": entry,
                })),
                Err(e) if e.kind() == io::ErrorKind::NotFound => print_json(serde_json::json!({
                    "ok": true,
                    "resolved": path.to_string_lossy(),
                    "exists": false,
                    "entry": serde_json::Value::Null,
                })),
                Err(e) => fail_json(1, e),
            }
        }
        "mkdir" => {
            let Some(raw) = it.next() else { fail_json(2, "mkdir requires PATH") };
            let mut make_unique = false;
            let mut private = false;
            for a in it {
                match a.as_str() {
                    "--unique" => make_unique = true,
                    "--private" => private = true,
                    other => fail_json(2, format!("unknown option {other}")),
                }
            }
            let base = match try_resolve_path(&raw) {
                Ok(p) => p,
                Err(e) => fail_json(1, e),
            };
            let target = if make_unique {
                match mkdir_unique(&base, private) {
                    Ok(p) => p,
                    Err(e) => fail_json(1, e),
                }
            } else {
                if let Err(e) = mkdir_one(&base, private) {
                    fail_json(1, e);
                }
                base
            };
            let entry = stat_entry(&target).ok();
            print_json(serde_json::json!({
                "ok": true,
                "path": target.to_string_lossy(),
                "entry": entry,
            }));
        }
        "rename" => {
            let (Some(raw_from), Some(raw_to)) = (it.next(), it.next()) else { fail_json(2, "rename requires FROM TO") };
            let mut make_unique = false;
            let mut apply_default_mode = false;
            let mut explicit_mode: Option<u32> = None;
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--unique" => make_unique = true,
                    "--default-mode" => apply_default_mode = true,
                    "--mode" => match it.next().and_then(|s| u32::from_str_radix(s.trim_start_matches("0o"), 8).ok()) {
                        Some(m) => explicit_mode = Some(m & 0o3777),
                        None => fail_json(2, "--mode requires an octal permission mask"),
                    },
                    other => fail_json(2, format!("unknown option {other}")),
                }
            }
            let from = match try_resolve_path(&raw_from) {
                Ok(p) => p,
                Err(e) => fail_json(1, e),
            };
            let base_to = match try_resolve_path(&raw_to) {
                Ok(p) => p,
                Err(e) => fail_json(1, e),
            };
            let to = match do_rename(&from, &base_to, make_unique, apply_default_mode, explicit_mode) {
                Ok(p) => p,
                Err(e) => fail_json(1, e),
            };
            print_json(serde_json::json!({ "ok": true, "path": to.to_string_lossy() }));
        }
        "copy" => {
            let (Some(raw_from), Some(raw_to)) = (it.next(), it.next()) else { fail_json(2, "copy requires FROM TO") };
            let from = match try_resolve_path(&raw_from) {
                Ok(p) => p,
                Err(e) => fail_json(1, e),
            };
            let base_to = match try_resolve_path(&raw_to) {
                Ok(p) => p,
                Err(e) => fail_json(1, e),
            };
            let to = match copy_staged(&from, &base_to) {
                Ok(p) => p,
                Err(e) => fail_json(1, e),
            };
            print_json(serde_json::json!({ "ok": true, "path": to.to_string_lossy() }));
        }
        "remove" => {
            let Some(raw) = it.next() else { fail_json(2, "remove requires PATH") };
            let (exp, stage) = match parse_remove_opts(it) {
                Ok(v) => v,
                Err(e) => fail_json(2, e),
            };
            let target = match try_resolve_path(&raw) {
                Ok(p) => p,
                Err(e) => fail_json(1, e),
            };
            if let Err(e) = do_remove(&target, &exp, stage) {
                fail_json(1, e);
            }
            print_json(serde_json::json!({ "ok": true }));
        }
        "read" => {
            let Some(raw) = it.next() else {
                eprintln!("olo-rex-link fs read: missing PATH");
                std::process::exit(2);
            };
            let (max_bytes, exp) = match parse_read_opts(it) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("olo-rex-link fs read: {e}");
                    std::process::exit(2);
                }
            };
            let path = match try_resolve_path(&raw) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("olo-rex-link fs read: {e}");
                    std::process::exit(1);
                }
            };
            let f = match open_verified_read(&path, &exp) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("olo-rex-link fs read: {e}");
                    std::process::exit(1);
                }
            };
            let mut out = io::BufWriter::new(io::stdout().lock());
            let res = match max_bytes {
                Some(cap) => io::copy(&mut f.take(cap), &mut out),
                None => io::copy(&mut io::BufReader::new(f), &mut out),
            };
            if let Err(e) = res.and_then(|_| out.flush()) {
                eprintln!("olo-rex-link fs read: {e}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }
        "write" => {
            let Some(raw) = it.next() else { fail_json(2, "write requires PATH") };
            let mut make_unique = false;
            let mut whole = None;
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--unique" => make_unique = true,
                    "--len" => match it.next().and_then(|n| n.parse::<u64>().ok()) {
                        Some(n) => whole = Some(n),
                        None => fail_json(2, "--len requires a number of bytes"),
                    },
                    other => fail_json(2, format!("unknown option {other}")),
                }
            }
            let base = match try_resolve_path(&raw) {
                Ok(p) => p,
                Err(e) => fail_json(1, e),
            };
            match write_atomic(&base, make_unique, io::stdin().lock(), whole) {
                Ok((target, bytes)) => print_json(serde_json::json!({ "ok": true, "path": target.to_string_lossy(), "bytes": bytes })),
                Err(e) => fail_json(1, e),
            }
        }
        other => {
            eprintln!("olo-rex-link fs: unknown operation {other}\n{USAGE}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_and_copy_and_write_atomic_work() {
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-link-fs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let f = dir.join("hello.txt");
        let (path, n) = write_atomic(&f, false, &b"world"[..], None).unwrap();
        assert_eq!(path, f);
        assert_eq!(n, 5);
        assert_eq!(fs::read(&f).unwrap(), b"world");

        // Short streams fail with UnexpectedEof; excess streams fail immediately
        // with InvalidData without touching the destination or leaving a temp file.
        for (came, said) in [(&b"wor"[..], 5), (&b""[..], 5)] {
            let cut = write_atomic(&f, false, came, Some(said)).unwrap_err();
            assert_eq!(cut.kind(), io::ErrorKind::UnexpectedEof, "{cut}");
            assert_eq!(fs::read(&f).unwrap(), b"world");
            assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "a part was left behind");
        }
        let excess = write_atomic(&f, false, &b"worlds"[..], Some(5)).unwrap_err();
        assert_eq!(excess.kind(), io::ErrorKind::InvalidData, "{excess}");
        assert_eq!(fs::read(&f).unwrap(), b"world");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "a part was left behind");
        assert_eq!(write_atomic(&f, false, &b"there"[..], Some(5)).unwrap().1, 5);
        assert_eq!(fs::read(&f).unwrap(), b"there");
        assert_eq!(write_atomic(&f, false, &b"world"[..], Some(5)).unwrap().1, 5);

        // Unique write picks "hello 2.txt" atomically without clobbering "hello.txt".
        let (u1, _) = write_atomic(&f, true, &b"second"[..], Some(6)).unwrap();
        assert_eq!(u1.file_name().unwrap(), "hello 2.txt");
        assert_eq!(fs::read(&f).unwrap(), b"world");
        assert_eq!(fs::read(&u1).unwrap(), b"second");

        // Unique rename also avoids a pre-existing broken symlink!
        let broken = dir.join("hello 3.txt");
        symlink(dir.join("missing-target"), &broken).unwrap();
        let src_ren = dir.join("stage.bin");
        fs::write(&src_ren, b"renamed").unwrap();
        let pub_ren = rename_unique(&src_ren, &f).unwrap();
        assert_eq!(pub_ren.file_name().unwrap(), "hello 4.txt", "must skip broken symlink at hello 3.txt");
        assert!(fs::symlink_metadata(&broken).unwrap().file_type().is_symlink(), "broken symlink must remain intact");
        assert_eq!(fs::read(&pub_ren).unwrap(), b"renamed");

        // Non-unique rename_noreplace refuses to clobber a broken symlink.
        let src2 = dir.join("stage2.bin");
        fs::write(&src2, b"no-clobber").unwrap();
        let err = rename_noreplace(&src2, &broken).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(src2.exists());

        // Private mkdir creates 0700 directory.
        let priv_dir = mkdir_unique(&dir.join("priv"), true).unwrap();
        assert_eq!(fs::metadata(&priv_dir).unwrap().permissions().mode() & 0o777, 0o700);

        let copy_dst = unique_path(&f);
        assert_eq!(copy_dst.file_name().unwrap(), "hello 5.txt");
        copy_recursive(&f, &copy_dst).unwrap();
        assert_eq!(fs::read(&copy_dst).unwrap(), b"world");

        let entry = stat_entry(&f).unwrap();
        assert_eq!(entry.name, "hello.txt");
        assert!(!entry.is_dir && entry.size == 5);

        assert_eq!(normalize_path(Path::new("/a/b/../c/./d")), PathBuf::from("/a/c/d"));
        assert_eq!(resolve_path("/var/tmp/olo-qa-rex-tr"), PathBuf::from("/var/tmp/olo-qa-rex-tr"));
        assert_eq!(resolve_path("/etc/hosts"), PathBuf::from("/etc/hosts"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_unique_renames_never_overwrite_each_other() {
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-link-race-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let base = dir.join("Pasted Picture.png");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let mut handles = Vec::new();
        for i in 0..4usize {
            let src = dir.join(format!("stage-{i}.png"));
            fs::write(&src, format!("payload-{i}")).unwrap();
            let b = barrier.clone();
            let base = base.clone();
            handles.push(std::thread::spawn(move || {
                b.wait();
                (i, rename_unique(&src, &base).unwrap())
            }));
        }
        let mut results: Vec<(usize, PathBuf)> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let unique_paths: std::collections::HashSet<PathBuf> = results.iter().map(|(_, p)| p.clone()).collect();
        assert_eq!(unique_paths.len(), 4, "all 4 concurrent publishers must land on distinct paths: {results:?}");
        results.sort_by_key(|(i, _)| *i);
        for (i, p) in &results {
            assert_eq!(fs::read_to_string(p).unwrap(), format!("payload-{i}"));
        }

        // Unsupported RENAME_NOREPLACE branch: regular files use atomic hard_link
        // (refusing collision), fail closed when hard links are also unsupported,
        // keep dst intact if src unlink fails, and directories fail closed.
        let s_file = dir.join("fb-src.txt");
        let d_file = dir.join("fb-dst.txt");
        fs::write(&s_file, b"new").unwrap();
        fs::write(&d_file, b"existing").unwrap();
        let err = rename_noreplace_inner(&s_file, &d_file, true, false).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&d_file).unwrap(), b"existing");
        assert!(s_file.exists());

        // When neither RENAME_NOREPLACE nor hard links work, fail closed with Unsupported
        // and never copy directly into the final destination path.
        let d_unsup = dir.join("fb-unsup.txt");
        let err = rename_noreplace_inner(&s_file, &d_unsup, true, true).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported, "{err}");
        assert!(!d_unsup.exists(), "must never copy into final dst on unsupported fallback");

        let d_free = dir.join("fb-free.txt");
        rename_noreplace_inner(&s_file, &d_free, true, false).unwrap();
        assert_eq!(fs::read(&d_free).unwrap(), b"new");
        assert!(!s_file.exists());

        // If hard_link succeeds and removing src fails (read-only src parent dir),
        // dst must remain intact instead of being unlinked.
        let ro_parent = dir.join("ro-parent");
        fs::create_dir(&ro_parent).unwrap();
        let ro_src = ro_parent.join("keep-dst.txt");
        fs::write(&ro_src, b"linked-payload").unwrap();
        fs::set_permissions(&ro_parent, fs::Permissions::from_mode(0o500)).unwrap();
        let kept_dst = dir.join("kept-dst.txt");
        let link_err = rename_noreplace_inner(&ro_src, &kept_dst, true, false).unwrap_err();
        assert_eq!(link_err.kind(), io::ErrorKind::PermissionDenied, "{link_err}");
        assert_eq!(fs::read(&kept_dst).unwrap(), b"linked-payload", "published dst must stay intact when src unlink fails");
        fs::set_permissions(&ro_parent, fs::Permissions::from_mode(0o700)).unwrap();

        let s_dir = dir.join("fb-src-dir");
        let d_dir = dir.join("fb-dst-dir");
        fs::create_dir(&s_dir).unwrap();
        let err = rename_noreplace_inner(&s_dir, &d_dir, true, false).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported, "dir without RENAME_NOREPLACE must fail closed");

        // Staged copy publishes atomically and cleans up on error without leaving a partial dst.
        let copy_src = dir.join("copy-src");
        fs::create_dir_all(copy_src.join("sub")).unwrap();
        fs::write(copy_src.join("sub/a.txt"), b"copied").unwrap();
        let copy_out = copy_staged(&copy_src, &dir.join("copy-out")).unwrap();
        assert_eq!(fs::read(copy_out.join("sub/a.txt")).unwrap(), b"copied");

        // Moving a directory into itself returns InvalidInput, not Unsupported.
        let self_dir = dir.join("self-move-dir");
        fs::create_dir(&self_dir).unwrap();
        let self_err = rename_noreplace(&self_dir, &self_dir.join("sub")).unwrap_err();
        assert_eq!(self_err.kind(), io::ErrorKind::InvalidInput, "moving dir into itself must return InvalidInput: {self_err}");

        // Overlong write against --len reads at most expected + 1 bytes, rejects
        // with InvalidData, and never leaves a temp file behind.
        let huge_reader = io::repeat(b'x');
        let err = write_atomic(&dir.join("bounded.bin"), false, huge_reader, Some(16)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{err}");
        assert!(!dir.join("bounded.bin").exists());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn verified_remove_symlinks_dirs_flags_self_rename_fifos_and_stage_cleanup() {
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-link-r2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 1. Malformed --expect-* flags are rejected (never silently turned into None).
        assert!(parse_remove_opts(["--expect-ino".into(), "not-a-number".into()].into_iter()).is_err());
        assert!(parse_remove_opts(["--expect-dev".into()].into_iter()).is_err());
        assert!(parse_read_opts(["--max".into(), "abc".into()].into_iter()).is_err());
        assert!(parse_read_opts(["--expect-size".into(), "-1".into()].into_iter()).is_err());

        // 2. Valid symlink & dangling symlink expose lstat_* fields and succeed under
        //    verified remove using lstat_* (without deleting the symlink target).
        let target_file = dir.join("target.txt");
        fs::write(&target_file, b"keep-me-safe").unwrap();
        let valid_link = dir.join("valid-link");
        symlink(&target_file, &valid_link).unwrap();
        let st_valid = stat_entry(&valid_link).unwrap();
        assert!(st_valid.is_link);
        assert_ne!(st_valid.lstat_ino, st_valid.ino, "lstat_ino must identify the symlink itself, not the target");
        let exp_valid = IdentityExpect {
            dev: Some(st_valid.lstat_dev),
            ino: Some(st_valid.lstat_ino),
            size: Some(st_valid.lstat_size),
            mtime_secs: Some(st_valid.lstat_mtime_secs),
            mtime_nanos: Some(st_valid.lstat_mtime_nanos),
            ctime_secs: Some(st_valid.lstat_ctime_secs),
            ctime_nanos: Some(st_valid.lstat_ctime_nanos),
        };
        do_remove(&valid_link, &exp_valid, false).unwrap();
        assert!(fs::symlink_metadata(&valid_link).is_err(), "valid symlink should be unlinked");
        assert_eq!(fs::read(&target_file).unwrap(), b"keep-me-safe", "symlink target must remain untouched");

        let dangling = dir.join("dangling-link");
        symlink(dir.join("nowhere"), &dangling).unwrap();
        let st_dang = stat_entry(&dangling).unwrap();
        assert!(st_dang.is_link);
        let exp_dang = IdentityExpect {
            dev: Some(st_dang.lstat_dev),
            ino: Some(st_dang.lstat_ino),
            size: Some(st_dang.lstat_size),
            mtime_secs: Some(st_dang.lstat_mtime_secs),
            mtime_nanos: Some(st_dang.lstat_mtime_nanos),
            ctime_secs: Some(st_dang.lstat_ctime_secs),
            ctime_nanos: Some(st_dang.lstat_ctime_nanos),
        };
        // Wrong inode refuses deletion.
        let mut wrong = exp_dang.clone();
        wrong.ino = Some(st_dang.lstat_ino.wrapping_add(1));
        assert!(do_remove(&dangling, &wrong, false).is_err());
        assert!(fs::symlink_metadata(&dangling).is_ok());
        // Matching lstat_* removes dangling symlink.
        do_remove(&dangling, &exp_dang, false).unwrap();
        assert!(fs::symlink_metadata(&dangling).is_err());

        // 3. Verified remove refuses recursive deletion of a non-empty directory.
        let non_empty = dir.join("non-empty");
        fs::create_dir(&non_empty).unwrap();
        fs::write(non_empty.join("precious.txt"), b"do-not-delete").unwrap();
        let st_dir = stat_entry(&non_empty).unwrap();
        let exp_dir = IdentityExpect {
            dev: Some(st_dir.lstat_dev),
            ino: Some(st_dir.lstat_ino),
            size: None,
            mtime_secs: Some(st_dir.lstat_mtime_secs),
            mtime_nanos: Some(st_dir.lstat_mtime_nanos),
            ctime_secs: Some(st_dir.lstat_ctime_secs),
            ctime_nanos: Some(st_dir.lstat_ctime_nanos),
        };
        let err = do_remove(&non_empty, &exp_dir, false).unwrap_err();
        assert!(err.contains("non-empty"), "{err}");
        assert_eq!(fs::read(non_empty.join("precious.txt")).unwrap(), b"do-not-delete");

        // 4. `fs rename A A` fails ENOENT on missing path and succeeds on dangling symlink.
        let missing = dir.join("does-not-exist");
        let err = do_rename(&missing, &missing, false, false, None).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        let self_link = dir.join("self-dangling");
        symlink(dir.join("missing-target"), &self_link).unwrap();
        assert_eq!(do_rename(&self_link, &self_link, false, false, None).unwrap(), self_link);

        // 5. FIFOs are rejected immediately without blocking in both copy_staged and open_verified_read.
        let fifo = dir.join("test.fifo");
        let c_fifo = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_fifo.as_ptr(), 0o600) }, 0);
        let copy_err = copy_staged(&fifo, &dir.join("fifo-copy")).unwrap_err();
        assert_eq!(copy_err.kind(), io::ErrorKind::InvalidInput, "{copy_err}");
        let read_err = open_verified_read(&fifo, &IdentityExpect::default()).unwrap_err();
        assert!(read_err.contains("not a regular file"), "{read_err}");

        // 6. Forced stage cleanup repairs 0555 subdirs via dir-fd fchmod without touching user trees.
        let stage = dir.join(".olo-tx-stage-test");
        let ro_sub = stage.join("ro-sub");
        fs::create_dir_all(&ro_sub).unwrap();
        fs::write(ro_sub.join("f.txt"), b"inside").unwrap();
        fs::set_permissions(&ro_sub, fs::Permissions::from_mode(0o555)).unwrap();
        do_remove(&stage, &IdentityExpect::default(), true).unwrap();
        assert!(!stage.exists());
        // `--stage` refuses non-stage names.
        let user_dir = dir.join("user-folder");
        fs::create_dir(&user_dir).unwrap();
        assert!(do_remove(&user_dir, &IdentityExpect::default(), true).is_err());
        assert!(user_dir.exists());

        fs::remove_dir_all(dir).unwrap();
    }
}
