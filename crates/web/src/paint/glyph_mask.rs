//! Bounded, device-sized glyph coverage. Source paths remain the raster oracle;
//! masks are drawn one pixel to one pixel, never filtered or rescaled.
use super::*;
use std::{collections::HashMap, sync::Arc};
use tiny_skia::{Mask, Path, Pixmap, Transform};

#[derive(Clone, Copy, Debug, Default)]
pub struct MaskStats {
    pub entries: usize,
    /// Coverage, scratch and a conservative path charge per mask variant.
    /// Excludes allocator and hash-table capacity overhead.
    pub bytes: usize,
    pub hits: usize,
    pub misses: usize,
    pub resets: usize,
}
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct Key {
    path: usize,
    transform: [u32; 6],
}
struct Entry {
    // Leases the path identity even after the outline/font cache evicts it.
    _path: Arc<Path>,
    pixels: Arc<Mask>,
    left: i32,
    top: i32,
}
#[derive(Default)]
pub(super) struct Cache {
    entries: HashMap<Key, Entry>,
    stats: MaskStats,
    scratch: Option<Mask>,
}
impl Cache {
    pub(super) fn stats(&self) -> MaskStats {
        self.stats
    }

    fn get(&mut self, path: &Arc<Path>, t: Transform, limits: Limits, cancel: Option<&AtomicBool>) -> Result<Option<(Arc<Mask>, i32, i32)>> {
        check(cancel)?;
        const SCRATCH: usize = 512 * 512;
        if limits.glyph_mask_bytes <= SCRATCH || limits.glyph_masks == 0 || !t.is_finite() {
            return Ok(None);
        }
        let key = Key { path: Arc::as_ptr(path) as usize, transform: [t.sx, t.ky, t.kx, t.sy, t.tx, t.ty].map(f32::to_bits) };
        if let Some(entry) = self.entries.get(&key) {
            self.stats.hits += 1;
            return Ok(Some((entry.pixels.clone(), entry.left, entry.top)));
        }
        let Some(bounds) = path.bounds().transform(t).and_then(|b| b.round_out()) else { return Ok(None) };
        if bounds.width() > 256 || bounds.height() > 256 || bounds.left() < 0 || bounds.top() < 0 || bounds.right() > 512 || bounds.bottom() > 512 {
            return Ok(None);
        }
        let bytes = (bounds.width() as usize)
            .saturating_mul(bounds.height() as usize)
            .saturating_add(std::mem::size_of_val(path.points()))
            .saturating_add(path.verbs().len())
            .saturating_add(size_of::<Entry>() + size_of::<Key>());
        if bytes > limits.glyph_mask_bytes - SCRATCH {
            return Ok(None);
        }
        self.stats.misses += 1;
        if self.scratch.is_none() {
            self.scratch = Some(Mask::new(512, 512).ok_or(Error::Limit)?);
            self.stats.bytes += SCRATCH;
        }
        let scratch = self.scratch.as_mut().unwrap();
        // Keep the exact device coordinates. Moving the path to the origin
        // changes floating-point curve subdivision and scan conversion.
        let mut pixels = Mask::new(bounds.width(), bounds.height()).ok_or(Error::Limit)?;
        scratch.fill_path(path, tiny_skia::FillRule::Winding, true, t);
        let (left, top) = (bounds.left(), bounds.top());
        for row in 0..bounds.height() as usize {
            let start = (top as usize + row) * 512 + left as usize;
            let src = &mut scratch.data_mut()[start..start + bounds.width() as usize];
            pixels.data_mut()[row * bounds.width() as usize..(row + 1) * bounds.width() as usize].copy_from_slice(src);
            src.fill(0);
        }
        check(cancel)?;
        let pixels = Arc::new(pixels);
        if self.entries.len() >= limits.glyph_masks || self.stats.bytes.saturating_add(bytes) > limits.glyph_mask_bytes {
            self.entries.clear();
            self.stats.bytes = SCRATCH;
            self.stats.resets += 1;
        }
        self.entries.insert(key, Entry { _path: path.clone(), pixels: pixels.clone(), left, top });
        self.stats.bytes += bytes;
        self.stats.entries = self.entries.len();
        Ok(Some((pixels, left, top)))
    }
}
impl Engine {
    pub(super) fn paint_glyph(&mut self, out: &mut Pixmap, path: &Arc<Path>, color: [u8; 4], t: Transform, clip: Option<&Mask>, cancel: Option<&AtomicBool>) -> Result<()> {
        let limits = self.limits();
        // The direct scan converter clips curves before rasterizing. Keep that
        // path at viewport edges, where a cropped whole-glyph mask can differ.
        let inside = out.width() <= 512
            && out.height() <= 512
            && path.bounds().transform(t).is_some_and(|b| b.left() >= 0. && b.top() >= 0. && b.right() <= out.width() as f32 && b.bottom() <= out.height() as f32);
        let cached = if inside { self.glyph_masks.get_or_insert_with(Default::default).get(path, t, limits, cancel)? } else { None };
        if let Some((pixels, x, y)) = cached {
            composite(out, &pixels, x, y, color, clip, cancel)?;
        } else {
            let mut paint = tiny_skia::Paint::default();
            paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
            out.fill_path(path, &paint, tiny_skia::FillRule::Winding, t, clip);
        }
        Ok(())
    }
}

/// Match tiny-skia 0.12's solid-color low-precision path pipeline, including
/// its integer rounding and its opaque/no-clip Source strength reduction.
/// A pre-tinted RGBA bitmap would round before applying the clip and change
/// pixels. Retain raw coverage so clipping, tint and alpha stay in their
/// original order. The direct-path differential test guards this contract.
fn composite(out: &mut Pixmap, mask: &Mask, x: i32, y: i32, color: [u8; 4], clip: Option<&Mask>, cancel: Option<&AtomicBool>) -> Result<()> {
    let (width, height) = (out.width() as usize, out.height() as usize);
    if clip.is_some_and(|c| c.width() != out.width() || c.height() != out.height()) {
        return Err(Error::Invalid);
    }
    let x0 = x.max(0) as usize;
    let x1 = (i64::from(x) + i64::from(mask.width())).clamp(0, width as i64) as usize;
    let y0 = y.max(0) as usize;
    let y1 = (i64::from(y) + i64::from(mask.height())).clamp(0, height as i64) as usize;
    if x0 >= x1 || y0 >= y1 {
        return Ok(());
    }
    let opaque = color[3] == 255 && clip.is_none();
    let mut source = color.map(u32::from);
    for c in &mut source[..3] {
        *c = (*c * u32::from(color[3]) + 127) / 255;
    }
    let div = |v: u32| (v + 255) >> 8;
    let data = out.data_mut();
    for row in y0..y1 {
        check(cancel)?;
        let begin = (row as i64 - i64::from(y)) as usize * mask.width() as usize + (x0 as i64 - i64::from(x)) as usize;
        let covers = &mask.data()[begin..begin + x1 - x0];
        let dst = &mut data[(row * width + x0) * 4..(row * width + x1) * 4];
        for (column, (&coverage, pixel)) in covers.iter().zip(dst.chunks_exact_mut(4)).enumerate() {
            let a = u32::from(coverage);
            if a == 0 {
                continue;
            }
            if opaque {
                for (dst, &src) in pixel.iter_mut().zip(&source) {
                    *dst = div(u32::from(*dst) * (255 - a) + src * a) as u8;
                }
            } else {
                let mut src = source;
                if let Some(clip) = clip {
                    let b = u32::from(clip.data()[row * width + x0 + column]);
                    if b == 0 {
                        continue;
                    }
                    for c in &mut src {
                        *c = div(*c * b);
                    }
                }
                for c in &mut src {
                    *c = div(*c * a);
                }
                let inv = 255 - src[3];
                for (dst, &src) in pixel.iter_mut().zip(&src) {
                    *dst = (src + div(u32::from(*dst) * inv)) as u8;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outline() -> Arc<Path> {
        let mut p = tiny_skia::PathBuilder::new();
        p.move_to(-2.3, 1.7);
        p.line_to(11.8, 0.2);
        p.quad_to(19.2, 9.7, 7.3, 17.1);
        p.cubic_to(1.1, 22.4, -4.2, 15.3, -2.3, 1.7);
        p.close();
        Arc::new(p.finish().unwrap())
    }

    #[test]
    fn fractional_transforms_preserve_pixels() {
        let path = outline();
        let mut cached = Engine::default();
        let mut direct = Engine::new(Limits { glyph_mask_bytes: 0, ..Default::default() });
        let mut seed = 0x19287364u32;
        let mut next = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / 16777216.
        };
        for _ in 0..10_000 {
            let t = Transform::from_row(0.1 + next() * 2., next() - 0.5, next() - 0.5, -0.1 - next() * 2., 20. + next() * 30., 40. + next() * 30.);
            let mut a = Pixmap::new(100, 80).unwrap();
            a.fill(tiny_skia::Color::WHITE);
            let mut b = a.clone();
            cached.paint_glyph(&mut a, &path, [17, 17, 17, 255], t, None, None).unwrap();
            direct.paint_glyph(&mut b, &path, [17, 17, 17, 255], t, None, None).unwrap();
            let mismatch = a.data().iter().zip(b.data()).position(|(a, b)| a != b);
            assert!(mismatch.is_none(), "{t:?}, first mismatch {mismatch:?}");
        }
    }

    #[test]
    fn transforms_and_viewport_edges_preserve_pixels() {
        let path = outline();
        let mut cached = Engine::default();
        let mut direct = Engine::new(Limits { glyph_mask_bytes: 0, ..Default::default() });
        for scale in [0.1, 1., 1.25, 1.5, 1.75, 2., 4.] {
            for (kx, ky) in [(0., 0.), (0.3, -0.2), (-0.7, 0.1)] {
                for x in [-40., -2.125, 0., 30.125, 99.75] {
                    for y in [-30., -1.5, 0., 20.25, 79.125] {
                        let t = Transform::from_row(scale, ky, kx, -scale, x, y);
                        let mut a = Pixmap::new(100, 80).unwrap();
                        a.fill(tiny_skia::Color::WHITE);
                        let mut b = a.clone();
                        cached.paint_glyph(&mut a, &path, [17, 17, 17, 255], t, None, None).unwrap();
                        direct.paint_glyph(&mut b, &path, [17, 17, 17, 255], t, None, None).unwrap();
                        let mismatch = a.data().iter().zip(b.data()).position(|(a, b)| a != b);
                        assert!(mismatch.is_none(), "{t:?}, first mismatch {mismatch:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn cache_identity_budgets_and_cancellation() {
        let path = outline();
        let mut c = Cache::default();
        let limits = Limits { glyph_masks: 2, ..Default::default() };
        let t = Transform::from_translate(40., 40.);
        let first = c.get(&path, t, limits, None).unwrap().unwrap();
        let repeated = c.get(&path, t, limits, None).unwrap().unwrap();
        assert!(Arc::ptr_eq(&first.0, &repeated.0));
        assert_eq!(c.stats().hits, 1);
        assert_eq!(Arc::strong_count(&path), 2);
        let bytes = c.stats().bytes;
        let second = c.get(&path, Transform::from_row(1.5, 0., 0., 1.5, 40., 40.), limits, None).unwrap().unwrap();
        assert!(!Arc::ptr_eq(&first.0, &second.0));
        assert_eq!(c.stats().entries, 2);
        c.get(&path, Transform::from_translate(40.5, 40.), limits, None).unwrap().unwrap();
        assert_eq!(c.stats().entries, 1);
        assert_eq!(c.stats().resets, 1);
        assert!(c.stats().bytes <= limits.glyph_mask_bytes);
        let mut small = Cache::default();
        assert!(small.get(&path, t, Limits { glyph_mask_bytes: bytes - 1, ..limits }, None).unwrap().is_none());
        assert_eq!(small.stats().entries, 0);
        assert!(small.get(&path, Transform::from_scale(100., 100.), limits, None).unwrap().is_none());
        let cancelled = AtomicBool::new(true);
        assert!(c.get(&path, t, limits, Some(&cancelled)).is_err());
        let distinct = outline();
        c.get(&distinct, Transform::from_translate(40.5, 40.), limits, None).unwrap().unwrap();
        assert_eq!(c.stats().entries, 2);
        let mut engine = Engine::default();
        engine.glyph_masks = Some(Box::new(c));
        engine.clear();
        assert_eq!(engine.glyph_mask_stats().entries, 0);
        assert_eq!(Arc::strong_count(&path), 1);
    }

    #[test]
    fn cached_glyphs_preserve_direct_raster_pixels() {
        let path = outline();
        let mut cached = Engine::default();
        let mut direct = Engine::new(Limits { glyph_mask_bytes: 0, ..Default::default() });
        let mut clip = Mask::new(100, 80).unwrap();
        for (i, c) in clip.data_mut().iter_mut().enumerate() {
            *c = ((i % 100 + i / 100) * 3) as u8;
        }
        for scale in [1., 1.25, 1.5, 1.75, 2.] {
            for phase in [0., 0.125, 0.25, 0.5, 0.75] {
                for color in [[0, 0, 0, 255], [17, 17, 17, 255], [13, 89, 201, 255], [13, 89, 201, 73]] {
                    for clip in [None, Some(&clip)] {
                        for background in [tiny_skia::Color::WHITE, tiny_skia::Color::TRANSPARENT, tiny_skia::Color::from_rgba8(29, 103, 173, 255), tiny_skia::Color::from_rgba8(133, 78, 21, 83)] {
                            let mut a = Pixmap::new(100, 80).unwrap();
                            a.fill(background);
                            let mut b = a.clone();
                            let t = Transform::from_row(scale, 0., 0., scale, 40. + phase, 20. + phase);
                            cached.paint_glyph(&mut a, &path, color, t, clip, None).unwrap();
                            direct.paint_glyph(&mut b, &path, color, t, clip, None).unwrap();
                            let mismatch = a.data().iter().zip(b.data()).position(|(a, b)| a != b);
                            assert!(mismatch.is_none(), "scale {scale} phase {phase} color {color:?} clip {}, first mismatch {mismatch:?}", clip.is_some());
                        }
                    }
                }
            }
        }
        assert!(cached.glyph_mask_stats().hits > 0);
    }
}
