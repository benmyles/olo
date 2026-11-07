//! Original Unicode 18 UAX #14 default break opportunities, on workers.
//! This does not choose line widths or implement CSS tailoring/dictionaries.
//! Combining sequences use constant scratch state, never a character array.
#[path = "linebreak_data.rs"]
mod data;
use data::class::{self, Class};
use smallvec::SmallVec;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Allowed,
    Required,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opportunity {
    pub offset: u32,
    pub kind: Kind,
}
const REQUIRED: u32 = 1 << 31;
fn decode(v: u32) -> Opportunity {
    Opportunity { offset: v & !REQUIRED, kind: if v & REQUIRED != 0 { Kind::Required } else { Kind::Allowed } }
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub text_bytes: usize,
    /// Logical opportunities, including the implicit required EOT marker.
    pub opportunities: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { text_bytes: 8 * 1024 * 1024, opportunities: 262144 }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Limit,
    Cancelled,
    Incomplete,
    Failed,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Unicode line breaking {self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub input_bytes: usize,
    pub units: usize,
    pub opportunities: usize,
    pub finished: bool,
    /// Raw Line_Break=SA scalars; browser layout still needs complex-context
    /// segmentation/dictionaries rather than silently treating these as words.
    pub complex_context_scalars: usize,
}
/// Immutable source and packed four-byte internal break records. EOT is an
/// implicit Required marker, including at zero for empty input, not a blank
/// line. Text without internal opportunities allocates no offset array.
pub struct Breaks {
    source: Arc<str>,
    offsets: Box<[u32]>,
    complex_context_scalars: usize,
}
impl Breaks {
    pub fn get(&self, index: usize) -> Option<Opportunity> {
        self.offsets.get(index).copied().map(decode).or_else(|| (index == self.offsets.len()).then_some(Opportunity { offset: self.source.len() as u32, kind: Kind::Required }))
    }
    pub fn source(&self) -> &Arc<str> {
        &self.source
    }
    pub fn iter(&self) -> Opportunities<'_> {
        Opportunities { offsets: &self.offsets, end: Some(self.source.len() as u32) }
    }
    pub fn payload_bytes(&self) -> usize {
        self.offsets.len() * size_of::<u32>()
    }
    pub fn complex_context_scalars(&self) -> usize {
        self.complex_context_scalars
    }
}
/// Allocation-free traversal, with the terminal marker derived from source.
pub struct Opportunities<'a> {
    offsets: &'a [u32],
    end: Option<u32>,
}
impl Iterator for Opportunities<'_> {
    type Item = Opportunity;
    fn next(&mut self) -> Option<Self::Item> {
        if let Some((&first, rest)) = self.offsets.split_first() {
            self.offsets = rest;
            Some(decode(first))
        } else {
            self.end.take().map(|offset| Opportunity { offset, kind: Kind::Required })
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.len();
        (n, Some(n))
    }
}
impl ExactSizeIterator for Opportunities<'_> {
    fn len(&self) -> usize {
        self.offsets.len() + usize::from(self.end.is_some())
    }
}
impl DoubleEndedIterator for Opportunities<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if let Some(offset) = self.end.take() {
            Some(Opportunity { offset, kind: Kind::Required })
        } else {
            let (&last, rest) = self.offsets.split_last()?;
            self.offsets = rest;
            Some(decode(last))
        }
    }
}
impl std::iter::FusedIterator for Opportunities<'_> {}
#[derive(Clone, Copy, Debug)]
struct Props(u16);
impl Props {
    fn new(c: char) -> Self {
        let cp = c as u32;
        if !cfg!(feature = "baseline-linebreak-ascii") && cp < 128 {
            return Self(data::ASCII[cp as usize]);
        }
        let mut props = Self(data::RANGES[data::RANGES.partition_point(|&(end, _)| end < cp)].1);
        // LB1 defaults. Preserve original auxiliary facts; raw classes remain
        // in the generated table for future explicit CSS tailoring.
        let class = match props.class() {
            class::AI | class::SG | class::XX => class::AL,
            class::CJ => class::NS,
            class::SA => {
                if props.0 & 4096 != 0 {
                    class::CM
                } else {
                    class::AL
                }
            }
            class => class,
        };
        props.0 = (props.0 & !63) | class as u16;
        props
    }
    fn class(self) -> Class {
        if cfg!(feature = "baseline-linebreak-class-table") { data::CLASS_IDS[(self.0 & 63) as usize] } else { (self.0 & 63) as u8 }
    }
    fn east(self) -> bool {
        self.0 & 64 != 0
    }
    fn initial(self) -> bool {
        self.0 & 128 != 0
    }
    fn final_quote(self) -> bool {
        self.0 & 256 != 0
    }
    fn potential_emoji(self) -> bool {
        self.0 & 512 != 0
    }
    fn circle(self) -> bool {
        self.0 & 1024 != 0
    }
    fn latin() -> Self {
        Self(class::AL as u16)
    }
}
fn hard(c: Class) -> bool {
    matches!(c, class::BK | class::CR | class::LF | class::NL)
}
fn no_base(c: Class) -> bool {
    hard(c) || matches!(c, class::SP | class::ZW)
}
fn alpha(c: Class) -> bool {
    matches!(c, class::AL | class::HL)
}
fn syllable(p: Props) -> bool {
    matches!(p.class(), class::AK | class::AS) || p.circle()
}
fn korean(c: Class) -> bool {
    matches!(c, class::JL | class::JV | class::JT | class::H2 | class::H3)
}
#[derive(Clone, Copy, Debug)]
struct Unit {
    start: u32,
    props: Props,
    zwj_end: bool,
}
impl Unit {
    fn class(self) -> Class {
        self.props.class()
    }
}
#[derive(Default)]
struct Context {
    left: Option<Unit>,
    before_left: Option<Unit>,
    nonspace: Option<Unit>,
    before_nonspace: Option<Unit>,
    numeric: bool,
    numeric_closed: bool,
    ri_odd: bool,
    /// UAX #14 as it stands, without browsers' tailoring (its conformance
    /// test): see `Job::untailored`.
    untailored: bool,
}
impl Context {
    fn consume(&mut self, u: Unit) {
        use class::*;
        let c = u.class();
        self.numeric_closed = matches!(c, CL | CP) && self.numeric;
        self.numeric = if c == NU { true } else { matches!(c, SY | IS) && self.numeric };
        self.ri_odd = c == RI && !self.ri_odd;
        if c != SP {
            self.nonspace = Some(u);
            self.before_nonspace = self.left;
        }
        self.before_left = self.left;
        self.left = Some(u);
    }
    fn boundary(&self, right: Unit, next: Option<Unit>, next2: Option<Unit>) -> Option<Kind> {
        use class::*;
        let left = self.left?;
        let a = left.class();
        let b = right.class();
        let n = next.map(Unit::class);
        let n2 = next2.map(Unit::class);
        // These pairs are prohibited by LB23/LB25/LB28 and no earlier rule
        // can allow them. Retain the full ordered rules as a measurement
        // control and verify both paths against every independent boundary.
        if !cfg!(feature = "baseline-linebreak-ascii") && (alpha(a) || a == NU) && (alpha(b) || b == NU) {
            return None;
        }
        // LB4–LB8a precede combining-sequence resolution (LB9/LB10 are in
        // the streaming unit builder). No boundary exists inside a unit.
        if a == BK {
            return Some(Kind::Required);
        }
        if a == CR && b == LF {
            return None;
        }
        if hard(a) {
            return Some(Kind::Required);
        }
        if hard(b) || matches!(b, SP | ZW) {
            return None;
        }
        if self.nonspace.is_some_and(|u| u.class() == ZW) {
            return Some(Kind::Allowed);
        }
        if left.zwj_end {
            return None;
        }
        // LB11–LB14.
        if a == WJ || b == WJ || a == GL {
            return None;
        }
        if b == GL && !matches!(a, SP | HY | HH) {
            return None;
        }
        if matches!(b, CL | CP | EX | SY) {
            return None;
        }
        let ns = self.nonspace.map(Unit::class);
        if ns == Some(OP) {
            return None;
        }
        // LB15a: keep initial quotation after its qualifying context, also
        // across spaces. All context here already ignores LB9 continuations.
        if let Some(q) = self.nonspace
            && q.class() == QU
            && q.props.initial()
            && self.before_nonspace.is_none_or(|u| hard(u.class()) || matches!(u.class(), OP | QU | GL | SP | ZW))
        {
            return None;
        }
        // LB15b/c/d.
        if b == QU && right.props.final_quote() && next.is_none_or(|u| hard(u.class()) || matches!(u.class(), SP | GL | WJ | CL | QU | CP | EX | IS | SY | ZW)) {
            return None;
        }
        if a == SP && b == IS && n == Some(NU) {
            return Some(Kind::Allowed);
        }
        if b == IS {
            return None;
        }
        // LB16–LB19a.
        if matches!(ns, Some(CL | CP)) && b == NS {
            return None;
        }
        if ns == Some(B2) && b == B2 {
            return None;
        }
        if a == SP {
            return Some(Kind::Allowed);
        }
        if (b == QU && !right.props.initial()) || (a == QU && !left.props.final_quote()) {
            return None;
        }
        if b == QU && (!left.props.east() || next.is_none_or(|u| !u.props.east())) {
            return None;
        }
        if a == QU && (!right.props.east() || self.before_left.is_none_or(|u| !u.props.east())) {
            return None;
        }
        // LB20–LB24.
        if a == CB || b == CB {
            return Some(Kind::Allowed);
        }
        if matches!(a, HY | HH) && alpha(b) && self.before_left.is_none_or(|u| hard(u.class()) || matches!(u.class(), SP | ZW | CB | GL)) {
            return None;
        }
        if matches!(b, BA | HH | HY | NS) || a == BB {
            return None;
        }
        if self.before_left.is_some_and(|u| u.class() == HL) && matches!(a, HY | HH) && b != HL {
            return None;
        }
        // LB21b, and as browsers do: no break after a solidus before a
        // letter (a URL's "https://example.com/a/path" stays whole, as in
        // Chrome; UAX #14 alone would break after each "/").
        if a == SY && (b == HL || (alpha(b) && !self.untailored)) {
            return None;
        }
        if b == IN {
            return None;
        }
        if (alpha(a) && b == NU) || (a == NU && alpha(b)) {
            return None;
        }
        if (a == PR && matches!(b, ID | EB | EM)) || (matches!(a, ID | EB | EM) && b == PO) {
            return None;
        }
        if (matches!(a, PR | PO) && alpha(b)) || (alpha(a) && matches!(b, PR | PO)) {
            return None;
        }
        // LB25: bounded right context and incrementally retained numeric tail.
        if matches!(b, PO | PR) && (self.numeric || self.numeric_closed) {
            return None;
        }
        if matches!(a, PO | PR) && (b == NU || (b == OP && (n == Some(NU) || (n == Some(IS) && n2 == Some(NU))))) {
            return None;
        }
        if b == NU && (matches!(a, HY | IS) || self.numeric) {
            return None;
        }
        // LB26/LB27.
        if (a == JL && matches!(b, JL | JV | H2 | H3)) || (matches!(a, JV | H2) && matches!(b, JV | JT)) || (matches!(a, JT | H3) && b == JT) {
            return None;
        }
        if (korean(a) && b == PO) || (a == PR && korean(b)) {
            return None;
        }
        // LB28/LB28a.
        if alpha(a) && alpha(b) {
            return None;
        }
        if a == AP && syllable(right.props) {
            return None;
        }
        if syllable(left.props) && matches!(b, VF | VI) {
            return None;
        }
        if a == VI && self.before_left.is_some_and(|u| syllable(u.props)) && (b == AK || right.props.circle()) {
            return None;
        }
        if syllable(left.props) && syllable(right.props) && n == Some(VF) {
            return None;
        }
        // LB29/LB30.
        if a == IS && alpha(b) {
            return None;
        }
        if ((alpha(a) || a == NU) && b == OP && !right.props.east()) || (a == CP && !left.props.east() && (alpha(b) || b == NU)) {
            return None;
        }
        // LB30a/LB30b/LB31.
        if a == RI && b == RI && self.ri_odd {
            return None;
        }
        if b == EM && (a == EB || left.props.potential_emoji()) {
            return None;
        }
        Some(Kind::Allowed)
    }
}
fn check(cancel: Option<&AtomicBool>) -> Result<()> {
    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
fn ascii_run_prefix(bytes: &[u8], class: Class) -> usize {
    use wide::u8x32;
    let mut at = 0;
    while bytes.len() - at >= 32 {
        let mut v = u8x32::new(bytes[at..at + 32].try_into().unwrap());
        let (lower, upper) = if class == class::AL {
            v |= u8x32::splat(0x20);
            (b'a', b'z')
        } else {
            (b'0', b'9')
        };
        let letters = (v.simd_ge(u8x32::splat(lower)) & v.simd_le(u8x32::splat(upper))).to_bitmask();
        if letters != u32::MAX {
            return at + (!letters).trailing_zeros() as usize;
        }
        at += 32;
    }
    at + bytes[at..].iter().take_while(|b| if class == class::AL { b.is_ascii_alphabetic() } else { b.is_ascii_digit() }).count()
}
/// Worker job. Lookahead retains at most three complete units and one pending
/// combining sequence. Backward context keeps four fixed unit values, never
/// an input-sized array. Two-unit right context handles numbers/quotations
/// without rescanning text.
pub struct Job<'a> {
    source: Arc<str>,
    limits: Limits,
    cancel: Option<&'a AtomicBool>,
    at: usize,
    pending: Option<Unit>,
    ready: SmallVec<[Unit; 4]>,
    context: Context,
    offsets: Vec<u32>,
    progress: Progress,
    failed: bool,
}
impl<'a> Job<'a> {
    /// UAX #14's default rules alone, without what browsers tailor (no
    /// break after a solidus before a letter): for the conformance test.
    pub fn untailored(mut self) -> Self {
        self.context.untailored = true;
        self
    }
    pub fn new(source: Arc<str>, limits: Limits, cancel: Option<&'a AtomicBool>) -> Result<Self> {
        check(cancel)?;
        if source.len() > limits.text_bytes || source.len() >= REQUIRED as usize {
            return Err(Error::Limit);
        }
        Ok(Self { source, limits, cancel, at: 0, pending: None, ready: SmallVec::new(), context: Context::default(), offsets: Vec::new(), progress: Progress::default(), failed: false })
    }
    pub fn progress(&self) -> Progress {
        self.progress
    }
    pub fn completed(&self) -> Result<Opportunities<'_>> {
        if self.failed {
            return Err(Error::Failed);
        }
        Ok(Opportunities { offsets: &self.offsets, end: self.progress.finished.then_some(self.source.len() as u32) })
    }
    pub fn step(&mut self, byte_budget: usize) -> Result<Progress> {
        if self.failed {
            return Err(Error::Failed);
        }
        if let Err(error) = check(self.cancel) {
            self.failed = true;
            return Err(error);
        }
        // Dispatch once per worker slice, not once per non-ASCII scalar. The
        // no-bulk monomorph has no speculative run checks in its hot loop.
        let ascii_slice = !cfg!(feature = "baseline-linebreak-ascii")
            && !cfg!(feature = "baseline-linebreak-runs")
            && byte_budget >= 4
            && self.source.as_bytes()[self.at..self.source.len().min(self.at.saturating_add(byte_budget))].is_ascii();
        let result = if ascii_slice { self.advance::<true>(byte_budget) } else { self.advance::<false>(byte_budget) };
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance<const BULK: bool>(&mut self, mut budget: usize) -> Result<Progress> {
        check(self.cancel)?;
        if self.progress.finished {
            return Ok(self.progress);
        }
        while budget > 0 && self.at < self.source.len() {
            check(self.cancel)?;
            if BULK
                && budget >= 4
                && self.source.as_bytes()[self.at].is_ascii_alphanumeric()
                && self.ready.len() == 2
                && let Some(run_class) = self.pending.map(Unit::class)
                && matches!(run_class, class::AL | class::NU)
                && self.context.left.is_some_and(|u| u.class() == run_class)
                && self.ready.iter().all(|u| u.class() == run_class)
            {
                let class = run_class;
                let count = ascii_run_prefix(&self.source.as_bytes()[self.at..self.source.len().min(self.at + budget.min(65536))], class);
                if count != 0 {
                    // Every skipped boundary is AL × AL or NU × NU. Advance the
                    // same context/lookahead frontier, preserving old unit
                    // properties/ZWJ endings when fewer than four are read.
                    let old = [self.ready[0], self.ready[1], self.pending.unwrap()];
                    let unit = |i: usize| if i < 3 { old[i] } else { Unit { start: (self.at + i - 3) as u32, props: Props(class as u16), zwj_end: false } };
                    let before = if count == 1 { self.context.left } else { Some(unit(count - 2)) };
                    self.context.left = Some(unit(count - 1));
                    self.context.before_left = before;
                    self.context.nonspace = self.context.left;
                    self.context.before_nonspace = before;
                    self.context.numeric = class == class::NU;
                    self.context.numeric_closed = false;
                    self.context.ri_odd = false;
                    self.ready[0] = unit(count);
                    self.ready[1] = unit(count + 1);
                    self.pending = Some(unit(count + 2));
                    self.progress.units += count;
                    self.at += count;
                    budget -= count;
                    self.progress.input_bytes = self.at;
                    continue;
                }
            }
            let c = self.source[self.at..].chars().next().unwrap();
            let props = Props::new(c);
            let class = props.class();
            self.progress.complex_context_scalars += usize::from(props.0 & 2048 != 0);
            let end = self.at + c.len_utf8();
            let continuation = matches!(class, class::CM | class::ZWJ);
            if continuation && self.pending.is_some_and(|u| !no_base(u.class())) {
                self.pending.as_mut().unwrap().zwj_end = class == class::ZWJ;
            } else {
                if let Some(unit) = self.pending.take() {
                    self.ready.push(unit);
                }
                self.pending = Some(Unit { start: self.at as u32, props: if continuation { Props::latin() } else { props }, zwj_end: class == class::ZWJ });
                if self.ready.len() >= 3 {
                    self.consume_ready()?;
                }
            }
            self.at = end;
            budget = budget.saturating_sub(c.len_utf8());
            self.progress.input_bytes = end;
        }
        if self.at == self.source.len() {
            if let Some(unit) = self.pending.take() {
                self.ready.push(unit);
            }
            while !self.ready.is_empty() {
                check(self.cancel)?;
                self.consume_ready()?;
            }
            if self.progress.opportunities == self.limits.opportunities {
                return Err(Error::Limit);
            }
            self.progress.opportunities += 1;
            self.progress.finished = true;
        }
        Ok(self.progress)
    }
    fn consume_ready(&mut self) -> Result<()> {
        let right = self.ready[0];
        if let Some(kind) = self.context.boundary(right, self.ready.get(1).copied(), self.ready.get(2).copied()) {
            self.push(right.start, kind)?;
        }
        self.context.consume(right);
        self.ready.remove(0);
        self.progress.units += 1;
        Ok(())
    }
    fn push(&mut self, offset: u32, kind: Kind) -> Result<()> {
        if self.offsets.len() == self.limits.opportunities {
            return Err(Error::Limit);
        }
        self.offsets.push(offset | if kind == Kind::Required { REQUIRED } else { 0 });
        self.progress.opportunities += 1;
        Ok(())
    }
    pub fn finish(self) -> Result<Breaks> {
        if self.failed {
            return Err(Error::Failed);
        }
        check(self.cancel)?;
        if !self.progress.finished {
            return Err(Error::Incomplete);
        }
        Ok(Breaks { source: self.source, offsets: self.offsets.into_boxed_slice(), complex_context_scalars: self.progress.complex_context_scalars })
    }
}
