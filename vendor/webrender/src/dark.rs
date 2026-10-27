/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Dark pages (Tabula's patch): while on, the scene builder recolours
//! each display item by what it is, as Chrome's "Auto Dark Mode for Web
//! Contents" does when painting: light backgrounds go dark, dark text and
//! lines go light, by turning their CIELAB lightness around (L to 110 − L)
//! and keeping hue and saturation. Colours already on the right side are
//! left alone, so a page that is dark by itself stays as it is, and
//! pictures and video are never touched.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

use api::ColorF;

static ON: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU32 = AtomicU32::new(0);

/// Turn dark pages on or off for every document in this process. Scenes
/// built from then on use it; a document shows it once its display list
/// is next built.
pub fn set(on: bool) {
    if ON.swap(on, Ordering::AcqRel) != on {
        GENERATION.fetch_add(1, Ordering::AcqRel);
    }
}

pub fn on() -> bool {
    ON.load(Ordering::Acquire)
}

/// Bumped by every change of [`set`].
pub fn generation() -> u32 {
    GENERATION.load(Ordering::Acquire)
}

/// What a colour paints, which decides whether it changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// Rectangles and gradients: darkened if light.
    Background,
    /// Text, text decorations: lightened if dark.
    Foreground,
    /// Borders and rules: darkened if light (a hairline on a white page
    /// stays a faint line on the dark one), lightened if very dark.
    Border,
}

/// Perceived brightness, 0–255 (Chrome's: the W3C formula).
fn brightness(c: &ColorF) -> f32 {
    (c.r * 299.0 + c.g * 587.0 + c.b * 114.0) / 1000.0 * 255.0
}

/// Whether `c`, painted as `role`, changes.
fn changes(c: &ColorF, role: Role) -> bool {
    if c.a == 0.0 {
        return false;
    }
    let b = brightness(c);
    match role {
        Role::Background => b > 128.0,
        Role::Foreground => b < 150.0,
        Role::Border => b > 128.0 || b < 40.0,
    }
}

/// `c` as it is painted now.
pub fn color(c: ColorF, role: Role) -> ColorF {
    if !on() || !changes(&c, role) {
        return c;
    }
    let key = (c.to_array().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8), role as u8);
    static CACHE: Mutex<Option<HashMap<([u8; 4], u8), ColorF>>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some(found) = cache.get(&key) {
        return *found;
    }
    let out = invert_lightness(c);
    if cache.len() > 4096 {
        cache.clear();
    }
    cache.insert(key, out);
    out
}

/// L to min(110 − L, 100) in CIELAB, hue and chroma kept; dark greys
/// just short of black go to Chrome's 18/255 so backgrounds match.
pub fn invert_lightness(c: ColorF) -> ColorF {
    let (l, a, b) = srgb_to_lab(c.r, c.g, c.b);
    let (r, g, bl) = lab_to_srgb((110.0 - l).min(100.0), a, b);
    let (r, g, bl) = (r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), bl.clamp(0.0, 1.0));
    const LOW: f32 = 18.0 / 255.0;
    const HIGH: f32 = 32.0 / 255.0;
    let grey = (r - g).abs() < 1.0 / 512.0 && (r - bl).abs() < 1.0 / 512.0;
    if grey && r < HIGH && r > LOW {
        return ColorF::new(LOW, LOW, LOW, c.a);
    }
    ColorF::new(r, g, bl, c.a)
}

/// Pictures this size or smaller (device pixels, each way) may be icons.
pub const ICON_SIZE: i32 = 160;

/// Whether a picture (BGRA, or RGBA if not `bgra`; premultiplied) is an
/// icon to turn around:
/// Chrome's decision tree (`DarkModeImageClassifier`) over up to 1000
/// samples. Few colours (a black glyph, a grey pictogram) turn around;
/// light ones on transparency, colourful ones and photographs don't.
/// Pictures it can't decide on are left alone.
pub fn is_icon(data: &[u8], bgra: bool, width: i32, height: i32, stride: i32) -> bool {
    if width <= 0 || height <= 0 || width > ICON_SIZE || height > ICON_SIZE {
        return false;
    }
    let pixel = |x: i32, y: i32| -> Option<[u8; 4]> {
        let i = (y * stride + x * 4) as usize;
        let p = data.get(i..i + 4)?;
        let a = p[3];
        let un = |v: u8| if a == 0 { 0 } else { ((v as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
        Some(if bgra { [un(p[2]), un(p[1]), un(p[0]), a] } else { [un(p[0]), un(p[1]), un(p[2]), a] })
    };
    // A grid of blocks, sampled evenly.
    let blocks_x = width.min(10);
    let blocks_y = height.min(10);
    let samples = (width * height).min(1000);
    let per_block = (samples / (blocks_x * blocks_y)).max(1);
    let step = (per_block as f32).sqrt();
    let mut opaque = Vec::new();
    let mut transparent = 0usize;
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let (x0, x1) = (width * bx / blocks_x, width * (bx + 1) / blocks_x);
            let (y0, y1) = (height * by / blocks_y, height * (by + 1) / blocks_y);
            let cx = (((x1 - x0) as f32 / step).ceil() as i32).max(1);
            let cy = (((y1 - y0) as f32 / step).ceil() as i32).max(1);
            let mut y = y0;
            while y < y1 {
                let mut x = x0;
                while x < x1 {
                    match pixel(x, y) {
                        Some(p) if p[3] >= 128 => opaque.push(p),
                        Some(_) => transparent += 1,
                        None => return false,
                    }
                    x += cx;
                }
                y += cy;
            }
        }
    }
    if opaque.is_empty() {
        return false;
    }
    let n = opaque.len() as f32;
    let chroma = |p: &[u8; 4]| p[..3].iter().max().unwrap() - p[..3].iter().min().unwrap();
    let grey = |p: &[u8; 4]| (p[0] as i32 - p[1] as i32).abs() + (p[1] as i32 - p[2] as i32).abs() <= 8;
    let light = |p: &[u8; 4]| (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000 >= 96;
    let colourful = opaque.iter().filter(|p| !grey(p)).count() > opaque.len() / 100;
    let mut buckets = std::collections::HashSet::new();
    for p in &opaque {
        buckets.insert(if colourful {
            ((p[0] as u16 >> 4) << 8) + ((p[1] as u16 >> 4) << 4) + (p[2] as u16 >> 4)
        } else {
            (p[0] as u16 * 5 + p[1] as u16 * 3 + p[2] as u16 * 2) / 10 / 16
        });
    }
    let buckets = buckets.len() as f32 / if colourful { 4096.0 } else { 16.0 };
    let transparency = transparent as f32 / (transparent as f32 + n);
    let high_luminance = opaque.iter().filter(|p| light(p)).count() as f32 / n;
    let saturated = opaque.iter().filter(|p| chroma(p) >= 80).count() as f32 / n;
    let chromatic = opaque.iter().filter(|p| chroma(p) >= 20).count() as f32 / n;
    let (low, high) = if colourful { (0.015137, 0.025635) } else { (0.8125, 1.0) };
    if transparency > 0.4 && high_luminance > 0.5 {
        return false;
    }
    if colourful && buckets < high && (saturated > 0.3 || (high_luminance > 0.5 && saturated > 0.1) || chromatic > 0.5) {
        return false;
    }
    buckets < low
}

/// A picture (as for [`is_icon`]) with every pixel's lightness turned
/// around, as [`invert_lightness`] does to colours.
pub fn invert_image(data: &[u8], bgra: bool) -> Vec<u8> {
    let (r, b) = if bgra { (2, 0) } else { (0, 2) };
    let mut seen: HashMap<[u8; 4], [u8; 4]> = HashMap::new();
    let mut out = data.to_vec();
    for p in out.chunks_exact_mut(4) {
        let key = [p[0], p[1], p[2], p[3]];
        if key[3] == 0 {
            continue;
        }
        let new = *seen.entry(key).or_insert_with(|| {
            let a = key[3] as f32 / 255.0;
            let c = invert_lightness(ColorF::new(key[r] as f32 / 255.0 / a, key[1] as f32 / 255.0 / a, key[b] as f32 / 255.0 / a, 1.0));
            let q = |v: f32| (v.clamp(0.0, 1.0) * a * 255.0).round() as u8;
            let mut out = [0, q(c.g), 0, key[3]];
            out[r] = q(c.r);
            out[b] = q(c.b);
            out
        });
        p.copy_from_slice(&new);
    }
    out
}

fn to_linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn to_srgb(v: f32) -> f32 {
    if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.max(0.0).powf(1.0 / 2.4) - 0.055 }
}

// D65 white.
const XN: f32 = 0.95047;
const YN: f32 = 1.0;
const ZN: f32 = 1.08883;

fn f(t: f32) -> f32 {
    const D: f32 = 6.0 / 29.0;
    if t > D * D * D { t.cbrt() } else { t / (3.0 * D * D) + 4.0 / 29.0 }
}

fn f_inv(t: f32) -> f32 {
    const D: f32 = 6.0 / 29.0;
    if t > D { t * t * t } else { 3.0 * D * D * (t - 4.0 / 29.0) }
}

fn srgb_to_lab(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let (r, g, b) = (to_linear(r), to_linear(g), to_linear(b));
    let x = 0.4124 * r + 0.3576 * g + 0.1805 * b;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = 0.0193 * r + 0.1192 * g + 0.9505 * b;
    let (fx, fy, fz) = (f(x / XN), f(y / YN), f(z / ZN));
    (116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz))
}

fn lab_to_srgb(l: f32, a: f32, b: f32) -> (f32, f32, f32) {
    let fy = (l + 16.0) / 116.0;
    let (x, y, z) = (XN * f_inv(fy + a / 500.0), YN * f_inv(fy), ZN * f_inv(fy - b / 200.0));
    let r = 3.2406 * x - 1.5372 * y - 0.4986 * z;
    let g = -0.9689 * x + 1.8758 * y + 0.0415 * z;
    let b = 0.0557 * x - 0.2040 * y + 1.0570 * z;
    (to_srgb(r), to_srgb(g), to_srgb(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(c: ColorF) -> [u8; 3] {
        [c.r, c.g, c.b].map(|v| (v * 255.0).round() as u8)
    }

    #[test]
    fn white_goes_dark_and_black_light() {
        let white = invert_lightness(ColorF::WHITE);
        assert!(rgb(white).iter().all(|&v| v < 32), "{:?}", rgb(white));
        assert_eq!(rgb(invert_lightness(ColorF::BLACK)), [255, 255, 255]);
        // Twice is (nearly) where it started, in the middle.
        let grey = ColorF::new(0.5, 0.5, 0.5, 1.0);
        let back = invert_lightness(invert_lightness(grey));
        assert!((back.r - 0.5).abs() < 0.08, "{:?}", back);
    }

    #[test]
    fn roles() {
        let white = ColorF::WHITE;
        let black = ColorF::BLACK;
        assert!(changes(&white, Role::Background) && !changes(&black, Role::Background));
        assert!(changes(&black, Role::Foreground) && !changes(&white, Role::Foreground));
        // A dark page's own colours stay.
        let page = ColorF::new(0.12, 0.12, 0.12, 1.0);
        let text = ColorF::new(0.91, 0.92, 0.93, 1.0);
        assert!(!changes(&page, Role::Background) && !changes(&text, Role::Foreground));
        assert!(!changes(&ColorF::TRANSPARENT, Role::Background));
    }

    fn picture(width: i32, height: i32, f: impl Fn(i32, i32) -> [u8; 4]) -> Vec<u8> {
        let mut v = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let [r, g, b, a] = f(x, y);
                let m = |c: u8| (c as u32 * a as u32 / 255) as u8;
                v.extend([m(b), m(g), m(r), a]);
            }
        }
        v
    }

    #[test]
    fn icons() {
        // A black glyph on transparency: an icon.
        let glyph = picture(48, 48, |x, y| if (x - 24).abs() < 10 && (y - 24).abs() < 3 { [0, 0, 0, 255] } else { [0, 0, 0, 0] });
        assert!(is_icon(&glyph, true, 48, 48, 48 * 4));
        let back = invert_image(&glyph, true);
        assert_eq!(&back[(24 * 48 + 24) * 4..][..4], &[255, 255, 255, 255]);
        // A white one is fine as it is.
        let white = picture(48, 48, |x, y| if (x - 24).abs() < 10 && (y - 24).abs() < 3 { [255, 255, 255, 255] } else { [0, 0, 0, 0] });
        assert!(!is_icon(&white, true, 48, 48, 48 * 4));
        // A photograph-like gradient, and anything big, are not.
        let photo = picture(64, 64, |x, y| [(x * 4) as u8, (y * 4) as u8, ((x + y) * 2) as u8, 255]);
        assert!(!is_icon(&photo, true, 64, 64, 64 * 4));
        assert!(!is_icon(&vec![0; 400 * 400 * 4], true, 400, 400, 1600));
        // A colourful logo (saturated, few colours) keeps its colours.
        let logo = picture(48, 48, |x, _| if x < 24 { [234, 67, 53, 255] } else { [66, 133, 244, 255] });
        assert!(!is_icon(&logo, true, 48, 48, 48 * 4));
    }

    #[test]
    fn hue_is_kept() {
        // A link blue turns light blue, not orange.
        let blue = invert_lightness(ColorF::new(0.10, 0.05, 0.67, 1.0));
        assert!(blue.b > blue.r && blue.b > blue.g && brightness(&blue) > 120.0, "{:?}", rgb(blue));
    }
}
