//! File operations — copy, move, duplicate, move to Trash, restore from
//! the Trash, empty Trash —
//! on worker threads, with live progress and cancellation, so Tracker never
//! blocks however big the job (see "The responsiveness rule").
//!
//! Copies use `copy_file_range` in chunks: zero-copy in the kernel (and
//! reflinks on btrfs) while still reporting progress and checking for Stop
//! between chunks. Moves on one file system are renames; across file
//! systems they copy and then remove the source.

use std::{
    fs::{self as sfs, File},
    io::{self, Read, Write},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
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
            *c = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        }
    }
}

pub struct Job {
    pub id: u64,
    /// "Copying 3 items to “Documents”".
    pub title: String,
    pub progress: Arc<Progress>,
    pub cancel: Arc<AtomicBool>,
    pub started: Instant,
}

/// What a finished job did.
#[derive(Debug, Default)]
pub struct Outcome {
    /// New items (copies, moved items at their new paths).
    pub created: Vec<PathBuf>,
    pub error: Option<String>,
    pub cancelled: bool,
}

fn title(kind: Kind, sources: &[PathBuf], dest: Option<&Path>) -> String {
    let what = match sources {
        [one] => format!("“{}”", one.file_name().unwrap_or_default().to_string_lossy()),
        many => format!("{} items", many.len()),
    };
    let to = dest.map(|d| format!(" to “{}”", fs::display_name(d))).unwrap_or_default();
    match kind {
        Kind::Copy => format!("Copying {what}{to}"),
        Kind::Move => format!("Moving {what}{to}"),
        Kind::Duplicate => format!("Duplicating {what}"),
        Kind::Trash => format!("Moving {what} to the Trash"),
        Kind::Restore => format!("Restoring {what}"),
        Kind::EmptyTrash => "Emptying the Trash".into(),
    }
}

/// Start a job on a worker thread; `done` runs on that thread when it ends.
pub fn start(
    id: u64,
    kind: Kind,
    sources: Vec<PathBuf>,
    dest: Option<PathBuf>,
    done: impl FnOnce(Outcome) + Send + 'static,
) -> Job {
    let job = Job {
        id,
        title: title(kind, &sources, dest.as_deref()),
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
            let r = w.run(kind, &sources, dest.as_deref(), &mut out.created);
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
    io::Error::new(e.kind(), format!("“{}”: {e}", p.file_name().unwrap_or_default().to_string_lossy()))
}

impl Worker<'_> {
    fn check(&self) -> io::Result<()> {
        if self.cancel.load(Ordering::Relaxed) { Err(cancelled()) } else { Ok(()) }
    }

    fn run(&self, kind: Kind, sources: &[PathBuf], dest: Option<&Path>, created: &mut Vec<PathBuf>) -> io::Result<()> {
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
                    created.push(target);
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
                    self.p.done_items.fetch_add(1, Ordering::Relaxed);
                }
            }
            Kind::Restore => {
                self.p.total_items.store(sources.len() as u64, Ordering::Relaxed);
                self.p.counting.store(false, Ordering::Relaxed);
                for s in sources {
                    self.check()?;
                    self.p.set_current(s);
                    let (origin, info) = fs::trash_origin(s)?;
                    let dir = origin.parent().ok_or_else(|| io::Error::other("no folder to restore into"))?;
                    // Its folder may have gone too; something new in its
                    // place keeps its name and this one gets a number.
                    sfs::create_dir_all(dir).map_err(|e| context(e, dir))?;
                    let target = fs::unique(dir, &origin.file_name().unwrap_or_default().to_string_lossy());
                    self.move_one(s, &target).map_err(|e| context(e, s))?;
                    let _ = sfs::remove_file(&info);
                    created.push(target);
                    self.p.done_items.fetch_add(1, Ordering::Relaxed);
                }
            }
            Kind::EmptyTrash => {
                let files = fs::trash_files();
                let items: Vec<PathBuf> = sfs::read_dir(&files)?.flatten().map(|e| e.path()).collect();
                for i in &items {
                    self.count_items(i)?;
                }
                self.p.counting.store(false, Ordering::Relaxed);
                for i in &items {
                    self.remove(i).map_err(|e| context(e, i))?;
                    if let Some(name) = i.file_name() {
                        let mut info = name.to_os_string();
                        info.push(".trashinfo");
                        let _ = sfs::remove_file(fs::trash_dir().join("info").join(info));
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
                let n = unsafe {
                    libc::copy_file_range(src.as_raw_fd(), std::ptr::null_mut(), dst.as_raw_fd(), std::ptr::null_mut(), CHUNK, 0)
                };
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

    fn remove_quietly(&self, p: &Path) -> io::Result<()> {
        if sfs::symlink_metadata(p)?.is_dir() { sfs::remove_dir_all(p) } else { sfs::remove_file(p) }
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

/// The line under the bar: the current item, how many items, time left.
pub fn detail(job: &Job) -> String {
    let p = &job.progress;
    let current = p.current.lock().map(|c| c.clone()).unwrap_or_default();
    let (tb, ti, di) = (
        p.total_bytes.load(Ordering::Relaxed),
        p.total_items.load(Ordering::Relaxed),
        p.done_items.load(Ordering::Relaxed),
    );
    let mut parts = vec![current];
    if p.counting.load(Ordering::Relaxed) {
        parts.push(format!("{} found so far", plural(ti, "item")));
    } else {
        if tb > 0 && ti > 1 {
            parts.push(format!("{} of {}", count(di.min(ti)), plural(ti, "item")));
        }
        // Estimate once there is enough to go on.
        let f = p.fraction() as f64;
        let elapsed = job.started.elapsed().as_secs_f64();
        if f > 0.02 && elapsed > 2.0 {
            let left = elapsed * (1.0 - f) / f;
            parts.push(if left < 60.0 {
                format!("about {} s left", left.ceil().max(1.0))
            } else {
                format!("about {} min left", (left / 60.0).ceil())
            });
        }
    }
    parts.retain(|s| !s.is_empty());
    parts.join(" · ")
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
        start(1, kind, sources, dest, move |o| tx.send(o).unwrap());
        rx.recv().unwrap()
    }

    #[test]
    fn copy_move_duplicate() {
        let dir = std::env::temp_dir().join(format!("tabula-ops-{}", std::process::id()));
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
        let dir = std::env::temp_dir().join(format!("tabula-ops-r-{}", std::process::id()));
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
        let dir = std::env::temp_dir().join(format!("tabula-ops-c-{}", std::process::id()));
        let _ = sfs::remove_dir_all(&dir);
        sfs::create_dir_all(dir.join("dst")).unwrap();
        sfs::write(dir.join("f.bin"), vec![1u8; 1 << 20]).unwrap();
        let (tx, rx) = mpsc::channel();
        let job = start(1, Kind::Copy, vec![dir.join("f.bin")], Some(dir.join("dst")), move |o| tx.send(o).unwrap());
        job.cancel.store(true, Ordering::Relaxed);
        let o = rx.recv().unwrap();
        // Either it finished before the cancel landed, or nothing is left behind.
        if o.cancelled {
            assert!(!dir.join("dst/f.bin").exists());
        }
        let _ = sfs::remove_dir_all(&dir);
    }
}
