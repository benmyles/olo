use super::*;
use crate::text::prepare::{ControlKind, MappingKind};

/// One atomic inline's margin-box advance and extents relative to its baseline.
/// Positions address semantic U+FFFC carriers in prepared text, never DOM text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Object {
    pub at: u32,
    pub advance: f64,
    pub ascent: f64,
    pub descent: f64,
}
/// A placed atomic item. Its vertical origin is the line baseline minus ascent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjectFragment {
    pub at: u32,
    pub line: u32,
    pub x: f64,
    pub advance: f64,
    pub ascent: f64,
    pub descent: f64,
}
struct Row {
    object: Object,
    prefix: f64,
}
struct Metrics {
    line: u32,
    baseline_extra: f64,
    after_extra: f64,
}
pub(super) struct Objects {
    rows: Box<[Row]>,
    placed: Vec<ObjectFragment>,
    metrics: Vec<Metrics>,
    shift: f64,
    pub monotonic: bool,
}
pub(super) struct LayoutObjects {
    pub placed: Box<[ObjectFragment]>,
    metrics: Box<[Metrics]>,
}
impl Objects {
    pub fn new(text: &Text, input: Vec<Object>, limits: Limits, cancel: Option<&AtomicBool>) -> Result<Self> {
        check(cancel)?;
        if input.len() > limits.fragments || input.len() > u32::MAX as usize {
            return Err(Error::Limit);
        }
        if input.len() != text.object_count() {
            return Err(Error::Invalid);
        }
        let mut expected = text.prepared.controls().iter().filter(|c| c.kind() == ControlKind::Object);
        let mut rows = Vec::<Row>::with_capacity(input.len());
        let mut prefix = 0.;
        let mut monotonic = true;
        for object in input {
            check(cancel)?;
            if ![object.advance, object.ascent, object.descent, object.ascent + object.descent].into_iter().all(f64::is_finite) {
                return Err(Error::Invalid);
            }
            let c = expected.next().ok_or(Error::Invalid)?;
            let m = text.prepared.mapping_at_source(c.offset()).ok_or(Error::Invalid)?;
            if m.kind != MappingKind::Copy || object.at != m.output.start + c.offset() - m.input.start {
                return Err(Error::Invalid);
            }
            let end = object.at.checked_add(3).ok_or(Error::Invalid)?;
            if text.prepared.text().get(object.at as usize..end as usize) != Some("\u{fffc}") || rows.last().is_some_and(|r| r.object.at >= object.at) {
                return Err(Error::Invalid);
            }
            let run = text.paragraph.runs.get(text.paragraph.runs.partition_point(|r| r.range.end <= object.at)).ok_or(Error::Stale)?;
            if run.range != (object.at..end) || !run.glyphs.is_empty() {
                return Err(Error::Stale);
            }
            prefix += object.advance;
            if !prefix.is_finite() {
                return Err(Error::Invalid);
            }
            monotonic &= object.advance >= 0.;
            rows.push(Row { object, prefix });
        }
        Ok(Self { rows: rows.into_boxed_slice(), placed: Vec::new(), metrics: Vec::new(), shift: 0., monotonic })
    }
    pub fn at(&self, at: u32) -> Option<Object> {
        self.rows.binary_search_by_key(&at, |r| r.object.at).ok().map(|i| self.rows[i].object)
    }
    pub fn width(&self, from: u32, to: u32) -> f64 {
        let lo = self.rows.partition_point(|r| r.object.at < from);
        let hi = self.rows.partition_point(|r| r.object.at < to);
        let before = |i: usize| i.checked_sub(1).map_or(0., |i| self.rows[i].prefix);
        before(hi) - before(lo)
    }
    pub fn line(&mut self, index: usize, fragments: &[Fragment], options: Options, limits: Limits, cancel: Option<&AtomicBool>) -> Result<()> {
        let line = u32::try_from(index).map_err(|_| Error::Limit)?;
        let mut ascent = options.baseline;
        let mut descent = options.line_height - options.baseline;
        for f in fragments {
            check(cancel)?;
            if let Some(o) = self.at(f.source.start) {
                if f.source.end != o.at + 3 {
                    return Err(Error::Invalid);
                }
                if self.placed.len() >= limits.fragments {
                    return Err(Error::Limit);
                }
                ascent = ascent.max(o.ascent);
                descent = descent.max(o.descent);
                self.placed.push(ObjectFragment { at: o.at, line, x: f.x, advance: o.advance, ascent: o.ascent, descent: o.descent });
            }
        }
        let height = ascent + descent;
        let baseline_extra = self.shift + (ascent - options.baseline);
        self.shift += height - options.line_height;
        if ![height, baseline_extra, self.shift, options.baseline + index as f64 * options.line_height + baseline_extra, (index + 1) as f64 * options.line_height + self.shift]
            .into_iter()
            .all(f64::is_finite)
        {
            return Err(Error::Invalid);
        }
        if ascent != options.baseline || descent != options.line_height - options.baseline {
            self.metrics.push(Metrics { line, baseline_extra, after_extra: self.shift });
        }
        Ok(())
    }
    pub fn finish(self) -> Box<LayoutObjects> {
        Box::new(LayoutObjects { placed: self.placed.into_boxed_slice(), metrics: self.metrics.into_boxed_slice() })
    }
}
impl LayoutObjects {
    pub fn baseline_extra(&self, line: usize) -> f64 {
        let i = self.metrics.partition_point(|r| (r.line as usize) <= line);
        i.checked_sub(1).map_or(0., |i| if self.metrics[i].line as usize == line { self.metrics[i].baseline_extra } else { self.metrics[i].after_extra })
    }
    pub fn height_extra(&self) -> f64 {
        self.metrics.last().map_or(0., |r| r.after_extra)
    }
    pub fn payload_bytes(&self) -> usize {
        std::mem::size_of_val(self.placed.as_ref()) + std::mem::size_of_val(self.metrics.as_ref())
    }
}
