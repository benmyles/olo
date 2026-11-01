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

use olo_ui::{
    gpui::{App, AppContext as _, Global, RenderImage},
    widgets::image_view::{self, Rgba},
};

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

impl Rgba for Raw {
    fn size(&self) -> [usize; 2] {
        self.size
    }
    fn rgba(&self) -> &[u8] {
        &self.rgba
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

/// Pictures made so far.
#[derive(Default)]
pub struct Pictures {
    made: HashMap<Key, Slot>,
}

impl Global for Pictures {}

/// At most this many kept; past it, the oldest go (the History window
/// scrolled through two hundred pictures).
const KEEP: usize = 400;

/// `source`'s picture made for a place `place` device pixels in size
/// (`shape`: how it fills it; fitted, it is its own size within it, to
/// be centred there); None while it's being made (the windows are drawn
/// again when it is) or if it can't be read.
pub fn get(cx: &mut App, source: &Source, place: (u32, u32), shape: Shape) -> Option<Arc<RenderImage>> {
    let place = (place.0.max(1), place.1.max(1));
    let key = (source.key(), place.0, place.1, shape);
    let pictures = cx.default_global::<Pictures>();
    match pictures.made.get(&key) {
        Some(Slot::Made(img)) => return Some(img.clone()),
        Some(Slot::Making | Slot::Missing) => return None,
        None => {}
    }
    if pictures.made.len() >= KEEP {
        // (Each leaves the GPU's atlas too, or they'd stay there.)
        let gone: Vec<Arc<RenderImage>> = pictures.made.drain().filter_map(|(_, slot)| if let Slot::Made(img) = slot { Some(img) } else { None }).collect();
        for img in gone {
            cx.drop_image(img, None);
        }
    }
    let pictures = cx.default_global::<Pictures>();
    pictures.made.insert(key.clone(), Slot::Making);
    let source = source.clone();
    let made = cx.background_spawn(async move {
        let raw = match &source {
            Source::Note(_, raw) => raw.clone(),
            Source::File(path) => Arc::new(Raw::load(path)?),
        };
        match shape {
            Shape::Cover => image_view::resampled(&raw.middle(place), place),
            Shape::Fit => image_view::resampled(raw.as_ref(), fitted(raw.size, place)),
        }
    });
    cx.spawn(async move |cx| {
        let img = made.await;
        let _ = cx.update(|cx| {
            cx.default_global::<Pictures>().made.insert(key, img.map_or(Slot::Missing, Slot::Made));
            cx.refresh_windows();
        });
    })
    .detach();
    None
}
