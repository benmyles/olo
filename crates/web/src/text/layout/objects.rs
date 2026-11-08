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
    /// An absolutely positioned box's place in the line (no size): a line
    /// holding only these (and collapsible space) is no line box.
    pub placeholder: bool,
    /// `vertical-align: top` or `bottom`: against the line box (its
    /// ascent and descent its height's split, as baseline-aligned).
    pub align: Align,
}
/// Where an object sits in its line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    /// By its baseline (`ascent` above the line's), `middle` included.
    #[default]
    Baseline,
    /// Its top at the line box's top.
    Top,
    /// Its bottom at the line box's bottom.
    Bottom,
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
pub(super) struct Objects {
    rows: Box<[Row]>,
    placed: Vec<ObjectFragment>,
    pub monotonic: bool,
}
pub(super) struct LayoutObjects {
    pub placed: Box<[ObjectFragment]>,
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
        Ok(Self { rows: rows.into_boxed_slice(), placed: Vec::new(), monotonic })
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
    /// Place this line's objects; returns the largest ascent and descent among them.
    pub fn line(&mut self, index: usize, fragments: &[Fragment], options: Options, limits: Limits, cancel: Option<&AtomicBool>) -> Result<(f64, f64)> {
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
                if o.align == Align::Baseline {
                    ascent = ascent.max(o.ascent);
                    descent = descent.max(o.descent);
                }
                self.placed.push(ObjectFragment { at: o.at, line, x: f.x, advance: o.advance, ascent: o.ascent, descent: o.descent });
            }
        }
        if ![ascent, descent, ascent + descent].into_iter().all(f64::is_finite) {
            return Err(Error::Invalid);
        }
        Ok((ascent, descent))
    }
    /// The line box's final extents (`ascent`, `descent`) grown for its
    /// top- and bottom-aligned objects taller than it, and those objects
    /// placed against it (CSS 2.2 §10.8.1).
    pub fn align_line(&mut self, line: usize, ascent: &mut f64, descent: &mut f64) {
        let Ok(line) = u32::try_from(line) else { return };
        let start = self.placed.partition_point(|p| p.line < line);
        for i in start..self.placed.len() {
            if self.placed[i].line != line {
                break;
            }
            let Some(o) = self.at(self.placed[i].at) else { continue };
            let height = o.ascent + o.descent;
            match o.align {
                Align::Baseline => continue,
                Align::Top => *descent = descent.max(height - *ascent),
                Align::Bottom => *ascent = ascent.max(height - *descent),
            }
        }
        for i in start..self.placed.len() {
            if self.placed[i].line != line {
                break;
            }
            let Some(o) = self.at(self.placed[i].at) else { continue };
            let height = o.ascent + o.descent;
            let a = match o.align {
                Align::Baseline => continue,
                Align::Top => *ascent,
                Align::Bottom => height - *descent,
            };
            self.placed[i].ascent = a;
            self.placed[i].descent = height - a;
        }
    }
    /// Whether these fragments are placeholders and collapsible space
    /// only, with a placeholder among them: the line is then zero-height
    /// (CSS 2.2 §9.4.2, no in-flow content).
    pub fn placeholders_only(&self, fragments: &[Fragment], text: &str, collapsible: bool) -> bool {
        let mut any = false;
        for f in fragments {
            match self.at(f.source.start) {
                Some(o) if o.placeholder => any = true,
                Some(_) => return false,
                None => {
                    let t = text.get(f.source.start as usize..f.source.end as usize).unwrap_or("x");
                    if !(collapsible && t.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\u{200b}'))) {
                        return false;
                    }
                }
            }
        }
        any
    }
    pub fn finish(self) -> Box<LayoutObjects> {
        Box::new(LayoutObjects { placed: self.placed.into_boxed_slice() })
    }
}
impl LayoutObjects {
    pub fn payload_bytes(&self) -> usize {
        std::mem::size_of_val(self.placed.as_ref())
    }
}
