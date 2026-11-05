//! CSS Syntax 3 tokenization, following the 2026 editor's draft. Source
//! offsets stay in original UTF-8 bytes; preprocessing happens on reads.
use super::{Error, Limits, Result};
use std::{
    borrow::Cow,
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    EofInComment,
    EofInString,
    NewlineInString,
    EofInUrl,
    BadUrl,
    InvalidEscape,
    EofInEscape,
    UnclosedBlock,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub offset: u32,
    pub kind: DiagnosticKind,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Numeric<'a> {
    pub value: f64,
    pub integer: bool,
    pub representation: &'a str,
}
#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind<'a> {
    Ident(Cow<'a, str>),
    Function(Cow<'a, str>),
    AtKeyword(Cow<'a, str>),
    Hash { value: Cow<'a, str>, id: bool },
    String { value: Cow<'a, str>, closed: bool },
    BadString,
    Url { value: Cow<'a, str>, closed: bool },
    BadUrl,
    Number(Numeric<'a>),
    Percentage(Numeric<'a>),
    Dimension { number: Numeric<'a>, unit: Cow<'a, str> },
    UnicodeRange { start: u32, end: u32 },
    Whitespace,
    Delim(char),
    Colon,
    Semicolon,
    Comma,
    Open(char),
    Close(char),
    Cdo,
    Cdc,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Token<'a> {
    pub kind: TokenKind<'a>,
    pub span: Range<u32>,
}

pub struct Tokenizer<'a> {
    input: &'a str,
    pos: usize,
    limits: Limits,
    cancelled: Option<&'a AtomicBool>,
    emitted: usize,
    decoded: usize,
    diagnostics: Vec<Diagnostic>,
    unicode_ranges: bool,
    failed: bool,
    next_check: usize,
}
impl<'a> Tokenizer<'a> {
    pub fn new(input: &'a str, limits: Limits, cancelled: Option<&'a AtomicBool>) -> Result<Self> {
        if input.len() > limits.input_bytes || input.len() >= u32::MAX as usize {
            return Err(Error::InputLimit);
        }
        Ok(Self { input, pos: 0, limits, cancelled, emitted: 0, decoded: 0, diagnostics: Vec::new(), unicode_ranges: false, failed: false, next_check: 0 })
    }
    /// Only enable for the unicode-range descriptor's special grammar.
    pub fn set_unicode_ranges(&mut self, enabled: bool) {
        self.unicode_ranges = enabled;
    }
    pub fn position(&self) -> usize {
        self.pos
    }
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
    pub(super) fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }
    fn check(&self) -> Result<()> {
        if self.cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
    }
    fn checkpoint(&mut self) -> Result<()> {
        if self.pos >= self.next_check {
            self.check()?;
            self.next_check = self.pos.saturating_add(4096);
        }
        Ok(())
    }
    fn diagnose(&mut self, kind: DiagnosticKind, offset: usize) -> Result<()> {
        if self.diagnostics.len() >= self.limits.diagnostics {
            return Err(Error::DiagnosticLimit);
        }
        self.diagnostics.push(Diagnostic { offset: offset as u32, kind });
        Ok(())
    }
    fn peek(&self) -> Option<char> {
        self.at(self.pos).map(|(c, _)| c)
    }
    fn at(&self, pos: usize) -> Option<(char, usize)> {
        let c = self.input.get(pos..)?.chars().next()?;
        Some(match c {
            '\r' => ('\n', 1 + usize::from(self.input.as_bytes().get(pos + 1) == Some(&b'\n'))),
            '\x0c' => ('\n', 1),
            '\0' => ('\u{fffd}', 1),
            c => (c, c.len_utf8()),
        })
    }
    fn advance(&mut self) -> Option<char> {
        let (c, n) = self.at(self.pos)?;
        self.pos += n;
        Some(c)
    }
    fn valid_escape(&self, pos: usize) -> bool {
        self.input.as_bytes().get(pos) == Some(&b'\\') && !self.at(pos + 1).is_some_and(|(c, _)| c == '\n')
    }
    fn ident_start(&self, pos: usize) -> bool {
        match self.at(pos) {
            Some(('-', n)) => self.at(pos + n).is_some_and(|(c, _)| ident_start(c) || c == '-') || self.valid_escape(pos + n),
            Some((c, _)) if ident_start(c) => true,
            Some(('\\', _)) => self.valid_escape(pos),
            _ => false,
        }
    }
    fn number_start(&self, pos: usize) -> bool {
        let b = self.input.as_bytes();
        let mut p = pos;
        if matches!(b.get(p), Some(b'+' | b'-')) {
            p += 1;
        }
        b.get(p).is_some_and(u8::is_ascii_digit) || b.get(p) == Some(&b'.') && b.get(p + 1).is_some_and(u8::is_ascii_digit)
    }
    fn space(&mut self) -> Result<()> {
        while matches!(self.input.as_bytes().get(self.pos), Some(b' ' | b'\t' | b'\n' | b'\r' | 0x0c)) {
            self.pos += 1;
            self.checkpoint()?;
        }
        Ok(())
    }
    fn escaped(&mut self) -> Result<char> {
        // Caller has consumed the reverse solidus.
        let Some(c) = self.advance() else {
            self.diagnose(DiagnosticKind::EofInEscape, self.pos)?;
            return Ok('\u{fffd}');
        };
        if let Some(mut value) = c.to_digit(16).filter(|_| c.is_ascii()) {
            for _ in 1..6 {
                let Some(c) = self.peek().filter(char::is_ascii) else { break };
                let Some(v) = c.to_digit(16) else { break };
                self.advance();
                value = value * 16 + v;
            }
            if self.peek().is_some_and(whitespace) {
                self.advance();
            }
            Ok(char::from_u32(value).filter(|&c| c != '\0').unwrap_or('\u{fffd}'))
        } else {
            Ok(c)
        }
    }
    fn append(&self, out: &mut Option<String>, start: usize, end: usize, text: &str) -> Result<()> {
        let prefix = if out.is_none() { end - start } else { 0 };
        let old = out.as_ref().map_or(0, String::len);
        let needed = old.checked_add(prefix).and_then(|n| n.checked_add(text.len())).ok_or(Error::DecodedLimit)?;
        if needed > self.limits.decoded_bytes.saturating_sub(self.decoded) {
            return Err(Error::DecodedLimit);
        }
        let remaining = self.limits.decoded_bytes.saturating_sub(self.decoded);
        let value = out.get_or_insert_with(String::new);
        if value.capacity() < needed {
            let capacity = needed.max(value.len().saturating_mul(2)).min(remaining);
            value.reserve_exact(capacity - value.len());
        }
        if prefix > 0 {
            value.push_str(&self.input[start..end]);
        }
        value.push_str(text);
        Ok(())
    }
    fn finish(&mut self, out: Option<String>, start: usize, end: usize) -> Result<Cow<'a, str>> {
        if let Some(out) = out {
            self.decoded = self.decoded.checked_add(out.len()).ok_or(Error::DecodedLimit)?;
            Ok(Cow::Owned(out))
        } else {
            Ok(Cow::Borrowed(&self.input[start..end]))
        }
    }
    fn name(&mut self) -> Result<Cow<'a, str>> {
        let start = self.pos;
        let mut out = None;
        loop {
            self.checkpoint()?;
            let run = self.pos;
            while self.input.as_bytes().get(self.pos).is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')) {
                self.pos += 1;
                self.checkpoint()?;
            }
            if self.pos > run {
                if out.is_some() {
                    self.append(&mut out, start, run, &self.input[run..self.pos])?;
                }
                continue;
            }
            let at = self.pos;
            match self.peek() {
                Some(c) if ident_continue(c) => {
                    self.advance();
                    if out.is_some() || self.input.as_bytes()[at] == 0 {
                        let mut b = [0; 4];
                        self.append(&mut out, start, at, c.encode_utf8(&mut b))?;
                    }
                }
                Some('\\') if self.valid_escape(self.pos) => {
                    self.advance();
                    let c = self.escaped()?;
                    let mut b = [0; 4];
                    self.append(&mut out, start, at, c.encode_utf8(&mut b))?;
                }
                _ => break,
            }
        }
        self.finish(out, start, self.pos)
    }
    fn string(&mut self, quote: char) -> Result<TokenKind<'a>> {
        let start = self.pos;
        let mut out = None;
        loop {
            self.check()?;
            // Bounded SIMD searches make long literals cheap and leave a
            // cancellation checkpoint at least every 64 KiB.
            let tail = &self.input[self.pos..];
            let bounded = &tail.as_bytes()[..tail.len().min(65536)];
            let mut n = find_string_special(bounded, quote as u8).unwrap_or(bounded.len());
            while !self.input.is_char_boundary(self.pos + n) {
                n -= 1;
            }
            if n > 0 {
                if out.is_some() {
                    self.append(&mut out, start, self.pos, &self.input[self.pos..self.pos + n])?;
                }
                self.pos += n;
                continue;
            }
            let at = self.pos;
            match self.peek() {
                None => {
                    self.diagnose(DiagnosticKind::EofInString, self.pos)?;
                    return Ok(TokenKind::String { value: self.finish(out, start, self.pos)?, closed: false });
                }
                Some(c) if c == quote => {
                    let value = self.finish(out, start, at)?;
                    self.advance();
                    return Ok(TokenKind::String { value, closed: true });
                }
                Some('\n') => {
                    self.diagnose(DiagnosticKind::NewlineInString, at)?;
                    return Ok(TokenKind::BadString);
                }
                Some('\\') => {
                    self.advance();
                    match self.peek() {
                        None => {
                            self.append(&mut out, start, at, "")?;
                        }
                        Some('\n') => {
                            self.advance();
                            self.append(&mut out, start, at, "")?;
                        }
                        _ => {
                            let c = self.escaped()?;
                            let mut b = [0; 4];
                            self.append(&mut out, start, at, c.encode_utf8(&mut b))?;
                        }
                    }
                }
                Some(c) => {
                    self.advance();
                    let mut b = [0; 4];
                    self.append(&mut out, start, at, c.encode_utf8(&mut b))?;
                }
            }
        }
    }
    fn bad_url(&mut self) -> Result<()> {
        loop {
            self.checkpoint()?;
            match self.advance() {
                None | Some(')') => return Ok(()),
                Some('\\') if self.valid_escape(self.pos - 1) => {
                    self.escaped()?;
                }
                _ => {}
            }
        }
    }
    fn url(&mut self) -> Result<TokenKind<'a>> {
        self.space()?;
        let start = self.pos;
        let mut out = None;
        loop {
            self.checkpoint()?;
            let at = self.pos;
            match self.peek() {
                None => {
                    self.diagnose(DiagnosticKind::EofInUrl, at)?;
                    return Ok(TokenKind::Url { value: self.finish(out, start, at)?, closed: false });
                }
                Some(')') => {
                    let value = self.finish(out, start, at)?;
                    self.advance();
                    return Ok(TokenKind::Url { value, closed: true });
                }
                Some(c) if whitespace(c) => {
                    let value = self.finish(out, start, at)?;
                    self.space()?;
                    match self.peek() {
                        Some(')') => {
                            self.advance();
                            return Ok(TokenKind::Url { value, closed: true });
                        }
                        None => {
                            self.diagnose(DiagnosticKind::EofInUrl, self.pos)?;
                            return Ok(TokenKind::Url { value, closed: false });
                        }
                        _ => {
                            self.diagnose(DiagnosticKind::BadUrl, at)?;
                            self.bad_url()?;
                            return Ok(TokenKind::BadUrl);
                        }
                    }
                }
                Some('"' | '\'' | '(' | '\x01'..='\x08' | '\x0b' | '\x0e'..='\x1f' | '\x7f') => {
                    self.diagnose(DiagnosticKind::BadUrl, at)?;
                    self.bad_url()?;
                    return Ok(TokenKind::BadUrl);
                }
                Some('\\') => {
                    if !self.valid_escape(at) {
                        self.diagnose(DiagnosticKind::BadUrl, at)?;
                        self.bad_url()?;
                        return Ok(TokenKind::BadUrl);
                    }
                    self.advance();
                    let c = self.escaped()?;
                    let mut b = [0; 4];
                    self.append(&mut out, start, at, c.encode_utf8(&mut b))?;
                }
                Some(c) => {
                    self.advance();
                    if out.is_some() || self.input.as_bytes()[at] == 0 {
                        let mut b = [0; 4];
                        self.append(&mut out, start, at, c.encode_utf8(&mut b))?;
                    }
                }
            }
        }
    }
    fn ident_like(&mut self) -> Result<TokenKind<'a>> {
        let name = self.name()?;
        if self.peek() == Some('(') {
            self.advance();
            if name.eq_ignore_ascii_case("url") {
                // Retain the last whitespace before a quote, per the
                // ident-like algorithm; quoted url() remains a function.
                while let Some((c, n)) = self.at(self.pos) {
                    if !whitespace(c) || !self.at(self.pos + n).is_some_and(|(next, _)| whitespace(next)) {
                        break;
                    }
                    self.advance();
                    self.checkpoint()?;
                }
                let quoted = matches!(self.peek(), Some('"' | '\'')) || self.at(self.pos).is_some_and(|(c, n)| whitespace(c) && matches!(self.at(self.pos + n), Some(('"' | '\'', _))));
                if !quoted {
                    return self.url();
                }
            }
            Ok(TokenKind::Function(name))
        } else {
            Ok(TokenKind::Ident(name))
        }
    }
    fn numeric(&mut self) -> Result<TokenKind<'a>> {
        let start = self.pos;
        let mut integer = true;
        if matches!(self.peek(), Some('+' | '-')) {
            self.advance();
        }
        self.digits()?;
        if self.input.as_bytes().get(self.pos) == Some(&b'.') && self.input.as_bytes().get(self.pos + 1).is_some_and(u8::is_ascii_digit) {
            self.pos += 1;
            self.digits()?;
            integer = false;
        }
        let b = self.input.as_bytes();
        if matches!(b.get(self.pos), Some(b'e' | b'E')) {
            let after = self.pos + 1 + usize::from(matches!(b.get(self.pos + 1), Some(b'+' | b'-')));
            if b.get(after).is_some_and(u8::is_ascii_digit) {
                self.pos = after;
                self.digits()?;
                integer = false;
            }
        }
        let representation = &self.input[start..self.pos];
        let number = Numeric { value: representation.parse::<f64>().expect("validated CSS number"), integer, representation };
        if self.ident_start(self.pos) {
            Ok(TokenKind::Dimension { number, unit: self.name()? })
        } else if self.peek() == Some('%') {
            self.advance();
            Ok(TokenKind::Percentage(number))
        } else {
            Ok(TokenKind::Number(number))
        }
    }
    fn digits(&mut self) -> Result<()> {
        while self.input.as_bytes().get(self.pos).is_some_and(u8::is_ascii_digit) {
            self.pos += 1;
            self.checkpoint()?;
        }
        Ok(())
    }
    fn unicode_range(&mut self) -> TokenKind<'a> {
        self.pos += 2;
        let mut start = 0u32;
        let mut n = 0;
        while n < 6 {
            let Some(v) = self.peek().filter(char::is_ascii).and_then(|c| c.to_digit(16)) else { break };
            self.advance();
            start = start * 16 + v;
            n += 1;
        }
        let mut end = start;
        let mut wildcard = false;
        while n < 6 && self.peek() == Some('?') {
            self.advance();
            start *= 16;
            end = end * 16 + 15;
            n += 1;
            wildcard = true;
        }
        if !wildcard && self.peek() == Some('-') && self.at(self.pos + 1).is_some_and(|(c, _)| c.is_ascii_hexdigit()) {
            self.pos += 1;
            end = 0;
            for _ in 0..6 {
                let Some(v) = self.peek().filter(char::is_ascii).and_then(|c| c.to_digit(16)) else { break };
                self.advance();
                end = end * 16 + v;
            }
        }
        TokenKind::UnicodeRange { start, end }
    }
    pub fn next_token(&mut self) -> Result<Option<Token<'a>>> {
        if self.failed {
            return Ok(None);
        }
        let result = self.read_token();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn read_token(&mut self) -> Result<Option<Token<'a>>> {
        self.check()?;
        loop {
            if !self.input[self.pos..].starts_with("/*") {
                break;
            }
            let start = self.pos;
            self.pos += 2;
            loop {
                self.check()?;
                let tail = &self.input.as_bytes()[self.pos..];
                let n = tail.len().min(65536);
                if let Some(at) = find_comment_end(&tail[..n]) {
                    self.pos += at + 2;
                    break;
                }
                if n == tail.len() {
                    self.pos = self.input.len();
                    self.diagnose(DiagnosticKind::EofInComment, start)?;
                    break;
                }
                self.pos += n - 1;
            }
        }
        if self.pos == self.input.len() {
            return Ok(None);
        }
        if self.emitted >= self.limits.tokens {
            return Err(Error::TokenLimit);
        }
        self.emitted += 1;
        let start = self.pos;
        let c = self.peek().unwrap();
        let kind = match c {
            c if whitespace(c) => {
                self.space()?;
                TokenKind::Whitespace
            }
            '"' | '\'' => {
                self.advance();
                self.string(c)?
            }
            '#' => {
                self.advance();
                if self.peek().is_some_and(ident_continue) || self.valid_escape(self.pos) {
                    let id = self.ident_start(self.pos);
                    TokenKind::Hash { value: self.name()?, id }
                } else {
                    TokenKind::Delim('#')
                }
            }
            '+' | '-' | '.' if self.number_start(self.pos) => self.numeric()?,
            '-' if self.input[self.pos..].starts_with("-->") => {
                self.pos += 3;
                TokenKind::Cdc
            }
            '<' if self.input[self.pos..].starts_with("<!--") => {
                self.pos += 4;
                TokenKind::Cdo
            }
            '@' => {
                self.advance();
                if self.ident_start(self.pos) { TokenKind::AtKeyword(self.name()?) } else { TokenKind::Delim('@') }
            }
            'u' | 'U' if self.unicode_ranges && self.input.as_bytes().get(self.pos + 1) == Some(&b'+') && self.at(self.pos + 2).is_some_and(|(c, _)| c == '?' || c.is_ascii_hexdigit()) => {
                self.unicode_range()
            }
            c if c.is_ascii_digit() => self.numeric()?,
            '(' | '[' | '{' => {
                self.advance();
                TokenKind::Open(c)
            }
            ')' | ']' | '}' => {
                self.advance();
                TokenKind::Close(c)
            }
            ':' => {
                self.advance();
                TokenKind::Colon
            }
            ';' => {
                self.advance();
                TokenKind::Semicolon
            }
            ',' => {
                self.advance();
                TokenKind::Comma
            }
            _ if self.ident_start(self.pos) => self.ident_like()?,
            c => {
                if c == '\\' {
                    self.diagnose(DiagnosticKind::InvalidEscape, self.pos)?;
                }
                self.advance();
                TokenKind::Delim(c)
            }
        };
        Ok(Some(Token { kind, span: start as u32..self.pos as u32 }))
    }
}
fn whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n')
}
fn ident_start(c: char) -> bool {
    c.is_ascii_alphabetic()
        || c == '_'
        || matches!(c as u32,0xb7|0xc0..=0xd6|0xd8..=0xf6|0xf8..=0x37d|0x37f..=0x1fff|0x200c|0x200d|0x203f|0x2040|0x2070..=0x218f|0x2c00..=0x2fef|0x3001..=0xd7ff|0xf900..=0xfdcf|0xfdf0..=0xfffd|0x10000..=0x10ffff)
}
fn ident_continue(c: char) -> bool {
    ident_start(c) || c.is_ascii_digit() || c == '-'
}
#[cfg(not(feature = "scalar-scans"))]
fn find_string_special(bytes: &[u8], quote: u8) -> Option<usize> {
    let first = memchr::memchr3(quote, b'\\', b'\n', bytes);
    // Never scan beyond an already-found candidate, especially on runs
    // containing many escapes; that rescans the same tail quadratically.
    memchr::memchr3(b'\r', b'\x0c', 0, &bytes[..first.unwrap_or(bytes.len())]).or(first)
}
#[cfg(feature = "scalar-scans")]
fn find_string_special(bytes: &[u8], quote: u8) -> Option<usize> {
    bytes.iter().position(|&b| b == quote || matches!(b, b'\\' | b'\n' | b'\r' | b'\x0c' | 0))
}
#[cfg(not(feature = "scalar-scans"))]
fn find_comment_end(bytes: &[u8]) -> Option<usize> {
    memchr::memmem::find(bytes, b"*/")
}
#[cfg(feature = "scalar-scans")]
fn find_comment_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|b| b == b"*/")
}
