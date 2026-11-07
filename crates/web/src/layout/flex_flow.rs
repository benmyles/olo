//! Initial row flex formatting integrated with retained normal-flow children.
//! Unsupported intrinsic contexts are errors, never estimated text widths.
use super::*;
use crate::css::{
    properties::Keyword as K,
    style::{Direction, Property as P, Style, Value},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Collect,
    Solve,
    Layout,
    Stretch,
    Align,
}
pub(super) enum Tick {
    Pending,
    Child(BoxId),
    Done,
}
struct Item {
    id: BoxId,
    used: Used,
    autos: u8,
    align: K,
    stretch: bool,
}
pub(super) struct Frame<'a> {
    pub id: BoxId,
    next: Option<BoxId>,
    items: Vec<Item>,
    inputs: Vec<flex::Item>,
    solver: Option<flex::Job<'a>>,
    line: Option<flex::Line>,
    measuring: Option<(usize, inline::IntrinsicJob<'a>)>,
    stage: Stage,
    index: usize,
    gap: f64,
    reverse: bool,
    justify: K,
    align: K,
    cross: f64,
    remaining: f64,
    main_autos: usize,
    assigned_autos: usize,
    cursor: f64,
    distributed_gap: f64,
}
fn keyword(s: &Style, p: P) -> Result<K> {
    if let Value::Keyword(k) = s.value(p) { Ok(*k) } else { Err(Error::Invalid) }
}
impl<'a> Frame<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(id: BoxId, next: Option<BoxId>, style: &Style, used: Used, queries: QueryBasis, scratch: &mut math::Scratch, cancel: Option<&AtomicBool>, scale: f64) -> Result<Self> {
        if style.axes().direction != Direction::Ltr || !matches!(keyword(style, P::FlexDirection)?, K::Row | K::RowReverse) {
            return Err(Error::Unsupported("non-LTR-row flex formatting"));
        }
        if keyword(style, P::FlexWrap)? != K::Nowrap {
            return Err(Error::Unsupported("wrapping flex formatting"));
        }
        let align = keyword(style, P::AlignItems)?;
        if align == K::Baseline {
            return Err(Error::Unsupported("flex baseline alignment"));
        }
        let gap = if keyword(style, P::ColumnGap).ok() == Some(K::Normal) {
            0.
        } else {
            used::length(style, P::ColumnGap, Some(used.width), Some(0.), queries, scratch, cancel, scale)?.ok_or(Error::Invalid)?
        };
        Ok(Self {
            id,
            next,
            items: Vec::new(),
            inputs: Vec::new(),
            solver: None,
            line: None,
            measuring: None,
            stage: Stage::Collect,
            index: 0,
            gap,
            reverse: keyword(style, P::FlexDirection)? == K::RowReverse,
            justify: keyword(style, P::JustifyContent)?,
            align,
            cross: 0.,
            remaining: 0.,
            main_autos: 0,
            assigned_autos: 0,
            cursor: 0.,
            distributed_gap: 0.,
        })
    }
}

fn needs_intrinsic(s: &Style) -> bool {
    matches!(s.value(P::MinWidth), Value::Keyword(K::Auto | K::MinContent | K::MaxContent))
        || [P::Width, P::MaxWidth].into_iter().any(|p| matches!(s.value(p), Value::Keyword(K::MinContent | K::MaxContent)))
        || matches!(s.value(P::FlexBasis), Value::Keyword(K::Content | K::MinContent | K::MaxContent))
        || (matches!(s.value(P::FlexBasis), Value::Keyword(K::Auto)) && matches!(s.value(P::Width), Value::Keyword(K::Auto)))
}
fn intrinsic_source(tree: &Tree, id: BoxId, cancel: Option<&AtomicBool>) -> Result<Option<u32>> {
    let b = tree.get(id).ok_or(Error::Invalid)?;
    if let BoxKind::InlineContent { source } = b.kind {
        return Ok(Some(source));
    }
    let mut next = b.first_child;
    let mut content = None;
    while let Some(id) = next {
        check(cancel)?;
        let child = tree.get(id).ok_or(Error::Invalid)?;
        next = child.next_sibling;
        if matches!(child.kind, BoxKind::Block { absolute: true, .. }) {
            continue;
        }
        if content.replace(child).is_some() {
            return Err(Error::Unsupported("nested flex item intrinsic contributions"));
        }
    }
    let Some(child) = content else { return Ok(None) };
    if let BoxKind::InlineContent { source } = child.kind {
        // Inline edges require independent intrinsic contribution rules.
        for edge in tree.inline_boxes(source).ok_or(Error::Invalid)? {
            check(cancel)?;
            let style = tree.styles.snapshot(edge.node).ok_or(Error::Stale)?;
            if used::has_inline_edge(style, true) || used::has_inline_edge(style, false) {
                return Err(Error::Unsupported("decorated flex text intrinsic sizing"));
            }
        }
        return Ok(Some(source));
    }
    Err(Error::Unsupported("nested flex item intrinsic contributions"))
}

#[allow(clippy::too_many_arguments)]
fn item(
    tree: &Tree,
    id: BoxId,
    parent: Used,
    queries: QueryBasis,
    widths: inline::IntrinsicWidths,
    align: K,
    scratch: &mut math::Scratch,
    cancel: Option<&AtomicBool>,
    scale: f64,
) -> Result<(Item, flex::Item)> {
    let b = tree.get(id).ok_or(Error::Invalid)?;
    if matches!(b.kind, BoxKind::InlineContent { .. }) {
        let used = Used {
            width: 0.,
            height: None,
            min_height: 0.,
            max_height: f64::MAX,
            padding: Edges::default(),
            border: Edges::default(),
            margin: Edges::default(),
            relative: Point::default(),
            bfc: true,
            rtl: false,
        };
        return Ok((Item { id, used, autos: 0, align: K::FlexStart, stretch: false }, flex::Item { base: widths.max_content, minimum: widths.min_content, ..Default::default() }));
    }
    if !matches!(b.kind, BoxKind::Block { .. }) {
        return Err(Error::Unsupported("replaced flex item sizing"));
    }
    let s = tree.style(id).ok_or(Error::Stale)?;
    if keyword(s, P::Visibility)? == K::Collapse {
        return Err(Error::Unsupported("collapsed flex items"));
    }
    if s.value(P::Order).scalar() != Some(0.) {
        return Err(Error::Unsupported("ordered flex items"));
    }
    let used = Used::flex(s, parent.width, parent.height.map(|h| parent.height(h)), queries, scratch, cancel, scale)?;
    let extra = used.padding.horizontal() + used.border.horizontal();
    let border_box = keyword(s, P::BoxSizing)? == K::BorderBox;
    let mut size = |p, fallback| match s.value(p) {
        Value::Keyword(K::MinContent) => Ok(Some(widths.min_content)),
        Value::Keyword(K::MaxContent) => Ok(Some(widths.max_content)),
        _ => used::length(s, p, Some(parent.width), fallback, queries, scratch, cancel, scale)
            .map(|v| v.map(|v| (v - if border_box && matches!(s.value(p), Value::Numeric(_)) { extra } else { 0. }).max(0.))),
    };
    let specified = size(P::Width, None)?;
    let maximum = size(P::MaxWidth, None)?.unwrap_or(f64::INFINITY);
    let minimum = if keyword(s, P::MinWidth).ok() == Some(K::Auto) {
        let scrollable = [P::OverflowX, P::OverflowY].into_iter().any(|p| !matches!(s.value(p), Value::Keyword(K::Visible | K::Clip)));
        if scrollable { 0. } else { widths.min_content.min(specified.unwrap_or(f64::INFINITY)).min(maximum) }
    } else {
        size(P::MinWidth, Some(0.))?.ok_or(Error::Invalid)?
    };
    let base = match s.value(P::FlexBasis) {
        Value::Keyword(K::Auto) => specified.unwrap_or(widths.max_content),
        Value::Keyword(K::Content | K::MaxContent) => widths.max_content,
        Value::Keyword(K::MinContent) => widths.min_content,
        Value::Numeric(_) => {
            let raw = used::length(s, P::FlexBasis, Some(parent.width), None, queries, scratch, cancel, scale)?.ok_or(Error::Invalid)?;
            if border_box && raw < extra {
                return Err(Error::Unsupported("negative border-box flex inner basis"));
            }
            raw - if border_box { extra } else { 0. }
        }
        _ => return Err(Error::Unsupported("fit-content flex basis")),
    };
    let mut autos = 0;
    for (bit, p) in [P::MarginLeft, P::MarginRight, P::MarginTop, P::MarginBottom].into_iter().enumerate() {
        if keyword(s, p).ok() == Some(K::Auto) {
            autos |= 1 << bit;
        }
    }
    let align = match keyword(s, P::AlignSelf)? {
        K::Auto => align,
        other => other,
    };
    if align == K::Baseline {
        return Err(Error::Unsupported("flex baseline alignment"));
    }
    let input = flex::Item {
        base,
        minimum,
        maximum,
        grow: s.value(P::FlexGrow).scalar().ok_or(Error::Invalid)?,
        shrink: s.value(P::FlexShrink).scalar().ok_or(Error::Invalid)?,
        outer: extra + used.margin.horizontal(),
    };
    Ok((Item { id, used, autos, align, stretch: matches!(s.value(P::Height), Value::Keyword(K::Auto)) }, input))
}
impl<'a> Job<'a> {
    pub(super) fn flex_child_used(&self, id: BoxId) -> Result<Option<Used>> {
        let Some(f) = self.flex.last().filter(|f| self.stack.last().is_some_and(|p| p.id == f.id)) else { return Ok(None) };
        let item = f.items.get(f.index).filter(|i| i.id == id).ok_or(Error::Invalid)?;
        let mut used = item.used;
        let parent = self.stack.last().ok_or(Error::Invalid)?.used;
        let cross = if f.stage == Stage::Stretch { Some(parent.height(f.cross)) } else { parent.height.map(|h| parent.height(h)) };
        if let Some(cross) = cross
            && stretches(item)
        {
            used.height = Some(used.height((cross - used.margin.vertical() - used.padding.vertical() - used.border.vertical()).max(0.)));
        }
        Ok(Some(used))
    }
    pub(super) fn place_flex(&mut self, id: BoxId, relative_y: f64) -> Result<bool> {
        let Some(f) = self.flex.last_mut().filter(|f| self.stack.last().is_some_and(|p| p.id == f.id)) else { return Ok(false) };
        let item = f.items.get(f.index).filter(|i| i.id == id).ok_or(Error::Invalid)?;
        let g = &mut self.geometry[id.index()];
        g.border_box.y = item.used.margin.top + relative_y;
        if f.stage == Stage::Layout {
            f.cross = f.cross.max(g.border_box.height + item.used.margin.vertical());
        }
        f.index += 1;
        Ok(true)
    }
    pub(super) fn flex_tick(&mut self) -> Result<Tick> {
        let fi = self.flex.len().checked_sub(1).ok_or(Error::Invalid)?;
        let parent = self.stack.last().ok_or(Error::Invalid)?;
        let used = parent.used;
        let queries = parent.queries;
        match self.flex[fi].stage {
            Stage::Collect => {
                if let Some((_, job)) = &mut self.flex[fi].measuring {
                    let before = job.progress().work;
                    let progress = job.step(128)?;
                    self.progress.work = self.progress.work.checked_add(progress.work - before).ok_or(Error::Limit)?;
                    if self.progress.work > self.limits.work {
                        return Err(Error::Limit);
                    }
                    if progress.finished {
                        let (source, job) = self.flex[fi].measuring.take().ok_or(Error::Invalid)?;
                        self.intrinsics.insert(source, job.finish()?);
                    }
                    return Ok(Tick::Pending);
                }
                let Some(id) = self.flex[fi].next else {
                    let f = &mut self.flex[fi];
                    f.solver = Some(flex::Job::new(std::mem::take(&mut f.inputs), used.width, f.gap, self.scale, self.limits, self.cancel)?);
                    f.stage = Stage::Solve;
                    return Ok(Tick::Pending);
                };
                if self.positioned(id)? {
                    self.flex[fi].next = self.cache.tree.get(id).ok_or(Error::Invalid)?.next_sibling;
                    self.defer_absolute(id)?;
                    return Ok(Tick::Pending);
                }
                let tree = &self.cache.tree;
                let b = tree.get(id).ok_or(Error::Invalid)?;
                let mut widths = inline::IntrinsicWidths::default();
                if matches!(b.kind, BoxKind::InlineContent { .. }) || needs_intrinsic(tree.style(id).ok_or(Error::Stale)?) {
                    if !matches!(b.kind, BoxKind::InlineContent { .. }) && keyword(tree.style(id).ok_or(Error::Stale)?, P::ContainerType)? != K::Normal {
                        return Err(Error::Unsupported("contained flex item intrinsic sizing"));
                    }
                    if let Some(source) = intrinsic_source(tree, id, self.cancel)? {
                        // Sources with identical retained typography/text share
                        // the same Arc. Measure that immutable geometry once.
                        let key = Arc::as_ptr(&self.cache.texts[source as usize]) as usize;
                        if let Some(cached) = self.intrinsics.get(&key).or_else(|| self.cache.intrinsics.as_ref().and_then(|m| m.get(&key))) {
                            widths = *cached;
                        } else {
                            self.flex[fi].measuring = Some((key, inline::IntrinsicJob::new(self.cache.texts[source as usize].clone(), Some(self.scale), self.limits.text, self.cancel)?));
                            return Ok(Tick::Pending);
                        }
                    }
                }
                let (item, input) = item(tree, id, used, queries, widths, self.flex[fi].align, &mut self.scratch, self.cancel, self.scale)?;
                let f = &mut self.flex[fi];
                if f.items.len() >= self.limits.boxes {
                    return Err(Error::Limit);
                }
                f.main_autos += (item.autos & 3).count_ones() as usize;
                f.items.push(item);
                f.inputs.push(input);
                f.next = b.next_sibling;
            }
            Stage::Solve => {
                let f = &mut self.flex[fi];
                let solver = f.solver.as_mut().ok_or(Error::Invalid)?;
                let before = solver.progress().work;
                let progress = solver.step(128)?;
                self.progress.work = self.progress.work.checked_add(progress.work - before).ok_or(Error::Limit)?;
                if self.progress.work > self.limits.work {
                    return Err(Error::Limit);
                }
                if progress.finished {
                    let line = f.solver.take().ok_or(Error::Invalid)?.finish()?;
                    // Publishing each resolved item is budgeted independently.
                    f.remaining = line.remaining();
                    f.line = Some(line);
                    f.stage = Stage::Layout;
                }
            }
            Stage::Layout | Stage::Stretch => {
                let f = &mut self.flex[fi];
                if let Some(item) = f.items.get_mut(f.index) {
                    item.used.width = *f.line.as_ref().ok_or(Error::Invalid)?.sizes().get(f.index).ok_or(Error::Invalid)?;
                    if f.stage == Stage::Stretch && !stretches(item) {
                        f.index += 1;
                    } else {
                        return Ok(Tick::Child(item.id));
                    }
                } else if f.stage == Stage::Layout && used.height.is_none() {
                    f.stage = Stage::Stretch;
                    f.index = 0;
                } else {
                    f.stage = Stage::Align;
                    f.index = 0;
                    let remaining = if f.main_autos != 0 && f.remaining > 0. { 0. } else { f.remaining };
                    let count = f.items.len() as f64;
                    let justify = match f.justify {
                        K::Start | K::Left => {
                            if f.reverse {
                                K::FlexEnd
                            } else {
                                K::FlexStart
                            }
                        }
                        K::End | K::Right => {
                            if f.reverse {
                                K::FlexStart
                            } else {
                                K::FlexEnd
                            }
                        }
                        K::SpaceBetween if remaining < 0. || count < 2. => K::FlexStart,
                        K::SpaceAround | K::SpaceEvenly if remaining < 0. => K::Center,
                        other => other,
                    };
                    let (offset, gap) = match justify {
                        K::FlexEnd => (remaining, 0.),
                        K::Center => (remaining / 2., 0.),
                        K::SpaceBetween if count > 1. => (0., remaining / (count - 1.)),
                        K::SpaceAround if count > 0. => (remaining / (2. * count), remaining / count),
                        K::SpaceEvenly => (remaining / (count + 1.), remaining / (count + 1.)),
                        _ => (0., 0.),
                    };
                    f.cursor = if justify == K::Center && f.reverse { remaining - used::grid(offset, self.scale) } else { used::grid(offset, self.scale) };
                    f.distributed_gap = gap;
                }
            }
            Stage::Align => {
                let f = &mut self.flex[fi];
                let Some(item) = f.items.get(f.index) else {
                    self.stack.last_mut().ok_or(Error::Invalid)?.cursor = f.cross;
                    self.flex.pop();
                    return Ok(Tick::Done);
                };
                let g = &mut self.geometry[item.id.index()];
                let mut margin = item.used.margin;
                if f.main_autos != 0 && f.remaining > 0. {
                    let mut share = || {
                        let before = rounded(f.remaining * f.assigned_autos as f64 / f.main_autos as f64, self.scale);
                        f.assigned_autos += 1;
                        rounded(f.remaining * f.assigned_autos as f64 / f.main_autos as f64, self.scale) - before
                    };
                    if f.reverse {
                        if item.autos & 2 != 0 {
                            margin.right = share();
                        }
                        if item.autos & 1 != 0 {
                            margin.left = share();
                        }
                    } else {
                        if item.autos & 1 != 0 {
                            margin.left = share();
                        }
                        if item.autos & 2 != 0 {
                            margin.right = share();
                        }
                    }
                }
                let start_margin = if f.reverse { margin.right } else { margin.left };
                g.border_box.x = if f.reverse { used.width - f.cursor - start_margin - g.border_box.width } else { f.cursor + start_margin } + item.used.relative.x;
                let cross_space = used.height(f.cross) - g.border_box.height - margin.vertical();
                let mut offset = 0.;
                if item.autos & 12 != 0 {
                    if cross_space > 0. {
                        if item.autos & 12 == 12 {
                            margin.top = used::grid(cross_space / 2., self.scale);
                            margin.bottom = cross_space - margin.top;
                        } else if item.autos & 4 != 0 {
                            margin.top = cross_space;
                        } else {
                            margin.bottom = cross_space;
                        }
                    } else {
                        margin.bottom += cross_space;
                    }
                } else {
                    offset = match item.align {
                        K::FlexEnd | K::End => cross_space,
                        K::Center => used::grid(cross_space / 2., self.scale),
                        _ => 0.,
                    };
                }
                g.border_box.y = margin.top + offset + item.used.relative.y;
                g.margin = margin;
                f.cursor += margin.horizontal() + g.border_box.width + f.gap;
                // Keep the cumulative fractional remainder, snap positions only.
                f.cursor += rounded(f.distributed_gap * (f.index + 1) as f64, self.scale) - rounded(f.distributed_gap * f.index as f64, self.scale);
                if ![g.border_box.x, g.border_box.y, f.cursor, f.cross].into_iter().all(f64::is_finite) {
                    return Err(Error::Invalid);
                }
                f.index += 1;
            }
        }
        Ok(Tick::Pending)
    }
}
fn stretches(item: &Item) -> bool {
    item.stretch && item.used.height.is_none() && item.autos & 12 == 0 && matches!(item.align, K::Normal | K::Stretch)
}

fn rounded(value: f64, scale: f64) -> f64 {
    (value * scale * 64.).round() / (scale * 64.)
}
