//! Icons: Olo's own set (data/icons/<px>/<name>.png, pre-scaled to device
//! sizes, in the theme chosen in Preferences ▸ Themes), with a
//! freedesktop theme's for other programs and file types. Each is drawn
//! from a bitmap made for exactly the device size it's shown at, a
//! texture pixel to a device pixel, never resampled by the GPU: a size
//! the set lacks (20 at 1.25×) is averaged down once from the next one
//! up. Decoded once per process, off the UI thread (a worker), the
//! what drew its place drawn again as each arrives.

use olo_sync::Mutex;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, mpsc},
};

use olo_interface::{Bounds, Corners, Painter, Pixels, RenderImage, point, px, size};

/// (theme, name, device pixels).
type Key = (&'static str, String, u32);

enum Slot {
    Loading,
    Ready(Arc<RenderImage>),
    Missing,
}

#[derive(Default)]
struct Cache {
    map: HashMap<Key, Slot>,
    theme: &'static str,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// `img` as the renderer wants it (BGRA, straight alpha), shrunk once to `px` if
/// it's bigger (box-averaged: what Chrome does with icons).
fn to_render_image(img: olo_xdg::Image, px: u32) -> Option<Arc<RenderImage>> {
    let (w, h) = (img.width, img.height);
    let mut rgba = img.rgba;
    if img.premultiplied {
        for p in rgba.chunks_exact_mut(4) {
            let a = u32::from(p[3]);
            if a > 0 && a < 255 {
                for c in &mut p[..3] {
                    *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
    }
    let side = w.max(h);
    let (tw, th, data) = if side > px && px > 0 {
        let fit = |v: u32| ((v as f32 * px as f32 / side as f32).round() as u32).max(1);
        let (tw, th) = (fit(w), fit(h));
        (tw, th, shrink(&rgba, w, h, tw, th))
    } else {
        (w, h, rgba)
    };
    let mut bgra = data;
    for p in bgra.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(tw, th, bgra)?;
    Some(Arc::new(RenderImage::new([image::Frame::new(buffer)])))
}

/// Averages `src` (RGBA, w × h) down to tw × th, weighting colour by
/// alpha so edges don't darken.
fn shrink(src: &[u8], w: u32, h: u32, tw: u32, th: u32) -> Vec<u8> {
    let mut out = vec![0u8; (tw * th * 4) as usize];
    let (sx, sy) = (w as f32 / tw as f32, h as f32 / th as f32);
    for ty in 0..th {
        for tx in 0..tw {
            let (x0, x1) = ((tx as f32 * sx) as u32, (((tx + 1) as f32 * sx).ceil() as u32).min(w));
            let (y0, y1) = ((ty as f32 * sy) as u32, (((ty + 1) as f32 * sy).ceil() as u32).min(h));
            let (mut r, mut g, mut b, mut a, mut n) = (0f32, 0f32, 0f32, 0f32, 0f32);
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = ((y * w + x) * 4) as usize;
                    let al = f32::from(src[i + 3]);
                    r += f32::from(src[i]) * al;
                    g += f32::from(src[i + 1]) * al;
                    b += f32::from(src[i + 2]) * al;
                    a += al;
                    n += 1.0;
                }
            }
            let o = ((ty * tw + tx) * 4) as usize;
            if a > 0.0 {
                out[o] = (r / a).round() as u8;
                out[o + 1] = (g / a).round() as u8;
                out[o + 2] = (b / a).round() as u8;
            }
            out[o + 3] = (a / n.max(1.0)).round() as u8;
        }
    }
    out
}

fn worker() -> &'static Mutex<mpsc::Sender<Key>> {
    static WORKER: OnceLock<Mutex<mpsc::Sender<Key>>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Key>();
        std::thread::Builder::new()
            .name("olo-icons".into())
            .spawn(move || {
                for key in rx {
                    let image = olo_xdg::load_icon_in(key.0, &key.1, key.2).and_then(|img| to_render_image(img, key.2));
                    let mut c = cache().lock().unwrap();
                    c.map.insert(key, image.map_or(Slot::Missing, Slot::Ready));
                    drop(c);
                    wake();
                }
            })
            .expect("icon worker");
        Mutex::new(tx)
    })
}

/// A redraw for pictures that came is on its way to the windows.
static WAKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What was drawn waiting for a picture loaded off the looper (an icon,
/// the wordmark) is drawn again: it's ready. (Several come at once: the
/// windows are told once.)
pub(crate) fn wake() {
    use std::sync::atomic::Ordering;
    if WAKING.swap(true, Ordering::AcqRel) {
        return;
    }
    olo_app::pool::run(|| {
        // (A moment for the rest of a batch.)
        std::thread::sleep(std::time::Duration::from_millis(2));
        WAKING.store(false, Ordering::Release);
        olo_interface::all_windows(|cx| cx.repaint_unfinished());
    })
    .detach();
}

/// Icon `name` (ours, a theme icon's name, or a file's path) for drawing
/// at `px` device pixels (None while it loads, or if there's none).
fn image(name: &str, px: u32) -> Option<Arc<RenderImage>> {
    image_in(olo_xdg::themes::current().id, name, px)
}

/// [`image`] in theme `theme` (another theme's, for Preferences ▸ Themes:
/// kept until the current theme changes).
fn image_in(theme: &'static str, name: &str, px: u32) -> Option<Arc<RenderImage>> {
    let key: Key = (theme, name.to_owned(), px);
    let mut c = cache().lock().unwrap();
    let current = olo_xdg::themes::current().id;
    if c.theme != current {
        c.theme = current;
        c.map.retain(|k, _| k.0 == current);
    }
    match c.map.get(&key) {
        Some(Slot::Ready(img)) => return Some(img.clone()),
        Some(Slot::Loading | Slot::Missing) => return None,
        None => {}
    }
    c.map.insert(key.clone(), Slot::Loading);
    drop(c);
    let _ = worker().lock().unwrap().send(key);
    None
}

/// Icon `name`'s bitmap for `px` device pixels (BGRA, straight alpha),
/// if it's loaded; asked for if not (a drag's picture, asked for ahead
/// as the press that may start one comes).
pub fn bitmap(name: &str, px: u32) -> Option<Arc<RenderImage>> {
    image(name, px)
}

/// Draws icon `name` filling `b` (square), from the bitmap made for its
/// device size, a texture pixel to a device pixel, centred on whole
/// pixels. Returns false if there's no such icon (true while it loads).
/// [`paint`], see-through (`opacity`, 0 to 1): an icon of something that
/// can't be used now.
pub fn paint_faded(g: &mut Painter, b: Bounds<Pixels>, name: &str, opacity: f32) -> bool {
    g.with_element_opacity(Some(opacity), |g| paint(g, b, name))
}

pub fn paint(g: &mut Painter, b: Bounds<Pixels>, name: &str) -> bool {
    paint_with(g, b, name, None)
}

/// [`paint`] in theme `theme` (its id), whatever the current one is.
pub fn paint_in(g: &mut Painter, b: Bounds<Pixels>, theme: &str, name: &str) -> bool {
    paint_themed(g, b, olo_xdg::themes::resolve(theme).id, name, None)
}

/// [`paint`], its colours multiplied by `tint` (a selected icon darkened,
/// as BeOS's Tracker shows one), still a texture pixel to a device pixel.
pub fn paint_tinted(g: &mut Painter, b: Bounds<Pixels>, name: &str, tint: olo_interface::Hsla) -> bool {
    paint_with(g, b, name, Some(tint))
}

fn paint_with(g: &mut Painter, b: Bounds<Pixels>, name: &str, tint: Option<olo_interface::Hsla>) -> bool {
    paint_themed(g, b, olo_xdg::themes::current().id, name, tint)
}

fn paint_themed(g: &mut Painter, b: Bounds<Pixels>, theme: &'static str, name: &str, tint: Option<olo_interface::Hsla>) -> bool {
    let scale = g.scale_factor();
    let logical = f32::from(b.size.width.max(b.size.height));
    let px_size = (logical * scale).round().max(1.0) as u32;
    let Some(img) = image_in(theme, name, px_size) else {
        let c = cache().lock().unwrap();
        let loading = !matches!(c.map.get(&(theme, name.to_owned(), px_size)), Some(Slot::Missing));
        if loading {
            // (What's painted now isn't what will show: not kept.)
            g.paint_unfinished();
        }
        return loading;
    };
    let s = img.size(0);
    let (w, h) = (s.width.0 as f32 / scale, s.height.0 as f32 / scale);
    // Its own size, centred, its top left on a device pixel.
    let c = b.center();
    let x = px(((f32::from(c.x) - w / 2.0) * scale).round() / scale);
    let y = px(((f32::from(c.y) - h / 2.0) * scale).round() / scale);
    let at = Bounds { origin: point(x, y), size: size(px(w), px(h)) };
    match tint {
        None => {
            let _ = g.paint_image(at, Corners::default(), img, 0, false);
        }
        // Two triangles over the same pixels, each texel sampled at its
        // middle, so nothing is blended with its neighbours.
        Some(tint) => {
            let (tl, tr, bl, br) = (at.origin, at.top_right(), at.bottom_left(), at.bottom_right());
            let (u0, u1) = (point(0.0, 0.0), point(1.0, 1.0));
            let tris = [[(tl, u0), (tr, point(1.0, 0.0)), (br, u1)], [(tl, u0), (br, u1), (bl, point(0.0, 1.0))]];
            let _ = g.paint_image_triangles(img, &tris, tint);
        }
    }
    crate::review::icon(g, at, [s.width.0 as usize, s.height.0 as usize]);
    true
}
