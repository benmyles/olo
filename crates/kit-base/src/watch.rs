//! Follow files and folders as they change, from the kernel's events
//! (inotify), never by looking again on a timer: whatever shows what is
//! on disk (a folder's items, a document, a state file another program
//! writes, the list of agent screens) changes the moment the disk does.
//! One thread serves the whole app; it sleeps in `poll` until the kernel
//! or a new watch wakes it, and gathers a burst of events (a big copy, an
//! editor's save) for a few milliseconds into one call.
//!
//! [`watch`] follows paths, files or folders, whether or not they exist
//! yet: a file saved by renaming another over it (as editors do), a
//! folder deleted and made again, one whose parent appears later. Each
//! path's folder is watched (for the path itself coming, going or being
//! replaced) and a folder's own items too. Dropping the [`Watch`] stops
//! it. Tracker keeps its own watcher (`apps/tracker/src/watch.rs`), which
//! also reads what changed for its windows.

use olo_sync::Mutex;
use std::{
    collections::{HashMap, HashSet},
    ffi::{CString, OsStr},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

/// How long a burst of events is gathered before it is handed on.
const GATHER: Duration = Duration::from_millis(25);

/// Events that say a folder's items (or a file's contents) changed.
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

type Changed = Arc<dyn Fn(&[PathBuf]) + Send + Sync>;

/// Paths being followed; stops when dropped.
pub struct Watch {
    id: u64,
}

impl Watch {
    /// Follow `paths` from now on instead (a file panel gone to another
    /// folder).
    pub fn set(&self, paths: impl IntoIterator<Item = PathBuf>) {
        let Some(s) = service() else { return };
        let mut st = s.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(w) = st.watchers.get_mut(&self.id) {
            w.paths = paths.into_iter().collect();
            w.there = w.paths.iter().map(|p| p.exists()).collect();
        }
        st.reconcile();
        drop(st);
        s.wake();
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let Some(s) = service() else { return };
        let mut st = s.state.lock().unwrap_or_else(|e| e.into_inner());
        st.watchers.remove(&self.id);
        st.reconcile();
    }
}

/// Call `changed` (on the watch thread: keep it short, such as a message
/// to the app) with those of `paths` that changed, whenever any do: a
/// file written, replaced, made or removed, a folder's items coming,
/// going or changing; for a folder, the items in it that changed come
/// too (after it), so a caller can pass over what isn't its business.
/// None if the kernel has no inotify to give.
pub fn watch(paths: impl IntoIterator<Item = PathBuf>, changed: impl Fn(&[PathBuf]) + Send + Sync + 'static) -> Option<Watch> {
    let s = service()?;
    let mut st = s.state.lock().unwrap_or_else(|e| e.into_inner());
    st.next += 1;
    let id = st.next;
    let paths: Vec<PathBuf> = paths.into_iter().collect();
    let there = paths.iter().map(|p| p.exists()).collect();
    st.watchers.insert(id, Watcher { paths, there, changed: Arc::new(changed) });
    st.reconcile();
    drop(st);
    s.wake();
    Some(Watch { id })
}

struct Watcher {
    paths: Vec<PathBuf>,
    /// Whether each was there when last looked (after folders came or
    /// went, one may have come or gone with them, unseen).
    there: Vec<bool>,
    changed: Changed,
}

struct State {
    fd: OwnedFd,
    next: u64,
    watchers: HashMap<u64, Watcher>,
    by_wd: HashMap<i32, PathBuf>,
    by_dir: HashMap<PathBuf, i32>,
}

impl State {
    /// The folders to watch for the paths followed: each path's nearest
    /// existing folder above it, and a folder path itself.
    fn wanted(&self) -> HashSet<PathBuf> {
        let mut want = HashSet::new();
        for w in self.watchers.values() {
            for p in &w.paths {
                if p.is_dir() {
                    want.insert(p.clone());
                }
                let mut up = p.parent();
                while let Some(d) = up {
                    if d.is_dir() {
                        want.insert(d.to_owned());
                        break;
                    }
                    up = d.parent();
                }
            }
        }
        want
    }

    fn reconcile(&mut self) {
        let want = self.wanted();
        let gone: Vec<PathBuf> = self.by_dir.keys().filter(|d| !want.contains(*d)).cloned().collect();
        for d in gone {
            if let Some(wd) = self.by_dir.remove(&d) {
                self.by_wd.remove(&wd);
                // SAFETY: valid fd; a stale wd just fails.
                unsafe { libc::inotify_rm_watch(self.fd.as_raw_fd(), wd) };
            }
        }
        for d in want {
            if self.by_dir.contains_key(&d) {
                continue;
            }
            let Ok(c) = CString::new(d.as_os_str().as_bytes()) else { continue };
            // SAFETY: valid fd and NUL-terminated path.
            let wd = unsafe { libc::inotify_add_watch(self.fd.as_raw_fd(), c.as_ptr(), MASK) };
            if wd < 0 {
                tracing::debug!("can't watch {}: {}", d.display(), std::io::Error::last_os_error());
                continue;
            }
            // Two paths naming one folder share its watch.
            if let Some(old) = self.by_wd.insert(wd, d.clone()) {
                self.by_dir.remove(&old);
            }
            self.by_dir.insert(d, wd);
        }
    }

    /// Read every event waiting: what was touched (a folder's item, or
    /// the folder itself), and whether folders came or went.
    fn read(&mut self, touched: &mut HashSet<PathBuf>) -> bool {
        let mut buf = [0u8; 16 * 1024];
        let mut structure = false;
        loop {
            // SAFETY: reading into our own buffer from a non-blocking fd.
            let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                return structure;
            }
            let mut at = 0;
            while at + std::mem::size_of::<libc::inotify_event>() <= n as usize {
                // SAFETY: the kernel writes whole events; read_unaligned
                // copes with where they fall.
                let ev: libc::inotify_event = unsafe { std::ptr::read_unaligned(buf[at..].as_ptr().cast()) };
                let name_at = at + std::mem::size_of::<libc::inotify_event>();
                let raw = &buf[name_at..name_at + ev.len as usize];
                let name = &raw[..raw.iter().position(|&b| b == 0).unwrap_or(raw.len())];
                at = name_at + ev.len as usize;
                if ev.mask & libc::IN_Q_OVERFLOW != 0 {
                    // Too much to say what: everything followed changed.
                    touched.extend(self.watchers.values().flat_map(|w| w.paths.iter().cloned()));
                    structure = true;
                    continue;
                }
                let Some(dir) = self.by_wd.get(&ev.wd).cloned() else { continue };
                if ev.mask & (libc::IN_IGNORED | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) != 0 {
                    // The folder itself went (or was swapped for another):
                    // watch what has its path now, if anything.
                    if ev.mask & libc::IN_IGNORED != 0 || ev.mask & libc::IN_MOVE_SELF != 0 {
                        self.by_wd.remove(&ev.wd);
                        self.by_dir.remove(&dir);
                        // SAFETY: valid fd; a gone wd just fails.
                        unsafe { libc::inotify_rm_watch(self.fd.as_raw_fd(), ev.wd) };
                    }
                    touched.insert(dir);
                    structure = true;
                    continue;
                }
                if ev.mask & (libc::IN_CREATE | libc::IN_DELETE | libc::IN_MOVED_FROM | libc::IN_MOVED_TO) != 0 && ev.mask & libc::IN_ISDIR != 0 {
                    structure = true;
                }
                touched.insert(dir.join(OsStr::from_bytes(name)));
                touched.insert(dir);
            }
        }
    }

    /// Each watcher's paths that changed: touched (written, replaced, an
    /// item in it changed), or, after folders came or went (`structure`),
    /// come or gone with them.
    fn calls(&mut self, touched: &HashSet<PathBuf>, structure: bool) -> Vec<(Changed, Vec<PathBuf>)> {
        let mut out = Vec::new();
        for w in self.watchers.values_mut() {
            let mut hit = Vec::new();
            for (p, there) in w.paths.iter().zip(w.there.iter_mut()) {
                let now = if structure { p.exists() } else { *there };
                if touched.contains(p) || now != *there {
                    hit.push(p.clone());
                }
                *there = now;
            }
            // The items that changed in folders followed.
            let items: Vec<PathBuf> = touched.iter().filter(|t| t.parent().is_some_and(|d| w.paths.iter().any(|p| p == d)) && !w.paths.contains(t)).cloned().collect();
            if !hit.is_empty() {
                hit.extend(items);
            }
            if !hit.is_empty() {
                out.push((w.changed.clone(), hit));
            }
        }
        out
    }
}

struct Service {
    state: Mutex<State>,
    /// Writing here wakes the thread (a watch added or changed).
    wake: OwnedFd,
}

impl Service {
    fn wake(&self) {
        // SAFETY: one byte to our own non-blocking pipe.
        unsafe { libc::write(self.wake.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
    }
}

fn service() -> Option<&'static Service> {
    static SERVICE: OnceLock<Option<&'static Service>> = OnceLock::new();
    *SERVICE.get_or_init(|| {
        // SAFETY: plain syscalls; the fds are owned from here on.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            tracing::warn!("inotify unavailable: {}", std::io::Error::last_os_error());
            return None;
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut pipe = [0; 2];
        if unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) } != 0 {
            return None;
        }
        let (woken, wake) = unsafe { (OwnedFd::from_raw_fd(pipe[0]), OwnedFd::from_raw_fd(pipe[1])) };
        let ino = fd.as_raw_fd();
        let s: &'static Service = Box::leak(Box::new(Service {
            state: Mutex::new(State { fd, next: 0, watchers: HashMap::new(), by_wd: HashMap::new(), by_dir: HashMap::new() }),
            wake,
        }));
        std::thread::Builder::new().name("kit-watch".into()).spawn(move || run(s, ino, woken)).ok()?;
        Some(s)
    })
}

fn run(s: &'static Service, ino: i32, woken: OwnedFd) {
    let mut touched = HashSet::new();
    let mut structure = false;
    let mut since: Option<Instant> = None;
    loop {
        // Idle: asleep until the kernel has an event (or a watch is
        // added). Gathering: asleep for the rest of the moment, the burst
        // read whole at its end (one wakeup, not one per event).
        let mut fds = [libc::pollfd { fd: woken.as_raw_fd(), events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: ino, events: libc::POLLIN, revents: 0 }];
        match since {
            // (Rounded up: a timeout of 0 for the last fraction of a
            // millisecond would spin.)
            Some(t) => unsafe { libc::poll(fds.as_mut_ptr(), 1, GATHER.saturating_sub(t.elapsed()).as_micros().div_ceil(1000) as i32) },
            // SAFETY: polling our own fds.
            None => unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) },
        };
        if fds[0].revents != 0 {
            let mut b = [0u8; 64];
            // SAFETY: draining our own pipe.
            while unsafe { libc::read(woken.as_raw_fd(), b.as_mut_ptr().cast(), b.len()) } > 0 {}
        }
        if since.is_none() && fds[1].revents != 0 {
            since = Some(Instant::now());
            continue;
        }
        if since.is_some_and(|t| t.elapsed() >= GATHER) {
            since = None;
            let calls = {
                let mut st = s.state.lock().unwrap_or_else(|e| e.into_inner());
                structure |= st.read(&mut touched);
                let structure = std::mem::take(&mut structure);
                if structure {
                    // Folders came or went: watch the ones there are now.
                    st.reconcile();
                }
                st.calls(&std::mem::take(&mut touched), structure)
            };
            for (changed, paths) in calls {
                changed(&paths);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn next(rx: &mpsc::Receiver<Vec<PathBuf>>) -> Vec<PathBuf> {
        rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default()
    }

    #[test]
    fn files_and_folders_as_they_change() {
        let dir = std::env::temp_dir().join(format!("olo-kit-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (file, folder) = (dir.join("state.json"), dir.join("later/sub"));
        let (tx, rx) = mpsc::channel();
        let w = watch([file.clone(), folder.clone()], move |p| tx.send(p.to_vec()).unwrap()).unwrap();
        // A file written, then replaced by a rename (an editor's save).
        std::fs::write(&file, "1").unwrap();
        assert_eq!(next(&rx), [file.clone()]);
        std::fs::write(dir.join("new"), "2").unwrap();
        std::fs::rename(dir.join("new"), &file).unwrap();
        assert_eq!(next(&rx), [file.clone()]);
        // A folder whose parent doesn't exist yet: made, then an item in it.
        std::fs::create_dir_all(&folder).unwrap();
        let seen = next(&rx);
        assert!(seen.contains(&folder), "{seen:?}");
        std::fs::write(folder.join("x"), "x").unwrap();
        assert_eq!(next(&rx), [folder.clone(), folder.join("x")]);
        // Swapped for an empty one (as Empty Trash does): still followed.
        std::fs::create_dir_all(dir.join("empty")).unwrap();
        std::fs::rename(&folder, dir.join("old")).unwrap();
        std::fs::rename(dir.join("empty"), &folder).unwrap();
        while rx.recv_timeout(Duration::from_millis(200)).is_ok() {}
        std::fs::write(folder.join("y"), "y").unwrap();
        assert_eq!(next(&rx), [folder.clone(), folder.join("y")]);
        // Dropped: nothing more.
        drop(w);
        std::fs::write(&file, "3").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
