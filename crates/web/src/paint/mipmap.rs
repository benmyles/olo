//! Worker-owned, bounded source-pixel mipmaps. They depend on the immutable
//! source, not device scale. Output tile coordinates remain scale-specific.
use super::*;
use crate::image::Image;
use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct ImageCacheStats {
    pub sources: usize,
    pub levels: usize,
    pub bytes: usize,
    pub hits: usize,
    pub misses: usize,
    pub generated_pixels: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn image(w: u32, h: u32) -> Arc<Image> {
        Image::from_rgba(
            w,
            h,
            (0..w * h).flat_map(|p| [(p.wrapping_mul(17)) as u8, (p.wrapping_mul(71)) as u8, (p.wrapping_mul(39)) as u8, (p.wrapping_mul(29)) as u8]).collect(),
            Default::default(),
            None,
        )
        .unwrap()
    }
    #[test]
    fn packed_reduction_matches_normalized_weighted_reference() {
        // Independent normalized convolution oracle, covering lane boundaries,
        // single-axis images, odd/even dimensions and premultiplied alpha.
        for w in 1..=19 {
            for h in 1..=17 {
                let source = image(w, h);
                let got = reduce(Plane::from(source.as_ref()), None).unwrap();
                let taps = |n: u32, out: u32| -> Vec<(u32, f64)> {
                    if n == 1 {
                        vec![(0, 1.)]
                    } else if n.is_multiple_of(2) {
                        vec![(out * 2, 0.5), (out * 2 + 1, 0.5)]
                    } else {
                        vec![(out * 2, 0.25), (out * 2 + 1, 0.5), (out * 2 + 2, 0.25)]
                    }
                };
                for y in 0..got.height {
                    for x in 0..got.width {
                        for c in 0..4 {
                            let mut expected = 0.;
                            for (sy, wy) in taps(h, y) {
                                for (sx, wx) in taps(w, x) {
                                    expected += f64::from(source.rgba()[((sy * w + sx) * 4 + c) as usize]) * wx * wy;
                                }
                            }
                            assert_eq!(got.rgba[((y * got.width + x) * 4 + c) as usize], expected.floor() as u8, "{w}x{h}, {x},{y},{c}");
                        }
                        let p = &got.rgba[((y * got.width + x) * 4) as usize..][..4];
                        assert!(p[..3].iter().all(|v| *v <= p[3]));
                    }
                }
            }
        }
        let source = Image::from_rgba(33, 35, vec![255; 33 * 35 * 4], Default::default(), None).unwrap();
        assert!(reduce(Plane::from(source.as_ref()), None).unwrap().rgba.iter().all(|v| *v == 255));
    }
    #[test]
    fn odd_axis_weights_are_explicit_and_checker_averages_floor() {
        let source = Image::from_rgba(3, 3, (0..9).flat_map(|p| if p == 4 { [255, 0, 0, 255] } else { [0, 0, 0, 255] }).collect(), Default::default(), None).unwrap();
        assert_eq!(&*reduce(Plane::from(source.as_ref()), None).unwrap().rgba, &[63, 0, 0, 255]);
        let checker = Image::from_rgba(
            8,
            8,
            (0..64)
                .flat_map(|p| {
                    let c = if (p % 8 + p / 8) % 2 == 0 { 0 } else { 255 };
                    [c, c, c, 255]
                })
                .collect(),
            Default::default(),
            None,
        )
        .unwrap();
        let mut cache = Cache::new(84, 1);
        for l in 1..=3 {
            let level = cache.get(&checker, l, None).unwrap();
            assert!(level.rgba.as_chunks::<4>().0.iter().all(|p| *p == [127, 127, 127, 255]));
        }
        assert_eq!(cache.stats().bytes, 84);
        assert_eq!(cache.stats().generated_pixels, 21);
        let a = cache.get(&checker, 2, None).unwrap();
        let b = cache.get(&checker, 2, None).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
    }
    #[test]
    fn budgets_eviction_weak_lifetimes_and_cancellation() {
        let source = image(8, 8);
        let mut cache = Cache::new(83, 1);
        assert!(matches!(cache.get(&source, 3, None), Err(Error::Limit)));
        assert_eq!((cache.stats().bytes, cache.stats().levels), (80, 2));
        let cancelled = AtomicBool::new(true);
        assert!(matches!(cache.get(&source, 1, Some(&cancelled)), Err(Error::Cancelled)));
        assert_eq!(cache.stats().bytes, 80);
        assert!(matches!(cache.get(&source, 0, None), Err(Error::Invalid)));
        assert!(matches!(cache.get(&source, 4, None), Err(Error::Invalid)));
        let other = image(8, 8);
        cache.get(&other, 1, None).unwrap();
        assert_eq!((cache.stats().sources, cache.stats().bytes), (1, 64));
        let weak = Arc::downgrade(&other);
        drop(other);
        assert!(weak.upgrade().is_none());
        cache.prepare(7);
        assert_eq!(cache.stats().bytes, 0);
        assert!(matches!(Cache::new(0, 1).get(&source, 1, None), Err(Error::Limit)));
        assert!(matches!(Cache::new(1024, 0).get(&source, 1, None), Err(Error::Limit)));
    }
}
pub(super) struct Level {
    pub width: u32,
    pub height: u32,
    pub rgba: Box<[u8]>,
}
#[derive(Clone, Copy)]
pub(super) struct Plane<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
}
impl Level {
    pub fn plane(&self) -> Plane<'_> {
        Plane { width: self.width, height: self.height, rgba: &self.rgba }
    }
}
impl<'a> From<&'a Image> for Plane<'a> {
    fn from(image: &'a Image) -> Self {
        Self { width: image.width(), height: image.height(), rgba: image.rgba() }
    }
}
struct Entry {
    source: Weak<Image>,
    levels: Vec<Arc<Level>>,
    bytes: usize,
    stamp: u64,
}
pub(super) struct Cache {
    entries: HashMap<usize, Entry>,
    max_bytes: usize,
    max_sources: usize,
    bytes: usize,
    clock: u64,
    document: u64,
    hits: usize,
    misses: usize,
    generated: u64,
}
impl Cache {
    pub fn new(max_bytes: usize, max_sources: usize) -> Self {
        Self { entries: HashMap::new(), max_bytes, max_sources, bytes: 0, clock: 0, document: 0, hits: 0, misses: 0, generated: 0 }
    }
    fn prune(&mut self) {
        self.entries.retain(|_, e| {
            if e.source.strong_count() != 0 {
                true
            } else {
                self.bytes -= e.bytes;
                false
            }
        });
    }
    pub fn prepare(&mut self, document: u64) {
        if self.document != document {
            self.document = document;
            self.prune();
        }
    }
    pub fn stats(&self) -> ImageCacheStats {
        ImageCacheStats {
            sources: self.entries.len(),
            levels: self.entries.values().map(|e| e.levels.len()).sum(),
            bytes: self.bytes,
            hits: self.hits,
            misses: self.misses,
            generated_pixels: self.generated,
        }
    }
    fn room(&mut self, key: usize, additional: usize) -> Result<()> {
        let own = self.entries.get(&key).map_or(0, |e| e.bytes);
        if own.checked_add(additional).is_none_or(|n| n > self.max_bytes) || self.max_sources == 0 {
            return Err(Error::Limit);
        }
        while self.bytes > self.max_bytes - additional || (!self.entries.contains_key(&key) && self.entries.len() >= self.max_sources) {
            let old = self.entries.iter().filter(|(k, _)| **k != key).min_by_key(|(_, e)| e.stamp).map(|(k, _)| *k).ok_or(Error::Limit)?;
            self.bytes -= self.entries.remove(&old).unwrap().bytes;
        }
        Ok(())
    }
    pub fn get(&mut self, image: &Arc<Image>, level: u32, cancel: Option<&AtomicBool>) -> Result<Arc<Level>> {
        check(cancel)?;
        let max = 31 - image.width().max(image.height()).leading_zeros();
        if level == 0 || level > max {
            return Err(Error::Invalid);
        }
        let key = Arc::as_ptr(image) as usize;
        self.clock = self.clock.wrapping_add(1);
        if let Some(e) = self.entries.get_mut(&key) {
            e.stamp = self.clock;
            if let Some(found) = e.levels.get(level as usize - 1) {
                self.hits = self.hits.saturating_add(1);
                return Ok(found.clone());
            }
        }
        self.misses = self.misses.saturating_add(1);
        self.prune();
        while self.entries.get(&key).map_or(0, |e| e.levels.len()) < level as usize {
            let previous = self.entries.get(&key).and_then(|e| e.levels.last()).cloned();
            let source = previous.as_ref().map_or_else(|| Plane::from(image.as_ref()), |p| p.plane());
            let (w, h) = ((source.width / 2).max(1), (source.height / 2).max(1));
            let bytes = (w as usize).checked_mul(h as usize).and_then(|n| n.checked_mul(4)).ok_or(Error::Limit)?;
            self.room(key, bytes)?; // before allocating, with the source prefix retained
            let made = Arc::new(reduce(source, cancel)?);
            self.generated = self.generated.saturating_add(u64::from(w) * u64::from(h));
            let e = self.entries.entry(key).or_insert_with(|| Entry { source: Arc::downgrade(image), levels: Vec::new(), bytes: 0, stamp: self.clock });
            e.levels.push(made);
            e.bytes += bytes;
            self.bytes += bytes;
        }
        Ok(self.entries[&key].levels[level as usize - 1].clone())
    }
}

fn axis(size: u32) -> (usize, u32) {
    if size == 1 {
        (1, 0)
    } else if size.is_multiple_of(2) {
        (2, 1)
    } else {
        (3, 2)
    }
}
fn reduce(source: Plane<'_>, cancel: Option<&AtomicBool>) -> Result<Level> {
    check(cancel)?;
    let (width, height) = ((source.width / 2).max(1), (source.height / 2).max(1));
    let mut rgba = vec![0; width as usize * height as usize * 4];
    let ((nx, sx), (ny, sy)) = (axis(source.width), axis(source.height));
    let shift = sx + sy;
    let weight = |n: usize, at: usize| -> u32 { if n == 3 && at == 1 { 1 } else { 0 } };
    let pixel = |x: usize, y: usize| {
        let at = (y * source.width as usize + x) * 4;
        u32::from_le_bytes(source.rgba[at..at + 4].try_into().unwrap())
    };
    for y in 0..height as usize {
        check(cancel)?;
        let start_y = if source.height == 1 { 0 } else { y * 2 };
        let mut x = 0;
        if !cfg!(feature = "scalar-image-processing") {
            while x + 4 <= width as usize {
                if x.is_multiple_of(128) {
                    check(cancel)?;
                }
                let mask = wide::u32x4::splat(0x00ff00ff);
                let (mut rb, mut ga) = (wide::u32x4::splat(0), wide::u32x4::splat(0));
                for dy in 0..ny {
                    for dx in 0..nx {
                        let p = wide::u32x4::from(std::array::from_fn(|i| pixel((x + i) * 2 + dx, start_y + dy)));
                        let w = weight(nx, dx) + weight(ny, dy);
                        rb += (p & mask) << w;
                        ga += ((p >> 8u32) & mask) << w;
                    }
                }
                let out = ((rb >> shift) & mask) | (((ga >> shift) & mask) << 8u32);
                for (i, p) in out.to_array().into_iter().enumerate() {
                    rgba[(y * width as usize + x + i) * 4..][..4].copy_from_slice(&p.to_le_bytes());
                }
                x += 4;
            }
        }
        for x in x..width as usize {
            if x.is_multiple_of(128) {
                check(cancel)?;
            }
            let start_x = if source.width == 1 { 0 } else { x * 2 };
            let mut sums = [0u16; 4];
            for dy in 0..ny {
                for dx in 0..nx {
                    let at = ((start_y + dy) * source.width as usize + start_x + dx) * 4;
                    for (c, sum) in sums.iter_mut().enumerate() {
                        *sum += u16::from(source.rgba[at + c]) << (weight(nx, dx) + weight(ny, dy));
                    }
                }
            }
            for (c, sum) in sums.into_iter().enumerate() {
                rgba[(y * width as usize + x) * 4 + c] = (sum >> shift) as u8;
            }
        }
    }
    check(cancel)?;
    Ok(Level { width, height, rgba: rgba.into_boxed_slice() })
}
