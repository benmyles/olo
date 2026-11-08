//! Formatting contexts whose children lay out as blocks first and are placed
//! once their heights are known: grid (CSS Grid 2, track sizing and
//! auto-placement) and column flex. Widths are settled before the children
//! lay out (grid columns from the track list and the items' measured
//! intrinsic widths); rows and positions follow at the container's end.
use super::*;
use crate::css::{
    properties::{AutoRepeat, Keyword as K, Property as P, TrackEntry, TrackSize, Tracks},
    style::{Style, Value},
};
#[derive(Clone, Copy, Debug)]
struct Cell {
    /// Track index ranges (end exclusive) in the resolved grids.
    columns: (usize, usize),
    rows: (usize, usize),
}
struct Item {
    id: BoxId,
    /// Border-box width settled before layout, with its resolved margins.
    used: Used,
    /// Min/max-content border-box contributions, margins included; the
    /// min one is the item's minimum contribution (its `min-width` where
    /// it has one), which only `auto` minimums take.
    contribution: (f64, f64),
    /// Its min-content contribution itself, which `min-content` (and
    /// `max-content`) minimums take.
    min_content: f64,
    cell: Cell,
    justify: K,
    align: K,
    /// The item's height after layout, border box plus vertical margins.
    outer_height: f64,
    grow: f64,
    shrink: f64,
    basis: Option<f64>,
    /// `order` (flex items are placed in order-modified document order).
    order: i32,
}
enum Kind {
    Grid {
        columns: Vec<f64>,
        rows: Vec<Track>,
        align_content: K,
        justify_content: K,
    },
    ColumnFlex {
        reverse: bool,
        justify: K,
    },
    /// A table row: cells at the enclosing table's column positions.
    Row {
        columns: Vec<f64>,
        spacing: (f64, f64),
    },
}
/// An open table: the column widths its rows place cells by.
pub(super) struct Table {
    pub id: BoxId,
    columns: Vec<f64>,
    spacing: (f64, f64),
}
pub(super) struct Container {
    pub id: BoxId,
    kind: Kind,
    items: Vec<Item>,
    row_gap: f64,
    column_gap: f64,
    width: f64,
    height: Option<f64>,
    /// `min-height` and `max-height`: a container taller than its content
    /// through `min-height` aligns its tracks in that height.
    min_height: f64,
    max_height: f64,
}
/// A finished column flex container's items, kept while its height may
/// still change.
pub(super) struct Column {
    items: Vec<Item>,
    row_gap: f64,
    width: f64,
    reverse: bool,
    justify: K,
    min_height: f64,
    max_height: f64,
}
impl Container {
    /// The height tracks are aligned in: the used height, or `min-height`
    /// when it exceeds the content's `natural` height; None when the
    /// content decides (nothing to distribute).
    fn definite_height(height: Option<f64>, min_height: f64, max_height: f64, natural: f64) -> Option<f64> {
        match height {
            Some(h) => Some(h.min(max_height).max(min_height)),
            None if min_height > natural => Some(min_height.min(max_height)),
            None => None,
        }
    }
}
/// A span of tracks (end exclusive) with its min- and max-content widths.
type Contribution = (usize, usize, f64, f64);
/// One bound of a track, resolved against the container.
#[derive(Clone, Copy, Debug)]
enum Bound {
    Fixed(f64),
    Auto,
    MinContent,
    MaxContent,
    FitContent(f64),
    Flex(f64),
}
#[derive(Clone, Copy, Debug)]
struct Track {
    min: Bound,
    max: Bound,
}
pub(in crate::layout) fn is_grid(style: &Style) -> bool {
    matches!(style.value(P::Display), Value::Display(crate::css::properties::Display::Box { inner: crate::css::properties::Inner::Grid, .. }))
}
pub(super) fn is_column_flex(style: &Style) -> bool {
    super::flow::is_flex(style) && !super::flow::is_row_flex(style)
}
pub(in crate::layout) fn is_table(style: &Style) -> bool {
    matches!(style.value(P::Display), Value::Display(crate::css::properties::Display::Box { inner: crate::css::properties::Inner::Table, .. }))
}
pub(in crate::layout) fn is_table_row(style: &Style) -> bool {
    matches!(style.value(P::Display), Value::Display(crate::css::properties::Display::Internal(crate::css::properties::Internal::TableRow)))
}
fn is_row_group(style: &Style) -> bool {
    use crate::css::properties::Internal as I;
    matches!(style.value(P::Display), Value::Display(crate::css::properties::Display::Internal(I::TableRowGroup | I::TableHeaderGroup | I::TableFooterGroup)))
}
fn is_cell(style: &Style) -> bool {
    matches!(style.value(P::Display), Value::Display(crate::css::properties::Display::Internal(crate::css::properties::Internal::TableCell)))
}
fn keyword(s: &Style, p: P) -> K {
    if let Value::Keyword(k) = s.value(p) { *k } else { K::Auto }
}
impl Job<'_> {
    fn bound(&mut self, size: &TrackSize, basis: Option<f64>, queries: QueryBasis) -> Result<Bound> {
        Ok(match size {
            TrackSize::Length(n) => {
                if basis.is_none() && n.expression().dependencies().contains(math::RelativeUnit::Percentage) {
                    Bound::Auto
                } else {
                    struct Basis(Option<f64>, QueryBasis);
                    impl math::Context for Basis {
                        fn factor(&self, unit: math::RelativeUnit) -> math::Result<f64> {
                            if unit == math::RelativeUnit::Percentage { self.0.map(|v| v / 100.).ok_or(math::Error::ContextUnavailable) } else { self.1.factor(unit) }
                        }
                    }
                    Bound::Fixed(used::grid(n.resolve(&Basis(basis, queries), &mut self.scratch, Default::default(), self.cancel)?.max(0.), self.scale))
                }
            }
            TrackSize::Fr(f) => Bound::Flex(*f),
            TrackSize::Auto => Bound::Auto,
            TrackSize::MinContent => Bound::MinContent,
            TrackSize::MaxContent => Bound::MaxContent,
            TrackSize::FitContent(n) => match self.bound(&TrackSize::Length(n.clone()), basis, queries)? {
                Bound::Fixed(v) => Bound::FitContent(v),
                _ => Bound::Auto,
            },
        })
    }
    /// The explicit tracks of one axis, auto-repeats expanded to what fits
    /// (one per item when nothing about the repeated tracks is definite,
    /// as `minmax(0, 1fr)`: as many as can be filled); with the range of
    /// an `auto-fit` repeat, whose empty tracks collapse after placement.
    fn tracks(&mut self, value: &Value, available: Option<f64>, gap: f64, queries: QueryBasis, items: usize) -> Result<(Vec<Track>, Option<(usize, usize)>)> {
        let Value::Tracks(Tracks(entries)) = value else { return Ok((Vec::new(), None)) };
        let mut out = Vec::new();
        let mut fit = None;
        for entry in entries.iter() {
            check(self.cancel)?;
            match entry {
                TrackEntry::Track(t) => out.push(Track { min: self.bound(&t.min, available, queries)?, max: self.bound(&t.max, available, queries)? }),
                TrackEntry::AutoRepeat(kind, tracks) => {
                    let mut body = Vec::with_capacity(tracks.len());
                    let mut definite = 0.;
                    for t in tracks.iter() {
                        let track = Track { min: self.bound(&t.min, available, queries)?, max: self.bound(&t.max, available, queries)? };
                        definite += match (track.max, track.min) {
                            (Bound::Fixed(v), _) | (_, Bound::Fixed(v)) => v,
                            _ => 0.,
                        };
                        body.push(track);
                    }
                    let fixed_elsewhere: f64 = entries
                        .iter()
                        .filter_map(|e| if let TrackEntry::Track(t) = e { Some(t) } else { None })
                        .map(|t| match (&t.max, &t.min) {
                            (TrackSize::Length(n), _) | (_, TrackSize::Length(n)) => n.expression().constant().map_or(0., |q| q.value.max(0.)),
                            _ => 0.,
                        })
                        .sum();
                    let others = entries.len() - 1;
                    let copies = match available {
                        Some(w) if definite > 0. => {
                            let room = w - fixed_elsewhere - gap * others as f64;
                            ((room + gap) / (definite + gap * body.len() as f64)).floor().max(1.) as usize
                        }
                        Some(_) => items.div_ceil(body.len().max(1)),
                        None => 1,
                    };
                    let copies = copies.min(10_000 / body.len().max(1)).max(1);
                    if matches!(kind, AutoRepeat::Fit) {
                        fit = Some((out.len(), out.len() + copies * body.len()));
                    }
                    for _ in 0..copies {
                        out.extend(body.iter().copied());
                    }
                }
            }
            if out.len() > 10_000 {
                return Err(Error::Limit);
            }
        }
        Ok((out, fit))
    }
    /// The rows of a table box, through its row groups, each with its cells.
    fn table_rows(&self, table: BoxId) -> Result<Vec<(BoxId, Vec<BoxId>)>> {
        let tree = &self.cache.tree;
        let mut rows = Vec::new();
        let mut next = tree.get(table).ok_or(Error::Invalid)?.first_child;
        let mut groups: Vec<Option<BoxId>> = Vec::new();
        while let Some(child) = next.or_else(|| groups.pop().flatten()) {
            check(self.cancel)?;
            let b = tree.get(child).ok_or(Error::Invalid)?;
            next = b.next_sibling;
            if b.absolute || !matches!(b.kind, BoxKind::Block { .. }) {
                continue;
            }
            let Some(s) = tree.style(child) else { continue };
            if is_row_group(s) {
                // Rows of this group come next; the table's own siblings after.
                groups.push(next);
                next = b.first_child;
            } else if is_table_row(s) {
                let mut cells = Vec::new();
                let mut c = b.first_child;
                while let Some(cell) = c {
                    let cb = tree.get(cell).ok_or(Error::Invalid)?;
                    c = cb.next_sibling;
                    if !cb.absolute && matches!(cb.kind, BoxKind::Block { .. }) && tree.style(cell).is_some_and(is_cell) {
                        cells.push(cell);
                    }
                }
                rows.push((child, cells));
            }
            if rows.len() > self.limits.boxes {
                return Err(Error::Limit);
            }
        }
        Ok(rows)
    }
    /// Column contributions of a table (min and max, with column spans).
    fn table_contributions(&mut self, table: BoxId, queries: QueryBasis, basis: f64) -> Result<(usize, Vec<Contribution>)> {
        let rows = self.table_rows(table)?;
        let mut contributions = Vec::new();
        let mut columns = 0;
        for (_, cells) in &rows {
            let mut at = 0usize;
            for &cell in cells {
                let span = usize::from(self.cache.tree.get(cell).ok_or(Error::Invalid)?.span[0]).max(1);
                let w = self.intrinsic(cell, queries, basis, 0)?;
                contributions.push((at, at + span, w.min_content, w.max_content));
                at += span;
            }
            columns = columns.max(at);
        }
        Ok((columns, contributions))
    }
    /// A table's min- and max-content widths, spacing included.
    pub(super) fn table_intrinsic(&mut self, table: BoxId, style: &Style, queries: QueryBasis, basis: f64) -> Result<(f64, f64)> {
        let (columns, contributions) = self.table_contributions(table, queries, basis)?;
        let (sx, _) = self.spacing(style, queries);
        let gaps = sx * (columns + 1) as f64;
        let tracks = vec![Track { min: Bound::Auto, max: Bound::Auto }; columns];
        let mins = size_tracks(&tracks, &contributions, None, sx, false, self.scale);
        let maxes = column_maxes(columns, &contributions);
        Ok((mins.iter().sum::<f64>() + gaps, maxes.iter().sum::<f64>() + gaps))
    }
    fn spacing(&mut self, style: &Style, queries: QueryBasis) -> (f64, f64) {
        let _ = queries;
        match style.value(P::BorderSpacing) {
            Value::Radius(r) => {
                let v = |n: &crate::css::properties::Numeric| n.expression().constant().map_or(0., |q| q.value).max(0.);
                (used::grid(v(&r.x), self.scale), used::grid(v(&r.y), self.scale))
            }
            _ => (0., 0.),
        }
    }
    /// Settle a table's columns and its own width (shrink-to-fit when auto),
    /// before its rows lay out.
    pub(super) fn begin_table(&mut self, id: BoxId, style: &Style, used: &mut Used, available: f64, queries: QueryBasis) -> Result<()> {
        let (columns, contributions) = self.table_contributions(id, queries, available)?;
        let (sx, sy) = self.spacing(style, queries);
        let gaps = sx * (columns + 1) as f64;
        let extra = used.padding.horizontal() + used.border.horizontal();
        let specified = used::length(style, P::Width, Some(available), None, queries, &mut self.scratch, self.cancel, self.scale)?;
        let tracks = vec![Track { min: Bound::Auto, max: Bound::Auto }; columns];
        let content = if specified.is_some() {
            used.width
        } else {
            let mins = size_tracks(&tracks, &contributions, None, sx, false, self.scale);
            let maxes = column_maxes(columns, &contributions);
            let (min, max) = (mins.iter().sum::<f64>() + gaps, maxes.iter().sum::<f64>() + gaps);
            let room = (available - extra - used.margin.horizontal()).max(0.);
            max.min(room).max(min).max(0.)
        };
        // size_tracks places the spacing between columns; the two edge gaps are ours.
        let widths = size_tracks(&tracks, &contributions, Some((content - 2. * sx).max(0.)), sx, true, self.scale);
        used.width = used::grid(content, self.scale);
        // Legacy centring (<center>, align=center) centres a table narrower
        // than its container, shrink-to-fit or sized (HTML §15.3.9).
        if let Some(parent) = self.stack.last()
            && self.cache.tree.style(parent.id).is_some_and(|p| matches!(p.value(P::TextAlign), Value::Keyword(K::WebkitCenter | K::WebkitRight)))
        {
            let right = self.cache.tree.style(parent.id).is_some_and(|p| matches!(p.value(P::TextAlign), Value::Keyword(K::WebkitRight)));
            // From the specified margins (auto being zero): the block width
            // equation may have given a sized table's free space to one side.
            let left_margin = used::length(style, P::MarginLeft, Some(available), Some(0.), queries, &mut self.scratch, self.cancel, self.scale)?.unwrap_or(0.);
            let right_margin = used::length(style, P::MarginRight, Some(available), Some(0.), queries, &mut self.scratch, self.cancel, self.scale)?.unwrap_or(0.);
            let free = (available - used.width - extra - left_margin - right_margin).max(0.);
            let start = used::grid(if right { free } else { free / 2. }, self.scale);
            used.margin.left = left_margin + start;
            used.margin.right = right_margin + free - start;
        }
        self.tables.push(Table { id, columns: widths, spacing: (sx, sy) });
        Ok(())
    }
    pub(super) fn finish_table(&mut self, id: BoxId) {
        if self.tables.last().is_some_and(|t| t.id == id) {
            self.tables.pop();
        }
    }
    /// Start a grid or column-flex container for the block whose frame was
    /// just pushed: settle every item's width.
    pub(super) fn begin_container(&mut self, id: BoxId, style: &Style, used: Used, queries: QueryBasis) -> Result<()> {
        let cache = self.cache.clone();
        let b = cache.tree.get(id).ok_or(Error::Invalid)?;
        let width = used.width;
        let height = used.height;
        let gap = |job: &mut Self, p: P| -> Result<f64> {
            if matches!(style.value(p), Value::Keyword(K::Normal)) {
                Ok(0.)
            } else {
                Ok(used::length(style, p, Some(width), Some(0.), queries, &mut job.scratch, job.cancel, job.scale)?.unwrap_or(0.).max(0.))
            }
        };
        let row_gap = gap(self, P::RowGap)?;
        let column_gap = gap(self, P::ColumnGap)?;
        let grid = is_grid(style);
        // Items in order, with their measured contributions.
        let mut items = Vec::new();
        let mut next = b.first_child;
        while let Some(child) = next {
            check(self.cancel)?;
            let c = cache.tree.get(child).ok_or(Error::Invalid)?;
            next = c.next_sibling;
            if c.absolute {
                continue;
            }
            let text = matches!(c.kind, BoxKind::InlineContent { .. });
            let s = if text { style } else { cache.tree.style(child).ok_or(Error::Stale)? };
            let w = self.intrinsic(child, queries, width, 0)?;
            let (margin_x, grow, shrink, basis) = if text {
                (0., 0., 1., None)
            } else {
                let mut m = |p| used::length(s, p, Some(width), Some(0.), queries, &mut self.scratch, self.cancel, self.scale).map(|v| v.unwrap_or(0.));
                let margin_x = m(P::MarginLeft)? + m(P::MarginRight)?;
                let basis = match s.value(P::FlexBasis) {
                    Value::Numeric(_) => used::length(s, P::FlexBasis, height, None, queries, &mut self.scratch, self.cancel, self.scale)?,
                    _ => None,
                };
                (margin_x, s.value(P::FlexGrow).scalar().unwrap_or(0.), s.value(P::FlexShrink).scalar().unwrap_or(1.), basis)
            };
            let justify = match keyword(s, P::JustifySelf) {
                K::Auto => keyword(style, P::JustifyItems),
                k => k,
            };
            let align = match keyword(s, P::AlignSelf) {
                K::Auto => keyword(style, P::AlignItems),
                k => k,
            };
            // A grid item's minimum contribution (CSS Grid §11.5): its
            // min-width when one is given (min-width: 0 contributes
            // nothing), nothing for a scroll container, else its
            // min-content size. GitHub's feature columns (min-width: 0) took
            // their screenshots' widths and overflowed the page.
            let mut minimum = w.min_content;
            if grid && !text {
                if !matches!(s.value(P::MinWidth), Value::Keyword(K::Auto)) {
                    let given = used::length(s, P::MinWidth, None, Some(0.), queries, &mut self.scratch, self.cancel, self.scale)?.unwrap_or(0.);
                    let border_box = matches!(s.value(P::BoxSizing), Value::Keyword(K::BorderBox));
                    minimum = if border_box { given } else { given + self.decoration_of(s, queries, width)? };
                } else if !matches!(s.value(P::OverflowX), Value::Keyword(K::Visible | K::Clip)) {
                    minimum = self.decoration_of(s, queries, width)?;
                }
                minimum = minimum.min(w.min_content);
            }
            if items.len() >= self.limits.boxes {
                return Err(Error::Limit);
            }
            items.push(Item {
                id: child,
                used: Used { width: 0., ..used },
                contribution: (minimum + margin_x, w.max_content + margin_x),
                min_content: w.min_content + margin_x,
                cell: Cell { columns: (0, 1), rows: (0, 1) },
                justify,
                align,
                outer_height: 0.,
                grow,
                shrink,
                basis,
                order: if text { 0 } else { s.value(P::Order).scalar().unwrap_or(0.).clamp(i32::MIN as f64, i32::MAX as f64) as i32 },
            });
        }
        let kind = if is_table_row(style) {
            // A row outside any table (an anonymous table in CSS) sizes its own columns.
            let (columns, spacing) = match self.tables.last() {
                Some(table) => (table.columns.clone(), table.spacing),
                None => {
                    let contributions: Vec<(usize, usize, f64, f64)> = items.iter().enumerate().map(|(i, item)| (i, i + 1, item.contribution.0, item.contribution.1)).collect();
                    let tracks = vec![Track { min: Bound::Auto, max: Bound::Auto }; items.len()];
                    (size_tracks(&tracks, &contributions, Some(width), 0., true, self.scale), (0., 0.))
                }
            };
            let mut at = 0usize;
            for item in &mut items {
                let c = cache.tree.get(item.id).ok_or(Error::Invalid)?;
                let span = usize::from(c.span[0]).max(1);
                item.cell = Cell { columns: (at, (at + span).min(columns.len().max(at + 1))), rows: (0, 1) };
                at += span;
                let (a, z) = item.cell.columns;
                let area = columns[a.min(columns.len())..z.min(columns.len())].iter().sum::<f64>() + spacing.0 * (z - a).saturating_sub(1) as f64;
                let s = cache.tree.style(item.id).ok_or(Error::Stale)?;
                let mut u = Used::flex(s, area, height, queries, &mut self.scratch, self.cancel, self.scale)?;
                u.margin = Edges::default();
                u.width = used::grid((area - u.padding.horizontal() - u.border.horizontal()).max(0.), self.scale);
                u.apply_aspect(s);
                // Cells align vertically by their vertical-align.
                item.align = match s.value(P::VerticalAlign) {
                    Value::Keyword(K::Middle) => K::Center,
                    Value::Keyword(K::Bottom) => K::End,
                    _ => K::Start,
                };
                item.used = u;
            }
            Kind::Row { columns, spacing }
        } else if grid {
            let (mut columns, fit_columns) = self.tracks(style.value(P::GridTemplateColumns), Some(width), column_gap, queries, items.len())?;
            let (explicit_rows, _) = self.tracks(style.value(P::GridTemplateRows), height, row_gap, queries, items.len())?;
            let auto_columns = self.auto_tracks(style.value(P::GridAutoColumns), Some(width), queries)?;
            let auto_rows = self.auto_tracks(style.value(P::GridAutoRows), height, queries)?;
            let flow_column = matches!(keyword(style, P::GridAutoFlow), K::Column | K::ColumnDense);
            let explicit_columns = columns.len();
            for item in &mut items {
                let c = cache.tree.get(item.id).ok_or(Error::Invalid)?;
                if !matches!(c.kind, BoxKind::InlineContent { .. }) {
                    let s = cache.tree.style(item.id).ok_or(Error::Stale)?;
                    item.cell = Cell { columns: lines(s, P::GridColumnStart, P::GridColumnEnd, explicit_columns), rows: lines(s, P::GridRowStart, P::GridRowEnd, explicit_rows.len()) };
                } else {
                    item.cell = Cell { columns: (usize::MAX, 1), rows: (usize::MAX, 1) };
                }
            }
            // Column flow: placed as row flow is, with the axes swapped.
            let (column_count, row_count) = if flow_column {
                for item in &mut items {
                    std::mem::swap(&mut item.cell.columns, &mut item.cell.rows);
                }
                let (rows, columns) = place(&mut items, explicit_rows.len(), explicit_columns, self.cancel)?;
                for item in &mut items {
                    std::mem::swap(&mut item.cell.columns, &mut item.cell.rows);
                }
                (columns, rows)
            } else {
                place(&mut items, explicit_columns, explicit_rows.len(), self.cancel)?
            };
            // auto-fit: repeated tracks no item occupies collapse away, their
            // gaps with them (CSS Grid 2 §7.2.3.2).
            if let Some((first, last)) = fit_columns {
                let mut occupied = vec![false; columns.len()];
                for item in &items {
                    for c in item.cell.columns.0..item.cell.columns.1.min(columns.len()) {
                        occupied[c] = true;
                    }
                }
                let mut map = Vec::with_capacity(columns.len() + 1);
                let mut kept = Vec::with_capacity(columns.len());
                for (i, track) in columns.iter().enumerate() {
                    map.push(kept.len());
                    if !(first..last).contains(&i) || occupied[i] {
                        kept.push(*track);
                    }
                }
                map.push(kept.len());
                if kept.len() < columns.len() {
                    for item in &mut items {
                        let (a, z) = item.cell.columns;
                        let a = map[a.min(columns.len())];
                        let z = map[z.min(columns.len())].max(a + 1);
                        item.cell.columns = (a, z);
                    }
                    columns = kept;
                }
            }
            while columns.len() < column_count.min(columns.len().max(column_count)) {
                let i = columns.len() - explicit_columns;
                columns.push(auto_columns[i % auto_columns.len()]);
            }
            let mut rows: Vec<Track> = explicit_rows.clone();
            while rows.len() < row_count {
                let i = rows.len() - explicit_rows.len();
                rows.push(auto_rows[i % auto_rows.len()]);
            }
            let justify_content = keyword(style, P::JustifyContent);
            // An item in tracks whose minimum is min- or max-content (Apple's
            // `min-content min-content` buttons) contributes its min-content
            // size there, not its `min-width` (CSS Grid §11.5).
            let contributions: Vec<(usize, usize, f64, f64)> = items
                .iter()
                .map(|i| {
                    let (a, z) = i.cell.columns;
                    let content_mins = (a..z.max(a + 1)).all(|c| columns.get(c).is_some_and(|t| matches!(t.min, Bound::MinContent | Bound::MaxContent)));
                    (a, z, if content_mins { i.min_content.max(i.contribution.0) } else { i.contribution.0 }, i.contribution.1)
                })
                .collect();
            let sizes = size_tracks(&columns, &contributions, Some(width), column_gap, matches!(justify_content, K::Normal | K::Stretch), self.scale);
            // Each item's border-box width from its columns and alignment.
            for item in &mut items {
                let (a, z) = item.cell.columns;
                let area = sizes[a..z].iter().sum::<f64>() + column_gap * (z - a).saturating_sub(1) as f64;
                let c = cache.tree.get(item.id).ok_or(Error::Invalid)?;
                let text = matches!(c.kind, BoxKind::InlineContent { .. });
                let s = if text { style } else { cache.tree.style(item.id).ok_or(Error::Stale)? };
                let mut u = if text { Used { width: area, ..used } } else { Used::flex(s, area, height, queries, &mut self.scratch, self.cancel, self.scale)? };
                let specified = if text { None } else { used::length(s, P::Width, Some(area), None, queries, &mut self.scratch, self.cancel, self.scale)? };
                let extra = u.padding.horizontal() + u.border.horizontal() + u.margin.horizontal();
                let stretch = matches!(item.justify, K::Normal | K::Stretch) && specified.is_none();
                u.width = if stretch {
                    (area - extra).max(0.)
                } else if let Some(w) = specified {
                    let border_box = matches!(s.value(P::BoxSizing), Value::Keyword(K::BorderBox));
                    (w - if border_box { u.padding.horizontal() + u.border.horizontal() } else { 0. }).max(0.)
                } else {
                    // fit-content: the available area, within the content's own range.
                    (area - extra).min(item.contribution.1 - u.margin.horizontal()).max(item.contribution.0 - u.margin.horizontal()).min((area - extra).max(0.)).max(0.)
                        - u.padding.horizontal()
                        - u.border.horizontal()
                };
                if !text {
                    u.width = self.clamp_width(s, &u, area, queries)?;
                }
                u.width = used::grid(u.width.max(0.), self.scale);
                // An automatic height from `aspect-ratio` and the settled width
                // (GitHub's feature visuals, size containers with a ratio).
                if !text {
                    u.apply_aspect(s);
                }
                item.used = u;
            }
            Kind::Grid { columns: sizes, rows, align_content: keyword(style, P::AlignContent), justify_content }
        } else {
            if !matches!(style.value(P::FlexWrap), Value::Keyword(K::Nowrap)) {
                self.note("flex-wrap as nowrap", id);
            }
            for item in &mut items {
                let c = cache.tree.get(item.id).ok_or(Error::Invalid)?;
                let text = matches!(c.kind, BoxKind::InlineContent { .. });
                let s = if text { style } else { cache.tree.style(item.id).ok_or(Error::Stale)? };
                let mut u = if text { Used { width, ..used } } else { Used::flex(s, width, height, queries, &mut self.scratch, self.cancel, self.scale)? };
                let specified = if text { None } else { used::length(s, P::Width, Some(width), None, queries, &mut self.scratch, self.cancel, self.scale)? };
                let extra = u.padding.horizontal() + u.border.horizontal() + u.margin.horizontal();
                let stretch = matches!(item.align, K::Normal | K::Stretch) && specified.is_none();
                u.width = if stretch {
                    (width - extra).max(0.)
                } else if let Some(w) = specified {
                    let border_box = matches!(s.value(P::BoxSizing), Value::Keyword(K::BorderBox));
                    (w - if border_box { u.padding.horizontal() + u.border.horizontal() } else { 0. }).max(0.)
                } else {
                    (item.contribution.1 - u.margin.horizontal()).min(width - u.margin.horizontal()).max(item.contribution.0 - u.margin.horizontal()).max(0.)
                        - u.padding.horizontal()
                        - u.border.horizontal()
                };
                if !text {
                    u.width = self.clamp_width(s, &u, width, queries)?;
                }
                u.width = used::grid(u.width.max(0.), self.scale);
                if !text {
                    u.apply_aspect(s);
                }
                item.used = u;
            }
            Kind::ColumnFlex { reverse: matches!(style.value(P::FlexDirection), Value::Keyword(K::ColumnReverse)), justify: keyword(style, P::JustifyContent) }
        };
        self.containers.push(Container { id, kind, items, row_gap, column_gap, width, height, min_height: used.min_height, max_height: used.max_height });
        Ok(())
    }
    /// An item's content-box width within its `min-width` and `max-width`
    /// (against `area`, by its box-sizing): a stretched or percentage-wide
    /// item is held to them as any block is (the BBC's page grid, 100%
    /// wide with a max-width, filled its whole area).
    /// Lay out a column flex container's items in `used_height` (None:
    /// their content decides); returns the content height. Items given a
    /// new height that are column containers themselves follow it.
    fn column_flex(&mut self, column: &Column, used_height: Option<f64>) -> Result<f64> {
        let scale = self.scale;
        let mut resized = Vec::new();
        let n = column.items.len();
        let gaps = column.row_gap * n.saturating_sub(1) as f64;
        // Hypothetical main sizes: a numeric basis replaces the content height.
        let mut sizes: Vec<f64> = column.items.iter().map(|i| i.basis.map_or(i.outer_height, |b| b + i.used.padding.vertical() + i.used.border.vertical() + i.used.margin.vertical())).collect();
        let height = Container::definite_height(used_height, column.min_height, column.max_height, sizes.iter().sum::<f64>() + gaps);
        if let Some(h) = height {
            let free = h - sizes.iter().sum::<f64>() - gaps;
            if free > 0. {
                let total: f64 = column.items.iter().map(|i| i.grow).sum();
                if total > 0. {
                    for (s, i) in sizes.iter_mut().zip(&column.items) {
                        *s += free * i.grow / total;
                    }
                }
            } else if free < 0. {
                let total: f64 = column.items.iter().zip(&sizes).map(|(i, s)| i.shrink * s).sum();
                if total > 0. {
                    for (s, i) in sizes.iter_mut().zip(&column.items) {
                        let scaled = i.shrink * *s;
                        *s = (*s + free * scaled / total).max(i.used.padding.vertical() + i.used.border.vertical() + i.used.margin.vertical());
                    }
                }
            }
        }
        let total = sizes.iter().sum::<f64>() + gaps;
        let free = height.map_or(0., |h| h - total);
        let grown = column.items.iter().any(|i| i.grow > 0.) && free > 0.;
        // Auto margins take the free space before justify-content
        // does (CSS Flexbox 8.1), on the main axis as on the cross.
        let tree = &self.cache.tree;
        let auto = |id: BoxId, p: P| tree.style(id).is_some_and(|s| matches!(tree.get(id).map(|b| &b.kind), Some(BoxKind::Block { .. })) && matches!(s.value(p), Value::Keyword(K::Auto)));
        let autos: Vec<[bool; 4]> = column.items.iter().map(|i| [auto(i.id, P::MarginTop), auto(i.id, P::MarginBottom), auto(i.id, P::MarginLeft), auto(i.id, P::MarginRight)]).collect();
        let auto_count = autos.iter().map(|a| a[0] as usize + a[1] as usize).sum::<usize>();
        let share = if !grown && free > 0. && auto_count > 0 { free / auto_count as f64 } else { 0. };
        let (offset, between) = if grown || share > 0. { (0., 0.) } else { distribute(column.justify, free, n) };
        // Order-modified document order (CSS Flexbox 5.4), reversed
        // for column-reverse.
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|&i| column.items[i].order);
        if column.reverse {
            order.reverse();
        }
        let mut y = offset;
        for (k, &i) in order.iter().enumerate() {
            let item = &column.items[i];
            let g = &mut self.geometry[item.id.index()];
            let outer_w = g.border_box.width + item.used.margin.horizontal();
            let [top, bottom, left, right] = autos[i];
            let dx = match (left, right, item.align) {
                (true, true, _) | (false, false, K::Center) => (column.width - outer_w) / 2.,
                (true, false, _) | (false, false, K::End | K::FlexEnd) => column.width - outer_w,
                _ => 0.,
            };
            y += if top { share } else { 0. };
            g.border_box.x = used::grid(item.used.margin.left + dx.max(0.), scale) + item.used.relative.x;
            g.border_box.y = used::grid(y + item.used.margin.top, scale) + item.used.relative.y;
            y += if bottom { share } else { 0. };
            let inner = sizes[i] - item.used.margin.vertical();
            if (inner - g.border_box.height).abs() > 1e-9 && (item.basis.is_some() || height.is_some()) {
                g.border_box.height = used::grid(inner.max(0.), scale);
                let content = g.border_box.height - item.used.padding.vertical() - item.used.border.vertical();
                resized.push((item.id, content));
            }
            g.margin = item.used.margin;
            y += sizes[i] + column.row_gap + if k + 1 < n { between } else { 0. };
        }
        for (child, content) in resized {
            self.refit_column(child, content)?;
        }
        Ok(total.max(0.))
    }
    /// A finished column container given `content` height by its parent:
    /// its items laid out again in it.
    pub(super) fn refit_column(&mut self, id: BoxId, content: f64) -> Result<()> {
        check(self.cancel)?;
        if let Some(column) = self.columns.remove(&id) {
            self.column_flex(&column, Some(content))?;
            self.columns.insert(id, column);
        }
        Ok(())
    }
    fn clamp_width(&mut self, s: &crate::css::style::Style, u: &Used, area: f64, queries: QueryBasis) -> Result<f64> {
        let decoration = u.padding.horizontal() + u.border.horizontal();
        let border_box = matches!(s.value(P::BoxSizing), Value::Keyword(K::BorderBox));
        let content = |v: f64| (v - if border_box { decoration } else { 0. }).max(0.);
        let maximum = used::length(s, P::MaxWidth, Some(area), None, queries, &mut self.scratch, self.cancel, self.scale)?.map(content);
        let minimum = used::length(s, P::MinWidth, Some(area), Some(0.), queries, &mut self.scratch, self.cancel, self.scale)?.map(content).unwrap_or(0.);
        Ok(u.width.min(maximum.unwrap_or(f64::INFINITY)).max(minimum))
    }
    /// A box's horizontal padding and border.
    fn decoration_of(&mut self, s: &crate::css::style::Style, queries: QueryBasis, width: f64) -> Result<f64> {
        let pad = used::length(s, P::PaddingLeft, Some(width), Some(0.), queries, &mut self.scratch, self.cancel, self.scale)?.unwrap_or(0.)
            + used::length(s, P::PaddingRight, Some(width), Some(0.), queries, &mut self.scratch, self.cancel, self.scale)?.unwrap_or(0.);
        Ok(pad.max(0.) + s.used_border_width(P::BorderLeftWidth)? + s.used_border_width(P::BorderRightWidth)?)
    }
    fn auto_tracks(&mut self, value: &Value, available: Option<f64>, queries: QueryBasis) -> Result<Vec<Track>> {
        let (tracks, _) = self.tracks(value, available, 0., queries, 1)?;
        Ok(if tracks.is_empty() { vec![Track { min: Bound::Auto, max: Bound::Auto }] } else { tracks })
    }
    fn container_item(&self, id: BoxId) -> Option<(usize, usize)> {
        let container = self.containers.last()?;
        let parent = self.stack.last()?;
        if container.id != parent.id {
            return None;
        }
        container.items.iter().position(|i| i.id == id).map(|i| (self.containers.len() - 1, i))
    }
    /// The settled width of an item of the innermost open container.
    pub(super) fn container_child_used(&self, id: BoxId) -> Result<Option<Used>> {
        Ok(self.container_item(id).map(|(c, i)| self.containers[c].items[i].used))
    }
    /// Record an item's laid-out height instead of placing it in flow.
    pub(super) fn place_container(&mut self, id: BoxId) -> Result<bool> {
        let Some((c, i)) = self.container_item(id) else { return Ok(false) };
        let g = self.geometry[id.index()];
        let item = &mut self.containers[c].items[i];
        item.outer_height = g.border_box.height + item.used.margin.vertical();
        Ok(true)
    }
    /// Position the items of the container whose frame is closing; returns
    /// the content height its items need.
    pub(super) fn finish_container(&mut self, id: BoxId) -> Result<Option<f64>> {
        if !self.containers.last().is_some_and(|c| c.id == id) {
            return Ok(None);
        }
        let container = self.containers.pop().ok_or(Error::Invalid)?;
        let scale = self.scale;
        let (used_height, min_height, max_height) = (container.height, container.min_height, container.max_height);
        let definite_height = move |natural: f64| Container::definite_height(used_height, min_height, max_height, natural);
        let content_height = match container.kind {
            Kind::Row { columns, spacing } => {
                let height = container.items.iter().map(|i| i.outer_height).fold(0f64, f64::max);
                let mut starts = Vec::with_capacity(columns.len() + 1);
                let mut x = spacing.0;
                for w in &columns {
                    starts.push(x);
                    x += w + spacing.0;
                }
                starts.push(x);
                for item in &container.items {
                    let (a, z) = item.cell.columns;
                    let g = &mut self.geometry[item.id.index()];
                    let cell_height = used::grid(height.max(g.border_box.height), scale);
                    let dy = match item.align {
                        K::Center => (height - g.border_box.height) / 2.,
                        K::End => height - g.border_box.height,
                        _ => 0.,
                    };
                    g.border_box.x = used::grid(starts[a.min(starts.len() - 1)], scale);
                    g.border_box.y = used::grid(dy.max(0.), scale);
                    let _ = z;
                    // A cell fills its row; content stays where its alignment put it.
                    if !matches!(item.align, K::Center | K::End) {
                        g.border_box.height = cell_height;
                    }
                    g.margin = Edges::default();
                }
                height + spacing.1
            }
            Kind::Grid { columns, rows, align_content, justify_content } => {
                let contributions: Vec<(usize, usize, f64, f64)> = container.items.iter().map(|i| (i.cell.rows.0, i.cell.rows.1, i.outer_height, i.outer_height)).collect();
                let stretch = matches!(align_content, K::Normal | K::Stretch);
                let gaps = |sizes: &[f64]| sizes.iter().sum::<f64>() + container.row_gap * sizes.len().saturating_sub(1) as f64;
                let natural = gaps(&size_tracks(&rows, &contributions, None, container.row_gap, stretch, scale));
                let height = definite_height(natural);
                let row_sizes = size_tracks(&rows, &contributions, height, container.row_gap, stretch, scale);
                let total_rows = gaps(&row_sizes);
                let total_columns = columns.iter().sum::<f64>() + container.column_gap * columns.len().saturating_sub(1) as f64;
                let column_offset = distribute(justify_content, container.width - total_columns, columns.len());
                let row_offset = distribute(align_content, height.map_or(0., |h| h - total_rows), row_sizes.len());
                let mut column_starts = Vec::with_capacity(columns.len() + 1);
                let mut x = column_offset.0;
                for (i, w) in columns.iter().enumerate() {
                    column_starts.push(x);
                    x += w + container.column_gap + if i + 1 < columns.len() { column_offset.1 } else { 0. };
                }
                column_starts.push(x);
                let mut row_starts = Vec::with_capacity(row_sizes.len() + 1);
                let mut y = row_offset.0;
                for (i, h) in row_sizes.iter().enumerate() {
                    row_starts.push(y);
                    y += h + container.row_gap + if i + 1 < row_sizes.len() { row_offset.1 } else { 0. };
                }
                row_starts.push(y);
                let mut stretched = Vec::new();
                for item in &container.items {
                    let (a, z) = item.cell.columns;
                    let (r, q) = item.cell.rows;
                    let area_w = column_starts[z] - column_starts[a] - container.column_gap - column_offset.1;
                    let area_h = row_starts[q] - row_starts[r] - container.row_gap - row_offset.1;
                    let g = &mut self.geometry[item.id.index()];
                    let outer_w = g.border_box.width + item.used.margin.horizontal();
                    let dx = match item.justify {
                        K::Center => (area_w - outer_w) / 2.,
                        K::End | K::FlexEnd | K::Right => area_w - outer_w,
                        _ => 0.,
                    };
                    let stretch_height = matches!(item.align, K::Normal | K::Stretch) && item.used.height.is_none();
                    let outer_h = g.border_box.height + item.used.margin.vertical();
                    let dy = match item.align {
                        K::Center if !stretch_height => (area_h - outer_h) / 2.,
                        K::End | K::FlexEnd if !stretch_height => area_h - outer_h,
                        _ => 0.,
                    };
                    g.border_box.x = used::grid(column_starts[a] + item.used.margin.left + dx.max(0.), scale) + item.used.relative.x;
                    g.border_box.y = used::grid(row_starts[r] + item.used.margin.top + dy.max(0.), scale) + item.used.relative.y;
                    if stretch_height && area_h - item.used.margin.vertical() > g.border_box.height {
                        g.border_box.height = used::grid(area_h - item.used.margin.vertical(), scale);
                        stretched.push((item.id, g.border_box.height - item.used.padding.vertical() - item.used.border.vertical()));
                    }
                    g.margin = item.used.margin;
                }
                // Stretched column containers lay their items out again.
                for (child, content) in stretched {
                    self.refit_column(child, content)?;
                }
                total_rows.max(0.)
            }
            Kind::ColumnFlex { reverse, justify } => {
                let column = Column { items: container.items, row_gap: container.row_gap, width: container.width, reverse, justify, min_height, max_height };
                let total = self.column_flex(&column, used_height)?;
                // A container whose height its content decides can be given
                // another by its parent's flexing or stretching: kept, so
                // its items are laid out again in that height.
                if used_height.is_none() {
                    self.columns.insert(id, column);
                }
                total
            }
        };
        Ok(Some(content_height))
    }
}
/// Leading offset and extra space between items for a content-distribution keyword.
fn distribute(keyword: K, free: f64, count: usize) -> (f64, f64) {
    if free <= 0. || count == 0 {
        return (0., 0.);
    }
    let n = count as f64;
    match keyword {
        K::Center => (free / 2., 0.),
        K::End | K::FlexEnd | K::Right => (free, 0.),
        K::SpaceBetween if count > 1 => (0., free / (n - 1.)),
        K::SpaceAround => (free / (2. * n), free / n),
        K::SpaceEvenly => (free / (n + 1.), free / (n + 1.)),
        _ => (0., 0.),
    }
}
/// Auto-placement (CSS Grid 2 §8.5, sparse, row flow): items with definite
/// positions first, then the rest in order from a cursor that never moves
/// back. Returns the column and row counts the grid ends up with.
fn place(items: &mut [Item], explicit_columns: usize, explicit_rows: usize, cancel: Option<&AtomicBool>) -> Result<(usize, usize)> {
    let columns = explicit_columns.max(1);
    let mut occupied: Vec<(usize, usize, usize, usize)> = Vec::new();
    let free = |occupied: &[(usize, usize, usize, usize)], c0: usize, c1: usize, r0: usize, r1: usize| !occupied.iter().any(|&(a, z, r, q)| c0 < z && a < c1 && r0 < q && r < r1);
    let mut rows = explicit_rows.max(1);
    let mut column_count = columns;
    // Definite rows and columns first.
    let mut auto: Vec<usize> = Vec::new();
    for (i, item) in items.iter_mut().enumerate() {
        let lines = &mut item.cell;
        let (c, r) = (lines.columns, lines.rows);
        let definite_c = c.0 != usize::MAX;
        let definite_r = r.0 != usize::MAX;
        if definite_c && definite_r {
            occupied.push((c.0, c.1, r.0, r.1));
            column_count = column_count.max(c.1);
            rows = rows.max(r.1);
        } else {
            auto.push(i);
        }
    }
    let mut cursor = (0usize, 0usize);
    for i in auto {
        check(cancel)?;
        let item = &mut items[i];
        let definite_c = item.cell.columns.0 != usize::MAX;
        let definite_r = item.cell.rows.0 != usize::MAX;
        let span_c = if definite_c { item.cell.columns.1.saturating_sub(item.cell.columns.0).max(1) } else { item.cell.columns.1.max(1) };
        let span_r = if definite_r { item.cell.rows.1.saturating_sub(item.cell.rows.0).max(1) } else { item.cell.rows.1.max(1) };
        if definite_r {
            // A definite row: the first free column in it, from the start.
            let (r0, r1) = item.cell.rows;
            let mut c0 = 0;
            while !free(&occupied, c0, c0 + span_c, r0, r1) {
                c0 += 1;
                if c0 > 10_000 {
                    return Err(Error::Limit);
                }
            }
            item.cell = Cell { columns: (c0, c0 + span_c), rows: (r0, r1) };
        } else if definite_c {
            let (c0, c1) = item.cell.columns;
            if c0 < cursor.0 {
                cursor.1 += 1;
            }
            let mut r0 = cursor.1;
            while !free(&occupied, c0, c1, r0, r0 + span_r) {
                r0 += 1;
                if r0 > 100_000 {
                    return Err(Error::Limit);
                }
            }
            item.cell = Cell { columns: (c0, c1), rows: (r0, r0 + span_r) };
            cursor = (c0, r0);
        } else {
            let mut guard = 0;
            loop {
                guard += 1;
                if guard > 1_000_000 {
                    return Err(Error::Limit);
                }
                if cursor.0 + span_c > columns.max(span_c) {
                    cursor = (0, cursor.1 + 1);
                    continue;
                }
                if free(&occupied, cursor.0, cursor.0 + span_c, cursor.1, cursor.1 + span_r) {
                    break;
                }
                cursor.0 += 1;
            }
            item.cell = Cell { columns: (cursor.0, cursor.0 + span_c), rows: (cursor.1, cursor.1 + span_r) };
            cursor.0 += span_c;
        }
        let c = item.cell;
        occupied.push((c.columns.0, c.columns.1, c.rows.0, c.rows.1));
        column_count = column_count.max(c.columns.1);
        rows = rows.max(c.rows.1);
    }
    Ok((column_count, rows))
}
/// Track sizing (CSS Grid 2 §12, simplified): intrinsic tracks take their
/// items' contributions (spanning items share theirs equally), free space
/// grows tracks to their limits, flexible tracks share what is left, and
/// auto tracks stretch when content distribution asks for it.
fn size_tracks(tracks: &[Track], items: &[(usize, usize, f64, f64)], available: Option<f64>, gap: f64, stretch: bool, scale: f64) -> Vec<f64> {
    let n = tracks.len();
    let mut base = vec![0f64; n];
    let mut limit = vec![f64::INFINITY; n];
    let mut flex = vec![None; n];
    for (i, t) in tracks.iter().enumerate() {
        if let Bound::Fixed(v) = t.min {
            base[i] = v;
        }
        match t.max {
            Bound::Fixed(v) => limit[i] = v.max(base[i]),
            Bound::Flex(f) => flex[i] = Some(f),
            _ => {}
        }
    }
    // Tracks whose minimum or maximum comes from their items.
    let intrinsic = |t: &Track| !matches!(t.min, Bound::Fixed(_)) || !matches!(t.max, Bound::Fixed(_));
    // Single-span items first, then spanning ones distributing their excess.
    for span in 1..=n {
        for &(a, z, min, max) in items.iter().filter(|(a, z, _, _)| z.saturating_sub(*a).max(1) == span) {
            let z = z.max(a + 1).min(n);
            if a >= n {
                continue;
            }
            // An item spanning a flexible track sizes only the flexible
            // tracks (CSS Grid §11.5, step 4): the auto ones beside them
            // (implicit columns past the explicit grid) stay as their own
            // items make them, 0 px for none.
            let spans_flex = (a..z).any(|i| flex[i].is_some());
            let spanned: Vec<usize> = (a..z).filter(|&i| intrinsic(&tracks[i]) && (!spans_flex || flex[i].is_some())).collect();
            if spanned.is_empty() {
                continue;
            }
            let gaps = gap * (z - a - 1) as f64;
            let fixed_min: f64 = (a..z).map(|i| base[i]).sum::<f64>() + gaps;
            let deficit = (min - fixed_min).max(0.) / spanned.len() as f64;
            for &i in &spanned {
                base[i] += deficit;
            }
            let mut used_max = 0.;
            for i in a..z {
                used_max += if limit[i].is_finite() { limit[i] } else { base[i] };
            }
            let want = match tracks[spanned[0]].max {
                Bound::MinContent => min,
                Bound::FitContent(l) => max.min(l).max(min),
                Bound::Fixed(v) => v,
                _ => max,
            };
            if spans_flex {
                continue;
            }
            let excess = (want - used_max - gaps).max(0.) / spanned.len() as f64;
            for &i in &spanned {
                let current = if limit[i].is_finite() { limit[i] } else { base[i] };
                limit[i] = (current + excess).max(base[i]);
            }
        }
    }
    for i in 0..n {
        if !limit[i].is_finite() {
            limit[i] = base[i];
        }
        if flex[i].is_some() {
            limit[i] = f64::INFINITY;
        }
    }
    let gaps = gap * n.saturating_sub(1) as f64;
    if let Some(available) = available {
        // Maximize tracks: grow those below their limits, equally.
        let mut free = available - base.iter().sum::<f64>() - gaps;
        let mut growable: Vec<usize> = (0..n).filter(|&i| flex[i].is_none() && base[i] < limit[i]).collect();
        while free > 1e-9 && !growable.is_empty() {
            let share = free / growable.len() as f64;
            let mut frozen = Vec::new();
            for &i in &growable {
                let room = limit[i] - base[i];
                let add = share.min(room);
                base[i] += add;
                free -= add;
                if add >= room - 1e-9 {
                    frozen.push(i);
                }
            }
            growable.retain(|i| !frozen.contains(i));
            if frozen.is_empty() {
                break;
            }
        }
        // Flexible tracks share the remaining space by their factors.
        if flex.iter().any(Option::is_some) {
            let mut leftover = available - gaps - (0..n).filter(|&i| flex[i].is_none()).map(|i| base[i]).sum::<f64>();
            let mut active: Vec<usize> = (0..n).filter(|&i| flex[i].is_some()).collect();
            loop {
                let factors: f64 = active.iter().map(|&i| flex[i].unwrap()).sum();
                let fr = if factors > 0. { (leftover / factors.max(1.)).max(0.) } else { 0. };
                let mut changed = false;
                let mut next = Vec::new();
                for &i in &active {
                    let size = flex[i].unwrap() * fr;
                    if size < base[i] {
                        // Its minimum is larger than its share: freeze at the minimum.
                        leftover -= base[i];
                        changed = true;
                    } else {
                        next.push(i);
                    }
                }
                if !changed {
                    for &i in &active {
                        base[i] = base[i].max(flex[i].unwrap() * fr);
                    }
                    break;
                }
                active = next;
                if active.is_empty() {
                    break;
                }
            }
        } else if stretch {
            let free = available - base.iter().sum::<f64>() - gaps;
            let autos: Vec<usize> = (0..n).filter(|&i| matches!(tracks[i].max, Bound::Auto)).collect();
            if free > 0. && !autos.is_empty() {
                for &i in &autos {
                    base[i] += free / autos.len() as f64;
                }
            }
        }
    }
    base.iter().map(|v| used::grid(v.max(0.), scale)).collect()
}
/// Resolve an item's lines from its style into track index ranges, with
/// usize::MAX marking an automatic start.
pub(super) fn lines(style: &Style, start: P, end: P, explicit: usize) -> (usize, usize) {
    use crate::css::properties::GridLine;
    let line = |p: P| -> Option<GridLine> {
        match style.value(p) {
            Value::GridLine(l) => Some(l.clone()),
            _ => None,
        }
    };
    let resolve = |l: &GridLine| -> Option<usize> {
        if l.span || l.name.is_some() && l.number.is_none() {
            return None;
        }
        let n = l.number?;
        Some(if n > 0 { (n - 1) as usize } else { (explicit as i64 + 1 + i64::from(n)).max(0) as usize })
    };
    let (s, e) = (line(start), line(end));
    let span_of = |l: &Option<GridLine>| l.as_ref().filter(|l| l.span).and_then(|l| l.number).map(|n| n.max(1) as usize).unwrap_or(1);
    match (s.as_ref().and_then(resolve), e.as_ref().and_then(resolve)) {
        (Some(a), Some(z)) if z > a => (a, z),
        (Some(a), Some(z)) if z < a => (z, a),
        (Some(a), Some(_)) => (a, a + 1),
        (Some(a), None) => (a, a + span_of(&e)),
        (None, Some(z)) => {
            let span = span_of(&s);
            (z.saturating_sub(span), z.max(1).max(span))
        }
        (None, None) => (usize::MAX, span_of(&s).max(span_of(&e))),
    }
}

/// Each column's max-content width: single-span cells' maxima, then spanning
/// cells' excess shared equally.
fn column_maxes(columns: usize, items: &[(usize, usize, f64, f64)]) -> Vec<f64> {
    let mut maxes = vec![0f64; columns];
    for &(a, z, _, max) in items.iter().filter(|(a, z, _, _)| z.saturating_sub(*a) <= 1) {
        if a < columns {
            maxes[a] = maxes[a].max(max);
        }
        let _ = z;
    }
    for &(a, z, _, max) in items.iter().filter(|(a, z, _, _)| z.saturating_sub(*a) > 1) {
        let z = z.min(columns);
        if a >= z {
            continue;
        }
        let current: f64 = maxes[a..z].iter().sum();
        let deficit = (max - current).max(0.) / (z - a) as f64;
        for m in &mut maxes[a..z] {
            *m += deficit;
        }
    }
    maxes
}
