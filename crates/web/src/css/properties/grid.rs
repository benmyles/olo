//! Grid track lists, line names, template areas and grid lines (CSS Grid
//! Layout 2), typed before cascade. Subgrid is not implemented.
use super::*;
/// One bound of a track: minmax()'s parts, or both bounds of a plain size.
#[derive(Clone, Debug)]
pub enum TrackSize {
    Length(Numeric),
    /// A flexible factor, `<number>fr`.
    Fr(f64),
    Auto,
    MinContent,
    MaxContent,
    FitContent(Numeric),
}
impl TrackSize {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Length(a), Self::Length(b)) | (Self::FitContent(a), Self::FitContent(b)) => a.same(b),
            (Self::Fr(a), Self::Fr(b)) => a.to_bits() == b.to_bits(),
            (Self::Auto, Self::Auto) | (Self::MinContent, Self::MinContent) | (Self::MaxContent, Self::MaxContent) => true,
            _ => false,
        }
    }
    fn reserved_bytes(&self) -> usize {
        match self {
            Self::Length(n) | Self::FitContent(n) => n.expression().memory().accounted_reserved_bytes,
            _ => 0,
        }
    }
    fn computed(&self, context: &impl math::Context, limits: math::Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        Ok(match self {
            Self::Length(n) => Self::Length(n.computed(context, true, limits, cancelled)?),
            Self::FitContent(n) => Self::FitContent(n.computed(context, true, limits, cancelled)?),
            other => other.clone(),
        })
    }
}
#[derive(Clone, Debug)]
pub struct Track {
    pub min: TrackSize,
    pub max: TrackSize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoRepeat {
    Fill,
    Fit,
}
#[derive(Clone, Debug)]
pub enum Entry {
    Track(Track),
    /// `repeat(auto-fill | auto-fit, …)`: as many copies as fit the container.
    AutoRepeat(AutoRepeat, Arc<[Track]>),
}
/// Line names: each line's index (counting an auto-repeat's tracks once)
/// and a name it has.
pub type LineNames = Arc<[(u32, Arc<str>)]>;
/// A track list and its line names; `none` is the keyword, never an empty
/// list.
#[derive(Clone, Debug)]
pub struct Tracks(pub Arc<[Entry]>, pub LineNames);
impl Tracks {
    pub(crate) fn same(&self, other: &Self) -> bool {
        self.1 == other.1
            && (Arc::ptr_eq(&self.0, &other.0)
                || self.0.len() == other.0.len()
                    && self.0.iter().zip(other.0.iter()).all(|(a, b)| match (a, b) {
                        (Entry::Track(a), Entry::Track(b)) => a.min.same(&b.min) && a.max.same(&b.max),
                        (Entry::AutoRepeat(ka, ta), Entry::AutoRepeat(kb, tb)) => ka == kb && ta.len() == tb.len() && ta.iter().zip(tb.iter()).all(|(a, b)| a.min.same(&b.min) && a.max.same(&b.max)),
                        _ => false,
                    }))
    }
    pub(crate) fn reserved_bytes(&self) -> usize {
        self.1.iter().map(|(_, n)| n.len() + size_of::<(u32, Arc<str>)>()).sum::<usize>()
            + self.0.len() * size_of::<Entry>()
            + self
                .0
                .iter()
                .map(|e| match e {
                    Entry::Track(t) => t.min.reserved_bytes() + t.max.reserved_bytes(),
                    Entry::AutoRepeat(_, tracks) => tracks.len() * size_of::<Track>() + tracks.iter().map(|t| t.min.reserved_bytes() + t.max.reserved_bytes()).sum::<usize>(),
                })
                .sum::<usize>()
    }
    pub fn computed(&self, context: &impl math::Context, limits: math::Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        let track = |t: &Track| -> Result<Track> { Ok(Track { min: t.min.computed(context, limits, cancelled)?, max: t.max.computed(context, limits, cancelled)? }) };
        let mut out = Vec::with_capacity(self.0.len());
        for e in self.0.iter() {
            check(cancelled)?;
            out.push(match e {
                Entry::Track(t) => Entry::Track(track(t)?),
                Entry::AutoRepeat(kind, tracks) => Entry::AutoRepeat(*kind, tracks.iter().map(track).collect::<Result<Vec<_>>>()?.into()),
            });
        }
        Ok(Self(out.into(), self.1.clone()))
    }
}
/// `grid-template-areas`: the named areas of a grid of `rows` × `columns`
/// cells, each by its lines (row start, column start, row end, column end,
/// from 0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridAreas {
    pub rows: u32,
    pub columns: u32,
    pub areas: Arc<[(Arc<str>, [u32; 4])]>,
}
impl GridAreas {
    pub(crate) fn reserved_bytes(&self) -> usize {
        self.areas.iter().map(|(n, _)| n.len() + size_of::<(Arc<str>, [u32; 4])>()).sum()
    }
}
/// The areas the strings of a `grid-template-areas` (or `grid-template`)
/// name: every row as many cells, each name a filled rectangle.
fn areas(rows: &[&str]) -> Result<GridAreas> {
    let mut grid: Vec<Vec<Option<&str>>> = Vec::with_capacity(rows.len());
    for row in rows {
        // Names, and runs of `.` (a cell no area takes).
        let mut cells = Vec::new();
        let mut chars = row.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if c.is_whitespace() {
                continue;
            }
            let dot = c == '.';
            let mut end = i + c.len_utf8();
            while let Some(&(j, d)) = chars.peek() {
                if d.is_whitespace() || (d == '.') != dot {
                    break;
                }
                end = j + d.len_utf8();
                chars.next();
            }
            cells.push(if dot { None } else { Some(&row[i..end]) });
        }
        if cells.is_empty() || grid.first().is_some_and(|first| first.len() != cells.len()) || cells.len() > MAX_TRACKS {
            return Err(Error::Invalid);
        }
        grid.push(cells);
    }
    if grid.is_empty() || grid.len() > MAX_TRACKS {
        return Err(Error::Invalid);
    }
    let mut found: Vec<(Arc<str>, [u32; 4])> = Vec::new();
    for (r, row) in grid.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            let Some(name) = cell else { continue };
            if found.iter().any(|(n, _)| &**n == *name) {
                continue;
            }
            // Its rectangle: from its first cell, as far right and down as
            // it goes; every cell there its own, and none outside.
            let width = row[c..].iter().take_while(|x| *x == cell).count();
            let height = grid[r..].iter().take_while(|g| g[c] == *cell).count();
            let inside = grid[r..r + height].iter().all(|g| g[c..c + width].iter().all(|x| x == cell));
            let count = grid.iter().flatten().filter(|x| *x == cell).count();
            if !inside || count != width * height {
                return Err(Error::Invalid);
            }
            found.push((Arc::from(*name), [r as u32, c as u32, (r + height) as u32, (c + width) as u32]));
        }
    }
    Ok(GridAreas { rows: grid.len() as u32, columns: grid[0].len() as u32, areas: found.into() })
}
/// `grid-template-areas`: `none` or strings.
pub(super) fn parse_areas(parts: &[Component<'_>]) -> Result<Value> {
    if let [c] = parts
        && is_ident(*c, "none")
    {
        return Ok(Value::Keyword(Keyword::None));
    }
    let rows: Vec<&str> = parts.iter().map(|c| if c.kind() == Kind::String { c.text().ok_or(Error::Invalid) } else { Err(Error::Invalid) }).collect::<Result<_>>()?;
    Ok(Value::GridAreas(areas(&rows)?))
}
/// The names in a `[…]` of a track list.
fn names_in(c: Component<'_>, line: u32, out: &mut Vec<(u32, Arc<str>)>) -> Result<()> {
    for n in c.children().filter(|c| c.kind() != Kind::Whitespace) {
        if n.kind() != Kind::Ident || is_ident(n, "span") || is_ident(n, "auto") {
            return Err(Error::Invalid);
        }
        out.push((line, Arc::from(n.text().unwrap())));
    }
    Ok(())
}
/// A grid line: `auto` is the keyword; an integer, a span, or a name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridLine {
    pub span: bool,
    /// Negative counts from the end; never zero.
    pub number: Option<i32>,
    pub name: Option<Arc<str>>,
}
impl GridLine {
    pub(crate) fn reserved_bytes(&self) -> usize {
        self.name.as_ref().map_or(0, |n| n.len())
    }
}
const MAX_TRACKS: usize = 10_000;
fn is_ident(c: Component<'_>, word: &str) -> bool {
    c.kind() == Kind::Ident && c.text().unwrap().eq_ignore_ascii_case(word)
}
fn size(c: Component<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<TrackSize> {
    if is_ident(c, "auto") {
        return Ok(TrackSize::Auto);
    }
    if is_ident(c, "min-content") {
        return Ok(TrackSize::MinContent);
    }
    if is_ident(c, "max-content") {
        return Ok(TrackSize::MaxContent);
    }
    if matches!(c.kind(), Kind::Dimension | Kind::IntegerDimension) && c.unit().is_some_and(|u| u.eq_ignore_ascii_case("fr")) {
        let n = c.number().ok_or(Error::Invalid)?;
        if !n.value.is_finite() || n.value < 0. {
            return Err(Error::Invalid);
        }
        return Ok(TrackSize::Fr(n.value));
    }
    if c.kind() == Kind::Function && c.text().unwrap().eq_ignore_ascii_case("fit-content") {
        let mut parts = c.children().filter(|c| c.kind() != Kind::Whitespace);
        let part = parts.next().ok_or(Error::Invalid)?;
        if parts.next().is_some() {
            return Err(Error::Invalid);
        }
        return Numeric::compile(part, NumericKind::LengthPercentage, Range::Nonnegative, limits, cancelled).map(TrackSize::FitContent);
    }
    if c.kind() == Kind::Ident {
        return Err(Error::Invalid);
    }
    Numeric::compile(c, NumericKind::LengthPercentage, Range::Nonnegative, limits, cancelled).map(TrackSize::Length)
}
fn track(c: Component<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Track> {
    if c.kind() == Kind::Function && c.text().unwrap().eq_ignore_ascii_case("minmax") {
        let parts: Vec<_> = c.children().filter(|c| c.kind() != Kind::Whitespace).collect();
        let [min, comma, max] = parts.as_slice() else { return Err(Error::Invalid) };
        if comma.kind() != Kind::Comma {
            return Err(Error::Invalid);
        }
        let min = size(*min, limits, cancelled)?;
        if matches!(min, TrackSize::Fr(_)) {
            return Err(Error::Invalid);
        }
        return Ok(Track { min, max: size(*max, limits, cancelled)? });
    }
    let s = size(c, limits, cancelled)?;
    Ok(match s {
        // A flexible size's minimum is auto (CSS Grid 2 §7.2.4).
        TrackSize::Fr(_) => Track { min: TrackSize::Auto, max: s },
        _ => Track { min: s.clone(), max: s },
    })
}
/// Tracks of one `repeat()` body, and its line names (by line, from its
/// first).
fn tracks<'a>(components: impl Iterator<Item = Component<'a>>, out: &mut Vec<Track>, names: &mut Vec<(u32, Arc<str>)>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<()> {
    for c in components.filter(|c| c.kind() != Kind::Whitespace) {
        check(cancelled)?;
        if c.kind() == Kind::Square {
            names_in(c, out.len() as u32, names)?;
            continue;
        }
        if out.len() >= MAX_TRACKS {
            return Err(Error::Limit);
        }
        out.push(track(c, limits, cancelled)?);
    }
    Ok(())
}
/// `<track-list>`, `none`, or `auto` for the auto-track properties.
pub(super) fn parse_tracks<'a>(values: impl Iterator<Item = Component<'a>>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Value> {
    let components: Vec<_> = values.filter(|c| c.kind() != Kind::Whitespace).collect();
    if let [c] = components.as_slice() {
        if is_ident(*c, "none") {
            return Ok(Value::Keyword(Keyword::None));
        }
        if is_ident(*c, "subgrid") || is_ident(*c, "masonry") {
            return Err(Error::Unsupported("subgrid or masonry tracks"));
        }
    }
    if components.is_empty() {
        return Err(Error::Invalid);
    }
    let mut entries = Vec::new();
    let mut names = Vec::new();
    // The line the next track starts at (an auto-repeat's tracks counted once).
    let mut line = 0u32;
    let mut auto_repeats = 0;
    for c in components {
        check(cancelled)?;
        if c.kind() == Kind::Square {
            names_in(c, line, &mut names)?;
            continue;
        }
        if c.kind() == Kind::Function && c.text().unwrap().eq_ignore_ascii_case("repeat") {
            let mut parts = c.children().filter(|c| c.kind() != Kind::Whitespace);
            let count = parts.next().ok_or(Error::Invalid)?;
            if parts.next().is_none_or(|c| c.kind() != Kind::Comma) {
                return Err(Error::Invalid);
            }
            let mut body = Vec::new();
            let mut body_names = Vec::new();
            tracks(c.children().between(count.after(), c.after()).skip_while(|c| c.kind() != Kind::Comma).skip(1), &mut body, &mut body_names, limits, cancelled)?;
            if body.is_empty() {
                return Err(Error::Invalid);
            }
            if is_ident(count, "auto-fill") || is_ident(count, "auto-fit") {
                auto_repeats += 1;
                if auto_repeats > 1 {
                    return Err(Error::Invalid);
                }
                // (Its names, on its first copy's lines.)
                names.extend(body_names.into_iter().map(|(l, n)| (line + l, n)));
                line += body.len() as u32;
                entries.push(Entry::AutoRepeat(if is_ident(count, "auto-fill") { AutoRepeat::Fill } else { AutoRepeat::Fit }, body.into()));
                continue;
            }
            let n = count.number().filter(|n| n.integer && n.value >= 1.).ok_or(Error::Invalid)?.value as usize;
            if entries.len().saturating_add(n.saturating_mul(body.len())) > MAX_TRACKS {
                return Err(Error::Limit);
            }
            for _ in 0..n {
                names.extend(body_names.iter().map(|(l, name)| (line + l, name.clone())));
                entries.extend(body.iter().cloned().map(Entry::Track));
                line += body.len() as u32;
            }
            continue;
        }
        if entries.len() >= MAX_TRACKS {
            return Err(Error::Limit);
        }
        entries.push(Entry::Track(track(c, limits, cancelled)?));
        line += 1;
    }
    if entries.is_empty() {
        return Err(Error::Invalid);
    }
    Ok(Value::Tracks(Tracks(entries.into(), names.into())))
}
/// `auto | <integer> <ident>? | span <integer>? <ident>? | <ident>`.
pub(super) fn parse_line(parts: &[Component<'_>]) -> Result<Value> {
    let mut line = GridLine { span: false, number: None, name: None };
    if parts.len() == 1 && is_ident(parts[0], "auto") {
        return Ok(Value::Keyword(Keyword::Auto));
    }
    if parts.is_empty() || parts.len() > 3 {
        return Err(Error::Invalid);
    }
    for c in parts {
        if is_ident(*c, "span") {
            if line.span {
                return Err(Error::Invalid);
            }
            line.span = true;
        } else if let Some(n) = c.number() {
            if !n.integer || n.value == 0. || line.number.is_some() || n.value.abs() > 1e6 {
                return Err(Error::Invalid);
            }
            line.number = Some(n.value as i32);
        } else if c.kind() == Kind::Ident {
            if line.name.is_some() || is_ident(*c, "auto") {
                return Err(Error::Invalid);
            }
            line.name = Some(Arc::from(c.text().unwrap()));
        } else {
            return Err(Error::Invalid);
        }
    }
    if line.span && line.number.is_some_and(|n| n < 0) || line.span && line.number.is_none() && line.name.is_none() {
        // `span` alone means span 1.
        if line.span && line.number.is_none() && line.name.is_none() {
            line.number = Some(1);
        } else {
            return Err(Error::Invalid);
        }
    }
    Ok(Value::GridLine(line))
}
/// The grid shorthands: `grid-row`, `grid-column` (`start / end`), `grid-area`
/// (`row-start / column-start / row-end / column-end`), `grid-template`
/// and `grid` (`rows / columns` only; the areas forms are not implemented).
pub(super) fn parse_shorthand(shorthand: Shorthand, values: Components<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Assignments> {
    let properties = shorthand.properties();
    let components: Vec<_> = values.clone().filter(|c| c.kind() != Kind::Whitespace).collect();
    let slashes: Vec<usize> = components.iter().enumerate().filter(|(_, c)| c.character() == Some('/')).map(|(i, _)| i).collect();
    match shorthand {
        Shorthand::GridRow | Shorthand::GridColumn | Shorthand::GridArea => {
            let mut groups: Vec<&[Component<'_>]> = Vec::new();
            let mut start = 0;
            for &i in &slashes {
                groups.push(&components[start..i]);
                start = i + 1;
            }
            groups.push(&components[start..]);
            if groups.len() > properties.len() || groups.iter().any(|g| g.is_empty()) {
                return Err(Error::Invalid);
            }
            let parsed: Vec<Value> = groups.iter().map(|g| parse_line(g)).collect::<Result<_>>()?;
            // An omitted end line copies a named start line, else auto.
            let copied = |i: usize| -> Value {
                let from = &parsed[i];
                match from {
                    Value::GridLine(l) if l.name.is_some() && !l.span && l.number.is_none() => from.clone(),
                    _ => Value::Keyword(Keyword::Auto),
                }
            };
            let values: Vec<Value> = match (shorthand, parsed.len()) {
                (Shorthand::GridArea, 1) => vec![parsed[0].clone(), copied(0), copied(0), copied(0)],
                (Shorthand::GridArea, 2) => vec![parsed[0].clone(), parsed[1].clone(), copied(0), copied(1)],
                (Shorthand::GridArea, 3) => vec![parsed[0].clone(), parsed[1].clone(), parsed[2].clone(), copied(1)],
                (Shorthand::GridArea, 4) => parsed,
                (_, 1) => vec![parsed[0].clone(), copied(0)],
                (_, 2) => parsed,
                _ => return Err(Error::Invalid),
            };
            Ok(properties.iter().zip(values).map(|(&property, value)| Assignment { property, value }).collect())
        }
        _ => {
            if components.len() == 1 && is_ident(components[0], "none") {
                return Ok(properties.iter().map(|&property| Assignment { property, value: Value::Wide(variables::Wide::Initial) }).collect());
            }
            // `[names]? "row" <size>? [names]? … [/ <track-list>]?`: the areas
            // with their rows (auto unless sized), and the columns.
            let strings = components.iter().take_while(|c| c.character() != Some('/')).any(|c| c.kind() == Kind::String);
            if strings {
                if slashes.len() > 1 || components.iter().any(|c| is_ident(*c, "auto-flow")) {
                    return Err(Error::Invalid);
                }
                let before: Vec<Component<'_>> = components.iter().take_while(|c| c.character() != Some('/')).copied().collect();
                let mut rows = Vec::new();
                let mut row_tracks = Vec::new();
                let mut names = Vec::new();
                let mut sized = true;
                for c in before {
                    check(cancelled)?;
                    match c.kind() {
                        Kind::String => {
                            if !sized {
                                row_tracks.push(Track { min: TrackSize::Auto, max: TrackSize::Auto });
                            }
                            rows.push(c.text().ok_or(Error::Invalid)?);
                            sized = false;
                        }
                        Kind::Square => names_in(c, if sized { row_tracks.len() } else { row_tracks.len() + 1 } as u32, &mut names)?,
                        _ if !sized => {
                            row_tracks.push(track(c, limits, cancelled)?);
                            sized = true;
                        }
                        _ => return Err(Error::Invalid),
                    }
                }
                if !sized {
                    row_tracks.push(Track { min: TrackSize::Auto, max: TrackSize::Auto });
                }
                let areas = areas(&rows)?;
                let columns = match slashes.as_slice() {
                    [_] => parse_tracks(values.skip_while(|c| c.character() != Some('/')).skip(1), limits, cancelled)?,
                    _ => Value::Keyword(Keyword::None),
                };
                if let Value::Tracks(t) = &columns
                    && t.0.len() != areas.columns as usize
                {
                    return Err(Error::Invalid);
                }
                let rows = Value::Tracks(Tracks(row_tracks.into_iter().map(Entry::Track).collect::<Vec<_>>().into(), names.into()));
                let mut out: Assignments = smallvec::smallvec![
                    Assignment { property: Property::GridTemplateRows, value: rows },
                    Assignment { property: Property::GridTemplateColumns, value: columns },
                    Assignment { property: Property::GridTemplateAreas, value: Value::GridAreas(areas) },
                ];
                for &property in properties.iter().filter(|p| !matches!(p, Property::GridTemplateRows | Property::GridTemplateColumns | Property::GridTemplateAreas)) {
                    out.push(Assignment { property, value: Value::Wide(variables::Wide::Initial) });
                }
                return Ok(out);
            }
            let [slash] = slashes.as_slice() else { return Err(Error::Unsupported("grid shorthand without a slash")) };
            let rows_source = values.clone().take_while(|c| c.character() != Some('/'));
            let columns_source = values.skip_while(|c| c.character() != Some('/')).skip(1);
            if components[slash + 1..].iter().any(|c| is_ident(*c, "auto-flow")) || components[..*slash].iter().any(|c| is_ident(*c, "auto-flow")) {
                return Err(Error::Unsupported("grid auto-flow shorthand"));
            }
            let rows = parse_tracks(rows_source, limits, cancelled)?;
            let columns = parse_tracks(columns_source, limits, cancelled)?;
            let mut out: Assignments = smallvec::smallvec![Assignment { property: Property::GridTemplateRows, value: rows }, Assignment { property: Property::GridTemplateColumns, value: columns }];
            for &property in properties.iter().filter(|p| !matches!(p, Property::GridTemplateRows | Property::GridTemplateColumns)) {
                out.push(Assignment { property, value: Value::Wide(variables::Wide::Initial) });
            }
            Ok(out)
        }
    }
}
