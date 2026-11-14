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
//!
//! What a change costs follows the change, not how much is followed (the
//! command palette follows every folder of a home, ~37,000): a folder
//! that comes or goes has the paths at and under it looked at again (the
//! paths are kept in order, so those are a range), each folder watched is
//! counted by the paths that want it, and a path's call is found from the
//! paths touched. A [`Watch`] grows and shrinks by what changed in it
//! ([`Watch::add`], [`Watch::remove`]); [`Watch::set`] looks only at what
//! differs from what it followed.

use olo_sync::Mutex;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::{CString, OsStr},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::{Path, PathBuf},
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
    /// folder). Only what differs from before is looked at.
    pub fn set(&self, paths: impl IntoIterator<Item = PathBuf>) {
        self.change(|st, id| {
            let new = Places::of(paths);
            let Some(w) = st.watchers.get_mut(&id) else { return };
            let old = std::mem::replace(&mut w.paths, new);
            let (old, new) = (&old, &st.watchers[&id].paths);
            let gone: Vec<PathBuf> = old.list.iter().filter(|p| !new.at.contains_key(*p)).cloned().collect();
            let came: Vec<PathBuf> = new.list.iter().filter(|p| !old.at.contains_key(*p)).cloned().collect();
            for p in &came {
                st.follow(p);
            }
            for p in &gone {
                st.unfollow(p);
            }
        });
    }

    /// Follow `paths` too.
    pub fn add(&self, paths: impl IntoIterator<Item = PathBuf>) {
        self.change(|st, id| {
            for p in paths {
                let Some(w) = st.watchers.get_mut(&id) else { return };
                if w.paths.push(&p) {
                    st.follow(&p);
                }
            }
        });
    }

    /// Follow `paths` no more.
    pub fn remove(&self, paths: impl IntoIterator<Item = PathBuf>) {
        self.change(|st, id| {
            for p in paths {
                let Some(w) = st.watchers.get_mut(&id) else { return };
                if w.paths.take(&p) {
                    st.unfollow(&p);
                }
            }
        });
    }

    fn change(&self, f: impl FnOnce(&mut State, u64)) {
        let Some(s) = service() else { return };
        let mut st = s.state.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut st, self.id);
        drop(st);
        s.wake();
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let Some(s) = service() else { return };
        let mut st = s.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(w) = st.watchers.remove(&self.id) {
            for p in &w.paths.list {
                st.unfollow(p);
            }
        }
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
    let paths = Places::of(paths);
    for p in &paths.list {
        st.follow(p);
    }
    st.watchers.insert(id, Watcher { paths, changed: Arc::new(changed) });
    drop(st);
    s.wake();
    Some(Watch { id })
}

/// A watcher's paths in the order given (what it's told comes in that
/// order), and each one's place in it.
#[derive(Default)]
struct Places {
    list: Vec<PathBuf>,
    at: HashMap<PathBuf, usize>,
}

impl Places {
    fn of(paths: impl IntoIterator<Item = PathBuf>) -> Places {
        let mut places = Places::default();
        for p in paths {
            places.push(&p);
        }
        places
    }

    /// False if it's there already.
    fn push(&mut self, p: &Path) -> bool {
        if self.at.contains_key(p) {
            return false;
        }
        self.at.insert(p.to_owned(), self.list.len());
        self.list.push(p.to_owned());
        true
    }

    /// False if it wasn't there.
    fn take(&mut self, p: &Path) -> bool {
        let Some(i) = self.at.remove(p) else { return false };
        self.list.swap_remove(i);
        if let Some(moved) = self.list.get(i) {
            self.at.insert(moved.clone(), i);
        }
        true
    }
}

struct Watcher {
    paths: Places,
    changed: Changed,
}

/// A path followed, by one watcher or more.
struct Followed {
    users: usize,
    /// Whether it was there when last looked.
    there: bool,
    /// The folders watched for it: itself, if it's a folder; the nearest
    /// one above it that exists (for it coming, going or being replaced).
    dirs: [Option<PathBuf>; 2],
}

/// Whether `p` is there, and the folders to watch for it.
fn look(p: &Path) -> (bool, [Option<PathBuf>; 2]) {
    let meta = std::fs::metadata(p);
    let own = meta.as_ref().is_ok_and(|m| m.is_dir()).then(|| p.to_owned());
    let mut up = p.parent();
    while let Some(d) = up {
        if d.is_dir() {
            break;
        }
        up = d.parent();
    }
    (meta.is_ok(), [own, up.map(Path::to_owned)])
}

/// A folder watched: its watch (None while it can't be: gone, or the
/// kernel refused), and how many paths want it.
struct Dir {
    wd: Option<i32>,
    wanted: usize,
}

struct State {
    fd: OwnedFd,
    next: u64,
    watchers: HashMap<u64, Watcher>,
    /// Every path followed, in order: what's under a folder is a range.
    paths: BTreeMap<PathBuf, Followed>,
    dirs: HashMap<PathBuf, Dir>,
    /// Each watch's folder: its names (two paths can name one folder,
    /// by a link, and share its watch).
    by_wd: HashMap<i32, Vec<PathBuf>>,
}

/// What a burst of events said.
#[derive(Default)]
struct Burst {
    /// What was touched: a folder's item, or the folder itself.
    touched: HashSet<PathBuf>,
    /// Folders that came or went (made, removed, moved in or out).
    moved: Vec<PathBuf>,
    /// The kernel lost count: everything may have changed.
    overflow: bool,
}

impl State {
    /// One more watcher follows `p`.
    fn follow(&mut self, p: &Path) {
        if let Some(f) = self.paths.get_mut(p) {
            f.users += 1;
            return;
        }
        let (there, dirs) = look(p);
        for d in dirs.iter().flatten() {
            self.want(d);
        }
        self.paths.insert(p.to_owned(), Followed { users: 1, there, dirs });
    }

    /// One less does.
    fn unfollow(&mut self, p: &Path) {
        let Some(f) = self.paths.get_mut(p) else { return };
        f.users -= 1;
        if f.users == 0
            && let Some(f) = self.paths.remove(p)
        {
            for d in f.dirs.iter().flatten() {
                self.unwant(d);
            }
        }
    }

    /// Looks at `p` again (a folder came or went at or above it): whether
    /// it's there changed.
    fn again(&mut self, p: &Path) -> bool {
        if !self.paths.contains_key(p) {
            return false;
        }
        let (there, dirs) = look(p);
        // (The new wanted before the old let go: a folder in both keeps
        // its watch, or has it made again if it was lost.)
        for d in dirs.iter().flatten() {
            self.want(d);
        }
        let Some(f) = self.paths.get_mut(p) else { return false };
        let old = std::mem::replace(&mut f.dirs, dirs);
        let flipped = std::mem::replace(&mut f.there, there) != there;
        for d in old.iter().flatten() {
            self.unwant(d);
        }
        flipped
    }

    /// Looks again at every path at or under `dir`; those whose being
    /// there changed go into `flipped`.
    fn again_under(&mut self, dir: &Path, flipped: &mut HashSet<PathBuf>) {
        let under: Vec<PathBuf> = self.paths.range(dir.to_owned()..).map(|(p, _)| p).take_while(|p| p.starts_with(dir)).cloned().collect();
        for p in under {
            if self.again(&p) {
                flipped.insert(p);
            }
        }
    }

    fn want(&mut self, d: &Path) {
        match self.dirs.get_mut(d) {
            Some(dir) => {
                dir.wanted += 1;
                if dir.wd.is_none() {
                    dir.wd = add_watch(&self.fd, d);
                    if let Some(wd) = dir.wd {
                        self.by_wd.entry(wd).or_default().push(d.to_owned());
                    }
                }
            }
            None => {
                let wd = add_watch(&self.fd, d);
                if let Some(wd) = wd {
                    self.by_wd.entry(wd).or_default().push(d.to_owned());
                }
                self.dirs.insert(d.to_owned(), Dir { wd, wanted: 1 });
            }
        }
    }

    fn unwant(&mut self, d: &Path) {
        let Some(dir) = self.dirs.get_mut(d) else { return };
        dir.wanted -= 1;
        if dir.wanted > 0 {
            return;
        }
        let wd = dir.wd;
        self.dirs.remove(d);
        if let Some(wd) = wd
            && let Some(names) = self.by_wd.get_mut(&wd)
        {
            names.retain(|n| n != d);
            if names.is_empty() {
                self.by_wd.remove(&wd);
                // SAFETY: valid fd; a stale wd just fails.
                unsafe { libc::inotify_rm_watch(self.fd.as_raw_fd(), wd) };
            }
        }
    }

    /// The watch `wd` is no more (its folder removed, or moved away):
    /// its folders are unwatched until looked at again.
    fn lost(&mut self, wd: i32) -> Vec<PathBuf> {
        let names = self.by_wd.remove(&wd).unwrap_or_default();
        for n in &names {
            if let Some(dir) = self.dirs.get_mut(n) {
                dir.wd = None;
            }
        }
        names
    }

    /// Read every event waiting into `burst`.
    fn read(&mut self, burst: &mut Burst) {
        let mut buf = [0u8; 16 * 1024];
        loop {
            // SAFETY: reading into our own buffer from a non-blocking fd.
            let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                return;
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
                    burst.overflow = true;
                    continue;
                }
                if ev.mask & (libc::IN_IGNORED | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) != 0 {
                    // The folder itself went (or was swapped for another):
                    // what has its path now, if anything, is looked at.
                    let names = if ev.mask & (libc::IN_IGNORED | libc::IN_MOVE_SELF) != 0 {
                        if ev.mask & libc::IN_MOVE_SELF != 0 {
                            // (Moved, its watch would follow it elsewhere.)
                            // SAFETY: valid fd; a gone wd just fails.
                            unsafe { libc::inotify_rm_watch(self.fd.as_raw_fd(), ev.wd) };
                        }
                        self.lost(ev.wd)
                    } else {
                        self.by_wd.get(&ev.wd).cloned().unwrap_or_default()
                    };
                    for n in names {
                        burst.touched.insert(n.clone());
                        burst.moved.push(n);
                    }
                    continue;
                }
                let Some(names) = self.by_wd.get(&ev.wd) else { continue };
                for dir in names {
                    let item = dir.join(OsStr::from_bytes(name));
                    if ev.mask & (libc::IN_CREATE | libc::IN_DELETE | libc::IN_MOVED_FROM | libc::IN_MOVED_TO) != 0 && ev.mask & libc::IN_ISDIR != 0 {
                        burst.moved.push(item.clone());
                    }
                    burst.touched.insert(item);
                    burst.touched.insert(dir.clone());
                }
            }
        }
    }

    /// What a burst changed, for each watcher: its paths touched (written,
    /// replaced, an item in it changed) or come or gone, in its order, and
    /// then the items that changed in its folders.
    fn calls(&mut self, burst: Burst) -> Vec<(Changed, Vec<PathBuf>)> {
        let Burst { mut touched, mut moved, overflow } = burst;
        let mut flipped = HashSet::new();
        if overflow {
            // Too much to say what: everything followed changed, and is
            // looked at again.
            let all: Vec<PathBuf> = self.paths.keys().cloned().collect();
            for p in &all {
                self.again(p);
            }
            touched.extend(all);
            moved.clear();
        }
        moved.sort();
        moved.dedup();
        // (A folder under another that moved is looked at with it.)
        moved.dedup_by(|b, a| b.starts_with(&*a));
        for m in &moved {
            self.again_under(m, &mut flipped);
        }
        // A file touched may have come or gone without a folder moving.
        for p in &touched {
            if let Some(f) = self.paths.get_mut(p) {
                let there = p.exists();
                if f.there != there {
                    f.there = there;
                    flipped.insert(p.clone());
                }
            }
        }
        let mut out = Vec::new();
        for w in self.watchers.values() {
            let mut hit: Vec<usize> = touched.iter().chain(&flipped).filter_map(|p| w.paths.at.get(p).copied()).collect();
            if hit.is_empty() {
                continue;
            }
            hit.sort_unstable();
            hit.dedup();
            let mut paths: Vec<PathBuf> = hit.into_iter().map(|i| w.paths.list[i].clone()).collect();
            // The items that changed in folders followed.
            paths.extend(touched.iter().filter(|t| t.parent().is_some_and(|d| w.paths.at.contains_key(d)) && !w.paths.at.contains_key(*t)).cloned());
            out.push((w.changed.clone(), paths));
        }
        out
    }
}

fn add_watch(fd: &OwnedFd, d: &Path) -> Option<i32> {
    let c = CString::new(d.as_os_str().as_bytes()).ok()?;
    // SAFETY: valid fd and NUL-terminated path.
    let wd = unsafe { libc::inotify_add_watch(fd.as_raw_fd(), c.as_ptr(), MASK) };
    if wd < 0 {
        tracing::debug!("can't watch {}: {}", d.display(), std::io::Error::last_os_error());
        return None;
    }
    Some(wd)
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
        let s: &'static Service =
            Box::leak(Box::new(Service { state: Mutex::new(State { fd, next: 0, watchers: HashMap::new(), paths: BTreeMap::new(), dirs: HashMap::new(), by_wd: HashMap::new() }), wake }));
        std::thread::Builder::new().name("kit-watch".into()).spawn(move || run(s, ino, woken)).ok()?;
        Some(s)
    })
}

fn run(s: &'static Service, ino: i32, woken: OwnedFd) {
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
                let mut burst = Burst::default();
                st.read(&mut burst);
                st.calls(burst)
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

    #[test]
    fn grown_shrunk_and_shared() {
        let dir = std::env::temp_dir().join(format!("olo-kit-watch-shared-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a")).unwrap();
        let (a, b, c) = (dir.join("a"), dir.join("b"), dir.join("c.txt"));
        let (tx1, rx1) = mpsc::channel();
        let (tx2, rx2) = mpsc::channel();
        let one = watch([a.clone()], move |p| tx1.send(p.to_vec()).unwrap()).unwrap();
        let two = watch([a.clone(), c.clone()], move |p| tx2.send(p.to_vec()).unwrap()).unwrap();
        // Both follow a: both hear of an item in it.
        std::fs::write(a.join("x"), "x").unwrap();
        assert_eq!(next(&rx1), [a.clone(), a.join("x")]);
        assert_eq!(next(&rx2), [a.clone(), a.join("x")]);
        // One gone, the other still hears.
        drop(one);
        std::fs::write(a.join("y"), "y").unwrap();
        assert_eq!(next(&rx2), [a.clone(), a.join("y")]);
        // Grown by a folder not there yet: made, it's heard of.
        two.add([b.clone()]);
        std::fs::create_dir(&b).unwrap();
        assert!(next(&rx2).contains(&b));
        // Shrunk: a's no more, c still is.
        two.remove([a.clone()]);
        std::fs::write(a.join("z"), "z").unwrap();
        std::fs::write(&c, "c").unwrap();
        assert_eq!(next(&rx2), [c.clone()]);
        // Set: b kept as it was, a back.
        two.set([b.clone(), a.clone()]);
        std::fs::write(b.join("w"), "w").unwrap();
        assert_eq!(next(&rx2), [b.clone(), b.join("w")]);
        // a removed and made again, with an item: still followed.
        std::fs::remove_dir_all(&a).unwrap();
        while rx2.recv_timeout(Duration::from_millis(200)).is_ok() {}
        std::fs::create_dir(&a).unwrap();
        while rx2.recv_timeout(Duration::from_millis(200)).is_ok() {}
        std::fs::write(a.join("v"), "v").unwrap();
        assert_eq!(next(&rx2), [a.clone(), a.join("v")]);
        drop(two);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn cpu_ms() -> f64 {
        // SAFETY: getrusage fills the struct it's given.
        let mut u: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
        let t = |v: libc::timeval| v.tv_sec as f64 * 1e3 + v.tv_usec as f64 / 1e3;
        t(u.ru_utime) + t(u.ru_stime)
    }

    /// What a folder made or removed costs with many followed, as the
    /// command palette follows every folder of a home (`--ignored
    /// --nocapture`; TMPDIR on disk: it makes 30,000 folders).
    #[test]
    #[ignore]
    fn a_change_among_many() {
        let dir = std::env::temp_dir().join(format!("olo-kit-watch-many-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut folders = vec![dir.clone()];
        for a in 0..30 {
            for b in 0..1000 {
                folders.push(dir.join(format!("a{a}/b{b}")));
            }
        }
        for f in &folders {
            std::fs::create_dir_all(f).unwrap();
        }
        let (tx, rx) = mpsc::channel();
        let t = cpu_ms();
        let w = watch(folders.clone(), move |p| tx.send(p.len()).unwrap()).unwrap();
        eprintln!("watching {} folders: {:.1} ms", folders.len(), cpu_ms() - t);
        let n = 20;
        let t = cpu_ms();
        for i in 0..n {
            let new = dir.join(format!("a{}/b{i}/new", i % 30));
            std::fs::create_dir(&new).unwrap();
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
            std::fs::remove_dir(&new).unwrap();
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        eprintln!("a folder made or removed: {:.2} ms", (cpu_ms() - t) / (2 * n) as f64);
        // As the palette does: its folders again, one more.
        let t = cpu_ms();
        for i in 0..n {
            let mut more = folders.clone();
            more.push(dir.join(format!("a0/b{i}/x")));
            w.set(more);
        }
        eprintln!("set again, one more: {:.2} ms", (cpu_ms() - t) / n as f64);
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
