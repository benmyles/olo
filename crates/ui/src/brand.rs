//! Olo's brand: the wordmark (`data/brand`, or the current theme's
//! `data/themes/<id>/brand`), drawn crisp at the size asked for, in its
//! light or dark version by the palette: the
//! source decoded once, off the UI thread, and trimmed to what it draws;
//! each size averaged down from it once, a texel to a device pixel.

use olo_sync::Mutex;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use gpui::{Bounds, Corners, Pixels, RenderImage, Window, point, px, size};

/// The trimmed wordmark: straight-alpha RGBA, its width and height.
type Source = Arc<(Vec<u8>, usize, usize)>;

enum Slot {
    Loading,
    Ready(Source),
    Missing,
}

#[derive(Default)]
struct Cache {
    sources: HashMap<(&'static str, bool), Slot>,
    /// Pictures by theme, background and device height, and the theme
    /// they were last asked for in (when that changes, the others go).
    current: &'static str,
    images: HashMap<(&'static str, bool, u32), Arc<RenderImage>>,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Theme `theme`'s wordmark, if it's loaded (asked for if not).
fn source(theme: &'static str, dark: bool) -> Option<Result<Source, ()>> {
    let mut c = cache().lock().unwrap();
    match c.sources.get(&(theme, dark)) {
        Some(Slot::Ready(s)) => return Some(Ok(s.clone())),
        Some(Slot::Missing) => return Some(Err(())),
        Some(Slot::Loading) => return None,
        None => {}
    }
    c.sources.insert((theme, dark), Slot::Loading);
    drop(c);
    std::thread::spawn(move || {
        let slot = load(theme, dark).map_or(Slot::Missing, |s| Slot::Ready(Arc::new(s)));
        cache().lock().unwrap().sources.insert((theme, dark), slot);
        crate::icons::wake();
    });
    None
}

/// Decodes theme `theme`'s wordmark and trims it to what it draws.
fn load(theme: &'static str, dark: bool) -> Option<(Vec<u8>, usize, usize)> {
    let img = olo_xdg::decode_png(&olo_xdg::themes::wordmark_file(theme, dark)?)?;
    let (w, h) = (img.width as usize, img.height as usize);
    let px = &img.rgba;
    let alpha = |x: usize, y: usize| px[(y * w + x) * 4 + 3] > 8;
    let cols: Vec<usize> = (0..w).filter(|&x| (0..h).any(|y| alpha(x, y))).collect();
    let rows: Vec<usize> = (0..h).filter(|&y| (0..w).any(|x| alpha(x, y))).collect();
    let (x0, x1, y0, y1) = (*cols.first()?, *cols.last()?, *rows.first()?, *rows.last()?);
    let (tw, th) = (x1 - x0 + 1, y1 - y0 + 1);
    let trimmed = (y0..=y1).flat_map(|y| px[(y * w + x0) * 4..(y * w + x1 + 1) * 4].to_vec()).collect();
    Some((trimmed, tw, th))
}

/// How wide the wordmark is when `height` tall, in its light or dark
/// version (they needn't be one shape); None while it loads or if there
/// is none.
pub fn wordmark_width(height: f32, dark: bool) -> Option<f32> {
    wordmark_width_in(olo_xdg::themes::current().id, height, dark)
}

/// [`wordmark_width`] for theme `theme` (its id).
pub fn wordmark_width_in(theme: &str, height: f32, dark: bool) -> Option<f32> {
    let theme = olo_xdg::themes::resolve(theme).id;
    source(theme, dark)?.ok().map(|s| height * s.1 as f32 / s.2 as f32)
}

/// What drawing the wordmark did.
pub enum Drawn {
    /// It's drawn here.
    At(Bounds<Pixels>),
    /// It's loading: its place is kept.
    Loading,
    /// There is none (draw the name instead).
    Missing,
}

/// Draws the wordmark `b.size.height` tall, centred in `b` once its
/// width is whole device pixels, its top on a device pixel.
pub fn wordmark_centred(window: &mut Window, b: Bounds<Pixels>, dark: bool) -> Drawn {
    wordmark_centred_in(window, b, dark, olo_xdg::themes::current().id)
}

/// [`wordmark_centred`] in theme `theme` (its id): Preferences ▸ Themes
/// shows each theme's.
pub fn wordmark_centred_in(window: &mut Window, b: Bounds<Pixels>, dark: bool, theme: &str) -> Drawn {
    wordmark_in(window, b, dark, theme, true)
}

/// [`wordmark_centred`], but at `b`'s left (on the pixel text there
/// starts on), centred only up and down: over a line of text that starts
/// where it does (About's version: centred, at 1.25× it started a pixel
/// left of the line under it).
pub fn wordmark_left(window: &mut Window, b: Bounds<Pixels>, dark: bool) -> Drawn {
    wordmark_in(window, b, dark, olo_xdg::themes::current().id, false)
}

fn wordmark_in(window: &mut Window, b: Bounds<Pixels>, dark: bool, theme: &str, centred: bool) -> Drawn {
    let theme = olo_xdg::themes::resolve(theme).id;
    let src = match source(theme, dark) {
        None => {
            // (What's painted now isn't what will show: not kept.)
            window.paint_unfinished();
            return Drawn::Loading;
        }
        Some(Err(())) => return Drawn::Missing,
        Some(Ok(s)) => s,
    };
    let scale = window.scale_factor();
    let th = (f32::from(b.size.height) * scale).round().max(1.0) as u32;
    let tw = ((th as f32) * src.1 as f32 / src.2 as f32).round().max(1.0) as u32;
    let image = {
        let mut c = cache().lock().unwrap();
        let current = olo_xdg::themes::current().id;
        if c.current != current {
            c.current = current;
            c.images.retain(|k, _| k.0 == current);
        }
        c.images.entry((theme, dark, th)).or_insert_with(|| picture(&src, tw, th)).clone()
    };
    let (w, h) = (tw as f32 / scale, th as f32 / scale);
    let c = b.center();
    let left = if centred { ((f32::from(c.x) * scale - tw as f32 / 2.0).round()) / scale } else { (f32::from(b.left()) * scale).round() / scale };
    let top = ((f32::from(c.y) - h / 2.0) * scale).round() / scale;
    let at = Bounds { origin: point(px(left), px(top)), size: size(px(w), px(h)) };
    let _ = window.paint_image(at, Corners::default(), image, 0, false);
    crate::review::icon(window, at, [tw as usize, th as usize]);
    Drawn::At(at)
}

/// The wordmark averaged down to `tw` × `th` (each output pixel the
/// alpha-weighted mean of the source area it covers), as GPUI wants it
/// (BGRA, straight alpha).
fn picture(src: &Source, tw: u32, th: u32) -> Arc<RenderImage> {
    let mut out = shrink(&src.0, src.1, src.2, tw as usize, th as usize);
    for p in out.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(tw, th, out).expect("a buffer of its size");
    Arc::new(RenderImage::new([image::Frame::new(buffer)]))
}

/// `px` (w×h RGBA, straight alpha) averaged down to tw×th.
fn shrink(px: &[u8], w: usize, h: usize, tw: usize, th: usize) -> Vec<u8> {
    let (sx, sy) = (w as f32 / tw as f32, h as f32 / th as f32);
    let mut out = Vec::with_capacity(tw * th * 4);
    for oy in 0..th {
        let (y0, y1) = (oy as f32 * sy, ((oy + 1) as f32 * sy).min(h as f32));
        for ox in 0..tw {
            let (x0, x1) = (ox as f32 * sx, ((ox + 1) as f32 * sx).min(w as f32));
            let mut acc = [0f32; 4];
            let mut area = 0f32;
            let mut y = y0.floor();
            while y < y1 {
                let wy = (y + 1.0).min(y1) - y.max(y0);
                let mut x = x0.floor();
                while x < x1 {
                    let wx = (x + 1.0).min(x1) - x.max(x0);
                    let i = (y as usize * w + x as usize) * 4;
                    let a = px[i + 3] as f32 / 255.0 * wx * wy;
                    for c in 0..3 {
                        acc[c] += px[i + c] as f32 * a;
                    }
                    acc[3] += a;
                    area += wx * wy;
                    x += 1.0;
                }
                y += 1.0;
            }
            let alpha = if area > 0.0 { acc[3] / area } else { 0.0 };
            let c = |i: usize| if acc[3] > 0.0 { (acc[i] / acc[3]).round() as u8 } else { 0 };
            out.extend([c(0), c(1), c(2), (alpha * 255.0).round() as u8]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrinking_keeps_colour_under_soft_edges() {
        // Half opaque red, half clear: the edge stays red, half as strong.
        let px = [255, 0, 0, 255, 0, 0, 255, 0, 255, 0, 0, 255, 0, 0, 255, 0];
        let out = shrink(&px, 2, 2, 1, 1);
        assert_eq!((out[0], out[2]), (255, 0));
        assert!((126..=129).contains(&out[3]), "{out:?}");
    }
}
