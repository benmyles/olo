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
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
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

/// Wakeable `eventfd` paired with [`Job::cancel`] so blocking Unix-socket waits
/// in [`crate::rex`] observe Stop immediately with zero 200 ms polling.
#[derive(Debug)]
pub struct CancelWake(Option<OwnedFd>);

impl Default for CancelWake {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelWake {
    pub fn new() -> Self {
        // SAFETY: eventfd(0, EFD_NONBLOCK | EFD_CLOEXEC) returns a fresh fd or -1.
        let fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
        if fd < 0 {
            Self(None)
        } else {
            // SAFETY: fd >= 0 is a freshly created owned descriptor.
            Self(Some(unsafe { OwnedFd::from_raw_fd(fd) }))
        }
    }

    pub fn wake(&self) {
        if let Some(fd) = &self.0 {
            let one: u64 = 1;
            // SAFETY: writing 8 bytes to a non-blocking eventfd is thread-safe.
            let _ = unsafe { libc::write(fd.as_raw_fd(), (&raw const one).cast::<libc::c_void>(), 8) };
        }
    }

    pub fn raw_fd(&self) -> Option<RawFd> {
        self.0.as_ref().map(AsRawFd::as_raw_fd)
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
    pub cancel_wake: Arc<CancelWake>,
    pub started: Instant,
}

impl Job {
    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Release);
        self.cancel_wake.wake();
    }
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
        cancel_wake: Arc::new(CancelWake::new()),
        started: Instant::now(),
    };
    let (progress, cancel, cancel_wake) = (job.progress.clone(), job.cancel.clone(), job.cancel_wake.clone());
    std::thread::Builder::new()
        .name("tracker-op".into())
        .spawn(move || {
            let w = Worker { p: &progress, cancel: &cancel, cancel_wake: &cancel_wake };
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
    cancel_wake: &'a CancelWake,
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
            for entry in sfs::read_dir(from).map_err(|e| context(e, from))? {
                let e = entry.map_err(|err| context(err, from))?;
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
                    match (fs::rex_path(s), fs::rex_path(dir)) {
                        (Some(sr), Some(dr)) if sr.host.eq_ignore_ascii_case(&dr.host) => {
                            let target = dr.join(sr.file_name());
                            let moved_to = crate::rex::rename(&sr, &target, true).map_err(|e| context(io::Error::other(e), s))?;
                            let pb = moved_to.to_path_buf();
                            moved.push((s.clone(), pb.clone()));
                            created.push(pb);
                        }
                        (None, None) => {
                            // Local item inside a mixed local/remote selection: use
                            // same-fs atomic no-replace rename (or checked cross-fs
                            // copy + leaf-verified removal), never unchecked remove.
                            let target = fs::unique(dir, &s.file_name().unwrap_or_default().to_string_lossy());
                            self.move_local_checked(s, &target).map_err(|e| context(e, s))?;
                            moved.push((s.clone(), target.clone()));
                            created.push(target);
                        }
                        _ => {
                            let copied = self.copy_any_checked(s, dir, true).map_err(|e| context(e, s))?;
                            // Only remove source when the transfer explicitly confirmed
                            // completeness AND source identity/endpoint matches what was copied.
                            if !copied.complete {
                                return Err(context(io::Error::other("Transfer did not confirm completeness (update Rex); leaving original in place."), s));
                            }
                            if let Some((sr, is_remote_dir, expect_res)) = copied.remote_source {
                                if is_remote_dir {
                                    // Never recursively `remove_dir_all` a remote source
                                    // folder from root-only metadata: keep the source intact
                                    // so modified or newly added descendants cannot be lost.
                                    created.push(copied.made);
                                    return Err(context(
                                        io::Error::other("Copied folder safely, but kept the original remote folder because per-entry verified remote removal is not yet available."),
                                        s,
                                    ));
                                }
                                let expect = expect_res.map_err(|e| context(io::Error::other(e), s))?;
                                crate::rex::remove_verified(&sr, &expect).map_err(|e| context(io::Error::other(e), s))?;
                            } else if let Some(local_manifest) = copied.local_source {
                                local_manifest.remove_verified().map_err(|e| context(e, s))?;
                            } else {
                                return Err(context(io::Error::other("Missing verified source identity; leaving original in place."), s));
                            }
                            created.push(copied.made);
                        }
                    }
                    self.p.done_items.fetch_add(1, Ordering::Relaxed);
                }
            }
            _ => return Err(io::Error::other("This operation isn’t available for remote folders.")),
        }
        Ok(())
    }

    /// Move a local source safely: atomic `renameat2(RENAME_NOREPLACE)` on the
    /// same filesystem, or full leaf-verified copy + bottom-up verified removal
    /// across filesystems (`EXDEV`).
    fn move_local_checked(&self, from: &Path, to: &Path) -> io::Result<()> {
        match local_rename_noreplace(from, to) {
            Ok(()) => Ok(()),
            Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
                let manifest = LocalSourceManifest::capture(from)?;
                self.measure(from)?;
                if let Err(ce) = self.copy(from, to) {
                    let _ = self.remove_quietly(to);
                    return Err(ce);
                }
                manifest.remove_verified()
            }
            Err(e) => Err(e),
        }
    }

    /// Copy one source (`s`, local or `rex://`) into folder `dir` (local or `rex://`).
    fn copy_any(&self, s: &Path, dir: &Path) -> io::Result<PathBuf> {
        self.copy_any_checked(s, dir, false).map(|c| c.made)
    }

    fn copy_any_checked(&self, s: &Path, dir: &Path, for_move: bool) -> io::Result<CopiedItem> {
        let leaf = fs::leaf_name(s);
        let same_parent = fs::parent_of(s).as_deref() == Some(dir);
        let wanted_name = if same_parent { fs::copy_name(&leaf) } else { leaf };
        match (fs::rex_path(s), fs::rex_path(dir)) {
            // Same remote host: native copy on the host (cancellable via Stop).
            (Some(sr), Some(dr)) if sr.host.eq_ignore_ascii_case(&dr.host) => {
                let target = dr.join(&wanted_name);
                let made = crate::rex::copy_remote(&sr, &target, self.cancel, Some(self.cancel_wake))?;
                Ok(CopiedItem { made: made.to_path_buf(), complete: true, remote_source: None, local_source: None })
            }
            // Remote -> Local: olo-rex-sync download with progress & Stop.
            (Some(sr), None) => {
                let st = crate::rex::stat(&sr).map_err(io::Error::other)?;
                if !st.exists {
                    return Err(io::Error::new(io::ErrorKind::NotFound, format!("“{}” doesn’t exist on {}.", sr.file_name(), sr.host)));
                }
                // Treat symlinks-to-directories as symlinks, never as real directories!
                let is_dir = st.entry.as_ref().is_some_and(|e| e.is_dir && !e.is_link);
                let expect_res = crate::rex::RemoveExpect::from_stat(&st);
                let target = fs::unique(dir, &wanted_name);
                let local_name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(wanted_name);
                let res = crate::rex::download_checked(&sr, dir, &local_name, is_dir, self.p, self.cancel, Some(self.cancel_wake))?;
                let expect_res = expect_res.and_then(|exp| {
                    if let Some(dl_ep) = &res.endpoint_id
                        && exp.endpoint_id != *dl_ep
                    {
                        return Err("Remote host endpoint changed between stat and download.".into());
                    }
                    Ok(exp)
                });
                Ok(CopiedItem { made: res.path, complete: res.complete, remote_source: Some((sr, is_dir, expect_res)), local_source: None })
            }
            // Local -> Remote: olo-rex-sync upload with progress & Stop.
            (None, Some(dr)) => {
                let (is_dir, manifest) = if for_move {
                    let m = LocalSourceManifest::capture(s)?;
                    (m.is_dir, Some(m))
                } else {
                    (sfs::symlink_metadata(s)?.is_dir(), None)
                };
                let res = crate::rex::upload_checked(s, &dr, &wanted_name, is_dir, self.p, self.cancel, Some(self.cancel_wake))?;
                Ok(CopiedItem { made: res.path.to_path_buf(), complete: res.complete, remote_source: None, local_source: manifest })
            }
            // Remote host A -> Remote host B: exclusive 0700 stage on disk in /var/tmp per hop.
            (Some(sr), Some(dr)) => {
                let st = crate::rex::stat(&sr).map_err(io::Error::other)?;
                if !st.exists {
                    return Err(io::Error::new(io::ErrorKind::NotFound, format!("“{}” doesn’t exist on {}.", sr.file_name(), sr.host)));
                }
                let is_dir = st.entry.as_ref().is_some_and(|e| e.is_dir && !e.is_link);
                let expect_res = crate::rex::RemoveExpect::from_stat(&st);
                let stage_guard = HopStageDir::create()?;
                let dl = crate::rex::download_checked(&sr, &stage_guard.0, &wanted_name, is_dir, self.p, self.cancel, Some(self.cancel_wake))?;
                let expect_res = expect_res.and_then(|exp| {
                    if let Some(dl_ep) = &dl.endpoint_id
                        && exp.endpoint_id != *dl_ep
                    {
                        return Err("Source host endpoint changed between stat and download.".into());
                    }
                    Ok(exp)
                });
                let up = crate::rex::upload_checked(&dl.path, &dr, &wanted_name, is_dir, self.p, self.cancel, Some(self.cancel_wake))?;
                Ok(CopiedItem { made: up.path.to_path_buf(), complete: dl.complete && up.complete, remote_source: Some((sr, is_dir, expect_res)), local_source: None })
            }
            (None, None) => {
                let target = fs::copy_target(s, dir);
                self.copy(s, &target)?;
                Ok(CopiedItem { made: target, complete: true, remote_source: None, local_source: None })
            }
        }
    }
}

struct CopiedItem {
    made: PathBuf,
    complete: bool,
    remote_source: Option<(olo_kit_base::tracker::RexPath, bool, Result<crate::rex::RemoveExpect, String>)>,
    local_source: Option<LocalSourceManifest>,
}

fn local_rename_noreplace(src: &Path, dst: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c_src = std::ffi::CString::new(src.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL byte"))?;
    let c_dst = std::ffi::CString::new(dst.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL byte"))?;
    // SAFETY: both pointers are valid NUL-terminated paths; AT_FDCWD is standard.
    let r = unsafe { libc::renameat2(libc::AT_FDCWD, c_src.as_ptr(), libc::AT_FDCWD, c_dst.as_ptr(), libc::RENAME_NOREPLACE) };
    if r == 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    if matches!(err.raw_os_error(), Some(libc::ENOSYS | libc::EINVAL)) {
        let lmeta = sfs::symlink_metadata(src)?;
        if lmeta.file_type().is_file() {
            match sfs::hard_link(src, dst) {
                Ok(()) => {
                    if let Err(e) = sfs::remove_file(src) {
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
            let target = sfs::read_link(src)?;
            std::os::unix::fs::symlink(&target, dst)?;
            if let Err(e) = sfs::remove_file(src) {
                return Err(io::Error::new(e.kind(), format!("destination symlink published at {}, but removing source failed: {e}", dst.display())));
            }
            return Ok(());
        }
        return Err(io::Error::new(io::ErrorKind::Unsupported, "atomic no-replace rename is not supported for directories on this filesystem"));
    }
    Err(err)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LocalLeafSnap {
    path: PathBuf,
    dev: u64,
    ino: u64,
    size: u64,
    mtime: i64,
    mtime_nsec: i64,
    ctime: i64,
    ctime_nsec: i64,
}

impl LocalLeafSnap {
    fn from_meta(path: PathBuf, md: &sfs::Metadata) -> Self {
        Self { path, dev: md.dev(), ino: md.ino(), size: md.len(), mtime: md.mtime(), mtime_nsec: md.mtime_nsec(), ctime: md.ctime(), ctime_nsec: md.ctime_nsec() }
    }
}

/// Complete pre-transfer manifest of a local source (single file/symlink or
/// directory tree). On post-transfer deletion, every leaf is verified against
/// its pre-transfer `lstat` identity before unlinking, and directories are
/// removed bottom-up with non-recursive `rmdir` so modified or newly added
/// descendants are NEVER deleted.
struct LocalSourceManifest {
    is_dir: bool,
    leaves: Vec<LocalLeafSnap>,
    /// Directories in bottom-up (post-order) removal order.
    dirs_bottom_up: Vec<PathBuf>,
}

impl LocalSourceManifest {
    fn capture(root: &Path) -> io::Result<Self> {
        let md = sfs::symlink_metadata(root)?;
        if !md.is_dir() {
            return Ok(Self { is_dir: false, leaves: vec![LocalLeafSnap::from_meta(root.to_path_buf(), &md)], dirs_bottom_up: Vec::new() });
        }
        let mut leaves = Vec::new();
        let mut dirs_bottom_up = Vec::new();
        fn walk(dir: &Path, leaves: &mut Vec<LocalLeafSnap>, dirs_bottom_up: &mut Vec<PathBuf>) -> io::Result<()> {
            for entry in sfs::read_dir(dir)? {
                let entry = entry?;
                let p = entry.path();
                let emd = sfs::symlink_metadata(&p)?;
                if emd.is_dir() {
                    walk(&p, leaves, dirs_bottom_up)?;
                } else {
                    leaves.push(LocalLeafSnap::from_meta(p, &emd));
                }
            }
            dirs_bottom_up.push(dir.to_path_buf());
            Ok(())
        }
        walk(root, &mut leaves, &mut dirs_bottom_up)?;
        Ok(Self { is_dir: true, leaves, dirs_bottom_up })
    }

    fn remove_verified(&self) -> io::Result<()> {
        for leaf in &self.leaves {
            let md = sfs::symlink_metadata(&leaf.path).map_err(|_| io::Error::other("Source changed during transfer; leaving remaining original files in place."))?;
            if md.is_dir() || LocalLeafSnap::from_meta(leaf.path.clone(), &md) != *leaf {
                return Err(io::Error::other("Source changed during transfer; leaving modified file in place."));
            }
            sfs::remove_file(&leaf.path)?;
        }
        for dir in &self.dirs_bottom_up {
            // Non-recursive `rmdir` fails with ENOTEMPTY if any new file appeared
            // inside the folder during transfer, preserving the new file safely!
            sfs::remove_dir(dir).map_err(|e| {
                if e.raw_os_error() == Some(libc::ENOTEMPTY) || e.kind() == io::ErrorKind::DirectoryNotEmpty {
                    io::Error::other("New files appeared in the source folder during transfer; leaving remaining folder in place.")
                } else {
                    e
                }
            })?;
        }
        Ok(())
    }
}

/// Exclusive `0700` outer staging directory created per host-to-host hop under
/// `/var/tmp`, cleaned up strictly by its owning hop on Drop (including `0555`
/// read-only subdirectories).
struct HopStageDir(PathBuf);

impl HopStageDir {
    fn create() -> io::Result<Self> {
        static HOP_SEQ: AtomicU64 = AtomicU64::new(0);
        let pid = std::process::id();
        for _ in 0..128 {
            let seq = HOP_SEQ.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            let dir = PathBuf::from(format!("/var/tmp/olo-tracker-hop-{pid}-{seq}-{nanos}"));
            let mut b = sfs::DirBuilder::new();
            b.mode(0o700);
            match b.create(&dir) {
                Ok(()) => return Ok(Self(dir)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(io::ErrorKind::AlreadyExists, "could not allocate exclusive hop staging directory"))
    }
}

struct HopDirGuard(*mut libc::DIR);
impl Drop for HopDirGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `self.0` came from `libc::fdopendir` and is closed once here.
            unsafe { libc::closedir(self.0) };
        }
    }
}

fn remove_hop_dir_fd(dir: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c_dir = std::ffi::CString::new(dir.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL byte"))?;
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    // SAFETY: `c_dir` is NUL-terminated.
    let raw_fd = unsafe { libc::open(c_dir.as_ptr(), flags) };
    if raw_fd < 0 {
        let err = io::Error::last_os_error();
        return if err.kind() == io::ErrorKind::NotFound { Ok(()) } else { Err(err) };
    }
    // SAFETY: `raw_fd` is newly opened and owned here.
    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    empty_hop_dir_fd(fd)?;
    match sfs::remove_dir(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn empty_hop_dir_fd(fd: OwnedFd) -> io::Result<()> {
    use std::os::fd::IntoRawFd as _;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if (st.st_mode & libc::S_IFMT) != libc::S_IFDIR {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "hop stage is not a directory"));
    }
    let euid = unsafe { libc::geteuid() };
    if st.st_uid != euid {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "hop stage not owned by current user"));
    }
    if (st.st_mode & 0o700) != 0o700 {
        let _ = unsafe { libc::fchmod(fd.as_raw_fd(), (st.st_mode & 0o7777) | 0o700) };
    }
    let raw = fd.into_raw_fd();
    let dirp = unsafe { libc::fdopendir(raw) };
    if dirp.is_null() {
        let err = io::Error::last_os_error();
        let _ = unsafe { OwnedFd::from_raw_fd(raw) };
        return Err(err);
    }
    let guard = HopDirGuard(dirp);
    let dir_fd = unsafe { libc::dirfd(guard.0) };
    let mut names: Vec<std::ffi::CString> = Vec::new();
    loop {
        unsafe { *libc::__errno_location() = 0 };
        let ent = unsafe { libc::readdir(guard.0) };
        if ent.is_null() {
            let err_no = unsafe { *libc::__errno_location() };
            if err_no != 0 {
                return Err(io::Error::from_raw_os_error(err_no));
            }
            break;
        }
        let cname = unsafe { std::ffi::CStr::from_ptr((*ent).d_name.as_ptr()) };
        let bytes = cname.to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        names.push(cname.to_owned());
    }
    for name in names {
        if unsafe { libc::unlinkat(dir_fd, name.as_ptr(), 0) } == 0 {
            continue;
        }
        let err = io::Error::last_os_error();
        if matches!(err.raw_os_error(), Some(libc::EISDIR | libc::EPERM)) {
            let open_flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
            let child_raw = unsafe { libc::openat(dir_fd, name.as_ptr(), open_flags) };
            if child_raw < 0 {
                return Err(io::Error::last_os_error());
            }
            let child_fd = unsafe { OwnedFd::from_raw_fd(child_raw) };
            empty_hop_dir_fd(child_fd)?;
            if unsafe { libc::unlinkat(dir_fd, name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
                return Err(io::Error::last_os_error());
            }
        } else if err.kind() != io::ErrorKind::NotFound {
            return Err(err);
        }
    }
    Ok(())
}

impl Drop for HopStageDir {
    fn drop(&mut self) {
        let _ = remove_hop_dir_fd(&self.0);
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

    #[test]
    fn hop_stage_dirs_are_exclusive_0700_and_clean_up_readonly_subdirs() {
        let h1 = HopStageDir::create().unwrap();
        let h2 = HopStageDir::create().unwrap();
        assert_ne!(h1.0, h2.0, "concurrent host-to-host hops must never share an outer stage");
        assert_eq!(sfs::metadata(&h1.0).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(sfs::metadata(&h2.0).unwrap().permissions().mode() & 0o777, 0o700);
        let ro = h1.0.join("sub");
        sfs::create_dir(&ro).unwrap();
        sfs::write(ro.join("f.txt"), b"x").unwrap();
        sfs::set_permissions(&ro, sfs::Permissions::from_mode(0o555)).unwrap();
        let p1 = h1.0.clone();
        let p2 = h2.0.clone();
        drop(h1);
        drop(h2);
        assert!(!p1.exists(), "hop stage with 0555 subdir must clean up on drop");
        assert!(!p2.exists());
    }

    #[test]
    fn verify_local_source_manifest_protects_replaced_modified_and_new_descendants() {
        let dir = PathBuf::from(format!("/var/tmp/olo-ops-snap-{}", std::process::id()));
        let _ = sfs::remove_dir_all(&dir);
        sfs::create_dir_all(&dir).unwrap();

        // 1. Single file replaced atomically with same length: manifest refuses deletion.
        let src = dir.join("doc.txt");
        sfs::write(&src, b"v1").unwrap();
        let manifest_file = LocalSourceManifest::capture(&src).unwrap();
        let tmp = dir.join("doc.tmp");
        sfs::write(&tmp, b"v2").unwrap();
        sfs::rename(&tmp, &src).unwrap();
        assert!(manifest_file.remove_verified().is_err(), "replaced single file must not be deleted");
        assert_eq!(sfs::read(&src).unwrap(), b"v2");

        // 2. Directory with a modified descendant: refuses deleting the modified file and keeps dir.
        let folder = dir.join("folder");
        sfs::create_dir_all(folder.join("sub")).unwrap();
        sfs::write(folder.join("sub/a.txt"), b"orig").unwrap();
        let m_mod = LocalSourceManifest::capture(&folder).unwrap();
        sfs::write(folder.join("sub/a.txt"), b"edited-during-transfer").unwrap();
        assert!(m_mod.remove_verified().is_err(), "modified descendant must not be deleted");
        assert_eq!(sfs::read(folder.join("sub/a.txt")).unwrap(), b"edited-during-transfer");

        // 3. Directory with a newly added descendant after manifest capture: non-recursive
        //    rmdir refuses ENOTEMPTY and leaves the new file completely intact!
        let folder2 = dir.join("folder2");
        sfs::create_dir_all(folder2.join("sub")).unwrap();
        sfs::write(folder2.join("sub/old.txt"), b"old").unwrap();
        let m_new = LocalSourceManifest::capture(&folder2).unwrap();
        sfs::write(folder2.join("sub/brand-new.txt"), b"precious-new").unwrap();
        let err = m_new.remove_verified().unwrap_err();
        assert!(err.to_string().contains("New files appeared"), "{err}");
        assert_eq!(sfs::read(folder2.join("sub/brand-new.txt")).unwrap(), b"precious-new");

        // 4. Unchanged directory removes cleanly leaf-by-leaf + bottom-up rmdir.
        let folder3 = dir.join("folder3");
        sfs::create_dir_all(folder3.join("sub")).unwrap();
        sfs::write(folder3.join("sub/ok.txt"), b"ok").unwrap();
        let m_ok = LocalSourceManifest::capture(&folder3).unwrap();
        m_ok.remove_verified().unwrap();
        assert!(!folder3.exists());

        // 5. Same-fs no-replace rename refuses clobbering an existing file or broken symlink.
        let s1 = dir.join("m1.txt");
        let d1 = dir.join("m2.txt");
        sfs::write(&s1, b"src").unwrap();
        std::os::unix::fs::symlink(dir.join("nowhere"), &d1).unwrap();
        assert_eq!(local_rename_noreplace(&s1, &d1).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert!(s1.exists());

        let _ = sfs::remove_dir_all(&dir);
    }
}
