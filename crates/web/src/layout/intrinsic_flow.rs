//! Min/max-content contributions of retained boxes, from measured text
//! geometry and resolved lengths; nothing is estimated. Flex items, shrink-to-
//! fit absolutes and, later, atomic inlines and tables size from these.
use super::*;
use crate::css::style::{Keyword as K, Property as P, Value};
impl Job<'_> {
    fn charge(&mut self, work: usize) -> Result<()> {
        self.progress.work = self.progress.work.checked_add(work).ok_or(Error::Limit)?;
        if self.progress.work > self.limits.work {
            return Err(Error::Limit);
        }
        Ok(())
    }
    /// Measured min/max-content widths of one text source, once per retained Text.
    pub(super) fn measure_text(&mut self, source: u32, queries: QueryBasis, basis: f64) -> Result<inline::IntrinsicWidths> {
        let text = self.cache.texts.get(source as usize).ok_or(Error::Invalid)?.clone();
        // Equal paragraphs share one Text; with inline objects their widths
        // still differ per paragraph, so those measure per source.
        let key = if text.object_count() == 0 { Arc::as_ptr(&text) as usize } else { usize::MAX - source as usize };
        if let Some(cached) = self.intrinsics.get(&key).or_else(|| self.cache.intrinsics.as_ref().and_then(|m| m.get(&key))) {
            return Ok(*cached);
        }
        // Inline edges (padding, margins on spans) are not in the measuring
        // job's model; such paragraphs are measured by laying them out.
        let edged = self
            .cache
            .tree
            .inline_boxes(source)
            .ok_or(Error::Invalid)?
            .iter()
            .any(|b| self.cache.tree.styles.snapshot(b.node).is_some_and(|s| used::has_inline_edge(s, true) || used::has_inline_edge(s, false)));
        let widths = match if edged { Err(inline::Error::Unsupported("inline edges")) } else { inline::IntrinsicJob::new(text.clone(), Some(self.scale), self.limits.text, self.cancel) } {
            Ok(mut job) => loop {
                let before = job.progress().work;
                let p = job.step(256)?;
                self.charge(p.work.saturating_sub(before))?;
                if p.finished {
                    break job.finish()?;
                }
            },
            Err(inline::Error::Unsupported(_)) => self.measure_by_layout(source, queries, basis)?,
            Err(e) => return Err(e.into()),
        };
        self.intrinsics.insert(key, widths);
        Ok(widths)
    }
    /// Text the measuring job does not cover (bidi, several shaping runs,
    /// inline objects, soft hyphens): lay it out at no width and at unbounded
    /// width. The widest line of each is the min-/max-content width, by definition.
    fn measure_by_layout(&mut self, source: u32, queries: QueryBasis, width: f64) -> Result<inline::IntrinsicWidths> {
        let cache = self.cache.clone();
        let container = cache.tree.text_source(source).ok_or(Error::Invalid)?.container;
        let style = cache.tree.style(container).ok_or(Error::Invalid)?;
        let mut id = cache.tree.get(container).ok_or(Error::Invalid)?.first_child;
        while let Some(child) = id {
            let b = cache.tree.get(child).ok_or(Error::Invalid)?;
            if matches!(b.kind, BoxKind::InlineContent { source: s } if s == source) {
                break;
            }
            id = b.next_sibling;
        }
        let id = id.ok_or(Error::Invalid)?;
        let mut widths = inline::IntrinsicWidths::default();
        let (text, spacing) = self.prepare_inline(source, style, width, queries)?;
        let text = &text;
        for (i, available) in [0., 1e7].into_iter().enumerate() {
            check(self.cancel)?;
            let objects = if text.object_count() != 0 { self.inline_objects(id, source, text, width, None, queries, true)?.0 } else { Vec::new() };
            let mut j = inline::Job::for_style_with_spacing_at_scale(self.engine, text.clone(), style, self.scope, available, self.scale, self.limits.text, self.cancel, spacing.clone())?;
            if !objects.is_empty() {
                j = j.with_objects(objects)?;
            }
            while !j.step(64)?.finished {}
            let work = j.progress().work;
            let l = j.finish()?;
            self.charge(work)?;
            let widest = l.lines().iter().map(|l| l.content_advance).fold(0f64, f64::max);
            if i == 0 {
                widths.min_content = widest;
            } else {
                widths.max_content = widest;
            }
        }
        if widths.min_content > widths.max_content {
            widths.min_content = widths.max_content;
        }
        Ok(widths)
    }
    /// The border-box min/max-content contribution of a box in a horizontal
    /// formatting context. Absolutely positioned boxes contribute nothing.
    pub(super) fn intrinsic(&mut self, id: BoxId, queries: QueryBasis, basis: f64, depth: usize) -> Result<inline::IntrinsicWidths> {
        let (content, extra) = self.intrinsic_content(id, queries, basis, depth)?;
        Ok(inline::IntrinsicWidths { min_content: content.min_content + extra, max_content: content.max_content + extra })
    }
    /// Content-box min/max-content widths and the box's horizontal padding plus
    /// border. A specified (non-percentage) width is the contribution; otherwise
    /// children decide: a row flex sums them, anything else takes the widest.
    pub(super) fn intrinsic_content(&mut self, id: BoxId, queries: QueryBasis, basis: f64, depth: usize) -> Result<(inline::IntrinsicWidths, f64)> {
        let t = (depth == 0 && self.profiling).then(std::time::Instant::now);
        let r = self.intrinsic_content_timed(id, queries, basis, depth);
        super::flow::profile(6, t);
        r
    }
    fn intrinsic_content_timed(&mut self, id: BoxId, queries: QueryBasis, basis: f64, depth: usize) -> Result<(inline::IntrinsicWidths, f64)> {
        self.intrinsic_content_of(id, queries, basis, depth, false)
    }
    /// The same, measuring the content itself even when a width is specified
    /// (a flex-basis of content, min-content or max-content asks for that).
    pub(super) fn intrinsic_content_of(&mut self, id: BoxId, queries: QueryBasis, basis: f64, depth: usize, ignore_width: bool) -> Result<(inline::IntrinsicWidths, f64)> {
        check(self.cancel)?;
        if depth > self.limits.depth {
            return Err(Error::Limit);
        }
        self.charge(1)?;
        let cache = self.cache.clone();
        let b = cache.tree.get(id).ok_or(Error::Invalid)?;
        match b.kind {
            BoxKind::InlineContent { source } => return Ok((self.measure_text(source, queries, basis)?, 0.)),
            BoxKind::Viewport => return Err(Error::Invalid),
            _ => {}
        }
        let s = cache.tree.style(id).ok_or(Error::Stale)?;
        let len = |p: P, fallback: Option<f64>, scratch: &mut math::Scratch| used::length(s, p, None, fallback, queries, scratch, self.cancel, self.scale);
        let padding = len(P::PaddingLeft, Some(0.), &mut self.scratch)?.unwrap_or(0.).max(0.) + len(P::PaddingRight, Some(0.), &mut self.scratch)?.unwrap_or(0.).max(0.);
        let border = s.used_border_width(P::BorderLeftWidth)? + s.used_border_width(P::BorderRightWidth)?;
        let extra = padding + border;
        let border_box = matches!(s.value(P::BoxSizing), Value::Keyword(K::BorderBox));
        let content = |v: f64| (v - if border_box { extra } else { 0. }).max(0.);
        let specified = if ignore_width { None } else { len(P::Width, None, &mut self.scratch)?.map(content) };
        let minimum = len(P::MinWidth, Some(0.), &mut self.scratch)?.map(content).unwrap_or(0.);
        let maximum = len(P::MaxWidth, None, &mut self.scratch)?.map(content).unwrap_or(f64::MAX).max(minimum);
        let mut widths = if let Some(w) = specified {
            inline::IntrinsicWidths { min_content: w, max_content: w }
        } else if super::container_flow::is_table(s) {
            let (min, max) = self.table_intrinsic(id, s, queries, basis)?;
            inline::IntrinsicWidths { min_content: min, max_content: max }
        } else if b.kind.is_replaced() {
            let (w, h) = cache.tree.natural(id)?;
            // A specified height with an automatic width sizes a replaced box by its ratio.
            let w = match len(P::Height, None, &mut self.scratch)? {
                Some(height) if h > 0. && w > 0. => height * w / h,
                _ => w,
            };
            inline::IntrinsicWidths { min_content: w, max_content: w }
        } else {
            let row = super::flow::is_row_flex(s) || super::container_flow::is_table_row(s);
            let wrap = row && !matches!(s.value(P::FlexWrap), Value::Keyword(K::Nowrap)) && !super::container_flow::is_table_row(s);
            let gap = if row && !matches!(s.value(P::ColumnGap), Value::Keyword(K::Normal)) { len(P::ColumnGap, Some(0.), &mut self.scratch)?.unwrap_or(0.).max(0.) } else { 0. };
            let (mut sum_min, mut sum_max, mut max_min, mut max_max, mut count) = (0f64, 0f64, 0f64, 0f64, 0usize);
            let mut next = b.first_child;
            while let Some(child) = next {
                check(self.cancel)?;
                let c = cache.tree.get(child).ok_or(Error::Invalid)?;
                next = c.next_sibling;
                if c.absolute {
                    continue;
                }
                let w = self.intrinsic(child, queries, basis, depth + 1)?;
                let margin = if matches!(c.kind, BoxKind::InlineContent { .. }) {
                    0.
                } else {
                    let cs = cache.tree.style(child).ok_or(Error::Stale)?;
                    let mut m = |p| used::length(cs, p, None, Some(0.), queries, &mut self.scratch, self.cancel, self.scale).map(|v| v.unwrap_or(0.));
                    m(P::MarginLeft)? + m(P::MarginRight)?
                };
                sum_min += w.min_content + margin;
                sum_max += w.max_content + margin;
                max_min = max_min.max(w.min_content + margin);
                max_max = max_max.max(w.max_content + margin);
                count += 1;
            }
            let gaps = gap * count.saturating_sub(1) as f64;
            if row {
                inline::IntrinsicWidths { min_content: if wrap { max_min } else { sum_min + gaps }, max_content: sum_max + gaps }
            } else {
                inline::IntrinsicWidths { min_content: max_min, max_content: max_max }
            }
        };
        widths.min_content = widths.min_content.clamp(minimum, maximum);
        widths.max_content = widths.max_content.clamp(minimum, maximum);
        if ![widths.min_content, widths.max_content, extra].into_iter().all(f64::is_finite) {
            return Err(Error::Invalid);
        }
        Ok((widths, extra))
    }
}
