//! A new window's glow (Preferences ▸ Appearance ▸ "Glow new windows
//! for N seconds", off by default): a soft halo in the look's focus blue
//! around its outline (its frame and its tab, as one shape), whole for
//! its time but the last second, over which it fades out. Drawn behind
//! the window from a few pieces made at the window's size in device
//! pixels and drawn one to one (`render::exact`): across the top a band
//! taking in the tab, where the halo is the distance to the frame and tab
//! together (so it follows the tab up and round it); down the sides and
//! along the bottom a one-pixel strip of its falloff stretched along the
//! edge; and the bottom corners round. Made again only when the window's
//! width, tab, look or scale changes; nothing once it has faded.

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
    /// right of it, from `reach` over its top down to the frame's top.
    band: MemoryRenderBuffer,
    band_size: (i32, i32),
    band_x: i32,
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
        let (bw, bh) = (right - left, top + r);
        let band = paint(bw, bh, c, peak, |x, y| {
            let (px, py) = (x + left as f32, y - bh as f32);
            let d = tab.map_or(f32::MAX, |t| distance(px, py, t)).min(distance(px, py, frame));
            glow(d)
        });
        Pieces {
            band,
            band_size: (bw, bh),
            band_x: left,
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
        let pieces = [
            (rect(l + p.band_x, t - p.band_size.1, p.band_size.0, p.band_size.1), &p.band, p.band_size),
            (rect(l - r, t, r, h), &p.left, (r, 1)),
            (rect(l + w, t, r, h), &p.right, (r, 1)),
            (rect(l, t + h, w, r), &p.bottom, (1, r)),
            (rect(l - r, t + h, r, r), &p.bottom_left, (r, r)),
            (rect(l + w, t + h, r, r), &p.bottom_right, (r, r)),
        ];
        pieces
            .into_iter()
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
        // Shorter than its fade: fading from the start.
        assert!(strength(Duration::ZERO, Duration::from_millis(500)) < 1.0);
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
