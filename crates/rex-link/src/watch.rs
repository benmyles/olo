//! `olo-rex-link sync watch ROOT [--exclude PATTERN]… [--settle MS]`: says
//! `changed` on its output once something under ROOT has changed and things
//! have been quiet for a moment (`--settle`, 500 ms by default: a build
//! writing a hundred files says it once), or at most five seconds after the
//! first change while they go on (a log written to all the time never goes
//! quiet), so the app keeps a folder of the
//! host's in step by syncing again then (HACKING, Rex, "Folders kept in
//! step"). inotify on every folder of the tree, folders made later watched
//! as they come; nothing done while nothing changes (it sleeps in `poll`,
//! with a time-out only while a change settles). A queue that overflowed
//! is said as a change (the app syncs it all); ROOT gone, `gone`, and it
//! ends; more folders than the system lets it watch, `error: …`, and it
//! ends. It ends too when its input closes: the app let go of it.

use std::collections::HashMap;
use std::ffi::CString;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use olo_rex_sync::index::Filter;

const USAGE: &str = "usage: olo-rex-link sync watch ROOT [--exclude PATTERN]… [--settle MS]";

/// The longest a change waits to be said while more keep coming.
const AT_MOST_MS: u64 = 5000;

/// What is watched in each folder: what changes what is in it.
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

struct Watches {
    fd: i32,
    root: PathBuf,
    filter: Filter,
    /// Each watch's folder, relative to the root ("" the root).
    by_wd: HashMap<i32, String>,
}

impl Watches {
    /// `rel` and every folder under it watched, as the filter lets them.
    fn add_tree(&mut self, rel: &str) -> Result<(), String> {
        let at = if rel.is_empty() { self.root.clone() } else { self.root.join(rel) };
        let Ok(c) = CString::new(at.as_os_str().as_bytes()) else { return Ok(()) };
        // SAFETY: a valid descriptor and a NUL-terminated path.
        let wd = unsafe { libc::inotify_add_watch(self.fd, c.as_ptr(), MASK) };
        if wd < 0 {
            let e = std::io::Error::last_os_error();
            return match e.raw_os_error() {
                // (More folders than the system lets one watch.)
                Some(libc::ENOSPC) => Err(format!("too many folders to watch here (fs.inotify.max_user_watches): {}", at.display())),
                // (Gone, or not a folder, or not ours to read: nothing to watch.)
                _ => Ok(()),
            };
        }
        self.by_wd.insert(wd, rel.to_owned());
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

fn say(word: &str) -> bool {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{word}").and_then(|_| out.flush()).is_ok()
}

pub fn run(args: Vec<String>) -> ! {
    let fail = |why: &str| -> ! {
        eprintln!("olo-rex-link sync watch: {why}\n{USAGE}");
        std::process::exit(2)
    };
    let mut it = args.into_iter();
    let Some(root) = it.next() else { fail("say which folder") };
    let root = PathBuf::from(root);
    let root = match std::env::var_os("HOME") {
        Some(home) if root.is_relative() => PathBuf::from(home).join(root),
        _ => root,
    };
    let (mut filter, mut settle) = (Filter::default(), 500u64);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--exclude" => filter.exclude.push(it.next().unwrap_or_else(|| fail("--exclude takes a pattern"))),
            "--settle" => settle = it.next().and_then(|m| m.parse().ok()).unwrap_or_else(|| fail("--settle takes milliseconds")),
            other => fail(&format!("{other} isn't one of its options")),
        }
    }
    if !std::fs::symlink_metadata(&root).is_ok_and(|m| m.is_dir()) {
        say("gone");
        std::process::exit(1);
    }
    // SAFETY: plain system calls on descriptors this owns.
    let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
    if fd < 0 {
        say(&format!("error: inotify: {}", std::io::Error::last_os_error()));
        std::process::exit(1);
    }
    let mut watches = Watches { fd, root: root.clone(), filter, by_wd: HashMap::new() };
    if let Err(why) = watches.add_tree("") {
        say(&format!("error: {why}"));
        std::process::exit(1);
    }
    if !say("watching") {
        std::process::exit(0);
    }
    let mut buf = vec![0u8; 64 << 10];
    let mut pending = false;
    // (When the change waiting to be said came first.)
    let mut since: Option<std::time::Instant> = None;
    loop {
        let mut fds = [libc::pollfd { fd, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 }];
        // (Asleep until something happens; a time-out only while a change
        // settles, and no later than its longest wait.)
        if pending && since.is_none() {
            since = Some(std::time::Instant::now());
        }
        let left = since.map_or(u64::MAX, |s| AT_MOST_MS.saturating_sub(s.elapsed().as_millis() as u64));
        let timeout = if pending { settle.min(left) as i32 } else { -1 };
        let n = if pending && left == 0 { 0 } else { unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout) } };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            std::process::exit(1);
        }
        if n == 0 {
            // Quiet for the settling time, or waited long enough: said once.
            pending = false;
            since = None;
            if !say("changed") {
                std::process::exit(0);
            }
            continue;
        }
        if fds[1].revents != 0 {
            // (Its input: the app's end. Closed, it is done; anything else
            // on it is read and ignored.)
            let mut sink = [0u8; 256];
            let got = unsafe { libc::read(0, sink.as_mut_ptr().cast(), sink.len()) };
            if got <= 0 {
                std::process::exit(0);
            }
        }
        if fds[0].revents & libc::POLLIN == 0 {
            continue;
        }
        loop {
            let got = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            if got <= 0 {
                break;
            }
            let mut at = 0usize;
            while at + std::mem::size_of::<libc::inotify_event>() <= got as usize {
                // SAFETY: the kernel wrote whole events into `buf`.
                let event = unsafe { std::ptr::read_unaligned(buf.as_ptr().add(at).cast::<libc::inotify_event>()) };
                let name_at = at + std::mem::size_of::<libc::inotify_event>();
                let name = std::ffi::CStr::from_bytes_until_nul(&buf[name_at..name_at + event.len as usize]).ok().and_then(|c| c.to_str().ok()).unwrap_or("").to_owned();
                at = name_at + event.len as usize;
                if event.mask & libc::IN_Q_OVERFLOW != 0 {
                    pending = true;
                    continue;
                }
                let Some(dir) = watches.by_wd.get(&event.wd).cloned() else { continue };
                if event.mask & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) != 0 {
                    watches.by_wd.remove(&event.wd);
                    if dir.is_empty() {
                        say("gone");
                        std::process::exit(1);
                    }
                    pending = true;
                    continue;
                }
                if event.mask & libc::IN_IGNORED != 0 {
                    watches.by_wd.remove(&event.wd);
                    continue;
                }
                let rel = if dir.is_empty() { name.clone() } else { format!("{dir}/{name}") };
                if name.is_empty() || olo_rex_sync::index::is_part_name(&name) {
                    continue;
                }
                let is_dir = event.mask & libc::IN_ISDIR != 0;
                if is_dir && !watches.filter.allow_dir(&rel) {
                    continue;
                }
                if !is_dir && !watches.filter.matches_entry(&rel) {
                    continue;
                }
                // (A folder made, or moved in: watched with what is in it.)
                if is_dir
                    && event.mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0
                    && let Err(why) = watches.add_tree(&rel)
                {
                    say(&format!("error: {why}"));
                    std::process::exit(1);
                }
                pending = true;
            }
        }
    }
}

/// (For the module's tests: a watch's folders.)
#[cfg(test)]
fn watched(root: &std::path::Path, filter: Filter) -> Vec<String> {
    let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
    let mut w = Watches { fd, root: root.to_path_buf(), filter, by_wd: HashMap::new() };
    w.add_tree("").unwrap();
    let mut v: Vec<String> = w.by_wd.into_values().collect();
    v.sort();
    unsafe { libc::close(fd) };
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every folder of the tree is watched, but those the filter leaves out.
    #[test]
    fn every_folder_but_those_left_out_is_watched() {
        let root = PathBuf::from(format!("/var/tmp/olo-rex-link-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["a/b", "target/debug", "c"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let filter = Filter { exclude: vec!["target".into()], ..Filter::default() };
        assert_eq!(watched(&root, filter), ["", "a", "a/b", "c"]);
        std::fs::remove_dir_all(root).unwrap();
    }
}
