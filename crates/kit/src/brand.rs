//! Olo's brand: the wordmark (`data/brand`, or the current theme's
//! `data/themes/<id>/brand`), drawn crisp at the size asked for, in its
//! light or dark version by the palette.

use std::{collections::HashMap, sync::Arc};

use egui::{Color32, ColorImage, Painter, Rect, TextureHandle, TextureOptions, pos2, vec2};

type Source = Arc<(Vec<[u8; 4]>, usize, usize)>;
/// Trimmed wordmarks by theme and light or dark (None: not found).
type Sources = HashMap<(&'static str, bool), Option<Source>>;

/// Theme `theme`'s wordmark, trimmed to what it draws: its pixels, width
/// and height.
fn source(theme: &'static str, dark: bool) -> Option<Source> {
    static CACHE: std::sync::Mutex<Option<Sources>> = std::sync::Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    cache
        .get_or_insert_default()
        .entry((theme, dark))
        .or_insert_with(|| {
            let img = olo_xdg::decode_png(&olo_xdg::themes::wordmark_file(theme, dark)?)?;
            let (w, h) = (img.width as usize, img.height as usize);
            let px: Vec<[u8; 4]> = img.rgba.as_chunks::<4>().0.to_vec();
            let alpha = |x: usize, y: usize| px[y * w + x][3] > 8;
            let cols: Vec<usize> = (0..w).filter(|&x| (0..h).any(|y| alpha(x, y))).collect();
            let rows: Vec<usize> = (0..h).filter(|&y| (0..w).any(|x| alpha(x, y))).collect();
            let (x0, x1, y0, y1) = (*cols.first()?, *cols.last()?, *rows.first()?, *rows.last()?);
            let (tw, th) = (x1 - x0 + 1, y1 - y0 + 1);
            let trimmed = (y0..=y1).flat_map(|y| px[y * w + x0..=y * w + x1].to_vec()).collect();
            Some(Arc::new((trimmed, tw, th)))
        })
        .clone()
}

/// How wide the wordmark is when `height` tall (0 if it can't be found),
/// in its light or dark version: they needn't be one shape.
pub fn wordmark_width(height: f32, dark: bool) -> f32 {
    wordmark_width_in(olo_xdg::themes::current().id, height, dark)
}

/// As [`wordmark_width`], for theme `theme`'s wordmark.
pub fn wordmark_width_in(theme: &str, height: f32, dark: bool) -> f32 {
    source(olo_xdg::themes::resolve(theme).id, dark).map_or(0.0, |s| height * s.1 as f32 / s.2 as f32)
}

/// Draw the wordmark `rect.height()` tall at `rect`'s left, centred
/// vertically; the rect it took, or None if it can't be found.
pub fn wordmark(painter: &Painter, rect: Rect, dark: bool) -> Option<Rect> {
    wordmark_in(painter, rect, dark, olo_xdg::themes::current().id)
}

/// Textures by theme, background and device height, and the theme they
/// were last asked for in (when that changes, the others go).
#[derive(Default)]
struct Textures {
    current: &'static str,
    map: HashMap<(&'static str, bool, usize), TextureHandle>,
}

/// As [`wordmark`], theme `theme`'s.
pub fn wordmark_in(painter: &Painter, rect: Rect, dark: bool, theme: &str) -> Option<Rect> {
    draw(painter, rect, dark, theme, false)
}

/// As [`wordmark_in`], centred across `rect` once its width is whole
/// device pixels, so wordmarks of different widths in a column share
/// one centre at every scale.
pub fn wordmark_centred_in(painter: &Painter, rect: Rect, dark: bool, theme: &str) -> Option<Rect> {
    draw(painter, rect, dark, theme, true)
}

fn draw(painter: &Painter, rect: Rect, dark: bool, theme: &str, centred: bool) -> Option<Rect> {
    let ctx = painter.ctx();
    let ppp = ctx.pixels_per_point();
    let theme = olo_xdg::themes::resolve(theme).id;
    let src = source(theme, dark)?;
    let th = (rect.height() * ppp).round().max(1.0) as usize;
    let tw = ((th as f32) * src.1 as f32 / src.2 as f32).round().max(1.0) as usize;
    let cache_id = egui::Id::unique("olo-brand-textures");
    let cache = ctx.data_mut(|d| d.get_temp_mut_or_default::<Arc<std::sync::Mutex<Textures>>>(cache_id).clone());
    let mut cache = cache.lock().unwrap();
    let now = olo_xdg::themes::current().id;
    if cache.current != now {
        cache.current = now;
        cache.map.retain(|k, _| k.0 == now);
    }
    let tex = cache
        .map
        .entry((theme, dark, th))
        .or_insert_with(|| ctx.load_texture(format!("wordmark:{theme}@{th}{}", if dark { "-dark" } else { "" }), shrink(&src.0, src.1, src.2, tw, th), TextureOptions::LINEAR))
        .clone();
    drop(cache);
    let size = vec2(tw as f32, th as f32) / ppp;
    let top = ((rect.center().y - size.y / 2.0) * ppp).round() / ppp;
    let left = if centred { (rect.center().x * ppp - tw as f32 / 2.0).round() / ppp } else { (rect.left() * ppp).round() / ppp };
    let at = Rect::from_min_size(pos2(left, top), size);
    painter.image(tex.id(), at, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    Some(at)
}

/// `px` (w×h, straight alpha) averaged down to tw×th, each output pixel
/// the alpha-weighted mean of the source area it covers.
pub(crate) fn shrink(px: &[[u8; 4]], w: usize, h: usize, tw: usize, th: usize) -> ColorImage {
    let (sx, sy) = (w as f32 / tw as f32, h as f32 / th as f32);
    let mut out = Vec::with_capacity(tw * th);
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
                    let p = px[y as usize * w + x as usize];
                    let a = p[3] as f32 / 255.0 * wx * wy;
                    for c in 0..3 {
                        acc[c] += p[c] as f32 * a;
                    }
                    acc[3] += a;
                    area += wx * wy;
                    x += 1.0;
                }
                y += 1.0;
            }
            let alpha = if area > 0.0 { acc[3] / area } else { 0.0 };
            let c = |i: usize| if acc[3] > 0.0 { (acc[i] / acc[3]).round() as u8 } else { 0 };
            out.push(Color32::from_rgba_unmultiplied(c(0), c(1), c(2), (alpha * 255.0).round() as u8));
        }
    }
    ColorImage::new([tw, th], out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrinking_keeps_colour_under_soft_edges() {
        // Half opaque red, half clear: the edge stays red, half as strong.
        let px = [[255, 0, 0, 255], [0, 0, 255, 0], [255, 0, 0, 255], [0, 0, 255, 0]];
        let img = shrink(&px, 2, 2, 1, 1);
        let c = img.pixels[0].to_srgba_unmultiplied();
        assert_eq!((c[0], c[2]), (255, 0));
        assert!((126..=129).contains(&c[3]), "{c:?}");
    }

    #[test]
    fn the_wordmark_is_shipped() {
        // From the repository's data directory, as in development.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        for file in ["brand/olo-wordmark.png", "brand/olo-wordmark-on-dark.png"] {
            let img = olo_xdg::decode_png(&dir.join(file)).expect("the wordmark");
            assert!(img.width > img.height && img.height >= 128);
        }
    }
}
