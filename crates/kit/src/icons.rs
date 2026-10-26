//! Icons: Tabula's own set (data/icons/<px>/<name>.png, pre-scaled to
//! device sizes) with a freedesktop icon-theme fallback, so Deskbar and
//! Tracker can show third-party apps and file types too.
//!
//! Textures are cached per window context and keyed by device size, so
//! every icon is drawn 1:1 from a source made for that size.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use egui::{Color32, ColorImage, Context, Painter, Rect, TextureHandle, TextureOptions, pos2};

/// Device-pixel sizes shipped in data/icons.
const OWN_SIZES: &[u32] = &[16, 24, 32, 48, 64, 96, 128, 256];

/// Directories holding Tabula's data (icons/, backgrounds/, ...).
pub fn data_dirs() -> &'static [PathBuf] {
    static DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    DIRS.get_or_init(|| {
        let mut v = Vec::new();
        if let Some(d) = std::env::var_os("TABULA_DATA") {
            v.push(PathBuf::from(d));
        }
        // Development: target/<profile>/<bin> → <repo>/data.
        if let Ok(exe) = std::env::current_exe() {
            for up in exe.ancestors().skip(1).take(4) {
                let d = up.join("data");
                if d.join("icons").is_dir() {
                    v.push(d);
                    break;
                }
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            v.push(Path::new(&home).join(".local/share/tabula"));
        }
        v.push("/usr/local/share/tabula".into());
        v.push("/usr/share/tabula".into());
        v.retain(|d| d.is_dir());
        v
    })
}

fn xdg_data_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        v.push(Path::new(&home).join(".local/share"));
    }
    let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    v.extend(sys.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
    v.push("/var/lib/flatpak/exports/share".into());
    v
}

/// Find the best source file for `name` at `px` device pixels.
fn find(name: &str, px: u32) -> Option<PathBuf> {
    if name.starts_with('/') {
        let p = PathBuf::from(name);
        return p.is_file().then_some(p);
    }
    // Ours: the smallest pre-scaled size at least as large as needed.
    for dir in data_dirs() {
        let sizes = OWN_SIZES.iter().filter(|&&s| s >= px).chain(OWN_SIZES.iter().rev().filter(|&&s| s < px));
        for s in sizes {
            let p = dir.join("icons").join(s.to_string()).join(format!("{name}.png"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    // Freedesktop themes.
    for base in xdg_data_dirs() {
        for theme in ["hicolor", "Adwaita", "breeze"] {
            let root = base.join("icons").join(theme);
            if !root.is_dir() {
                continue;
            }
            let mut best: Option<(u32, PathBuf)> = None;
            for sub in ["apps", "mimetypes", "places", "devices", "categories", "actions", "status"] {
                let svg = root.join("scalable").join(sub).join(format!("{name}.svg"));
                if svg.is_file() {
                    return Some(svg);
                }
                for s in [16u32, 22, 24, 32, 48, 64, 96, 128, 256, 512] {
                    let p = root.join(format!("{s}x{s}")).join(sub).join(format!("{name}.png"));
                    if p.is_file() {
                        let better = match &best {
                            None => true,
                            Some((b, _)) => (s >= px && (*b < px || s < *b)) || (*b < px && s > *b),
                        };
                        if better {
                            best = Some((s, p));
                        }
                    }
                }
            }
            if let Some((_, p)) = best {
                return Some(p);
            }
        }
        for ext in ["svg", "png"] {
            let p = base.join("pixmaps").join(format!("{name}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn decode_png(path: &Path) -> Option<ColorImage> {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).ok()?));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let px: Vec<Color32> = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => buf[..w * h * 4]
            .as_chunks::<4>().0.iter()
            .map(|p| Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]))
            .collect(),
        (png::ColorType::Rgb, png::BitDepth::Eight) => {
            buf[..w * h * 3].as_chunks::<3>().0.iter().map(|p| Color32::from_rgb(p[0], p[1], p[2])).collect()
        }
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => buf[..w * h * 2]
            .as_chunks::<2>().0.iter()
            .map(|p| Color32::from_rgba_unmultiplied(p[0], p[0], p[0], p[1]))
            .collect(),
        (png::ColorType::Grayscale, png::BitDepth::Eight) => buf[..w * h].iter().map(|&g| Color32::from_gray(g)).collect(),
        _ => return None,
    };
    Some(ColorImage::new([w, h], px))
}

fn render_svg(path: &Path, px: u32) -> Option<ColorImage> {
    let data = std::fs::read(path).ok()?;
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    let scale = px as f32 / size.width().max(size.height());
    let mut pixmap = resvg::tiny_skia::Pixmap::new(px, px)?;
    let dx = (px as f32 - size.width() * scale) / 2.0;
    let dy = (px as f32 - size.height() * scale) / 2.0;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, dx, dy), &mut pixmap.as_mut());
    // tiny-skia is premultiplied, as egui wants.
    let px_data: Vec<Color32> = pixmap
        .pixels()
        .iter()
        .map(|p| Color32::from_rgba_premultiplied(p.red(), p.green(), p.blue(), p.alpha()))
        .collect();
    Some(ColorImage::new([px as usize, px as usize], px_data))
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
    let image = find(name, px).and_then(|p| {
        if p.extension().is_some_and(|e| e == "svg") { render_svg(&p, px) } else { decode_png(&p) }
    });
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

/// The icon name a desktop app declares (`Icon=` in its .desktop file),
/// looked up by app id / desktop file name.
pub fn icon_for_app(app_id: &str) -> Option<String> {
    let file = crate::desktop_entry::find(app_id)?;
    file.icon
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
        let tex = image::open(&key.0).ok().map(|img| {
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

/// One background thread decodes thumbnails, in request order.
fn thumb_worker() -> std::sync::mpsc::Sender<Box<dyn FnOnce() + Send>> {
    static TX: std::sync::OnceLock<std::sync::Mutex<std::sync::mpsc::Sender<Box<dyn FnOnce() + Send>>>> =
        std::sync::OnceLock::new();
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
