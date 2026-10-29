//! Icons: Olo's own set (data/icons/<px>/<name>.png, pre-scaled to
//! device sizes, in the theme chosen in Preferences ▸ Themes) with a
//! freedesktop icon-theme fallback, so Deskbar and Tracker can show
//! third-party apps and file types too.
//!
//! Textures are cached per window context and keyed by theme and device
//! size, so every icon is drawn 1:1 from a source made for that size.
//! When the theme changes the kit repaints every window and the old
//! theme's textures go. Icons are decoded once for the whole process; a
//! frame decodes what it can in its budget and a worker the rest
//! (`icon`).

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
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

/// `img` for drawing at `px` device pixels: a bitmap bigger than that
/// (the next size up: ours come at 16, 24, 32, …, so 20 at 1.25× and 28
/// at 1.75× have none of their own) averaged down to exactly it once, so
/// it is drawn a texture pixel to a device pixel rather than resampled by
/// the GPU every frame (soft), as Chrome scales its icons.
fn fitted(img: olo_xdg::Image, px: u32) -> ColorImage {
    let (w, h) = (img.width, img.height);
    let side = w.max(h);
    if img.premultiplied || side <= px || px == 0 {
        return to_color_image(img);
    }
    let fit = |v: u32| ((v as f32 * px as f32 / side as f32).round() as usize).max(1);
    crate::brand::shrink(img.rgba.as_chunks::<4>().0, w as usize, h as usize, fit(w), fit(h))
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

/// Textures by theme, name and device size, and the theme they were last
/// asked for in (when that changes, the others go).
#[derive(Default)]
struct Cache {
    current: &'static str,
    map: HashMap<Key, Option<TextureHandle>>,
}

/// An icon: its theme, name and device size.
type Key = (&'static str, String, u32);

/// Icons decoded, for every window of the process (None: there is no such
/// icon), and the windows waiting for one being decoded.
#[derive(Default)]
struct Decoded {
    map: HashMap<Key, Option<Arc<ColorImage>>>,
    waiting: HashMap<Key, Vec<Context>>,
}

static DECODED: Mutex<Option<Decoded>> = Mutex::new(None);

/// How long a frame may spend finding and decoding icons itself (more
/// than one icon's worth: a frame shows what it can at once); the rest
/// are decoded by a worker and drawn a frame or two later. Finding one
/// searches folders and decoding one can take milliseconds, and a frame
/// has 8.3 (HACKING.md, the responsiveness rule).
const DECODE_BUDGET: Duration = Duration::from_micros(1500);

/// How an icon stands.
enum Icon {
    Ready(TextureHandle),
    /// Being decoded: its place is kept and nothing drawn yet.
    Loading,
    Missing,
}

fn decode(theme: &str, name: &str, px: u32) -> Option<Arc<ColorImage>> {
    olo_xdg::load_icon_in(theme, name, px).map(|img| Arc::new(fitted(img, px)))
}

/// Decode `key` on the icon worker, then repaint `ctx`.
fn decode_later(key: Key, ctx: &Context) {
    static WORKER: OnceLock<Mutex<std::sync::mpsc::Sender<Key>>> = OnceLock::new();
    let first = {
        let mut d = DECODED.lock().unwrap();
        let waiting = d.get_or_insert_with(Decoded::default).waiting.entry(key.clone()).or_default();
        let first = waiting.is_empty();
        if !waiting.iter().any(|c| c == ctx) {
            waiting.push(ctx.clone());
        }
        first
    };
    if !first {
        return;
    }
    let tx = WORKER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Key>();
        std::thread::Builder::new()
            .name("olo-icons".into())
            .spawn(move || {
                for key in rx {
                    let image = decode(key.0, &key.1, key.2);
                    let waiting = {
                        let mut d = DECODED.lock().unwrap();
                        let d = d.get_or_insert_with(Decoded::default);
                        d.map.insert(key.clone(), image);
                        d.waiting.remove(&key).unwrap_or_default()
                    };
                    for ctx in waiting {
                        ctx.request_repaint();
                    }
                }
            })
            .expect("icon worker");
        Mutex::new(tx)
    });
    let _ = tx.lock().unwrap().send(key);
}

/// Icon `name` for drawing at `px` device pixels on `ctx`, in `theme`:
/// from this window's textures, else decoded already (by any window),
/// else decoded now while this frame's budget lasts (and always in a
/// window's first frame, which shows everything at once), else by the
/// worker.
fn icon(ctx: &Context, theme: &str, name: &str, logical: f32) -> Icon {
    let theme = olo_xdg::themes::resolve(theme).id;
    let px = (logical * ctx.pixels_per_point()).round().max(1.0) as u32;
    let key = (theme, name.to_owned(), px);
    let cache_id = egui::Id::unique("olo-icon-cache");
    let cache = ctx.data_mut(|d| d.get_temp_mut_or_default::<Arc<Mutex<Cache>>>(cache_id).clone());
    let mut cache = cache.lock().unwrap();
    let now = olo_xdg::themes::current().id;
    if cache.current != now {
        cache.current = now;
        cache.map.retain(|k, _| k.0 == now);
        if let Some(d) = DECODED.lock().unwrap().as_mut() {
            d.map.retain(|k, _| k.0 == now);
        }
    }
    if let Some(hit) = cache.map.get(&key) {
        return hit.clone().map_or(Icon::Missing, Icon::Ready);
    }
    let decoded = DECODED.lock().unwrap().get_or_insert_with(Decoded::default).map.get(&key).cloned();
    let image = match decoded {
        Some(image) => image,
        None => {
            // This frame's time spent decoding, by its pass.
            let spent_id = egui::Id::unique("olo-icon-decode-time");
            let pass = ctx.cumulative_pass_nr();
            let (at, spent) = ctx.data(|d| d.get_temp::<(u64, Duration)>(spent_id)).unwrap_or((pass, Duration::ZERO));
            let spent = if at == pass { spent } else { Duration::ZERO };
            if pass > 0 && spent >= DECODE_BUDGET {
                decode_later(key, ctx);
                return Icon::Loading;
            }
            let t = Instant::now();
            let image = decode(theme, name, px);
            ctx.data_mut(|d| d.insert_temp(spent_id, (pass, spent + t.elapsed())));
            DECODED.lock().unwrap().get_or_insert_with(Decoded::default).map.insert(key.clone(), image.clone());
            image
        }
    };
    let tex = image.map(|img| ctx.load_texture(format!("icon:{theme}/{name}@{px}"), (*img).clone(), TextureOptions::LINEAR));
    cache.map.insert(key, tex.clone());
    tex.map_or(Icon::Missing, Icon::Ready)
}

/// The icon `name` (ours, a theme icon name, or an absolute path) as a
/// texture for drawing at `logical` points on `ctx`, in the current theme
/// (None: there is no such icon, or it is still being decoded).
pub fn texture(ctx: &Context, name: &str, logical: f32) -> Option<TextureHandle> {
    texture_in(ctx, olo_xdg::themes::current().id, name, logical)
}

/// As [`texture`], in theme `theme` (e.g. to show a theme before it is
/// chosen).
pub fn texture_in(ctx: &Context, theme: &str, name: &str, logical: f32) -> Option<TextureHandle> {
    match icon(ctx, theme, name, logical) {
        Icon::Ready(tex) => Some(tex),
        Icon::Loading | Icon::Missing => None,
    }
}

/// Draw icon `name` filling `rect` (snapped to device pixels). Returns
/// false if no such icon exists (true while it is being decoded: its
/// place is kept, and it shows a frame or two later).
pub fn paint(painter: &Painter, rect: Rect, name: &str) -> bool {
    paint_tinted(painter, rect, name, Color32::WHITE)
}

/// As [`paint`], in theme `theme`.
pub fn paint_in(painter: &Painter, rect: Rect, theme: &str, name: &str) -> bool {
    paint_icon(painter, rect, icon(painter.ctx(), theme, name, rect.width().max(rect.height())), Color32::WHITE)
}

/// As [`paint`], multiplied by `tint` (e.g. half alpha for a dragged or
/// cut item, or the selection darkening Tracker applies).
pub fn paint_tinted(painter: &Painter, rect: Rect, name: &str, tint: Color32) -> bool {
    paint_icon(painter, rect, icon(painter.ctx(), olo_xdg::themes::current().id, name, rect.width().max(rect.height())), tint)
}

fn paint_icon(painter: &Painter, rect: Rect, icon: Icon, tint: Color32) -> bool {
    match icon {
        Icon::Ready(tex) => {
            paint_texture(painter, rect, &tex, tint);
            true
        }
        Icon::Loading => true,
        Icon::Missing => false,
    }
}

/// `tex` in `rect`: at its own size when it was made for it (within a
/// device pixel), a texture pixel to a device pixel, centred from a whole
/// one. Snapping `rect`'s corners instead can make it a pixel narrower or
/// wider than the texture when its size isn't whole device pixels (15
/// points at 1.5×), and the icon is resampled (soft). A bitmap smaller
/// than the rect (no bigger size to shrink) is stretched to it.
fn paint_texture(painter: &Painter, rect: Rect, tex: &TextureHandle, tint: Color32) {
    let ctx = painter.ctx();
    let ppp = ctx.pixels_per_point();
    let [w, h] = tex.size();
    let (tw, th) = ((rect.width() * ppp).round(), (rect.height() * ppp).round());
    let r = if (w as f32 - tw).abs() <= 1.0 && (h as f32 - th).abs() <= 1.0 {
        crate::style::snap_centred(ctx, rect.center(), egui::vec2(w as f32, h as f32) / ppp)
    } else {
        crate::style::snap(ctx, rect)
    };
    painter.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
}

/// `tex` filling the circle inscribed in `rect` (the middle square of it,
/// if it isn't square): a person's picture. A hairline `ring` (when not
/// transparent) keeps its edge clean.
pub fn paint_round(painter: &Painter, rect: Rect, tex: &TextureHandle, ring: Color32) {
    let size = tex.size_vec2();
    // The middle square, in texture coordinates.
    let (kx, ky) = if size.x > size.y { (size.y / size.x, 1.0) } else { (1.0, size.x / size.y.max(1.0)) };
    let uv = |dx: f32, dy: f32| pos2(0.5 + dx * kx / 2.0, 0.5 + dy * ky / 2.0);
    let center = rect.center();
    let radius = rect.width().min(rect.height()) / 2.0;
    let mut mesh = egui::Mesh::with_texture(tex.id());
    mesh.vertices.push(egui::epaint::Vertex { pos: center, uv: uv(0.0, 0.0), color: Color32::WHITE });
    // Enough sides that it's round at any scale.
    let n = 64u32;
    for i in 0..n {
        let (dy, dx) = (i as f32 / n as f32 * std::f32::consts::TAU).sin_cos();
        mesh.vertices.push(egui::epaint::Vertex { pos: center + egui::vec2(dx, dy) * radius, uv: uv(dx, dy), color: Color32::WHITE });
    }
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    painter.add(mesh);
    if ring.a() > 0 {
        painter.circle_stroke(center, radius - 0.25, egui::Stroke::new(0.75, ring));
    }
}

/// `tex` covering `rect` (cropped to its shape about the middle, never
/// stretched), its corners rounded by `radius`: a photo or artwork.
pub fn paint_cover(painter: &Painter, rect: Rect, tex: &TextureHandle, radius: f32) {
    let size = tex.size_vec2();
    let k = (rect.width() / rect.height().max(1.0)) / (size.x / size.y.max(1.0));
    let uv = if k > 1.0 {
        // Wider than the picture: crop its top and bottom.
        let h = 1.0 / k;
        Rect::from_min_max(pos2(0.0, (1.0 - h) / 2.0), pos2(1.0, (1.0 + h) / 2.0))
    } else {
        Rect::from_min_max(pos2((1.0 - k) / 2.0, 0.0), pos2((1.0 + k) / 2.0, 1.0))
    };
    let r = crate::style::snap(painter.ctx(), rect);
    painter.add(egui::epaint::RectShape::filled(r, radius, Color32::WHITE).with_texture(tex.id(), uv));
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
