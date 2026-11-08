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

use olo_protocol::diskbackup::{Reply, Request, Settings, State, Target};
use olo_ui::{
    Proxy,
    gpui::{App, Global},
};

use crate::Msg;

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

impl Global for Marks {}

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
pub fn backed_up(cx: &App, path: &Path) -> bool {
    let (Some(m), Some(dir), Some(name)) = (cx.try_global::<Marks>(), path.parent(), path.file_name().and_then(|n| n.to_str())) else { return false };
    m.dirs.get(dir).is_some_and(|names| names.contains(name))
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
pub fn followed(cx: &mut App, shown: &std::collections::BTreeSet<PathBuf>, proxy: &Proxy, on: bool, last_ok: Option<i64>) -> bool {
    let m = cx.default_global::<Marks>();
    if (on, last_ok) == (m.on, m.last_ok) {
        return false;
    }
    m.on = on;
    m.last_ok = last_ok;
    ON.store(on, std::sync::atomic::Ordering::Relaxed);
    if !on {
        m.dirs.clear();
        return true;
    }
    for dir in shown {
        ask(cx, dir, proxy);
    }
    false
}

/// The folders shown now: those no longer shown forgotten, new ones asked.
pub fn shown(cx: &mut App, now: &std::collections::BTreeSet<PathBuf>, proxy: &Proxy) {
    let m = cx.default_global::<Marks>();
    m.dirs.retain(|d, _| now.contains(d));
    let new: Vec<PathBuf> = now.iter().filter(|d| !m.dirs.contains_key(*d) && !m.asking.contains(*d)).cloned().collect();
    for dir in new {
        ask(cx, &dir, proxy);
    }
}

/// `dir` changed: asked again.
pub fn changed(cx: &mut App, dir: &Path, proxy: &Proxy) {
    if cx.try_global::<Marks>().is_some_and(|m| m.dirs.contains_key(dir) || m.asking.contains(dir)) {
        ask(cx, dir, proxy);
    }
}

/// Ask the service about `dir`, on a thread of its own (once at a time a
/// folder: asked while it's being answered, asked again after).
fn ask(cx: &mut App, dir: &Path, proxy: &Proxy) {
    let m = cx.default_global::<Marks>();
    if !m.on {
        return;
    }
    if !m.asking.insert(dir.to_owned()) {
        m.again.insert(dir.to_owned());
        return;
    }
    let (dir, proxy) = (dir.to_owned(), proxy.clone());
    let _ = std::thread::Builder::new().name("tracker-marks".into()).spawn(move || {
        let names = match olo_protocol::diskbackup::call(&Request::Marks { dir: dir.display().to_string() }) {
            Ok(Reply::Marks { backed_up, .. }) => Some(backed_up.into_iter().collect()),
            _ => None,
        };
        proxy.send(Msg::Marks(dir, names));
    });
}

/// The service answered about `dir`: whether anything shown changed.
pub fn answered(cx: &mut App, dir: PathBuf, names: Option<HashSet<String>>, proxy: &Proxy) -> bool {
    let m = cx.default_global::<Marks>();
    m.asking.remove(&dir);
    let again = m.again.remove(&dir);
    let names = names.unwrap_or_default();
    let changed = m.dirs.get(&dir) != Some(&names);
    m.dirs.insert(dir.clone(), names);
    if again {
        ask(cx, &dir, proxy);
    }
    changed
}
