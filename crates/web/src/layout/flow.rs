use super::containers::QueryBasis;
use super::used::Used;
use super::*;
use crate::{
    css::{
        math,
        style::{Keyword as K, Property as P, StyleTree, Value},
    },
    font::Scope,
    text::{layout as inline, linebreak, prepare, shaping},
};
use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    sync::{Arc, atomic::AtomicBool},
};
#[path = "absolute_flow.rs"]
mod absolute_flow;
#[path = "container_flow.rs"]
mod container_flow;
#[path = "flex_flow.rs"]
mod flex_flow;
#[path = "intrinsic_flow.rs"]
mod intrinsic_flow;
pub(super) fn is_flex(style: &crate::css::style::Style) -> bool {
    matches!(style.value(crate::css::style::Property::Display), crate::css::style::Value::Display(crate::css::properties::Display::Box { inner: crate::css::properties::Inner::Flex, .. }))
}
/// Flex containers whose main axis is horizontal get flex formatting; column
/// containers stack their items like blocks until column layout exists.
pub(in crate::layout) use container_flow::is_grid;
pub(super) fn is_row_flex(style: &crate::css::style::Style) -> bool {
    is_flex(style) && matches!(style.value(P::FlexDirection), Value::Keyword(K::Row | K::RowReverse))
}
/// Project an original-source box boundary through whitespace preparation.
fn boundary(prepared: &prepare::Prepared, at: u32) -> Result<u32> {
    if at as usize == prepared.source().len() {
        return Ok(prepared.text().len() as u32);
    }
    let m = prepared.mapping_at_source(at).ok_or(Error::Invalid)?;
    Ok(if m.kind == prepare::MappingKind::Copy { m.output.start + at - m.input.start } else { m.output.start })
}
fn shaping_breaks(prepared: &prepare::Prepared, source: &[u32]) -> Result<Box<[u32]>> {
    let mut out = source.iter().map(|at| boundary(prepared, *at)).collect::<Result<Vec<_>>>()?;
    for c in prepared.controls().iter().filter(|c| c.kind() == prepare::ControlKind::Object) {
        let at = boundary(prepared, c.offset())?;
        out.extend([at, at + 3]);
    }
    out.retain(|b| *b > 0 && (*b as usize) < prepared.text().len());
    out.sort_unstable();
    out.dedup();
    Ok(out.into_boxed_slice())
}
/// A span's baseline shift from its vertical-align: superscripts and
/// subscripts by the parent's size, lengths and percentages (of the
/// span's line-height) as given; other keywords shift nothing here.
fn baseline_shift(s: &crate::css::style::Style, parent_size: f64) -> f64 {
    match s.value(P::VerticalAlign) {
        Value::Keyword(K::Super) => parent_size / 3.,
        Value::Keyword(K::Sub) => -parent_size / 5.,
        Value::Numeric(n) => {
            struct Basis(f64);
            impl math::Context for Basis {
                fn factor(&self, unit: math::RelativeUnit) -> math::Result<f64> {
                    if unit == math::RelativeUnit::Percentage { Ok(self.0 / 100.) } else { Err(math::Error::ContextUnavailable) }
                }
            }
            n.resolve(&Basis(s.line_height()), &mut math::Scratch::default(), Default::default(), None).unwrap_or(0.)
        }
        _ => 0.,
    }
}
/// The style runs of a paragraph: every span styled differently from its
/// container (font, size, line-height, baseline shift), in prepared-text
/// coordinates, adjacent equal ones merged.
fn style_runs(
    tree: &Tree,
    scope: &Scope,
    style: &crate::css::style::Style,
    source: &TextSource,
    controls: &[Control],
    prepared: &prepare::Prepared,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<shaping::StyleRun>> {
    let mut runs: Vec<shaping::StyleRun> = Vec::new();
    let parent_size = style.font().request.size;
    for span in &source.spans {
        check(cancel)?;
        let s = tree.styles.snapshot(span.style_node).ok_or(Error::Stale)?;
        if !controls.is_empty() && controls.binary_search_by_key(&span.range.start, |c| c.offset).is_ok() {
            continue;
        }
        let shift = baseline_shift(s, parent_size);
        let features = shaping::features_of(s);
        let same_font = s.font().request == style.font().request && s.font().context() == style.font().context();
        if same_font && s.line_height() == style.line_height() && shift == 0. && features == shaping::features_of(style) {
            continue;
        }
        let start = boundary(prepared, span.range.start)?;
        let end = boundary(prepared, span.range.end)?;
        if start >= end {
            continue;
        }
        if let Some(last) = runs.last_mut()
            && last.range.end == start
            && last.request == s.font().request
            && last.line_height == s.line_height()
            && last.shift == shift
            && last.features == features
        {
            last.range.end = end;
            continue;
        }
        let request = s.font().request.clone();
        let metrics = scope.metrics(&request, cancel).map_err(|e| match e {
            crate::font::Error::Cancelled => Error::Cancelled,
            crate::font::Error::Limit => Error::Limit,
            crate::font::Error::Stale => Error::Stale,
            _ => Error::Invalid,
        })?;
        runs.push(shaping::StyleRun {
            range: start..end,
            request,
            size: s.font().request.size,
            line_height: s.line_height(),
            ascent: metrics.ascent.max(0.),
            descent: (-metrics.descent).max(0.),
            shift,
            features,
        });
    }
    Ok(runs)
}
#[allow(clippy::too_many_arguments)] // The paragraph, its breaks, styles and worker resources.
fn shape_text(
    engine: &mut shaping::Engine,
    scope: &Scope,
    style: &crate::css::style::Style,
    prepared: Arc<prepare::Prepared>,
    boundaries: Box<[u32]>,
    styles: Vec<shaping::StyleRun>,
    limits: inline::Limits,
    cancel: Option<&AtomicBool>,
) -> Result<Arc<inline::Text>> {
    let mut s = shaping::ParagraphJob::for_layout_style(engine, scope, style, prepared.text().clone(), Default::default(), cancel)?.with_shaping_breaks(boundaries)?.with_style_runs(styles)?;
    if prepared.controls().iter().any(|c| c.kind() == prepare::ControlKind::Object) {
        let positions = prepared.controls().iter().filter(|c| c.kind() == prepare::ControlKind::Object).map(|c| boundary(&prepared, c.offset())).collect::<Result<Box<[_]>>>()?;
        s = s.with_inline_objects(positions)?;
    }
    while !s.step(128)?.finished {}
    let mut paragraph = s.finish()?;
    // letter-spacing adds to every glyph's advance, word-spacing to spaces'
    // (CSS Text 3 §8), in each run's font units.
    let size = style.font().request.size;
    let letter = style.used_text_spacing(P::LetterSpacing, size, Default::default(), cancel).unwrap_or(0.);
    let word = style.used_text_spacing(P::WordSpacing, size, Default::default(), cancel).unwrap_or(0.);
    if (letter != 0. || word != 0.) && size > 0. {
        for run in paragraph.runs.iter_mut() {
            let unit = f64::from(run.font.units_per_em()) / run.size.max(1e-6);
            let letter_units = (letter * unit).round() as i32;
            let word_units = (word * unit).round() as i32;
            let source = run.source.clone();
            let mut glyphs = run.glyphs.to_vec();
            for g in &mut glyphs {
                let space = source.as_bytes().get(g.cluster as usize).is_some_and(|b| matches!(b, b' ' | b'\t'));
                g.x_advance = g.x_advance.saturating_add(letter_units).saturating_add(if space { word_units } else { 0 });
            }
            run.glyphs = glyphs.into_boxed_slice();
        }
    }
    let s = Arc::new(paragraph);
    let mut b = linebreak::Job::new(prepared.text().clone(), Default::default(), cancel)?;
    while !b.step(4096)?.finished {}
    let b = Arc::new(b.finish()?);
    let mut t = inline::TextJob::new(engine, prepared, s, b, limits, cancel)?;
    while !t.step(128)?.finished {}
    Ok(t.finish()?)
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    pub boxes: usize,
    pub texts: usize,
    pub lines: usize,
    pub reused_texts: usize,
    pub work: usize,
    pub finished: bool,
}
/// Width-independent text/font geometry for a frozen box/style generation.
pub struct Cache {
    tree: Arc<Tree>,
    texts: Arc<[Arc<inline::Text>]>,
    /// SVG texts shaped into paths, by viewport index then text slot.
    svg_texts: Arc<[Box<[Option<tiny_skia::Path>]>]>,
    /// The prepared texts by their identity (content, breaks, fonts and
    /// spacing), for the next preparation of a changed document to keep
    /// what is the same.
    shared: Arc<HashMap<TextKey, Arc<inline::Text>>>,
    intrinsics: Option<Arc<HashMap<usize, inline::IntrinsicWidths>>>,
    glyph_geometry_bytes: usize,
    fallbacks: Fallbacks,
}
impl Cache {
    /// The shaped texts of an SVG viewport, cloned for its resolved geometry.
    pub(crate) fn svg_texts(&self, vector: u32) -> Vec<Option<tiny_skia::Path>> {
        self.svg_texts.get(vector as usize).map(|t| t.to_vec()).unwrap_or_default()
    }
    pub fn tree(&self) -> &Arc<Tree> {
        &self.tree
    }
    /// What text preparation substituted (typography flattened into its paragraph).
    pub fn fallbacks(&self) -> &Fallbacks {
        &self.fallbacks
    }
    pub fn text(&self, source: u32) -> Option<&Arc<inline::Text>> {
        self.texts.get(source as usize)
    }
    /// Distinct retained text geometries measured for flex intrinsic sizing.
    /// Equal shared Text objects use one entry; width-only reflow keeps it.
    pub fn intrinsic_measurements(&self) -> usize {
        self.intrinsics.as_ref().map_or(0, |m| m.len())
    }
    /// Occupied key/width-pair payload only. Excludes hash-table spare capacity,
    /// control bytes, headers, allocator overhead and the shared text/fonts.
    pub fn intrinsic_payload_bytes(&self) -> usize {
        self.intrinsic_measurements() * size_of::<(usize, inline::IntrinsicWidths)>()
    }
    /// Only native glyph + text-index payload; excludes the tree, preparation
    /// maps, font/source allocations, headers, capacity and allocator overhead.
    pub fn glyph_geometry_payload_bytes(&self) -> usize {
        self.glyph_geometry_bytes
    }
}
/// A prepared text's identity: its container's font, its content, and a
/// hash of its spans, breaks, controls, styled runs and spacing.
type TextKey = (u64, Arc<str>, u64);
/// What identifies a font for text preparation: everything in its request
/// and the scope it resolved in (not the allocation: a restyled element
/// gets a new one for the same font).
fn font_identity(font: &crate::css::style::Font) -> u64 {
    let mut hash = DefaultHasher::new();
    let r = &font.request;
    r.size.to_bits().hash(&mut hash);
    r.weight.to_bits().hash(&mut hash);
    r.width.to_bits().hash(&mut hash);
    match r.slant {
        crate::css::style::FontSlant::Normal => 0u8.hash(&mut hash),
        crate::css::style::FontSlant::Italic => 1u8.hash(&mut hash),
        crate::css::style::FontSlant::Left => 2u8.hash(&mut hash),
        crate::css::style::FontSlant::Right => 3u8.hash(&mut hash),
        crate::css::style::FontSlant::Oblique(a) => {
            4u8.hash(&mut hash);
            a.to_bits().hash(&mut hash);
        }
    }
    r.families.hash(&mut hash);
    (r.axes.writing_mode as u8, r.axes.direction as u8).hash(&mut hash);
    font.context().hash(&mut hash);
    hash.finish()
}
pub struct PrepareJob<'a> {
    engine: &'a mut shaping::Engine,
    scope: &'a Scope,
    tree: Arc<Tree>,
    texts: Vec<Arc<inline::Text>>,
    /// An earlier preparation: a source whose text, spans, styles, edges
    /// and controls are as they were takes its text from it by index,
    /// without computing a key.
    previous: Option<Arc<Cache>>,
    shared: HashMap<TextKey, Arc<inline::Text>>,
    glyph_geometry_bytes: usize,
    fallbacks: Fallbacks,
    cancel: Option<&'a AtomicBool>,
    limits: Limits,
    progress: Progress,
    failed: bool,
}
impl<'a> PrepareJob<'a> {
    pub fn new(engine: &'a mut shaping::Engine, scope: &'a Scope, tree: Arc<Tree>, limits: Limits, cancel: Option<&'a AtomicBool>) -> Result<Self> {
        check(cancel)?;
        if tree.boxes.len() > limits.boxes || tree.text_bytes() > limits.text_bytes {
            return Err(Error::Limit);
        }
        Ok(Self {
            engine,
            scope,
            tree,
            texts: Vec::new(),
            previous: None,
            shared: HashMap::new(),
            glyph_geometry_bytes: 0,
            fallbacks: Fallbacks::default(),
            cancel,
            limits,
            progress: Progress::default(),
            failed: false,
        })
    }
    pub fn fallbacks(&self) -> &Fallbacks {
        &self.fallbacks
    }
    /// Start from an earlier preparation's texts: those whose identity is
    /// unchanged are kept, not shaped again.
    pub fn with_previous(mut self, previous: &Arc<Cache>) -> Self {
        if self.shared.is_empty() && previous.shared.len() <= self.limits.cached_texts {
            self.shared = previous.shared.as_ref().clone();
        }
        self.previous = Some(previous.clone());
        self
    }
    /// Whether source `index` is, to the shaper, the same as the previous
    /// preparation's at that index: the same text, spans and controls,
    /// and styles the same as before (a kept style is the same object).
    fn unchanged_since(&self, index: usize, old_tree: &Tree) -> bool {
        let (Some(old), Some(new)) = (old_tree.sources.get(index), self.tree.sources.get(index)) else { return false };
        if !(Arc::ptr_eq(&old.source, &new.source) || *old.source == *new.source) || old.spans.len() != new.spans.len() {
            return false;
        }
        let (Some(container_old), Some(container_new)) = (old_tree.style(old.container), self.tree.style(new.container)) else { return false };
        if !container_old.same_as(container_new) {
            return false;
        }
        for (a, b) in old.spans.iter().zip(new.spans.iter()) {
            if a.range != b.range || a.node != b.node {
                return false;
            }
            match (old_tree.styles.snapshot(a.style_node), self.tree.styles.snapshot(b.style_node)) {
                (Some(x), Some(y)) if x.same_as(y) => {}
                _ => return false,
            }
        }
        let (controls_old, controls_new) = (old_tree.controls(index as u32), self.tree.controls(index as u32));
        if controls_old.len() != controls_new.len() || controls_old.iter().zip(controls_new).any(|(x, y)| x.offset != y.offset || x.kind != y.kind) {
            return false;
        }
        match (old_tree.inline_boxes(index as u32), self.tree.inline_boxes(index as u32)) {
            (Some(o), Some(n)) => {
                if o.len() != n.len() {
                    return false;
                }
                for (x, y) in o.iter().zip(n) {
                    if x.range != y.range || x.first != y.first || x.last != y.last || x.node != y.node {
                        return false;
                    }
                    match (old_tree.styles.snapshot(x.node), self.tree.styles.snapshot(y.node)) {
                        (Some(p), Some(q)) if p.same_as(q) => {}
                        _ => return false,
                    }
                }
            }
            (None, None) => {}
            _ => return false,
        }
        true
    }
    fn one(&mut self, index: usize) -> Result<Arc<inline::Text>> {
        // The same source as last time: its text, by index.
        if let Some(previous) = self.previous.clone()
            && let Some(text) = previous.texts.get(index)
            && self.unchanged_since(index, &previous.tree)
        {
            self.progress.reused_texts += 1;
            return Ok(text.clone());
        }
        let source = &self.tree.sources[index];
        let container = source.container;
        let style = self.tree.style(container).ok_or(Error::Invalid)?;
        let whitespace = prepare::WhiteSpace::for_style(style)?;
        let source_controls = self.tree.controls(index as u32);
        let controls: Box<[_]> = source_controls.iter().map(|c| prepare::Control::new(c.offset, c.kind)).collect::<std::result::Result<_, _>>()?;
        let mut raw_breaks = Vec::new();
        for b in self.tree.inline_boxes(index as u32).ok_or(Error::Invalid)? {
            check(self.cancel)?;
            let s = self.tree.styles.snapshot(b.node).ok_or(Error::Stale)?;
            if b.first && used::has_inline_edge(s, true) {
                raw_breaks.push(b.range.start);
            }
            if b.last && used::has_inline_edge(s, false) {
                raw_breaks.push(b.range.end);
            }
        }
        raw_breaks.sort_unstable();
        raw_breaks.dedup();
        // The paragraph shaper owns one font request/size. A differently
        // styled inline (bold, another size, another white-space) is shaped
        // with its paragraph's typography until styled runs exist; that is
        // recorded, never silent.
        // Spans keep their own font, size, line-height and baseline shift as
        // style runs; white-space and direction stay the paragraph's.
        let mut flattened = false;
        for span in &source.spans {
            check(self.cancel)?;
            let s = self.tree.styles.snapshot(span.style_node).ok_or(Error::Stale)?;
            if source_controls.binary_search_by_key(&span.range.start, |c| c.offset).ok().is_some_and(|i| source_controls[i].kind == prepare::ControlKind::Object) {
                continue;
            }
            let control = !source_controls.is_empty() && source_controls.binary_search_by_key(&span.range.start, |c| c.offset).is_ok();
            if !flattened && s.axes() != style.axes() {
                flattened = true;
                self.fallbacks.note("inline direction flattened into its paragraph", Some(span.node));
            }
            if !flattened && !control && prepare::WhiteSpace::for_style(s)? != whitespace {
                flattened = true;
                self.fallbacks.note("inline white-space flattened into its paragraph", Some(span.node));
            }
            let options = inline::Options::for_style(s, self.scope, 0., self.cancel)?;
            for what in options.ignored.names() {
                self.fallbacks.note(what, Some(span.node));
            }
        }
        let container_node = self.tree.get(container).map(|b| b.node);
        for what in inline::Options::for_style(style, self.scope, 0., self.cancel)?.ignored.names() {
            self.fallbacks.note(what, container_node);
        }
        let key = (self.limits.cached_texts > 0).then(|| {
            let mut hash = DefaultHasher::new();
            for span in &source.spans {
                span.range.hash(&mut hash)
            }
            raw_breaks.hash(&mut hash);
            if !controls.is_empty() {
                controls.hash(&mut hash);
            }
            // Styled spans shape in their own fonts: part of the identity too.
            for span in &source.spans {
                if let Some(s) = self.tree.styles.snapshot(span.style_node) {
                    font_identity(s.font()).hash(&mut hash);
                    s.line_height().to_bits().hash(&mut hash);
                    baseline_shift(s, style.font().request.size).to_bits().hash(&mut hash);
                    shaping::features_of(s).hash(&mut hash);
                }
            }
            // Spacing shapes into the glyph advances, so it is part of the identity.
            let size = style.font().request.size;
            style.used_text_spacing(crate::css::style::Property::LetterSpacing, size, Default::default(), self.cancel).unwrap_or(0.).to_bits().hash(&mut hash);
            style.used_text_spacing(crate::css::style::Property::WordSpacing, size, Default::default(), self.cancel).unwrap_or(0.).to_bits().hash(&mut hash);
            shaping::features_of(style).hash(&mut hash);
            (font_identity(style.font()), source.source.clone(), hash.finish())
        });
        if let Some(text) = key.as_ref().and_then(|key| self.shared.get(key))
            && text.prepared().spans().len() == source.spans.len()
            && text.prepared().spans().iter().zip(source.spans.iter()).all(|(a, b)| a.range == b.range && a.whitespace == whitespace)
            && text.paragraph().analysis.paragraphs().all(|(_, level)| (level % 2 != 0) == (style.axes().direction == crate::css::style::Direction::Rtl))
            && text.paragraph().shaping_breaks() == shaping_breaks(text.prepared(), &raw_breaks)?.as_ref()
            && text.prepared().controls() == controls.as_ref()
        {
            self.progress.reused_texts += 1;
            return Ok(text.clone());
        }
        let spans: Arc<[prepare::Span]> = source.spans.iter().map(|s| prepare::Span { range: s.range.clone(), whitespace }).collect();
        let mut p = prepare::Job::new(source.source.clone(), spans, prepare::Limits { text_bytes: self.limits.text.text_bytes, ..Default::default() }, self.cancel)?.with_controls(controls)?;
        while !p.step(4096)?.finished {}
        let p = Arc::new(p.finish()?);
        let boundaries = shaping_breaks(&p, &raw_breaks)?;
        let styles = style_runs(&self.tree, self.scope, style, source, source_controls, &p, self.cancel)?;
        let text = shape_text(self.engine, self.scope, style, p, boundaries, styles, self.limits.text, self.cancel)?;
        self.glyph_geometry_bytes += text.geometry_payload_bytes() + text.paragraph().glyph_payload_bytes();
        if self.shared.len() < self.limits.cached_texts
            && let Some(key) = key
        {
            self.shared.insert(key, text.clone());
        }
        Ok(text)
    }
    pub fn progress(&self) -> Progress {
        self.progress
    }
    pub fn step(&mut self, budget: usize) -> Result<Progress> {
        if self.failed {
            return Err(Error::Failed);
        }
        let r = self.advance(budget);
        if r.is_err() {
            self.failed = true
        }
        r
    }
    fn advance(&mut self, budget: usize) -> Result<Progress> {
        check(self.cancel)?;
        for _ in 0..budget {
            check(self.cancel)?;
            if self.texts.len() == self.tree.sources.len() {
                self.progress.finished = true;
                break;
            }
            if self.progress.work >= self.limits.work {
                return Err(Error::Limit);
            }
            let t = self.one(self.texts.len())?;
            self.texts.push(t);
            self.progress.texts = self.texts.len();
            self.progress.work += 1;
        }
        Ok(self.progress)
    }
    pub fn finish(self) -> Result<Arc<Cache>> {
        if self.failed {
            return Err(Error::Failed);
        }
        check(self.cancel)?;
        if !self.progress.finished {
            return Err(Error::Incomplete);
        }
        // SVG texts: shaped here, where the fonts are.
        let mut svg_texts: Vec<Box<[Option<tiny_skia::Path>]>> = Vec::new();
        let mut fallbacks = self.fallbacks;
        for vector in self.tree.vectors().iter() {
            let mut paths = Vec::with_capacity(vector.texts.len());
            for source in &vector.texts {
                let path = match self.tree.styles.snapshot(source.node) {
                    Some(style) => match crate::svg::text::shape(self.engine, self.scope, style, source, 1 << 20, self.cancel) {
                        Ok(path) => path,
                        Err(e @ (Error::Cancelled | Error::Stale)) => return Err(e),
                        Err(_) => {
                            fallbacks.note("SVG text not shaped", Some(source.node));
                            None
                        }
                    },
                    None => None,
                };
                paths.push(path);
            }
            svg_texts.push(paths.into_boxed_slice());
        }
        Ok(Arc::new(Cache {
            tree: self.tree,
            texts: Arc::from(self.texts),
            svg_texts: Arc::from(svg_texts),
            shared: Arc::new(self.shared),
            intrinsics: None,
            glyph_geometry_bytes: self.glyph_geometry_bytes,
            fallbacks,
        }))
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Geometry {
    /// Border box relative to the parent's content origin, including relative
    /// positioning. Normal-flow placement uses the unshifted position.
    pub border_box: Rect,
    pub padding: Edges,
    pub border_width: Edges,
    pub margin: Edges,
}
impl Geometry {
    pub fn content_box(self) -> Rect {
        Rect {
            x: self.border_box.x + self.border_width.left + self.padding.left,
            y: self.border_box.y + self.border_width.top + self.padding.top,
            width: (self.border_box.width - self.border_width.horizontal() - self.padding.horizontal()).max(0.),
            height: (self.border_box.height - self.border_width.vertical() - self.padding.vertical()).max(0.),
        }
    }
}
/// A `position: sticky` box: its place in the flow (document px), its
/// containing block and its insets; `offset` says how far the scroll
/// moves it from there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sticky {
    pub root: BoxId,
    pub rect: Rect,
    pub container: Rect,
    pub top: Option<f64>,
    pub bottom: Option<f64>,
}
impl Sticky {
    /// The shift (document px, down positive) at scroll `scroll` in a
    /// viewport `height` tall: stuck to its inset once the scroll passes
    /// it, never beyond its containing block.
    pub fn offset(&self, scroll: f64, height: f64) -> f64 {
        let mut offset = 0.;
        if let Some(top) = self.top {
            let wanted = scroll + top - self.rect.y;
            let limit = (self.container.y + self.container.height - (self.rect.y + self.rect.height)).max(0.);
            offset = wanted.clamp(0., limit);
        }
        if let Some(bottom) = self.bottom
            && height.is_finite()
        {
            let wanted = (scroll + height - bottom) - (self.rect.y + self.rect.height);
            let limit = (self.container.y - self.rect.y).min(0.);
            if wanted < 0. {
                offset = wanted.max(limit);
            }
        }
        offset
    }
}
pub struct Layout {
    cache: Arc<Cache>,
    geometry: Box<[Geometry]>,
    texts: Arc<[Option<inline::Layout>]>,
    scale: f64,
}
impl Layout {
    pub fn device_scale(&self) -> f64 {
        self.scale
    }
    /// This layout with other styles for the same boxes: a restyle that
    /// changed only what is painted keeps every position and size.
    pub fn with_styles(&self, styles: Arc<StyleTree>, document: &crate::dom::Document, images: Option<Arc<crate::image::Set>>) -> Self {
        let tree = Arc::new(self.cache.tree.with_styles(styles, document, images));
        let cache = Arc::new(Cache {
            tree,
            texts: self.cache.texts.clone(),
            svg_texts: self.cache.svg_texts.clone(),
            shared: self.cache.shared.clone(),
            intrinsics: self.cache.intrinsics.clone(),
            glyph_geometry_bytes: self.cache.glyph_geometry_bytes,
            fallbacks: self.cache.fallbacks.clone(),
        });
        Self { cache, geometry: self.geometry.clone(), texts: self.texts.clone(), scale: self.scale }
    }
    pub fn cache(&self) -> &Arc<Cache> {
        &self.cache
    }
    /// The styles changed in place, when nothing else holds this layout's
    /// cache, tree or styles (else None, and a copy is the way): a patch
    /// over a few elements without a new tree. The tree follows the
    /// document's revision.
    pub fn patch_styles<R>(&mut self, document: &crate::dom::Document, patch: impl FnOnce(&mut StyleTree) -> R) -> Option<R> {
        let cache = Arc::get_mut(&mut self.cache)?;
        let tree = Arc::get_mut(&mut cache.tree)?;
        let styles = tree.styles_mut(document)?;
        Some(patch(styles))
    }
    pub fn geometry(&self, id: BoxId) -> Option<&Geometry> {
        self.geometry.get(id.index())
    }
    /// The identity of the laid-out texts: the same while the layout is
    /// kept, new after a relayout.
    pub fn texts_identity(&self) -> usize {
        Arc::as_ptr(&self.texts) as *const u8 as usize
    }
    pub fn text_layout(&self, id: BoxId) -> Option<&inline::Layout> {
        let BoxKind::InlineContent { source } = self.cache.tree.get(id)?.kind else { return None };
        self.texts.get(source as usize)?.as_ref()
    }
    /// Walk ancestors on demand. A painter should traverse once carrying the
    /// content origin, rather than call this independently for every glyph.
    pub fn bounds(&self, id: BoxId) -> Option<Rect> {
        let mut r = self.geometry(id)?.border_box;
        let mut at = self.cache.tree.get(id)?.parent;
        while let Some(p) = at {
            let c = self.geometry(p)?.content_box();
            r.x += c.x;
            r.y += c.y;
            at = self.cache.tree.get(p)?.parent;
        }
        Some(r)
    }
    /// The element under a point of the page (document coordinates): the
    /// smallest box containing it, later boxes winning ties (painted above),
    /// by its node. Transforms are not followed yet.
    pub fn hit_test(&self, x: f64, y: f64) -> Option<crate::dom::NodeId> {
        self.hit_test_scrolled(x, y, 0., f64::INFINITY)
    }
    /// The node at document point (x, y) with the page scrolled by
    /// `scroll` (CSS px) in a viewport `height` tall: a box fixed to the
    /// viewport is where the viewport is, so it is tested against the
    /// viewport point; a sticky box is where the scroll put it.
    pub fn hit_test_scrolled(&self, x: f64, y: f64, scroll: f64, height: f64) -> Option<crate::dom::NodeId> {
        let tree = &self.cache.tree;
        let fixed = self.fixed_boxes();
        // Each box's sticky shift, through its sticky root.
        let stickies = self.sticky();
        let mut shift = vec![0f64; self.geometry.len()];
        for (id, b) in tree.iter() {
            let own = stickies.iter().find(|s| s.root == id).map(|s| s.offset(scroll, height));
            shift[id.index()] = own.unwrap_or_else(|| b.parent.map_or(0., |p| shift[p.index()]));
        }
        let mut best: Option<(f64, bool, usize, BoxId)> = None;
        // Absolute positions by one walk, parents before children.
        let mut origins = vec![(0f64, 0f64); self.geometry.len()];
        for (id, b) in tree.iter() {
            let g = self.geometry[id.index()];
            let (ox, oy) = b.parent.map_or((0., 0.), |p| origins[p.index()]);
            let c = g.content_box();
            origins[id.index()] = (ox + c.x, oy + c.y);
            if matches!(b.kind, BoxKind::Viewport) || b.marker {
                continue;
            }
            let r = Rect { x: ox + g.border_box.x, y: oy + g.border_box.y, width: g.border_box.width, height: g.border_box.height };
            let is_fixed = fixed[id.index()];
            let y = if is_fixed { y - scroll } else { y - shift[id.index()] };
            if x < r.x || y < r.y || x >= r.x + r.width || y >= r.y + r.height {
                continue;
            }
            // A fixed box paints over the page: it wins over anything under it.
            let area = r.width * r.height;
            if best.is_none_or(|(a, f, _, _)| (is_fixed && !f) || (is_fixed == f && area <= a)) {
                best = Some((area, is_fixed, id.index(), id));
            }
        }
        best.and_then(|(_, _, _, id)| tree.get(id)).map(|b| b.node)
    }
    /// The `position: sticky` boxes (not under fixed ones): where each
    /// sits in the flow, its containing block and its insets, for the
    /// shift the scroll gives it (CSS Positioned Layout 3 §3.4).
    pub fn sticky(&self) -> Vec<Sticky> {
        let tree = &self.cache.tree;
        let fixed = self.fixed_boxes();
        let mut out = Vec::new();
        let mut scratch = math::Scratch::default();
        for (id, b) in tree.iter() {
            if fixed[id.index()] {
                continue;
            }
            let Some(style) = tree.style(id) else { continue };
            if !matches!(style.value(P::Position), Value::Keyword(K::Sticky)) {
                continue;
            }
            let (Some(rect), Some(parent)) = (self.bounds(id), b.parent) else { continue };
            let Some(parent_bounds) = self.bounds(parent) else { continue };
            let Some(pg) = self.geometry(parent) else { continue };
            let container = Rect {
                x: parent_bounds.x + pg.border_width.left + pg.padding.left,
                y: parent_bounds.y + pg.border_width.top + pg.padding.top,
                width: (parent_bounds.width - pg.border_width.horizontal() - pg.padding.horizontal()).max(0.),
                height: (parent_bounds.height - pg.border_width.vertical() - pg.padding.vertical()).max(0.),
            };
            let queries = QueryBasis::viewport(Viewport { width: container.width, height: container.height, scale: self.scale });
            let inset = |p: P, scratch: &mut math::Scratch| used::length(style, p, Some(container.height), None, queries, scratch, None, self.scale).ok().flatten();
            let top = inset(P::Top, &mut scratch);
            let bottom = inset(P::Bottom, &mut scratch);
            if top.is_none() && bottom.is_none() {
                continue;
            }
            out.push(Sticky { root: id, rect, container, top, bottom });
        }
        out
    }
    /// Which boxes are fixed to the viewport (a `position: fixed` box and
    /// its descendants, unless a transformed ancestor contains it).
    pub fn fixed_boxes(&self) -> Box<[bool]> {
        let tree = &self.cache.tree;
        let mut fixed = vec![false; self.geometry.len()];
        let mut transformed = vec![false; self.geometry.len()];
        for (id, b) in tree.iter() {
            let (parent_fixed, parent_transformed) = b.parent.map_or((false, false), |p| (fixed[p.index()], transformed[p.index()]));
            let style = tree.style(id);
            let own_transform = style.is_some_and(|s| matches!(s.value(P::Transform), Value::Transforms(_)));
            let own_fixed = !parent_transformed && style.is_some_and(|s| matches!(s.value(P::Position), Value::Keyword(K::Fixed)));
            fixed[id.index()] = parent_fixed || own_fixed;
            transformed[id.index()] = parent_transformed || own_transform;
        }
        fixed.into_boxed_slice()
    }
    pub fn geometry_payload_bytes(&self) -> usize {
        self.geometry.len() * size_of::<Geometry>()
            + self
                .texts
                .iter()
                .enumerate()
                .filter_map(|(i, t)| t.as_ref().map(|t| (i, t)))
                .map(|(i, t)| t.geometry_payload_bytes() + if Arc::ptr_eq(t.text(), &self.cache.texts[i]) { 0 } else { t.text().geometry_payload_bytes() + t.text().paragraph().glyph_payload_bytes() })
                .sum::<usize>()
    }
}
#[derive(Clone, Copy, Debug, Default)]
struct Strut {
    positive: f64,
    negative: f64,
}
impl Strut {
    fn new(v: f64) -> Self {
        Self { positive: v.max(0.), negative: v.min(0.) }
    }
    fn union(self, b: Self) -> Self {
        Self { positive: self.positive.max(b.positive), negative: self.negative.min(b.negative) }
    }
    fn value(self) -> f64 {
        self.positive + self.negative
    }
}
struct Frame {
    id: BoxId,
    next: Option<BoxId>,
    used: Used,
    cursor: f64,
    pending: Strut,
    top: Strut,
    at_start: bool,
    collapse_top: bool,
    collapse_bottom: bool,
    through: bool,
    queries: QueryBasis,
    /// A text box laying out its atomic inlines before its lines.
    atomic_host: bool,
    /// A block formatting context root: floats inside it stay here.
    bfc: bool,
    /// This frame's content box relative to its block formatting context's
    /// content box (prospective: before margins collapse).
    bfc_origin: Point,
    /// The floats of this formatting context (a root only), margin boxes in
    /// its content coordinates.
    floats: Vec<FloatBox>,
    /// This box floats (left or not), placed when its content is built.
    float_left: Option<bool>,
}
#[derive(Clone, Copy, Debug)]
struct FloatBox {
    left: bool,
    rect: Rect,
}
struct Completed {
    before: Strut,
    after: Strut,
    through: bool,
    relative_y: f64,
}
/// The sizes of a text box's atomic inlines for line layout, and their
/// geometry with the prepared-text offset each sits at.
type InlineObjects = (Vec<inline::Object>, Vec<(BoxId, Geometry, u32)>);
/// The containing frame's values a text box needs, copied so layout can
/// borrow itself mutably while sizing the text's atomic inlines.
#[derive(Clone, Copy)]
struct ParentFrame {
    id: BoxId,
    used: Used,
    queries: QueryBasis,
}
thread_local! {
    /// OLO_WEB_TIMING: layout work by kind, (count, seconds): blocks
    /// entered, texts laid out, replaced boxes, frames finished, flex
    /// ticks, absolutes begun, intrinsic sizings.
    pub(super) static LAYOUT_PROFILE: std::cell::Cell<[(u32, f64); 7]> = const { std::cell::Cell::new([(0, 0.); 7]) };
}
pub(super) fn profile(slot: usize, started: Option<std::time::Instant>) {
    if let Some(t) = started {
        LAYOUT_PROFILE.with(|p| {
            let mut v = p.get();
            v[slot].0 += 1;
            v[slot].1 += t.elapsed().as_secs_f64();
            p.set(v);
        });
    }
}
pub struct Job<'a> {
    engine: &'a mut shaping::Engine,
    scope: &'a Scope,
    cache: Arc<Cache>,
    limits: Limits,
    cancel: Option<&'a AtomicBool>,
    /// OLO_WEB_TIMING: the work profiled by kind.
    profiling: bool,
    geometry: Vec<Geometry>,
    texts: Vec<Option<inline::Layout>>,
    stack: Vec<Frame>,
    flex: Vec<flex_flow::Frame<'a>>,
    containers: Vec<container_flow::Container>,
    tables: Vec<container_flow::Table>,
    absolute: Vec<absolute_flow::Deferred>,
    absolute_at: usize,
    absolute_indices: HashMap<BoxId, usize>,
    active_absolute: Option<absolute_flow::Active>,
    intrinsics: HashMap<usize, inline::IntrinsicWidths>,
    scratch: math::Scratch,
    progress: Progress,
    fallbacks: Fallbacks,
    failed: bool,
    scale: f64,
}
impl<'a> Job<'a> {
    pub fn new(engine: &'a mut shaping::Engine, scope: &'a Scope, cache: Arc<Cache>, viewport: Viewport, limits: Limits, cancel: Option<&'a AtomicBool>) -> Result<Self> {
        check(cancel)?;
        if ![viewport.width, viewport.height].into_iter().all(|v| v.is_finite() && v >= 0.)
            || !viewport.scale.is_finite()
            || viewport.scale <= 0.
            || !(viewport.scale * 64.).is_finite()
            || viewport.scale * 64. == 0.
            || ![viewport.width, viewport.height].into_iter().all(|v| (v * viewport.scale * 64.).is_finite())
        {
            return Err(Error::Invalid);
        }
        if cache.tree.styles.iter().next().is_some_and(|(_, s)| s.device_scale() != viewport.scale) {
            return Err(Error::Stale);
        }
        if cache.tree.boxes.len() > limits.boxes || cache.tree.text_bytes() > limits.text_bytes || limits.depth == 0 {
            return Err(Error::Limit);
        }
        let used = Used {
            width: used::grid(viewport.width, viewport.scale),
            height: Some(used::grid(viewport.height, viewport.scale)),
            min_height: 0.,
            max_height: f64::MAX,
            margin: Edges::default(),
            padding: Edges::default(),
            border: Edges::default(),
            relative: Point::default(),
            bfc: true,
            rtl: false,
        };
        let root = cache.tree.root();
        let next = cache.tree.boxes[0].first_child;
        let geometry = vec![Geometry::default(); cache.tree.boxes.len()];
        let texts = (0..cache.texts.len()).map(|_| None).collect();
        Ok(Self {
            engine,
            scope,
            cache,
            limits,
            cancel,
            profiling: std::env::var_os("OLO_WEB_TIMING").is_some(),
            geometry,
            texts,
            stack: vec![Frame {
                id: root,
                next,
                used,
                cursor: 0.,
                pending: Strut::default(),
                top: Strut::default(),
                at_start: true,
                collapse_top: false,
                collapse_bottom: false,
                through: false,
                queries: QueryBasis::viewport(viewport),
                atomic_host: false,
                bfc: true,
                bfc_origin: Point::default(),
                floats: Vec::new(),
                float_left: None,
            }],
            flex: Vec::new(),
            containers: Vec::new(),
            tables: Vec::new(),
            absolute: Default::default(),
            absolute_at: 0,
            absolute_indices: HashMap::new(),
            active_absolute: None,
            intrinsics: HashMap::new(),
            scratch: math::Scratch::default(),
            progress: Progress::default(),
            fallbacks: Fallbacks::default(),
            failed: false,
            scale: viewport.scale,
        })
    }
    pub fn progress(&self) -> Progress {
        self.progress
    }
    /// What this layout substituted (column flex as blocks, baseline alignment as start, …).
    pub fn fallbacks(&self) -> &Fallbacks {
        &self.fallbacks
    }
    pub(super) fn note(&mut self, what: &'static str, id: BoxId) {
        let node = self.cache.tree.get(id).map(|b| b.node);
        self.fallbacks.note(what, node);
    }
    fn enter(&mut self, id: BoxId) -> Result<()> {
        if self.stack.len() >= self.limits.depth {
            return Err(Error::Limit);
        }
        let frame = self.stack.last().ok_or(Error::Invalid)?;
        let parent = frame.used;
        let queries = frame.queries;
        let (parent_origin, parent_cursor, parent_pending, parent_collapsing) = (frame.bfc_origin, frame.cursor, frame.pending.value(), frame.at_start && frame.collapse_top);
        let parent_id = frame.id;
        let cache = self.cache.clone();
        let b = cache.tree.get(id).ok_or(Error::Invalid)?;
        let BoxKind::Block { flow_root, root, .. } = b.kind else { return Err(Error::Invalid) };
        let style = cache.tree.style(id).ok_or(Error::Invalid)?;
        if style.font().context() != self.scope.context() {
            return Err(Error::Stale);
        }
        let mut float_left = None;
        let mut used = if let Some(a) = self.active_absolute.as_ref().filter(|a| a.id == id) {
            a.used
        } else if let Some(used) = self.flex_child_used(id)? {
            used
        } else if let Some(used) = self.container_child_used(id)? {
            used
        } else if let Some(used) = self.atomic_used(id, style, parent, queries)? {
            used
        } else {
            let mut used = Used::new(style, parent.width, parent.height.map(|h| parent.height(h)), parent.rtl, queries, &mut self.scratch, self.cancel, self.scale)?;
            float_left = self.float_side(style);
            // Legacy centring (<center>, align=center): a sized block in a
            // -webkit-center container is centred, as by auto margins.
            if float_left.is_none()
                && !container_flow::is_table(style)
                && !matches!(style.value(P::Width), Value::Keyword(K::Auto))
                && matches!(style.value(P::MarginLeft), Value::Keyword(K::Auto)) == matches!(style.value(P::MarginRight), Value::Keyword(K::Auto))
                && let Some(p) = cache.tree.style(parent_id)
                && let Value::Keyword(align @ (K::WebkitCenter | K::WebkitRight)) = p.value(P::TextAlign)
            {
                let outer = used.width + used.padding.horizontal() + used.border.horizontal();
                let specified_left = used::length(style, P::MarginLeft, Some(parent.width), Some(0.), queries, &mut self.scratch, self.cancel, self.scale)?.unwrap_or(0.);
                let specified_right = used::length(style, P::MarginRight, Some(parent.width), Some(0.), queries, &mut self.scratch, self.cancel, self.scale)?.unwrap_or(0.);
                let free = parent.width - outer - specified_left - specified_right;
                if free > 0. {
                    used.margin.left = specified_left + used::grid(if *align == K::WebkitRight { free } else { free / 2. }, self.scale);
                    used.margin.right = specified_right;
                }
            }
            if float_left.is_some() {
                // A float is a formatting context of its own, shrink-to-fit
                // wide unless sized, its auto margins zero (CSS 2.2 §10.3.5).
                used.bfc = true;
                let extra = used.padding.horizontal() + used.border.horizontal();
                // The block width equation does not apply: the margins are
                // the specified ones, auto being zero.
                used.margin.left = used::length(style, P::MarginLeft, Some(parent.width), Some(0.), queries, &mut self.scratch, self.cancel, self.scale)?.unwrap_or(0.);
                used.margin.right = used::length(style, P::MarginRight, Some(parent.width), Some(0.), queries, &mut self.scratch, self.cancel, self.scale)?.unwrap_or(0.);
                if matches!(style.value(P::Width), Value::Keyword(K::Auto)) {
                    let (widths, _) = self.intrinsic_content_of(id, queries, parent.width, 0, false)?;
                    let available = (parent.width - extra - used.margin.horizontal()).max(0.);
                    used.width = widths.max_content.min(available).max(widths.min_content.min(available)).max(0.);
                }
            }
            used
        };
        used.bfc |= flow_root || root;
        let prospective_y = if float_left.is_some() {
            parent_cursor + if parent_collapsing { 0. } else { parent_pending }
        } else if parent_collapsing {
            0.
        } else {
            parent_cursor + parent_pending.max(used.margin.top)
        };
        let bfc_origin = if used.bfc {
            Point::default()
        } else {
            Point { x: parent_origin.x + used.margin.left + used.relative.x + used.border.left + used.padding.left, y: parent_origin.y + prospective_y + used.border.top + used.padding.top }
        };
        if container_flow::is_table(style) {
            self.begin_table(id, style, &mut used, parent.width, queries)?;
        }
        let collapse_top = !used.bfc && used.padding.top + used.border.top == 0.;
        let collapse_bottom = !used.bfc && used.height.is_none() && used.min_height == 0. && used.padding.bottom + used.border.bottom == 0.;
        let through = !used.bfc && used.padding.vertical() + used.border.vertical() == 0. && used.min_height == 0. && used.height.is_none_or(|h| h == 0.);
        self.geometry[id.index()] = Geometry {
            border_box: Rect { x: used.margin.left + used.relative.x, y: 0., width: used.width + used.padding.horizontal() + used.border.horizontal(), height: 0. },
            padding: used.padding,
            border_width: used.border,
            margin: used.margin,
        };
        if is_row_flex(style) {
            let frame = flex_flow::Frame::new(id, b.first_child, style, used, queries, &mut self.scratch, &mut self.fallbacks, self.cancel, self.scale)?;
            self.flex.push(frame);
        } else if container_flow::is_grid(style) || container_flow::is_column_flex(style) || container_flow::is_table_row(style) {
            self.begin_container(id, style, used, queries)?;
        }
        let queries = queries.descendants(style, used.width, used.height(0.))?;
        self.stack.push(Frame {
            id,
            next: b.first_child,
            used,
            cursor: 0.,
            pending: Strut::default(),
            top: Strut::new(used.margin.top),
            at_start: true,
            collapse_top,
            collapse_bottom,
            through,
            queries,
            atomic_host: false,
            bfc: used.bfc,
            bfc_origin,
            floats: Vec::new(),
            float_left,
        });
        Ok(())
    }
    /// `float: left | right` on a box in normal flow (not a flex or grid
    /// item, not absolutely positioned).
    /// A laid-out box's last in-flow line box's baseline, from its border
    /// box's top; None when it has no in-flow line box.
    fn last_baseline(&self, id: BoxId, depth: usize) -> Result<Option<f64>> {
        check(self.cancel)?;
        if depth > self.limits.depth {
            return Err(Error::Limit);
        }
        let tree = &self.cache.tree;
        let b = tree.get(id).ok_or(Error::Invalid)?;
        let g = self.geometry.get(id.index()).copied().unwrap_or_default();
        if let BoxKind::InlineContent { source } = b.kind {
            let Some(l) = self.texts.get(source as usize).and_then(Option::as_ref) else { return Ok(None) };
            let lines = l.lines().len();
            return Ok(if lines == 0 { None } else { l.baseline(lines - 1) });
        }
        if !matches!(b.kind, BoxKind::Block { .. }) {
            return Ok(None);
        }
        let mut children = Vec::new();
        let mut next = b.first_child;
        while let Some(c) = next {
            let child = tree.get(c).ok_or(Error::Invalid)?;
            next = child.next_sibling;
            if child.absolute || child.marker {
                continue;
            }
            if let Some(cs) = tree.style(c)
                && matches!(cs.value(P::Float), Value::Keyword(K::Left | K::Right | K::InlineStart | K::InlineEnd))
            {
                continue;
            }
            children.push(c);
        }
        for c in children.into_iter().rev() {
            if let Some(baseline) = self.last_baseline(c, depth + 1)? {
                let cg = self.geometry.get(c.index()).copied().unwrap_or_default();
                return Ok(Some(g.border_width.top + g.padding.top + cg.border_box.y + baseline));
            }
        }
        Ok(None)
    }
    fn float_side(&self, style: &crate::css::style::Style) -> Option<bool> {
        let parent = self.stack.last()?;
        if self.flex.last().is_some_and(|f| f.id == parent.id) || self.containers.last().is_some_and(|c| c.id == parent.id) || parent.atomic_host {
            return None;
        }
        match style.value(P::Float) {
            Value::Keyword(K::Left | K::InlineStart) => Some(true),
            Value::Keyword(K::Right | K::InlineEnd) => Some(false),
            _ => None,
        }
    }
    /// The frame holding the current formatting context's floats.
    fn bfc_index(&self) -> usize {
        self.stack.iter().rposition(|f| f.bfc).unwrap_or(0)
    }
    /// The prospective y of the next in-flow thing in the current frame, in
    /// the formatting context's coordinates.
    fn flow_y(&self) -> f64 {
        let f = self.stack.last().expect("a frame");
        f.bfc_origin.y + f.cursor + if f.at_start && f.collapse_top { 0. } else { f.pending.value() }
    }
    /// Place a float whose margin box is `width` × `height` (CSS 2.2 §9.5.1):
    /// at the current y or lower, against its containing block's edge or the
    /// floats already there, never above an earlier float; record it.
    fn place_float(&mut self, id: BoxId, left: bool, margin: Edges, width: f64, height: f64) -> Result<()> {
        let bfc = self.bfc_index();
        let f = self.stack.last().ok_or(Error::Invalid)?;
        let (origin, container_width) = (f.bfc_origin, f.used.width);
        let mut y = self.flow_y();
        for fb in &self.stack[bfc].floats {
            y = y.max(fb.rect.y);
        }
        let (bound_left, bound_right) = (origin.x, origin.x + container_width);
        let mut candidates: Vec<f64> = self.stack[bfc].floats.iter().map(|fb| fb.rect.y + fb.rect.height).filter(|b| *b > y).collect();
        candidates.sort_by(|a, b| a.total_cmp(b));
        candidates.insert(0, y);
        let mut placed = None;
        for y in candidates {
            let (mut l, mut r) = (bound_left, bound_right);
            for fb in &self.stack[bfc].floats {
                if fb.rect.y < y + height.max(1.) && fb.rect.y + fb.rect.height > y {
                    if fb.left {
                        l = l.max(fb.rect.x + fb.rect.width);
                    } else {
                        r = r.min(fb.rect.x);
                    }
                }
            }
            if r - l >= width || (l == bound_left && r == bound_right) {
                placed = Some((if left { l } else { r - width }, y));
                break;
            }
        }
        let (x, y) = placed.ok_or(Error::Invalid)?;
        if ![x, y, width, height].iter().all(|v| v.is_finite()) {
            return Err(Error::Invalid);
        }
        let g = &mut self.geometry[id.index()];
        g.border_box.x = used::grid(x - origin.x + margin.left, self.scale);
        g.border_box.y = used::grid(y - origin.y + margin.top, self.scale);
        if self.stack[bfc].floats.len() >= self.limits.boxes {
            return Err(Error::Limit);
        }
        self.stack[bfc].floats.push(FloatBox { left, rect: Rect { x, y, width, height } });
        Ok(())
    }
    /// The floats beside a paragraph starting now in the current frame, in
    /// the paragraph's coordinates.
    fn exclusions(&self) -> Vec<inline::Exclusion> {
        let bfc = self.bfc_index();
        let f = self.stack.last().expect("a frame");
        let y0 = self.flow_y();
        self.stack[bfc]
            .floats
            .iter()
            .filter(|fb| fb.rect.y + fb.rect.height > y0)
            .map(|fb| inline::Exclusion {
                top: fb.rect.y - y0,
                bottom: fb.rect.y + fb.rect.height - y0,
                left: fb.left.then_some(fb.rect.x + fb.rect.width - f.bfc_origin.x),
                right: (!fb.left).then_some(fb.rect.x - f.bfc_origin.x),
            })
            .collect()
    }
    /// `clear`: move the frame's cursor below the floats of that side
    /// (CSS 2.2 §9.5.2); clearance ends margin collapsing above the box.
    fn clear(&mut self, style: &crate::css::style::Style) -> Result<()> {
        let (left, right) = match style.value(P::Clear) {
            Value::Keyword(K::Left | K::InlineStart) => (true, false),
            Value::Keyword(K::Right | K::InlineEnd) => (false, true),
            Value::Keyword(K::Both) => (true, true),
            _ => return Ok(()),
        };
        let bfc = self.bfc_index();
        let bottom = self.stack[bfc].floats.iter().filter(|fb| if fb.left { left } else { right }).map(|fb| fb.rect.y + fb.rect.height).fold(f64::NEG_INFINITY, f64::max);
        if bottom == f64::NEG_INFINITY {
            return Ok(());
        }
        let f = self.stack.last_mut().ok_or(Error::Invalid)?;
        let current = f.bfc_origin.y + f.cursor + if f.at_start && f.collapse_top { 0. } else { f.pending.value() };
        if bottom > current {
            f.cursor = bottom - f.bfc_origin.y;
            f.pending = Strut::default();
            f.at_start = false;
        }
        Ok(())
    }
    fn place(&mut self, id: BoxId, result: Completed) -> Result<()> {
        if self.place_flex(id, result.relative_y)? || self.place_container(id)? {
            return Ok(());
        }
        let f = self.stack.last_mut().ok_or(Error::Invalid)?;
        if f.atomic_host {
            // The line layout places atomic inlines from their object fragments.
            return Ok(());
        }
        let y = if f.at_start && f.collapse_top {
            f.top = f.top.union(result.before);
            if result.through {
                f.top = f.top.union(result.after)
            }
            0.
        } else {
            f.cursor + f.pending.union(result.before).value()
        };
        let g = &mut self.geometry[id.index()];
        g.border_box.y = y + result.relative_y;
        if result.through {
            if !(f.at_start && f.collapse_top) {
                f.pending = f.pending.union(result.before).union(result.after)
            }
        } else {
            f.cursor = y + g.border_box.height;
            f.pending = result.after;
            f.at_start = false;
        }
        if ![g.border_box.x, g.border_box.y, g.border_box.width, g.border_box.height, f.cursor].into_iter().all(f64::is_finite) {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    fn text(&mut self, id: BoxId, source: u32) -> Result<Completed> {
        let cache = self.cache.clone();
        let parent = self.stack.last().ok_or(Error::Invalid)?;
        let parent = ParentFrame { id: parent.id, used: parent.used, queries: parent.queries };
        let style = cache.tree.style(parent.id).ok_or(Error::Invalid)?;
        let available = match self.flex_child_used(id)? {
            Some(u) => u.width,
            None => self.container_child_used(id)?.map_or(parent.used.width, |u| u.width),
        };
        let (text, spacing) = self.prepare_inline(source, style, parent.used.width, parent.queries)?;
        let (objects, image_geometry) = if text.object_count() != 0 {
            self.inline_objects(id, source, &text, parent.used.width, parent.used.height.map(|h| parent.used.height(h)), parent.queries, false)?
        } else {
            (Vec::new(), Vec::new())
        };
        let exclusions = self.exclusions();
        let lay_out = |job: &mut Self, break_width: Option<f64>| -> Result<inline::Layout> {
            let mut j = inline::Job::for_style_with_spacing_at_scale(job.engine, text.clone(), style, job.scope, available, job.scale, job.limits.text, job.cancel, spacing.clone())?;
            if !exclusions.is_empty() {
                j = j.with_exclusions(exclusions.clone())?;
            }
            if let Some(w) = break_width {
                j = j.with_break_width(w)?;
            }
            if !objects.is_empty() {
                j = j.with_objects(objects.clone())?;
            }
            while !j.step(64)?.finished {}
            let work = j.progress().work;
            let l = j.finish()?;
            job.progress.work = job.progress.work.checked_add(work).ok_or(Error::Limit)?;
            if job.progress.work > job.limits.work {
                return Err(Error::Limit);
            }
            Ok(l)
        };
        let mut l = lay_out(self, None)?;
        // text-wrap: balance (CSS Text 4 §5.1.1): the narrowest break width
        // that keeps the line count, so the lines come out about as long as
        // each other. A bisection between the longest a line can be (the
        // text's width over the count) and the available width.
        if l.lines().len() > 1 && matches!(style.value(P::TextWrapStyle), Value::Keyword(K::Balance)) {
            let count = l.lines().len();
            let total: f64 = l.lines().iter().map(|line| line.advance).sum();
            let (mut low, mut high) = ((total / count as f64).max(0.), available);
            for _ in 0..12 {
                if high - low < 0.5 {
                    break;
                }
                let mid = (low + high) / 2.;
                let candidate = lay_out(self, Some(mid))?;
                if candidate.lines().len() <= count {
                    high = mid;
                    l = candidate;
                } else {
                    low = mid;
                }
            }
        }
        self.progress.lines += l.lines().len();
        self.progress.texts += 1;
        let through = l.lines().is_empty();
        self.geometry[id.index()] = Geometry { border_box: Rect { x: 0., y: 0., width: available, height: l.height() }, ..Default::default() };
        for placed in l.object_fragments() {
            let index = image_geometry.binary_search_by_key(&placed.at, |(_, _, at)| *at).map_err(|_| Error::Invalid)?;
            let (child, mut g, _) = image_geometry[index];
            g.border_box.x = placed.x + g.margin.left;
            g.border_box.y = l.baseline(placed.line as usize).ok_or(Error::Invalid)? - placed.ascent + g.margin.top;
            self.geometry[child.index()] = g;
            self.progress.boxes += 1;
        }
        self.texts[source as usize] = Some(l);
        Ok(Completed { before: Strut::default(), after: Strut::default(), through, relative_y: 0. })
    }
    /// The atomic inlines of one text box in source order: their sizes for line
    /// layout and their geometry, resolved against the containing block.
    #[allow(clippy::too_many_arguments)] // Explicit containing-block geometry and worker resources.
    pub(super) fn inline_objects(&mut self, id: BoxId, source: u32, text: &Arc<inline::Text>, width: f64, height: Option<f64>, queries: QueryBasis, measuring: bool) -> Result<InlineObjects> {
        let cache = self.cache.clone();
        let container = cache.tree.get(id).ok_or(Error::Invalid)?.parent.ok_or(Error::Invalid)?;
        let style = cache.tree.style(container).ok_or(Error::Invalid)?;
        let prepared = text.prepared();
        let mut image_geometry = Vec::new();
        let mut objects = Vec::new();
        let mut next = cache.tree.get(id).ok_or(Error::Invalid)?.first_child;
        let mut image_controls = cache.tree.controls(source).iter().filter(|c| c.kind == prepare::ControlKind::Object);
        while let Some(child) = next {
            let b = cache.tree.get(child).ok_or(Error::Invalid)?;
            next = b.next_sibling;
            check(self.cancel)?;
            let s = cache.tree.style(child).ok_or(Error::Stale)?;
            let text_source = cache.tree.text_source(source).ok_or(Error::Invalid)?;
            let raw = image_controls.next().ok_or(Error::Invalid)?.offset;
            if text_source.span_at(raw).ok_or(Error::Invalid)?.node != b.node {
                return Err(Error::Invalid);
            }
            let at = boundary(prepared, raw)?;
            let middle = matches!(s.value(P::VerticalAlign), Value::Keyword(K::Middle));
            let (g, margin) = if matches!(b.kind, BoxKind::Block { .. }) {
                // An atomic inline: laid out already, or measured when this
                // paragraph's intrinsic width is being measured.
                if measuring {
                    let w = self.intrinsic(child, queries, width, 0)?;
                    let mut m = |p| used::length(s, p, Some(width), Some(0.), queries, &mut self.scratch, self.cancel, self.scale).map(|v| v.unwrap_or(0.));
                    let margin = Edges { left: m(P::MarginLeft)?, right: m(P::MarginRight)?, top: 0., bottom: 0. };
                    (Geometry { border_box: Rect { x: 0., y: 0., width: w.max_content, height: 0. }, margin, ..Default::default() }, margin)
                } else {
                    let g = self.geometry[child.index()];
                    (g, g.margin)
                }
            } else {
                let natural = cache.tree.natural(child)?;
                let used = super::images::used(s, width, height, natural, false, false, queries, &mut self.scratch, self.cancel, self.scale)?;
                (super::images::geometry(used), used.margin)
            };
            let height = g.border_box.height + margin.vertical();
            // An inline-block's baseline is its last line box's (CSS 2.1
            // §10.8.1) when it has one and shows its overflow; otherwise,
            // and for other atomic inlines, the bottom margin edge.
            let inline_block = !measuring
                && matches!(b.kind, BoxKind::Block { .. })
                && matches!(s.value(P::Display), Value::Display(crate::css::properties::Display::Box { inner: crate::css::properties::Inner::Flow | crate::css::properties::Inner::FlowRoot, .. }))
                && matches!(s.value(P::OverflowX), Value::Keyword(K::Visible))
                && matches!(s.value(P::OverflowY), Value::Keyword(K::Visible));
            let ascent = if middle && matches!(b.kind, BoxKind::InlineVector { .. } | BoxKind::Block { .. }) {
                (height + style.font().metrics.ex) / 2.
            } else if inline_block && let Some(baseline) = self.last_baseline(child, 0)? {
                baseline + margin.top
            } else {
                height
            };
            objects.push(inline::Object { at, advance: g.border_box.width + margin.horizontal(), ascent, descent: height - ascent });
            image_geometry.push((child, g, at));
        }
        Ok((objects, image_geometry))
    }
    /// The paragraph's text shaped at this width's inline edges, and the edge
    /// spacing to lay it out with. Percentage edges can change the shaping
    /// boundaries, so the width-independent text is reshaped when they do.
    pub(super) fn prepare_inline(&mut self, source: u32, style: &crate::css::style::Style, width: f64, queries: QueryBasis) -> Result<(Arc<inline::Text>, Vec<inline::Spacing>)> {
        let cache = self.cache.clone();
        let mut text = cache.texts[source as usize].clone();
        let mut spacing = Vec::new();
        let mut breaks = Vec::new();
        for b in cache.tree.inline_boxes(source).ok_or(Error::Invalid)? {
            check(self.cancel)?;
            let s = cache.tree.styles.snapshot(b.node).ok_or(Error::Stale)?;
            if !used::has_inline_edge(s, true) && !used::has_inline_edge(s, false) {
                continue;
            }
            let ((left, left_break), (right, right_break)) = used::inline_edges(s, width, queries, &mut self.scratch, self.cancel, self.scale)?;
            if !left_break && !right_break {
                continue;
            }
            let start = boundary(text.prepared(), b.range.start)?;
            let end = boundary(text.prepared(), b.range.end)?;
            if b.first && left_break {
                breaks.push(start);
            }
            if b.last && right_break {
                breaks.push(end);
            }
            if start == end {
                let advance = if b.first { left } else { 0. } + if b.last { right } else { 0. };
                spacing.push(inline::Spacing { at: start, advance, side: if start == 0 { inline::Side::Start } else { inline::Side::End } });
            } else {
                if b.first && left_break {
                    spacing.push(inline::Spacing { at: start, advance: left, side: inline::Side::Start });
                }
                if b.last && right_break {
                    spacing.push(inline::Spacing { at: end, advance: right, side: inline::Side::End });
                }
            }
        }
        for c in text.prepared().controls().iter().filter(|c| c.kind() == prepare::ControlKind::Object) {
            let at = boundary(text.prepared(), c.offset())?;
            breaks.extend([at, at + 3]);
        }
        breaks.retain(|b| *b > 0 && (*b as usize) < text.prepared().text().len());
        breaks.sort_unstable();
        breaks.dedup();
        if text.paragraph().shaping_breaks() != breaks.as_slice() {
            let styles = text.paragraph().styles.to_vec();
            text = shape_text(self.engine, self.scope, style, text.prepared().clone(), breaks.into_boxed_slice(), styles, self.limits.text, self.cancel)?;
        }
        Ok((text, spacing))
    }
    /// The width of an atomic inline about to lay out under its text box's
    /// host frame: fit-content within the paragraph's available width.
    fn atomic_used(&mut self, id: BoxId, style: &crate::css::style::Style, parent: Used, queries: QueryBasis) -> Result<Option<Used>> {
        if !self.stack.last().is_some_and(|f| f.atomic_host) {
            return Ok(None);
        }
        let mut used = Used::flex(style, parent.width, parent.height.map(|h| parent.height(h)), queries, &mut self.scratch, self.cancel, self.scale)?;
        let specified = used::length(style, P::Width, Some(parent.width), None, queries, &mut self.scratch, self.cancel, self.scale)?;
        let extra = used.padding.horizontal() + used.border.horizontal();
        used.width = match specified {
            Some(w) => {
                let border_box = matches!(style.value(P::BoxSizing), Value::Keyword(K::BorderBox));
                (w - if border_box { extra } else { 0. }).max(0.)
            }
            None => {
                let (w, _) = self.intrinsic_content(id, queries, parent.width, 0)?;
                let available = (parent.width - extra - used.margin.horizontal()).max(0.);
                w.max_content.min(available).max(w.min_content.min(available)).max(0.)
            }
        };
        used.width = used::grid(used.width, self.scale);
        Ok(Some(used))
    }
    pub fn step(&mut self, budget: usize) -> Result<Progress> {
        if self.failed {
            return Err(Error::Failed);
        }
        let r = self.advance(budget);
        if r.is_err() {
            self.failed = true
        }
        r
    }
    fn advance(&mut self, budget: usize) -> Result<Progress> {
        check(self.cancel)?;
        for _ in 0..budget {
            check(self.cancel)?;
            if self.progress.finished {
                break;
            }
            self.progress.work += 1;
            if self.progress.work > self.limits.work {
                return Err(Error::Limit);
            }
            if self.stack.is_empty() {
                if self.absolute_at == self.absolute.len() {
                    self.progress.finished = true;
                    break;
                }
                let t = self.profiling.then(std::time::Instant::now);
                self.begin_absolute()?;
                profile(5, t);
                continue;
            }
            if self.stack.last().is_some_and(|f| f.atomic_host) {
                // Lay out the text box's atomic inlines, then its lines.
                let f = self.stack.last_mut().ok_or(Error::Invalid)?;
                let mut child = f.next;
                while let Some(c) = child {
                    let b = &self.cache.tree.boxes[c.index()];
                    if matches!(b.kind, BoxKind::Block { .. }) {
                        break;
                    }
                    child = b.next_sibling;
                }
                match child {
                    Some(c) => {
                        f.next = self.cache.tree.boxes[c.index()].next_sibling;
                        self.enter(c)?;
                    }
                    None => {
                        let host = self.stack.pop().ok_or(Error::Invalid)?;
                        let BoxKind::InlineContent { source } = self.cache.tree.boxes[host.id.index()].kind else { return Err(Error::Invalid) };
                        let t = self.profiling.then(std::time::Instant::now);
                        let r = self.text(host.id, source)?;
                        self.place(host.id, r)?;
                        profile(1, t);
                        self.progress.boxes += 1;
                    }
                }
                continue;
            }
            let child = if self.flex.last().is_some_and(|f| self.stack.last().is_some_and(|parent| f.id == parent.id)) {
                let t = self.profiling.then(std::time::Instant::now);
                let tick = self.flex_tick()?;
                profile(4, t);
                match tick {
                    flex_flow::Tick::Pending => continue,
                    flex_flow::Tick::Child(id) => Some(id),
                    flex_flow::Tick::Done => None,
                }
            } else {
                let f = self.stack.last_mut().ok_or(Error::Invalid)?;
                let child = f.next;
                if let Some(child) = child {
                    f.next = self.cache.tree.boxes[child.index()].next_sibling;
                }
                child
            };
            if let Some(child) = child {
                if self.positioned(child)? {
                    self.defer_absolute(child)?;
                    continue;
                }
                let cache = self.cache.clone();
                let b = &cache.tree.boxes[child.index()];
                if matches!(b.kind, BoxKind::Block { .. } | BoxKind::BlockImage { .. } | BoxKind::BlockVector { .. } | BoxKind::BlockReplaced { .. })
                    && let Some(s) = cache.tree.style(child)
                {
                    self.clear(s)?;
                }
                match b.kind {
                    BoxKind::Block { .. } => {
                        let t = self.profiling.then(std::time::Instant::now);
                        self.enter(child)?;
                        profile(0, t);
                    }
                    BoxKind::InlineContent { source } => {
                        let mut atomic = b.first_child;
                        while let Some(c) = atomic {
                            let cb = &self.cache.tree.boxes[c.index()];
                            if matches!(cb.kind, BoxKind::Block { .. }) {
                                break;
                            }
                            atomic = cb.next_sibling;
                        }
                        if atomic.is_some() {
                            if self.stack.len() >= self.limits.depth {
                                return Err(Error::Limit);
                            }
                            let parent = self.stack.last().ok_or(Error::Invalid)?;
                            let (used, queries) = (parent.used, parent.queries);
                            self.stack.push(Frame {
                                id: child,
                                next: b.first_child,
                                used,
                                cursor: 0.,
                                pending: Strut::default(),
                                top: Strut::default(),
                                at_start: true,
                                collapse_top: false,
                                collapse_bottom: false,
                                through: false,
                                queries,
                                atomic_host: true,
                                bfc: false,
                                bfc_origin: self.stack.last().map_or(Point::default(), |p| p.bfc_origin),
                                floats: Vec::new(),
                                float_left: None,
                            });
                        } else {
                            let t = self.profiling.then(std::time::Instant::now);
                            let r = self.text(child, source)?;
                            self.place(child, r)?;
                            profile(1, t);
                            self.progress.boxes += 1
                        }
                    }
                    BoxKind::BlockImage { .. } | BoxKind::BlockVector { .. } | BoxKind::BlockReplaced { .. } => {
                        let t = self.profiling.then(std::time::Instant::now);
                        let frame = self.stack.last().ok_or(Error::Invalid)?;
                        let parent = frame.used;
                        let queries = frame.queries;
                        let s = self.cache.tree.style(child).ok_or(Error::Invalid)?;
                        let natural = self.cache.tree.natural(child)?;
                        let mut used = super::images::used(s, parent.width, parent.height.map(|h| parent.height(h)), natural, true, parent.rtl, queries, &mut self.scratch, self.cancel, self.scale)?;
                        let flexed = match self.flex_child_used(child)? {
                            Some(u) => Some(u),
                            None => self.container_child_used(child)?,
                        };
                        if let Some(flexed) = flexed {
                            // The flex line decided the main size; an auto height keeps the ratio.
                            let ratio = if natural.0 > 0. && natural.1 > 0. { natural.0 / natural.1 } else { 0. };
                            if matches!(s.value(P::Height), Value::Keyword(K::Auto)) && ratio > 0. && flexed.width != used.width {
                                used.height = Some(used::grid(flexed.width / ratio, self.scale));
                            }
                            used.width = flexed.width;
                            used.margin.left = flexed.margin.left;
                            used.margin.right = flexed.margin.right;
                        }
                        self.geometry[child.index()] = super::images::geometry(used);
                        if let Some(left) = self.float_side(s).filter(|_| flexed.is_none()) {
                            // A floated image: its specified margins, auto being zero.
                            let mut margin = used.margin;
                            margin.left = used::length(s, P::MarginLeft, Some(parent.width), Some(0.), queries, &mut self.scratch, self.cancel, self.scale)?.unwrap_or(0.);
                            margin.right = used::length(s, P::MarginRight, Some(parent.width), Some(0.), queries, &mut self.scratch, self.cancel, self.scale)?.unwrap_or(0.);
                            self.geometry[child.index()].margin = margin;
                            let g = self.geometry[child.index()];
                            self.place_float(child, left, margin, g.border_box.width + margin.horizontal(), g.border_box.height + margin.vertical())?;
                        } else {
                            self.place(child, Completed { before: Strut::new(used.margin.top), after: Strut::new(used.margin.bottom), through: false, relative_y: 0. })?;
                        }
                        profile(2, t);
                        self.progress.boxes += 1;
                    }
                    BoxKind::InlineImage { .. } | BoxKind::InlineVector { .. } | BoxKind::InlineReplaced { .. } | BoxKind::Viewport => return Err(Error::Invalid),
                }
            } else {
                let t = self.profiling.then(std::time::Instant::now);
                let mut f = self.stack.pop().ok_or(Error::Invalid)?;
                if let Some(height) = self.finish_container(f.id)? {
                    f.cursor = height;
                    f.pending = Strut::default();
                }
                self.finish_table(f.id);
                // A formatting context root contains its floats (CSS 2.2 §10.6.7).
                if f.bfc && !f.floats.is_empty() {
                    let bottom = f.floats.iter().map(|fb| fb.rect.y + fb.rect.height).fold(0f64, f64::max);
                    let flow = f.cursor + if f.collapse_bottom { 0. } else { f.pending.value() };
                    if bottom > flow {
                        f.cursor = bottom;
                        f.pending = Strut::default();
                    }
                }
                let auto = f.cursor + if f.collapse_bottom { 0. } else { f.pending.value() };
                let height = f.used.height(auto.max(0.));
                let g = &mut self.geometry[f.id.index()];
                if f.id == self.cache.tree.root() {
                    g.border_box = Rect { x: 0., y: 0., width: f.used.width, height }
                } else {
                    g.border_box.height = height + f.used.padding.vertical() + f.used.border.vertical()
                }
                let through = f.through && f.at_start;
                let after = Strut::new(f.used.margin.bottom).union(if f.collapse_bottom && !through { f.pending } else { Strut::default() });
                let result = Completed { before: f.top, after, through, relative_y: f.used.relative.y };
                self.progress.boxes += 1;
                if self.finish_absolute(f.id)? {
                    self.progress.finished = self.absolute_at == self.absolute.len();
                } else if self.stack.is_empty() {
                    self.progress.finished = self.absolute_at == self.absolute.len()
                } else if let Some(left) = f.float_left {
                    let g = self.geometry[f.id.index()];
                    self.place_float(f.id, left, f.used.margin, g.border_box.width + f.used.margin.horizontal(), g.border_box.height + f.used.margin.vertical())?;
                } else {
                    self.place(f.id, result)?
                }
                profile(3, t);
            }
        }
        Ok(self.progress)
    }
    pub fn finish(mut self) -> Result<Layout> {
        if self.failed {
            return Err(Error::Failed);
        }
        check(self.cancel)?;
        if !self.progress.finished {
            return Err(Error::Incomplete);
        }
        if self.profiling {
            let p = LAYOUT_PROFILE.with(|p| p.replace([(0, 0.); 7]));
            let names = ["enter", "text", "replaced", "finish", "flex", "absolute", "intrinsic"];
            let parts: Vec<String> = names.iter().zip(p).filter(|(_, (n, _))| *n > 0).map(|(name, (n, secs))| format!("{name} {n}×{:.1} ms", secs * 1000.)).collect();
            eprintln!("layout profile: {}", parts.join(", "));
        }
        if !self.intrinsics.is_empty() {
            if let Some(previous) = &self.cache.intrinsics {
                for (&source, &widths) in previous.iter() {
                    check(self.cancel)?;
                    self.intrinsics.entry(source).or_insert(widths);
                }
            }
            self.cache = Arc::new(Cache {
                tree: self.cache.tree.clone(),
                texts: self.cache.texts.clone(),
                svg_texts: self.cache.svg_texts.clone(),
                shared: self.cache.shared.clone(),
                intrinsics: Some(Arc::new(self.intrinsics)),
                glyph_geometry_bytes: self.cache.glyph_geometry_bytes,
                fallbacks: self.cache.fallbacks.clone(),
            });
        }
        Ok(Layout { cache: self.cache, geometry: self.geometry.into_boxed_slice(), texts: Arc::from(self.texts), scale: self.scale })
    }
}
