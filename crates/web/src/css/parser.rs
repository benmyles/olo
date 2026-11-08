//! Generic CSS rule/declaration boundaries. Property and selector grammar
//! validation is a separate stage; these results must not be applied as style.
use super::{Component, Components, Error, Kind, Result, Syntax};
use std::{
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntaxError {
    Empty,
    Invalid,
    ExtraInput,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatementKind {
    AtRule,
    QualifiedRule,
    Declaration,
    Invalid(SyntaxError),
}
#[derive(Debug)]
pub enum ParseError {
    Resource(Error),
    Syntax(SyntaxError),
}
impl From<Error> for ParseError {
    fn from(e: Error) -> Self {
        Self::Resource(e)
    }
}
impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CSS parse {self:?}")
    }
}
impl std::error::Error for ParseError {}
#[derive(Clone)]
pub struct Statement<'a> {
    tree: &'a Syntax,
    kind: StatementKind,
    name: Option<u32>,
    value: Range<u32>,
    block: Option<Range<u32>>,
    important: bool,
}
impl<'a> Statement<'a> {
    pub fn kind(&self) -> StatementKind {
        self.kind
    }
    pub fn name(&self) -> Option<&'a str> {
        self.name.and_then(|n| self.tree.component(n).text())
    }
    pub fn value(&self) -> Components<'a> {
        self.tree.range(self.value.clone())
    }
    pub fn block(&self) -> Option<Components<'a>> {
        self.block.clone().map(|r| self.tree.range(r))
    }
    pub fn important(&self) -> bool {
        self.important
    }
    /// The sheet it is in.
    pub fn sheet(&self) -> &'a Syntax {
        self.tree
    }
    /// Where its block starts in its sheet: the block's identity there.
    pub fn block_start(&self) -> Option<u32> {
        self.block.as_ref().map(|r| r.start)
    }
    /// Parse this rule's block once, then retain the results in the style
    /// compiler; do not repeat it during layout or painting.
    pub fn block_contents(&self, cancelled: Option<&AtomicBool>) -> Result<Vec<Statement<'a>>> {
        match &self.block {
            Some(r) => self.tree.list(r.clone(), ListMode::Mixed, cancelled),
            None => Ok(Vec::new()),
        }
    }
}
#[derive(Clone, Copy)]
enum ListMode {
    Stylesheet,
    Rules,
    Declarations,
    Mixed,
}
impl Syntax {
    pub fn stylesheet(&self, cancelled: Option<&AtomicBool>) -> Result<Vec<Statement<'_>>> {
        self.list(self.all_range(), ListMode::Stylesheet, cancelled)
    }
    pub fn rule_list(&self, cancelled: Option<&AtomicBool>) -> Result<Vec<Statement<'_>>> {
        self.list(self.all_range(), ListMode::Rules, cancelled)
    }
    pub fn declarations(&self, cancelled: Option<&AtomicBool>) -> Result<Vec<Statement<'_>>> {
        self.list(self.all_range(), ListMode::Declarations, cancelled)
    }
    pub fn block_contents(&self, cancelled: Option<&AtomicBool>) -> Result<Vec<Statement<'_>>> {
        self.list(self.all_range(), ListMode::Mixed, cancelled)
    }
    pub fn one_component(&self) -> std::result::Result<Component<'_>, SyntaxError> {
        let mut values = self.components().filter(|c| c.kind() != Kind::Whitespace);
        let value = values.next().ok_or(SyntaxError::Empty)?;
        if values.next().is_some() { Err(SyntaxError::ExtraInput) } else { Ok(value) }
    }
    pub fn one_rule(&self, cancelled: Option<&AtomicBool>) -> std::result::Result<Statement<'_>, ParseError> {
        let mut at = self.skip_whitespace(0, self.all_range().end);
        let end = self.all_range().end;
        if at == end {
            return Err(ParseError::Syntax(SyntaxError::Empty));
        }
        let rule = self.consume_rule(&mut at, end, false, false, cancelled)?;
        if let StatementKind::Invalid(e) = rule.kind {
            return Err(ParseError::Syntax(e));
        }
        if self.skip_whitespace(at, end) != end {
            return Err(ParseError::Syntax(SyntaxError::ExtraInput));
        }
        Ok(rule)
    }
    pub fn one_declaration(&self, cancelled: Option<&AtomicBool>) -> std::result::Result<Statement<'_>, ParseError> {
        let end = self.all_range().end;
        let start = self.skip_whitespace(0, end);
        if start == end {
            return Err(ParseError::Syntax(SyntaxError::Empty));
        }
        let mut stop = end;
        let mut after = end;
        for c in self.range(start..end) {
            check(cancelled)?;
            if c.kind() == Kind::Semicolon {
                stop = c.at;
                after = c.after();
                break;
            }
        }
        let declaration = self.declaration(start..stop, cancelled)?.ok_or(ParseError::Syntax(SyntaxError::Invalid))?;
        if self.skip_whitespace(after, end) != end {
            return Err(ParseError::Syntax(SyntaxError::ExtraInput));
        }
        Ok(declaration)
    }
    fn invalid(&self) -> Statement<'_> {
        Statement { tree: self, kind: StatementKind::Invalid(SyntaxError::Invalid), name: None, value: 0..0, block: None, important: false }
    }
    fn skip_whitespace(&self, mut at: u32, end: u32) -> u32 {
        while at < end && self.component(at).kind() == Kind::Whitespace {
            at = self.component(at).after();
        }
        at
    }
    fn consume_rule<'a>(&'a self, at: &mut u32, end: u32, nested: bool, stop_semicolon: bool, cancelled: Option<&AtomicBool>) -> Result<Statement<'a>> {
        let start = *at;
        let first = self.component(start);
        let is_at = first.kind() == Kind::AtKeyword;
        let prelude = if is_at { first.after() } else { start };
        *at = prelude;
        loop {
            check(cancelled)?;
            if *at >= end {
                return Ok(if is_at { Statement { tree: self, kind: StatementKind::AtRule, name: Some(start), value: prelude..end, block: None, important: false } } else { self.invalid() });
            }
            let c = self.component(*at);
            if c.kind() == Kind::Semicolon && (is_at || stop_semicolon) {
                let before = *at;
                *at = c.after();
                return Ok(if is_at { Statement { tree: self, kind: StatementKind::AtRule, name: Some(start), value: prelude..before, block: None, important: false } } else { self.invalid() });
            }
            if nested && c.kind() == Kind::Close && c.character() == Some('}') {
                return Ok(if is_at { Statement { tree: self, kind: StatementKind::AtRule, name: Some(start), value: prelude..*at, block: None, important: false } } else { self.invalid() });
            }
            if c.kind() == Kind::Curly {
                let before = *at;
                *at = c.after();
                if !is_at {
                    let p = self.skip_whitespace(prelude, before);
                    if p < before && self.component(p).kind() == Kind::Ident && self.component(p).text().unwrap().starts_with("--") {
                        let next = self.skip_whitespace(self.component(p).after(), before);
                        if next < before && self.component(next).kind() == Kind::Colon {
                            return Ok(self.invalid());
                        }
                    }
                }
                return Ok(Statement {
                    tree: self,
                    kind: if is_at { StatementKind::AtRule } else { StatementKind::QualifiedRule },
                    name: is_at.then_some(start),
                    value: prelude..before,
                    block: Some(c.child_range()),
                    important: false,
                });
            }
            *at = c.after();
        }
    }
    fn declaration<'a>(&'a self, range: Range<u32>, cancelled: Option<&AtomicBool>) -> Result<Option<Statement<'a>>> {
        let start = self.skip_whitespace(range.start, range.end);
        if start == range.end || self.component(start).kind() != Kind::Ident {
            return Ok(None);
        }
        let colon = self.skip_whitespace(self.component(start).after(), range.end);
        if colon == range.end || self.component(colon).kind() != Kind::Colon {
            return Ok(None);
        }
        let value = self.skip_whitespace(self.component(colon).after(), range.end);
        let mut previous = None;
        let mut last = None;
        let mut before_previous = None;
        let mut block_count = 0usize;
        let mut other_count = 0usize;
        for c in self.range(value..range.end) {
            check(cancelled)?;
            if c.kind() != Kind::Whitespace {
                before_previous = previous;
                previous = last;
                last = Some(c);
                if c.kind() == Kind::Curly {
                    block_count += 1;
                } else {
                    other_count += 1;
                }
            }
        }
        let important =
            matches!((previous,last), (Some(a),Some(b)) if a.kind()==Kind::Delim && a.character()==Some('!') && b.kind()==Kind::Ident && b.text().unwrap().eq_ignore_ascii_case("important"));
        let trimmed = if important { before_previous.map_or(value, Component::after) } else { last.map_or(value, Component::after) };
        if important {
            other_count -= 2;
        }
        if !self.component(start).text().unwrap().starts_with("--") && block_count > 0 && (other_count > 0 || block_count > 1) {
            return Ok(None);
        }
        Ok(Some(Statement { tree: self, kind: StatementKind::Declaration, name: Some(start), value: value..trimmed, block: None, important }))
    }
    fn list<'a>(&'a self, range: Range<u32>, mode: ListMode, cancelled: Option<&AtomicBool>) -> Result<Vec<Statement<'a>>> {
        let mut at = range.start;
        let end = range.end;
        let mut output = Vec::new();
        while at < end {
            check(cancelled)?;
            let c = self.component(at);
            if c.kind() == Kind::Whitespace
                || matches!(mode, ListMode::Stylesheet) && matches!(c.kind(), Kind::Cdo | Kind::Cdc)
                || matches!(mode, ListMode::Declarations | ListMode::Mixed) && c.kind() == Kind::Semicolon
            {
                at = c.after();
                continue;
            }
            if matches!(mode, ListMode::Mixed) && c.kind() == Kind::Close && c.character() == Some('}') {
                break;
            }
            if output.len() >= self.item_limit() {
                return Err(Error::ItemLimit);
            }
            if c.kind() == Kind::AtKeyword || matches!(mode, ListMode::Stylesheet | ListMode::Rules) {
                let rule = self.consume_rule(&mut at, end, matches!(mode, ListMode::Mixed), false, cancelled)?;
                if !matches!(mode, ListMode::Declarations) {
                    output.push(rule);
                }
                continue;
            }
            let start = at;
            let mut stop = at;
            let header = self.skip_whitespace(c.after(), end);
            let candidate = c.kind() == Kind::Ident && header < end && self.component(header).kind() == Kind::Colon;
            let custom = c.kind() == Kind::Ident && c.text().unwrap().starts_with("--");
            let mut block_seen = false;
            let mut non_block_seen = false;
            let value_start = if candidate { self.skip_whitespace(self.component(header).after(), end) } else { at };
            let mut possible = candidate;
            if cfg!(feature = "baseline-css-parser") || candidate || matches!(mode, ListMode::Declarations) {
                while stop < end {
                    check(cancelled)?;
                    let v = self.component(stop);
                    if v.kind() == Kind::Semicolon || matches!(mode, ListMode::Mixed) && v.kind() == Kind::Close && v.character() == Some('}') {
                        break;
                    }
                    if !cfg!(feature = "baseline-css-parser") && matches!(mode, ListMode::Mixed) && !custom && stop >= value_start && v.kind() != Kind::Whitespace {
                        if v.kind() == Kind::Curly {
                            if block_seen || non_block_seen {
                                possible = false;
                                break;
                            }
                            block_seen = true;
                        } else {
                            if block_seen {
                                // A sole {} block may be followed by !important.
                                if v.kind() == Kind::Delim && v.character() == Some('!') {
                                    let important = self.skip_whitespace(v.after(), end);
                                    if important < end && self.component(important).kind() == Kind::Ident && self.component(important).text().unwrap().eq_ignore_ascii_case("important") {
                                        let terminator = self.skip_whitespace(self.component(important).after(), end);
                                        if terminator == end
                                            || self.component(terminator).kind() == Kind::Semicolon
                                            || self.component(terminator).kind() == Kind::Close && self.component(terminator).character() == Some('}')
                                        {
                                            stop = terminator;
                                            break;
                                        }
                                    }
                                }
                                possible = false;
                                break;
                            }
                            non_block_seen = true;
                        }
                    }
                    stop = v.after();
                }
            }
            if let Some(decl) = if possible { self.declaration(start..stop, cancelled)? } else { None } {
                output.push(decl);
                at = stop;
            } else if matches!(mode, ListMode::Mixed) {
                output.push(self.consume_rule(&mut at, end, true, true, cancelled)?);
                if at == start {
                    at = c.after();
                }
            } else {
                output.push(self.invalid());
                at = stop;
            }
        }
        Ok(output)
    }
}
fn check(cancelled: Option<&AtomicBool>) -> Result<()> {
    if cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
