use std::{
    borrow::Cow,
    collections::HashSet,
    sync::atomic::{AtomicBool, Ordering},
};

#[cfg(not(feature = "scalar-scans"))]
use memchr::{memchr, memchr2, memchr3};

#[cfg(feature = "scalar-scans")]
fn memchr(a: u8, bytes: &[u8]) -> Option<usize> {
    bytes.iter().position(|b| *b == a)
}
#[cfg(feature = "scalar-scans")]
fn memchr2(a: u8, b: u8, bytes: &[u8]) -> Option<usize> {
    bytes.iter().position(|x| *x == a || *x == b)
}
#[cfg(feature = "scalar-scans")]
fn memchr3(a: u8, b: u8, c: u8, bytes: &[u8]) -> Option<usize> {
    bytes.iter().position(|x| *x == a || *x == b || *x == c)
}

use super::entities::ENTITIES;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute<'a> {
    pub name: Cow<'a, str>,
    pub value: Cow<'a, str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag<'a> {
    pub name: Cow<'a, str>,
    pub attributes: Vec<Attribute<'a>>,
    pub self_closing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Doctype<'a> {
    pub name: Option<Cow<'a, str>>,
    pub public_id: Option<Cow<'a, str>>,
    pub system_id: Option<Cow<'a, str>>,
    pub force_quirks: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token<'a> {
    StartTag(Tag<'a>),
    EndTag(Tag<'a>),
    Text(Cow<'a, str>),
    Comment(Cow<'a, str>),
    ProcessingInstruction { target: Cow<'a, str>, data: Cow<'a, str> },
    Doctype(Doctype<'a>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Data,
    Rcdata(String),
    RawText(String),
    ScriptData(String),
    Plaintext,
    Cdata,
}

/// Limits apply before allocating. Caller runs parsing on a worker;
/// cancellation is also checked inside long attribute and script runs.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub input_bytes: usize,
    pub tokens: usize,
    pub attributes_per_tag: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { input_bytes: 32 * 1024 * 1024, tokens: 2_000_000, attributes_per_tag: 4096 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InputLimit,
    TokenLimit,
    AttributeLimit,
    Cancelled,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InputLimit => "HTML input byte limit exceeded",
            Self::TokenLimit => "HTML token limit exceeded",
            Self::AttributeLimit => "HTML attribute limit exceeded",
            Self::Cancelled => "HTML parsing cancelled",
        })
    }
}

impl std::error::Error for Error {}

pub struct Tokenizer<'a> {
    input: &'a str,
    pos: usize,
    mode: Mode,
    limits: Limits,
    emitted: usize,
    cancelled: Option<&'a AtomicBool>,
    finished: bool,
}

impl<'a> Tokenizer<'a> {
    pub fn new(input: &'a str, limits: Limits) -> Result<Self, Error> {
        if input.len() > limits.input_bytes {
            return Err(Error::InputLimit);
        }
        Ok(Self { input, pos: 0, mode: Mode::Data, limits, emitted: 0, cancelled: None, finished: false })
    }

    pub fn with_cancellation(mut self, cancelled: &'a AtomicBool) -> Self {
        self.cancelled = Some(cancelled);
        self
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }
    pub fn position(&self) -> usize {
        self.pos
    }

    fn check_cancelled(&self) -> Result<(), Error> {
        if self.cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
    }

    /// Borrows unchanged names, values and text from the source. Tokens
    /// need not be retained: tree construction consumes one at a time.
    pub fn next_token(&mut self) -> Result<Option<Token<'a>>, Error> {
        if self.finished {
            return Ok(None);
        }
        self.check_cancelled()?;
        if self.pos == self.input.len() {
            self.finished = true;
            return Ok(None);
        }
        if self.emitted >= self.limits.tokens {
            self.finished = true;
            return Err(Error::TokenLimit);
        }
        let token = self.read_token();
        if token.is_err() {
            self.finished = true;
        }
        if matches!(token, Ok(Some(_))) {
            self.emitted += 1;
        }
        token
    }

    fn read_token(&mut self) -> Result<Option<Token<'a>>, Error> {
        loop {
            if self.pos == self.input.len() {
                return Ok(None);
            }
            if self.mode != Mode::Data {
                if let Some(text) = self.raw_text()? {
                    return Ok(Some(Token::Text(text)));
                }
                self.mode = Mode::Data;
                continue;
            }
            let start = self.pos;
            let tail = &self.input[start..];
            if !tail.starts_with('<') {
                self.pos += memchr(b'<', tail.as_bytes()).unwrap_or(tail.len());
                return Ok(Some(Token::Text(decode(&self.input[start..self.pos], true, false, false))));
            }
            if tail.starts_with("<!--") {
                self.pos += 4;
                return Ok(Some(Token::Comment(self.comment()?)));
            }
            if starts_ascii(tail, "<!doctype") {
                self.pos += 9;
                return Ok(Some(Token::Doctype(self.doctype()?)));
            }
            if tail.starts_with("<?") {
                self.pos += 2;
                return Ok(self.processing_instruction());
            }
            if tail.starts_with("<!") {
                self.pos += 2;
                return Ok(Some(Token::Comment(self.bogus_comment())));
            }
            let end_tag = tail.starts_with("</");
            let name_at = start + if end_tag { 2 } else { 1 };
            if self.input.as_bytes().get(name_at).is_some_and(u8::is_ascii_alphabetic) {
                self.pos = name_at;
                return self.tag(end_tag);
            }
            if end_tag && name_at < self.input.len() {
                self.pos = name_at;
                if self.byte() == Some(b'>') {
                    self.pos += 1;
                    continue;
                }
                return Ok(Some(Token::Comment(self.bogus_comment())));
            }
            // An invalid tag opener is text, reconsume everything after '<'.
            self.pos += 1;
            return Ok(Some(Token::Text(Cow::Borrowed(&self.input[start..self.pos]))));
        }
    }

    fn byte(&self) -> Option<u8> {
        self.input.as_bytes().get(self.pos).copied()
    }

    fn skip_space(&mut self) {
        while self.byte().is_some_and(space) {
            self.pos += 1;
        }
    }

    fn tag(&mut self, end: bool) -> Result<Option<Token<'a>>, Error> {
        let start = self.pos;
        while let Some(b) = self.byte() {
            if space(b) || b == b'/' || b == b'>' {
                break;
            }
            self.pos += 1;
        }
        let tag_name = name(&self.input[start..self.pos]);
        let mut attributes = Vec::new();
        let mut attribute_names: Option<HashSet<Cow<'a, str>>> = None;
        let mut self_closing = false;
        let mut seen = 0;
        loop {
            self.check_cancelled()?;
            self.skip_space();
            match self.byte() {
                None => return Ok(None), // EOF in a tag discards that tag.
                Some(b'>') => {
                    self.pos += 1;
                    break;
                }
                Some(b'/') => {
                    self.pos += 1;
                    if self.byte() == Some(b'>') {
                        self_closing = true;
                        self.pos += 1;
                        break;
                    }
                    continue;
                }
                _ => {}
            }
            seen += 1;
            if seen > self.limits.attributes_per_tag {
                return Err(Error::AttributeLimit);
            }
            let at = self.pos;
            self.pos += 1; // '=' may itself begin an erroneous attribute name.
            while let Some(b) = self.byte() {
                if space(b) || matches!(b, b'=' | b'/' | b'>') {
                    break;
                }
                self.pos += 1;
            }
            let attr_name = name(&self.input[at..self.pos]);
            self.skip_space();
            let value = if self.byte() == Some(b'=') {
                self.pos += 1;
                self.skip_space();
                let quote = self.byte().filter(|b| matches!(b, b'\'' | b'"'));
                if quote.is_some() {
                    self.pos += 1;
                }
                let at = self.pos;
                if let Some(q) = quote {
                    self.pos += memchr(q, &self.input.as_bytes()[at..]).unwrap_or(self.input.len() - at);
                } else {
                    while let Some(b) = self.byte() {
                        if space(b) || b == b'>' {
                            break;
                        }
                        if self.pos & 4095 == 0 {
                            self.check_cancelled()?;
                        }
                        self.pos += 1;
                    }
                }
                let value = decode(&self.input[at..self.pos], true, true, true);
                if quote.is_some() && self.byte().is_some() {
                    self.pos += 1;
                }
                value
            } else {
                Cow::Borrowed("")
            };
            // First wins, including differently cased duplicate names.
            // Small tags avoid a hash allocation. Large tags avoid the
            // quadratic duplicate scan (an untrusted-input DoS otherwise).
            if attributes.len() == 8 && attribute_names.is_none() {
                attribute_names = Some(attributes.iter().map(|a: &Attribute<'_>| a.name.clone()).collect());
            }
            let unique = if let Some(names) = &mut attribute_names { names.insert(attr_name.clone()) } else { !attributes.iter().any(|a: &Attribute<'_>| a.name == attr_name) };
            if unique {
                attributes.push(Attribute { name: attr_name, value });
            }
        }
        let tag = Tag { name: tag_name, attributes, self_closing };
        Ok(Some(if end { Token::EndTag(tag) } else { Token::StartTag(tag) }))
    }

    fn bogus_comment(&mut self) -> Cow<'a, str> {
        let start = self.pos;
        self.pos += memchr(b'>', &self.input.as_bytes()[start..]).unwrap_or(self.input.len() - start);
        let text = decode(&self.input[start..self.pos], false, false, true);
        if self.byte().is_some() {
            self.pos += 1;
        }
        text
    }

    fn processing_instruction(&mut self) -> Option<Token<'a>> {
        let at = self.pos;
        self.byte()?;
        let first_valid = self.byte().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_');
        if !first_valid {
            self.pos = at - 1;
            return Some(Token::Comment(self.bogus_comment()));
        }
        while self.byte().is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')) {
            self.pos += 1;
        }
        self.byte()?;
        let target = &self.input[at..self.pos];
        if !first_valid || self.byte().is_some_and(|b| !space(b) && !matches!(b, b'?' | b'>')) || target.eq_ignore_ascii_case("xml") || target.eq_ignore_ascii_case("xml-stylesheet") {
            self.pos = at - 1;
            return Some(Token::Comment(self.bogus_comment()));
        }
        self.skip_space();
        let at = self.pos;
        self.pos += memchr(b'>', &self.input.as_bytes()[at..]).unwrap_or(self.input.len() - at);
        self.byte()?;
        let data = self.input[at..self.pos].strip_suffix('?').unwrap_or(&self.input[at..self.pos]);
        self.pos += 1;
        Some(Token::ProcessingInstruction { target: Cow::Borrowed(target), data: decode(data, false, false, false) })
    }

    fn comment(&mut self) -> Result<Cow<'a, str>, Error> {
        let start = self.pos;
        if self.byte() == Some(b'>') {
            self.pos += 1;
            return Ok(Cow::Borrowed(""));
        }
        if self.input[start..].starts_with("->") {
            self.pos += 2;
            return Ok(Cow::Borrowed(""));
        }
        let mut scan = start;
        while scan < self.input.len() {
            self.check_cancelled()?;
            let Some(i) = memchr(b'-', &self.input.as_bytes()[scan..]) else { break };
            scan += i;
            let tail = &self.input[scan..];
            let close = if tail.starts_with("-->") {
                3
            } else if tail.starts_with("--!>") {
                4
            } else {
                0
            };
            if close != 0 {
                self.pos = scan + close;
                return Ok(decode(&self.input[start..scan], false, false, true));
            }
            scan += 1;
        }
        self.pos = self.input.len();
        // Pending comment-end characters at EOF are not emitted.
        let text = &self.input[start..];
        let text = text.strip_suffix("--!").or_else(|| text.strip_suffix("--")).or_else(|| text.strip_suffix('-')).unwrap_or(text);
        Ok(decode(text, false, false, true))
    }

    fn doctype(&mut self) -> Result<Doctype<'a>, Error> {
        let mut token = Doctype { name: None, public_id: None, system_id: None, force_quirks: false };
        self.skip_space();
        let start = self.pos;
        while let Some(b) = self.byte() {
            if space(b) || b == b'>' {
                break;
            }
            self.pos += 1;
        }
        if start == self.pos {
            token.force_quirks = true;
        } else {
            token.name = Some(name(&self.input[start..self.pos]));
        }
        self.skip_space();
        if self.byte() == Some(b'>') {
            self.pos += 1;
            return Ok(token);
        }
        if self.byte().is_none() {
            token.force_quirks = true;
            return Ok(token);
        }
        let public = starts_ascii(&self.input[self.pos..], "public");
        let system = starts_ascii(&self.input[self.pos..], "system");
        if !public && !system {
            token.force_quirks = true;
            self.bogus_comment();
            return Ok(token);
        }
        self.pos += 6;
        self.skip_space();
        let Some((id, closed)) = self.doctype_id() else {
            token.force_quirks = true;
            self.bogus_comment();
            return Ok(token);
        };
        token.force_quirks |= !closed;
        if public {
            token.public_id = Some(id);
        } else {
            token.system_id = Some(id);
        }
        self.skip_space();
        if public && self.byte().is_some_and(|b| matches!(b, b'\'' | b'"')) {
            if let Some((id, closed)) = self.doctype_id() {
                token.system_id = Some(id);
                token.force_quirks |= !closed;
            }
            self.skip_space();
        }
        match self.byte() {
            Some(b'>') => {
                self.pos += 1;
            }
            None => {
                token.force_quirks = true;
            }
            _ => {
                // After a complete system identifier, garbage is ignored
                // without forcing quirks (the bogus-doctype state).
                if token.system_id.is_none() {
                    token.force_quirks = true;
                }
                self.bogus_comment();
            }
        }
        self.check_cancelled()?;
        Ok(token)
    }

    fn doctype_id(&mut self) -> Option<(Cow<'a, str>, bool)> {
        let q = self.byte()?;
        if !matches!(q, b'\'' | b'"') {
            return None;
        }
        self.pos += 1;
        let at = self.pos;
        self.pos += memchr2(q, b'>', &self.input.as_bytes()[at..]).unwrap_or(self.input.len() - at);
        let id = decode(&self.input[at..self.pos], false, false, true);
        let closed = self.byte() == Some(q);
        if closed {
            self.pos += 1;
        }
        Some((id, closed))
    }

    fn raw_text(&mut self) -> Result<Option<Cow<'a, str>>, Error> {
        let start = self.pos;
        if self.mode == Mode::Cdata {
            let remaining = &self.input[start..];
            let end = remaining.find("]]>").unwrap_or(remaining.len());
            self.pos += end + if end < remaining.len() { 3 } else { 0 };
            self.mode = Mode::Data;
            return Ok((end > 0).then(|| decode(&remaining[..end], false, false, false)));
        }
        if self.mode == Mode::Plaintext {
            self.pos = self.input.len();
            return Ok(Some(decode(&self.input[start..], false, false, true)));
        }
        let (end, refs, script) = match &self.mode {
            Mode::Rcdata(end) => (end.as_str(), true, false),
            Mode::RawText(end) => (end.as_str(), false, false),
            Mode::ScriptData(end) => (end.as_str(), false, true),
            _ => unreachable!(),
        };
        let mut scan = start;
        let mut escaped = false;
        let mut double = false;
        while scan < self.input.len() {
            self.check_cancelled()?;
            let Some(i) = memchr2(b'<', b'-', &self.input.as_bytes()[scan..]) else { break };
            scan += i;
            let tail = &self.input[scan..];
            if script && escaped && tail.starts_with("-->") {
                escaped = false;
                double = false;
                scan += 3;
                continue;
            }
            if script && !escaped && tail.starts_with("<!--") {
                escaped = true;
                scan += 4;
                continue;
            }
            if tail.starts_with("</") && appropriate_end(&tail[2..], end) {
                if double {
                    double = false;
                    scan += 2 + end.len();
                    continue;
                }
                self.pos = scan;
                return Ok((scan > start).then(|| decode(&self.input[start..scan], refs, false, true)));
            }
            if script && escaped && !double && tail.starts_with('<') && appropriate_end(&tail[1..], "script") {
                double = true;
                scan += 7;
                continue;
            }
            scan += 1;
        }
        self.pos = self.input.len();
        Ok(Some(decode(&self.input[start..], refs, false, true)))
    }
}

fn space(b: u8) -> bool {
    matches!(b, b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}
fn starts_ascii(text: &str, prefix: &str) -> bool {
    text.as_bytes().get(..prefix.len()).is_some_and(|s| s.eq_ignore_ascii_case(prefix.as_bytes()))
}
fn appropriate_end(text: &str, end: &str) -> bool {
    !end.is_empty() && starts_ascii(text, end) && text.as_bytes().get(end.len()).is_some_and(|b| space(*b) || matches!(b, b'/' | b'>'))
}

fn name(text: &str) -> Cow<'_, str> {
    let decoded = decode(text, false, false, true);
    if decoded.bytes().any(|b| b.is_ascii_uppercase()) { Cow::Owned(decoded.to_ascii_lowercase()) } else { decoded }
}

/// The no-transformation path returns the input slice without allocating.
fn decode(text: &str, refs: bool, attribute: bool, replace_null: bool) -> Cow<'_, str> {
    let changes = memchr3(b'\r', if replace_null { 0 } else { b'\r' }, if refs { b'&' } else { b'\r' }, text.as_bytes()).is_some();
    if !changes {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    while pos < text.len() {
        match text.as_bytes()[pos] {
            b'\r' => {
                out.push('\n');
                pos += 1 + usize::from(text.as_bytes().get(pos + 1) == Some(&b'\n'));
            }
            0 if replace_null => {
                out.push('\u{fffd}');
                pos += 1;
            }
            b'&' if refs => {
                if let Some((used, value)) = reference(&text[pos + 1..], attribute) {
                    out.push_str(&value);
                    pos += 1 + used;
                } else {
                    out.push('&');
                    pos += 1;
                }
            }
            _ => {
                let start = pos;
                pos += 1;
                while pos < text.len() && !matches!(text.as_bytes()[pos], b'\r' | 0 | b'&') {
                    pos += 1;
                }
                // Delimiters are ASCII, so both ends are UTF-8 boundaries.
                out.push_str(&text[start..pos]);
            }
        }
    }
    Cow::Owned(out)
}

fn reference(text: &str, attribute: bool) -> Option<(usize, Cow<'static, str>)> {
    if let Some(number) = text.strip_prefix('#') {
        let hex = number.starts_with(['x', 'X']);
        let mut pos = 1 + usize::from(hex);
        let start = pos;
        let mut value = 0u32;
        while let Some(b) = text.as_bytes().get(pos) {
            let digit = match b {
                b'0'..=b'9' => (b - b'0') as u32,
                b'a'..=b'f' if hex => (b - b'a' + 10) as u32,
                b'A'..=b'F' if hex => (b - b'A' + 10) as u32,
                _ => break,
            };
            value = value.saturating_mul(if hex { 16 } else { 10 }).saturating_add(digit);
            pos += 1;
        }
        if pos == start {
            return None;
        }
        if text.as_bytes().get(pos) == Some(&b';') {
            pos += 1;
        }
        const C1: [u32; 32] = [
            0x20ac, 0x81, 0x201a, 0x192, 0x201e, 0x2026, 0x2020, 0x2021, 0x2c6, 0x2030, 0x160, 0x2039, 0x152, 0x8d, 0x17d, 0x8f, 0x90, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014, 0x2dc,
            0x2122, 0x161, 0x203a, 0x153, 0x9d, 0x17e, 0x178,
        ];
        if (0x80..=0x9f).contains(&value) {
            value = C1[(value - 0x80) as usize];
        }
        if value == 0 {
            value = 0xfffd;
        }
        let c = char::from_u32(value).unwrap_or('\u{fffd}');
        return Some((pos, Cow::Owned(c.to_string())));
    }
    // HTML's longest named reference is 32 ASCII bytes including ';'.
    // Bound the lookahead even on megabytes of attacker-controlled text.
    let end = text.bytes().take(32).take_while(|b| b.is_ascii_alphanumeric()).count();
    let end = end + usize::from(text.as_bytes().get(end) == Some(&b';'));
    for len in (1..=end).rev() {
        let candidate = &text[..len];
        if let Ok(i) = ENTITIES.binary_search_by_key(&candidate, |(key, _)| *key) {
            if !candidate.ends_with(';') && attribute && text.as_bytes().get(len).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'=') {
                return None;
            }
            return Some((len, Cow::Borrowed(ENTITIES[i].1)));
        }
    }
    None
}
