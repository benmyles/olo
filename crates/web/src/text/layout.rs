//! Horizontal text geometry over prepared source and retained glyph runs.
//! Worker-only. This is not block layout, painting or complete CSS Inline.
mod index;
mod spacing;
use super::{
    linebreak::{Breaks, Kind},
    prepare::{Collapse, Prepared, WhiteSpace},
    shaping::{self, Direction, Engine, Paragraph, Run, RunOptions},
};
use index::{End, Index, Window};
use smallvec::SmallVec;
use spacing::Spaces;
pub use spacing::{Side, Spacing};
use std::{
    collections::HashMap,
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
const NONE: u32 = u32::MAX;
const OWNED: u32 = 1 << 31;
#[derive(Debug)]
pub enum Error {
    Invalid,
    Stale,
    Limit,
    Cancelled,
    Incomplete,
    Failed,
    Unsupported(&'static str),
    Shaping(shaping::Error),
}
impl From<shaping::Error> for Error {
    fn from(e: shaping::Error) -> Self {
        match e {
            shaping::Error::Cancelled => Self::Cancelled,
            shaping::Error::Limit => Self::Limit,
            shaping::Error::Stale => Self::Stale,
            shaping::Error::Unsupported(s) => Self::Unsupported(s),
            e => Self::Shaping(e),
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "text layout {self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
fn check(c: Option<&AtomicBool>) -> Result<()> {
    if c.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub text_bytes: usize,
    pub clusters: usize,
    pub points: usize,
    pub lines: usize,
    pub fragments: usize,
    pub cached_runs: usize,
    pub cached_glyphs: usize,
    pub shape_bytes: usize,
    pub shape_glyphs: usize,
    pub work: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            text_bytes: 65536,
            clusters: 131072,
            points: 131072,
            lines: 65536,
            fragments: 131072,
            cached_runs: 512,
            cached_glyphs: 32768,
            shape_bytes: 4 * 1024 * 1024,
            shape_glyphs: 4 * 1024 * 1024,
            work: 4 * 1024 * 1024,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    End,
    Left,
    Right,
    Center,
}
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub width: f64,
    pub line_height: f64,
    pub baseline: f64,
    pub align: Align,
    pub rtl: bool,
    pub last_align: Option<Align>,
}
impl Default for Options {
    fn default() -> Self {
        Self { width: 800., line_height: 20., baseline: 16., align: Align::Start, rtl: false, last_align: None }
    }
}
impl Options {
    pub fn for_style(style: &crate::css::style::Style, scope: &crate::font::Scope, width: f64, cancel: Option<&AtomicBool>) -> Result<Self> {
        check(cancel)?;
        use crate::css::style::{Keyword as K, Property as P, Value as V, WritingMode};
        if style.font().context() != scope.context() {
            return Err(Error::Stale);
        }
        if style.axes().writing_mode != WritingMode::HorizontalTb {
            return Err(Error::Unsupported("vertical inline geometry"));
        }
        for property in [P::WordBreak, P::OverflowWrap] {
            if !matches!(style.value(property), V::Keyword(K::Normal)) {
                return Err(Error::Unsupported("CSS break tailoring"));
            }
        }
        for property in [P::LetterSpacing, P::WordSpacing] {
            if style.used_text_spacing(property, style.font().request.size, Default::default(), cancel).map_err(|e| match e {
                crate::css::style::Error::Cancelled => Error::Cancelled,
                _ => Error::Unsupported("text spacing context"),
            })? != 0.
            {
                return Err(Error::Unsupported("nonzero text spacing"));
            }
        }
        let align = |v: &V| -> Result<Align> {
            match v {
                V::Keyword(k) => match k {
                    K::Start => Ok(Align::Start),
                    K::End => Ok(Align::End),
                    K::Left => Ok(Align::Left),
                    K::Right => Ok(Align::Right),
                    K::Center => Ok(Align::Center),
                    _ => Err(Error::Unsupported("text alignment/justification")),
                },
                _ => Err(Error::Invalid),
            }
        };
        let last_align = if matches!(style.value(P::TextAlignLast), V::Keyword(K::Auto)) { None } else { Some(align(style.value(P::TextAlignLast))?) };
        let metrics = scope.metrics(&style.font().request, cancel).map_err(shaping::Error::from)?;
        if style.font().context() != scope.context() {
            return Err(Error::Stale);
        }
        Ok(Self {
            width,
            line_height: style.line_height(),
            baseline: metrics.ascent + (style.line_height() - (metrics.ascent - metrics.descent)) / 2.,
            align: align(style.value(P::TextAlign))?,
            rtl: style.axes().direction == crate::css::style::Direction::Rtl,
            last_align,
        })
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    pub points: usize,
    pub lines: usize,
    pub fragments: usize,
    pub reshaped_glyphs: usize,
    pub work: usize,
    pub finished: bool,
}
#[derive(Clone, Copy)]
struct Point {
    end: End,
    offset: u32,
    hang: u32,
    kind: Kind,
}
type ShapeKey = (u32, u32, u32, bool);
#[derive(Default)]
struct Memo {
    runs: HashMap<ShapeKey, Arc<Run>>,
    glyphs: usize,
    shape_bytes: usize,
    shape_glyphs: usize,
}
impl Memo {
    #[allow(clippy::too_many_arguments)] // The worker context and optional parent cache are independent.
    fn shape(&mut self, engine: &mut Engine, p: &Paragraph, run: u32, range: Range<u32>, rtl: bool, limits: Limits, cancel: Option<&AtomicBool>, parent: Option<&Memo>) -> Result<Arc<Run>> {
        check(cancel)?;
        let key = (run, range.start, range.end, rtl);
        if let Some(r) = self.runs.get(&key).or_else(|| parent.and_then(|m| m.runs.get(&key))) {
            return Ok(r.clone());
        }
        self.shape_bytes = self.shape_bytes.checked_add(range.len()).ok_or(Error::Limit)?;
        if self.shape_bytes > limits.shape_bytes {
            return Err(Error::Limit);
        }
        let base = p.runs.get(run as usize).ok_or(Error::Invalid)?;
        let context = p.context_range(range.start as usize..range.end as usize)?;
        let shaped = Arc::new(engine.shape_run(
            base.font.selection(),
            p.analysis.source().clone(),
            range.start as usize..range.end as usize,
            RunOptions {
                direction: if rtl { Direction::RightToLeft } else { Direction::LeftToRight },
                script: base.script,
                language: Some(&p.language),
                features: &[],
                beginning_of_text: range.start as usize == context.start,
                end_of_text: range.end as usize == context.end,
                context: Some((context.start, context.end)),
            },
            cancel,
        )?);
        self.shape_glyphs = self.shape_glyphs.checked_add(shaped.glyphs.len()).ok_or(Error::Limit)?;
        if self.shape_glyphs > limits.shape_glyphs {
            return Err(Error::Limit);
        }
        if self.runs.len() < limits.cached_runs && self.glyphs.saturating_add(shaped.glyphs.len()) <= limits.cached_glyphs {
            self.glyphs += shaped.glyphs.len();
            self.runs.insert(key, shaped.clone());
        }
        Ok(shaped)
    }
}
fn units(run: &Run, range: Range<u32>) -> i64 {
    run.glyphs[range.start as usize..range.end as usize].iter().map(|g| i64::from(g.x_advance)).sum()
}
fn all_units(run: &Run) -> i64 {
    units(run, 0..run.glyphs.len() as u32)
}
fn bidi(c: char) -> bool {
    matches!(c,'\u{61c}'|'\u{200e}'|'\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')
}
fn other_space(c: char) -> bool {
    matches!(c, '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}
fn collapses(w: WhiteSpace) -> bool {
    matches!(w.collapse, Collapse::Collapse | Collapse::PreserveBreaks)
}
/// Width-independent retained data. Reuse across width/alignment changes.
pub struct Text {
    prepared: Arc<Prepared>,
    paragraph: Arc<Paragraph>,
    index: Index,
    points: Box<[Point]>,
    hanging: Box<[End]>,
    memo: Memo,
    whitespace: WhiteSpace,
    monotonic: bool,
    complex: usize,
}
impl Text {
    pub fn prepared(&self) -> &Arc<Prepared> {
        &self.prepared
    }
    pub fn paragraph(&self) -> &Arc<Paragraph> {
        &self.paragraph
    }
    pub fn complex_context_scalars(&self) -> usize {
        self.complex
    }
    /// Index/point/extra-glyph payload; excludes shared source/fonts/base glyphs,
    /// collection capacity, headers and hash table overhead.
    pub fn geometry_payload_bytes(&self) -> usize {
        self.index.payload_bytes()
            + self.points.len() * size_of::<Point>()
            + self.hanging.len() * size_of::<End>()
            + self.memo.glyphs * size_of::<shaping::Glyph>()
            + std::mem::size_of_val(self.paragraph.shaping_breaks())
    }
}
/// Prepare cached interval measures. Initial scope: one font request/size and
/// uniform whitespace settings, with font/script/bidi fallback runs. LF works;
/// tabs, mixed inline-box policies and synthesized Cc glyphs remain explicit.
pub struct TextJob<'a> {
    engine: &'a mut Engine,
    prepared: Arc<Prepared>,
    paragraph: Arc<Paragraph>,
    breaks: Arc<Breaks>,
    index: Index,
    limits: Limits,
    cancel: Option<&'a AtomicBool>,
    whitespace: WhiteSpace,
    next: usize,
    at: usize,
    trailing: Option<u32>,
    lf_trailing: Option<u32>,
    last_lf: Option<u32>,
    points: Vec<Point>,
    hanging: Vec<End>,
    memo: Memo,
    previous_end: End,
    monotonic: bool,
    progress: Progress,
    failed: bool,
}
impl<'a> TextJob<'a> {
    pub fn new(engine: &'a mut Engine, prepared: Arc<Prepared>, paragraph: Arc<Paragraph>, breaks: Arc<Breaks>, limits: Limits, cancel: Option<&'a AtomicBool>) -> Result<Self> {
        check(cancel)?;
        let source = prepared.text();
        if !Arc::ptr_eq(source, paragraph.analysis.source()) || !Arc::ptr_eq(source, breaks.source()) {
            return Err(Error::Stale);
        }
        if source.len() > limits.text_bytes || paragraph.runs.len() >= OWNED as usize {
            return Err(Error::Limit);
        }
        if !paragraph.font_size.is_finite() || paragraph.font_size < 0. || paragraph.font_size != paragraph.request().size || paragraph.runs.iter().any(|r| r.font.units_per_em() == 0) {
            return Err(Error::Invalid);
        }
        let whitespace = prepared.spans().first().map_or(WhiteSpace::default(), |s| s.whitespace);
        if prepared.spans().iter().any(|s| s.whitespace != whitespace) {
            return Err(Error::Unsupported("mixed inline whitespace needs box context"));
        }
        if source.chars().any(|c| c.is_control() && c != '\n') {
            return Err(Error::Unsupported("tabs and visible control glyph layout"));
        }
        let index = Index::new(&paragraph, limits, cancel)?;
        Ok(Self {
            engine,
            prepared,
            paragraph,
            breaks,
            index,
            limits,
            cancel,
            whitespace,
            next: 0,
            at: 0,
            trailing: None,
            lf_trailing: None,
            last_lf: None,
            points: Vec::new(),
            hanging: Vec::new(),
            memo: Memo::default(),
            previous_end: End::default(),
            monotonic: true,
            progress: Progress::default(),
            failed: false,
        })
    }
    pub fn progress(&self) -> Progress {
        self.progress
    }
    pub fn step(&mut self, budget: usize) -> Result<Progress> {
        if self.failed {
            return Err(Error::Failed);
        }
        let result = self.advance(budget);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn end(&mut self, byte: u32) -> Result<End> {
        let (mut end, window) = self.index.boundary(&self.paragraph, byte)?;
        if let Some(w) = window {
            let rtl = self.paragraph.runs[w.run as usize].direction == Direction::RightToLeft;
            let run = self.memo.shape(self.engine, &self.paragraph, w.run, w.start..byte, rtl, self.limits, self.cancel, None)?;
            end.units = w.start_units + all_units(&run);
        }
        Ok(end)
    }
    fn advance(&mut self, budget: usize) -> Result<Progress> {
        check(self.cancel)?;
        if self.progress.finished {
            return Ok(self.progress);
        }
        for _ in 0..budget {
            check(self.cancel)?;
            let Some(mut next) = self.breaks.get(self.next) else {
                self.progress.finished = true;
                break;
            };
            if !self.whitespace.wrap && next.kind == Kind::Allowed {
                self.next += 1;
                self.progress.work += 1;
                if self.progress.work > self.limits.work {
                    return Err(Error::Limit);
                }
                continue;
            }
            let source = self.prepared.text().clone();
            let mut offset = next.offset;
            while self.at < next.offset as usize {
                check(self.cancel)?;
                let c = source[self.at..].chars().next().unwrap();
                let from = self.at as u32;
                self.at += c.len_utf8();
                if c == '\n' {
                    self.last_lf = Some(from);
                    self.lf_trailing = self.trailing;
                    self.trailing = None;
                } else {
                    self.last_lf = None;
                    if c == ' ' {
                        self.trailing.get_or_insert(from);
                    } else if !bidi(c) {
                        self.trailing = None;
                    }
                }
                if self.whitespace.wrap && self.whitespace.collapse == Collapse::BreakSpaces && (c == ' ' || other_space(c)) && self.at < next.offset as usize {
                    offset = self.at as u32;
                    next.kind = Kind::Allowed;
                    break;
                }
            }
            if offset == next.offset {
                self.next += 1;
            }
            let (paint_end, trailing) = if self.last_lf == Some(offset.saturating_sub(1)) { (offset - 1, self.lf_trailing) } else { (offset, self.trailing) };
            let end_byte = if collapses(self.whitespace) { trailing.unwrap_or(paint_end) } else { paint_end };
            let end = self.end(end_byte)?;
            let mut hang = NONE;
            if !collapses(self.whitespace)
                && self.whitespace.wrap
                && self.whitespace.collapse != Collapse::BreakSpaces
                && let Some(byte) = trailing
            {
                let alt = self.end(byte)?;
                hang = self.hanging.len() as u32;
                self.hanging.push(alt);
            }
            if self.points.len() >= self.limits.points {
                return Err(Error::Limit);
            }
            let effective = if hang == NONE { end } else { self.hanging[hang as usize] };
            let delta = self.index.advance(&self.paragraph, self.previous_end, effective)?;
            self.monotonic &= delta >= 0.;
            self.previous_end = effective;
            self.points.push(Point { end, offset, hang, kind: next.kind });
            self.progress.points += 1;
            self.progress.work += 1;
            self.progress.reshaped_glyphs = self.memo.shape_glyphs;
            if self.progress.work > self.limits.work {
                return Err(Error::Limit);
            }
            if offset as usize == source.len() {
                self.progress.finished = true;
                break;
            }
        }
        Ok(self.progress)
    }
    pub fn finish(self) -> Result<Arc<Text>> {
        if self.failed {
            return Err(Error::Failed);
        }
        check(self.cancel)?;
        if !self.progress.finished {
            return Err(Error::Incomplete);
        }
        Ok(Arc::new(Text {
            prepared: self.prepared,
            paragraph: self.paragraph,
            index: self.index,
            points: self.points.into_boxed_slice(),
            hanging: self.hanging.into_boxed_slice(),
            memo: self.memo,
            whitespace: self.whitespace,
            monotonic: self.monotonic,
            complex: self.breaks.complex_context_scalars(),
        }))
    }
}
#[derive(Clone, Debug)]
pub struct Fragment {
    pub source: Range<u32>,
    pub glyphs: Range<u32>,
    pub x: f64,
    pub advance: f64,
    pub level: u8,
    run: u32,
}
#[derive(Clone, Debug)]
pub struct Line {
    pub source: Range<u32>,
    pub text: Range<u32>,
    pub fragments: Range<u32>,
    pub advance: f64,
    pub content_advance: f64,
    pub offset: f64,
    pub overflow: bool,
    pub end: Kind,
}
pub struct Layout {
    text: Arc<Text>,
    lines: Box<[Line]>,
    fragments: Box<[Fragment]>,
    owned: Box<[Arc<Run>]>,
    options: Options,
}
impl Layout {
    pub fn text(&self) -> &Arc<Text> {
        &self.text
    }
    pub fn lines(&self) -> &[Line] {
        &self.lines
    }
    pub fn fragments(&self) -> &[Fragment] {
        &self.fragments
    }
    pub fn run(&self, fragment: &Fragment) -> Option<&Run> {
        if fragment.run & OWNED == 0 { self.text.paragraph.runs.get(fragment.run as usize) } else { self.owned.get((fragment.run & !OWNED) as usize).map(Arc::as_ref) }
    }
    pub fn baseline(&self, line: usize) -> Option<f64> {
        (line < self.lines.len()).then_some(self.options.baseline + line as f64 * self.options.line_height)
    }
    pub fn height(&self) -> f64 {
        self.lines.len() as f64 * self.options.line_height
    }
    pub fn geometry_payload_bytes(&self) -> usize {
        self.lines.len() * size_of::<Line>() + self.fragments.len() * size_of::<Fragment>() + self.owned.iter().map(|r| r.glyph_payload_bytes()).sum::<usize>()
    }
}
/// Greedy line selection and visual fragments. Glyph slices at certified safe
/// cuts share the original paragraph; only unsafe edge windows are reshaped.
pub struct Job<'a> {
    engine: &'a mut Engine,
    text: Arc<Text>,
    options: Options,
    spacing: Spaces,
    spacing_start: u32,
    limits: Limits,
    cancel: Option<&'a AtomicBool>,
    memo: Memo,
    from: u32,
    paint_start: u32,
    start: End,
    start_window: Option<Window>,
    scan: usize,
    first: usize,
    best: Option<usize>,
    lines: Vec<Line>,
    fragments: Vec<Fragment>,
    owned: Vec<Arc<Run>>,
    progress: Progress,
    failed: bool,
}
impl<'a> Job<'a> {
    pub fn for_style(
        engine: &'a mut Engine,
        text: Arc<Text>,
        style: &crate::css::style::Style,
        scope: &crate::font::Scope,
        width: f64,
        limits: Limits,
        cancel: Option<&'a AtomicBool>,
    ) -> Result<Self> {
        Self::for_style_with_spacing(engine, text, style, scope, width, limits, cancel, Vec::new())
    }
    #[allow(clippy::too_many_arguments)] // Retained text, style/font context and width-dependent edges are explicit worker inputs.
    pub fn for_style_with_spacing(
        engine: &'a mut Engine,
        text: Arc<Text>,
        style: &crate::css::style::Style,
        scope: &crate::font::Scope,
        width: f64,
        limits: Limits,
        cancel: Option<&'a AtomicBool>,
        spacing: Vec<Spacing>,
    ) -> Result<Self> {
        if text.paragraph.font_context() != style.font().context()
            || text.paragraph.request() != &style.font().request
            || text.whitespace != WhiteSpace::for_style(style).map_err(|_| Error::Invalid)?
        {
            return Err(Error::Stale);
        }
        let options = Options::for_style(style, scope, width, cancel)?;
        if text.paragraph.analysis.paragraphs().any(|(_, level)| (!level.is_multiple_of(2)) != options.rtl) {
            return Err(Error::Stale);
        }
        Self::with_spacing(engine, text, options, limits, cancel, spacing)
    }
    pub fn new(engine: &'a mut Engine, text: Arc<Text>, options: Options, limits: Limits, cancel: Option<&'a AtomicBool>) -> Result<Self> {
        Self::with_spacing(engine, text, options, limits, cancel, Vec::new())
    }
    pub fn with_spacing(engine: &'a mut Engine, text: Arc<Text>, options: Options, limits: Limits, cancel: Option<&'a AtomicBool>, spacing: Vec<Spacing>) -> Result<Self> {
        check(cancel)?;
        if ![options.width, options.line_height, options.baseline].into_iter().all(f64::is_finite) || options.width < 0. || options.line_height < 0. {
            return Err(Error::Invalid);
        }
        if text.points.len() > limits.points || text.prepared.text().len() > limits.text_bytes {
            return Err(Error::Limit);
        }
        if spacing.iter().any(|s| s.at > 0 && (s.at as usize) < text.prepared.text().len() && text.paragraph.shaping_breaks().binary_search(&s.at).is_err()) {
            return Err(Error::Stale);
        }
        let spacing = Spaces::new(spacing, text.prepared.text(), limits.fragments, cancel)?;
        if !spacing.is_empty() && (options.rtl || text.paragraph.runs.iter().any(|r| r.direction != Direction::LeftToRight)) {
            return Err(Error::Unsupported("bidi inline box spacing"));
        }
        let mut job = Self {
            engine,
            text,
            options,
            spacing,
            spacing_start: 0,
            limits,
            cancel,
            memo: Memo::default(),
            from: 0,
            paint_start: 0,
            start: End::default(),
            start_window: None,
            scan: 0,
            first: 0,
            best: None,
            lines: Vec::new(),
            fragments: Vec::new(),
            owned: Vec::new(),
            progress: Progress::default(),
            failed: false,
        };
        job.begin()?;
        Ok(job)
    }
    pub fn progress(&self) -> Progress {
        self.progress
    }
    pub fn completed_lines(&self) -> Result<&[Line]> {
        if self.failed { Err(Error::Failed) } else { Ok(&self.lines) }
    }
    fn shape(&mut self, run: u32, range: Range<u32>, rtl: bool) -> Result<Arc<Run>> {
        self.memo.shape(self.engine, &self.text.paragraph, run, range, rtl, self.limits, self.cancel, Some(&self.text.memo))
    }
    fn begin(&mut self) -> Result<()> {
        let source = self.text.prepared.text();
        let mut at = self.from as usize;
        let mut trimmed = at;
        if collapses(self.text.whitespace) {
            for c in source[at..].chars() {
                check(self.cancel)?;
                at += c.len_utf8();
                if c == ' ' {
                    trimmed = at;
                } else if !bidi(c) {
                    break;
                }
            }
        }
        self.paint_start = trimmed as u32;
        if trimmed == source.len() && !self.spacing.contains(self.spacing_start, self.from, source.len() as u32) {
            self.progress.finished = true;
            return Ok(());
        }
        if source.is_empty() {
            if self.limits.lines == 0 || self.limits.work == 0 {
                return Err(Error::Limit);
            }
            let advance = self.spacing.width(0, 0, 0, 0, true);
            let offset = self.aligned_offset(advance, Kind::Required);
            if !advance.is_finite() || !offset.is_finite() {
                return Err(Error::Invalid);
            }
            self.lines.push(Line { source: 0..0, text: 0..0, fragments: 0..0, advance, content_advance: advance, offset, overflow: advance > self.options.width, end: Kind::Required });
            self.progress.lines = 1;
            self.progress.work = 1;
            self.progress.finished = true;
            return Ok(());
        }
        let (mut start, window) = self.text.index.boundary(&self.text.paragraph, self.paint_start)?;
        if let Some(w) = window {
            let rtl = self.text.paragraph.runs[w.run as usize].direction == Direction::RightToLeft;
            let run = self.shape(w.run, self.paint_start..w.end, rtl)?;
            start.units = w.end_units - all_units(&run);
        }
        self.start = start;
        self.start_window = window;
        self.first = self.scan;
        self.best = None;
        Ok(())
    }
    fn width(&mut self, end: End) -> Result<f64> {
        if end.byte <= self.paint_start {
            return Ok(0.);
        }
        if let Some(w) = self.start_window
            && w.run == end.run
            && end.byte < w.end
        {
            let rtl = self.text.paragraph.runs[w.run as usize].direction == Direction::RightToLeft;
            let run = self.shape(w.run, self.paint_start..end.byte, rtl)?;
            return Ok(all_units(&run) as f64 * (self.text.paragraph.font_size / f64::from(run.font.units_per_em())));
        }
        self.text.index.advance(&self.text.paragraph, self.start, end)
    }
    fn point_width(&mut self, p: Point) -> Result<(f64, End)> {
        let space = self.spacing.width(self.spacing_start, self.from, p.end.byte, p.offset, p.offset as usize == self.text.prepared.text().len());
        let full = self.width(p.end)? + space;
        if p.hang != NONE && (p.kind == Kind::Allowed || full > self.options.width) {
            let end = self.text.hanging[p.hang as usize];
            return Ok((self.width(end)? + space, end));
        }
        Ok((full, p.end))
    }
    pub fn step(&mut self, budget: usize) -> Result<Progress> {
        if self.failed {
            return Err(Error::Failed);
        }
        let result = self.advance(budget);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance(&mut self, budget: usize) -> Result<Progress> {
        check(self.cancel)?;
        if self.progress.finished {
            return Ok(self.progress);
        }
        for _ in 0..budget {
            check(self.cancel)?;
            let p = *self.text.points.get(self.scan).ok_or(Error::Invalid)?;
            let current = self.scan;
            self.scan += 1;
            self.progress.work += 1;
            if self.progress.work > self.limits.work {
                return Err(Error::Limit);
            }
            if (p.offset <= self.paint_start || p.end.byte < self.paint_start) && !self.spacing.contains(self.spacing_start, self.from, p.offset) {
                self.first = self.scan;
                continue;
            }
            let (width, effective) = self.point_width(p)?;
            if width <= self.options.width {
                self.best = Some(current);
            }
            let monotonic = self.text.monotonic && self.start_window.is_none_or(|w| effective.byte >= w.end);
            if p.kind == Kind::Required || (width > self.options.width && monotonic) {
                let chosen = self.best.unwrap_or(self.first.min(current));
                self.emit_line(chosen)?;
                self.from = self.text.points[chosen].offset;
                self.spacing_start = self.text.points[chosen].end.byte;
                self.scan = chosen + 1;
                if self.from as usize == self.text.prepared.text().len() {
                    self.progress.finished = true;
                    break;
                }
                self.begin()?;
                if self.progress.finished {
                    break;
                }
            }
        }
        self.progress.lines = self.lines.len();
        self.progress.fragments = self.fragments.len();
        self.progress.reshaped_glyphs = self.memo.shape_glyphs;
        Ok(self.progress)
    }
    fn append_part(&mut self, ri: usize, source: Range<u32>, level: u8, totals: &mut SmallVec<[(usize, i64); 8]>) -> Result<()> {
        check(self.cancel)?;
        if self.fragments.len() >= self.limits.fragments {
            return Err(Error::Limit);
        }
        let base = &self.text.paragraph.runs[ri];
        let rtl = !level.is_multiple_of(2);
        let slice =
            if !cfg!(feature = "baseline-inline-reshape") && (base.direction == Direction::RightToLeft) == rtl { self.text.index.glyph_slice(&self.text.paragraph, ri, source.clone())? } else { None };
        let (run, glyphs, count) = if let Some(slice) = slice {
            (ri as u32, slice.clone(), units(base, slice))
        } else {
            let shaped = self.shape(ri as u32, source.clone(), rtl)?;
            if self.owned.len() >= OWNED as usize {
                return Err(Error::Limit);
            }
            let count = all_units(&shaped);
            let glyphs = 0..shaped.glyphs.len() as u32;
            let id = OWNED | self.owned.len() as u32;
            self.owned.push(shaped);
            (id, glyphs, count)
        };
        let advance = count as f64 * (self.text.paragraph.font_size / f64::from(self.text.paragraph.runs[ri].font.units_per_em()));
        totals.push((ri, count));
        self.fragments.push(Fragment { source, glyphs, x: 0., advance, level, run });
        Ok(())
    }
    fn visual_piece(&mut self, ri: usize, source: Range<u32>, level: u8, totals: &mut SmallVec<[(usize, i64); 8]>) -> Result<()> {
        let base = &self.text.paragraph.runs[ri];
        if (base.direction == Direction::RightToLeft) != (!level.is_multiple_of(2)) {
            return self.append_part(ri, source, level, totals);
        }
        let (_, left) = self.text.index.boundary(&self.text.paragraph, source.start)?;
        let (_, right) = self.text.index.boundary(&self.text.paragraph, source.end)?;
        let mut parts = SmallVec::<[Range<u32>; 3]>::new();
        let mut from = source.start;
        if let Some(w) = left {
            let to = w.end.min(source.end);
            parts.push(from..to);
            from = to;
        }
        if from < source.end {
            let tail = right.map_or(source.end, |w| w.start.max(from));
            if from < tail {
                parts.push(from..tail);
            }
            if tail < source.end {
                parts.push(tail..source.end);
            }
        }
        if !level.is_multiple_of(2) {
            parts.reverse();
        }
        for part in parts {
            self.append_part(ri, part, level, totals)?;
        }
        Ok(())
    }
    fn emit_line(&mut self, chosen: usize) -> Result<()> {
        if self.lines.len() >= self.limits.lines {
            return Err(Error::Limit);
        }
        let point = self.text.points[chosen];
        let (fit, fit_end) = self.point_width(point)?;
        let visible = self.paint_start..point.end.byte.max(self.paint_start);
        let visual = self.text.paragraph.analysis.visual_runs(self.from as usize..point.offset as usize)?;
        let first = self.fragments.len();
        let mut totals = SmallVec::<[(usize, i64); 8]>::new();
        for v in visual {
            let start = visible.start.max(v.range.start);
            let end = visible.end.min(v.range.end);
            if start >= end {
                continue;
            }
            let lo = self.text.paragraph.runs.partition_point(|r| r.range.end <= start);
            let hi = self.text.paragraph.runs.partition_point(|r| r.range.start < end);
            for n in 0..hi - lo {
                let ri = if v.level % 2 == 0 { lo + n } else { hi - 1 - n };
                let r = &self.text.paragraph.runs[ri];
                let range = r.range.start.max(start)..r.range.end.min(end);
                if !range.is_empty() {
                    self.visual_piece(ri, range, v.level, &mut totals)?;
                }
            }
        }
        totals.sort_unstable_by_key(|&(i, _)| i);
        let mut content = 0.;
        let mut at = 0;
        while at < totals.len() {
            let ri = totals[at].0;
            let mut sum = 0;
            while at < totals.len() && totals[at].0 == ri {
                sum += totals[at].1;
                at += 1;
            }
            content += sum as f64 * (self.text.paragraph.font_size / f64::from(self.text.paragraph.runs[ri].font.units_per_em()));
        }
        content += self.spacing.width(self.spacing_start, self.from, point.end.byte, point.offset, point.offset as usize == self.text.prepared.text().len());
        if !content.is_finite() || !fit.is_finite() {
            return Err(Error::Invalid);
        }
        let advance = if fit_end.byte == point.end.byte { content } else { fit };
        let offset = self.aligned_offset(advance, point.kind);
        let para_rtl = !self.text.paragraph.analysis.paragraph_level(self.from as usize).unwrap_or(0).is_multiple_of(2);
        if !offset.is_finite() || !(self.options.baseline + self.lines.len() as f64 * self.options.line_height).is_finite() || !((self.lines.len() + 1) as f64 * self.options.line_height).is_finite() {
            return Err(Error::Invalid);
        }
        let mut x = offset - if para_rtl { content - advance } else { 0. };
        for fragment in &mut self.fragments[first..] {
            fragment.x = x + self.spacing.before(self.spacing_start, self.from, fragment.source.start);
            x += fragment.advance;
        }
        self.lines.push(Line {
            source: self.from..point.offset,
            text: visible,
            fragments: first as u32..self.fragments.len() as u32,
            advance,
            content_advance: content,
            offset,
            overflow: advance > self.options.width,
            end: point.kind,
        });
        Ok(())
    }
    fn aligned_offset(&self, advance: f64, end: Kind) -> f64 {
        let align = if end == Kind::Required { self.options.last_align.unwrap_or(self.options.align) } else { self.options.align };
        match align {
            Align::Left => 0.,
            Align::Right => self.options.width - advance,
            Align::Center => (self.options.width - advance) / 2.,
            Align::Start => {
                if self.options.rtl {
                    self.options.width - advance
                } else {
                    0.
                }
            }
            Align::End => {
                if self.options.rtl {
                    0.
                } else {
                    self.options.width - advance
                }
            }
        }
    }
    pub fn finish(self) -> Result<Layout> {
        if self.failed {
            return Err(Error::Failed);
        }
        check(self.cancel)?;
        if !self.progress.finished {
            return Err(Error::Incomplete);
        }
        Ok(Layout { text: self.text, lines: self.lines.into_boxed_slice(), fragments: self.fragments.into_boxed_slice(), owned: self.owned.into_boxed_slice(), options: self.options })
    }
}
