//! A new window's glow (Preferences ▸ Appearance ▸ "Glow new windows
//! for N seconds", off by default): a soft halo in the look's focus blue
//! around its outline (its frame and its tab, as one shape), whole for
//! its time but the last second, over which it fades out. Drawn behind
//! the window from a few pieces made at the window's size in device
//! pixels and drawn one to one (`render::exact`): across the top a band
//! taking in the tab, where the halo is the distance to the frame and tab
//! together (so it follows the tab up and round it); down the sides and
//! along the bottom a one-pixel strip of its falloff stretched along the
//! edge; and the bottom corners round. The band itself is in parts
//! (`band_parts`): painted pixel by pixel only where it changes across
//! (round the tab, and past the frame's ends), and where it doesn't (over
//! the frame's top away from the tab, most of a wide window) a strip one
//! pixel wide stretched across, as the sides' are. (Painted whole, a
//! window filling a 5K display took 7.7 ms of app_server's loop as it
//! opened: its log's "slow window elements: … glow 10.9 ms".) Made
//! again only when the window's width, tab, look or scale changes;
//! nothing once it has faded.

use std::time::Duration;

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    utils::{Buffer, Physical, Rectangle, Size, Transform},
};

use crate::render::Exact;

/// How far out it reaches (points).
pub const REACH: i32 = 12;
/// How strong it is at the outline, in a light look and a dark one (it
/// needs more to show against a dark desktop).
const PEAK_LIGHT: f32 = 0.55;
const PEAK_DARK: f32 = 0.7;
/// The fade at its end.
const FADE: Duration = Duration::from_secs(1);

/// How strong a window's glow is `age` after it opened, glowing `total`:
/// whole, then easing out over the last second.
pub fn strength(age: Duration, total: Duration) -> f32 {
    if age >= total {
        return 0.0;
    }
    let left = (total - age).as_secs_f32();
    let fade = FADE.min(total).as_secs_f32();
    if left >= fade {
        return 1.0;
    }
    let t = left / fade;
    t * t * (3.0 - 2.0 * t)
}

/// The falloff out from the outline, `d` the distance as a fraction of
/// the reach: strong close in, a long soft tail.
fn falloff(d: f32) -> f32 {
    let t = (1.0 - d).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t) * t
}

/// What the pieces were made for.
#[derive(Debug, Clone, PartialEq)]
struct Key {
    reach: i32,
    color: [u8; 3],
    peak: u32,
    /// The frame's width and the tab's left, right and height, in device
    /// pixels from the frame's left and top (None: no tab).
    width: i32,
    tab: Option<(i32, i32, i32)>,
}

struct Pieces {
    key: Key,
    /// The band across the top: from `reach` left of the shape to `reach`
    /// right of it, from `reach` over its top down to the frame's top,
    /// in parts side by side: each one's left (from the frame's), its
    /// width, and its picture, as wide as the part or one pixel wide to
    /// be stretched across it.
    band: Vec<(i32, i32, MemoryRenderBuffer, i32)>,
    band_height: i32,
    /// The left and right sides' strip (`reach` × 1), the bottom's
    /// (1 × `reach`), and the bottom corners (`reach` × `reach`).
    left: MemoryRenderBuffer,
    right: MemoryRenderBuffer,
    bottom: MemoryRenderBuffer,
    bottom_left: MemoryRenderBuffer,
    bottom_right: MemoryRenderBuffer,
}

/// A window's glow, kept with it while it shows.
#[derive(Default)]
pub struct Glow {
    pieces: Option<Pieces>,
}

/// Premultiplied RGBA rows (`Abgr8888`), `w` × `h`, each pixel's
/// strength from `at(x, y)` (its centre, in device pixels).
fn paint(w: i32, h: i32, color: [u8; 3], peak: f32, at: impl Fn(f32, f32) -> f32) -> MemoryRenderBuffer {
    let mut data = Vec::with_capacity((w * h * 4).max(0) as usize);
    for y in 0..h {
        for x in 0..w {
            let a = (peak * at(x as f32 + 0.5, y as f32 + 0.5)).clamp(0.0, 1.0);
            data.extend_from_slice(&[(color[0] as f32 * a).round() as u8, (color[1] as f32 * a).round() as u8, (color[2] as f32 * a).round() as u8, (255.0 * a).round() as u8]);
        }
    }
    let size: Size<i32, Buffer> = (w.max(1), h.max(1)).into();
    // (Translucent throughout: no opaque region, so what is behind shows.)
    MemoryRenderBuffer::from_slice(&data, Fourcc::Abgr8888, size, 1, Transform::Normal, None)
}

/// The distance from point (`x`, `y`) to the rectangle from (`l`, `t`)
/// to (`r`, `b`), 0 inside.
fn distance(x: f32, y: f32, (l, t, r, b): (f32, f32, f32, f32)) -> f32 {
    let dx = (l - x).max(x - r).max(0.0);
    let dy = (t - y).max(y - b).max(0.0);
    (dx * dx + dy * dy).sqrt()
}

/// The band across a window's top, from `left` to `right` (device
/// pixels from the frame's left), in parts: each one's left, its width,
/// and whether the halo is the same all across it (flat). It changes
/// across only within `reach` of something that ends: past the frame's
/// two ends, and from `reach` before the tab to `reach` after it; over
/// the frame's top anywhere else it is the falloff up from the frame,
/// column after column.
fn band_parts(left: i32, right: i32, width: i32, tab: Option<(i32, i32, i32)>, reach: i32) -> Vec<(i32, i32, bool)> {
    // Where it changes across, in order, joined where they meet.
    let mut busy: Vec<(i32, i32)> = vec![(left, 0)];
    if let Some((l, r, _)) = tab {
        busy.push((l - reach, r + reach));
    }
    busy.push((width, right));
    busy.sort();
    let mut joined: Vec<(i32, i32)> = Vec::new();
    for (a, b) in busy {
        let (a, b) = (a.max(left), b.min(right));
        if b <= a {
            continue;
        }
        match joined.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => joined.push((a, b)),
        }
    }
    let mut parts = Vec::new();
    let mut at = left;
    for (a, b) in joined {
        if a > at {
            parts.push((at, a - at, true));
        }
        parts.push((a, b - a, false));
        at = b;
    }
    if right > at {
        parts.push((at, right - at, true));
    }
    parts
}

impl Pieces {
    fn make(key: Key, peak: f32) -> Pieces {
        let (r, c) = (key.reach, key.color);
        let reach = r as f32;
        let glow = |d: f32| falloff(d / reach);
        let frame = (0.0, 0.0, key.width as f32, f32::MAX);
        let tab = key.tab.map(|(l, rr, h)| (l as f32, -(h as f32), rr as f32, 0.0));
        let top = key.tab.map_or(0, |t| t.2);
        let left = key.tab.map_or(0, |t| t.0.min(0)) - r;
        let right = key.tab.map_or(key.width, |t| t.1.max(key.width)) + r;
        // (Band pixels from its own top left: the frame's top at its foot.)
        let bh = top + r;
        let at = |px: f32, y: f32| {
            let py = y - bh as f32;
            glow(tab.map_or(f32::MAX, |t| distance(px, py, t)).min(distance(px, py, frame)))
        };
        let band = band_parts(left, right, key.width, key.tab, r)
            .into_iter()
            .map(|(x, w, flat)| {
                // (Flat: every column as its first.)
                let src = if flat { 1 } else { w };
                (x, w, paint(src, bh, c, peak, |px, y| at(px + x as f32, y)), src)
            })
            .collect();
        Pieces {
            band,
            band_height: bh,
            left: paint(r, 1, c, peak, |x, _| glow(reach - x)),
            right: paint(r, 1, c, peak, |x, _| glow(x)),
            bottom: paint(1, r, c, peak, |_, y| glow(y)),
            bottom_left: paint(r, r, c, peak, |x, y| glow(((reach - x).powi(2) + y.powi(2)).sqrt())),
            bottom_right: paint(r, r, c, peak, |x, y| glow((x.powi(2) + y.powi(2)).sqrt())),
            key,
        }
    }
}

impl Glow {
    /// The glow round `frame` (device pixels) and the tab on it (`tab`:
    /// its left and right, from the frame's left, and its height), at
    /// `strength`, in the look's colours at `scale`, behind the window.
    pub fn elements<R>(&mut self, renderer: &mut R, frame: Rectangle<i32, Physical>, tab: Option<(i32, i32, i32)>, scale: f64, strength: f32) -> Vec<Exact<MemoryRenderBufferRenderElement<R>>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let palette = crate::theme::palette();
        let peak = if palette.is_dark() { PEAK_DARK } else { PEAK_LIGHT };
        let c = palette.keyboard_navigation;
        let key = Key { reach: (REACH as f64 * scale).round() as i32, color: [c[0], c[1], c[2]], peak: peak.to_bits(), width: frame.size.w, tab };
        if self.pieces.as_ref().is_none_or(|p| p.key != key) {
            self.pieces = Some(Pieces::make(key, peak));
        }
        let p = self.pieces.as_ref().unwrap();
        let r = p.key.reach;
        let (l, t, w, h) = (frame.loc.x, frame.loc.y, frame.size.w, frame.size.h);
        let rect = |x: i32, y: i32, w: i32, h: i32| Rectangle::<i32, Physical>::new((x, y).into(), (w, h).into());
        let band = p.band.iter().map(|(x, bw, buffer, src)| (rect(l + x, t - p.band_height, *bw, p.band_height), buffer, (*src, p.band_height)));
        let pieces = [
            (rect(l - r, t, r, h), &p.left, (r, 1)),
            (rect(l + w, t, r, h), &p.right, (r, 1)),
            (rect(l, t + h, w, r), &p.bottom, (1, r)),
            (rect(l - r, t + h, r, r), &p.bottom_left, (r, r)),
            (rect(l + w, t + h, r, r), &p.bottom_right, (r, r)),
        ];
        band.chain(pieces)
            .filter(|(rect, _, _)| rect.size.w > 0 && rect.size.h > 0)
            .filter_map(|(rect, buffer, src)| crate::render::exact(renderer, rect, buffer, src, Some(strength), Kind::Unspecified))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_then_fading() {
        let total = Duration::from_secs(5);
        assert_eq!(strength(Duration::ZERO, total), 1.0);
        assert_eq!(strength(Duration::from_secs(4), total), 1.0);
        let half = strength(Duration::from_millis(4500), total);
        assert!(half > 0.3 && half < 0.7, "{half}");
        assert_eq!(strength(total, total), 0.0);
        // Shorter than its fade: fading all its time.
        let short = Duration::from_millis(500);
        assert_eq!(strength(Duration::ZERO, short), 1.0);
        assert!(strength(Duration::from_millis(250), short) < 1.0);
    }

    /// What making a window's glow costs (printed; `--nocapture`): a
    /// window filling a 5K display at 2.5×, its tab at the left.
    #[test]
    fn what_making_one_costs() {
        let key = Key { reach: 30, color: [0x33, 0x66, 0xcc], peak: PEAK_LIGHT.to_bits(), width: 4640, tab: Some((0, 520, 53)) };
        let mut best = Duration::MAX;
        for _ in 0..7 {
            let t = std::time::Instant::now();
            std::hint::black_box(Pieces::make(key.clone(), PEAK_LIGHT));
            best = best.min(t.elapsed());
        }
        println!("a 4640-pixel-wide window's glow made in {:.2} ms (the best of 7)", best.as_secs_f64() * 1e3);
    }

    /// The band's parts: side by side from end to end, painted pixel by
    /// pixel only round the tab and past the frame's ends; and every
    /// column of a flat part is what painting the band whole gives there.
    #[test]
    fn the_band_in_parts_is_the_band() {
        let whole = |left: i32, right: i32, width: i32, tab: Option<(i32, i32, i32)>, reach: i32| -> Vec<Vec<u32>> {
            let (r, bh) = (reach as f32, tab.map_or(0, |t| t.2) + reach);
            let frame = (0.0, 0.0, width as f32, f32::MAX);
            let t = tab.map(|(l, rr, h)| (l as f32, -(h as f32), rr as f32, 0.0));
            (left..right)
                .map(|x| {
                    (0..bh)
                        .map(|y| {
                            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5 - bh as f32);
                            falloff(t.map_or(f32::MAX, |t| distance(px, py, t)).min(distance(px, py, frame)) / r).to_bits()
                        })
                        .collect()
                })
                .collect()
        };
        for (width, tab, reach) in [
            (4640, Some((0, 520, 53)), 30),
            (800, Some((300, 420, 21)), 12),
            (400, Some((-20, 90, 21)), 12),
            (300, Some((250, 340, 21)), 12),
            (60, Some((0, 60, 21)), 12),
            (500, None, 18),
            (10, Some((0, 8, 21)), 12),
        ] {
            let left = tab.map_or(0, |t: (i32, i32, i32)| t.0.min(0)) - reach;
            let right = tab.map_or(width, |t| t.1.max(width)) + reach;
            let parts = band_parts(left, right, width, tab, reach);
            // End to end, in order, none empty.
            let mut at = left;
            for (x, w, _) in &parts {
                assert_eq!((*x, *w > 0), (at, true), "{width} {tab:?}: {parts:?}");
                at += w;
            }
            assert_eq!(at, right, "{width} {tab:?}: {parts:?}");
            // A flat part's columns are all its first, in the band whole.
            let band = whole(left, right, width, tab, reach);
            for (x, w, flat) in &parts {
                if *flat {
                    let first = &band[(x - left) as usize];
                    assert!((0..*w).all(|i| &band[(x - left + i) as usize] == first), "{width} {tab:?}: the part at {x} isn't flat");
                }
            }
        }
        // A wide window: all but the tab's part and the ends is flat.
        let parts = band_parts(-30, 4670, 4640, Some((0, 520, 53)), 30);
        assert_eq!(parts, [(-30, 580, false), (550, 4090, true), (4640, 30, false)]);
        // No tab: the two ends, and one strip between.
        assert_eq!(band_parts(-18, 518, 500, None, 18), [(-18, 18, false), (0, 500, true), (500, 18, false)]);
    }

    #[test]
    fn strongest_at_the_outline() {
        assert!(falloff(0.0) > 0.99);
        assert!(falloff(0.5) < falloff(0.25));
        assert_eq!(falloff(1.0), 0.0);
        // The tab's glow: as strong beside the tab as beside the frame, and
        // round its corner.
        assert_eq!(distance(-3.0, -10.0, (0.0, -21.0, 80.0, 0.0)), 3.0);
        assert!((distance(-3.0, -24.0, (0.0, -21.0, 80.0, 0.0)) - 18f32.sqrt()).abs() < 1e-5);
    }
}
