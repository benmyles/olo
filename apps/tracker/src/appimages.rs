//! AppImages shown with their own icons (Ben, 2026-11-09: "if an appimage
//! has an icon in it for the app show that icon in the tracker"): the one
//! in its image, taken out on a thread of its own (`olo_appimage::info`:
//! once for each version of an AppImage, kept on disk and shared with
//! Deskbar), the windows drawn again as each comes; the kind's icon
//! meanwhile. Asked for as an item is drawn, kept by its path, size and
//! time.

use olo_sync::Mutex;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{OnceLock, mpsc},
    time::SystemTime,
};

use crate::fs::{Entry, Kind};

type Version = (u64, Option<SystemTime>);

enum Slot {
    Loading(Version),
    Ready(Version, Option<&'static str>),
}

static CACHE: Mutex<Option<HashMap<PathBuf, Slot>>> = Mutex::new(None);

/// What can be one: `.AppImage`, or a program with no extension (one
/// is an ELF program marked as an AppImage inside).
fn could_be(e: &Entry) -> bool {
    !e.is_dir
        && e.downloading.is_none()
        && match e.kind {
            Kind::Application => e.path.extension().is_some_and(|x| x.eq_ignore_ascii_case("appimage")),
            Kind::Executable => e.path.extension().is_none(),
            _ => false,
        }
}

fn worker() -> &'static Mutex<mpsc::Sender<(PathBuf, Version)>> {
    static TX: OnceLock<Mutex<mpsc::Sender<(PathBuf, Version)>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<(PathBuf, Version)>();
        let _ = std::thread::Builder::new().name("appimages".into()).spawn(move || {
            for (path, version) in rx {
                let icon = olo_appimage::info(&path).and_then(|a| a.icon).map(intern);
                let found = icon.is_some();
                if let Ok(mut c) = CACHE.lock() {
                    c.get_or_insert_with(HashMap::new).insert(path, Slot::Ready(version, icon));
                }
                if found {
                    crate::thumbs::wake();
                }
            }
        });
        Mutex::new(tx)
    })
}

/// The icon in AppImage `e`, if it is one and has one: None while it's
/// read (the windows are drawn again once it is).
pub fn icon(e: &Entry) -> Option<&'static str> {
    if !could_be(e) {
        return None;
    }
    let version = (e.size, e.modified);
    let mut c = CACHE.lock().ok()?;
    let map = c.get_or_insert_with(HashMap::new);
    match map.get(&e.path) {
        Some(Slot::Ready(v, icon)) if *v == version => return *icon,
        Some(Slot::Loading(v)) if *v == version => return None,
        _ => {}
    }
    // (A folder of programs, /usr/bin, is thousands: those that aren't
    // AppImages let go of after a while.)
    if map.len() > 4096 {
        map.retain(|_, s| !matches!(s, Slot::Ready(_, None)));
    }
    map.insert(e.path.clone(), Slot::Loading(version));
    drop(c);
    let _ = worker().lock().ok()?.send((e.path.clone(), version));
    None
}

/// An icon's path as the icon names Tracker passes about are: kept for
/// good (one for each version of each AppImage seen, a few).
fn intern(path: PathBuf) -> &'static str {
    static KEPT: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let s = path.to_string_lossy();
    let Ok(mut kept) = KEPT.lock() else { return Box::leak(s.into_owned().into_boxed_str()) };
    if let Some(k) = kept.iter().find(|k| **k == s) {
        return k;
    }
    let k: &'static str = Box::leak(s.into_owned().into_boxed_str());
    kept.push(k);
    k
}
