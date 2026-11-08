//! Row flex formatting integrated with retained normal-flow children. Item
//! intrinsic sizes come from measured text geometry (intrinsic_flow), never
//! estimates. Lines wrap (`flex-wrap`), each solved on its own and placed by
//! `align-content`; column direction is a container of its own and baseline
//! alignment is a recorded fallback until implemented.
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
    /// `order`: items are laid out in order-modified document order.
    order: i32,
}
/// One flex line: the items it holds, their solved main sizes, and its
/// cross size and offset (CSS Flexbox 9.3, 9.4).
struct Line {
    range: std::ops::Range<usize>,
    solved: Option<flex::Line>,
    remaining: f64,
    cross: f64,
    y: f64,
    /// Items with auto main-axis margins, and how many have taken their share.
    autos: usize,
    assigned: usize,
}
pub(super) struct Frame<'a> {
    pub id: BoxId,
    next: Option<BoxId>,
    items: Vec<Item>,
    inputs: Vec<flex::Item>,
    solver: Option<flex::Job<'a>>,
    lines: Vec<Line>,
    /// The line being solved, then the line the item at `index` is in.
    line: usize,
    stage: Stage,
    index: usize,
    gap: f64,
    row_gap: f64,
    wrap: bool,
    reverse: bool,
    justify: K,
    align: K,
    align_content: K,
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
        let wrap = match keyword(style, P::FlexWrap)? {
            K::Nowrap => false,
            K::WrapReverse => {
                fallbacks.note("flex-wrap: wrap-reverse as wrap", node);
                true
            }
            _ => true,
        };
        let mut align = keyword(style, P::AlignItems)?;
        if align == K::Baseline {
            fallbacks.note("flex baseline alignment as flex-start", node);
            align = K::FlexStart;
        }
        let gap_of = |p: P, scratch: &mut math::Scratch| -> Result<f64> {
            if keyword(style, p).ok() == Some(K::Normal) { Ok(0.) } else { used::length(style, p, Some(used.width), Some(0.), queries, scratch, cancel, scale)?.ok_or(Error::Invalid) }
        };
        let gap = gap_of(P::ColumnGap, scratch)?;
        let row_gap = gap_of(P::RowGap, scratch)?;
        Ok(Self {
            id,
            next,
            items: Vec::new(),
            inputs: Vec::new(),
            solver: None,
            lines: Vec::new(),
            line: 0,
            stage: Stage::Collect,
            index: 0,
            gap,
            row_gap,
            wrap,
            reverse: keyword(style, P::FlexDirection)? == K::RowReverse,
            justify: keyword(style, P::JustifyContent)?,
            align,
            align_content: keyword(style, P::AlignContent)?,
            cursor: 0.,
            distributed_gap: 0.,
        })
    }
    /// Split the collected items into lines: as many as fit each line by
    /// their hypothetical main sizes (CSS Flexbox 9.3 step 5), or all in one.
    fn partition(&mut self, width: f64) {
        let n = self.inputs.len();
        let mut lines = Vec::new();
        let mut start = 0;
        let mut sum = 0.;
        for (i, input) in self.inputs.iter().enumerate() {
            let size = input.base.max(input.minimum).min(input.maximum.max(input.minimum)) + input.outer;
            let with_gap = if i > start { sum + self.gap + size } else { size };
            if self.wrap && i > start && with_gap > width + 1e-6 {
                lines.push(start..i);
                start = i;
                sum = size;
            } else {
                sum = with_gap;
            }
        }
        lines.push(start..n);
        self.lines = lines
            .into_iter()
            .map(|range| {
                let autos = self.items[range.clone()].iter().map(|i| (i.autos & 3).count_ones() as usize).sum();
                Line { range, solved: None, remaining: 0., cross: 0., y: 0., autos, assigned: 0 }
            })
            .collect();
        self.line = 0;
    }
    /// The line holding the item at `index`, advancing `line` as items go by.
    fn line_of(&mut self, index: usize) -> usize {
        while self.line + 1 < self.lines.len() && !self.lines[self.line].range.contains(&index) {
            self.line += 1;
        }
        self.line
    }
    /// Place the lines along the cross axis: a single line is the container
    /// when that is definite, else its content clamped by the container's
    /// min and max (CSS Flexbox 9.4 step 8); several lines share the
    /// container's free space by `align-content` (step 15), stretching by
    /// default.
    fn place_lines(&mut self, used: Used, scale: f64) {
        let n = self.lines.len();
        let gaps = self.row_gap * n.saturating_sub(1) as f64;
        let total: f64 = self.lines.iter().map(|l| l.cross).sum::<f64>() + gaps;
        let container = used.height.map(|h| used.height(h));
        let (mut y, mut between) = (0., 0.);
        if n == 1 {
            self.lines[0].cross = used.height(self.lines[0].cross);
        } else if let Some(h) = container {
            let free = h - total;
            match self.align_content {
                K::Normal | K::Stretch if free > 0. => {
                    let share = free / n as f64;
                    for l in &mut self.lines {
                        l.cross += share;
                    }
                }
                K::Center => y = free / 2.,
                K::FlexEnd | K::End => y = free,
                K::SpaceBetween if free > 0. => between = free / (n - 1) as f64,
                K::SpaceAround if free > 0. => {
                    y = free / (2. * n as f64);
                    between = free / n as f64;
                }
                K::SpaceEvenly if free > 0. => {
                    y = free / (n + 1) as f64;
                    between = y;
                }
                _ => {}
            }
        }
        for l in &mut self.lines {
            l.y = used::grid(y, scale);
            y += l.cross + self.row_gap + between;
        }
    }
    /// The lines' total cross size, with the gaps between them.
    fn cross_extent(&self) -> f64 {
        self.lines.last().map_or(0., |l| l.y + l.cross)
    }
}

fn needs_intrinsic(s: &Style) -> bool {
    matches!(s.value(P::MinWidth), Value::Keyword(K::Auto | K::MinContent | K::MaxContent | K::FitContent))
        || [P::Width, P::MaxWidth].into_iter().any(|p| matches!(s.value(p), Value::Keyword(K::MinContent | K::MaxContent | K::FitContent)))
        || matches!(s.value(P::FlexBasis), Value::Keyword(K::Content | K::MinContent | K::MaxContent | K::FitContent))
        || (matches!(s.value(P::FlexBasis), Value::Keyword(K::Auto)) && matches!(s.value(P::Width), Value::Keyword(K::Auto | K::FitContent)))
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
        // An anonymous item's align-self is auto: the container's
        // align-items places it (stretching text changes nothing seen).
        let align = match align {
            K::Stretch | K::Normal | K::Baseline => K::FlexStart,
            other => other,
        };
        return Ok((Item { id, used, autos: 0, align, stretch: false, order: 0 }, flex::Item { base: widths.max_content, minimum: widths.min_content, ..Default::default() }));
    }
    let s = tree.style(id).ok_or(Error::Stale)?;
    if keyword(s, P::Visibility)? == K::Collapse {
        fallbacks.note("visibility: collapse flex item kept", node);
    }
    let order = s.value(P::Order).scalar().unwrap_or(0.).clamp(i32::MIN as f64, i32::MAX as f64) as i32;
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
        let used = super::images::used(s, parent.width, parent.height.map(|h| parent.height(h)), natural, false, false, false, queries, scratch, cancel, scale)?;
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
        return Ok((Item { id, used, autos, align, stretch: false, order }, input));
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
        // fit-content: the content's range, clamped by the container.
        Value::Keyword(K::FitContent) => Ok(Some(widths.max_content.min(parent.width - extra).max(widths.min_content.min(parent.width - extra)).max(0.))),
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
    Ok((Item { id, used, autos, align, stretch: matches!(s.value(P::Height), Value::Keyword(K::Auto)), order }, input))
}
impl<'a> Job<'a> {
    pub(super) fn flex_child_used(&self, id: BoxId) -> Result<Option<Used>> {
        let Some(f) = self.flex.last().filter(|f| self.stack.last().is_some_and(|p| p.id == f.id)) else { return Ok(None) };
        let item = f.items.get(f.index).filter(|i| i.id == id).ok_or(Error::Invalid)?;
        let mut used = item.used;
        let parent = self.stack.last().ok_or(Error::Invalid)?.used;
        let line = &f.lines[f.line];
        // The cross size the item stretches to: its line's once the lines
        // are sized, else the container's for a single definite line.
        let cross = if f.stage == Stage::Stretch {
            Some(line.cross)
        } else if f.lines.len() == 1 {
            parent.height.map(|h| parent.height(h))
        } else {
            None
        };
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
            let line = f.line;
            f.lines[line].cross = f.lines[line].cross.max(g.border_box.height + item.used.margin.vertical());
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
                    // Order-modified document order (CSS Flexbox 5.4): by
                    // `order`, document order among equals.
                    if f.items.windows(2).any(|w| w[0].order > w[1].order) {
                        let mut both: Vec<(Item, flex::Item)> = std::mem::take(&mut f.items).into_iter().zip(std::mem::take(&mut f.inputs)).collect();
                        both.sort_by_key(|(item, _)| item.order);
                        (f.items, f.inputs) = both.into_iter().unzip();
                    }
                    f.partition(used.width);
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
                f.items.push(item);
                f.inputs.push(input);
                f.next = b.next_sibling;
            }
            Stage::Solve => {
                let f = &mut self.flex[fi];
                if f.solver.is_none() {
                    let range = f.lines[f.line].range.clone();
                    f.solver = Some(flex::Job::new(f.inputs[range].to_vec(), used.width, f.gap, self.scale, self.limits, self.cancel)?);
                }
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
                    let l = &mut f.lines[f.line];
                    l.remaining = line.remaining();
                    l.solved = Some(line);
                    f.line += 1;
                    if f.line == f.lines.len() {
                        f.line = 0;
                        f.index = 0;
                        f.stage = Stage::Layout;
                    }
                }
            }
            Stage::Layout | Stage::Stretch => {
                let f = &mut self.flex[fi];
                if f.index < f.items.len() {
                    let line = f.line_of(f.index);
                    let start = f.lines[line].range.start;
                    let item = &mut f.items[f.index];
                    item.used.width = *f.lines[line].solved.as_ref().ok_or(Error::Invalid)?.sizes().get(f.index - start).ok_or(Error::Invalid)?;
                    if f.stage == Stage::Layout
                        && let Some(s) = self.cache.tree.style(item.id)
                    {
                        item.used.apply_aspect(s);
                    }
                    if f.stage == Stage::Stretch && !stretches(item) {
                        f.index += 1;
                    } else {
                        return Ok(Tick::Child(item.id));
                    }
                } else if f.stage == Stage::Layout && (used.height.is_none() || f.lines.len() > 1) {
                    // Lines sized by their content: place them, then stretch
                    // the items that fill their line.
                    f.place_lines(used, self.scale);
                    f.stage = Stage::Stretch;
                    f.index = 0;
                    f.line = 0;
                } else {
                    if f.lines.len() == 1 && used.height.is_some() {
                        f.place_lines(used, self.scale);
                    }
                    f.stage = Stage::Align;
                    f.index = 0;
                    f.line = 0;
                }
            }
            Stage::Align => {
                let f = &mut self.flex[fi];
                if f.index >= f.items.len() {
                    let extent = f.cross_extent();
                    self.stack.last_mut().ok_or(Error::Invalid)?.cursor = extent;
                    self.flex.pop();
                    return Ok(Tick::Done);
                }
                let li = f.line_of(f.index);
                if f.index == f.lines[li].range.start {
                    // The line's main-axis distribution (CSS Flexbox 9.5).
                    let line = &f.lines[li];
                    let remaining = if line.autos != 0 && line.remaining > 0. { 0. } else { line.remaining };
                    let count = line.range.len() as f64;
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
                let item = &f.items[f.index];
                let line = &mut f.lines[li];
                let g = &mut self.geometry[item.id.index()];
                let mut margin = item.used.margin;
                if line.autos != 0 && line.remaining > 0. {
                    let mut share = || {
                        let before = rounded(line.remaining * line.assigned as f64 / line.autos as f64, self.scale);
                        line.assigned += 1;
                        rounded(line.remaining * line.assigned as f64 / line.autos as f64, self.scale) - before
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
                let cross_space = line.cross - g.border_box.height - margin.vertical();
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
                g.border_box.y = line.y + margin.top + offset + item.used.relative.y;
                g.margin = margin;
                f.cursor += margin.horizontal() + g.border_box.width + f.gap;
                // Keep the cumulative fractional remainder, snap positions only.
                let k = (f.index - line.range.start) as f64;
                f.cursor += rounded(f.distributed_gap * (k + 1.), self.scale) - rounded(f.distributed_gap * k, self.scale);
                if ![g.border_box.x, g.border_box.y, f.cursor, line.cross].into_iter().all(f64::is_finite) {
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
