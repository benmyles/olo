use super::*;
use std::collections::HashMap;

/// Per-job selection memoization, keyed by complete immutable host context and
/// request contents. Separate equal family allocations can share a Font. Keys
/// retain the family list, so freeing/reusing a source address cannot alias it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    context: u64,
    numbers: [u64; 4],
    kind: u8,
    axes: [u8; 3],
    families: Families,
}
impl Key {
    fn new(r: &FontRequest, context: u64) -> Self {
        // Exhaustive patterns make additions to request/axis inputs require
        // updating this cache key, rather than silently sharing across them.
        let FontRequest { size, weight, width, slant, families, axes } = r;
        let Axes { writing_mode, direction, text_orientation } = *axes;
        let (kind, slope) = match *slant {
            FontSlant::Normal => (0, 0.),
            FontSlant::Italic => (1, 0.),
            FontSlant::Left => (2, 0.),
            FontSlant::Right => (3, 0.),
            FontSlant::Oblique(a) => (4, a),
        };
        Self {
            context,
            numbers: [size.to_bits(), weight.to_bits(), width.to_bits(), slope.to_bits()],
            kind,
            axes: [writing_mode as u8, direction as u8, text_orientation as u8],
            families: families.clone(),
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct FontCacheStats {
    pub hits: usize,
    pub misses: usize,
    pub entries: usize,
    /// HashMap slot payload only; referenced Fonts/families are accounted by
    /// completed styles. Excludes allocator/hash controls; not process RSS.
    pub reserved_bytes: usize,
}
pub(super) struct FontPool {
    values: HashMap<Key, Arc<Font>>,
    limit: usize,
    hits: usize,
    misses: usize,
}
impl FontPool {
    pub fn new(limit: usize) -> Self {
        Self { values: HashMap::new(), limit, hits: 0, misses: 0 }
    }
    pub fn resolve(&mut self, host: &impl Host, request: FontRequest) -> Result<Arc<Font>> {
        if self.limit == 0 {
            return Ok(Arc::new(Font::new(host, request)?));
        }
        // Do not spend unbounded time hashing a valid but enormous fallback
        // list merely for this optional cache. The early exit also bounds
        // traversal for huge lists of tiny names.
        let mut family_bytes = 64 * 1024usize;
        let cacheable = request.families.as_slice().iter().all(|family| {
            let bytes = size_of::<Family>().saturating_add(match family {
                Family::Named(name) => name.len(),
                Family::Generic(_) => 0,
            });
            if bytes > family_bytes {
                false
            } else {
                family_bytes -= bytes;
                true
            }
        });
        if !cacheable {
            return Ok(Arc::new(Font::new(host, request)?));
        }
        let key = Key::new(&request, host.font_context());
        if let Some(font) = self.values.get(&key) {
            self.hits = self.hits.saturating_add(1);
            return Ok(font.clone());
        }
        self.misses = self.misses.saturating_add(1);
        let font = Arc::new(Font::new(host, request)?);
        if font.context != key.context {
            return Err(Error::Stale);
        }
        // Caching is optional: a full pool or failed reservation does not
        // change the computed value or prevent normal uncached allocation.
        if self.values.len() < self.limit && self.values.try_reserve(1).is_ok() {
            self.values.insert(key, font.clone());
        }
        Ok(font)
    }
    pub fn stats(&self) -> FontCacheStats {
        FontCacheStats { hits: self.hits, misses: self.misses, entries: self.values.len(), reserved_bytes: self.values.capacity().saturating_mul(size_of::<(Key, Arc<Font>)>()) }
    }
}
