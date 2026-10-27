//! Icons: Olo's own set (data/icons/<px>/<name>.png, pre-scaled to
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

pub use olo_xdg::{data_dirs, icon_for_app};

fn to_color_image(img: olo_xdg::Image) -> ColorImage {
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

/// The name shown with a drag picture, and where.
#[derive(Clone, Copy)]
pub enum DragLabel<'a> {
    None,
    /// Under the icon, as in an icon view.
    Below(&'a str),
    /// Beside it, as in a list.
    Right(&'a str),
}

/// A picture to drag `count` things by, as Tracker's were: icon `name` at
/// `logical` points with the item's name highlighted beside or below it,
/// see-through (so what is under it shows), the icon stacked twice over
/// for several. The pointer holds it at `hotspot` (logical, from the
/// icon's top left).
pub fn drag_icon(ctx: &Context, name: &str, logical: f32, count: usize, hotspot: egui::Vec2, label: DragLabel) -> Option<crate::dnd::DragIcon> {
    let ppp = ctx.pixels_per_point();
    let scale = ppp.ceil().max(1.0) as u32;
    let sc = scale as usize;
    let px = (logical * scale as f32).round().max(1.0) as u32;
    let icon = to_color_image(olo_xdg::load_icon(name, px)?);
    let [w, h] = icon.size;
    // Several: two more copies behind, each a little up and to the right.
    let (step, copies) = if count > 1 { (2 * sc, 3) } else { (0, 1) };
    let (iw, ih) = (w + step * (copies - 1), h + step * (copies - 1));

    // The name, from the glyphs egui already rasterised for this scale.
    let text = match label {
        DragLabel::Below(t) | DragLabel::Right(t) => Some(t),
        DragLabel::None => None,
    };
    let galley = text.map(|t| ctx.fonts_mut(|f| f.layout_no_wrap(t.to_owned(), crate::style::plain_font(), Color32::WHITE)));
    let (pad_x, pad_y, gap) = (4 * sc, sc, 2 * sc);
    let (lw, lh) = galley.as_ref().map_or((0, 0), |g| ((g.size().x * ppp).ceil() as usize + 2 * pad_x, (g.size().y * ppp).ceil() as usize + 2 * pad_y));
    let (cw, ch, icon_at, label_at) = match label {
        DragLabel::Below(_) => {
            let cw = iw.max(lw);
            (cw, ih + gap + lh, ((cw - iw) / 2, 0), ((cw - lw) / 2, ih + gap))
        }
        DragLabel::Right(_) => {
            let ch = ih.max(lh);
            (iw + gap + lw, ch, (0, (ch - ih) / 2), (iw + gap, (ch - lh) / 2))
        }
        DragLabel::None => (iw, ih, (0, 0), (0, 0)),
    };
    let mut out = ColorImage::new([cw, ch], vec![Color32::TRANSPARENT; cw * ch]);
    // Source over destination, both premultiplied.
    let over = |out: &mut ColorImage, x: usize, y: usize, src: Color32| {
        let dst = &mut out.pixels[y * cw + x];
        let a = 255 - src.a() as u32;
        let mix = |s: u8, d: u8| (s as u32 + d as u32 * a / 255).min(255) as u8;
        *dst = Color32::from_rgba_premultiplied(mix(src.r(), dst.r()), mix(src.g(), dst.g()), mix(src.b(), dst.b()), mix(src.a(), dst.a()));
    };
    for k in (0..copies).rev() {
        let (ox, oy) = (icon_at.0 + step * k, icon_at.1 + step * (copies - 1 - k));
        let fade = if k == 0 { 0.75 } else { 0.45 };
        for y in 0..h {
            for x in 0..w {
                over(&mut out, x + ox, y + oy, icon.pixels[y * w + x].gamma_multiply(fade));
            }
        }
    }
    if let Some(g) = galley {
        let p = crate::style::palette(ctx);
        let bg = crate::style::c(p.list_selected_background).gamma_multiply(0.7);
        let fg = crate::style::c(p.list_selected_item_text).gamma_multiply(0.9);
        for y in label_at.1..label_at.1 + lh {
            for x in label_at.0..label_at.0 + lw {
                over(&mut out, x, y, bg);
            }
        }
        let atlas = ctx.fonts(|f| f.image());
        let [aw, _] = atlas.size;
        let (tx, ty) = (label_at.0 + pad_x, label_at.1 + pad_y);
        for placed in &g.rows {
            for glyph in &placed.row.glyphs {
                let uv = glyph.uv_rect;
                if uv.is_nothing() {
                    continue;
                }
                let at = (placed.pos + glyph.pos.to_vec2() + uv.offset) * ppp;
                for v in uv.min[1]..uv.max[1] {
                    for u in uv.min[0]..uv.max[0] {
                        let x = tx as f32 + at.x.round() + (u - uv.min[0]) as f32;
                        let y = ty as f32 + at.y.round() + (v - uv.min[1]) as f32;
                        if x < 0.0 || y < 0.0 || x as usize >= cw || y as usize >= ch {
                            continue;
                        }
                        let cov = atlas.pixels[v as usize * aw + u as usize];
                        let src = if glyph.is_color { cov } else { fg.gamma_multiply(cov.a() as f32 / 255.0) };
                        over(&mut out, x as usize, y as usize, src);
                    }
                }
            }
        }
    }
    let shift = egui::vec2(icon_at.0 as f32, (icon_at.1 + step * (copies - 1)) as f32) / scale as f32;
    Some(crate::dnd::DragIcon { image: out, scale, hotspot: hotspot + shift })
}

type Cache = HashMap<(String, u32), Option<TextureHandle>>;

/// The icon `name` (ours, a theme icon name, or an absolute path) as a
/// texture for drawing at `logical` points on `ctx`.
pub fn texture(ctx: &Context, name: &str, logical: f32) -> Option<TextureHandle> {
    let px = (logical * ctx.pixels_per_point()).round().max(1.0) as u32;
    let key = (name.to_owned(), px);
    let cache_id = egui::Id::unique("olo-icon-cache");
    if let Some(hit) = ctx.data(|d| d.get_temp::<Arc<std::sync::Mutex<Cache>>>(cache_id)).and_then(|c| c.lock().unwrap().get(&key).cloned()) {
        return hit;
    }
    let image = olo_xdg::load_icon(name, px).map(to_color_image);
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
    let cache_id = egui::Id::unique("olo-thumb-cache");
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
