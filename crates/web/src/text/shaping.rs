//! Native retained glyph runs. Original source/font/cache orchestration around
//! HarfRust's OpenType shaper; worker-only, no layout or drawing.
use crate::font::{self, FaceId, Selection};
use harfrust::{Buffer, Font, ShapePlan, ShaperFont};
mod paragraph;
pub use crate::text::unicode::{Graphemes, graphemes};
pub use harfrust::{Direction, Feature, Language, Script};
pub use paragraph::{Analysis, BaseDirection, Paragraph, ParagraphJob, ParagraphLimits, ParagraphReport, Progress, StyleRun, VisualRun, features_of};
use smallvec::SmallVec;
use std::{
    collections::HashMap,
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Debug)]
pub enum Error {
    Invalid,
    Limit,
    Cancelled,
    Failed,
    Incomplete,
    Missing,
    Stale,
    Unsupported(&'static str),
    Font(font::Error),
    Shaper(harfrust::ShapeError),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "text shaping {self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
impl From<font::Error> for Error {
    fn from(e: font::Error) -> Self {
        match e {
            font::Error::Cancelled => Self::Cancelled,
            font::Error::Limit => Self::Limit,
            font::Error::Missing => Self::Missing,
            font::Error::Stale => Self::Stale,
            font::Error::Unsupported(s) => Self::Unsupported(s),
            e => Self::Font(e),
        }
    }
}
fn check(cancel: Option<&AtomicBool>) -> Result<()> {
    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub run_bytes: usize,
    pub run_glyphs: usize,
    pub features: usize,
    pub cached_faces: usize,
    pub cached_instances: usize,
    pub cached_plans: usize,
    /// Conservative sum of file bytes per cached base face (TTC may overcount).
    /// Opaque font/shaper caches and allocator overhead are not included.
    pub cached_source_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { run_bytes: 16 * 1024, run_glyphs: 32768, features: 64, cached_faces: 64, cached_instances: 256, cached_plans: 256, cached_source_bytes: 32 * 1024 * 1024 }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct CacheStats {
    pub faces: usize,
    pub instances: usize,
    pub plans: usize,
    pub source_bytes: usize,
    pub font_hits: usize,
    pub plan_hits: usize,
}
#[derive(Clone, PartialEq, Eq, Hash)]
struct FontKey {
    face: FaceId,
    coordinates: SmallVec<[([u8; 4], u32); 5]>,
    bold: bool,
    shear: Option<u64>,
}
impl FontKey {
    fn new(s: &Selection) -> Self {
        Self { face: s.face, coordinates: s.coordinates.iter().map(|c| (c.tag, c.value.to_bits())).collect(), bold: s.synthesis.bold, shear: s.synthesis.shear_degrees.map(f64::to_bits) }
    }
}
#[derive(Clone, PartialEq, Hash)]
struct PlanKey {
    font: FontKey,
    direction: Direction,
    script: Script,
    language: Option<Language>,
    features: SmallVec<[Feature; 4]>,
}
// Feature consists only of integer tag/value/range fields; its PartialEq is
// reflexive, despite upstream not marking it Eq.
impl Eq for PlanKey {}
/// Font leases survive catalog/worker-cache eviction. Instances share their
/// source and parsed base font, rather than copying bytes for each variation.
pub struct GlyphFont {
    selection: Selection,
    model: Font,
}
impl GlyphFont {
    pub fn selection(&self) -> &Selection {
        &self.selection
    }
    pub fn units_per_em(&self) -> u16 {
        self.model.units_per_em()
    }
}
/// Compact glyph data in font units; one run scale converts to CSS pixels.
/// Cluster values are absolute UTF-8 byte offsets in the retained source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyph {
    pub id: u32,
    pub cluster: u32,
    pub x_advance: i32,
    pub y_advance: i32,
    pub x_offset: i32,
    pub y_offset: i32,
    pub unsafe_to_break: bool,
}
pub struct Run {
    pub source: Arc<str>,
    pub range: Range<u32>,
    pub font: Arc<GlyphFont>,
    pub script: Script,
    pub direction: Direction,
    pub glyphs: Box<[Glyph]>,
    /// The CSS font size this run is drawn at (its style run's, else the paragraph's).
    pub size: f64,
    /// Index of the paragraph's style run, or u32::MAX for the paragraph's own style.
    pub style: u32,
}
impl Run {
    pub fn advance(&self, size: f64) -> (f64, f64) {
        let unit = size / f64::from(self.font.units_per_em());
        let (x, y) = self.glyphs.iter().fold((0i64, 0i64), |(x, y), g| (x + i64::from(g.x_advance), y + i64::from(g.y_advance)));
        (x as f64 * unit, y as f64 * unit)
    }
    pub fn glyph_payload_bytes(&self) -> usize {
        std::mem::size_of_val(self.glyphs.as_ref())
    }
}
#[derive(Clone, Copy)]
pub struct RunOptions<'a> {
    pub direction: Direction,
    pub script: Script,
    pub language: Option<&'a str>,
    pub features: &'a [Feature],
    pub beginning_of_text: bool,
    pub end_of_text: bool,
    /// Optional surrounding paragraph bounds; prevents context leaking across
    /// independent paragraphs while preserving context across font runs.
    pub context: Option<(usize, usize)>,
}
impl Default for RunOptions<'_> {
    fn default() -> Self {
        Self { direction: Direction::LeftToRight, script: Script::LATIN, language: None, features: &[], beginning_of_text: true, end_of_text: true, context: None }
    }
}
/// Reuse on one worker. No background work or global cache. clear() releases
/// cache/source leases and the reusable buffer; completed Runs stay valid.
pub struct Engine {
    limits: Limits,
    bases: HashMap<FaceId, Font>,
    instances: HashMap<FontKey, Arc<GlyphFont>>,
    plans: HashMap<PlanKey, Arc<ShapePlan>>,
    buffer: Buffer,
    stats: CacheStats,
}
impl Engine {
    pub fn new(limits: Limits) -> Self {
        Self { limits, bases: HashMap::new(), instances: HashMap::new(), plans: HashMap::new(), buffer: Buffer::new(), stats: CacheStats::default() }
    }
    pub fn clear(&mut self) {
        self.bases.clear();
        self.instances.clear();
        self.plans.clear();
        self.buffer = Buffer::new();
        self.stats = CacheStats::default();
    }
    pub fn cache_stats(&self) -> CacheStats {
        CacheStats { faces: self.bases.len(), instances: self.instances.len(), plans: self.plans.len(), ..self.stats }
    }
    fn font(&mut self, selection: &Selection) -> Result<(Arc<GlyphFont>, bool)> {
        let key = FontKey::new(selection);
        if let Some(font) = self.instances.get(&key) {
            self.stats.font_hits = self.stats.font_hits.saturating_add(1);
            return Ok((font.clone(), true));
        }
        let base = if let Some(font) = self.bases.get(&selection.face) {
            font.clone()
        } else {
            let font = selection.shaping_base().map_err(Error::from)?;
            if self.bases.len() < self.limits.cached_faces && self.stats.source_bytes.saturating_add(selection.data().len()) <= self.limits.cached_source_bytes {
                self.stats.source_bytes += selection.data().len();
                self.bases.insert(selection.face, font.clone());
            }
            font
        };
        let model = base.instance_builder().variations(selection.coordinates.iter().map(|c| (harfrust::Tag::new(&c.tag), c.value))).build();
        let font = Arc::new(GlyphFont { selection: selection.clone(), model });
        let cached = self.bases.contains_key(&selection.face) && self.instances.len() < self.limits.cached_instances;
        if cached {
            self.instances.insert(key, font.clone());
        }
        Ok((font, cached))
    }
    /// Shape one uniform direction/script/font run with source context. Absolute
    /// UTF-8 clusters also define feature ranges. Font size/spacing/line breaking
    /// and CSS processing are the caller's responsibility, not guessed here.
    pub fn shape_run(&mut self, selection: &Selection, source: Arc<str>, range: Range<usize>, options: RunOptions<'_>, cancel: Option<&AtomicBool>) -> Result<Run> {
        check(cancel)?;
        if range.start > range.end
            || range.end > source.len()
            || range.end > u32::MAX as usize
            || !source.is_char_boundary(range.start)
            || !source.is_char_boundary(range.end)
            || options.direction == Direction::Invalid
        {
            return Err(Error::Invalid);
        }
        if range.len() > self.limits.run_bytes || options.features.len() > self.limits.features {
            return Err(Error::Limit);
        }
        let (context_start, context_end) = options.context.unwrap_or((0, source.len()));
        if context_start > range.start || context_end < range.end || context_end > source.len() || !source.is_char_boundary(context_start) || !source.is_char_boundary(context_end) {
            return Err(Error::Invalid);
        }
        if selection.coordinates.iter().any(|c| !c.value.is_finite()) || selection.synthesis.shear_degrees.is_some_and(|a| !a.is_finite()) {
            return Err(Error::Invalid);
        }
        let language = options
            .language
            .filter(|s| !s.is_empty())
            .map(|s| {
                if s.len() > 255 {
                    return Err(Error::Limit);
                }
                s.parse::<Language>().map_err(|_| Error::Invalid)
            })
            .transpose()?;
        let (font, cached) = self.font(selection)?;
        check(cancel)?;
        let key = PlanKey { font: FontKey::new(selection), direction: options.direction, script: options.script, language: language.clone(), features: options.features.iter().copied().collect() };
        let plan = if let Some(plan) = self.plans.get(&key) {
            self.stats.plan_hits = self.stats.plan_hits.saturating_add(1);
            plan.clone()
        } else {
            let plan = Arc::new(ShapePlan::new(&font.model, options.direction, Some(options.script), language.as_ref(), options.features));
            if cached && self.plans.len() < self.limits.cached_plans {
                self.plans.insert(key, plan.clone());
            }
            plan
        };
        self.buffer.reset();
        self.buffer.set_direction(options.direction);
        self.buffer.set_script(Some(options.script));
        self.buffer.set_language(language);
        self.buffer.set_cluster_level(harfrust::ClusterLevel::MonotoneGraphemes);
        let mut flags = harfrust::BufferFlags::empty();
        if options.beginning_of_text {
            flags |= harfrust::BufferFlags::BEGINNING_OF_TEXT;
        }
        if options.end_of_text {
            flags |= harfrust::BufferFlags::END_OF_TEXT;
        }
        self.buffer.set_flags(flags);
        self.buffer.set_pre_context(&source[context_start..range.start]);
        self.buffer.set_post_context(&source[range.end..context_end]);
        for (offset, c) in source[range.clone()].char_indices() {
            check(cancel)?;
            self.buffer.push(c as u32, (range.start + offset) as u32);
        }
        let shaping_font = ShaperFont::new(&font.model);
        let result = harfrust::shape(&shaping_font, &mut self.buffer, harfrust::ShapeOptions::new().plan(Some(&plan)).features(options.features));
        if let Err(error) = result {
            self.buffer = Buffer::new();
            return Err(Error::Shaper(error));
        }
        check(cancel)?;
        if !self.buffer.allocation_successful() || self.buffer.glyph_infos().len() > self.limits.run_glyphs {
            self.buffer = Buffer::new();
            return Err(Error::Limit);
        }
        let glyphs = self
            .buffer
            .glyph_infos()
            .iter()
            .zip(self.buffer.glyph_positions())
            .map(|(i, p)| Glyph {
                id: i.glyph_id,
                cluster: i.cluster,
                x_advance: p.x_advance,
                y_advance: p.y_advance,
                x_offset: p.x_offset,
                y_offset: p.y_offset,
                unsafe_to_break: i.unsafe_to_break(),
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        check(cancel)?;
        Ok(Run { source, range: range.start as u32..range.end as u32, font, script: options.script, direction: options.direction, glyphs, size: 0., style: u32::MAX })
    }
}
impl Default for Engine {
    fn default() -> Self {
        Self::new(Limits::default())
    }
}
