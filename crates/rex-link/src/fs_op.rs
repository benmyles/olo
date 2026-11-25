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
//!   write PATH               atomically write stdin bytes to PATH

use std::ffi::CString;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
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
       olo-rex-link fs write PATH";

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

fn unique_path(base: &Path) -> PathBuf {
    if fs::symlink_metadata(base).is_err() {
        return base.to_path_buf();
    }
    let parent = base.parent().unwrap_or(Path::new("/"));
    let name = base.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "New Folder".into());
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !e.is_empty() => (s, format!(".{e}")),
        _ => (name.as_str(), String::new()),
    };
    for n in 2..=10_000u32 {
        let cand = parent.join(format!("{stem} {n}{ext}"));
        if fs::symlink_metadata(&cand).is_err() {
            return cand;
        }
    }
    base.to_path_buf()
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
        fs::create_dir(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dst.join(entry.file_name()))?;
        }
        let _ = fs::set_permissions(dst, fs::Permissions::from_mode(lmeta.permissions().mode() & 0o777));
        return Ok(());
    }
    let mut reader = fs::File::open(src)?;
    let mut writer = fs::OpenOptions::new().write(true).create_new(true).open(dst)?;
    io::copy(&mut reader, &mut writer)?;
    drop(writer);
    let _ = fs::set_permissions(dst, fs::Permissions::from_mode(lmeta.permissions().mode() & 0o777));
    Ok(())
}

fn write_atomic(dst: &Path, mut input: impl Read) -> io::Result<u64> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let existing_mode = fs::symlink_metadata(dst).ok().filter(|m| m.is_file()).map(|m| m.permissions().mode() & 0o777);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dst.parent().unwrap_or(Path::new("/")).join(format!(".olo-fs-part-{:08x}{:08x}", std::process::id(), seq));
    let res = (|| -> io::Result<u64> {
        let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        let n = io::copy(&mut input, &mut f)?;
        drop(f);
        if let Some(mode) = existing_mode {
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(mode));
        }
        fs::rename(&tmp, dst)?;
        Ok(n)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
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
            let target = if make_unique { unique_path(&base) } else { base };
            if let Err(e) = fs::create_dir(&target) {
                fail_json(1, e);
            }
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
            let to = if make_unique && from != base_to { unique_path(&base_to) } else { base_to };
            if from != to && fs::symlink_metadata(&to).is_ok() {
                fail_json(1, "a file with that name already exists");
            }
            if let Err(e) = fs::rename(&from, &to) {
                fail_json(1, e);
            }
            print_json(serde_json::json!({ "ok": true, "path": to.to_string_lossy() }));
        }
        "copy" => {
            let (Some(raw_from), Some(raw_to)) = (it.next(), it.next()) else { fail_json(2, "copy requires FROM TO") };
            let (from, to) = (resolve_path(&raw_from), unique_path(&resolve_path(&raw_to)));
            if let Err(e) = copy_recursive(&from, &to) {
                fail_json(1, e);
            }
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
            for a in it {
                match a.as_str() {
                    "--unique" => make_unique = true,
                    other => fail_json(2, format!("unknown option {other}")),
                }
            }
            let base = resolve_path(&raw);
            let target = if make_unique { unique_path(&base) } else { base };
            match write_atomic(&target, io::stdin().lock()) {
                Ok(bytes) => print_json(serde_json::json!({ "ok": true, "path": target.to_string_lossy(), "bytes": bytes })),
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
        let n = write_atomic(&f, &b"world"[..]).unwrap();
        assert_eq!(n, 5);
        assert_eq!(fs::read(&f).unwrap(), b"world");

        let copy_dst = unique_path(&f);
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
}
