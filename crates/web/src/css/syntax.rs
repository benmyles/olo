//! Immutable preorder component arena. Child lists skip entire subtrees;
//! blocks/functions never recursively own nodes, including on error/drop.
use super::{Diagnostic, DiagnosticKind, Error, Limits, Numeric, Result, TokenKind, Tokenizer};
use std::{
    borrow::Cow,
    ops::Range,
    sync::{Arc, atomic::AtomicBool},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Ident,
    Function,
    AtKeyword,
    Hash,
    IdHash,
    String,
    UnclosedString,
    BadString,
    Url,
    UnclosedUrl,
    BadUrl,
    Number,
    Integer,
    Percentage,
    IntegerPercentage,
    Dimension,
    IntegerDimension,
    UnicodeRange,
    Whitespace,
    Delim,
    Colon,
    Semicolon,
    Comma,
    Round,
    Square,
    Curly,
    Close,
    Cdo,
    Cdc,
}
const KINDS: [Kind; 29] = [
    Kind::Ident,
    Kind::Function,
    Kind::AtKeyword,
    Kind::Hash,
    Kind::IdHash,
    Kind::String,
    Kind::UnclosedString,
    Kind::BadString,
    Kind::Url,
    Kind::UnclosedUrl,
    Kind::BadUrl,
    Kind::Number,
    Kind::Integer,
    Kind::Percentage,
    Kind::IntegerPercentage,
    Kind::Dimension,
    Kind::IntegerDimension,
    Kind::UnicodeRange,
    Kind::Whitespace,
    Kind::Delim,
    Kind::Colon,
    Kind::Semicolon,
    Kind::Comma,
    Kind::Round,
    Kind::Square,
    Kind::Curly,
    Kind::Close,
    Kind::Cdo,
    Kind::Cdc,
];
#[derive(Clone, Copy)]
struct Text {
    start: u32,
    len: u32,
}
impl Text {
    fn bits(self) -> u64 {
        self.start as u64 | ((self.len as u64) << 32)
    }
    fn from_bits(bits: u64) -> Self {
        Self { start: bits as u32, len: (bits >> 32) as u32 }
    }
}
struct Dimension {
    unit: Text,
    representation_end: u32,
}
struct Node {
    value: u64,
    start: u32,
    end: u32,
    after: u32,
    tag: u32,
}
impl Node {
    fn kind(&self) -> Kind {
        KINDS[(self.tag & 255) as usize]
    }
    fn auxiliary(&self) -> usize {
        (self.tag >> 8) as usize
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Memory {
    pub source_bytes: usize,
    pub source_reserved_bytes: usize,
    pub nodes: usize,
    pub node_size: usize,
    pub node_reserved_bytes: usize,
    pub decoded_bytes: usize,
    pub decoded_slot_reserved_bytes: usize,
    pub dimension_reserved_bytes: usize,
    pub diagnostic_reserved_bytes: usize,
}
#[derive(Clone)]
enum Source {
    Shared(Arc<str>),
    Owned(Arc<String>),
}
impl std::ops::Deref for Source {
    type Target = str;
    fn deref(&self) -> &str {
        match self {
            Self::Shared(s) => s,
            Self::Owned(s) => s,
        }
    }
}
impl Source {
    fn capacity(&self) -> usize {
        match self {
            Self::Shared(s) => s.len(),
            Self::Owned(s) => s.capacity(),
        }
    }
}
pub struct Syntax {
    source: Source,
    // Immutable backing range; avoid dispatching source ownership for
    // every source-backed token stored during construction.
    source_start: usize,
    source_bytes: usize,
    nodes: Vec<Node>,
    decoded: Vec<Box<str>>,
    decoded_bytes: usize,
    dimensions: Vec<Dimension>,
    diagnostics: Vec<Diagnostic>,
    limits: Limits,
}
impl std::fmt::Debug for Syntax {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CssSyntax").field("memory", &self.memory()).finish_non_exhaustive()
    }
}
impl Syntax {
    pub fn parse(source: Arc<str>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        Self::parse_with_unicode_ranges(source, limits, cancelled, false)
    }
    /// Special descriptor mode, not normal stylesheet tokenization.
    pub fn parse_with_unicode_ranges(source: Arc<str>, limits: Limits, cancelled: Option<&AtomicBool>, unicode_ranges: bool) -> Result<Self> {
        Self::parse_source(Source::Shared(source), limits, cancelled, unicode_ranges)
    }
    /// Retain an already decoded buffer without copying the full source.
    pub fn parse_owned(source: String, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        if source.capacity() > limits.input_bytes {
            return Err(Error::InputLimit);
        }
        Self::parse_source(Source::Owned(Arc::new(source)), limits, cancelled, false)
    }
    fn parse_source(source: Source, limits: Limits, cancelled: Option<&AtomicBool>, unicode_ranges: bool) -> Result<Self> {
        let mut tokenizer = Tokenizer::new(&source, limits, cancelled)?;
        tokenizer.set_unicode_ranges(unicode_ranges);
        let mut tree = Self {
            source: source.clone(),
            source_start: source.as_ptr() as usize,
            source_bytes: source.len(),
            nodes: Vec::new(),
            decoded: Vec::new(),
            decoded_bytes: 0,
            dimensions: Vec::new(),
            diagnostics: Vec::new(),
            limits,
        };
        let mut stack = Vec::<(u32, char)>::new();
        while let Some(token) = tokenizer.next_token()? {
            if let TokenKind::Close(c) = token.kind
                && stack.last().is_some_and(|&(_, closing)| closing == c)
            {
                let (at, _) = stack.pop().unwrap();
                let after = tree.nodes.len() as u32;
                let node = &mut tree.nodes[at as usize];
                node.after = after;
                node.end = token.span.end;
                node.tag |= 1 << 8;
                continue;
            }
            if tree.nodes.len() >= limits.tokens || tree.nodes.len() >= u32::MAX as usize - 1 {
                return Err(Error::TokenLimit);
            }
            let at = tree.nodes.len() as u32;
            let (kind, value, aux, closing) = match token.kind {
                TokenKind::Ident(s) => (Kind::Ident, tree.store(s)?.bits(), 0, None),
                TokenKind::Function(s) => (Kind::Function, tree.store(s)?.bits(), 0, Some(')')),
                TokenKind::AtKeyword(s) => (Kind::AtKeyword, tree.store(s)?.bits(), 0, None),
                TokenKind::Hash { value, id } => (if id { Kind::IdHash } else { Kind::Hash }, tree.store(value)?.bits(), 0, None),
                TokenKind::String { value, closed } => (if closed { Kind::String } else { Kind::UnclosedString }, tree.store(value)?.bits(), 0, None),
                TokenKind::Url { value, closed } => (if closed { Kind::Url } else { Kind::UnclosedUrl }, tree.store(value)?.bits(), 0, None),
                TokenKind::BadString => (Kind::BadString, 0, 0, None),
                TokenKind::BadUrl => (Kind::BadUrl, 0, 0, None),
                TokenKind::Number(n) => (if n.integer { Kind::Integer } else { Kind::Number }, n.value.to_bits(), 0, None),
                TokenKind::Percentage(n) => (if n.integer { Kind::IntegerPercentage } else { Kind::Percentage }, n.value.to_bits(), 0, None),
                TokenKind::Dimension { number, unit } => {
                    if tree.dimensions.len() >= 0x1000000 {
                        return Err(Error::TokenLimit);
                    }
                    let aux = tree.dimensions.len() as u32;
                    let unit = tree.store(unit)?;
                    tree.dimensions.push(Dimension { unit, representation_end: token.span.start + number.representation.len() as u32 });
                    (if number.integer { Kind::IntegerDimension } else { Kind::Dimension }, number.value.to_bits(), aux, None)
                }
                TokenKind::UnicodeRange { start, end } => (Kind::UnicodeRange, start as u64 | ((end as u64) << 32), 0, None),
                TokenKind::Whitespace => (Kind::Whitespace, 0, 0, None),
                TokenKind::Delim(c) => (Kind::Delim, c as u64, 0, None),
                TokenKind::Colon => (Kind::Colon, 0, 0, None),
                TokenKind::Semicolon => (Kind::Semicolon, 0, 0, None),
                TokenKind::Comma => (Kind::Comma, 0, 0, None),
                TokenKind::Open(c) => {
                    let (k, closing) = match c {
                        '(' => (Kind::Round, ')'),
                        '[' => (Kind::Square, ']'),
                        '{' => (Kind::Curly, '}'),
                        _ => unreachable!(),
                    };
                    (k, 0, 0, Some(closing))
                }
                TokenKind::Close(c) => (Kind::Close, c as u64, 0, None),
                TokenKind::Cdo => (Kind::Cdo, 0, 0, None),
                TokenKind::Cdc => (Kind::Cdc, 0, 0, None),
            };
            tree.nodes.push(Node { value, start: token.span.start, end: token.span.end, after: at + 1, tag: kind as u32 | (aux << 8) });
            if let Some(closing) = closing {
                if stack.len() >= limits.depth {
                    return Err(Error::DepthLimit);
                }
                stack.push((at, closing));
            }
        }
        tree.diagnostics = tokenizer.into_diagnostics();
        let after = tree.nodes.len() as u32;
        for (at, _) in stack.into_iter().rev() {
            if cancelled.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
                return Err(Error::Cancelled);
            }
            let node = &mut tree.nodes[at as usize];
            node.after = after;
            node.end = source.len() as u32;
            if tree.diagnostics.len() >= limits.diagnostics {
                return Err(Error::DiagnosticLimit);
            }
            tree.diagnostics.push(Diagnostic { offset: node.start, kind: DiagnosticKind::UnclosedBlock });
        }
        Ok(tree)
    }
    fn store(&mut self, text: Cow<'_, str>) -> Result<Text> {
        if let Cow::Borrowed(s) = text
            && let Some(start) = (s.as_ptr() as usize).checked_sub(self.source_start).filter(|&start| start <= self.source_bytes && s.len() <= self.source_bytes - start)
        {
            return Ok(Text { start: start as u32, len: s.len() as u32 });
        }
        if text.is_empty() {
            return Ok(Text { start: 0, len: 0 });
        }
        if text.len() > self.limits.decoded_bytes.saturating_sub(self.decoded_bytes) {
            return Err(Error::DecodedLimit);
        }
        if self.decoded.len() >= u32::MAX as usize {
            return Err(Error::DecodedLimit);
        }
        let start = self.decoded.len() as u32;
        self.decoded_bytes += text.len();
        self.decoded.push(text.into_owned().into_boxed_str());
        Ok(Text { start, len: u32::MAX })
    }
    fn text(&self, text: Text) -> &str {
        if text.len == u32::MAX { &self.decoded[text.start as usize] } else { &self.source[text.start as usize..(text.start + text.len) as usize] }
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
    pub fn components(&self) -> Components<'_> {
        Components { tree: self, next: 0, end: self.nodes.len() as u32 }
    }
    pub fn memory(&self) -> Memory {
        Memory {
            source_bytes: self.source.len(),
            source_reserved_bytes: self.source.capacity(),
            nodes: self.nodes.len(),
            node_size: std::mem::size_of::<Node>(),
            node_reserved_bytes: self.nodes.capacity() * std::mem::size_of::<Node>(),
            decoded_bytes: self.decoded_bytes,
            decoded_slot_reserved_bytes: self.decoded.capacity() * std::mem::size_of::<Box<str>>(),
            dimension_reserved_bytes: self.dimensions.capacity() * std::mem::size_of::<Dimension>(),
            diagnostic_reserved_bytes: self.diagnostics.capacity() * std::mem::size_of::<Diagnostic>(),
        }
    }
    pub(super) fn all_range(&self) -> Range<u32> {
        0..self.nodes.len() as u32
    }
    pub(super) fn range(&self, range: Range<u32>) -> Components<'_> {
        Components { tree: self, next: range.start, end: range.end }
    }
    pub(super) fn component(&self, at: u32) -> Component<'_> {
        Component { tree: self, at }
    }
    pub(super) fn item_limit(&self) -> usize {
        self.limits.items
    }
}
#[derive(Clone)]
pub struct Components<'a> {
    tree: &'a Syntax,
    next: u32,
    end: u32,
}
impl<'a> Iterator for Components<'a> {
    type Item = Component<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.end {
            return None;
        }
        let at = self.next;
        self.next = self.tree.nodes[at as usize].after;
        Some(Component { tree: self.tree, at })
    }
}
#[derive(Clone, Copy)]
pub struct Component<'a> {
    tree: &'a Syntax,
    pub(super) at: u32,
}
impl<'a> Component<'a> {
    pub fn kind(self) -> Kind {
        self.tree.nodes[self.at as usize].kind()
    }
    pub fn span(self) -> Range<u32> {
        let n = &self.tree.nodes[self.at as usize];
        n.start..n.end
    }
    pub fn text(self) -> Option<&'a str> {
        if matches!(self.kind(), Kind::Ident | Kind::Function | Kind::AtKeyword | Kind::Hash | Kind::IdHash | Kind::String | Kind::UnclosedString | Kind::Url | Kind::UnclosedUrl) {
            Some(self.tree.text(Text::from_bits(self.tree.nodes[self.at as usize].value)))
        } else {
            None
        }
    }
    pub fn character(self) -> Option<char> {
        if matches!(self.kind(), Kind::Delim | Kind::Close) { char::from_u32(self.tree.nodes[self.at as usize].value as u32) } else { None }
    }
    pub fn number(self) -> Option<Numeric<'a>> {
        let n = &self.tree.nodes[self.at as usize];
        let integer = matches!(n.kind(), Kind::Integer | Kind::IntegerPercentage | Kind::IntegerDimension);
        let end = match n.kind() {
            Kind::Integer | Kind::Number => n.end,
            Kind::Percentage | Kind::IntegerPercentage => n.end - 1,
            Kind::Dimension | Kind::IntegerDimension => self.tree.dimensions[n.auxiliary()].representation_end,
            _ => return None,
        };
        Some(Numeric { value: f64::from_bits(n.value), integer, representation: &self.tree.source[n.start as usize..end as usize] })
    }
    pub fn unit(self) -> Option<&'a str> {
        if matches!(self.kind(), Kind::Dimension | Kind::IntegerDimension) { Some(self.tree.text(self.tree.dimensions[self.tree.nodes[self.at as usize].auxiliary()].unit)) } else { None }
    }
    pub fn unicode_range(self) -> Option<std::ops::RangeInclusive<u32>> {
        if self.kind() == Kind::UnicodeRange {
            let v = self.tree.nodes[self.at as usize].value;
            Some(v as u32..=(v >> 32) as u32)
        } else {
            None
        }
    }
    pub fn is_container(self) -> bool {
        matches!(self.kind(), Kind::Function | Kind::Round | Kind::Square | Kind::Curly)
    }
    pub fn is_closed(self) -> bool {
        self.is_container() && self.tree.nodes[self.at as usize].auxiliary() != 0
    }
    pub fn children(self) -> Components<'a> {
        let range = if self.is_container() { self.at + 1..self.after() } else { self.at..self.at };
        self.tree.range(range)
    }
    pub(super) fn after(self) -> u32 {
        self.tree.nodes[self.at as usize].after
    }
    pub(super) fn child_range(self) -> Range<u32> {
        self.at + 1..self.after()
    }
}
