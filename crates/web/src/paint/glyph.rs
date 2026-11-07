use super::*;
use crate::text::shaping::GlyphFont;
use std::{collections::HashMap, sync::Arc};
use tiny_skia::{Path, PathBuilder};
#[derive(Clone)]
pub(super) struct Ink {
    pub path: Option<Arc<Path>>,
    pub bounds: Option<layout::Rect>,
}
struct FontEntry {
    font: Arc<GlyphFont>,
    glyphs: HashMap<u32, Ink>,
    stamp: u64,
    bytes: usize,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct CacheStats {
    pub fonts: usize,
    pub font_bytes: usize,
    pub glyphs: usize,
    pub path_bytes: usize,
    pub hits: usize,
    pub misses: usize,
}
pub struct Engine {
    pub(super) image_mips: Option<Box<super::mipmap::Cache>>,
    fonts: HashMap<usize, FontEntry>,
    limits: Limits,
    clock: u64,
    stats: CacheStats,
}
impl Default for Engine {
    fn default() -> Self {
        Self::new(Default::default())
    }
}
impl Engine {
    pub fn new(limits: Limits) -> Self {
        Self { image_mips: None, fonts: HashMap::new(), limits, clock: 0, stats: CacheStats::default() }
    }
    pub(super) fn limits(&self) -> Limits {
        self.limits
    }
    pub fn clear(&mut self) {
        self.clear_image_cache();
        self.fonts.clear();
        self.clock = 0;
        self.stats = CacheStats::default()
    }
    pub fn cache_stats(&self) -> CacheStats {
        self.stats
    }
    pub fn image_cache_stats(&self) -> super::ImageCacheStats {
        self.image_mips.as_ref().map_or_else(Default::default, |c| c.stats())
    }
    pub fn clear_image_cache(&mut self) {
        self.image_mips = None;
    }
    pub(super) fn ink(&mut self, font: &Arc<GlyphFont>, glyph: u32, cancel: Option<&AtomicBool>) -> Result<Ink> {
        check(cancel)?;
        let id = u16::try_from(glyph).map_err(|_| Error::Invalid)?;
        let key = Arc::as_ptr(font) as usize;
        self.clock = self.clock.wrapping_add(1);
        if let Some(entry) = self.fonts.get_mut(&key) {
            entry.stamp = self.clock;
            if let Some(ink) = entry.glyphs.get(&glyph) {
                self.stats.hits += 1;
                return Ok(ink.clone());
            }
        }
        self.stats.misses += 1;
        let selection = font.selection();
        let face = selection.parsed()?;
        let gid = ttf_parser::GlyphId(id);
        if face.is_color_glyph(gid) || face.glyph_svg_image(gid).is_some() || face.glyph_raster_image(gid, u16::MAX).is_some() {
            // Colour, SVG and bitmap glyphs are not drawn yet: no ink, rather
            // than a page that fails on its first emoji.
            return Ok(Ink { path: None, bounds: None });
        }
        let mut builder = Outline { path: PathBuilder::new(), segments: 0, limit: self.limits.path_segments, cancel, failed: false };
        face.outline_glyph(gid, &mut builder);
        check(cancel)?;
        if builder.failed {
            return Err(Error::Limit);
        }
        let mut path = builder.path.finish();
        // Synthetic oblique: shear the outline by the slope's angle. (Synthetic
        // bold is drawn at the face's own weight.)
        if let (Some(degrees), Some(p)) = (selection.synthesis.shear_degrees, path.as_ref()) {
            let shear = -(degrees.to_radians().tan()) as f32;
            if shear.is_finite() && shear != 0. {
                path = p.clone().transform(tiny_skia::Transform::from_row(1., 0., shear, 1., 0., 0.));
            }
        }
        let bytes = path.as_ref().map_or(0, |p| p.verbs().len() + std::mem::size_of_val(p.points()));
        let bounds = path.as_ref().map(|p| {
            let b = p.bounds();
            layout::Rect { x: f64::from(b.x()), y: f64::from(b.y()), width: f64::from(b.width()), height: f64::from(b.height()) }
        });
        let ink = Ink { path: path.map(Arc::new), bounds };
        if self.limits.fonts == 0 || selection.data().len() > self.limits.font_bytes || self.stats.glyphs >= self.limits.glyphs || self.stats.path_bytes.saturating_add(bytes) > self.limits.path_bytes
        {
            return Ok(ink);
        }
        if !self.fonts.contains_key(&key) {
            while self.fonts.len() >= self.limits.fonts || self.stats.font_bytes.saturating_add(selection.data().len()) > self.limits.font_bytes {
                let old = *self.fonts.iter().min_by_key(|(_, f)| f.stamp).unwrap().0;
                let removed = self.fonts.remove(&old).unwrap();
                self.stats.glyphs -= removed.glyphs.len();
                self.stats.path_bytes -= removed.bytes;
                self.stats.font_bytes -= removed.font.selection().data().len();
            }
            self.stats.font_bytes += selection.data().len();
            self.fonts.insert(key, FontEntry { font: font.clone(), glyphs: HashMap::new(), stamp: self.clock, bytes: 0 });
        }
        let entry = self.fonts.get_mut(&key).unwrap();
        debug_assert!(Arc::ptr_eq(&entry.font, font));
        entry.glyphs.insert(glyph, ink.clone());
        entry.bytes += bytes;
        self.stats.glyphs += 1;
        self.stats.path_bytes += bytes;
        self.stats.fonts = self.fonts.len();
        Ok(ink)
    }
}
struct Outline<'a> {
    path: PathBuilder,
    segments: usize,
    limit: usize,
    cancel: Option<&'a AtomicBool>,
    failed: bool,
}
impl Outline<'_> {
    fn next(&mut self) -> bool {
        self.segments += 1;
        self.failed |= self.segments > self.limit || check(self.cancel).is_err();
        !self.failed
    }
}
impl ttf_parser::OutlineBuilder for Outline<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        if self.next() {
            self.path.move_to(x, y)
        }
    }
    fn line_to(&mut self, x: f32, y: f32) {
        if self.next() {
            self.path.line_to(x, y)
        }
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        if self.next() {
            self.path.quad_to(x1, y1, x, y)
        }
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        if self.next() {
            self.path.cubic_to(x1, y1, x2, y2, x, y)
        }
    }
    fn close(&mut self) {
        if self.next() {
            self.path.close()
        }
    }
}
