//! Pictures made small (a wallpaper's preview, a picture file's), decoded
//! and shrunk on a thread of their own in the order asked for, kept by
//! the file (its path and when it last changed) and size; every window is
//! drawn again as each comes, so a view just asks again.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::SystemTime,
};

use futures::StreamExt as _;
use gpui::{App, Global, RenderImage};
use olo_sync::Mutex;

type Key = (PathBuf, u32, Option<SystemTime>);

enum Slot {
    Loading,
    Ready(Option<Arc<RenderImage>>),
}

static CACHE: Mutex<Option<HashMap<Key, Slot>>> = Mutex::new(None);

/// The worker's queue, and the app's side that redraws as pictures come.
struct Loader {
    tx: mpsc::Sender<Key>,
}

impl Global for Loader {}

fn loader(cx: &mut App) -> mpsc::Sender<Key> {
    if let Some(l) = cx.try_global::<Loader>() {
        return l.tx.clone();
    }
    let (tx, rx) = mpsc::channel::<Key>();
    let (done_tx, mut done_rx) = futures::channel::mpsc::unbounded::<()>();
    let _ = std::thread::Builder::new().name("pictures".into()).spawn(move || {
        for key in rx {
            let img = make(&key.0, key.1);
            CACHE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(key, Slot::Ready(img));
            if done_tx.unbounded_send(()).is_err() {
                return;
            }
        }
    });
    cx.spawn(async move |cx| {
        while done_rx.next().await.is_some() {
            // (Several at once: one redraw.)
            while done_rx.try_recv().is_ok() {}
            let _ = cx.update(|cx| cx.refresh_windows());
        }
    })
    .detach();
    cx.set_global(Loader { tx: tx.clone() });
    tx
}

/// The picture at `path` no bigger than `px` pixels on a side, its shape
/// kept, as GPUI draws it (BGRA).
fn make(path: &Path, px: u32) -> Option<Arc<RenderImage>> {
    let (img, _) = olo_images::open(path).ok()?;
    let mut img = img.thumbnail(px, px).to_rgba8();
    for p in img.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new([image::Frame::new(img)])))
}

/// The picture at `path`, `px` device pixels at most on a side: None
/// while it's made (the windows are drawn again then), or if it isn't a
/// picture that can be read.
pub fn thumbnail(cx: &mut App, path: &Path, px: u32) -> Option<Arc<RenderImage>> {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let key: Key = (path.to_owned(), px, mtime);
    {
        let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        let map = c.get_or_insert_with(HashMap::new);
        match map.get(&key) {
            Some(Slot::Ready(img)) => return img.clone(),
            Some(Slot::Loading) => return None,
            None => {}
        }
        // A few dozen kept: previews come a handful at a time.
        if map.len() > 64 {
            map.retain(|_, s| matches!(s, Slot::Loading));
        }
        map.insert(key.clone(), Slot::Loading);
    }
    let _ = loader(cx).send(key);
    None
}
