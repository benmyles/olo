//! `olo-rex-link fs <op> [ARGS…]`: native Rust filesystem helper on a Rex host
//! used by the Olo desktop's Tracker integration (`rex://HOST/PATH`) through
//! Rex's SSH connection pool. Never shells out to system utilities.
//!
//! Subcommands:
//!   list PATH [--hidden]     list directory entries + filesystem free space (JSON)
//!   stat PATH                stat a single path, resolving `~` / relative paths (JSON)
//!   mkdir PATH [--unique]    create directory (`--unique`: append ` 2`, ` 3`… if taken)
//!   rename FROM TO           rename or move within this host
//!   copy FROM TO             recursively copy file, symlink, or directory within this host
//!   remove PATH              permanently remove file, symlink, or directory (refuses `/` and `$HOME`)
//!   read PATH [--max N]      stream file bytes directly to stdout (optional byte cap)
//!   write PATH [--unique] [--len N]  atomically write stdin bytes to PATH

use std::ffi::CString;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

const USAGE: &str = "\
usage: olo-rex-link fs list PATH [--hidden]
       olo-rex-link fs stat PATH
       olo-rex-link fs mkdir PATH [--unique]
       olo-rex-link fs rename FROM TO
       olo-rex-link fs copy FROM TO
       olo-rex-link fs remove PATH
       olo-rex-link fs read PATH [--max N]
       olo-rex-link fs write PATH [--unique] [--len N]";

#[derive(Debug, Clone, Serialize)]
pub struct FsEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    pub mtime_secs: i64,
    pub mtime_nanos: u32,
    pub mode: u32,
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// Resolve `~`, `~/…`, or relative paths against `$HOME`, normalizing `.` / `..`.
pub fn resolve_path(raw: &str) -> PathBuf {
    let expanded = if raw == "~" {
        home_dir()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home_dir().join(rest)
    } else {
        let p = PathBuf::from(raw);
        if p.is_relative() { home_dir().join(p) } else { p }
    };
    normalize_path(&expanded)
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
        mode: meta.permissions().mode(),
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

const UNIQUE_MAX_ATTEMPTS: u32 = 10_000;

fn unique_candidates(base: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    let parent = base.parent().unwrap_or(Path::new("/"));
    let name = base.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "New Folder".into());
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !e.is_empty() => (s.to_owned(), format!(".{e}")),
        _ => (name, String::new()),
    };
    std::iter::once(base.to_path_buf()).chain((2..=UNIQUE_MAX_ATTEMPTS).map(move |n| parent.join(format!("{stem} {n}{ext}"))))
}

#[cfg(test)]
fn unique_path(base: &Path) -> io::Result<PathBuf> {
    for cand in unique_candidates(base) {
        match fs::symlink_metadata(&cand) {
            Ok(_) => continue,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(cand),
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("no free unique name for {} after {UNIQUE_MAX_ATTEMPTS} attempts", base.display())))
}

/// Calls Linux `renameat2(RENAME_NOREPLACE)` so `src` is atomically moved to
/// `dst` only when `dst` does not exist.
fn renameat2_noreplace(src: &Path, dst: &Path) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let c_src = CString::new(src.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in source path"))?;
        let c_dst = CString::new(dst.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in destination path"))?;
        // SAFETY: both paths are valid NUL-terminated C strings.
        let rc = unsafe { libc::syscall(libc::SYS_renameat2, libc::AT_FDCWD, c_src.as_ptr(), libc::AT_FDCWD, c_dst.as_ptr(), libc::RENAME_NOREPLACE) };
        if rc == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EINVAL)
            && let (Ok(cs), Some(dp)) = (fs::canonicalize(src), dst.parent().and_then(|p| fs::canonicalize(p).ok()))
            && dp.starts_with(&cs)
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "cannot move a folder inside itself"));
        }
        if matches!(err.raw_os_error(), Some(libc::ENOSYS | libc::EINVAL | libc::EOPNOTSUPP)) {
            return Err(io::Error::new(io::ErrorKind::Unsupported, err));
        }
        Err(err)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (src, dst);
        Err(io::Error::new(io::ErrorKind::Unsupported, "atomic no-replace rename is unsupported on this platform"))
    }
}

/// Atomically publishes an `fs_op`-owned temp file `tmp` at `dst` ONLY when
/// `dst` does not already exist (`AlreadyExists` on collision). Uses
/// `renameat2(RENAME_NOREPLACE)` on Linux, falling back to `hard_link` +
/// unlink ONLY for these owned regular temp files if the filesystem returns
/// `Unsupported`.
fn publish_temp_noreplace(tmp: &Path, dst: &Path) -> io::Result<()> {
    match renameat2_noreplace(tmp, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::Unsupported => {
            fs::hard_link(tmp, dst)?;
            let _ = fs::remove_file(tmp);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Atomically moves a user entry (file, symlink, or directory) to `dst` ONLY
/// when `dst` does not exist. Never falls back to `hard_link` + `remove_file`
/// (which fails on directories with `EPERM` and risks unlinking a concurrently
/// replaced source file).
fn move_user_noreplace(from: &Path, dst: &Path) -> io::Result<()> {
    renameat2_noreplace(from, dst)
        .map_err(|e| if e.kind() == io::ErrorKind::Unsupported { io::Error::new(io::ErrorKind::Unsupported, "atomic no-replace rename is unsupported on this filesystem") } else { e })
}

/// Reads the current process umask on Linux without mutating global state
/// (defaults to `0o022` if unavailable).
fn process_umask() -> u32 {
    fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find_map(|l| l.strip_prefix("Umask:")).and_then(|v| u32::from_str_radix(v.trim(), 8).ok())).unwrap_or(0o022)
}

/// RAII cleanup guard armed ONLY after `create_new(true)` actually created the
/// temp file (never unlinks a pre-existing colliding file on `AlreadyExists`).
struct TempFileGuard(Option<PathBuf>);

impl TempFileGuard {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = fs::remove_file(p);
        }
    }
}

fn create_temp_beside(parent: &Path, seq: &AtomicU64) -> io::Result<(PathBuf, fs::File, TempFileGuard)> {
    for _ in 0..1_000 {
        let n = seq.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!(".olo-fs-part-{:08x}{:08x}", std::process::id(), n));
        // Always stage privately with mode 0600 so replacing a private file
        // never exposes its contents under the default umask before rename.
        match fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp) {
            Ok(f) => {
                let guard = TempFileGuard(Some(tmp.clone()));
                return Ok((tmp, f, guard));
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "failed to allocate unique temp file after 1000 attempts"))
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

fn copy_dir_contents(src: &Path, dst: &Path, src_lmeta: &fs::Metadata) -> io::Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        copy_recursive(&entry.path(), &dst.join(entry.file_name()))?;
    }
    fs::set_permissions(dst, fs::Permissions::from_mode(src_lmeta.permissions().mode() & 0o777))?;
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
        // Stage directory privately at 0o700 so contents of a private source
        // directory are never exposed under the default umask while copying,
        // and propagate any final `set_permissions` error.
        fs::DirBuilder::new().mode(0o700).create(dst)?;
        return copy_dir_contents(src, dst, &lmeta);
    }
    let mut reader = fs::File::open(src)?;
    let mut writer = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(dst)?;
    io::copy(&mut reader, &mut writer)?;
    writer.set_permissions(fs::Permissions::from_mode(lmeta.permissions().mode() & 0o777))?;
    drop(writer);
    Ok(())
}

/// RAII cleanup guard for an `fs_op`-owned private staging directory.
/// Restores owner `0o700` on the top staging directory before removing it so
/// read-only source modes (`0o500`) never prevent error cleanup.
struct TempDirGuard(Option<PathBuf>);

impl TempDirGuard {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o700));
            let _ = fs::remove_dir_all(p);
        }
    }
}

fn create_staging_dir_beside(parent: &Path, seq: &AtomicU64) -> io::Result<(PathBuf, TempDirGuard)> {
    for _ in 0..1_000 {
        let n = seq.fetch_add(1, Ordering::Relaxed);
        let stage = parent.join(format!(".olo-fs-stage-{:08x}{:08x}", std::process::id(), n));
        match fs::DirBuilder::new().mode(0o700).create(&stage) {
            Ok(()) => {
                let guard = TempDirGuard(Some(stage.clone()));
                return Ok((stage, guard));
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "failed to allocate unique staging dir after 1000 attempts"))
}

/// Atomically copies `src` into the first free unique name (`base_to`,
/// `stem 2.ext`, …). For directories, populates a private `0o700` staging
/// directory on the destination filesystem and publishes it via atomic
/// no-replace rename (`renameat2_noreplace`), retrying `AlreadyExists` ONLY on
/// the final top-level publication and cleaning up the private staging tree on
/// any failure so no visible partial destination is ever left behind.
fn copy_unique(src: &Path, base_to: &Path) -> io::Result<PathBuf> {
    static DIR_STAGE_SEQ: AtomicU64 = AtomicU64::new(0);
    let lmeta = fs::symlink_metadata(src)?;
    if lmeta.is_dir() {
        let parent = base_to.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("/"));
        if let (Ok(cs), Ok(dp)) = (fs::canonicalize(src), fs::canonicalize(parent))
            && dp.starts_with(&cs)
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "cannot copy a folder inside itself"));
        }
        let (stage, mut guard) = create_staging_dir_beside(parent, &DIR_STAGE_SEQ)?;
        copy_dir_contents(src, &stage, &lmeta)?;
        for cand in unique_candidates(base_to) {
            match renameat2_noreplace(&stage, &cand) {
                Ok(()) => {
                    guard.disarm();
                    return Ok(cand);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("no free unique name for {} after {UNIQUE_MAX_ATTEMPTS} attempts", base_to.display())));
    }
    for cand in unique_candidates(base_to) {
        match copy_recursive(src, &cand) {
            Ok(()) => return Ok(cand),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("no free unique name for {} after {UNIQUE_MAX_ATTEMPTS} attempts", base_to.display())))
}

/// Writes `input` beside `dst` and puts it in its place when it is all
/// there. `whole`: how long the file is, where its sender said (`--len`):
/// fewer bytes (`UnexpectedEof`) or more than that (`InvalidData`, rejected
/// immediately without copying further) and nothing is put in place, the file
/// there left as it was.
fn write_atomic(dst: &Path, input: impl Read, whole: Option<u64>) -> io::Result<u64> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let parent = dst.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("/"));
    fs::create_dir_all(parent)?;
    let existing_mode = fs::symlink_metadata(dst).ok().filter(|m| m.is_file()).map(|m| m.permissions().mode() & 0o777);
    let (tmp, mut f, mut guard) = create_temp_beside(parent, &SEQ)?;
    let n = copy_bounded(input, &mut f, whole)?;
    let target_mode = existing_mode.unwrap_or_else(|| 0o666 & !process_umask());
    f.set_permissions(fs::Permissions::from_mode(target_mode))?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, dst)?;
    guard.disarm();
    Ok(n)
}

/// Writes `input` to a verified temp file first, then atomically publishes it
/// at the first free unique name (`base`, `stem 2.ext`, …, `stem 10000.ext`)
/// using [`publish_temp_noreplace`] so two concurrent `--unique` saves can never
/// overwrite each other. Returns the actual published path and byte count, or
/// `AlreadyExists` if all 10,000 candidates are occupied (never overwrites `base`).
fn write_atomic_unique(base: &Path, input: impl Read, whole: Option<u64>) -> io::Result<(PathBuf, u64)> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let parent = base.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("/"));
    fs::create_dir_all(parent)?;
    let (tmp, mut f, mut guard) = create_temp_beside(parent, &SEQ)?;
    let n = copy_bounded(input, &mut f, whole)?;
    let target_mode = 0o666 & !process_umask();
    if target_mode != 0o600 {
        f.set_permissions(fs::Permissions::from_mode(target_mode))?;
    }
    f.sync_all()?;
    drop(f);
    for cand in unique_candidates(base) {
        match publish_temp_noreplace(&tmp, &cand) {
            Ok(()) => {
                guard.disarm();
                return Ok((cand, n));
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("no free unique name for {} after {UNIQUE_MAX_ATTEMPTS} attempts", base.display())))
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
            let dir = resolve_path(&raw);
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
            let path = resolve_path(&raw);
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
            for a in it {
                match a.as_str() {
                    "--unique" => make_unique = true,
                    other => fail_json(2, format!("unknown option {other}")),
                }
            }
            let base = resolve_path(&raw);
            let target = if make_unique {
                let mut created = None;
                for cand in unique_candidates(&base) {
                    match fs::create_dir(&cand) {
                        Ok(()) => {
                            created = Some(cand);
                            break;
                        }
                        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                        Err(e) => fail_json(1, e),
                    }
                }
                match created {
                    Some(t) => t,
                    None => fail_json(1, format!("no free unique name for {} after {UNIQUE_MAX_ATTEMPTS} attempts", base.display())),
                }
            } else {
                if let Err(e) = fs::create_dir(&base) {
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
            for a in it {
                match a.as_str() {
                    "--unique" => make_unique = true,
                    other => fail_json(2, format!("unknown option {other}")),
                }
            }
            let from = resolve_path(&raw_from);
            let base_to = resolve_path(&raw_to);
            let to = if from == base_to {
                if let Err(e) = fs::symlink_metadata(&from) {
                    fail_json(1, e);
                }
                base_to
            } else if make_unique {
                let mut moved = None;
                for cand in unique_candidates(&base_to) {
                    match move_user_noreplace(&from, &cand) {
                        Ok(()) => {
                            moved = Some(cand);
                            break;
                        }
                        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                        Err(e) => fail_json(1, e),
                    }
                }
                match moved {
                    Some(t) => t,
                    None => fail_json(1, format!("no free unique name for {} after {UNIQUE_MAX_ATTEMPTS} attempts", base_to.display())),
                }
            } else {
                match move_user_noreplace(&from, &base_to) {
                    Ok(()) => base_to,
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => fail_json(1, "a file with that name already exists"),
                    Err(e) => fail_json(1, e),
                }
            };
            print_json(serde_json::json!({ "ok": true, "path": to.to_string_lossy() }));
        }
        "copy" => {
            let (Some(raw_from), Some(raw_to)) = (it.next(), it.next()) else { fail_json(2, "copy requires FROM TO") };
            let from = resolve_path(&raw_from);
            let to = match copy_unique(&from, &resolve_path(&raw_to)) {
                Ok(p) => p,
                Err(e) => fail_json(1, e),
            };
            print_json(serde_json::json!({ "ok": true, "path": to.to_string_lossy() }));
        }
        "remove" => {
            let Some(raw) = it.next() else { fail_json(2, "remove requires PATH") };
            let target = resolve_path(&raw);
            if target == Path::new("/") || target == home_dir() {
                fail_json(1, "refusing to delete the root or home directory");
            }
            let meta = match fs::symlink_metadata(&target) {
                Ok(m) => m,
                Err(e) => fail_json(1, e),
            };
            let res = if meta.is_dir() { fs::remove_dir_all(&target) } else { fs::remove_file(&target) };
            if let Err(e) = res {
                fail_json(1, e);
            }
            print_json(serde_json::json!({ "ok": true }));
        }
        "read" => {
            let Some(raw) = it.next() else {
                eprintln!("olo-rex-link fs read: missing PATH");
                std::process::exit(2);
            };
            let mut max_bytes: Option<u64> = None;
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--max" => max_bytes = it.next().and_then(|n| n.parse().ok()),
                    other => {
                        eprintln!("olo-rex-link fs read: unknown option {other}");
                        std::process::exit(2);
                    }
                }
            }
            let path = resolve_path(&raw);
            let f = match fs::File::open(&path) {
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
            let mut it = it;
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
            let base = resolve_path(&raw);
            if make_unique {
                match write_atomic_unique(&base, io::stdin().lock(), whole) {
                    Ok((target, bytes)) => print_json(serde_json::json!({ "ok": true, "path": target.to_string_lossy(), "bytes": bytes })),
                    Err(e) => fail_json(1, e),
                }
            } else {
                match write_atomic(&base, io::stdin().lock(), whole) {
                    Ok(bytes) => print_json(serde_json::json!({ "ok": true, "path": base.to_string_lossy(), "bytes": bytes })),
                    Err(e) => fail_json(1, e),
                }
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
        let n = write_atomic(&f, &b"world"[..], None).unwrap();
        assert_eq!(n, 5);
        assert_eq!(fs::read(&f).unwrap(), b"world");
        // Short streams fail with UnexpectedEof; excess streams fail immediately
        // with InvalidData (even an infinite stream!), leaving the destination
        // untouched and no temporary files behind.
        for (came, said) in [(&b"wor"[..], 5), (&b""[..], 5)] {
            let cut = write_atomic(&f, came, Some(said)).unwrap_err();
            assert_eq!(cut.kind(), io::ErrorKind::UnexpectedEof, "{cut}");
            assert_eq!(fs::read(&f).unwrap(), b"world");
            assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "a part was left behind");
        }
        let excess = write_atomic(&f, &b"worlds"[..], Some(5)).unwrap_err();
        assert_eq!(excess.kind(), io::ErrorKind::InvalidData, "{excess}");
        assert_eq!(fs::read(&f).unwrap(), b"world");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "a part was left behind");

        // An infinite stream with `--len 5` must be rejected immediately after
        // 5 + 1 bytes without copying to disk forever.
        struct CountingRepeat(u64);
        impl Read for CountingRepeat {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                for b in buf.iter_mut() {
                    *b = b'x';
                }
                self.0 += buf.len() as u64;
                Ok(buf.len())
            }
        }
        let mut inf = CountingRepeat(0);
        let err = write_atomic(&f, &mut inf, Some(5)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{err}");
        assert_eq!(inf.0, 6, "bounded read must stop after expected+1 bytes, read {}", inf.0);
        assert_eq!(fs::read(&f).unwrap(), b"world");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "a part was left behind");

        assert_eq!(write_atomic(&f, &b"there"[..], Some(5)).unwrap(), 5);
        assert_eq!(fs::read(&f).unwrap(), b"there");
        assert_eq!(write_atomic(&f, &b"world"[..], Some(5)).unwrap(), 5);

        let copy_dst = unique_path(&f).unwrap();
        assert_eq!(copy_dst.file_name().unwrap(), "hello 2.txt");
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
    fn temp_file_collision_retries_without_deleting_preexisting_file() {
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-link-fs-coll-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let seq = AtomicU64::new(42);
        let pre = dir.join(format!(".olo-fs-part-{:08x}{:08x}", std::process::id(), 42u64));
        fs::write(&pre, b"someone elses file").unwrap();

        let (tmp, _f, guard) = create_temp_beside(&dir, &seq).unwrap();
        assert_ne!(tmp, pre, "must skip colliding candidate");
        assert_eq!(fs::read(&pre).unwrap(), b"someone elses file", "colliding file must never be deleted");
        drop(guard);
        assert!(!tmp.exists(), "owned temp file must be cleaned up on drop");
        assert_eq!(fs::read(&pre).unwrap(), b"someone elses file", "pre-existing file still intact after drop");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_unique_writes_never_overwrite_each_other() {
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-link-fs-uniq-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let base = dir.join("Pasted Picture.png");
        let n_threads = 12usize;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(n_threads));
        let results: Vec<(PathBuf, String)> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..n_threads)
                .map(|i| {
                    let b = barrier.clone();
                    let base = base.clone();
                    s.spawn(move || {
                        let body = format!("payload-{i:02}");
                        b.wait();
                        let (path, n) = write_atomic_unique(&base, body.as_bytes(), Some(body.len() as u64)).unwrap();
                        assert_eq!(n, body.len() as u64);
                        (path, body)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let paths: std::collections::HashSet<PathBuf> = results.iter().map(|(p, _)| p.clone()).collect();
        assert_eq!(paths.len(), n_threads, "every concurrent --unique write must get a distinct path: {paths:?}");
        for (p, expected) in &results {
            assert_eq!(fs::read_to_string(p).unwrap(), *expected, "content corrupted for {}", p.display());
        }
        // No leftover `.olo-fs-part-*` temp files.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), n_threads);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn round2_fs_op_hardening_private_staging_eintr_noreplace_and_unique_errors() {
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-link-fs-r2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 1. Staging temp file is created mode 0600, and replacing a private 0600 file preserves 0600.
        let seq = AtomicU64::new(1);
        let (tmp, _f, guard) = create_temp_beside(&dir, &seq).unwrap();
        assert_eq!(fs::symlink_metadata(&tmp).unwrap().permissions().mode() & 0o777, 0o600, "staging temp must be mode 0600");
        drop(guard);

        let secret = dir.join("secret.key");
        fs::write(&secret, b"old-secret").unwrap();
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
        write_atomic(&secret, &b"new-secret"[..], Some(10)).unwrap();
        assert_eq!(fs::read(&secret).unwrap(), b"new-secret");
        assert_eq!(fs::symlink_metadata(&secret).unwrap().permissions().mode() & 0o777, 0o600);

        // 2. `copy_bounded` retries `EINTR` when probing for excess trailing bytes.
        struct EintrAfterExact {
            data: &'static [u8],
            pos: usize,
            eintr_left: usize,
            trailing: Option<u8>,
        }
        impl Read for EintrAfterExact {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.pos < self.data.len() {
                    let n = (self.data.len() - self.pos).min(buf.len());
                    buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
                    self.pos += n;
                    return Ok(n);
                }
                if self.eintr_left > 0 {
                    self.eintr_left -= 1;
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "simulated EINTR"));
                }
                if let Some(b) = self.trailing.take() {
                    buf[0] = b;
                    return Ok(1);
                }
                Ok(0)
            }
        }
        let eintr_ok = dir.join("eintr-ok.txt");
        write_atomic(&eintr_ok, EintrAfterExact { data: b"hello", pos: 0, eintr_left: 2, trailing: None }, Some(5)).unwrap();
        assert_eq!(fs::read(&eintr_ok).unwrap(), b"hello");
        let eintr_err = write_atomic(&eintr_ok, EintrAfterExact { data: b"hello", pos: 0, eintr_left: 2, trailing: Some(b'!') }, Some(5)).unwrap_err();
        assert_eq!(eintr_err.kind(), io::ErrorKind::InvalidData);

        // 3. `move_user_noreplace` refuses to overwrite an existing file and moves directories atomically.
        let src_file = dir.join("src.txt");
        let dst_file = dir.join("dst.txt");
        fs::write(&src_file, b"source").unwrap();
        fs::write(&dst_file, b"existing").unwrap();
        let err = move_user_noreplace(&src_file, &dst_file).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&src_file).unwrap(), b"source");
        assert_eq!(fs::read(&dst_file).unwrap(), b"existing");

        let src_dir = dir.join("src-dir");
        let dst_dir = dir.join("dst-dir");
        fs::create_dir(&src_dir).unwrap();
        fs::write(src_dir.join("inside.txt"), b"ok").unwrap();
        move_user_noreplace(&src_dir, &dst_dir).unwrap();
        assert!(!src_dir.exists() && fs::read(dst_dir.join("inside.txt")).unwrap() == b"ok");

        // 4. `unique_path` propagates non-NotFound errors (ENOTDIR) instead of treating them as free.
        let not_a_dir_child = dst_file.join("child.txt");
        let uerr = unique_path(&not_a_dir_child).unwrap_err();
        assert_ne!(uerr.kind(), io::ErrorKind::NotFound, "ENOTDIR must propagate: {uerr}");

        // 5. Concurrent `copy_unique` calls never overwrite each other.
        let copy_src = dir.join("copy-src.txt");
        fs::write(&copy_src, b"copy-body").unwrap();
        let copy_base = dir.join("copied.txt");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
        let paths: std::collections::HashSet<PathBuf> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..6)
                .map(|_| {
                    let b = barrier.clone();
                    let s_p = copy_src.clone();
                    let d_p = copy_base.clone();
                    s.spawn(move || {
                        b.wait();
                        copy_unique(&s_p, &d_p).unwrap()
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(paths.len(), 6);

        // 6. A31 / A21 / N9:
        //    - Moving a directory inside itself via `move_user_noreplace` returns `InvalidInput` (NOT `Unsupported`!).
        //    - `copy_unique` copies a private 0o700 directory preserving its mode and creates a unique top-level name.
        let self_dir = dir.join("self-move-dir");
        fs::create_dir(&self_dir).unwrap();
        let self_err = move_user_noreplace(&self_dir, &self_dir.join("sub")).unwrap_err();
        assert_eq!(self_err.kind(), io::ErrorKind::InvalidInput, "N9: moving dir into itself must return InvalidInput, not Unsupported: {self_err}");

        let priv_src = dir.join("priv-src");
        fs::DirBuilder::new().mode(0o700).create(&priv_src).unwrap();
        fs::write(priv_src.join("secret.txt"), b"top-secret").unwrap();
        let priv_dst = copy_unique(&priv_src, &dir.join("priv-dst")).unwrap();
        assert_eq!(fs::metadata(&priv_dst).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::read(priv_dst.join("secret.txt")).unwrap(), b"top-secret");

        // 7. Astra #23: Failed recursive `copy_unique` stages privately and cleans
        //    up staging on inner failure, leaving ZERO visible partial destination!
        let bad_src = dir.join("bad-src");
        fs::create_dir(&bad_src).unwrap();
        fs::write(bad_src.join("good.txt"), b"first").unwrap();
        let unreadable = bad_src.join("unreadable.txt");
        fs::write(&unreadable, b"nope").unwrap();
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
        let bad_dst = dir.join("bad-dst");
        let copy_res = copy_unique(&bad_src, &bad_dst);
        let _ = fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o600));
        if unsafe { libc::geteuid() } != 0 {
            assert!(copy_res.is_err(), "unreadable child must fail copy_unique");
            assert!(!bad_dst.exists(), "Astra #23: failed recursive copy must NOT leave visible partial destination");
            let leftover_stage = fs::read_dir(&dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with(".olo-fs-stage-"));
            assert!(!leftover_stage, "Astra #23: private staging dir must be cleaned up on error");
        }

        fs::remove_dir_all(dir).unwrap();
    }
}
