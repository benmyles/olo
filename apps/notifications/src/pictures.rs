//! The pictures notes show, each made for the place it's drawn in: a
//! note's own (sent with it) and those in files (a history entry's kept
//! picture, a note's large picture), cut to fill their place when they
//! must (a cover) and resampled exactly to its device size on a worker,
//! so a texture pixel is a device pixel. Kept by what, how big and how
//! cut; the windows are drawn again as each arrives.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use olo_kit::{Painter, RenderImage};
use olo_sync::Mutex;

/// Pixels in no toolkit's type: RGBA, premultiplied.
#[derive(Debug, PartialEq)]
pub struct Raw {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

impl Raw {
    /// From straight (not premultiplied) RGBA, as a note sends it.
    pub fn from_unmultiplied(width: usize, height: usize, rgba: &[u8]) -> Raw {
        let b = olo_preview::Bitmap::from_rgba_unmultiplied([width, height], rgba);
        Raw { size: b.size, rgba: b.rgba }
    }

    fn load(path: &Path) -> Option<Raw> {
        let img = image::open(path).ok()?.into_rgba8();
        Some(Raw::from_unmultiplied(img.width() as usize, img.height() as usize, img.as_raw()))
    }

    /// Its middle, as wide for its height as `w` × `h`.
    fn middle(&self, (w, h): (u32, u32)) -> Raw {
        let [sw, sh] = self.size;
        let want = w as f32 / h.max(1) as f32;
        let (cw, ch) = if sw as f32 / sh.max(1) as f32 > want { (((sh as f32 * want).round() as usize).clamp(1, sw), sh) } else { (sw, ((sw as f32 / want).round() as usize).clamp(1, sh)) };
        let (x0, y0) = ((sw - cw) / 2, (sh - ch) / 2);
        let mut rgba = Vec::with_capacity(cw * ch * 4);
        for y in y0..y0 + ch {
            let row = (y * sw + x0) * 4;
            rgba.extend_from_slice(&self.rgba[row..row + cw * 4]);
        }
        Raw { size: [cw, ch], rgba }
    }
}

/// Where a picture comes from.
#[derive(Clone)]
pub enum Source {
    /// Sent with note `id`.
    Note(u32, Arc<Raw>),
    File(PathBuf),
}

impl Source {
    fn key(&self) -> String {
        match self {
            Source::Note(id, _) => format!("note:{id}:{:p}", Arc::as_ptr(self.raw_ref())),
            Source::File(p) => format!("file:{}", p.display()),
        }
    }

    fn raw_ref(&self) -> &Arc<Raw> {
        match self {
            Source::Note(_, r) => r,
            Source::File(_) => unreachable!("a file's pixels are read on the worker"),
        }
    }
}

/// How a picture fills its place.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Shape {
    /// All of it, as large as fits (its own proportions).
    Fit,
    /// Filling it, its middle cut out.
    Cover,
}

/// A picture's own size, `(w, h)` pixels, fitted within `place`.
pub fn fitted(size: [usize; 2], place: (u32, u32)) -> (u32, u32) {
    let [w, h] = size.map(|v| v.max(1) as f32);
    let k = (place.0 as f32 / w).min(place.1 as f32 / h);
    (((w * k).round() as u32).max(1), ((h * k).round() as u32).max(1))
}

type Key = (String, u32, u32, Shape);

enum Slot {
    Making,
    Made(Arc<RenderImage>),
    Missing,
}

/// Pictures made so far (one cache for the program: the banners and the
/// History window draw the same ones).
static MADE: Mutex<Option<HashMap<Key, Slot>>> = Mutex::new(None);

/// At most this many kept; past it, they all go and are made again as
/// they're drawn (the History window scrolled through two hundred
/// pictures). One let go of takes its texture with it.
const KEEP: usize = 400;

/// `source`'s picture made for a place `place` device pixels in size
/// (`shape`: how it fills it; fitted, it is its own size within it, to
/// be centred there), for what `g` is painting: None while it's being
/// made (the drawing is made again when it is) or if it can't be read.
pub fn get(g: &mut Painter, source: &Source, place: (u32, u32), shape: Shape) -> Option<Arc<RenderImage>> {
    let place = (place.0.max(1), place.1.max(1));
    let key = (source.key(), place.0, place.1, shape);
    {
        let mut made = MADE.lock().unwrap_or_else(|e| e.into_inner());
        let made = made.get_or_insert_with(HashMap::new);
        match made.get(&key) {
            Some(Slot::Made(img)) => return Some(img.clone()),
            Some(Slot::Missing) => return None,
            Some(Slot::Making) => {
                g.paint_unfinished();
                return None;
            }
            None => {}
        }
        if made.len() >= KEEP {
            made.retain(|_, slot| matches!(slot, Slot::Making));
        }
        made.insert(key.clone(), Slot::Making);
    }
    g.paint_unfinished();
    let source = source.clone();
    olo_kit::app_kit::pool::run(move || {
        let img = (|| {
            let raw = match &source {
                Source::Note(_, raw) => raw.clone(),
                Source::File(path) => Arc::new(Raw::load(path)?),
            };
            let size = |r: &Raw| (r.size[0] as u32, r.size[1] as u32);
            match shape {
                Shape::Cover => {
                    let middle = raw.middle(place);
                    olo_kit::pictures::resampled(&middle.rgba, size(&middle), place)
                }
                Shape::Fit => olo_kit::pictures::resampled(&raw.rgba, size(&raw), fitted(raw.size, place)),
            }
        })();
        MADE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(key, img.map_or(Slot::Missing, Slot::Made));
        // What was drawn without it is drawn again.
        olo_kit::all_windows(|cx| cx.repaint_unfinished());
    })
    .detach();
    None
}
