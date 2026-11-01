//! Icons: Olo's own set (data/icons/<px>/<name>.png, pre-scaled to device
//! sizes, in the theme chosen in Preferences ▸ Themes), with a
//! freedesktop theme's for other programs and file types. Each is drawn
//! from a bitmap made for exactly the device size it's shown at, a
//! texture pixel to a device pixel, never resampled by the GPU: a size
//! the set lacks (20 at 1.25×) is averaged down once from the next one
//! up. Decoded once per process, off the UI thread (a worker), the
//! windows drawn again as each arrives; until then its place is kept.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, mpsc},
};

use gpui::{App, Bounds, Corners, Pixels, RenderImage, Window, point, px, size};

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

/// `img` as GPUI wants it (BGRA, straight alpha), shrunk once to `px` if
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
                    if let Some(wake) = WAKE.get() {
                        wake();
                    }
                }
            })
            .expect("icon worker");
        Mutex::new(tx)
    })
}

/// Draws the windows again once icons arrive (set by `init`).
static WAKE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

pub(crate) fn init(cx: &mut App) {
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
    let _ = WAKE.set(Box::new(move || {
        let _ = tx.unbounded_send(());
    }));
    cx.spawn(async move |cx| {
        use futures::StreamExt as _;
        while rx.next().await.is_some() {
            // (Several at once: drawn again once.)
            while rx.try_recv().is_ok() {}
            let _ = cx.update(|cx| cx.refresh_windows());
        }
    })
    .detach();
}

/// Icon `name` (ours, a theme icon's name, or a file's path) for drawing
/// at `px` device pixels (None while it loads, or if there's none).
fn image(name: &str, px: u32) -> Option<Arc<RenderImage>> {
    let theme = olo_xdg::themes::current().id;
    let key: Key = (theme, name.to_owned(), px);
    let mut c = cache().lock().unwrap();
    if c.theme != theme {
        c.theme = theme;
        c.map.retain(|k, _| k.0 == theme);
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

/// Draws icon `name` filling `b` (square), from the bitmap made for its
/// device size, a texture pixel to a device pixel, centred on whole
/// pixels. Returns false if there's no such icon (true while it loads).
pub fn paint(window: &mut Window, b: Bounds<Pixels>, name: &str) -> bool {
    let scale = window.scale_factor();
    let logical = f32::from(b.size.width.max(b.size.height));
    let px_size = (logical * scale).round().max(1.0) as u32;
    let Some(img) = image(name, px_size) else {
        let c = cache().lock().unwrap();
        return !matches!(c.map.get(&(olo_xdg::themes::current().id, name.to_owned(), px_size)), Some(Slot::Missing));
    };
    let s = img.size(0);
    let (w, h) = (s.width.0 as f32 / scale, s.height.0 as f32 / scale);
    // Its own size, centred, its top left on a device pixel.
    let c = b.center();
    let x = px(((f32::from(c.x) - w / 2.0) * scale).round() / scale);
    let y = px(((f32::from(c.y) - h / 2.0) * scale).round() / scale);
    let at = Bounds { origin: point(x, y), size: size(px(w), px(h)) };
    let _ = window.paint_image(at, Corners::default(), img, 0, false);
    crate::review::icon(window, at, [s.width.0 as usize, s.height.0 as usize]);
    true
}
