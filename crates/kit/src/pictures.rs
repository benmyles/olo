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

/// A file, the size asked for, and when the file last changed. A height
/// of 0: no bigger than the width on a side ([`thumbnail`]); else fitted
/// within that many pixels ([`fitted`]).
type Key = (PathBuf, u32, u32, Option<SystemTime>);

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
                let img = if key.2 == 0 { make(&key.0, key.1) } else { make_fitted(&key.0, (key.1, key.2)) };
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
    get(g, path, (px, 0))
}

/// The picture at `path` fitted whole within `place` device pixels, its
/// shape kept (to be centred there, a texture pixel to a device pixel):
/// resampled on its premultiplied colours (Lanczos made smaller,
/// Catmull-Rom made bigger), so one with clear corners (a person's round
/// picture) keeps clean edges. None while it's made (the drawing is made
/// again when it has been), or if it isn't a picture that can be read.
pub fn fitted(g: &mut Painter, path: &Path, place: (u32, u32)) -> Option<Arc<RenderImage>> {
    get(g, path, (place.0.max(1), place.1.max(1)))
}

/// [`fitted`]'s picture, made.
fn make_fitted(path: &Path, place: (u32, u32)) -> Option<Arc<RenderImage>> {
    let img = olo_images::open(path).ok()?.0.to_rgba8();
    let (sw, sh) = (img.width().max(1), img.height().max(1));
    let mut rgba = img.into_raw();
    // (Premultiplied: what's resampled.)
    for p in rgba.chunks_exact_mut(4) {
        let a = u32::from(p[3]);
        if a < 255 {
            for c in &mut p[..3] {
                *c = ((u32::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
    let k = (place.0 as f32 / sw as f32).min(place.1 as f32 / sh as f32);
    let (w, h) = (((sw as f32 * k).round() as u32).max(1), ((sh as f32 * k).round() as u32).max(1));
    resampled(&rgba, (sw, sh), (w, h))
}

/// Premultiplied RGBA pixels, `from` in size, resampled to `to` device
/// pixels (Lanczos made smaller; Catmull-Rom made bigger, as sharp to the
/// eye at a third of the cost), as a picture to draw a texel a device
/// pixel. Off the window's looper: it's milliseconds for a big one.
pub fn resampled(rgba: &[u8], from: (u32, u32), to: (u32, u32)) -> Option<Arc<RenderImage>> {
    use fast_image_resize::{
        FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
        images::{Image, ImageRef},
    };
    let ((sw, sh), (w, h)) = (from, to);
    let mut out = if (sw, sh) == (w, h) {
        rgba.to_vec()
    } else {
        let from = ImageRef::new(sw, sh, rgba, PixelType::U8x4).ok()?;
        let mut to = Image::new(w, h, PixelType::U8x4);
        let filter = if w > sw || h > sh { FilterType::CatmullRom } else { FilterType::Lanczos3 };
        Resizer::new().resize(&from, &mut to, &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(filter)).use_alpha(false)).ok()?;
        to.into_vec()
    };
    // (Straight BGRA: as the Interface Kit draws it.)
    for p in out.as_chunks_mut::<4>().0 {
        let a = p[3];
        if a != 0 && a != 255 {
            let k = 255.0 / f32::from(a);
            for c in &mut p[..3] {
                *c = (f32::from(*c) * k).round().min(255.0) as u8;
            }
        }
        p.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new([image::Frame::new(image::RgbaImage::from_raw(w, h, out)?)])))
}

fn get(g: &mut Painter, path: &Path, (w, h): (u32, u32)) -> Option<Arc<RenderImage>> {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let key: Key = (path.to_owned(), w, h, mtime);
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

/// A picture from pixels already as the Interface Kit draws them, BGRA
/// (`w` × `h`: a captured screen's frame, shrunk), to draw as it is.
pub fn from_bgra(w: u32, h: u32, bgra: Vec<u8>) -> Option<Arc<RenderImage>> {
    Some(Arc::new(RenderImage::new([image::Frame::new(image::RgbaImage::from_raw(w, h, bgra)?)])))
}
