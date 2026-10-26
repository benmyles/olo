//! Tab icons from the page's favicon, which Servo hands over decoded.
//! Icons come in any size (16 to 512 px and up), so we shrink them on the
//! CPU to one 32 px square (16 points at 2×) with an area average, which
//! keeps them crisp where the GPU's bilinear minification would alias;
//! small ones are doubled pixel for pixel.

use servo::{Image, PixelFormat};
use tabula_kit::egui::{Color32, ColorImage};

/// The square tab icons are made at, in pixels.
pub const SIZE: usize = 32;

/// The favicon as a `SIZE` square, centred, its aspect kept.
pub fn square(image: &Image) -> Option<ColorImage> {
    let (w, h) = (image.width as usize, image.height as usize);
    let data = image.data();
    let px = |i: usize| -> [u8; 4] {
        match image.format {
            PixelFormat::K8 => [data[i], data[i], data[i], 255],
            PixelFormat::KA8 => [data[2 * i], data[2 * i], data[2 * i], data[2 * i + 1]],
            PixelFormat::RGB8 => [data[3 * i], data[3 * i + 1], data[3 * i + 2], 255],
            PixelFormat::RGBA8 => [data[4 * i], data[4 * i + 1], data[4 * i + 2], data[4 * i + 3]],
            PixelFormat::BGRA8 => [data[4 * i + 2], data[4 * i + 1], data[4 * i], data[4 * i + 3]],
        }
    };
    let bytes = match image.format {
        PixelFormat::K8 => 1,
        PixelFormat::KA8 => 2,
        PixelFormat::RGB8 => 3,
        PixelFormat::RGBA8 | PixelFormat::BGRA8 => 4,
    };
    if w == 0 || h == 0 || data.len() < w * h * bytes {
        return None;
    }
    let rgba: Vec<[u8; 4]> = (0..w * h).map(px).collect();
    Some(fit(w, h, &rgba))
}

/// Scale `rgba` (`w`×`h`) into a centred `SIZE` square: an area average
/// going down, nearest pixels going up.
fn fit(w: usize, h: usize, rgba: &[[u8; 4]]) -> ColorImage {
    let scale = SIZE as f32 / w.max(h) as f32;
    let (tw, th) = (((w as f32 * scale).round() as usize).max(1), ((h as f32 * scale).round() as usize).max(1));
    let (ox, oy) = ((SIZE - tw) / 2, (SIZE - th) / 2);
    let mut out = ColorImage::filled([SIZE, SIZE], Color32::TRANSPARENT);
    for ty in 0..th {
        let y0 = ty * h / th;
        let y1 = ((ty + 1) * h / th).max(y0 + 1);
        for tx in 0..tw {
            let x0 = tx * w / tw;
            let x1 = ((tx + 1) * w / tw).max(x0 + 1);
            // Premultiplied, so transparent pixels don't darken the edges.
            let mut sum = [0u32; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let [r, g, b, a] = rgba[y * w + x];
                    let a32 = a as u32;
                    sum[0] += r as u32 * a32;
                    sum[1] += g as u32 * a32;
                    sum[2] += b as u32 * a32;
                    sum[3] += a32;
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let a = sum[3] / n;
            let ch = |s: u32| (s / 255 / n) as u8;
            out[(ox + tx, oy + ty)] = Color32::from_rgba_premultiplied(ch(sum[0]), ch(sum[1]), ch(sum[2]), a as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrinks_and_centres() {
        // 64×32 opaque red: 32×16 in the middle of the square.
        let img = fit(64, 32, &vec![[255, 0, 0, 255]; 64 * 32]);
        assert_eq!(img.size, [SIZE, SIZE]);
        assert_eq!(img[(0, 7)], Color32::TRANSPARENT);
        assert_eq!(img[(0, 8)], Color32::from_rgb(255, 0, 0));
        assert_eq!(img[(31, 23)], Color32::from_rgb(255, 0, 0));
        assert_eq!(img[(31, 24)], Color32::TRANSPARENT);
        // White and clear columns average to half-alpha white, not grey.
        let stripes: Vec<[u8; 4]> = (0..64 * 64).map(|i| if i % 2 == 0 { [255, 255, 255, 255] } else { [0, 0, 0, 0] }).collect();
        let img = fit(64, 64, &stripes);
        assert_eq!(img[(5, 5)], Color32::from_rgba_premultiplied(127, 127, 127, 127));
    }

    #[test]
    fn doubles_small_icons() {
        let mut px = vec![[0, 0, 255, 255]; 16 * 16];
        px[0] = [255, 0, 0, 255];
        let img = fit(16, 16, &px);
        assert_eq!(img[(1, 1)], Color32::from_rgb(255, 0, 0));
        assert_eq!(img[(2, 2)], Color32::from_rgb(0, 0, 255));
    }
}
