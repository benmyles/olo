//! Copy component values without tokenizing their serialized form again.
//! Source bytes remain coherent for numeric representations and debugging;
//! boundary comments prevent pasted tokens from changing their identity.
use super::*;
use smallvec::SmallVec;
use std::sync::atomic::Ordering;

pub(in crate::css) struct Assembler {
    source: String,
    nodes: Vec<Node>,
    decoded: Vec<Box<str>>,
    decoded_bytes: usize,
    dimensions: Vec<Dimension>,
    limits: Limits,
    previous: Option<(Kind, Option<char>)>,
}
impl Assembler {
    pub fn new(limits: Limits) -> Self {
        Self { source: String::new(), nodes: Vec::new(), decoded: Vec::new(), decoded_bytes: 0, dimensions: Vec::new(), limits, previous: None }
    }
    pub fn len(&self) -> u32 {
        self.nodes.len() as u32
    }
    fn append(&mut self, text: &str) -> Result<()> {
        if text.len() > self.limits.input_bytes.saturating_sub(self.source.len()) || text.len() > (u32::MAX as usize).saturating_sub(self.source.len()) {
            return Err(Error::InputLimit);
        }
        self.source.push_str(text);
        Ok(())
    }
    fn boundary(&mut self, c: Component<'_>) -> Result<()> {
        if let Some((a, ac)) = self.previous
            && needs_boundary(a, ac, c.kind(), c.character())
        {
            self.append("/**/")?;
        }
        Ok(())
    }
    fn text(&mut self, original: &Syntax, text: Text, old_start: u32, new_start: u32, copied_end: u32) -> Result<Text> {
        if text.len != u32::MAX && text.start >= old_start && text.start + text.len <= copied_end {
            return Ok(Text { start: text.start - old_start + new_start, len: text.len });
        }
        self.decoded_text(original.text(text))
    }
    fn decoded_text(&mut self, text: &str) -> Result<Text> {
        if text.is_empty() {
            return Ok(Text { start: 0, len: 0 });
        }
        if text.len() > self.limits.decoded_bytes.saturating_sub(self.decoded_bytes) || self.decoded.len() >= u32::MAX as usize {
            return Err(Error::DecodedLimit);
        }
        let start = self.decoded.len() as u32;
        self.decoded_bytes += text.len();
        self.decoded.push(text.into());
        Ok(Text { start, len: u32::MAX })
    }
    fn copy_node(&mut self, c: Component<'_>, start: u32, old_end: u32) -> Result<u32> {
        if self.nodes.len() >= self.limits.tokens || self.nodes.len() >= u32::MAX as usize - 1 {
            return Err(Error::TokenLimit);
        }
        let n = &c.tree.nodes[c.at as usize];
        let mut value = n.value;
        let mut tag = n.tag;
        if c.text().is_some() {
            value = self.text(c.tree, Text::from_bits(value), n.start, start, old_end)?.bits();
        }
        if matches!(c.kind(), Kind::Dimension | Kind::IntegerDimension) {
            if self.dimensions.len() >= 0x1000000 {
                return Err(Error::TokenLimit);
            }
            let d = &c.tree.dimensions[n.auxiliary()];
            let unit = self.text(c.tree, d.unit, n.start, start, old_end)?;
            tag = c.kind() as u32 | ((self.dimensions.len() as u32) << 8);
            self.dimensions.push(Dimension { unit, representation_end: d.representation_end - n.start + start });
        }
        let at = self.nodes.len() as u32;
        self.nodes.push(Node { value, start, end: self.source.len() as u32, after: at + 1, tag });
        Ok(at)
    }
    pub fn leaf(&mut self, c: Component<'_>) -> Result<()> {
        self.boundary(c)?;
        let start = self.source.len() as u32;
        let span = c.span();
        if matches!(c.kind(), Kind::UnclosedString | Kind::UnclosedUrl) {
            let text = c.text().unwrap();
            self.append(if c.kind() == Kind::UnclosedString { "\"" } else { "url(" })?;
            for ch in text.chars() {
                if ch.is_ascii_control() {
                    self.append(&format!("\\{:x} ", ch as u32))?;
                } else if ch == '\\' || ch == '"' || c.kind() == Kind::UnclosedUrl && matches!(ch, '\'' | '(' | ')' | ' ') {
                    self.append("\\")?;
                    self.append(ch.encode_utf8(&mut [0; 4]))?;
                } else {
                    self.append(ch.encode_utf8(&mut [0; 4]))?;
                }
            }
            self.append(if c.kind() == Kind::UnclosedString { "\"" } else { ")" })?;
            let at = self.copy_node(c, start, span.start)?;
            self.nodes[at as usize].tag = if c.kind() == Kind::UnclosedString { Kind::String } else { Kind::Url } as u32;
        } else {
            self.append(&c.tree.source[span.start as usize..span.end as usize])?;
            self.copy_node(c, start, span.end)?;
            // A standalone escaped-newline delimiter needs its newline to
            // remain a delimiter if the source is subsequently reparsed.
            if c.character() == Some('\\') {
                self.append("\n")?;
            }
        }
        self.previous = Some((c.kind(), c.character()));
        Ok(())
    }
    pub fn open(&mut self, c: Component<'_>) -> Result<u32> {
        self.boundary(c)?;
        let start = self.source.len() as u32;
        let span = c.span();
        let raw = &c.tree.source[span.start as usize..span.end as usize];
        let prefix = if c.kind() == Kind::Function {
            let mut i = 0;
            let bytes = raw.as_bytes();
            while i < bytes.len() && bytes[i] != b'(' {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            (i + 1).min(raw.len())
        } else {
            1
        };
        self.append(&raw[..prefix])?;
        let at = self.copy_node(c, start, span.start + prefix as u32)?;
        self.nodes[at as usize].tag &= 255;
        self.previous = None;
        Ok(at)
    }
    pub fn close(&mut self, at: u32, kind: Kind) -> Result<()> {
        self.append(match kind {
            Kind::Square => "]",
            Kind::Curly => "}",
            _ => ")",
        })?;
        let after = self.nodes.len() as u32;
        let n = &mut self.nodes[at as usize];
        n.after = after;
        n.end = self.source.len() as u32;
        n.tag |= 1 << 8;
        self.previous = None;
        Ok(())
    }
    pub fn copy(&mut self, values: Components<'_>, cancelled: Option<&AtomicBool>) -> Result<()> {
        let mut stack: SmallVec<[_; 8]> = SmallVec::new();
        stack.push((values, None));
        while let Some((input, _)) = stack.last_mut() {
            if cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Err(Error::Cancelled);
            }
            if let Some(c) = input.next() {
                if c.is_container() {
                    if stack.len() > self.limits.depth {
                        return Err(Error::DepthLimit);
                    }
                    let at = self.open(c)?;
                    stack.push((c.children(), Some((at, c.kind()))));
                } else {
                    self.leaf(c)?;
                }
            } else if let Some((_, Some((at, kind)))) = stack.pop() {
                self.close(at, kind)?;
            }
        }
        Ok(())
    }
    pub fn finish(mut self) -> Syntax {
        if self.source.capacity() > self.limits.input_bytes {
            self.source.shrink_to_fit();
        }
        let source = Source::Owned(Arc::new(self.source));
        Syntax {
            source_start: source.as_ptr() as usize,
            source_bytes: source.len(),
            source,
            nodes: self.nodes,
            decoded: self.decoded,
            decoded_bytes: self.decoded_bytes,
            dimensions: self.dimensions,
            diagnostics: Vec::new(),
            limits: self.limits,
            blocks: Default::default(),
        }
    }
}
fn needs_boundary(a: Kind, ac: Option<char>, b: Kind, bc: Option<char>) -> bool {
    let name = matches!(b, Kind::Ident | Kind::Function | Kind::Url | Kind::UnclosedUrl | Kind::BadUrl);
    let number = matches!(b, Kind::Number | Kind::Integer | Kind::Percentage | Kind::IntegerPercentage | Kind::Dimension | Kind::IntegerDimension);
    let dash = b == Kind::Delim && bc == Some('-');
    match a {
        Kind::Ident => name || number || dash || b == Kind::Cdc || b == Kind::Round || b == Kind::Delim && bc == Some('>'),
        Kind::AtKeyword | Kind::Hash | Kind::IdHash | Kind::Dimension | Kind::IntegerDimension => name || number || dash || b == Kind::Cdc,
        Kind::Number | Kind::Integer => name || number || b == Kind::Cdc || b == Kind::Delim && bc == Some('%'),
        Kind::Delim => match ac {
            Some('#' | '-') => name || number || dash || b == Kind::Cdc,
            Some('@') => name || dash || b == Kind::Cdc,
            Some('.' | '+') => number,
            Some('/') => b == Kind::Delim && bc == Some('*'),
            // Prevent the three-token spelling < ! -- from forming a CDO.
            Some('<') => b == Kind::Delim && bc == Some('!'),
            _ => false,
        },
        _ => false,
    }
}
