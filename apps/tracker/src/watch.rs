//! Live updates: one thread watches every folder Tracker is showing with
//! inotify (plus the mount table, for volumes), and posts what changed to
//! the UI as it happens. Changed items are stat'ed here, off the UI
//! thread, so windows just splice them in. Bursts are gathered for a few
//! milliseconds so a big copy redraws a handful of times, not per file.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    ffi::{CString, OsStr},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

use crate::fs::{self, Entry};

/// What a watched folder saw.
#[derive(Debug, Clone)]
pub enum Change {
    /// `path` (in `dir`) appeared or changed (`Some`), or is gone (`None`).
    Item { dir: PathBuf, path: PathBuf, entry: Option<Entry> },
    /// Too much happened to say exactly what, or the folder itself was
    /// deleted or moved: list it again.
    Rescan(PathBuf),
    /// Something was mounted or unmounted.
    Mounts,
}

/// How long to gather events before posting them. Short enough to feel
/// instant, long enough that a burst arrives as one batch.
const GATHER: Duration = Duration::from_millis(30);

pub struct Watcher {
    tx: mpsc::Sender<BTreeSet<PathBuf>>,
    wake: OwnedFd,
    current: BTreeSet<PathBuf>,
}

impl Watcher {
    /// Start watching; batches go to `send`.
    pub fn new(send: impl Fn(Vec<Change>) + Send + 'static) -> Option<Watcher> {
        // SAFETY: plain syscalls; the fds are owned from here on.
        let inotify = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if inotify < 0 {
            tracing::warn!("inotify unavailable: {}", std::io::Error::last_os_error());
            return None;
        }
        let inotify = unsafe { OwnedFd::from_raw_fd(inotify) };
        let mut pipe = [0; 2];
        if unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) } != 0 {
            return None;
        }
        let (rx_end, wake) = unsafe { (OwnedFd::from_raw_fd(pipe[0]), OwnedFd::from_raw_fd(pipe[1])) };
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("watch".into())
            .spawn(move || run(inotify, rx_end, rx, send))
            .ok()?;
        Some(Watcher { tx, wake, current: BTreeSet::new() })
    }

    /// Watch exactly `paths` from now on. Cheap when nothing changed, so
    /// it can be called every frame.
    pub fn set(&mut self, paths: BTreeSet<PathBuf>) {
        if paths == self.current {
            return;
        }
        self.current = paths.clone();
        if self.tx.send(paths).is_ok() {
            // SAFETY: writing one byte to our own non-blocking pipe.
            unsafe { libc::write(self.wake.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
        }
    }
}

const MASK: u32 = libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_MODIFY
    | libc::IN_ATTRIB
    | libc::IN_CLOSE_WRITE
    | libc::IN_DELETE_SELF
    | libc::IN_MOVE_SELF
    | libc::IN_ONLYDIR
    | libc::IN_EXCL_UNLINK;

struct State {
    fd: OwnedFd,
    /// The folders wanted watched, whether or not they are now.
    want: BTreeSet<PathBuf>,
    by_wd: HashMap<i32, PathBuf>,
    by_path: HashMap<PathBuf, i32>,
    /// Items touched since the batch started, by folder.
    items: BTreeSet<(PathBuf, PathBuf)>,
    rescan: BTreeSet<PathBuf>,
    mounts: bool,
    since: Option<Instant>,
}

impl State {
    fn add(&mut self, path: &Path) {
        let Ok(c) = CString::new(path.as_os_str().as_bytes()) else { return };
        // SAFETY: valid fd and NUL-terminated path.
        let wd = unsafe { libc::inotify_add_watch(self.fd.as_raw_fd(), c.as_ptr(), MASK) };
        if wd < 0 {
            tracing::debug!("can't watch {}: {}", path.display(), std::io::Error::last_os_error());
            return;
        }
        self.by_wd.insert(wd, path.to_owned());
        self.by_path.insert(path.to_owned(), wd);
    }

    fn reconcile(&mut self, want: &BTreeSet<PathBuf>) {
        self.want = want.clone();
        let gone: Vec<PathBuf> = self.by_path.keys().filter(|p| !want.contains(*p)).cloned().collect();
        for p in gone {
            if let Some(wd) = self.by_path.remove(&p) {
                // Two paths can name one folder (and so one watch): only
                // drop it when no other wanted path uses it.
                if !self.by_path.values().any(|&w| w == wd) {
                    // SAFETY: valid fd; a stale wd just fails.
                    unsafe { libc::inotify_rm_watch(self.fd.as_raw_fd(), wd) };
                    self.by_wd.remove(&wd);
                }
            }
        }
        for p in want {
            if !self.by_path.contains_key(p) {
                self.add(p);
            }
        }
    }

    fn touch(&mut self) {
        self.since.get_or_insert_with(Instant::now);
    }

    /// Read and sort out every event waiting on the inotify fd.
    fn read_events(&mut self) {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            // SAFETY: reading into our own buffer.
            let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                return;
            }
            let mut at = 0;
            while at + std::mem::size_of::<libc::inotify_event>() <= n as usize {
                // SAFETY: the kernel writes whole, aligned-enough events;
                // read_unaligned copes either way.
                let ev: libc::inotify_event = unsafe { std::ptr::read_unaligned(buf[at..].as_ptr().cast()) };
                let name_at = at + std::mem::size_of::<libc::inotify_event>();
                let raw = &buf[name_at..name_at + ev.len as usize];
                let name = &raw[..raw.iter().position(|&b| b == 0).unwrap_or(raw.len())];
                at = name_at + ev.len as usize;
                self.event(ev.wd, ev.mask, OsStr::from_bytes(name));
            }
        }
    }

    fn event(&mut self, wd: i32, mask: u32, name: &OsStr) {
        if mask & libc::IN_Q_OVERFLOW != 0 {
            self.rescan.extend(self.by_path.keys().cloned());
            self.touch();
            return;
        }
        if mask & libc::IN_IGNORED != 0 {
            // The watch is gone (folder deleted, or we removed it): one
            // made again in its place is watched again.
            if let Some(p) = self.by_wd.remove(&wd) {
                self.by_path.retain(|_, w| *w != wd);
                if self.want.contains(&p) {
                    self.add(&p);
                }
                self.rescan.insert(p);
                self.touch();
            }
            return;
        }
        let Some(dir) = self.by_wd.get(&wd).cloned() else { return };
        if mask & libc::IN_MOVE_SELF != 0 {
            // Moved away (renamed, or swapped for an empty one as Empty
            // Trash does): the watch went with it; watch what has the
            // path now.
            let paths: Vec<PathBuf> = self.by_path.iter().filter(|(_, w)| **w == wd).map(|(p, _)| p.clone()).collect();
            // SAFETY: valid fd; its IN_IGNORED then finds no folder.
            unsafe { libc::inotify_rm_watch(self.fd.as_raw_fd(), wd) };
            self.by_wd.remove(&wd);
            self.by_path.retain(|_, w| *w != wd);
            for p in paths {
                self.add(&p);
                self.rescan.insert(p);
            }
        } else if mask & libc::IN_DELETE_SELF != 0 {
            self.rescan.insert(dir);
        } else if !name.is_empty() {
            // Every path naming this folder (a symlinked folder and its
            // target share one watch) hears about it.
            let dirs: Vec<PathBuf> = self.by_path.iter().filter(|(_, w)| **w == wd).map(|(p, _)| p.clone()).collect();
            for d in dirs {
                let path = d.join(name);
                self.items.insert((d, path));
            }
        } else {
            return;
        }
        self.touch();
    }

    fn flush(&mut self, send: &impl Fn(Vec<Change>)) {
        self.since = None;
        let mut out = Vec::new();
        if std::mem::take(&mut self.mounts) {
            out.push(Change::Mounts);
        }
        let rescan = std::mem::take(&mut self.rescan);
        for (dir, path) in std::mem::take(&mut self.items) {
            if rescan.contains(&dir) {
                continue;
            }
            out.push(Change::Item { entry: fs::entry(&path), dir, path });
        }
        out.extend(rescan.into_iter().map(Change::Rescan));
        if !out.is_empty() {
            send(out);
        }
    }
}

fn run(fd: OwnedFd, wake: OwnedFd, rx: mpsc::Receiver<BTreeSet<PathBuf>>, send: impl Fn(Vec<Change>)) {
    let mut s = State {
        fd,
        want: BTreeSet::new(),
        by_wd: HashMap::new(),
        by_path: HashMap::new(),
        items: BTreeSet::new(),
        rescan: BTreeSet::new(),
        mounts: false,
        since: None,
    };
    // The mount table polls "priority" whenever something is (un)mounted.
    let mounts = std::fs::File::open("/proc/self/mountinfo").ok();
    let mut wanted: HashSet<PathBuf> = HashSet::new();
    loop {
        let timeout = match s.since {
            Some(t) => GATHER.saturating_sub(t.elapsed()).as_millis() as i32,
            None => -1,
        };
        let mut fds = [
            libc::pollfd { fd: s.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: wake.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: mounts.as_ref().map_or(-1, |f| f.as_raw_fd()), events: libc::POLLPRI, revents: 0 },
        ];
        // SAFETY: polling our own fds.
        let r = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout) };
        if r < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return;
        }
        if fds[1].revents != 0 {
            let mut b = [0u8; 64];
            // SAFETY: draining our own pipe.
            let n = unsafe { libc::read(wake.as_raw_fd(), b.as_mut_ptr().cast(), b.len()) };
            if n == 0 {
                return; // the Watcher was dropped
            }
            let mut latest = None;
            loop {
                match rx.try_recv() {
                    Ok(set) => latest = Some(set),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return,
                }
            }
            if let Some(set) = latest {
                s.reconcile(&set);
                // A folder whose watch died (deleted) and was asked for
                // again gets re-added above.
                wanted = set.into_iter().collect();
            }
        }
        if fds[0].revents != 0 {
            s.read_events();
            // Folders that vanished and came back (a `git checkout`, an
            // editor's save-by-rename of a folder) are watched again.
            let missing: Vec<PathBuf> = wanted.iter().filter(|p| !s.by_path.contains_key(*p)).cloned().collect();
            for p in missing {
                if p.is_dir() {
                    s.add(&p);
                }
            }
        }
        if fds[2].revents != 0 {
            if let Some(f) = &mounts {
                // Reading the table to the end re-arms the notification.
                let _ = std::io::copy(&mut &*f, &mut std::io::sink());
                let _ = std::io::Seek::rewind(&mut &*f);
            }
            s.mounts = true;
            s.touch();
        }
        if s.since.is_some_and(|t| t.elapsed() >= GATHER) {
            s.flush(&send);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(rx: &mpsc::Receiver<Vec<Change>>, pred: impl Fn(&Change) -> bool) -> Change {
        let deadline = Instant::now() + Duration::from_secs(3);
        while let Ok(batch) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            if let Some(c) = batch.into_iter().find(|c| pred(c)) {
                return c;
            }
        }
        panic!("no matching change");
    }

    #[test]
    fn a_folder_swapped_for_another_is_still_followed() {
        let root = std::env::temp_dir().join(format!("tracker-watch-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (dir, empty) = (root.join("files"), root.join("empty"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::write(dir.join("old.txt"), "x").unwrap();
        let (tx, rx) = mpsc::channel();
        let mut w = Watcher::new(move |b| {
            let _ = tx.send(b);
        })
        .unwrap();
        w.set(BTreeSet::from([dir.clone()]));
        std::thread::sleep(Duration::from_millis(50));
        // Swapped at once for an empty one, as Empty Trash does.
        std::fs::rename(&dir, root.join("gone")).unwrap();
        std::fs::rename(&empty, &dir).unwrap();
        wait(&rx, |c| matches!(c, Change::Rescan(d) if *d == dir));
        // What goes into the new one is seen.
        let new = dir.join("new.txt");
        std::fs::write(&new, "y").unwrap();
        wait(&rx, |c| matches!(c, Change::Item { path, entry: Some(_), .. } if *path == new));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sees_changes() {
        let dir = std::env::temp_dir().join(format!("tracker-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, rx) = mpsc::channel();
        let mut w = Watcher::new(move |b| {
            let _ = tx.send(b);
        })
        .unwrap();
        w.set(BTreeSet::from([dir.clone()]));
        std::thread::sleep(Duration::from_millis(50));

        let a = dir.join("a.txt");
        std::fs::write(&a, b"hello").unwrap();
        match wait(&rx, |c| matches!(c, Change::Item { path, entry: Some(e), .. } if *path == a && e.size == 5)) {
            Change::Item { dir: d, .. } => assert_eq!(d, dir),
            _ => unreachable!(),
        }
        let b = dir.join("b.txt");
        std::fs::rename(&a, &b).unwrap();
        wait(&rx, |c| matches!(c, Change::Item { path, entry: None, .. } if *path == a));
        std::fs::remove_file(&b).unwrap();
        wait(&rx, |c| matches!(c, Change::Item { path, entry: None, .. } if *path == b));
        std::fs::remove_dir(&dir).unwrap();
        wait(&rx, |c| matches!(c, Change::Rescan(p) if *p == dir));
    }
}
