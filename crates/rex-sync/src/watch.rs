//! Zero-polling Linux directory watcher built on `inotify` + `poll`: used
//! both by `olo-rex-link sync watch` on a host and in-process by the Rex app
//! when watching a folder on this computer (`--to-host` and `--two-way` sync).
//!
//! Sleeps in `poll` with an infinite timeout while idle, waking only on kernel
//! inotify events or when `stop_fd` closes. Coalesces bursts over `settle_ms`
//! (default 500 ms) and caps continuous writes at `MAX_WAIT_MS` (5 s).

use std::collections::HashMap;
use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::Instant;

use crate::index::{Filter, is_part_name};

/// Default quiet period before emitting [`WatchEvent::Changed`].
pub const DEFAULT_SETTLE_MS: u64 = 500;

/// Longest time a pending change waits while continuous writes keep arriving.
pub const MAX_WAIT_MS: u64 = 5000;

/// What is watched in each directory: everything that changes its contents.
const MASK: u32 = libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_MODIFY
    | libc::IN_CLOSE_WRITE
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_ATTRIB
    | libc::IN_DELETE_SELF
    | libc::IN_MOVE_SELF
    | libc::IN_ONLYDIR;

/// Event delivered by [`watch_dir`] and [`spawn_watch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// Initial watches are installed on the tree; changes from now on will be seen.
    Watching,
    /// One or more filtered entries under the root changed and settled.
    Changed,
    /// The watched root itself was deleted, moved, or never existed as a directory.
    Gone,
    /// A watch could not be installed (e.g. `ENOSPC` / `max_user_watches`).
    Error(String),
}

struct OwnedFd(i32);

impl Drop for OwnedFd {
    fn drop(&mut self) {
        if self.0 >= 0 {
            // SAFETY: this wrapper exclusively owns the file descriptor.
            unsafe {
                libc::close(self.0);
            }
        }
    }
}

struct Watches {
    fd: i32,
    root: PathBuf,
    filter: Filter,
    /// Each watch descriptor's directory path relative to `root` (`""` for root).
    by_wd: HashMap<i32, String>,
}

impl Watches {
    /// Watch `rel` and every subdirectory under it permitted by `self.filter`.
    fn add_tree(&mut self, rel: &str) -> Result<(), String> {
        let at = if rel.is_empty() { self.root.clone() } else { self.root.join(rel) };
        let Ok(c) = CString::new(at.as_os_str().as_bytes()) else { return Ok(()) };
        // SAFETY: `self.fd` is a valid inotify fd and `c` is NUL-terminated.
        let wd = unsafe { libc::inotify_add_watch(self.fd, c.as_ptr(), MASK) };
        if wd < 0 {
            let e = io::Error::last_os_error();
            return match e.raw_os_error() {
                Some(libc::ENOSPC) => Err(format!("too many folders to watch here (fs.inotify.max_user_watches): {}", at.display())),
                _ => Ok(()),
            };
        }
        self.by_wd.insert(wd, rel.to_owned());
        if !self.filter.recursive {
            return Ok(());
        }
        let Ok(listing) = std::fs::read_dir(&at) else { return Ok(()) };
        for entry in listing.flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let Ok(name) = entry.file_name().into_string() else { continue };
            let child = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            if self.filter.allow_dir(&child) {
                self.add_tree(&child)?;
            }
        }
        Ok(())
    }
}

/// Watch `root` synchronously until `root` disappears, an error occurs,
/// `stop_fd` reaches EOF (pass `0` for stdin, or a pipe read end), or
/// `on_event` returns `false`.
pub fn watch_dir(root: &Path, filter: Filter, settle_ms: u64, stop_fd: i32, mut on_event: impl FnMut(WatchEvent) -> bool) {
    if !std::fs::symlink_metadata(root).is_ok_and(|m| m.is_dir()) {
        let _ = on_event(WatchEvent::Gone);
        return;
    }
    // SAFETY: creates a non-blocking, close-on-exec inotify descriptor.
    let raw_fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
    if raw_fd < 0 {
        let _ = on_event(WatchEvent::Error(format!("inotify: {}", io::Error::last_os_error())));
        return;
    }
    let fd = OwnedFd(raw_fd);
    let mut watches = Watches { fd: fd.0, root: root.to_path_buf(), filter, by_wd: HashMap::new() };
    if let Err(why) = watches.add_tree("") {
        let _ = on_event(WatchEvent::Error(why));
        return;
    }
    if !on_event(WatchEvent::Watching) {
        return;
    }

    let mut buf = vec![0u8; 64 << 10];
    let mut pending = false;
    let mut since: Option<Instant> = None;

    loop {
        let mut fds = [libc::pollfd { fd: fd.0, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: stop_fd, events: libc::POLLIN, revents: 0 }];
        if pending && since.is_none() {
            since = Some(Instant::now());
        }
        let left = since.map_or(u64::MAX, |s| MAX_WAIT_MS.saturating_sub(s.elapsed().as_millis() as u64));
        let timeout = if pending { settle_ms.min(left).min(i32::MAX as u64) as i32 } else { -1 };
        let nfds: libc::nfds_t = if stop_fd >= 0 { 2 } else { 1 };
        // SAFETY: `fds` has at least `nfds` valid `pollfd` entries.
        let n = if pending && left == 0 { 0 } else { unsafe { libc::poll(fds.as_mut_ptr(), nfds, timeout) } };
        if n < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            let _ = on_event(WatchEvent::Error(format!("poll: {}", io::Error::last_os_error())));
            return;
        }
        if n == 0 {
            pending = false;
            since = None;
            if !on_event(WatchEvent::Changed) {
                return;
            }
            continue;
        }
        if stop_fd >= 0 && fds[1].revents != 0 {
            let mut sink = [0u8; 256];
            // SAFETY: `stop_fd` is readable/hung-up and `sink` is a valid buffer.
            let got = unsafe { libc::read(stop_fd, sink.as_mut_ptr().cast(), sink.len()) };
            if got <= 0 {
                return;
            }
        }
        if fds[0].revents & libc::POLLIN == 0 {
            continue;
        }
        loop {
            // SAFETY: `fd.0` is a non-blocking inotify fd and `buf` is valid.
            let got = unsafe { libc::read(fd.0, buf.as_mut_ptr().cast(), buf.len()) };
            if got <= 0 {
                break;
            }
            let mut at = 0usize;
            while at + std::mem::size_of::<libc::inotify_event>() <= got as usize {
                // SAFETY: kernel writes whole `inotify_event` structs into `buf`.
                let event = unsafe { std::ptr::read_unaligned(buf.as_ptr().add(at).cast::<libc::inotify_event>()) };
                let name_at = at + std::mem::size_of::<libc::inotify_event>();
                let name_end = (name_at + event.len as usize).min(got as usize);
                let name = std::ffi::CStr::from_bytes_until_nul(&buf[name_at..name_end]).ok().and_then(|c| c.to_str().ok()).unwrap_or("").to_owned();
                at = name_end;

                if event.mask & libc::IN_Q_OVERFLOW != 0 {
                    pending = true;
                    continue;
                }
                let Some(dir) = watches.by_wd.get(&event.wd).cloned() else { continue };
                if event.mask & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) != 0 {
                    watches.by_wd.remove(&event.wd);
                    if dir.is_empty() {
                        let _ = on_event(WatchEvent::Gone);
                        return;
                    }
                    pending = true;
                    continue;
                }
                if event.mask & libc::IN_IGNORED != 0 {
                    watches.by_wd.remove(&event.wd);
                    continue;
                }
                if name.is_empty() || is_part_name(&name) {
                    continue;
                }
                let rel = if dir.is_empty() { name } else { format!("{dir}/{name}") };
                let is_dir = event.mask & libc::IN_ISDIR != 0;
                if is_dir && !watches.filter.allow_dir(&rel) {
                    continue;
                }
                if !is_dir && !watches.filter.matches_entry(&rel) {
                    continue;
                }
                if is_dir
                    && event.mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0
                    && let Err(why) = watches.add_tree(&rel)
                {
                    let _ = on_event(WatchEvent::Error(why));
                    return;
                }
                pending = true;
            }
        }
    }
}

/// Handle to a background directory watcher started by [`spawn_watch`].
/// Dropping the handle closes its stop pipe, waking `poll` immediately so the
/// background thread exits without polling or leaking descriptors.
pub struct WatchHandle {
    stop_write: Option<OwnedFd>,
    thread: Option<JoinHandle<()>>,
}

impl WatchHandle {
    /// Signal the watcher thread to stop immediately without waiting for it.
    pub fn signal_stop(&mut self) {
        self.stop_write = None;
    }

    /// Stop the watcher and wait for its thread to exit.
    pub fn stop(mut self) {
        self.stop_write = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        self.stop_write = None;
    }
}

/// Start [`watch_dir`] on a background OS thread with a clean pipe-based stop
/// channel. Zero CPU when idle.
pub fn spawn_watch(root: PathBuf, filter: Filter, settle_ms: u64, on_event: impl FnMut(WatchEvent) -> bool + Send + 'static) -> io::Result<WatchHandle> {
    let mut fds = [-1i32; 2];
    // SAFETY: creates a close-on-exec, non-blocking pipe pair.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let read_fd = OwnedFd(fds[0]);
    let write_fd = OwnedFd(fds[1]);
    let thread = std::thread::Builder::new().name("olo-rex-watch".into()).spawn(move || {
        let _keep_read = read_fd;
        watch_dir(&root, filter, settle_ms, _keep_read.0, on_event);
    })?;
    Ok(WatchHandle { stop_write: Some(write_fd), thread: Some(thread) })
}

/// Return the sorted relative directory paths watched under `root` for `filter` (for tests).
#[cfg(test)]
fn watched_dirs(root: &Path, filter: Filter) -> Vec<String> {
    let fd = OwnedFd(unsafe { libc::inotify_init1(libc::IN_CLOEXEC) });
    assert!(fd.0 >= 0);
    let mut w = Watches { fd: fd.0, root: root.to_path_buf(), filter, by_wd: HashMap::new() };
    w.add_tree("").unwrap();
    let mut v: Vec<String> = w.by_wd.into_values().collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-sync-watch-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn watches_respect_exclude_and_shallow() {
        let root = scratch("dirs");
        for d in ["a/b", "target/debug", "c"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let filtered = Filter { exclude: vec!["target".into()], ..Filter::default() };
        assert_eq!(watched_dirs(&root, filtered), ["", "a", "a/b", "c"]);
        assert_eq!(watched_dirs(&root, Filter::shallow()), [""]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn spawn_watch_notifies_ignores_excluded_and_stops_cleanly() {
        let root = scratch("live");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let (tx, rx) = mpsc::channel();
        let filter = Filter { recursive: true, exclude: vec!["*.tmp".into(), "ignored".into()], ..Filter::default() };
        let handle = spawn_watch(root.clone(), filter, 100, move |ev| tx.send(ev).is_ok()).unwrap();

        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), WatchEvent::Watching);

        // Excluded file and excluded directory produce no Changed event.
        std::fs::write(root.join("skip.tmp"), b"x").unwrap();
        std::fs::create_dir_all(root.join("ignored")).unwrap();
        std::fs::write(root.join("ignored/a.txt"), b"x").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(250)).is_err());

        // Real file emits one Changed event.
        std::fs::write(root.join("sub/real.txt"), b"hello").unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), WatchEvent::Changed);

        // Stopping wakes poll and joins promptly.
        let t0 = Instant::now();
        handle.stop();
        assert!(t0.elapsed() < Duration::from_secs(1), "stop took {:?}", t0.elapsed());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shallow_watch_ignores_subdirectory_changes() {
        let root = scratch("shallow");
        std::fs::create_dir_all(root.join("nested")).unwrap();
        let (tx, rx) = mpsc::channel();
        let handle = spawn_watch(root.clone(), Filter::shallow(), 100, move |ev| tx.send(ev).is_ok()).unwrap();

        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), WatchEvent::Watching);
        std::fs::write(root.join("nested/deep.txt"), b"ignored").unwrap();
        std::fs::create_dir_all(root.join("another")).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(250)).is_err());

        std::fs::write(root.join("top.txt"), b"seen").unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), WatchEvent::Changed);

        handle.stop();
        std::fs::remove_dir_all(root).unwrap();
    }
}
