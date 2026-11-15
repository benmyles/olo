//! Where extraction writes, and what compression reads.
//!
//! A `Sink` writes into one folder of its own and never outside it:
//! every name is a `safe_path` (no `..`, nothing absolute), every file is
//! made new (never through something already there), every folder on the
//! way is one it made itself, and links are made last, a symbolic one
//! only when what it points to stays inside the folder, a hard one only
//! to a file it wrote. setuid and setgid are dropped from permissions; a
//! folder's own permissions and time are set at the end, deepest first,
//! so a read-only folder is still filled.

use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use crate::{Error, Result, unique};

pub struct Sink {
    root: PathBuf,
    /// Folders made (relative).
    dirs: HashSet<PathBuf>,
    /// Files written (relative), for hard links to point to.
    files: HashSet<PathBuf>,
    symlinks: Vec<(PathBuf, String)>,
    hardlinks: Vec<(PathBuf, PathBuf)>,
    /// Folders' permissions and times, set at the end.
    dir_meta: Vec<(PathBuf, Option<u32>, Option<i64>)>,
}

/// Permissions as they're kept: the ordinary bits, without setuid,
/// setgid or sticky.
fn plain(mode: u32) -> u32 {
    mode & 0o777
}

/// Set `path`'s modification time (not following a link).
pub(crate) fn set_mtime(path: &Path, mtime: i64) {
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else { return };
    let t = libc::timespec { tv_sec: mtime as libc::time_t, tv_nsec: 0 };
    let times = [t, t];
    // SAFETY: a valid C string and two timespecs.
    unsafe {
        libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), libc::AT_SYMLINK_NOFOLLOW);
    }
}

impl Sink {
    pub fn new(root: &Path) -> Sink {
        Sink { root: root.to_owned(), dirs: HashSet::new(), files: HashSet::new(), symlinks: Vec::new(), hardlinks: Vec::new(), dir_meta: Vec::new() }
    }

    /// Every folder above `rel`, made (or one made already: never a link
    /// or a file in the way).
    fn parents(&mut self, rel: &Path) -> io::Result<()> {
        let mut at = PathBuf::new();
        let parts: Vec<_> = rel.components().collect();
        for part in &parts[..parts.len().saturating_sub(1)] {
            at.push(part);
            if self.dirs.contains(&at) {
                continue;
            }
            let full = self.root.join(&at);
            match full.symlink_metadata() {
                Ok(m) if m.is_dir() => {}
                Ok(_) => return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} is a file, and something goes in it", at.display()))),
                Err(_) => fs::create_dir(&full)?,
            }
            self.dirs.insert(at.clone());
        }
        Ok(())
    }

    pub fn dir(&mut self, rel: &Path, mode: Option<u32>, mtime: Option<i64>) -> io::Result<()> {
        self.parents(rel)?;
        let full = self.root.join(rel);
        if !self.dirs.contains(rel) {
            match full.symlink_metadata() {
                Ok(m) if m.is_dir() => {}
                Ok(_) => return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} is there already", rel.display()))),
                Err(_) => fs::create_dir(&full)?,
            }
            self.dirs.insert(rel.to_owned());
        }
        if mode.is_some() || mtime.is_some() {
            self.dir_meta.push((rel.to_owned(), mode, mtime));
        }
        Ok(())
    }

    /// Where a new file `rel` goes, its folders made (for a reader that
    /// writes the file itself).
    pub fn file_path(&mut self, rel: &Path) -> io::Result<PathBuf> {
        self.parents(rel)?;
        let full = self.root.join(rel);
        if full.symlink_metadata().is_ok() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} comes twice", rel.display())));
        }
        self.files.insert(rel.to_owned());
        Ok(full)
    }

    /// Write file `rel` from `r`.
    pub fn file(&mut self, rel: &Path, r: &mut dyn Read, mode: Option<u32>, mtime: Option<i64>) -> io::Result<()> {
        let full = self.file_path(rel)?;
        let mut f = OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(&full)?;
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = match r.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            f.write_all(&buf[..n])?;
        }
        drop(f);
        fs::set_permissions(&full, fs::Permissions::from_mode(plain(mode.unwrap_or(0o644)) | 0o400))?;
        if let Some(t) = mtime {
            set_mtime(&full, t);
        }
        Ok(())
    }

    /// A symbolic link `rel` to `target`, made at the end if it stays
    /// inside.
    pub fn symlink(&mut self, rel: &Path, target: &str) {
        self.symlinks.push((rel.to_owned(), target.to_owned()));
    }

    /// A hard link `rel` to the file `target` (both inside).
    pub fn hardlink(&mut self, rel: &Path, target: PathBuf) {
        self.hardlinks.push((rel.to_owned(), target));
    }

    /// The links, then the folders' permissions and times.
    pub fn finish(&mut self) -> Result<()> {
        for (rel, target) in std::mem::take(&mut self.hardlinks) {
            if !self.files.contains(&target) {
                continue;
            }
            let full = self.file_path(&rel)?;
            if fs::hard_link(self.root.join(&target), &full).is_err() {
                fs::copy(self.root.join(&target), &full)?;
            }
        }
        for (rel, target) in std::mem::take(&mut self.symlinks) {
            if !stays_inside(&rel, &target) {
                continue;
            }
            self.parents(&rel)?;
            let full = self.root.join(&rel);
            if full.symlink_metadata().is_ok() {
                continue;
            }
            std::os::unix::fs::symlink(&target, &full)?;
        }
        let mut metas = std::mem::take(&mut self.dir_meta);
        metas.sort_by_key(|(p, ..)| std::cmp::Reverse(p.components().count()));
        for (rel, mode, mtime) in metas {
            let full = self.root.join(&rel);
            if let Some(m) = mode {
                let _ = fs::set_permissions(&full, fs::Permissions::from_mode(plain(m) | 0o700));
            }
            if let Some(t) = mtime {
                set_mtime(&full, t);
            }
        }
        Ok(())
    }
}

/// Whether a link at `rel` to `target` points inside the folder: never
/// absolute, and never more `..` than it is deep.
fn stays_inside(rel: &Path, target: &str) -> bool {
    if target.is_empty() || target.starts_with('/') || target.contains('\0') {
        return false;
    }
    let mut depth: i64 = rel.components().count() as i64 - 1;
    for c in Path::new(target).components() {
        match c {
            Component::ParentDir => depth -= 1,
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => return false,
        }
        if depth < 0 {
            return false;
        }
    }
    true
}

/// A hidden folder of its own in `into`, for what's extracted.
pub(crate) fn stage_dir(into: &Path) -> io::Result<PathBuf> {
    static N: AtomicU64 = AtomicU64::new(0);
    loop {
        let p = into.join(format!(".olo-extracting-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        match fs::create_dir(&p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

/// What came out, put in place: one thing as it is beside the archive,
/// several in a folder named for it (a name taken gets a number).
pub(crate) fn settle(stage: &Path, into: &Path, base: &str) -> Result<Vec<PathBuf>> {
    let mut top: Vec<PathBuf> = fs::read_dir(stage)?.flatten().map(|e| e.path()).collect();
    top.sort();
    match top.as_slice() {
        [] => Ok(Vec::new()),
        [one] => {
            let to = unique(&into.join(one.file_name().unwrap_or_default()));
            fs::rename(one, &to)?;
            Ok(vec![to])
        }
        _ => {
            let folder = unique(&into.join(base));
            fs::create_dir(&folder)?;
            for p in top {
                fs::rename(&p, folder.join(p.file_name().unwrap_or_default()))?;
            }
            Ok(vec![folder])
        }
    }
}

/// What goes into an archive.
#[derive(Clone, Debug)]
pub enum WalkKind {
    File,
    Dir,
    Symlink(String),
}

#[derive(Clone, Debug)]
pub struct Walked {
    pub path: PathBuf,
    /// Its name in the archive ("photos/2026/a.jpg").
    pub name: String,
    pub kind: WalkKind,
    pub size: u64,
    pub mode: u32,
    pub mtime: i64,
}

/// Everything under `sources`, each under its own name (links not
/// followed; what can't be put in an archive, sockets and devices, left
/// out).
pub(crate) fn walk(sources: &[PathBuf], cancel: &AtomicBool) -> Result<Vec<Walked>> {
    let mut out = Vec::new();
    for s in sources {
        let name = s.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or_else(|| Error::Unsupported(format!("{} has no name", s.display())))?;
        walk_one(s, name, &mut out, cancel)?;
    }
    Ok(out)
}

fn walk_one(path: &Path, name: String, out: &mut Vec<Walked>, cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    let m = path.symlink_metadata()?;
    use std::os::unix::fs::MetadataExt;
    let base = |kind, size| Walked { path: path.to_owned(), name: name.clone(), kind, size, mode: m.mode() & 0o7777, mtime: m.mtime() };
    let ft = m.file_type();
    if ft.is_symlink() {
        let target = fs::read_link(path)?.to_string_lossy().into_owned();
        out.push(base(WalkKind::Symlink(target), 0));
    } else if ft.is_dir() {
        out.push(base(WalkKind::Dir, 0));
        let mut children: Vec<_> = fs::read_dir(path)?.flatten().collect();
        children.sort_by_key(|e| e.file_name());
        for c in children {
            let child = format!("{name}/{}", c.file_name().to_string_lossy());
            walk_one(&c.path(), child, out, cancel)?;
        }
    } else if ft.is_file() {
        out.push(base(WalkKind::File, m.len()));
    }
    Ok(())
}

/// `path`, opened to read (for an archive's entry).
pub(crate) fn open(path: &Path) -> io::Result<File> {
    File::open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_stay_inside() {
        assert!(stays_inside(Path::new("a/link"), "b"));
        assert!(stays_inside(Path::new("a/link"), "../c"));
        assert!(!stays_inside(Path::new("a/link"), "../../etc/passwd"));
        assert!(!stays_inside(Path::new("link"), "/etc/passwd"));
        assert!(!stays_inside(Path::new("link"), ".."));
    }
}
