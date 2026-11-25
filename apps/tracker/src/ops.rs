//! File operations — copy, move, duplicate, move to Trash, restore from
//! the Trash, empty Trash, compress, extract —
//! on worker threads, with live progress and cancellation, so Tracker never
//! blocks however big the job (see "The responsiveness rule").
//!
//! Each reports what it did (`Outcome`: what it made, what it moved from
//! where to where), which is what Edit ▸ Undo takes back (`undo.rs`);
//! `Kind::PutBack` takes items back to exactly where they were.
//!
//! Copies use `copy_file_range` in chunks: zero-copy in the kernel (and
//! reflinks on btrfs) while still reporting progress and checking for Stop
//! between chunks. Moves on one file system are renames; across file
//! systems they copy and then remove the source.

use olo_sync::Mutex;
use std::{
    fs::{self as sfs, File},
    io::{self, Read, Write},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};

use crate::fs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Copy,
    Move,
    Duplicate,
    Trash,
    /// Put items in the Trash back where they came from.
    Restore,
    EmptyTrash,
    /// Permanently delete remote items on a Rex host (confirmed first).
    Delete,
    /// Each source back to exactly its target (`start_to`): a move
    /// undone. Not over anything now there, nor of anything gone.
    PutBack,
    /// The sources into a new archive of this format, beside them
    /// (`olo_archive`).
    Compress(olo_archive::Format),
    /// Each archive's contents out beside it (or into `dest`).
    Extract,
}

/// Counters the worker updates and the status window reads.
#[derive(Debug, Default)]
pub struct Progress {
    pub total_bytes: AtomicU64,
    pub done_bytes: AtomicU64,
    pub total_items: AtomicU64,
    pub done_items: AtomicU64,
    /// Still measuring what there is to do.
    pub counting: AtomicBool,
    /// Name of the item being worked on.
    pub current: Mutex<String>,
}

impl Progress {
    /// 0–1. Each item counts as `ITEM_COST` bytes of work as well as its
    /// size, so thousands of tiny files and one huge one both move the bar
    /// at the pace the job actually goes.
    pub fn fraction(&self) -> f32 {
        const ITEM_COST: f64 = 64.0 * 1024.0;
        let (tb, db) = (self.total_bytes.load(Ordering::Relaxed), self.done_bytes.load(Ordering::Relaxed));
        let (ti, di) = (self.total_items.load(Ordering::Relaxed), self.done_items.load(Ordering::Relaxed));
        let total = tb as f64 + ti as f64 * ITEM_COST;
        if total == 0.0 || self.counting.load(Ordering::Relaxed) {
            return 0.0;
        }
        ((db.min(tb) as f64 + di.min(ti) as f64 * ITEM_COST) / total).min(1.0) as f32
    }

    fn set_current(&self, p: &Path) {
        if let Ok(mut c) = self.current.lock() {
            *c = fs::leaf_name(p);
        }
    }
}

pub struct Job {
    pub id: u64,
    pub kind: Kind,
    /// "Copying 3 items to “Documents”".
    pub title: String,
    /// "Finished copying 3 items to “Documents”", for a notification.
    pub finished: String,
    /// Where the items went, if anywhere.
    pub dest: Option<PathBuf>,
    /// What it was started on (to run it again with a password).
    pub sources: Vec<PathBuf>,
    pub progress: Arc<Progress>,
    pub cancel: Arc<AtomicBool>,
    pub started: Instant,
}

/// What a finished job did.
#[derive(Debug, Default)]
pub struct Outcome {
    /// New items (copies, moved items at their new paths).
    pub created: Vec<PathBuf>,
    /// What was moved (moved, put back, trashed, restored): from, to.
    pub moved: Vec<(PathBuf, PathBuf)>,
    pub error: Option<String>,
    pub cancelled: bool,
    /// An archive wants a password: none was given (false), or the one
    /// given was wrong (true).
    pub needs_password: Option<bool>,
}

fn title(kind: Kind, sources: &[PathBuf], dest: Option<&Path>) -> String {
    let what = match sources {
        [one] => format!("“{}”", fs::leaf_name(one)),
        many => format!("{} items", many.len()),
    };
    let to = dest.map(|d| format!(" to “{}”", fs::display_name(d))).unwrap_or_default();
    match kind {
        Kind::Copy => format!("Copying {what}{to}"),
        Kind::Move => format!("Moving {what}{to}"),
        Kind::Duplicate => format!("Duplicating {what}"),
        Kind::Trash => olo_kit::tr!("Moving {} to the Trash", what),
        Kind::Restore => format!("Restoring {what}"),
        Kind::EmptyTrash => olo_kit::words::tr("Emptying the Trash").into(),
        Kind::Delete => olo_kit::tr!("Deleting {}", what),
        Kind::PutBack => olo_kit::tr!("Putting {} back", what),
        Kind::Compress(f) => format!("Compressing {what} ({})", f.label()),
        Kind::Extract => format!("Extracting {what}"),
    }
}

/// The job's title once it is done: "Finished copying …".
fn finished(kind: Kind, sources: &[PathBuf], dest: Option<&Path>) -> String {
    let t = title(kind, sources, dest);
    let mut lower = t.chars();
    let first = lower.next().map(|c| c.to_lowercase().collect::<String>()).unwrap_or_default();
    format!("Finished {first}{}", lower.as_str())
}

/// Start a job on a worker thread (`targets`: each source's own, for
/// `Kind::PutBack`); `done` runs on that thread when it ends.
#[cfg(test)]
pub fn start(id: u64, kind: Kind, sources: Vec<PathBuf>, dest: Option<PathBuf>, targets: Vec<PathBuf>, done: impl FnOnce(Outcome) + Send + 'static) -> Job {
    start_with(id, kind, sources, dest, targets, None, done)
}

/// `start`, with an archive's password (Compress, Extract).
pub fn start_with(id: u64, kind: Kind, sources: Vec<PathBuf>, dest: Option<PathBuf>, targets: Vec<PathBuf>, password: Option<String>, done: impl FnOnce(Outcome) + Send + 'static) -> Job {
    let job = Job {
        id,
        kind,
        title: title(kind, &sources, dest.as_deref()),
        finished: finished(kind, &sources, dest.as_deref()),
        dest: dest.clone(),
        sources: sources.clone(),
        progress: Arc::new(Progress { counting: AtomicBool::new(true), ..Default::default() }),
        cancel: Arc::new(AtomicBool::new(false)),
        started: Instant::now(),
    };
    let (progress, cancel) = (job.progress.clone(), job.cancel.clone());
    std::thread::Builder::new()
        .name("tracker-op".into())
        .spawn(move || {
            let w = Worker { p: &progress, cancel: &cancel };
            let mut out = Outcome::default();
            let r = match kind {
                Kind::Compress(_) | Kind::Extract => w.archive(kind, &sources, dest.as_deref(), targets.first().map(PathBuf::as_path), password.as_deref(), &mut out),
                _ => w.run(kind, &sources, dest.as_deref(), &targets, &mut out),
            };
            match r {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => out.cancelled = true,
                Err(e) => out.error = Some(e.to_string()),
            }
            done(out);
        })
        .expect("thread");
    job
}

struct Worker<'a> {
    p: &'a Progress,
    cancel: &'a AtomicBool,
}

/// The error a cancelled job unwinds with.
fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "cancelled")
}

/// Attach the item's name to an error, for the alert.
fn context(e: io::Error, p: &Path) -> io::Error {
    if e.kind() == io::ErrorKind::Interrupted {
        return e;
    }
    io::Error::new(e.kind(), format!("“{}”: {e}", fs::leaf_name(p)))
}

impl Worker<'_> {
    /// Compress or extract (`olo_archive`), its progress as bytes of the
    /// whole; what it made in `out.created`.
    fn archive(&self, kind: Kind, sources: &[PathBuf], dest: Option<&Path>, named: Option<&Path>, password: Option<&str>, out: &mut Outcome) -> io::Result<()> {
        let p = self.p;
        let mut progress = |pr: &olo_archive::Progress| {
            p.total_bytes.store(pr.total, Ordering::Relaxed);
            p.done_bytes.store(pr.done, Ordering::Relaxed);
            if pr.total > 0 {
                p.counting.store(false, Ordering::Relaxed);
            }
            if let Ok(mut c) = p.current.lock() {
                *c = pr.item.rsplit('/').next().unwrap_or(&pr.item).to_owned();
            }
        };
        let ctl = olo_archive::Control::new(self.cancel, &mut progress);
        let result = match kind {
            Kind::Compress(format) => {
                let dir = match dest {
                    Some(d) => d.to_owned(),
                    None => sources.first().and_then(|s| s.parent()).unwrap_or(Path::new("/")).to_owned(),
                };
                // (Named by Compress As…, or after what's in it.)
                let to = olo_archive::unique(&named.map(Path::to_owned).unwrap_or_else(|| dir.join(olo_archive::archive_name(sources, format))));
                olo_archive::compress(sources, format, &to, password, &ctl).map(|()| out.created.push(to))
            }
            _ => sources.iter().try_for_each(|s| {
                let into = dest.map(Path::to_owned).unwrap_or_else(|| s.parent().unwrap_or(Path::new("/")).to_owned());
                olo_archive::extract(s, &into, password, &ctl).map(|made| out.created.extend(made))
            }),
        };
        match result {
            Ok(()) => Ok(()),
            Err(olo_archive::Error::Cancelled) => Err(cancelled()),
            Err(olo_archive::Error::Password(wrong)) => {
                out.needs_password = Some(wrong);
                Ok(())
            }
            Err(e) => Err(io::Error::other(e.to_string())),
        }
    }

    fn check(&self) -> io::Result<()> {
        if self.cancel.load(Ordering::Relaxed) { Err(cancelled()) } else { Ok(()) }
    }

    fn run(&self, kind: Kind, sources: &[PathBuf], dest: Option<&Path>, targets: &[PathBuf], out: &mut Outcome) -> io::Result<()> {
        let created = &mut out.created;
        let moved = &mut out.moved;
        let any_remote = sources.iter().any(|s| fs::is_remote(s)) || dest.is_some_and(fs::is_remote) || targets.iter().any(|t| fs::is_remote(t));
        if any_remote || kind == Kind::Delete {
            return self.run_remote(kind, sources, dest, targets, out);
        }
        match kind {
            Kind::Copy | Kind::Duplicate => {
                for s in sources {
                    self.measure(s)?;
                }
                self.p.counting.store(false, Ordering::Relaxed);
                for s in sources {
                    let dir = match dest {
                        Some(d) => d.to_owned(),
                        None => s.parent().unwrap_or(Path::new("/")).to_owned(),
                    };
                    let target = fs::copy_target(s, &dir);
                    created.push(target.clone());
                    if let Err(e) = self.copy(s, &target) {
                        created.pop();
                        return Err(e);
                    }
                }
            }
            Kind::Move => {
                let dir = dest.ok_or_else(|| io::Error::other("no destination"))?;
                self.p.total_items.store(sources.len() as u64, Ordering::Relaxed);
                self.p.counting.store(false, Ordering::Relaxed);
                for s in sources {
                    self.check()?;
                    self.p.set_current(s);
                    let target = fs::unique(dir, &s.file_name().unwrap_or_default().to_string_lossy());
                    self.move_one(s, &target).map_err(|e| context(e, s))?;
                    moved.push((s.clone(), target.clone()));
                    created.push(target);
                    self.p.done_items.fetch_add(1, Ordering::Relaxed);
                }
            }
            Kind::PutBack => {
                self.p.total_items.store(sources.len() as u64, Ordering::Relaxed);
                self.p.counting.store(false, Ordering::Relaxed);
                for (s, target) in sources.iter().zip(targets) {
                    self.check()?;
                    self.p.set_current(s);
                    let name = |p: &Path| p.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    if sfs::symlink_metadata(s).is_err() {
                        return Err(io::Error::other(olo_kit::tr!("“{}” isn't there any more", name(s))));
                    }
                    if sfs::symlink_metadata(target).is_ok() {
                        return Err(io::Error::other(olo_kit::tr!("something else is called “{}” there now", name(target))));
                    }
                    // Its folder may have gone meanwhile: made again.
                    if let Some(dir) = target.parent() {
                        sfs::create_dir_all(dir).map_err(|e| context(e, dir))?;
                    }
                    self.move_one(s, target).map_err(|e| context(e, s))?;
                    moved.push((s.clone(), target.clone()));
                    created.push(target.clone());
                    self.p.done_items.fetch_add(1, Ordering::Relaxed);
                }
            }
            Kind::Trash => {
                self.p.total_items.store(sources.len() as u64, Ordering::Relaxed);
                self.p.counting.store(false, Ordering::Relaxed);
                for s in sources {
                    self.check()?;
                    self.p.set_current(s);
                    let (target, info) = fs::trash_slot(s)?;
                    self.move_one(s, &target).map_err(|e| {
                        let _ = sfs::remove_file(&info);
                        context(e, s)
                    })?;
                    moved.push((s.clone(), target));
                    self.p.done_items.fetch_add(1, Ordering::Relaxed);
                }
            }
            Kind::Restore => {
                self.p.total_items.store(sources.len() as u64, Ordering::Relaxed);
                self.p.counting.store(false, Ordering::Relaxed);
                for s in sources {
                    self.check()?;
                    self.p.set_current(s);
                    // (Emptied, or put back by hand, since: an undo said so.)
                    if sfs::symlink_metadata(s).is_err() {
                        return Err(io::Error::other(olo_kit::tr!("“{}” isn't in the Trash any more", s.file_name().unwrap_or_default().to_string_lossy())));
                    }
                    let (origin, info) = fs::trash_origin(s)?;
                    let dir = origin.parent().ok_or_else(|| io::Error::other("no folder to restore into"))?;
                    // Its folder may have gone too; something new in its
                    // place keeps its name and this one gets a number.
                    sfs::create_dir_all(dir).map_err(|e| context(e, dir))?;
                    let target = fs::unique(dir, &origin.file_name().unwrap_or_default().to_string_lossy());
                    self.move_one(s, &target).map_err(|e| context(e, s))?;
                    let _ = sfs::remove_file(&info);
                    moved.push((s.clone(), target.clone()));
                    created.push(target);
                    self.p.done_items.fetch_add(1, Ordering::Relaxed);
                }
            }
            // (Run by `archive` or `run_remote`.)
            Kind::Compress(_) | Kind::Extract | Kind::Delete => {}
            Kind::EmptyTrash => {
                // Empty at once, as far as anyone looking can tell: the
                // Trash's files and info are swapped for empty folders (one
                // atomic rename each), so its icon and any Trash window
                // change now; what was in them is deleted where it went,
                // with progress. Cancelled or failed, what's left goes back.
                let trash = fs::trash_dir();
                let (files, info) = (fs::trash_files(), trash.join("info"));
                sfs::create_dir_all(&files)?;
                sfs::create_dir_all(&info)?;
                let stage = fs::unique(&trash, EMPTYING);
                let (old_files, old_info) = (stage.join("files"), stage.join("info"));
                sfs::create_dir_all(&old_files)?;
                sfs::create_dir_all(&old_info)?;
                if exchange(&files, &old_files).is_err() {
                    // (A file system that can't swap: emptied in place.)
                    let _ = sfs::remove_dir_all(&stage);
                    return self.empty_in(&files, &info);
                }
                let _ = exchange(&info, &old_info);
                // Left by an emptying that was cut short: going too.
                let earlier: Vec<PathBuf> = sfs::read_dir(&trash)?.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(EMPTYING) && e.path() != stage).map(|e| e.path()).collect();
                let r = self.empty_in(&old_files, &old_info).and_then(|()| {
                    for e in &earlier {
                        self.remove(e).map_err(|err| context(err, e))?;
                    }
                    Ok(())
                });
                match r {
                    Ok(()) => sfs::remove_dir_all(&stage)?,
                    Err(e) => {
                        put_back(&old_files, &old_info, &files, &info);
                        let _ = sfs::remove_dir_all(&stage);
                        return Err(e);
                    }
                }
            }
        }
        Ok(())
    }

    /// Add `p`'s bytes and items to the totals.
    fn measure(&self, p: &Path) -> io::Result<()> {
        self.check()?;
        let md = sfs::symlink_metadata(p).map_err(|e| context(e, p))?;
        self.p.total_items.fetch_add(1, Ordering::Relaxed);
        if md.is_dir() {
            for e in sfs::read_dir(p).map_err(|e| context(e, p))?.flatten() {
                self.measure(&e.path())?;
            }
        } else if md.is_file() {
            self.p.total_bytes.fetch_add(md.len(), Ordering::Relaxed);
        }
        Ok(())
    }

    fn count_items(&self, p: &Path) -> io::Result<()> {
        self.check()?;
        self.p.total_items.fetch_add(1, Ordering::Relaxed);
        if sfs::symlink_metadata(p)?.is_dir() {
            for e in sfs::read_dir(p)?.flatten() {
                self.count_items(&e.path())?;
            }
        }
        Ok(())
    }

    /// Rename, or copy then remove across file systems.
    fn move_one(&self, from: &Path, to: &Path) -> io::Result<()> {
        match sfs::rename(from, to) {
            Ok(()) => Ok(()),
            Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
                self.measure(from)?;
                if let Err(e) = self.copy(from, to) {
                    // Leave the source untouched and drop the partial copy.
                    let _ = self.remove_quietly(to);
                    return Err(e);
                }
                self.remove(from)
            }
            Err(e) => Err(e),
        }
    }

    fn copy(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.check()?;
        self.p.set_current(from);
        let md = sfs::symlink_metadata(from).map_err(|e| context(e, from))?;
        let r = if md.file_type().is_symlink() {
            std::os::unix::fs::symlink(sfs::read_link(from)?, to)
        } else if md.is_dir() {
            sfs::create_dir(to).map_err(|e| context(e, to))?;
            for e in sfs::read_dir(from).map_err(|e| context(e, from))?.flatten() {
                self.copy(&e.path(), &to.join(e.file_name()))?;
            }
            sfs::set_permissions(to, md.permissions())
        } else {
            self.copy_file(from, to, &md)
        };
        self.p.done_items.fetch_add(1, Ordering::Relaxed);
        r.map_err(|e| context(e, from))
    }

    fn copy_file(&self, from: &Path, to: &Path, md: &sfs::Metadata) -> io::Result<()> {
        let src = File::open(from)?;
        let dst = sfs::OpenOptions::new().write(true).create_new(true).open(to)?;
        let r = self.copy_data(&src, &dst, md.len());
        if r.is_err() {
            drop(dst);
            let _ = sfs::remove_file(to);
            return r;
        }
        dst.set_permissions(md.permissions())?;
        let times = sfs::FileTimes::new().set_modified(md.modified()?).set_accessed(md.accessed()?);
        let _ = dst.set_times(times);
        Ok(())
    }

    /// Copy contents in chunks, kernel-side when possible.
    fn copy_data(&self, src: &File, dst: &File, len: u64) -> io::Result<()> {
        const CHUNK: usize = 8 << 20;
        let mut left = len;
        let mut kernel = true;
        let mut buf = Vec::new();
        while left > 0 || !kernel {
            self.check()?;
            let n = if kernel {
                // SAFETY: valid fds; null offsets use and advance the file positions.
                let n = unsafe { libc::copy_file_range(src.as_raw_fd(), std::ptr::null_mut(), dst.as_raw_fd(), std::ptr::null_mut(), CHUNK, 0) };
                if n < 0 {
                    let e = io::Error::last_os_error();
                    match e.raw_os_error() {
                        // Not supported here: fall back to read/write.
                        Some(libc::EXDEV | libc::EINVAL | libc::ENOSYS | libc::EOPNOTSUPP) if left == len => {
                            kernel = false;
                            buf.resize(1 << 20, 0);
                            continue;
                        }
                        _ => return Err(e),
                    }
                }
                n as usize
            } else {
                let n = (&*src).read(&mut buf)?;
                (&*dst).write_all(&buf[..n])?;
                n
            };
            if n == 0 {
                break; // end of file (it may have shrunk meanwhile)
            }
            left = left.saturating_sub(n as u64);
            self.p.done_bytes.fetch_add(n as u64, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Delete `p` (recursively), counting items as it goes.
    fn remove(&self, p: &Path) -> io::Result<()> {
        self.check()?;
        self.p.set_current(p);
        let md = sfs::symlink_metadata(p)?;
        if md.is_dir() {
            for e in sfs::read_dir(p)?.flatten() {
                self.remove(&e.path())?;
            }
            sfs::remove_dir(p)?;
        } else {
            sfs::remove_file(p)?;
        }
        self.p.done_items.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Delete everything in a Trash's `files` and the `info` beside it.
    fn empty_in(&self, files: &Path, info: &Path) -> io::Result<()> {
        let items: Vec<PathBuf> = sfs::read_dir(files)?.flatten().map(|e| e.path()).collect();
        for i in &items {
            self.count_items(i)?;
        }
        self.p.counting.store(false, Ordering::Relaxed);
        for i in &items {
            self.remove(i).map_err(|e| context(e, i))?;
            if let Some(name) = i.file_name() {
                let mut record = name.to_os_string();
                record.push(".trashinfo");
                let _ = sfs::remove_file(info.join(record));
            }
        }
        Ok(())
    }

    fn remove_quietly(&self, p: &Path) -> io::Result<()> {
        if sfs::symlink_metadata(p)?.is_dir() { sfs::remove_dir_all(p) } else { sfs::remove_file(p) }
    }

    /// Operations where any source or destination is a `rex://HOST/PATH` URI
    /// (same-host rename/copy/delete via `olo-rex-link fs`, cross-host and
    /// desktop↔host transfers via `olo-rex-sync` with progress and Stop).
    fn run_remote(&self, kind: Kind, sources: &[PathBuf], dest: Option<&Path>, targets: &[PathBuf], out: &mut Outcome) -> io::Result<()> {
        let created = &mut out.created;
        let moved = &mut out.moved;
        match kind {
            Kind::Delete => {
                self.p.total_items.store(sources.len() as u64, Ordering::Relaxed);
                self.p.counting.store(false, Ordering::Relaxed);
                for s in sources {
                    self.check()?;
                    self.p.set_current(s);
                    let rp = fs::rex_path(s).ok_or_else(|| io::Error::other("not a remote path"))?;
                    crate::rex::remove(&rp).map_err(|e| context(io::Error::other(e), s))?;
                    self.p.done_items.fetch_add(1, Ordering::Relaxed);
                }
            }
            Kind::PutBack => {
                self.p.total_items.store(sources.len() as u64, Ordering::Relaxed);
                self.p.counting.store(false, Ordering::Relaxed);
                for (s, target) in sources.iter().zip(targets) {
                    self.check()?;
                    self.p.set_current(s);
                    let (Some(sr), Some(tr)) = (fs::rex_path(s), fs::rex_path(target)) else {
                        return Err(io::Error::other("can’t undo across local and remote paths"));
                    };
                    let st = crate::rex::stat(&sr).map_err(|e| context(io::Error::other(e), s))?;
                    if !st.exists {
                        return Err(io::Error::other(olo_kit::tr!("“{}” isn't there any more", fs::leaf_name(s))));
                    }
                    let dst_st = crate::rex::stat(&tr).map_err(|e| context(io::Error::other(e), target))?;
                    if dst_st.exists {
                        return Err(io::Error::other(olo_kit::tr!("something else is called “{}” there now", fs::leaf_name(target))));
                    }
                    let back = crate::rex::rename(&sr, &tr, false).map_err(|e| context(io::Error::other(e), s))?;
                    let back_pb = back.to_path_buf();
                    moved.push((s.clone(), back_pb.clone()));
                    created.push(back_pb);
                    self.p.done_items.fetch_add(1, Ordering::Relaxed);
                }
            }
            Kind::Copy | Kind::Duplicate => {
                self.p.total_items.store(sources.len() as u64, Ordering::Relaxed);
                for s in sources {
                    if !fs::is_remote(s) {
                        let _ = self.measure(s);
                    } else if let Some(rp) = fs::rex_path(s)
                        && let Ok(st) = crate::rex::stat(&rp)
                        && let Some(e) = st.entry
                    {
                        self.p.total_bytes.fetch_add(e.size, Ordering::Relaxed);
                    }
                }
                self.p.counting.store(false, Ordering::Relaxed);
                for s in sources {
                    self.check()?;
                    self.p.set_current(s);
                    let dir = match dest {
                        Some(d) => d.to_owned(),
                        None => fs::parent_of(s).ok_or_else(|| io::Error::other("no destination folder"))?,
                    };
                    let made = self.copy_any(s, &dir).map_err(|e| context(e, s))?;
                    created.push(made);
                    self.p.done_items.fetch_add(1, Ordering::Relaxed);
                }
            }
            Kind::Move => {
                let dir = dest.ok_or_else(|| io::Error::other("no destination"))?;
                self.p.total_items.store(sources.len() as u64, Ordering::Relaxed);
                for s in sources {
                    if !fs::is_remote(s) {
                        let _ = self.measure(s);
                    } else if let Some(rp) = fs::rex_path(s)
                        && let Ok(st) = crate::rex::stat(&rp)
                        && let Some(e) = st.entry
                    {
                        self.p.total_bytes.fetch_add(e.size, Ordering::Relaxed);
                    }
                }
                self.p.counting.store(false, Ordering::Relaxed);
                for s in sources {
                    self.check()?;
                    self.p.set_current(s);
                    if let (Some(sr), Some(dr)) = (fs::rex_path(s), fs::rex_path(dir))
                        && sr.host.eq_ignore_ascii_case(&dr.host)
                    {
                        let target = dr.join(sr.file_name());
                        let moved_to = crate::rex::rename(&sr, &target, true).map_err(|e| context(io::Error::other(e), s))?;
                        let pb = moved_to.to_path_buf();
                        moved.push((s.clone(), pb.clone()));
                        created.push(pb);
                    } else {
                        let made = self.copy_any(s, dir).map_err(|e| context(e, s))?;
                        // Only remove source once the transfer succeeded cleanly.
                        if let Some(sr) = fs::rex_path(s) {
                            crate::rex::remove(&sr).map_err(|e| context(io::Error::other(e), s))?;
                        } else {
                            self.remove(s).map_err(|e| context(e, s))?;
                        }
                        created.push(made);
                    }
                    self.p.done_items.fetch_add(1, Ordering::Relaxed);
                }
            }
            _ => return Err(io::Error::other("This operation isn’t available for remote folders.")),
        }
        Ok(())
    }

    /// Copy one source (`s`, local or `rex://`) into folder `dir` (local or `rex://`).
    fn copy_any(&self, s: &Path, dir: &Path) -> io::Result<PathBuf> {
        let leaf = fs::leaf_name(s);
        let same_parent = fs::parent_of(s).as_deref() == Some(dir);
        let wanted_name = if same_parent { fs::copy_name(&leaf) } else { leaf };
        match (fs::rex_path(s), fs::rex_path(dir)) {
            // Same remote host: native copy on the host.
            (Some(sr), Some(dr)) if sr.host.eq_ignore_ascii_case(&dr.host) => {
                let target = dr.join(&wanted_name);
                let made = crate::rex::copy_remote(&sr, &target).map_err(io::Error::other)?;
                Ok(made.to_path_buf())
            }
            // Remote -> Local: olo-rex-sync download with progress & Stop.
            (Some(sr), None) => {
                let st = crate::rex::stat(&sr).map_err(io::Error::other)?;
                let is_dir = st.entry.as_ref().is_some_and(|e| e.is_dir);
                let target = fs::unique(dir, &wanted_name);
                let local_name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(wanted_name);
                crate::rex::download(&sr, dir, &local_name, is_dir, self.p, self.cancel)
            }
            // Local -> Remote: olo-rex-sync upload with progress & Stop.
            (None, Some(dr)) => {
                let md = sfs::symlink_metadata(s)?;
                let is_dir = md.is_dir();
                let made = crate::rex::upload(s, &dr, &wanted_name, is_dir, self.p, self.cancel)?;
                Ok(made.to_path_buf())
            }
            // Remote host A -> Remote host B: stage on disk in /var/tmp via olo-rex-sync, then upload.
            (Some(sr), Some(dr)) => {
                let st = crate::rex::stat(&sr).map_err(io::Error::other)?;
                let is_dir = st.entry.as_ref().is_some_and(|e| e.is_dir);
                let stage = PathBuf::from(format!("/var/tmp/olo-tracker-hop-{}-{}", std::process::id(), self.p.done_items.load(Ordering::Relaxed)));
                let _ = sfs::remove_dir_all(&stage);
                sfs::create_dir_all(&stage)?;
                let res = (|| -> io::Result<PathBuf> {
                    let local = crate::rex::download(&sr, &stage, &wanted_name, is_dir, self.p, self.cancel)?;
                    let made = crate::rex::upload(&local, &dr, &wanted_name, is_dir, self.p, self.cancel)?;
                    Ok(made.to_path_buf())
                })();
                let _ = sfs::remove_dir_all(&stage);
                res
            }
            (None, None) => {
                let target = fs::copy_target(s, dir);
                self.copy(s, &target)?;
                Ok(target)
            }
        }
    }
}

/// What an emptying's folder in the Trash is called (".emptying 2", …).
const EMPTYING: &str = ".emptying";

/// Swap two paths, atomically (both stay there throughout).
fn exchange(a: &Path, b: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let (ca, cb) = (std::ffi::CString::new(a.as_os_str().as_bytes())?, std::ffi::CString::new(b.as_os_str().as_bytes())?);
    // SAFETY: two NUL-terminated paths, relative to nothing.
    let r = unsafe { libc::renameat2(libc::AT_FDCWD, ca.as_ptr(), libc::AT_FDCWD, cb.as_ptr(), libc::RENAME_EXCHANGE) };
    if r == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// An emptying cut short: what is left in `old_files` (and its records in
/// `old_info`) back in the Trash, under names free there.
fn put_back(old_files: &Path, old_info: &Path, files: &Path, info: &Path) {
    let Ok(left) = sfs::read_dir(old_files) else { return };
    for e in left.flatten() {
        let name = e.file_name();
        let target = fs::unique(files, &name.to_string_lossy());
        if sfs::rename(e.path(), &target).is_err() {
            continue;
        }
        let mut record = name.clone();
        record.push(".trashinfo");
        let mut to = target.file_name().unwrap_or_default().to_os_string();
        to.push(".trashinfo");
        let _ = sfs::rename(old_info.join(record), info.join(to));
    }
}

/// The status bar's trailing text: "12.3 of 45 MB" (or items).
pub fn amount(job: &Job) -> String {
    let p = &job.progress;
    let (tb, db) = (p.total_bytes.load(Ordering::Relaxed), p.done_bytes.load(Ordering::Relaxed));
    let (ti, di) = (p.total_items.load(Ordering::Relaxed), p.done_items.load(Ordering::Relaxed));
    if p.counting.load(Ordering::Relaxed) {
        "Preparing…".into()
    } else if tb > 0 {
        format!("{} of {}", fs::format_size(db.min(tb)), fs::format_size(tb))
    } else {
        format!("{} of {}", count(di.min(ti)), plural(ti, "item"))
    }
}

/// The line under the bar: the current item (cut short to fit, as it
/// changes), and how many items and the time left (shown whole).
pub fn detail(job: &Job) -> (String, String) {
    let p = &job.progress;
    let current = p.current.lock().map(|c| c.clone()).unwrap_or_default();
    let (tb, ti, di) = (p.total_bytes.load(Ordering::Relaxed), p.total_items.load(Ordering::Relaxed), p.done_items.load(Ordering::Relaxed));
    let mut parts = Vec::new();
    if p.counting.load(Ordering::Relaxed) {
        parts.push(olo_kit::words::count(ti, "1 item found so far", "{n} items found so far"));
    } else {
        if tb > 0 && ti > 1 {
            parts.push(format!("{} of {}", count(di.min(ti)), plural(ti, "item")));
        }
        // Estimate once there is enough to go on.
        let f = p.fraction() as f64;
        let elapsed = job.started.elapsed().as_secs_f64();
        if f > 0.02 && elapsed > 2.0 {
            let left = elapsed * (1.0 - f) / f;
            parts.push(if left < 60.0 { olo_kit::tr!("about {} s left", left.ceil().max(1.0)) } else { olo_kit::tr!("about {} min left", (left / 60.0).ceil()) });
        }
    }
    (current, parts.join(" · "))
}

/// 12,345
fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn plural(n: u64, what: &str) -> String {
    if n == 1 { format!("1 {what}") } else { format!("{} {what}s", count(n)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn run(kind: Kind, sources: Vec<PathBuf>, dest: Option<PathBuf>) -> Outcome {
        let (tx, rx) = mpsc::channel();
        start(1, kind, sources, dest, Vec::new(), move |o| tx.send(o).unwrap());
        rx.recv().unwrap()
    }

    #[test]
    fn an_emptying_swaps_and_puts_back_what_is_left() {
        let dir = std::env::temp_dir().join(format!("olo-empty-{}", std::process::id()));
        let _ = sfs::remove_dir_all(&dir);
        let (files, info, old_files, old_info) = (dir.join("files"), dir.join("info"), dir.join("s/files"), dir.join("s/info"));
        for d in [&files, &info, &old_files, &old_info] {
            sfs::create_dir_all(d).unwrap();
        }
        // Swapped at once: the Trash's folder is empty, what was in it aside.
        sfs::write(files.join("a.txt"), "a").unwrap();
        exchange(&files, &old_files).unwrap();
        assert!(sfs::read_dir(&files).unwrap().next().is_none());
        assert!(old_files.join("a.txt").exists());
        // Cancelled: it goes back, beside one trashed meanwhile.
        sfs::write(old_info.join("a.txt.trashinfo"), "[Trash Info]").unwrap();
        sfs::write(files.join("a.txt"), "new").unwrap();
        put_back(&old_files, &old_info, &files, &info);
        assert_eq!(sfs::read_to_string(files.join("a 2.txt")).unwrap(), "a");
        assert!(info.join("a 2.txt.trashinfo").exists() && sfs::read_dir(&old_files).unwrap().next().is_none());
        let _ = sfs::remove_dir_all(&dir);
    }

    fn run_pw(kind: Kind, sources: Vec<PathBuf>, dest: Option<PathBuf>, password: Option<&str>) -> Outcome {
        let (tx, rx) = mpsc::channel();
        start_with(1, kind, sources, dest, Vec::new(), password.map(str::to_owned), move |o| tx.send(o).unwrap());
        rx.recv().unwrap()
    }

    /// Compress beside, extract beside (into a folder named for it when
    /// it holds several things), a password asked for and checked.
    #[test]
    fn compress_and_extract() {
        let dir = std::env::current_exe().unwrap().parent().unwrap().join(format!("olo-arch-ops-{}", std::process::id()));
        let _ = sfs::remove_dir_all(&dir);
        sfs::create_dir_all(dir.join("photos/sub")).unwrap();
        sfs::write(dir.join("photos/sub/a.txt"), "a").unwrap();
        sfs::write(dir.join("notes.txt"), "n").unwrap();
        let o = run(Kind::Compress(olo_archive::Format::Zip), vec![dir.join("photos"), dir.join("notes.txt")], None);
        assert!(o.error.is_none(), "{o:?}");
        assert_eq!(o.created, [dir.join("Archive.zip")]);
        let o = run(Kind::Extract, vec![dir.join("Archive.zip")], None);
        assert_eq!(o.created, [dir.join("Archive")]);
        assert_eq!(sfs::read_to_string(dir.join("Archive/photos/sub/a.txt")).unwrap(), "a");
        // Undo puts what they made in the Trash: it's what they made.
        assert!(matches!(crate::undo::Change::of(Kind::Extract, &o), Some(crate::undo::Change::Made(_))));

        let o = run_pw(Kind::Compress(olo_archive::Format::SevenZip), vec![dir.join("notes.txt")], None, Some("pw"));
        assert_eq!(o.created, [dir.join("notes.txt.7z")]);
        let o = run(Kind::Extract, vec![dir.join("notes.txt.7z")], Some(dir.join("photos")));
        assert_eq!(o.needs_password, Some(false), "{o:?}");
        let o = run_pw(Kind::Extract, vec![dir.join("notes.txt.7z")], Some(dir.join("photos")), Some("no"));
        assert!(o.needs_password == Some(true) || o.error.is_some(), "{o:?}");
        let o = run_pw(Kind::Extract, vec![dir.join("notes.txt.7z")], Some(dir.join("photos")), Some("pw"));
        assert_eq!(o.created, [dir.join("photos/notes.txt")], "{o:?}");
        let _ = sfs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_move_duplicate() {
        let dir = std::env::temp_dir().join(format!("olo-ops-{}", std::process::id()));
        let _ = sfs::remove_dir_all(&dir);
        sfs::create_dir_all(dir.join("a/sub")).unwrap();
        sfs::create_dir_all(dir.join("b")).unwrap();
        sfs::write(dir.join("a/sub/big.bin"), vec![7u8; 3 << 20]).unwrap();
        sfs::write(dir.join("a/x.txt"), "x").unwrap();
        std::os::unix::fs::symlink("x.txt", dir.join("a/link")).unwrap();

        let o = run(Kind::Copy, vec![dir.join("a")], Some(dir.join("b")));
        assert!(o.error.is_none(), "{o:?}");
        assert_eq!(o.created, [dir.join("b/a")]);
        assert_eq!(sfs::read(dir.join("b/a/sub/big.bin")).unwrap().len(), 3 << 20);
        assert_eq!(sfs::read_link(dir.join("b/a/link")).unwrap(), Path::new("x.txt"));

        let o = run(Kind::Duplicate, vec![dir.join("a/x.txt")], None);
        assert_eq!(o.created, [dir.join("a/x copy.txt")]);

        let o = run(Kind::Move, vec![dir.join("a/x copy.txt")], Some(dir.join("b")));
        assert_eq!(o.created, [dir.join("b/x copy.txt")]);
        assert!(!dir.join("a/x copy.txt").exists());
        let _ = sfs::remove_dir_all(&dir);
    }

    /// Items go back where their .trashinfo says, into folders made again
    /// if need be, never over something that has taken their place.
    #[test]
    fn restore_from_the_trash() {
        let dir = std::env::temp_dir().join(format!("olo-ops-r-{}", std::process::id()));
        let _ = sfs::remove_dir_all(&dir);
        let (files, info) = (dir.join("Trash/files"), dir.join("Trash/info"));
        sfs::create_dir_all(&files).unwrap();
        sfs::create_dir_all(&info).unwrap();
        sfs::create_dir_all(dir.join("docs")).unwrap();
        let trashed = |name: &str, from: &str| {
            sfs::write(files.join(name), name).unwrap();
            let path = dir.join(from).display().to_string().replace(' ', "%20");
            sfs::write(info.join(format!("{name}.trashinfo")), format!("[Trash Info]\nPath={path}\nDeletionDate=2026-10-26T22:00:00\n")).unwrap();
            files.join(name)
        };
        let a = trashed("a b.txt", "docs/a b.txt");
        let gone = trashed("deep.txt", "gone/away/deep.txt");
        let clash = trashed("taken.txt", "docs/taken.txt");
        sfs::write(dir.join("docs/taken.txt"), "new").unwrap();
        let o = run(Kind::Restore, vec![a, gone, clash], None);
        assert!(o.error.is_none(), "{o:?}");
        assert_eq!(o.created, [dir.join("docs/a b.txt"), dir.join("gone/away/deep.txt"), dir.join("docs/taken 2.txt")]);
        assert_eq!(sfs::read_to_string(dir.join("docs/taken.txt")).unwrap(), "new");
        assert_eq!(sfs::read_to_string(dir.join("docs/taken 2.txt")).unwrap(), "taken.txt");
        assert_eq!(sfs::read_dir(&files).unwrap().count() + sfs::read_dir(&info).unwrap().count(), 0, "nothing left in the Trash");
        // No record: an error, and it stays put.
        sfs::write(files.join("orphan"), "").unwrap();
        let o = run(Kind::Restore, vec![files.join("orphan")], None);
        assert!(o.error.is_some_and(|e| e.contains("no record")));
        assert!(files.join("orphan").exists());
        let _ = sfs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_removes_partial_copy() {
        let dir = std::env::temp_dir().join(format!("olo-ops-c-{}", std::process::id()));
        let _ = sfs::remove_dir_all(&dir);
        sfs::create_dir_all(dir.join("dst")).unwrap();
        sfs::write(dir.join("f.bin"), vec![1u8; 1 << 20]).unwrap();
        let (tx, rx) = mpsc::channel();
        let job = start(1, Kind::Copy, vec![dir.join("f.bin")], Some(dir.join("dst")), Vec::new(), move |o| tx.send(o).unwrap());
        job.cancel.store(true, Ordering::Relaxed);
        let o = rx.recv().unwrap();
        // Either it finished before the cancel landed, or nothing is left behind.
        if o.cancelled {
            assert!(!dir.join("dst/f.bin").exists());
        }
        let _ = sfs::remove_dir_all(&dir);
    }
}
