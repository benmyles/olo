//! DiskBackup's marks (Ben, 2026-11-07: "in tracker we should show a green
//! tick next to files if the most recent version is backed up"): for each
//! folder shown, which of its files the newest snapshot holds as they are
//! now, asked of DiskBackup's service (`Request::Marks`) off the UI thread:
//! when the folder is first shown, when it changes, and when a backup
//! finishes. Its icons wear a tick (`items::paint_mark`). Nothing is asked
//! while DiskBackup isn't set up, or its service isn't running.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use olo_kit::Proxy;
use olo_protocol::diskbackup::{Reply, Request, Settings, State, Target};
use olo_sync::Mutex;

use crate::ToHub;

#[derive(Default)]
pub struct Marks {
    /// DiskBackup set up and its service running.
    on: bool,
    /// Its last whole backup (a new one: every folder asked again).
    last_ok: Option<i64>,
    /// Each folder's files backed up as they are now, by name.
    dirs: HashMap<PathBuf, HashSet<String>>,
    /// Asked, and not answered yet; and changed meanwhile (ask again).
    asking: HashSet<PathBuf>,
    again: HashSet<PathBuf>,
}

/// (Every window's: each draws its marks on its own looper; the app's
/// looper keeps them.)
static MARKS: Mutex<Option<Marks>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Marks) -> R) -> R {
    let mut m = MARKS.lock().unwrap_or_else(|e| e.into_inner());
    f(m.get_or_insert_with(Marks::default))
}

/// DiskBackup is on (for the menus, which have no `App`).
static ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn on() -> bool {
    ON.load(std::sync::atomic::Ordering::Relaxed)
}

/// Its versions, in DiskBackup's window for them (Older Versions…).
pub fn older_versions(path: &Path) {
    let argv = ["olo-diskbackup".into(), "--versions-window".into(), path.as_os_str().to_owned()];
    if let Err(e) = olo_xdg::open::spawn(&argv) {
        tracing::warn!("tracker: olo-diskbackup --versions-window: {e}");
    }
}

/// `path`'s current version is backed up.
pub fn backed_up(path: &Path) -> bool {
    if !on() {
        return false;
    }
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else { return false };
    with(|m| m.dirs.get(dir).is_some_and(|names| names.contains(name)))
}

/// DiskBackup as it is now: whether it's on, its last whole backup.
pub fn state() -> (bool, Option<i64>) {
    let s = Settings::load();
    if s.target == Target::None {
        return (false, None);
    }
    let st = State::load();
    (st.pid != 0, st.last_ok)
}

/// DiskBackup went on or off, or finished a backup (or Tracker started):
/// every folder in `shown` asked again. Whether the marks changed.
pub fn followed(shown: &std::collections::BTreeSet<PathBuf>, proxy: &Proxy, on: bool, last_ok: Option<i64>) -> bool {
    let cleared = with(|m| {
        if (on, last_ok) == (m.on, m.last_ok) {
            return None;
        }
        m.on = on;
        m.last_ok = last_ok;
        ON.store(on, std::sync::atomic::Ordering::Relaxed);
        if !on {
            m.dirs.clear();
        }
        Some(!on)
    });
    match cleared {
        None => false,
        Some(true) => true,
        Some(false) => {
            for dir in shown {
                ask(dir, proxy);
            }
            false
        }
    }
}

/// The folders shown now: those no longer shown forgotten, new ones asked.
pub fn shown(now: &std::collections::BTreeSet<PathBuf>, proxy: &Proxy) {
    let new: Vec<PathBuf> = with(|m| {
        m.dirs.retain(|d, _| now.contains(d));
        now.iter().filter(|d| !m.dirs.contains_key(*d) && !m.asking.contains(*d)).cloned().collect()
    });
    for dir in new {
        ask(&dir, proxy);
    }
}

/// `dir` changed: asked again.
pub fn changed(dir: &Path, proxy: &Proxy) {
    if with(|m| m.dirs.contains_key(dir) || m.asking.contains(dir)) {
        ask(dir, proxy);
    }
}

/// Ask the service about `dir`, on a thread of its own (once at a time a
/// folder: asked while it's being answered, asked again after).
fn ask(dir: &Path, proxy: &Proxy) {
    let go = with(|m| {
        if !m.on {
            return false;
        }
        if !m.asking.insert(dir.to_owned()) {
            m.again.insert(dir.to_owned());
            return false;
        }
        true
    });
    if !go {
        return;
    }
    let (dir, proxy) = (dir.to_owned(), proxy.clone());
    let _ = std::thread::Builder::new().name("tracker-marks".into()).spawn(move || {
        let names = match olo_protocol::diskbackup::call(&Request::Marks { dir: dir.display().to_string() }) {
            Ok(Reply::Marks { backed_up, .. }) => Some(backed_up.into_iter().collect()),
            _ => None,
        };
        proxy.send(ToHub::Marks(dir, names));
    });
}

/// The service answered about `dir`: whether anything shown changed.
pub fn answered(dir: PathBuf, names: Option<HashSet<String>>, proxy: &Proxy) -> bool {
    let (again, changed) = with(|m| {
        m.asking.remove(&dir);
        let again = m.again.remove(&dir);
        let names = names.unwrap_or_default();
        let changed = m.dirs.get(&dir) != Some(&names);
        m.dirs.insert(dir.clone(), names);
        (again, changed)
    });
    if again {
        ask(&dir, proxy);
    }
    changed
}
