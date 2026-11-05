//! Gain maps: an SDR picture's HDR version, as how many stops brighter each
//! point of it is on a display in HDR (Ben, 2026-11-05: "generate and add
//! hdr versions of all the olo wallpapers that are used when in hdr mode").
//! The picture stays as it is (what SDR shows); beside it a greyscale
//! picture at half its size, 0 for SDR's light and 255 for `MAX_STOPS`
//! more, as Ultra HDR and Apple's HDR photos keep theirs. Kept small (a
//! 4K wallpaper's is a few hundred kilobytes, against tens of megabytes
//! for a 16-bit HDR copy) and relative to SDR's white, so "Everything
//! else's brightness" moves the whole picture with no new texture.
//!
//! [`make`] finds what should shine in a picture made for SDR: points
//! that stand out from what's around them (light sources, lit LEDs, the
//! moon, glints and speculars) shine most; the near-white around a sun,
//! somewhat; a large bright surface (a yellow cube's face, a sky) stays
//! at SDR's white or close, so nothing glares. [`apply`] makes the HDR
//! picture from the SDR one and its map, as half floats of linear light
//! (1.0 SDR's white), what app_server's wallpaper draws in HDR.

use std::path::{Path, PathBuf};

use image::{GrayImage, RgbaImage};

/// The most a gain map brightens: two stops, 4× SDR's white (on the
/// ThinkPad's OLED at 400 cd/m² for SDR, its 1,600 cd/m² peak).
pub const MAX_STOPS: f32 = 2.0;

/// Where `picture`'s gain map is kept: beside it, `NAME.gain.png`.
pub fn path_for(picture: &Path) -> PathBuf {
    picture.with_extension("gain.png")
}

/// The gain map `picture` has, if it has one.
pub fn find(picture: &Path) -> Option<PathBuf> {
    let p = path_for(picture);
    p.is_file().then_some(p)
}

/// Gamma 2.2 (sRGB as a display shows it) to linear light.
fn linear(v: f32) -> f32 {
    v.max(0.0).powf(2.2)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A box blur of radius `r`, across then down, `passes` times (three make
/// a Gaussian's shape), in place.
fn box_blur(v: &mut [f32], w: usize, h: usize, r: usize, passes: usize) {
    if r == 0 || w == 0 || h == 0 {
        return;
    }
    let mut line = vec![0.0f32; w.max(h)];
    let mut pass = |v: &mut [f32], n: usize, count: usize, at: &dyn Fn(usize, usize) -> usize| {
        for k in 0..count {
            let mut sum = 0.0f32;
            // A window of 2r+1, the edges repeated.
            let get = |v: &[f32], i: isize| v[at(k, i.clamp(0, n as isize - 1) as usize)];
            for i in -(r as isize)..=(r as isize) {
                sum += get(v, i);
            }
            for i in 0..n {
                line[i] = sum / (2 * r + 1) as f32;
                sum += get(v, i as isize + r as isize + 1) - get(v, i as isize - r as isize);
            }
            for i in 0..n {
                v[at(k, i)] = line[i];
            }
        }
    };
    for _ in 0..passes {
        pass(v, w, h, &|row, x| row * w + x);
        pass(v, h, w, &|col, y| y * w + col);
    }
}

/// The gain map for SDR picture `rgba` (straight alpha, opaque or not), at
/// half its size.
pub fn make(rgba: &RgbaImage) -> GrayImage {
    let (sw, sh) = (rgba.width() as usize, rgba.height() as usize);
    let (w, h) = ((sw / 2).max(1), (sh / 2).max(1));
    // At half size: each point the mean of its four in light, and the
    // brightest channel (a yellow LED is bright in red and green, not in
    // luminance alone), and luminance encoded again for thresholds.
    let mut lum = vec![0.0f32; w * h];
    let mut most = vec![0.0f32; w * h];
    let mut lum_enc = vec![0.0f32; w * h];
    let table: Vec<f32> = (0..256).map(|v| linear(v as f32 / 255.0)).collect();
    for y in 0..h {
        for x in 0..w {
            let (mut r, mut g, mut b) = (0.0f32, 0.0f32, 0.0f32);
            let mut n = 0.0;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (px, py) = ((2 * x + dx).min(sw - 1), (2 * y + dy).min(sh - 1));
                let p = rgba.get_pixel(px as u32, py as u32).0;
                r += table[p[0] as usize];
                g += table[p[1] as usize];
                b += table[p[2] as usize];
                n += 1.0;
            }
            let (r, g, b) = (r / n, g / n, b / n);
            let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            lum[y * w + x] = l;
            most[y * w + x] = r.max(g).max(b);
            lum_enc[y * w + x] = l.powf(1.0 / 2.2);
        }
    }
    // What's around each point: its light blurred over about 1.5% of the
    // picture's width.
    let mut around = lum.clone();
    box_blur(&mut around, w, h, (w as f32 * 0.015 / 1.7).round().max(1.0) as usize, 3);
    let mut stops = vec![0.0f32; w * h];
    for i in 0..w * h {
        // Standing out: a point much brighter than what's around it.
        let stands_out = smoothstep(1.15, 2.2, lum[i] / (around[i] + 0.01));
        let bright = smoothstep(0.55, 0.92, most[i].powf(1.0 / 2.2));
        // Near-white: a sun's glow, a lamp's halo; a plain bright area a
        // little only.
        let white = smoothstep(0.90, 1.0, lum_enc[i]);
        let pale = smoothstep(0.75, 0.92, lum_enc[i]);
        stops[i] = (0.85 * bright * stands_out + 0.5 * white + 0.12 * pale).min(1.0);
    }
    // Soft edges (a map at half size is drawn larger, and a hard step
    // would show as one), once: more would dim a small light.
    box_blur(&mut stops, w, h, 1, 1);
    GrayImage::from_fn(w as u32, h as u32, |x, y| image::Luma([(stops[y as usize * w + x as usize] * 255.0).round().clamp(0.0, 255.0) as u8]))
}

/// The HDR picture from SDR picture `rgba` (premultiplied, as the renderer
/// wants it) and `gain` (its gain map at the same size): RGBA half floats
/// of linear light, 1.0 SDR's white, the picture's own primaries.
pub fn apply(rgba: &[u8], gain: &[u8]) -> Vec<u16> {
    // Every channel value at every gain: 64K half floats, looked up.
    let mut table = vec![0u16; 256 * 256];
    for g in 0..256 {
        let k = 2f32.powf(g as f32 / 255.0 * MAX_STOPS);
        for c in 0..256 {
            table[g * 256 + c] = half::f16::from_f32(linear(c as f32 / 255.0) * k).to_bits();
        }
    }
    let one = half::f16::ONE.to_bits();
    let mut out = Vec::with_capacity(rgba.len());
    for (p, &g) in rgba.chunks_exact(4).zip(gain) {
        let row = &table[g as usize * 256..][..256];
        let a = p[3];
        if a == 255 {
            out.extend_from_slice(&[row[p[0] as usize], row[p[1] as usize], row[p[2] as usize], one]);
        } else {
            // Premultiplied: light scaled by coverage, as the 8-bit one.
            let k = a as f32 / 255.0;
            let lift = 2f32.powf(g as f32 / 255.0 * MAX_STOPS);
            let unpremul = |c: u8| if a == 0 { 0.0 } else { (c as f32 / k / 255.0).min(1.0) };
            let f = |c: u8| half::f16::from_f32(linear(unpremul(c)) * lift * k).to_bits();
            out.extend_from_slice(&[f(p[0]), f(p[1]), f(p[2]), half::f16::from_f32(k).to_bits()]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dark scene with a small light in it, a large plain bright area,
    /// and a white glow.
    fn scene() -> RgbaImage {
        RgbaImage::from_fn(400, 200, |x, y| {
            let px = |r, g, b| image::Rgba([r, g, b, 255]);
            if (100..106).contains(&x) && (100..106).contains(&y) {
                px(255, 220, 40) // a lit yellow LED
            } else if x >= 250 && x < 380 {
                px(240, 200, 30) // a large yellow face
            } else if x < 40 {
                px(252, 252, 250) // near-white glow
            } else {
                px(20, 24, 40) // the night
            }
        })
    }

    #[test]
    fn what_stands_out_shines_and_plain_bright_areas_dont() {
        let map = make(&scene());
        assert_eq!(map.dimensions(), (200, 100));
        let at = |x: u32, y: u32| map.get_pixel(x / 2, y / 2)[0];
        assert!(at(103, 103) > 150, "the LED: {}", at(103, 103));
        assert!(at(320, 100) < 40, "the large face: {}", at(320, 100));
        assert!(at(10, 100) > 90, "the white glow: {}", at(10, 100));
        assert!(at(200, 40) < 5, "the night: {}", at(200, 40));
    }

    #[test]
    fn applied_white_is_sdr_white_and_full_gain_four_times_it() {
        let px = [255u8, 255, 255, 255, 255, 255, 255, 255];
        let out = apply(&px, &[0, 255]);
        let f = |i: usize| half::f16::from_bits(out[i]).to_f32();
        assert!((f(0) - 1.0).abs() < 1e-3 && (f(3) - 1.0).abs() < 1e-3);
        assert!((f(4) - 4.0).abs() < 1e-2, "{}", f(4));
        // A mid grey is gamma 2.2's light.
        let g = apply(&[128, 128, 128, 255], &[0]);
        assert!((half::f16::from_bits(g[0]).to_f32() - (128.0f32 / 255.0).powf(2.2)).abs() < 1e-3);
    }
}
