//! CSS Text phase I over a single inline formatting context, before bidi and
//! shaping. Input is DOM text: LF is a segment break; CR is a space, not CRLF
//! normalization. No DOM mutation, trimming at line edges or tab positioning.
//! The initial UA policy converts collapsible segment breaks to spaces (CSS
//! Text 3 §4.1.3 permits UA policy). Language-specific removal remains open.
use crate::css::style::{Keyword, Property, Style, Value};
use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Collapse {
    Collapse,
    Preserve,
    PreserveBreaks,
    PreserveSpaces,
    BreakSpaces,
}
impl Collapse {
    fn collapses(self) -> bool {
        matches!(self, Self::Collapse | Self::PreserveBreaks)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WhiteSpace {
    pub collapse: Collapse,
    pub wrap: bool,
}
impl Default for WhiteSpace {
    fn default() -> Self {
        Self { collapse: Collapse::Collapse, wrap: true }
    }
}
impl WhiteSpace {
    pub fn for_style(style: &Style) -> Result<Self> {
        let collapse = match style.value(Property::WhiteSpaceCollapse) {
            Value::Keyword(Keyword::Collapse) => Collapse::Collapse,
            Value::Keyword(Keyword::Preserve) => Collapse::Preserve,
            Value::Keyword(Keyword::PreserveBreaks) => Collapse::PreserveBreaks,
            Value::Keyword(Keyword::PreserveSpaces) => Collapse::PreserveSpaces,
            Value::Keyword(Keyword::BreakSpaces) => Collapse::BreakSpaces,
            _ => return Err(Error::Invalid),
        };
        let wrap = match style.value(Property::TextWrapMode) {
            Value::Keyword(Keyword::Wrap) => true,
            Value::Keyword(Keyword::Nowrap) => false,
            _ => return Err(Error::Invalid),
        };
        Ok(Self { collapse, wrap })
    }
}
/// Semantic controls in a formatting source, never inserted into the DOM.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ControlKind {
    Break,
    Opportunity,
}
impl ControlKind {
    pub fn carrier(self) -> &'static str {
        match self {
            Self::Break => "\n",
            Self::Opportunity => "\u{200b}",
        }
    }
}
/// Four-byte source position and semantic tag. Control-bearing contexts are
/// limited to the low 30 source-position bits, independently of the byte quota.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Control(u32);
impl Control {
    const POSITION: u32 = (1 << 30) - 1;
    pub fn new(offset: u32, kind: ControlKind) -> Result<Self> {
        if offset > Self::POSITION {
            return Err(Error::Limit);
        }
        Ok(Self(offset | ((kind as u32) << 30)))
    }
    pub fn offset(self) -> u32 {
        self.0 & Self::POSITION
    }
    pub fn kind(self) -> ControlKind {
        if self.0 >> 30 == 0 { ControlKind::Break } else { ControlKind::Opportunity }
    }
}
/// A nonempty UTF-8 range. Spans partition the source without gaps. The host
/// keeps each span's DOM node/local offset, font and other style data. A span
/// boundary is transparent to collapsing; replaced/atomic inlines must split
/// contexts instead of being silently omitted from this input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub range: Range<u32>,
    pub whitespace: WhiteSpace,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingKind {
    Copy,
    Replace,
    Removed,
}
impl MappingKind {
    fn code(self) -> u32 {
        match self {
            Self::Copy => 0,
            Self::Replace => 1,
            Self::Removed => 2,
        }
    }
}
// End coordinates are the next record's starts (or the source/output lengths).
// Span IDs use 30 bits; the remaining two hold the transformation kind. No
// per-character index, redundant ends, pointer or allocation in each record.
struct EncodedMapping {
    input: u32,
    output: u32,
    tag: u32,
    #[cfg(feature = "baseline-text-map-wide")]
    _wide: [u32; 3],
}
/// Ordered source coverage, including removed text (an empty output range).
/// Copy/Replace ranges have equal byte lengths. CR/TAB/LF replacements are
/// one-byte spaces; consecutive mappings merge only within one source span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    pub input: Range<u32>,
    pub output: Range<u32>,
    pub span: u32,
    pub kind: MappingKind,
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub text_bytes: usize,
    pub spans: usize,
    pub mappings: usize,
    pub trim_ranges: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { text_bytes: 8 * 1024 * 1024, spans: 4096, mappings: 262144, trim_ranges: 131072 }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Limit,
    Cancelled,
    Incomplete,
    Failed,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CSS text preparation {self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// Input byte visits: a backward adjacency scan, then a forward transform.
    pub scanned_bytes: usize,
    pub prepared_bytes: usize,
    pub finished: bool,
}
/// Immutable result for shaping/layout/structured inspection. Original source
/// and span ownership survive even when an entire text node collapsed away.
pub struct Prepared {
    source: Arc<str>,
    text: Arc<str>,
    spans: Arc<[Span]>,
    mappings: Box<[EncodedMapping]>,
    controls: Option<triomphe::ThinArc<(), Control>>,
}
impl Prepared {
    pub fn source(&self) -> &Arc<str> {
        &self.source
    }
    pub fn text(&self) -> &Arc<str> {
        &self.text
    }
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }
    pub fn controls(&self) -> &[Control] {
        self.controls.as_ref().map_or(&[], |f| &f.slice)
    }
    pub fn mappings(&self) -> impl ExactSizeIterator<Item = Mapping> + DoubleEndedIterator + '_ {
        (0..self.mappings.len()).map(|i| self.mapping(i).unwrap())
    }
    fn mapping(&self, i: usize) -> Option<Mapping> {
        let m = self.mappings.get(i)?;
        let input_end = self.mappings.get(i + 1).map_or(self.source.len() as u32, |m| m.input);
        let output_end = self.mappings.get(i + 1).map_or(self.text.len() as u32, |m| m.output);
        Some(Mapping {
            input: m.input..input_end,
            output: m.output..output_end,
            span: m.tag >> 2,
            kind: match m.tag & 3 {
                0 => MappingKind::Copy,
                1 => MappingKind::Replace,
                _ => MappingKind::Removed,
            },
        })
    }
    pub fn mapping_at_source(&self, byte: u32) -> Option<Mapping> {
        let i = self.mappings.partition_point(|m| m.input <= byte).checked_sub(1)?;
        self.mapping(i).filter(|m| m.input.contains(&byte))
    }
    /// Only emitted bytes, not the removed source intervals at this position.
    /// These are UTF-8 byte coordinates, not yet grapheme/caret hit testing.
    pub fn mapping_at_output(&self, byte: u32) -> Option<Mapping> {
        let i = self.mappings.partition_point(|m| m.output <= byte).checked_sub(1)?;
        self.mapping(i).filter(|m| m.output.contains(&byte))
    }
    pub fn mapping_payload_bytes(&self) -> usize {
        self.mappings.len() * Self::mapping_record_bytes()
    }
    pub const fn mapping_record_bytes() -> usize {
        size_of::<EncodedMapping>()
    }
}
fn bidi_control(c: char) -> bool {
    matches!(c, '\u{61c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}
fn space_tab(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r')
}
fn check(cancel: Option<&AtomicBool>) -> Result<()> {
    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
// Bytes that cannot be whitespace, bidi controls or UTF-8 continuation bytes.
// The scalar remainder also keeps very short words cheap. No target-specific
// dispatch or unsafe code: wide uses the target's native portable SIMD lanes.
fn printable_prefix(bytes: &[u8]) -> usize {
    use wide::u8x32;
    let mut at = 0;
    while bytes.len() - at >= 32 {
        let v = u8x32::new(bytes[at..at + 32].try_into().unwrap());
        let special = (v.simd_le(u8x32::splat(0x20)) | v.simd_ge(u8x32::splat(0x7f))).to_bitmask();
        if special != 0 {
            return at + special.trailing_zeros() as usize;
        }
        at += 32;
    }
    at + bytes[at..].iter().take_while(|&&b| b.is_ascii_graphic()).count()
}
/// Worker-only, cancellable and resumable. Two linear scans avoid unbounded
/// lookahead over whitespace/bidi controls and retain no per-character array.
/// No output can be published after failure or before both scans complete.
pub struct Job<'a> {
    source: Arc<str>,
    spans: Arc<[Span]>,
    limits: Limits,
    cancel: Option<&'a AtomicBool>,
    reverse: bool,
    at: usize,
    span: usize,
    after_break: bool,
    trims: Vec<Range<u32>>,
    trim: usize,
    previous_space: bool,
    output: Option<String>,
    mappings: Vec<EncodedMapping>,
    progress: Progress,
    failed: bool,
    controls: Box<[Control]>,
}
impl<'a> Job<'a> {
    pub fn new(source: Arc<str>, spans: Arc<[Span]>, limits: Limits, cancel: Option<&'a AtomicBool>) -> Result<Self> {
        check(cancel)?;
        if source.len() > limits.text_bytes || source.len() > u32::MAX as usize || spans.len() > limits.spans || spans.len() >= 1 << 30 {
            return Err(Error::Limit);
        }
        let mut end = 0;
        for span in spans.iter() {
            check(cancel)?;
            if span.range.start != end
                || span.range.start >= span.range.end
                || span.range.end as usize > source.len()
                || !source.is_char_boundary(span.range.start as usize)
                || !source.is_char_boundary(span.range.end as usize)
            {
                return Err(Error::Invalid);
            }
            end = span.range.end;
        }
        if end as usize != source.len() {
            return Err(Error::Invalid);
        }
        let at = source.len();
        let span = spans.len().saturating_sub(1);
        Ok(Self {
            source,
            spans,
            limits,
            cancel,
            reverse: true,
            at,
            span,
            after_break: false,
            trims: Vec::new(),
            trim: 0,
            previous_space: false,
            output: None,
            mappings: Vec::new(),
            progress: Progress::default(),
            failed: false,
            controls: Box::default(),
        })
    }
    /// Host-provided semantic controls in the formatting source. Forced breaks
    /// survive every collapsing mode; optional controls retain an explicit point.
    pub fn with_controls(mut self, controls: Box<[Control]>) -> Result<Self> {
        check(self.cancel)?;
        if self.failed || self.progress.finished || self.progress.scanned_bytes != 0 || !self.controls.is_empty() {
            return Err(Error::Invalid);
        }
        if controls.len() > self.limits.spans {
            return Err(Error::Limit);
        }
        if controls.windows(2).any(|p| p[0].offset() + p[0].kind().carrier().len() as u32 > p[1].offset())
            || controls.iter().any(|c| !self.source.get(c.offset() as usize..).is_some_and(|s| s.starts_with(c.kind().carrier())))
        {
            return Err(Error::Invalid);
        }
        self.controls = controls;
        Ok(self)
    }
    pub fn progress(&self) -> Progress {
        self.progress
    }
    pub fn step(&mut self, byte_budget: usize) -> Result<Progress> {
        if self.failed {
            return Err(Error::Failed);
        }
        let result = self.advance(byte_budget);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance(&mut self, mut budget: usize) -> Result<Progress> {
        check(self.cancel)?;
        if self.progress.finished {
            return Ok(self.progress);
        }
        while budget != 0 {
            check(self.cancel)?;
            if self.reverse {
                if self.at == 0 {
                    self.trims.reverse();
                    self.reverse = false;
                    self.span = 0;
                    self.after_break = false;
                    continue;
                }
                if !cfg!(feature = "baseline-text-prepare") && !self.after_break {
                    // Only a segment break can start backward adjacency work.
                    // Bound the SIMD search and never strand a UTF-8 cursor.
                    let mut start = self.at.saturating_sub(budget.min(65536));
                    while !self.source.is_char_boundary(start) {
                        start += 1;
                    }
                    let from = memchr::memrchr(b'\n', &self.source.as_bytes()[start..self.at]).map_or(start, |i| start + i + 1);
                    let count = self.at - from;
                    if count != 0 {
                        self.at = from;
                        budget -= count;
                        self.progress.scanned_bytes += count;
                        continue;
                    }
                }
                while self.at <= self.spans[self.span].range.start as usize {
                    self.span -= 1;
                }
                let c = self.source[..self.at].chars().next_back().unwrap();
                let start = self.at - c.len_utf8();
                if c == '\n' {
                    self.after_break = true;
                } else if bidi_control(c) { /* transparent to whitespace adjacency */
                } else if space_tab(c) && self.spans[self.span].whitespace.collapse.collapses() {
                    if self.after_break {
                        if let Some(last) = self.trims.last_mut().filter(|r| r.start == self.at as u32) {
                            last.start = start as u32;
                        } else {
                            if self.trims.len() == self.limits.trim_ranges {
                                return Err(Error::Limit);
                            }
                            self.trims.push(start as u32..self.at as u32);
                        }
                    }
                } else {
                    self.after_break = false;
                }
                self.at = start;
                budget = budget.saturating_sub(c.len_utf8());
                self.progress.scanned_bytes += c.len_utf8();
            } else {
                if self.at == self.source.len() {
                    self.progress.finished = true;
                    break;
                }
                while self.at >= self.spans[self.span].range.end as usize {
                    self.span += 1;
                }
                if !cfg!(feature = "baseline-text-prepare") {
                    let stop = self.spans[self.span].range.end as usize;
                    let count = printable_prefix(&self.source.as_bytes()[self.at..stop.min(self.at.saturating_add(budget))]);
                    if count != 0 {
                        self.emit(self.at + count, MappingKind::Copy, None)?;
                        self.at += count;
                        budget -= count;
                        self.progress.scanned_bytes += count;
                        self.after_break = false;
                        self.previous_space = false;
                        continue;
                    }
                }
                let c = self.source[self.at..].chars().next().unwrap();
                let end = self.at + c.len_utf8();
                let collapse = self.spans[self.span].whitespace.collapse;
                while self.trim < self.trims.len() && self.trims[self.trim].end as usize <= self.at {
                    self.trim += 1;
                }
                let trimmed = self.trims.get(self.trim).is_some_and(|r| r.contains(&(self.at as u32)));
                let (kind, replacement) = if trimmed || (space_tab(c) && collapse.collapses() && self.after_break) {
                    (MappingKind::Removed, None)
                } else if c == '\n' {
                    self.after_break = true;
                    match collapse {
                        _ if self.controls.binary_search_by_key(&(self.at as u32), |c| c.offset()).is_ok_and(|i| self.controls[i].kind() == ControlKind::Break) => {
                            self.previous_space = false;
                            (MappingKind::Copy, None)
                        }
                        Collapse::Collapse => {
                            let duplicate = self.previous_space;
                            self.previous_space = true;
                            if duplicate { (MappingKind::Removed, None) } else { (MappingKind::Replace, Some(' ')) }
                        }
                        Collapse::PreserveSpaces => {
                            self.previous_space = false;
                            (MappingKind::Replace, Some(' '))
                        }
                        _ => {
                            self.previous_space = false;
                            (MappingKind::Copy, None)
                        }
                    }
                } else if space_tab(c) {
                    self.after_break = false;
                    if collapse.collapses() {
                        let duplicate = self.previous_space;
                        self.previous_space = true;
                        if duplicate {
                            (MappingKind::Removed, None)
                        } else if c == ' ' {
                            (MappingKind::Copy, None)
                        } else {
                            (MappingKind::Replace, Some(' '))
                        }
                    } else {
                        self.previous_space = false;
                        if c == '\r' || (c == '\t' && collapse == Collapse::PreserveSpaces) { (MappingKind::Replace, Some(' ')) } else { (MappingKind::Copy, None) }
                    }
                } else {
                    if !bidi_control(c) {
                        self.after_break = false;
                        self.previous_space = false;
                    }
                    (MappingKind::Copy, None)
                };
                self.emit(end, kind, replacement)?;
                self.at = end;
                budget = budget.saturating_sub(c.len_utf8());
                self.progress.scanned_bytes += c.len_utf8();
            }
        }
        // Empty input and exact-budget completion must not need a magic extra
        // worker call; a zero budget does no scan or transform work.
        if self.at == self.source.len() && (!self.reverse || self.source.is_empty()) {
            self.progress.finished = true;
        }
        Ok(self.progress)
    }
    fn emit(&mut self, end: usize, kind: MappingKind, replacement: Option<char>) -> Result<()> {
        let start = self.progress.prepared_bytes as u32;
        let length = if kind == MappingKind::Removed { 0 } else { end - self.at };
        let finish = start + length as u32;
        let tag = (self.span as u32) << 2 | kind.code();
        let merged = self.mappings.last().is_some_and(|m| m.tag == tag);
        if !merged && self.mappings.len() == self.limits.mappings {
            return Err(Error::Limit);
        }
        if kind != MappingKind::Copy && self.output.is_none() {
            self.output = Some(self.source[..self.at].to_owned());
        }
        if let Some(output) = &mut self.output {
            match kind {
                MappingKind::Copy => output.push_str(&self.source[self.at..end]),
                MappingKind::Replace => output.push(replacement.unwrap()),
                MappingKind::Removed => (),
            }
        }
        if !merged {
            self.mappings.push(EncodedMapping {
                input: self.at as u32,
                output: start,
                tag,
                #[cfg(feature = "baseline-text-map-wide")]
                _wide: [0; 3],
            });
        }
        self.progress.prepared_bytes = finish as usize;
        Ok(())
    }
    pub fn finish(self) -> Result<Prepared> {
        if self.failed {
            return Err(Error::Failed);
        }
        check(self.cancel)?;
        if !self.progress.finished {
            return Err(Error::Incomplete);
        }
        let text = self.output.map_or_else(|| self.source.clone(), Arc::from);
        let controls = (!self.controls.is_empty()).then(|| triomphe::ThinArc::from_header_and_iter((), self.controls.into_vec().into_iter()));
        Ok(Prepared { source: self.source, text, spans: self.spans, mappings: self.mappings.into_boxed_slice(), controls })
    }
}
