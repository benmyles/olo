use super::*;
const UNSAFE: u32 = 1 << 31;
/// One compact row per glyph cluster. Ends follow from the next row/run.
/// Prefixes stay in signed integer font units until a line interval is known.
struct Cluster {
    units: i64,
    byte: u32,
    edge: u32,
}
struct RunIndex {
    rows: Range<usize>,
    units: i64,
}
pub(super) struct Index {
    rows: Vec<Cluster>,
    runs: Vec<RunIndex>,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Window {
    pub run: u32,
    pub start: u32,
    pub end: u32,
    pub start_units: i64,
    pub end_units: i64,
}
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct End {
    pub units: i64,
    pub byte: u32,
    pub run: u32,
}
impl Index {
    pub fn new(p: &Paragraph, limits: Limits, cancel: Option<&AtomicBool>) -> Result<Self> {
        let source = p.analysis.source();
        let mut rows = Vec::new();
        let mut runs = Vec::new();
        let mut end = 0;
        for run in &p.runs {
            check(cancel)?;
            if !Arc::ptr_eq(&run.source, source)
                || run.range.start < end
                || run.range.start >= run.range.end
                || run.range.end as usize > source.len()
                || !source.is_char_boundary(run.range.start as usize)
                || !source.is_char_boundary(run.range.end as usize)
                || source[end as usize..run.range.start as usize].bytes().any(|b| b != b'\n')
                || source[run.range.start as usize..run.range.end as usize].contains('\n')
            {
                return Err(Error::Invalid);
            }
            if !matches!(run.direction, Direction::LeftToRight | Direction::RightToLeft) {
                return Err(Error::Unsupported("vertical inline geometry"));
            }
            if run.glyphs.len() >= UNSAFE as usize {
                return Err(Error::Limit);
            }
            let rtl = run.direction == Direction::RightToLeft;
            let first = rows.len();
            let mut at = 0;
            let mut units = 0;
            let logical = |i: usize| if rtl { run.glyphs.len() - 1 - i } else { i };
            if run.glyphs.is_empty() || run.glyphs[logical(0)].cluster != run.range.start {
                if rows.len() >= limits.clusters {
                    return Err(Error::Limit);
                }
                rows.push(Cluster { units: 0, byte: run.range.start, edge: if rtl { run.glyphs.len() as u32 } else { 0 } });
            }
            while at < run.glyphs.len() {
                check(cancel)?;
                let byte = run.glyphs[logical(at)].cluster;
                if byte < run.range.start || byte >= run.range.end || !source.is_char_boundary(byte as usize) || rows.last().is_some_and(|c| rows.len() > first && c.byte >= byte) {
                    return Err(Error::Invalid);
                }
                let edge = if rtl { run.glyphs.len() - at } else { at };
                let prefix = units;
                let mut unsafe_start = false;
                while at < run.glyphs.len() && run.glyphs[logical(at)].cluster == byte {
                    check(cancel)?;
                    let g = run.glyphs[logical(at)];
                    if g.y_advance != 0 {
                        return Err(Error::Unsupported("non-horizontal glyph advances"));
                    }
                    units += i64::from(g.x_advance);
                    unsafe_start |= g.unsafe_to_break;
                    at += 1;
                }
                if rows.len() >= limits.clusters {
                    return Err(Error::Limit);
                }
                rows.push(Cluster { units: prefix, byte, edge: edge as u32 | if unsafe_start { UNSAFE } else { 0 } });
            }
            runs.push(RunIndex { rows: first..rows.len(), units });
            end = run.range.end;
        }
        if source[end as usize..].bytes().any(|b| b != b'\n') {
            return Err(Error::Invalid);
        }
        Ok(Self { rows, runs })
    }
    pub fn payload_bytes(&self) -> usize {
        self.rows.len() * size_of::<Cluster>() + self.runs.len() * size_of::<RunIndex>()
    }
    pub fn boundary(&self, p: &Paragraph, byte: u32) -> Result<(End, Option<Window>)> {
        let source = p.analysis.source();
        if byte as usize > source.len() || !source.is_char_boundary(byte as usize) {
            return Err(Error::Invalid);
        }
        let ri = p.runs.partition_point(|r| r.range.end <= byte);
        if ri == p.runs.len() || byte <= p.runs[ri].range.start {
            return Ok((End { units: 0, byte, run: ri as u32 }, None));
        }
        let info = &self.runs[ri];
        let slice = &self.rows[info.rows.clone()];
        let i = slice.partition_point(|c| c.byte <= byte).saturating_sub(1);
        let c = &slice[i];
        let end = End { units: c.units, byte, run: ri as u32 };
        if byte == c.byte && c.edge & UNSAFE == 0 {
            return Ok((end, None));
        }
        let mut left = i;
        while left > 0 && slice[left].edge & UNSAFE != 0 {
            left -= 1;
        }
        let mut right = i + 1;
        while right < slice.len() && slice[right].edge & UNSAFE != 0 {
            right += 1;
        }
        Ok((
            end,
            Some(Window {
                run: ri as u32,
                start: slice[left].byte,
                end: slice.get(right).map_or(p.runs[ri].range.end, |c| c.byte),
                start_units: slice[left].units,
                end_units: slice.get(right).map_or(info.units, |c| c.units),
            }),
        ))
    }
    pub fn advance(&self, p: &Paragraph, a: End, b: End) -> Result<f64> {
        self.advance_measured(p, a, b, None)
    }
    pub fn advance_measured(&self, p: &Paragraph, a: End, b: End, pixel_scale: Option<f64>) -> Result<f64> {
        if a.run > b.run {
            return Err(Error::Invalid);
        }
        let measure = |i: usize, units: i64| measured(units, p.runs[i].size, p.runs[i].font.units_per_em(), pixel_scale);
        if a.run == b.run {
            let width = if a.run as usize == p.runs.len() { 0. } else { measure(a.run as usize, b.units - a.units)? };
            return if width.is_finite() { Ok(width) } else { Err(Error::Invalid) };
        }
        let mut width = 0.;
        for i in a.run as usize..=b.run as usize {
            if i == p.runs.len() {
                break;
            }
            let before = if i == a.run as usize { a.units } else { 0 };
            let after = if i == b.run as usize { b.units } else { self.runs[i].units };
            width += measure(i, after - before)?;
        }
        if !width.is_finite() {
            return Err(Error::Invalid);
        }
        Ok(width)
    }
    /// Original shaping edge; unlike line ends, this never reshapes a cut.
    pub fn retained_edge(&self, p: &Paragraph, ri: usize, byte: u32) -> Result<usize> {
        let run = &p.runs[ri];
        if byte < run.range.start || byte > run.range.end {
            return Err(Error::Invalid);
        }
        let rtl = run.direction == Direction::RightToLeft;
        if byte == run.range.end {
            return Ok(if rtl { 0 } else { run.glyphs.len() });
        }
        let rows = &self.rows[self.runs[ri].rows.clone()];
        let at = rows.partition_point(|r| r.byte <= byte).saturating_sub(1);
        Ok((rows[at].edge & !UNSAFE) as usize)
    }
    pub fn glyph_slice(&self, p: &Paragraph, ri: usize, range: Range<u32>) -> Result<Option<Range<u32>>> {
        let run = &p.runs[ri];
        let info = &self.runs[ri];
        let slice = &self.rows[info.rows.clone()];
        let edge = |byte: u32| -> Option<u32> {
            if byte == run.range.end {
                return Some(if run.direction == Direction::RightToLeft { 0 } else { run.glyphs.len() as u32 });
            }
            let i = slice.binary_search_by_key(&byte, |c| c.byte).ok()?;
            let c = &slice[i];
            (byte == run.range.start || c.edge & UNSAFE == 0).then_some(c.edge & !UNSAFE)
        };
        let (Some(a), Some(b)) = (edge(range.start), edge(range.end)) else { return Ok(None) };
        Ok(Some(a.min(b)..a.max(b)))
    }
}

/// Linear scalable-font advances use a 16.16 scale over a 26.6 device size.
/// Layout measures then enclose each item on the 1/64 device-pixel grid.
pub(super) fn measured(units: i64, size: f64, upem: u16, pixel_scale: Option<f64>) -> Result<f64> {
    let width = if let Some(scale) = pixel_scale {
        let device_size = (size * scale * 64.).trunc();
        let fixed_scale = (device_size * 65536. / f64::from(upem)).round();
        let fixed_advance = (units as f64 * fixed_scale / 64.).round();
        (fixed_advance / 1024.).ceil() / (64. * scale)
    } else {
        units as f64 * (size / f64::from(upem))
    };
    if width.is_finite() { Ok(width) } else { Err(Error::Invalid) }
}
