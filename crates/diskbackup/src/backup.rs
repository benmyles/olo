//! Backing up: the folders walked, each file whose size, times and inode
//! are as in the last snapshot taken as it was (not read), the rest read a
//! chunk at a time into the repository; then each folder's tree, the trees
//! of the folders above them up to `/`, and the snapshot. Read gently: no
//! access times changed, and what was read let go of from the page cache
//! (it was the backup's, not anyone's working set).

use std::{
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
    },
    time::Instant,
};

use crate::{
    Error, Result,
    crypto::Id,
    exclude::Excludes,
    repo::{Repo, Snapshot, Summary, now},
    tree::{Kind, Node, Tree},
};

/// What a backup is doing, for whoever shows it (read as it goes).
#[derive(Debug, Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub bytes: AtomicU64,
    /// New content read so far (before compression).
    pub new_bytes: AtomicU64,
    /// The file being read.
    pub current: Mutex<String>,
}

pub struct Options<'a> {
    /// The folders to back up (absolute).
    pub paths: Vec<PathBuf>,
    pub excludes: &'a Excludes,
    pub host: String,
    /// Stay on each folder's own file system.
    pub one_file_system: bool,
}

struct Walk<'a, 'r> {
    repo: &'r mut Repo,
    opts: &'a Options<'a>,
    cancel: &'a AtomicBool,
    progress: &'a Progress,
    summary: Summary,
    /// What files are read into (one for all).
    buf: Vec<u8>,
}

/// Back up `opts.paths` into `repo`, taking unchanged files from `parent`
/// (the last snapshot); the snapshot kept, and its id.
pub fn backup(repo: &mut Repo, opts: &Options, parent: Option<(Id, &Snapshot)>, cancel: &AtomicBool, progress: &Progress) -> Result<(Id, Snapshot)> {
    let started = Instant::now();
    let stored = repo.stored;
    let mut w = Walk { repo, opts, cancel, progress, summary: Summary::default(), buf: Vec::new() };
    // Packs read back from their own lists as the index was read (an index
    // file damaged): named in an index file of their own now, so the next
    // to read it needn't.
    if w.repo.has_unnamed() {
        let _ = w.repo.find_lost();
    }
    let parent_root = match parent {
        Some((_, s)) => match w.repo.tree(&s.tree) {
            Ok(t) => Some(t),
            // Not in the index (an index file lost?): the packs no index
            // names read back from their own lists, and again; else
            // everything read as new.
            Err(Error::Damaged(_)) => {
                let _ = w.repo.find_lost();
                w.repo.tree(&s.tree).ok()
            }
            Err(e) => return Err(e),
        },
        None => None,
    };
    let root = match w.ancestors(Path::new("/"), parent_root.as_ref(), &opts.paths) {
        Ok(root) => root,
        Err(e) => {
            // Stopped or failed: the index of what went up so far up too,
            // so the next backup goes on from there.
            let _ = w.repo.write_index();
            return Err(e);
        }
    };
    let mut summary = w.summary;
    summary.stored_bytes = repo.stored - stored;
    summary.seconds = started.elapsed().as_secs_f64();
    let snapshot = Snapshot { time: now(), host: opts.host.clone(), paths: opts.paths.iter().map(|p| p.display().to_string()).collect(), tree: root, parent: parent.map(|(id, _)| id), summary };
    let id = repo.save_snapshot(&snapshot)?;
    // (What the snapshot's trees and packs stored, the index's too.)
    let mut snapshot = snapshot;
    snapshot.summary.stored_bytes = repo.stored - stored;
    Ok((id, snapshot))
}

/// `name`'s node, as the file system has it now.
fn node(name: &[u8], md: &std::fs::Metadata, kind: Kind) -> Node {
    Node {
        name: name.to_vec(),
        kind,
        mode: md.mode() & 0o7777,
        mtime: md.mtime() * 1_000_000_000 + md.mtime_nsec(),
        ctime: md.ctime() * 1_000_000_000 + md.ctime_nsec(),
        ino: md.ino(),
        size: if kind == Kind::File { md.len() } else { 0 },
        content: Vec::new(),
        subtree: None,
        target: None,
    }
}

impl Walk<'_, '_> {
    fn check(&self) -> Result<()> {
        if self.cancel.load(Relaxed) { Err(Error::Cancelled) } else { Ok(()) }
    }

    /// The tree of `at` holding only the way down to each of `paths` (the
    /// folders backed up below it), each of those whole.
    fn ancestors(&mut self, at: &Path, prev: Option<&Tree>, paths: &[PathBuf]) -> Result<Id> {
        // The next name down to each path, grouped.
        let mut below: std::collections::BTreeMap<Vec<u8>, Vec<PathBuf>> = Default::default();
        let mut whole = false;
        for p in paths {
            match p.strip_prefix(at).ok().and_then(|rest| rest.components().next()) {
                Some(Component::Normal(name)) => below.entry(name.as_encoded_bytes().to_vec()).or_default().push(p.clone()),
                _ if p == at => whole = true,
                _ => {}
            }
        }
        if whole {
            let dev = std::fs::symlink_metadata(at)?.dev();
            return self.dir(at, prev, dev);
        }
        let mut nodes = Vec::new();
        for (name, paths) in below {
            let path = at.join(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(&name));
            let Ok(md) = std::fs::symlink_metadata(&path) else {
                self.summary.errors.push(format!("{}: not there", path.display()));
                continue;
            };
            let mut n = node(&name, &md, Kind::Dir);
            let sub = prev.and_then(|t| t.get(&name)).and_then(|n| n.subtree);
            let sub = match sub {
                Some(id) => Some(self.repo.tree(&id)?),
                None => None,
            };
            n.subtree = Some(self.ancestors(&path, sub.as_ref(), &paths)?);
            nodes.push(n);
        }
        Ok(self.repo.put(&Tree { nodes }.encode(), true)?.0)
    }

    /// Folder `path`, everything in it, as a tree; its id.
    fn dir(&mut self, path: &Path, prev: Option<&Tree>, dev: u64) -> Result<Id> {
        self.check()?;
        self.summary.dirs += 1;
        let mut names: Vec<std::ffi::OsString> = match std::fs::read_dir(path) {
            Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.file_name())).collect(),
            Err(e) => {
                self.summary.errors.push(format!("{}: {e}", path.display()));
                Vec::new()
            }
        };
        names.sort_by(|a, b| a.as_encoded_bytes().cmp(b.as_encoded_bytes()));
        let mut nodes = Vec::with_capacity(names.len());
        for name in names {
            self.check()?;
            let p = path.join(&name);
            let md = match std::fs::symlink_metadata(&p) {
                Ok(md) => md,
                Err(e) => {
                    self.summary.errors.push(format!("{}: {e}", p.display()));
                    continue;
                }
            };
            let ft = md.file_type();
            let kind = if ft.is_file() {
                Kind::File
            } else if ft.is_dir() {
                Kind::Dir
            } else if ft.is_symlink() {
                Kind::Symlink
            } else {
                continue; // sockets, pipes, devices: nothing to keep
            };
            if self.opts.excludes.excluded(&p, kind == Kind::Dir) {
                continue;
            }
            let bytes = name.as_encoded_bytes();
            let mut n = node(bytes, &md, kind);
            let before = prev.and_then(|t| t.get(bytes));
            match kind {
                Kind::File => {
                    // (Unreadable: said, and left out; stopped, or the
                    // store failing: the backup stops.)
                    if !self.file(&p, &mut n, before)? {
                        continue;
                    }
                }
                Kind::Dir => {
                    if self.opts.one_file_system && md.dev() != dev {
                        continue;
                    }
                    let sub = match before.and_then(|b| b.subtree) {
                        Some(id) => self.repo.tree(&id).ok(),
                        None => None,
                    };
                    n.subtree = Some(self.dir(&p, sub.as_ref(), dev)?);
                }
                Kind::Symlink => match std::fs::read_link(&p) {
                    Ok(t) => n.target = Some(t.into_os_string().into_encoded_bytes()),
                    Err(e) => {
                        self.summary.errors.push(format!("{}: {e}", p.display()));
                        continue;
                    }
                },
            }
            nodes.push(n);
        }
        Ok(self.repo.put(&Tree { nodes }.encode(), true)?.0)
    }

    /// A file's chunks into `n`: as before if it's unchanged (and they're
    /// all still there), else read; whether it's kept. An unreadable one is
    /// said and left out (false); stopped, or the store failing, an error.
    fn file(&mut self, p: &Path, n: &mut Node, before: Option<&Node>) -> Result<bool> {
        self.summary.files += 1;
        self.summary.bytes += n.size;
        self.progress.files.fetch_add(1, Relaxed);
        self.progress.bytes.fetch_add(n.size, Relaxed);
        if let Some(b) = before
            && b.unchanged(n)
            && b.content.iter().all(|id| self.repo.has(id))
        {
            n.content = b.content.clone();
            self.summary.unchanged_files += 1;
            return Ok(true);
        }
        if let Ok(mut c) = self.progress.current.lock() {
            *c = p.display().to_string();
        }
        let file = match open_gently(p) {
            Ok(f) => f,
            Err(e) => {
                self.summary.errors.push(format!("{}: {e}", p.display()));
                return Ok(false);
            }
        };
        let mut content = Vec::new();
        let (repo, cancel, progress) = (&mut *self.repo, self.cancel, self.progress);
        let mut new_bytes = 0u64;
        // What stops the backup (not the file's own fault).
        let mut fatal = None;
        let r = crate::chunker::chunks_in(&mut self.buf, &file, |chunk| {
            if cancel.load(Relaxed) {
                fatal = Some(Error::Cancelled);
                return Err(Error::Cancelled);
            }
            let (id, new) = repo.put(chunk, false).map_err(|e| {
                fatal = Some(e);
                Error::Cancelled
            })?;
            if new {
                new_bytes += chunk.len() as u64;
                progress.new_bytes.fetch_add(chunk.len() as u64, Relaxed);
            }
            content.push(id);
            Ok(())
        });
        // What was read isn't anyone's working set: let it go.
        // SAFETY: a valid descriptor; advice only.
        unsafe { libc::posix_fadvise(std::os::fd::AsRawFd::as_raw_fd(&file), 0, 0, libc::POSIX_FADV_DONTNEED) };
        if let Some(e) = fatal {
            return Err(e);
        }
        if let Err(e) = r {
            self.summary.errors.push(format!("{}: {e}", p.display()));
            return Ok(false);
        }
        n.content = content;
        self.summary.new_bytes += new_bytes;
        if before.is_some() {
            self.summary.changed_files += 1;
        } else {
            self.summary.new_files += 1;
        }
        Ok(true)
    }
}

/// `path` is as `kept` (a snapshot's node of it) says it was: its size,
/// times and inode (so its content too, as a backup takes it).
pub fn unchanged_since(kept: &Node, path: &Path) -> bool {
    let Ok(md) = std::fs::symlink_metadata(path) else { return false };
    let kind = if md.is_dir() {
        Kind::Dir
    } else if md.file_type().is_symlink() {
        Kind::Symlink
    } else {
        Kind::File
    };
    node(&kept.name, &md, kind).unchanged(kept)
}

/// Opened for reading without changing its access time (where we may).
fn open_gently(p: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOATIME).open(p).or_else(|_| std::fs::File::open(p))
}
