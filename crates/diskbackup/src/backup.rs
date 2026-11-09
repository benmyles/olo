//! Backing up: the folders walked, each file whose size, times and inode
//! are as in the last snapshot taken as it was (not read), the rest read a
//! chunk at a time into the repository; then each folder's tree, the trees
//! of the folders above them up to `/`, and the snapshot. Read gently: no
//! access times changed, and what was read let go of from the page cache
//! (it was the backup's, not anyone's working set).
//!
//! One that doesn't finish keeps what it got through (Ben, 2026-11-08: "it
//! needs to track in-progress / unfinished backups too and they need to
//! resume if i start again"): every `CHECKPOINT`, and when it's stopped,
//! the folders done so far (and, down the one under way, as far as it got)
//! are kept as an unfinished snapshot, with the chunks of the file it was
//! part way through. The next backup goes on from it: a file it holds as
//! it is now isn't read again, besides those the last finished one holds.
//! Once one finishes, the unfinished ones go.

use std::{
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, Instant},
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
    /// What went up to the store so far (compressed and sealed).
    pub uploaded: AtomicU64,
    /// The file being read.
    pub current: Mutex<String>,
    /// What there is to go through (`count`'s, or a caller's estimate).
    pub total_files: AtomicU64,
    pub total_bytes: AtomicU64,
    /// The totals are the last backup's, not counted.
    pub estimated: AtomicBool,
    /// `COUNTING`, `READING`, `SAVING` or `CHECKING`.
    pub stage: AtomicU8,
    /// Going on from an unfinished backup (one stopped part way).
    pub resuming: AtomicBool,
    /// Files read (the rest taken as they were).
    pub read: AtomicU64,
}

/// What's done so far is kept (an unfinished snapshot) this often.
const CHECKPOINT: Duration = Duration::from_secs(5 * 60);

/// `Progress::stage`s.
pub const COUNTING: u8 = 0;
pub const READING: u8 = 1;
pub const SAVING: u8 = 2;
/// (The service's, after: the backup checked.)
pub const CHECKING: u8 = 3;
/// Stopped: what's done so far being kept.
pub const STOPPING: u8 = 4;

/// What `opts` would back up, counted (files, and their bytes) into
/// `progress`'s totals as it goes: a first backup's plan, with nothing
/// before it to go by. Only the folders' entries are read (no file's
/// content), with the backup's own rules (left out, one file system).
pub fn count(opts: &Options, cancel: &AtomicBool, progress: &Progress) -> Result<()> {
    progress.stage.store(COUNTING, Relaxed);
    fn walk(dir: &Path, dev: u64, opts: &Options, cancel: &AtomicBool, progress: &Progress) -> Result<()> {
        if cancel.load(Relaxed) {
            return Err(Error::Cancelled);
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return Ok(()) };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(md) = std::fs::symlink_metadata(&p) else { continue };
            let ft = md.file_type();
            if !(ft.is_file() || ft.is_dir()) || opts.excludes.excluded(&p, ft.is_dir()) {
                continue;
            }
            if ft.is_file() {
                progress.total_files.fetch_add(1, Relaxed);
                progress.total_bytes.fetch_add(md.len(), Relaxed);
            } else if !(opts.one_file_system && md.dev() != dev) {
                walk(&p, dev, opts, cancel, progress)?;
            }
        }
        Ok(())
    }
    for p in &opts.paths {
        if let Ok(md) = std::fs::symlink_metadata(p) {
            walk(p, md.dev(), opts, cancel, progress)?;
        }
    }
    Ok(())
}

pub struct Options<'a> {
    /// The folders to back up (absolute).
    pub paths: Vec<PathBuf>,
    pub excludes: &'a Excludes,
    pub host: String,
    /// Stay on each folder's own file system.
    pub one_file_system: bool,
}

/// A folder under way: its own node (none for `/`), and what's been gone
/// through in it so far (in name order).
struct Level {
    node: Option<Node>,
    nodes: Vec<Node>,
}

/// A folder as the backups before had it: the last finished one, and the
/// unfinished one this goes on from; and whether it's on the way down to
/// where that one stopped (its last folder at each level).
#[derive(Default)]
struct Before {
    done: Option<Tree>,
    unfinished: Option<Tree>,
    frontier: bool,
}

impl Before {
    /// Folder `name`'s, in this one. A tree that can't be read is as if it
    /// weren't there (the finished one's an error, if `strict`).
    fn child(&self, repo: &Repo, name: &[u8], strict: bool) -> Result<Before> {
        let sub = |t: &Option<Tree>| t.as_ref().and_then(|t| t.get(name)).and_then(|n| n.subtree);
        let done = match sub(&self.done) {
            Some(id) if strict => Some(repo.tree(&id)?),
            Some(id) => repo.tree(&id).ok(),
            None => None,
        };
        let unfinished = sub(&self.unfinished).and_then(|id| repo.tree(&id).ok());
        let frontier = self.frontier && self.unfinished.as_ref().and_then(|t| t.nodes.last()).is_some_and(|l| l.name == name);
        Ok(Before { done, unfinished, frontier })
    }
}

struct Walk<'a, 'r> {
    repo: &'r mut Repo,
    opts: &'a Options<'a>,
    cancel: &'a AtomicBool,
    progress: &'a Progress,
    started: Instant,
    /// What the store had been sent before (so `progress.uploaded` is this
    /// backup's).
    stored_from: u64,
    summary: Summary,
    /// What files are read into (one for all).
    buf: Vec<u8>,
    /// The folders under way, `/` first.
    levels: Vec<Level>,
    /// The last finished snapshot's id.
    parent: Option<Id>,
    /// The unfinished one this goes on from: its summary (added to this
    /// one's).
    resumed: Option<Summary>,
    /// Got past where it stopped (it isn't needed any more).
    passed: bool,
    /// This computer's unfinished snapshots from before (they go once this
    /// has got past where they stopped), and this one's own, as last kept.
    others: Vec<Id>,
    saved: Option<Id>,
    last_saved: Instant,
    /// The file being read (its size), and its chunks so far.
    under_way: Option<u64>,
    reading: Vec<Id>,
}

/// Back up `opts.paths` into `repo`, taking unchanged files from `parent`
/// (the last finished snapshot), and going on from where an unfinished one
/// of this computer's since got to; the snapshot kept, and its id.
pub fn backup(repo: &mut Repo, opts: &Options, parent: Option<(Id, &Snapshot)>, cancel: &AtomicBool, progress: &Progress) -> Result<(Id, Snapshot)> {
    let started = Instant::now();
    let stored = repo.stored;
    progress.stage.store(READING, Relaxed);
    // This computer's unfinished ones: the one since the last finished
    // that got furthest is gone on from; all go once this one finishes.
    // (One that can't be read: none, and everything is read again.)
    let unfinished: Vec<(Id, Snapshot)> = repo.unfinished().unwrap_or_default().into_iter().filter(|(_, s)| s.host == opts.host).collect();
    let after = parent.map_or(i64::MIN, |(_, s)| s.time);
    let resume = unfinished.iter().filter(|(_, s)| s.time > after).max_by_key(|(_, s)| (s.summary.files, s.time));
    progress.resuming.store(resume.is_some(), Relaxed);
    let mut w = Walk {
        repo,
        opts,
        cancel,
        progress,
        started,
        stored_from: stored,
        summary: Summary::default(),
        buf: Vec::new(),
        levels: Vec::new(),
        parent: parent.map(|(id, _)| id),
        resumed: resume.map(|(_, s)| s.summary.clone()),
        passed: resume.is_none(),
        others: unfinished.iter().map(|(id, _)| *id).collect(),
        saved: None,
        last_saved: Instant::now(),
        under_way: None,
        reading: Vec::new(),
    };
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
    let before = Before { done: parent_root, unfinished: resume.and_then(|(_, s)| w.repo.tree(&s.tree).ok()), frontier: resume.is_some() };
    w.enter(None);
    let root = match w.ancestors(Path::new("/"), before, &opts.paths).and_then(|()| w.leave()) {
        Ok(root) => root,
        Err(e) => {
            // Stopped: what's done so far kept, so the next goes on from
            // there. Failed (the store, say), or that didn't go: at least
            // the index of what went up so far, so it isn't sent again
            // (and the last checkpoint goes on).
            if matches!(e, Error::Cancelled) {
                progress.stage.store(STOPPING, Relaxed);
                let _ = w.keep_unfinished();
            }
            let _ = w.repo.write_index();
            return Err(e);
        }
    };
    progress.stage.store(SAVING, Relaxed);
    let Walk { mut summary, resumed, others, saved, .. } = w;
    let earlier = resumed.unwrap_or_default();
    summary.new_bytes += earlier.new_bytes;
    summary.stored_bytes = repo.stored - stored + earlier.stored_bytes;
    summary.seconds = started.elapsed().as_secs_f64() + earlier.seconds;
    let snapshot = Snapshot {
        time: now(),
        host: opts.host.clone(),
        paths: opts.paths.iter().map(|p| p.display().to_string()).collect(),
        tree: root,
        parent: parent.map(|(id, _)| id),
        summary,
        extra: Vec::new(),
    };
    let id = repo.save_snapshot(&snapshot)?;
    // Finished: the unfinished ones (this one's own, and those it went on
    // from) aren't needed any more.
    for u in others.iter().chain(saved.iter()) {
        let _ = repo.forget_unfinished(u);
    }
    // (What the snapshot's trees and packs stored, the index's too.)
    let mut snapshot = snapshot;
    snapshot.summary.stored_bytes = repo.stored - stored + earlier.stored_bytes;
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

    /// Into a folder (its node; none for `/`): what's gone through goes in
    /// its level till `leave`.
    fn enter(&mut self, node: Option<Node>) {
        self.levels.push(Level { node, nodes: Vec::new() });
    }

    /// Out of the folder entered last, done: its tree kept, and its node
    /// (with it) in the folder above's; the tree's id.
    fn leave(&mut self) -> Result<Id> {
        let Level { node, nodes } = self.levels.pop().expect("a folder entered");
        let tree = Tree { nodes };
        let id = match self.repo.put(&tree.encode(), true) {
            Ok((id, _)) => id,
            Err(e) => {
                self.levels.push(Level { node, nodes: tree.nodes });
                return Err(e);
            }
        };
        if let Some(mut n) = node {
            n.subtree = Some(id);
            self.add(n);
        }
        Ok(id)
    }

    /// `n`, gone through, in the folder under way.
    fn add(&mut self, n: Node) {
        self.levels.last_mut().expect("a folder entered").nodes.push(n);
    }

    /// Past where the unfinished one this goes on from had got to: a name
    /// after its last, on the way down to where it stopped (both go in name
    /// order).
    fn passing(&mut self, prev: &Before, name: &[u8]) {
        if !self.passed && prev.frontier && prev.unfinished.as_ref().and_then(|t| t.nodes.last()).is_none_or(|l| name > l.name.as_slice()) {
            self.passed = true;
        }
    }

    /// What's done so far, as trees: each folder under way as far as it
    /// got, the one under way below it last; the root's id.
    fn partial(&mut self) -> Result<Id> {
        let mut below: Option<Node> = None;
        let mut root = Id::default();
        for l in self.levels.iter().rev() {
            let mut nodes = l.nodes.clone();
            nodes.extend(below.take());
            root = self.repo.put(&Tree { nodes }.encode(), true)?.0;
            below = l.node.clone().map(|mut n| {
                n.subtree = Some(root);
                n
            });
        }
        Ok(root)
    }

    /// What's done so far kept as this backup's unfinished snapshot (in
    /// place of the one it kept before), so one stopped, failed or cut off
    /// (the computer turned off) goes on from there; the unfinished ones of
    /// before go too, once this has got past where they stopped.
    fn keep_unfinished(&mut self) -> Result<()> {
        let mut summary = self.summary.clone();
        // (The file it was part way through isn't in it: its chunks so far
        // are, as `extra`.)
        if let Some(size) = self.under_way {
            summary.files -= 1;
            summary.bytes -= size;
        }
        // (Nothing gone through yet: what was kept before stays.)
        if (summary.files == 0 && self.reading.is_empty()) || self.levels.is_empty() {
            return Ok(());
        }
        let tree = self.partial()?;
        self.repo.flush()?;
        let earlier = self.resumed.clone().unwrap_or_default();
        summary.new_bytes += earlier.new_bytes;
        summary.stored_bytes = self.repo.stored - self.stored_from + earlier.stored_bytes;
        summary.seconds = self.started.elapsed().as_secs_f64() + earlier.seconds;
        let s = Snapshot {
            time: now(),
            host: self.opts.host.clone(),
            paths: self.opts.paths.iter().map(|p| p.display().to_string()).collect(),
            tree,
            parent: self.parent,
            summary,
            extra: self.reading.clone(),
        };
        let id = self.repo.save_unfinished(&s)?;
        if let Some(old) = self.saved.replace(id).filter(|old| *old != id) {
            let _ = self.repo.forget_unfinished(&old);
        }
        if self.passed {
            for o in std::mem::take(&mut self.others) {
                let _ = self.repo.forget_unfinished(&o);
            }
        }
        self.last_saved = Instant::now();
        Ok(())
    }

    /// Into the folder under way (whose level is entered): the way down to
    /// each of `paths` (the folders backed up below `at`), each of those
    /// whole.
    fn ancestors(&mut self, at: &Path, prev: Before, paths: &[PathBuf]) -> Result<()> {
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
        for (name, paths) in below {
            self.passing(&prev, &name);
            let path = at.join(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(&name));
            let Ok(md) = std::fs::symlink_metadata(&path) else {
                self.summary.errors.push(format!("{}: not there", path.display()));
                continue;
            };
            let sub = prev.child(self.repo, &name, true)?;
            self.enter(Some(node(&name, &md, Kind::Dir)));
            self.ancestors(&path, sub, &paths)?;
            self.leave()?;
        }
        Ok(())
    }

    /// Folder `path`, everything in it, into its level (entered).
    fn dir(&mut self, path: &Path, prev: Before, dev: u64) -> Result<()> {
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
        for name in names {
            self.check()?;
            // (Between files: the level as it is, whole.)
            if self.last_saved.elapsed() >= CHECKPOINT {
                self.keep_unfinished()?;
            }
            let bytes = name.as_encoded_bytes();
            self.passing(&prev, bytes);
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
            let mut n = node(bytes, &md, kind);
            match kind {
                Kind::File => {
                    let (done, unfinished) = (prev.done.as_ref().and_then(|t| t.get(bytes)), prev.unfinished.as_ref().and_then(|t| t.get(bytes)));
                    // (Unreadable: said, and left out; stopped, or the
                    // store failing: the backup stops.)
                    if !self.file(&p, &mut n, done, unfinished)? {
                        continue;
                    }
                }
                Kind::Dir => {
                    if self.opts.one_file_system && md.dev() != dev {
                        continue;
                    }
                    let sub = prev.child(self.repo, bytes, false)?;
                    self.enter(Some(n));
                    self.dir(&p, sub, dev)?;
                    self.leave()?;
                    continue;
                }
                Kind::Symlink => match std::fs::read_link(&p) {
                    Ok(t) => n.target = Some(t.into_os_string().into_encoded_bytes()),
                    Err(e) => {
                        self.summary.errors.push(format!("{}: {e}", p.display()));
                        continue;
                    }
                },
            }
            self.add(n);
        }
        Ok(())
    }

    /// A file's chunks into `n`: as before if it's unchanged since the
    /// unfinished backup this goes on from or the last finished one (and
    /// they're all still there), else read; whether it's kept. An
    /// unreadable one is said and left out (false); stopped, or the store
    /// failing, an error (`reading` holding its chunks so far).
    fn file(&mut self, p: &Path, n: &mut Node, done: Option<&Node>, unfinished: Option<&Node>) -> Result<bool> {
        self.summary.files += 1;
        self.summary.bytes += n.size;
        self.progress.files.fetch_add(1, Relaxed);
        self.progress.bytes.fetch_add(n.size, Relaxed);
        let repo = &*self.repo;
        if let Some(b) = [unfinished, done].into_iter().flatten().find(|b| b.unchanged(n) && b.content.iter().all(|id| repo.has(id))) {
            n.content = b.content.clone();
            // (New or changed since the last finished one, even if it was
            // read before the backup it went on from stopped.)
            match done {
                Some(d) if d.unchanged(n) => self.summary.unchanged_files += 1,
                Some(_) => self.summary.changed_files += 1,
                None => self.summary.new_files += 1,
            }
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
        self.reading.clear();
        self.under_way = Some(n.size);
        self.progress.read.fetch_add(1, Relaxed);
        let (repo, cancel, progress, stored_from, reading) = (&mut *self.repo, self.cancel, self.progress, self.stored_from, &mut self.reading);
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
                progress.uploaded.store(repo.stored.saturating_sub(stored_from), Relaxed);
            }
            reading.push(id);
            Ok(())
        });
        // What was read isn't anyone's working set: let it go.
        // SAFETY: a valid descriptor; advice only.
        unsafe { libc::posix_fadvise(std::os::fd::AsRawFd::as_raw_fd(&file), 0, 0, libc::POSIX_FADV_DONTNEED) };
        if let Some(e) = fatal {
            return Err(e);
        }
        self.under_way = None;
        if let Err(e) = r {
            self.reading.clear();
            self.summary.errors.push(format!("{}: {e}", p.display()));
            return Ok(false);
        }
        n.content = std::mem::take(&mut self.reading);
        self.summary.new_bytes += new_bytes;
        if done.is_some() {
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
