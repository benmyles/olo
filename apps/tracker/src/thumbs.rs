//! Pictures made small (the column view's preview of a selected picture),
//! decoded and shrunk on a thread of their own, in the order asked for,
//! kept by the file (its path and when it last changed) and size; the
//! windows are drawn again as each comes.

use olo_sync::Mutex;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, mpsc},
    time::SystemTime,
};

use olo_kit::{Proxy, RenderImage};

type Key = (PathBuf, u32, Option<SystemTime>);

enum Slot {
    Loading,
    Ready(Option<Arc<RenderImage>>),
}

static CACHE: Mutex<Option<HashMap<Key, Slot>>> = Mutex::new(None);
static WAKE: OnceLock<Proxy> = OnceLock::new();

/// Who to tell when one is ready (sent [`crate::ToHub::Thumb`]).
pub fn init(proxy: Proxy) {
    let _ = WAKE.set(proxy);
}

/// Draw the windows again: a picture is ready (a thumbnail, an
/// AppImage's icon).
pub fn wake() {
    if let Some(p) = WAKE.get() {
        p.send(crate::ToHub::Thumb);
    }
}

fn worker() -> &'static Mutex<mpsc::Sender<Key>> {
    static TX: OnceLock<Mutex<mpsc::Sender<Key>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Key>();
        let _ = std::thread::Builder::new().name("thumbnails".into()).spawn(move || {
            for key in rx {
                let img = make(&key.0, key.1);
                if let Ok(mut c) = CACHE.lock() {
                    c.get_or_insert_with(HashMap::new).insert(key, Slot::Ready(img));
                }
                wake();
            }
        });
        Mutex::new(tx)
    })
}

/// The picture at `path` no bigger than `px` pixels on a side, its shape
/// kept, as the kit draws it (BGRA).
fn make(path: &Path, px: u32) -> Option<Arc<RenderImage>> {
    let (img, _) = olo_images::open(path).ok()?;
    let mut img = img.thumbnail(px, px).to_rgba8();
    for p in img.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new([image::Frame::new(img)])))
}

/// The thumbnail of the picture at `path`, `px` device pixels at most on
/// a side: None while it's made (the windows are drawn again then), or
/// if it isn't a picture that can be read.
pub fn get(path: &Path, px: u32) -> Option<Arc<RenderImage>> {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let key: Key = (path.to_owned(), px, mtime);
    let mut c = CACHE.lock().ok()?;
    let map = c.get_or_insert_with(HashMap::new);
    match map.get(&key) {
        Some(Slot::Ready(img)) => return img.clone(),
        Some(Slot::Loading) => return None,
        None => {}
    }
    // Only a few kept: the column view shows one at a time.
    if map.len() > 32 {
        map.retain(|_, s| matches!(s, Slot::Loading));
    }
    map.insert(key.clone(), Slot::Loading);
    drop(c);
    let _ = worker().lock().ok()?.send(key);
    None
}
