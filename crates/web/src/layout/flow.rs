use super::used::Used;
use super::*;
use crate::{
    css::math,
    font::Scope,
    text::{layout as inline, linebreak, prepare, shaping},
};
use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    sync::{Arc, atomic::AtomicBool},
};
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
    out.retain(|b| *b > 0 && (*b as usize) < prepared.text().len());
    out.sort_unstable();
    out.dedup();
    Ok(out.into_boxed_slice())
}
fn shape_text(
    engine: &mut shaping::Engine,
    scope: &Scope,
    style: &crate::css::style::Style,
    prepared: Arc<prepare::Prepared>,
    boundaries: Box<[u32]>,
    limits: inline::Limits,
    cancel: Option<&AtomicBool>,
) -> Result<Arc<inline::Text>> {
    let mut s = shaping::ParagraphJob::for_layout_style(engine, scope, style, prepared.text().clone(), Default::default(), cancel)?.with_shaping_breaks(boundaries)?;
    while !s.step(128)?.finished {}
    let s = Arc::new(s.finish()?);
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
    texts: Box<[Arc<inline::Text>]>,
    glyph_geometry_bytes: usize,
}
impl Cache {
    pub fn tree(&self) -> &Arc<Tree> {
        &self.tree
    }
    pub fn text(&self, source: u32) -> Option<&Arc<inline::Text>> {
        self.texts.get(source as usize)
    }
    /// Only native glyph + text-index payload; excludes the tree, preparation
    /// maps, font/source allocations, headers, capacity and allocator overhead.
    pub fn glyph_geometry_payload_bytes(&self) -> usize {
        self.glyph_geometry_bytes
    }
}
pub struct PrepareJob<'a> {
    engine: &'a mut shaping::Engine,
    scope: &'a Scope,
    tree: Arc<Tree>,
    texts: Vec<Arc<inline::Text>>,
    shared: HashMap<(usize, Arc<str>, u64), Arc<inline::Text>>,
    glyph_geometry_bytes: usize,
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
        Ok(Self { engine, scope, tree, texts: Vec::new(), shared: HashMap::new(), glyph_geometry_bytes: 0, cancel, limits, progress: Progress::default(), failed: false })
    }
    fn one(&mut self, index: usize) -> Result<Arc<inline::Text>> {
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
        // The paragraph shaper currently owns one font request/size. Refuse to
        // flatten a differently styled inline into the surrounding typography.
        for span in &source.spans {
            check(self.cancel)?;
            let s = self.tree.styles.snapshot(span.style_node).ok_or(Error::Stale)?;
            let control = !source_controls.is_empty() && source_controls.binary_search_by_key(&span.range.start, |c| c.offset).is_ok();
            if s.font().request != style.font().request
                || s.font().context() != style.font().context()
                || s.axes() != style.axes()
                || s.line_height() != style.line_height()
                || (!control && prepare::WhiteSpace::for_style(s)? != whitespace)
            {
                return Err(Error::Unsupported("mixed inline typography/whitespace"));
            }
            inline::Options::for_style(s, self.scope, 0., self.cancel)?;
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
            (Arc::as_ptr(style.font()) as usize, source.source.clone(), hash.finish())
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
        let text = shape_text(self.engine, self.scope, style, p, boundaries, self.limits.text, self.cancel)?;
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
        Ok(Arc::new(Cache { tree: self.tree, texts: self.texts.into_boxed_slice(), glyph_geometry_bytes: self.glyph_geometry_bytes }))
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
pub struct Layout {
    cache: Arc<Cache>,
    geometry: Box<[Geometry]>,
    texts: Box<[Option<inline::Layout>]>,
    scale: f64,
}
impl Layout {
    pub fn device_scale(&self) -> f64 {
        self.scale
    }
    pub fn cache(&self) -> &Arc<Cache> {
        &self.cache
    }
    pub fn geometry(&self, id: BoxId) -> Option<&Geometry> {
        self.geometry.get(id.index())
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
}
struct Completed {
    before: Strut,
    after: Strut,
    through: bool,
    relative_y: f64,
}
pub struct Job<'a> {
    engine: &'a mut shaping::Engine,
    scope: &'a Scope,
    cache: Arc<Cache>,
    limits: Limits,
    cancel: Option<&'a AtomicBool>,
    geometry: Vec<Geometry>,
    texts: Vec<Option<inline::Layout>>,
    stack: Vec<Frame>,
    scratch: math::Scratch,
    progress: Progress,
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
            geometry,
            texts,
            stack: vec![Frame { id: root, next, used, cursor: 0., pending: Strut::default(), top: Strut::default(), at_start: true, collapse_top: false, collapse_bottom: false, through: false }],
            scratch: math::Scratch::default(),
            progress: Progress::default(),
            failed: false,
            scale: viewport.scale,
        })
    }
    pub fn progress(&self) -> Progress {
        self.progress
    }
    fn enter(&mut self, id: BoxId) -> Result<()> {
        if self.stack.len() >= self.limits.depth {
            return Err(Error::Limit);
        }
        let parent = self.stack.last().ok_or(Error::Invalid)?.used;
        let b = self.cache.tree.get(id).ok_or(Error::Invalid)?;
        let BoxKind::Block { flow_root, root } = b.kind else { return Err(Error::Invalid) };
        let style = self.cache.tree.style(id).ok_or(Error::Invalid)?;
        if style.font().context() != self.scope.context() {
            return Err(Error::Stale);
        }
        let mut used = Used::new(style, parent.width, parent.height.map(|h| parent.height(h)), parent.rtl, &mut self.scratch, self.cancel, self.scale)?;
        used.bfc |= flow_root || root;
        let collapse_top = !used.bfc && used.padding.top + used.border.top == 0.;
        let collapse_bottom = !used.bfc && used.height.is_none() && used.min_height == 0. && used.padding.bottom + used.border.bottom == 0.;
        let through = !used.bfc && used.padding.vertical() + used.border.vertical() == 0. && used.min_height == 0. && used.height.is_none_or(|h| h == 0.);
        self.geometry[id.index()] = Geometry {
            border_box: Rect { x: used.margin.left + used.relative.x, y: 0., width: used.width + used.padding.horizontal() + used.border.horizontal(), height: 0. },
            padding: used.padding,
            border_width: used.border,
            margin: used.margin,
        };
        self.stack.push(Frame { id, next: b.first_child, used, cursor: 0., pending: Strut::default(), top: Strut::new(used.margin.top), at_start: true, collapse_top, collapse_bottom, through });
        Ok(())
    }
    fn place(&mut self, id: BoxId, result: Completed) -> Result<()> {
        let f = self.stack.last_mut().ok_or(Error::Invalid)?;
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
        let parent = self.stack.last().ok_or(Error::Invalid)?;
        let style = self.cache.tree.style(parent.id).ok_or(Error::Invalid)?;
        let mut text = self.cache.texts[source as usize].clone();
        let mut spacing = Vec::new();
        let mut breaks = Vec::new();
        for b in self.cache.tree.inline_boxes(source).ok_or(Error::Invalid)? {
            check(self.cancel)?;
            let s = self.cache.tree.styles.snapshot(b.node).ok_or(Error::Stale)?;
            if !used::has_inline_edge(s, true) && !used::has_inline_edge(s, false) {
                continue;
            }
            let ((left, left_break), (right, right_break)) = used::inline_edges(s, parent.used.width, &mut self.scratch, self.cancel, self.scale)?;
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
        breaks.retain(|b| *b > 0 && (*b as usize) < text.prepared().text().len());
        breaks.sort_unstable();
        breaks.dedup();
        if text.paragraph().shaping_breaks() != breaks.as_slice() {
            // Percentage/calc edges can become zero at this width. Keep the
            // width-independent cache, but shape the actual used boundaries.
            text = shape_text(self.engine, self.scope, style, text.prepared().clone(), breaks.into_boxed_slice(), self.limits.text, self.cancel)?;
        }
        let mut j = inline::Job::for_style_with_spacing_at_scale(self.engine, text, style, self.scope, parent.used.width, self.scale, self.limits.text, self.cancel, spacing)?;
        while !j.step(64)?.finished {}
        let work = j.progress().work;
        let l = j.finish()?;
        self.progress.work = self.progress.work.checked_add(work).ok_or(Error::Limit)?;
        if self.progress.work > self.limits.work {
            return Err(Error::Limit);
        }
        self.progress.lines += l.lines().len();
        self.progress.texts += 1;
        let through = l.lines().is_empty();
        self.geometry[id.index()] = Geometry { border_box: Rect { x: 0., y: 0., width: parent.used.width, height: l.height() }, ..Default::default() };
        self.texts[source as usize] = Some(l);
        Ok(Completed { before: Strut::default(), after: Strut::default(), through, relative_y: 0. })
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
            let f = self.stack.last_mut().ok_or(Error::Invalid)?;
            if let Some(child) = f.next {
                let b = &self.cache.tree.boxes[child.index()];
                f.next = b.next_sibling;
                match b.kind {
                    BoxKind::Block { .. } => self.enter(child)?,
                    BoxKind::InlineContent { source } => {
                        let r = self.text(child, source)?;
                        self.place(child, r)?;
                        self.progress.boxes += 1
                    }
                    BoxKind::Viewport => return Err(Error::Invalid),
                }
            } else {
                let f = self.stack.pop().ok_or(Error::Invalid)?;
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
                if self.stack.is_empty() { self.progress.finished = true } else { self.place(f.id, result)? }
            }
        }
        Ok(self.progress)
    }
    pub fn finish(self) -> Result<Layout> {
        if self.failed {
            return Err(Error::Failed);
        }
        check(self.cancel)?;
        if !self.progress.finished {
            return Err(Error::Incomplete);
        }
        Ok(Layout { cache: self.cache, geometry: self.geometry.into_boxed_slice(), texts: self.texts.into_boxed_slice(), scale: self.scale })
    }
}
