//! Row flex formatting integrated with retained normal-flow children. Item
//! intrinsic sizes come from measured text geometry (intrinsic_flow), never
//! estimates. Wrapping, column direction and baseline alignment are recorded
//! fallbacks until implemented.
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
    pub fn new(
        id: BoxId,
        next: Option<BoxId>,
        style: &Style,
        used: Used,
        queries: QueryBasis,
        scratch: &mut math::Scratch,
        fallbacks: &mut Fallbacks,
        cancel: Option<&AtomicBool>,
        scale: f64,
    ) -> Result<Self> {
        if !matches!(keyword(style, P::FlexDirection)?, K::Row | K::RowReverse) {
            return Err(Error::Invalid);
        }
        let node = None;
        if style.axes().direction != Direction::Ltr {
            fallbacks.note("right-to-left flex as left-to-right", node);
        }
        if keyword(style, P::FlexWrap)? != K::Nowrap {
            fallbacks.note("flex-wrap as nowrap", node);
        }
        let mut align = keyword(style, P::AlignItems)?;
        if align == K::Baseline {
            fallbacks.note("flex baseline alignment as flex-start", node);
            align = K::FlexStart;
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
#[allow(clippy::too_many_arguments)]
fn item(
    tree: &Tree,
    id: BoxId,
    parent: Used,
    queries: QueryBasis,
    widths: inline::IntrinsicWidths,
    align: K,
    scratch: &mut math::Scratch,
    fallbacks: &mut Fallbacks,
    cancel: Option<&AtomicBool>,
    scale: f64,
) -> Result<(Item, flex::Item)> {
    let b = tree.get(id).ok_or(Error::Invalid)?;
    let node = Some(b.node);
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
    let s = tree.style(id).ok_or(Error::Stale)?;
    if keyword(s, P::Visibility)? == K::Collapse {
        fallbacks.note("visibility: collapse flex item kept", node);
    }
    if s.value(P::Order).scalar() != Some(0.) {
        fallbacks.note("flex order ignored", node);
    }
    let align = match keyword(s, P::AlignSelf)? {
        K::Auto => align,
        K::Baseline => {
            fallbacks.note("flex baseline alignment as flex-start", node);
            K::FlexStart
        }
        other => other,
    };
    if b.kind.is_replaced() {
        // Images, vectors and placeholders: the natural size is the content
        // size, and the automatic minimum is that content (CSS Flexbox 4.5).
        let natural = tree.natural(id)?;
        let used = super::images::used(s, parent.width, parent.height.map(|h| parent.height(h)), natural, false, false, queries, scratch, cancel, scale)?;
        let extra = used.padding.horizontal() + used.border.horizontal();
        let maximum = used::length(s, P::MaxWidth, Some(parent.width), None, queries, scratch, cancel, scale)?.unwrap_or(f64::INFINITY);
        let minimum = if keyword(s, P::MinWidth).ok() == Some(K::Auto) {
            used.width.min(maximum)
        } else {
            used::length(s, P::MinWidth, Some(parent.width), Some(0.), queries, scratch, cancel, scale)?.unwrap_or(0.)
        };
        let mut autos = 0;
        for (bit, p) in [P::MarginLeft, P::MarginRight, P::MarginTop, P::MarginBottom].into_iter().enumerate() {
            if keyword(s, p).ok() == Some(K::Auto) {
                autos |= 1 << bit;
            }
        }
        let input = flex::Item {
            base: used.width,
            minimum,
            maximum,
            grow: s.value(P::FlexGrow).scalar().ok_or(Error::Invalid)?,
            shrink: s.value(P::FlexShrink).scalar().ok_or(Error::Invalid)?,
            outer: extra + used.margin.horizontal(),
        };
        return Ok((Item { id, used, autos, align, stretch: false }, input));
    }
    if !matches!(b.kind, BoxKind::Block { .. }) {
        return Err(Error::Invalid);
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
            (raw - if border_box { extra } else { 0. }).max(0.)
        }
        _ => {
            fallbacks.note("fit-content flex basis as content", node);
            widths.max_content.min(specified.unwrap_or(f64::INFINITY)).max(widths.min_content)
        }
    };
    let mut autos = 0;
    for (bit, p) in [P::MarginLeft, P::MarginRight, P::MarginTop, P::MarginBottom].into_iter().enumerate() {
        if keyword(s, p).ok() == Some(K::Auto) {
            autos |= 1 << bit;
        }
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
                let cache = self.cache.clone();
                let b = cache.tree.get(id).ok_or(Error::Invalid)?;
                let mut widths = inline::IntrinsicWidths::default();
                let text = matches!(b.kind, BoxKind::InlineContent { .. });
                if text || needs_intrinsic(cache.tree.style(id).ok_or(Error::Stale)?) {
                    if !text && keyword(cache.tree.style(id).ok_or(Error::Stale)?, P::ContainerType)? != K::Normal {
                        self.fallbacks.note("query container flex item sized by its content", Some(b.node));
                    }
                    // Content-box widths: item() adds the item's own padding, border and margin.
                    let content_basis = !text && matches!(cache.tree.style(id).ok_or(Error::Stale)?.value(P::FlexBasis), Value::Keyword(K::Content | K::MinContent | K::MaxContent));
                    widths = self.intrinsic_content_of(id, queries, used.width, 0, content_basis)?.0;
                }
                let align = self.flex[fi].align;
                let (item, input) = item(&cache.tree, id, used, queries, widths, align, &mut self.scratch, &mut self.fallbacks, self.cancel, self.scale)?;
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
