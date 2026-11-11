//! Pictures made small (a wallpaper's preview, a picture file's), decoded
//! and shrunk on a thread of their own in the order asked for, kept by
//! the file (its path and when it last changed) and size. A drawing asks
//! as it's made; while its picture isn't ready it draws without, and is
//! drawn again, alone, when the picture comes
//! (`Painter::paint_unfinished`).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, mpsc},
    time::SystemTime,
};

use olo_interface::{Painter, RenderImage};
use olo_sync::Mutex;

type Key = (PathBuf, u32, Option<SystemTime>);

enum Slot {
    Loading,
    Ready(Option<Arc<RenderImage>>),
}

static CACHE: Mutex<Option<HashMap<Key, Slot>>> = Mutex::new(None);

/// The worker's queue (the worker starts with the first picture asked
/// for).
fn loader() -> &'static Mutex<mpsc::Sender<Key>> {
    static LOADER: OnceLock<Mutex<mpsc::Sender<Key>>> = OnceLock::new();
    LOADER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Key>();
        let _ = std::thread::Builder::new().name("pictures".into()).spawn(move || {
            for key in rx {
                let img = make(&key.0, key.1);
                CACHE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(key, Slot::Ready(img));
                // What was drawn without it is drawn again.
                olo_interface::all_windows(|cx| cx.repaint_unfinished());
            }
        });
        Mutex::new(tx)
    })
}

/// The picture at `path` no bigger than `px` pixels on a side, its shape
/// kept, as the Interface Kit draws it (BGRA).
fn make(path: &Path, px: u32) -> Option<Arc<RenderImage>> {
    let (img, _) = olo_images::open(path).ok()?;
    let mut img = img.thumbnail(px, px).to_rgba8();
    for p in img.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new([image::Frame::new(img)])))
}

/// The picture at `path`, `px` device pixels at most on a side, for what
/// `g` is painting: None while it's made (the drawing is made again
/// when it has been), or if it isn't a picture that can be read.
pub fn thumbnail(g: &mut Painter, path: &Path, px: u32) -> Option<Arc<RenderImage>> {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let key: Key = (path.to_owned(), px, mtime);
    {
        let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        let map = c.get_or_insert_with(HashMap::new);
        match map.get(&key) {
            Some(Slot::Ready(img)) => return img.clone(),
            Some(Slot::Loading) => {
                g.paint_unfinished();
                return None;
            }
            None => {}
        }
        // A few dozen kept: previews come a handful at a time.
        if map.len() > 64 {
            map.retain(|_, s| matches!(s, Slot::Loading));
        }
        map.insert(key.clone(), Slot::Loading);
    }
    g.paint_unfinished();
    let _ = loader().lock().unwrap_or_else(|e| e.into_inner()).send(key);
    None
}
