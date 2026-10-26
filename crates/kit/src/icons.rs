//! Icons: Tabula's own set (data/icons/<px>/<name>.png, pre-scaled to
//! device sizes) with a freedesktop icon-theme fallback, so Deskbar and
//! Tracker can show third-party apps and file types too.
//!
//! Textures are cached per window context and keyed by device size, so
//! every icon is drawn 1:1 from a source made for that size.

use std::{
    collections::HashMap,
    path::Path,
    sync::Arc,
};

use egui::{Color32, ColorImage, Context, Painter, Rect, TextureHandle, TextureOptions, pos2};

pub use tabula_xdg::{data_dirs, icon_for_app};

fn to_color_image(img: tabula_xdg::Image) -> ColorImage {
    let px: Vec<Color32> = img
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            if img.premultiplied {
                Color32::from_rgba_premultiplied(p[0], p[1], p[2], p[3])
            } else {
                Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3])
            }
        })
        .collect();
    ColorImage::new([img.width as usize, img.height as usize], px)
}

type Cache = HashMap<(String, u32), Option<TextureHandle>>;

/// The icon `name` (ours, a theme icon name, or an absolute path) as a
/// texture for drawing at `logical` points on `ctx`.
pub fn texture(ctx: &Context, name: &str, logical: f32) -> Option<TextureHandle> {
    let px = (logical * ctx.pixels_per_point()).round().max(1.0) as u32;
    let key = (name.to_owned(), px);
    let cache_id = egui::Id::unique("tabula-icon-cache");
    if let Some(hit) = ctx.data(|d| d.get_temp::<Arc<std::sync::Mutex<Cache>>>(cache_id)).and_then(|c| c.lock().unwrap().get(&key).cloned()) {
        return hit;
    }
    let image = tabula_xdg::load_icon(name, px).map(to_color_image);
    let tex = image.map(|img| ctx.load_texture(format!("icon:{name}@{px}"), img, TextureOptions::LINEAR));
    let cache = ctx.data_mut(|d| d.get_temp_mut_or_default::<Arc<std::sync::Mutex<Cache>>>(cache_id).clone());
    cache.lock().unwrap().insert(key, tex.clone());
    tex
}

/// Draw icon `name` filling `rect` (snapped to device pixels). Returns
/// false if no such icon exists.
pub fn paint(painter: &Painter, rect: Rect, name: &str) -> bool {
    paint_tinted(painter, rect, name, Color32::WHITE)
}

/// As [`paint`], multiplied by `tint` (e.g. half alpha for a dragged or
/// cut item, or the selection darkening Tracker applies).
pub fn paint_tinted(painter: &Painter, rect: Rect, name: &str, tint: Color32) -> bool {
    let ctx = painter.ctx();
    let Some(tex) = texture(ctx, name, rect.width().max(rect.height())) else {
        return false;
    };
    let ppp = ctx.pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let r = Rect::from_min_max(pos2(snap(rect.min.x), snap(rect.min.y)), pos2(snap(rect.max.x), snap(rect.max.y)));
    painter.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
    true
}

// ---- thumbnails ----------------------------------------------------------

enum Thumb {
    Loading,
    Ready(Option<TextureHandle>),
}

type ThumbKey = (std::path::PathBuf, u32, Option<std::time::SystemTime>);
type ThumbCache = Arc<std::sync::Mutex<HashMap<ThumbKey, Thumb>>>;

/// A thumbnail of the image at `path` no larger than `logical` points on
/// a side (aspect kept), decoded and scaled on a background thread. None
/// while it loads (a repaint follows) or if it isn't a readable image.
pub fn thumbnail(ctx: &Context, path: &Path, logical: f32) -> Option<TextureHandle> {
    let px = (logical * ctx.pixels_per_point()).round().max(1.0) as u32;
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let key: ThumbKey = (path.to_owned(), px, mtime);
    let cache_id = egui::Id::unique("tabula-thumb-cache");
    let cache = ctx.data_mut(|d| d.get_temp_mut_or_default::<ThumbCache>(cache_id).clone());
    let mut map = cache.lock().unwrap();
    match map.get(&key) {
        Some(Thumb::Ready(t)) => return t.clone(),
        Some(Thumb::Loading) => return None,
        None => {}
    }
    map.insert(key.clone(), Thumb::Loading);
    drop(map);
    let (ctx2, cache2) = (ctx.clone(), cache.clone());
    let _ = thumb_worker().send(Box::new(move || {
        let tex = crate::images::open(&key.0).ok().map(|(img, _)| {
            let img = img.thumbnail(px, px).to_rgba8();
            let size = [img.width() as usize, img.height() as usize];
            let ci = ColorImage::from_rgba_unmultiplied(size, img.as_raw());
            ctx2.load_texture(format!("thumb:{}@{px}", key.0.display()), ci, TextureOptions::LINEAR)
        });
        cache2.lock().unwrap().insert(key, Thumb::Ready(tex));
        ctx2.request_repaint();
    }));
    None
}

type Job = Box<dyn FnOnce() + Send>;

/// One background thread decodes thumbnails, in request order.
fn thumb_worker() -> std::sync::mpsc::Sender<Job> {
    static TX: std::sync::OnceLock<std::sync::Mutex<std::sync::mpsc::Sender<Job>>> = std::sync::OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Box<dyn FnOnce() + Send>>();
        std::thread::Builder::new()
            .name("thumbnails".into())
            .spawn(move || {
                for job in rx {
                    job();
                }
            })
            .expect("thumbnail thread");
        std::sync::Mutex::new(tx)
    })
    .lock()
    .unwrap()
    .clone()
}

/// Where to draw `tex` inside `bounds`: centred, aspect kept, scaled down
/// to fit but never up past one texel per device pixel (no blur), and
/// snapped to device pixels.
pub fn fit(ctx: &Context, tex: &TextureHandle, bounds: Rect) -> Rect {
    let ppp = ctx.pixels_per_point();
    let native = tex.size_vec2() / ppp;
    let k = (bounds.width() / native.x).min(bounds.height() / native.y).min(1.0);
    let r = Rect::from_center_size(bounds.center(), native * k);
    let snap = |v: f32| (v * ppp).round() / ppp;
    Rect::from_min_max(pos2(snap(r.min.x), snap(r.min.y)), pos2(snap(r.max.x), snap(r.max.y)))
}
