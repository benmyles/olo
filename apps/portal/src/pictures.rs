//! Pictures from files (a person's, for the Account portal's question; a
//! background's, for the Wallpaper portal's), fitted whole within the
//! place they're drawn at its device size, resampled on a worker so a
//! texture pixel is a device pixel, and kept; the windows are drawn again
//! as each arrives.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use olo_ui::{
    gpui::{App, AppContext as _, Global, RenderImage},
    widgets::image_view::{self, Rgba},
};

/// Pixels decoded from a file: RGBA, premultiplied.
struct Raw {
    size: [usize; 2],
    rgba: Vec<u8>,
}

impl Rgba for Raw {
    fn size(&self) -> [usize; 2] {
        self.size
    }
    fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

fn load(path: &Path) -> Option<Raw> {
    let img = olo_images::open(path).ok()?.0.to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut rgba = img.into_raw();
    for p in rgba.chunks_exact_mut(4) {
        let a = u32::from(p[3]);
        if a < 255 {
            for c in &mut p[..3] {
                *c = ((u32::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
    Some(Raw { size: [w, h], rgba })
}

/// A picture `size` pixels, fitted within `place`: its own proportions.
pub fn fitted(size: [usize; 2], place: (u32, u32)) -> (u32, u32) {
    let [w, h] = size.map(|v| v.max(1) as f32);
    let k = (place.0 as f32 / w).min(place.1 as f32 / h);
    (((w * k).round() as u32).max(1), ((h * k).round() as u32).max(1))
}

enum Slot {
    Making,
    Made(Arc<RenderImage>),
    Missing,
}

#[derive(Default)]
struct Pictures(HashMap<(PathBuf, u32, u32), Slot>);

impl Global for Pictures {}

/// `path`'s picture fitted within `place` device pixels (to be centred
/// there); None while it's being made, or if it can't be read.
pub fn get(cx: &mut App, path: &Path, place: (u32, u32)) -> Option<Arc<RenderImage>> {
    let place = (place.0.max(1), place.1.max(1));
    let key = (path.to_owned(), place.0, place.1);
    let pictures = cx.default_global::<Pictures>();
    match pictures.0.get(&key) {
        Some(Slot::Made(img)) => return Some(img.clone()),
        Some(Slot::Making | Slot::Missing) => return None,
        None => {}
    }
    pictures.0.insert(key.clone(), Slot::Making);
    let path = path.to_owned();
    let made = cx.background_spawn(async move {
        let raw = load(&path)?;
        image_view::resampled(&raw, fitted(raw.size, place))
    });
    cx.spawn(async move |cx| {
        let img = made.await;
        let _ = cx.update(|cx| {
            cx.default_global::<Pictures>().0.insert(key, img.map_or(Slot::Missing, Slot::Made));
            cx.refresh_windows();
        });
    })
    .detach();
    None
}
