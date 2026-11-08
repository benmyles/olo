//! Original HTML tree construction. Scripts are parsed inertly; execution,
//! encoding detection and streaming input are separate integration work.
//! Unsupported insertion algorithms fail explicitly instead of returning
//! a plausible but knowingly wrong tree. See docs/browser/TODO.md.

use super::{Doctype, Mode, Tag, Token, Tokenizer};
use crate::dom::{self, Document, Namespace, NodeId, NodeType};
use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
mod foreign;
mod fragments;
pub use fragments::{FragmentContext, ParsedFragment, parse_fragment};
mod tables;
mod templates;

// The common insertion path checks one cached byte. Update these only
// when entering/leaving a table fallback or changing the template stack.
const FOSTER_INSERTION: u8 = 1;
const TEMPLATE_INSERTION: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuirksMode {
    NoQuirks,
    LimitedQuirks,
    Quirks,
}

#[derive(Clone, Debug)]
pub struct ParseOptions {
    /// Controls noscript parsing only; this parser does not execute JS yet.
    pub scripting: bool,
    pub tokenizer_limits: super::Limits,
    pub document_limits: dom::Limits,
    pub stack_limit: usize,
    pub formatting_limit: usize,
}
impl Default for ParseOptions {
    fn default() -> Self {
        Self { scripting: true, tokenizer_limits: super::Limits::default(), document_limits: dom::Limits::default(), stack_limit: 512, formatting_limit: 512 }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    Tokenizer(super::Error),
    Document(dom::Error),
    StackLimit,
    FormattingLimit,
    ReprocessingLimit,
    Unsupported(&'static str),
}
impl From<super::Error> for ParseError {
    fn from(e: super::Error) -> Self {
        Self::Tokenizer(e)
    }
}
impl From<dom::Error> for ParseError {
    fn from(e: dom::Error) -> Self {
        Self::Document(e)
    }
}
impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTML tree construction: {self:?}")
    }
}
impl std::error::Error for ParseError {}
type Result<T> = std::result::Result<T, ParseError>;

#[derive(Debug)]
pub struct ParsedDocument {
    pub document: Document,
    pub quirks: QuirksMode,
}

/// Complete-buffer, cancellable parser for worker threads. Successful
/// return currently proves only supported static tree construction, not
/// script execution or website compatibility. Diagnostics are unfinished.
pub fn parse(source: Arc<str>, options: ParseOptions, cancelled: Option<&AtomicBool>) -> Result<ParsedDocument> {
    let mut tokenizer = Tokenizer::new(&source, options.tokenizer_limits)?;
    if let Some(cancelled) = cancelled {
        tokenizer = tokenizer.with_cancellation(cancelled);
    }
    let mut builder = Builder::new(source.clone(), &source, options, cancelled)?;
    parse_tokens(&mut builder, &mut tokenizer)?;
    Ok(ParsedDocument { document: builder.document, quirks: builder.quirks })
}

fn parse_tokens<'a>(builder: &mut Builder<'a>, tokenizer: &mut Tokenizer<'a>) -> Result<()> {
    loop {
        if builder.cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(ParseError::Tokenizer(super::Error::Cancelled));
        }
        let token = tokenizer.next_token()?;
        let eof = token.is_none();
        let previous = if builder.foreign_seen { builder.stack.last().copied() } else { None };
        builder.process(token, tokenizer)?;
        // Namespace-dependent tokenizer state follows stack changes. Text
        // and ignored tokens do not repeat a DOM lookup after an earlier SVG.
        if builder.foreign_seen && builder.stack.last().copied() != previous {
            tokenizer.set_allow_cdata(builder.adjusted_current().is_some_and(|n| builder.document.namespace(n).unwrap() != Some(Namespace::Html)));
        }
        if eof {
            break;
        }
    }
    builder.initialize_selected_content()?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Insertion {
    Initial,
    BeforeHtml,
    BeforeHead,
    Head,
    HeadNoscript,
    AfterHead,
    ColumnGroup,
    Body,
    Text,
    AfterBody,
    AfterAfterBody,
    Frameset,
    AfterFrameset,
    AfterAfterFrameset,
    Table,
    TableText,
    Caption,
    TableBody,
    Row,
    Cell,
    Template,
}
#[derive(Clone, Copy)]
enum Scope {
    Normal,
    Button,
    List,
    Table,
}
#[derive(Clone)]
struct Formatting<'a> {
    node: NodeId,
    token: Tag<'a>,
}

struct Builder<'a> {
    source_text: &'a str,
    cancelled: Option<&'a AtomicBool>,
    document: Document,
    stack: Vec<NodeId>,
    formatting: Vec<Option<Formatting<'a>>>,
    selected_content: Vec<NodeId>,
    template_modes: Vec<Insertion>,
    /// Open templates that are declarative shadow roots (`shadowrootmode`).
    declarative_shadows: Vec<crate::dom::NodeId>,
    // The builder owns/roots this temporary node until fragment finish.
    // Keep its local arena slot rather than a 24-byte optional public ID.
    fragment_context: Option<std::num::NonZeroU32>,
    head: Option<NodeId>,
    form: Option<NodeId>,
    mode: Insertion,
    original_mode: Insertion,
    frameset_ok: bool,
    ignore_lf: bool,
    insertion_flags: u8,
    foreign_seen: bool,
    pending_table_text: Cow<'a, str>,
    quirks: QuirksMode,
    options: ParseOptions,
}

fn synthetic(name: &'static str) -> Tag<'static> {
    Tag { name: Cow::Borrowed(name), attributes: Vec::new(), self_closing: false }
}
fn whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c)
}
fn formatting_name(name: &str) -> bool {
    matches!(name, "a" | "b" | "big" | "code" | "em" | "font" | "i" | "nobr" | "s" | "small" | "strike" | "strong" | "tt" | "u")
}
fn heading(name: &str) -> bool {
    matches!(name, "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
}
fn block(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "center"
            | "details"
            | "dialog"
            | "dir"
            | "div"
            | "dl"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "header"
            | "hgroup"
            | "main"
            | "menu"
            | "nav"
            | "ol"
            | "p"
            | "search"
            | "section"
            | "summary"
            | "ul"
    )
}
fn head_name(name: &str) -> bool {
    matches!(name, "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "script" | "style" | "template" | "title")
}
fn html_special(name: &str) -> bool {
    block(name)
        || heading(name)
        || matches!(
            name,
            "applet"
                | "area"
                | "base"
                | "basefont"
                | "bgsound"
                | "body"
                | "br"
                | "button"
                | "caption"
                | "col"
                | "colgroup"
                | "dd"
                | "dt"
                | "embed"
                | "form"
                | "frame"
                | "frameset"
                | "hr"
                | "html"
                | "iframe"
                | "img"
                | "input"
                | "keygen"
                | "li"
                | "link"
                | "listing"
                | "marquee"
                | "meta"
                | "noembed"
                | "noframes"
                | "noscript"
                | "object"
                | "param"
                | "plaintext"
                | "pre"
                | "script"
                | "select"
                | "source"
                | "style"
                | "table"
                | "tbody"
                | "td"
                | "template"
                | "textarea"
                | "tfoot"
                | "th"
                | "thead"
                | "title"
                | "tr"
                | "track"
                | "wbr"
                | "xmp"
        )
}

impl<'a> Builder<'a> {
    fn new(source: Arc<str>, source_text: &'a str, options: ParseOptions, cancelled: Option<&'a AtomicBool>) -> Result<Self> {
        Ok(Self {
            source_text,
            cancelled,
            document: Document::new(source, options.document_limits)?,
            stack: Vec::new(),
            formatting: Vec::new(),
            selected_content: Vec::new(),
            template_modes: Vec::new(),
            declarative_shadows: Vec::new(),
            fragment_context: None,
            head: None,
            form: None,
            mode: Insertion::Initial,
            original_mode: Insertion::Initial,
            frameset_ok: true,
            ignore_lf: false,
            insertion_flags: 0,
            foreign_seen: false,
            pending_table_text: Cow::Borrowed(""),
            quirks: QuirksMode::NoQuirks,
            options,
        })
    }
    fn current(&self) -> NodeId {
        *self.stack.last().expect("HTML root inserted")
    }
    fn fragment_element(&self) -> Option<NodeId> {
        self.fragment_context.map(|slot| self.document.handle(slot))
    }
    fn adjusted_current(&self) -> Option<NodeId> {
        if self.stack.len() == 1 && self.fragment_context.is_some() { self.fragment_element() } else { self.stack.last().copied() }
    }
    fn name(&self, node: NodeId) -> &str {
        self.document.name(node).expect("builder owns node").unwrap_or("")
    }
    fn is(&self, node: NodeId, name: &str) -> bool {
        self.document.namespace(node).unwrap() == Some(Namespace::Html) && self.name(node) == name
    }
    fn push(&mut self, node: NodeId) -> Result<()> {
        if self.stack.len() >= self.options.stack_limit {
            return Err(ParseError::StackLimit);
        }
        self.stack.push(node);
        Ok(())
    }
    fn create(&mut self, tag: &mut Tag<'a>) -> Result<NodeId> {
        let node = self.document.create_element(&tag.name, Namespace::Html)?;
        if cfg!(feature = "baseline-parser") {
            for a in tag.attributes.drain(..) {
                self.document.set_attribute(node, &a.name, a.value)?;
            }
        } else {
            self.document.initial_attributes(node, tag.attributes.drain(..).map(|a| (a.name, a.value)))?;
        }
        if tag.name == "selectedcontent" {
            self.selected_content.push(node);
        }
        Ok(node)
    }
    fn insert(&mut self, tag: &mut Tag<'a>) -> Result<NodeId> {
        let node = self.create(tag)?;
        let current = self.stack.last().copied().unwrap_or(self.document.root());
        if self.insertion_flags != 0 {
            self.insert_adjusted_node(current, node)?;
        } else {
            self.document.append_child(current, node)?;
        }
        self.push(node)?;
        Ok(node)
    }
    fn insert_text(&mut self, text: Cow<'a, str>) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let parent = self.current();
        if self.insertion_flags != 0 {
            return self.insert_adjusted_text(parent, text);
        }
        if let Some(last) = self.document.last_child(parent)?
            && self.document.node_type(last)? == NodeType::Text
        {
            self.document.append_value(last, text)?;
            return Ok(());
        }
        let text = self.document.create_text(text)?;
        self.document.append_child(parent, text)?;
        Ok(())
    }
    fn miscellaneous(&mut self, token: &mut Option<Token<'a>>, parent: NodeId) -> Result<bool> {
        let node = match token {
            Some(Token::Comment(text)) => {
                if self.mode == Insertion::TableText {
                    return Ok(false);
                }
                Some(self.document.create_comment(std::mem::replace(text, Cow::Borrowed("")))?)
            }
            Some(Token::ProcessingInstruction { target, data }) => {
                if self.mode == Insertion::TableText {
                    return Ok(false);
                }
                Some(self.document.create_processing_instruction(target, std::mem::replace(data, Cow::Borrowed("")))?)
            }
            _ => None,
        };
        if let Some(node) = node {
            let parent = if self.template_modes.is_empty() { parent } else { self.document.template_contents(parent)?.unwrap_or(parent) };
            self.document.append_child(parent, node)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn scope(&self, names: &[&str], scope: Scope) -> Option<usize> {
        for (at, &node) in self.stack.iter().enumerate().rev() {
            let (name, namespace) = self.document.element_name_and_namespace(node).unwrap();
            let html = namespace == Namespace::Html;
            if html && names.contains(&name) {
                return Some(at);
            }
            let boundary = if matches!(scope, Scope::Table) {
                html && matches!(name, "html" | "table" | "template")
            } else if html {
                matches!(name, "applet" | "caption" | "html" | "table" | "td" | "th" | "marquee" | "object" | "select" | "template")
                    || matches!(scope, Scope::Button) && name == "button"
                    || matches!(scope, Scope::List) && matches!(name, "ol" | "ul")
            } else {
                self.foreign_scope_boundary(node)
            };
            if boundary {
                return None;
            }
        }
        None
    }
    fn implied(&mut self, except: &str) {
        while let Some(&node) = self.stack.last() {
            let name = self.name(node);
            if name == except || !matches!(name, "dd" | "dt" | "li" | "optgroup" | "option" | "p" | "rb" | "rp" | "rt" | "rtc") || self.document.namespace(node).unwrap() != Some(Namespace::Html) {
                break;
            }
            self.stack.pop();
        }
    }
    fn node_in_scope(&self, target: NodeId) -> bool {
        for &node in self.stack.iter().rev() {
            if node == target {
                return true;
            }
            if self.scope_boundary(node) {
                return false;
            }
        }
        false
    }
    fn formatting_marker(&mut self) -> Result<()> {
        if self.formatting.len() >= self.options.formatting_limit {
            return Err(ParseError::FormattingLimit);
        }
        self.formatting.push(None);
        Ok(())
    }
    fn close_p(&mut self) -> Result<()> {
        if let Some(at) = self.scope(&["p"], Scope::Button) {
            self.implied("p");
            self.stack.truncate(at);
        }
        Ok(())
    }
    fn close_scoped(&mut self, name: &str, scope: Scope) {
        if let Some(at) = self.scope(&[name], scope) {
            self.implied(name);
            self.stack.truncate(at);
        }
    }
    fn generic_end(&mut self, name: &str) {
        for at in (0..self.stack.len()).rev() {
            let node = self.stack[at];
            if self.is(node, name) {
                self.implied(name);
                self.stack.truncate(at);
                return;
            }
            if self.special(node) {
                return;
            }
        }
    }
    fn merge_attributes(&mut self, node: NodeId, tag: &mut Tag<'a>) -> Result<()> {
        for a in tag.attributes.drain(..) {
            if self.document.attribute(node, &a.name)?.is_none() {
                self.document.set_attribute(node, &a.name, a.value)?;
            }
        }
        Ok(())
    }
    fn start_text(&mut self, tag: &mut Tag<'a>, tokenizer: &mut Tokenizer<'a>, mode: Mode) -> Result<()> {
        self.insert(tag)?;
        tokenizer.set_mode(mode);
        self.original_mode = self.mode;
        self.mode = Insertion::Text;
        Ok(())
    }
    fn reconstruct(&mut self) -> Result<()> {
        let Some(Some(last)) = self.formatting.last() else { return Ok(()) };
        if self.stack.contains(&last.node) {
            return Ok(());
        }
        let mut at = self.formatting.len() - 1;
        while at > 0 && self.formatting[at - 1].as_ref().is_some_and(|f| !self.stack.contains(&f.node)) {
            at -= 1;
        }
        while at < self.formatting.len() {
            let mut token = self.formatting[at].as_ref().unwrap().token.clone();
            let node = self.insert(&mut token)?;
            self.formatting[at].as_mut().unwrap().node = node;
            at += 1;
        }
        Ok(())
    }
    fn clear_formatting(&mut self) {
        while let Some(entry) = self.formatting.pop() {
            if entry.is_none() {
                break;
            }
        }
    }
    fn add_formatting(&mut self, node: NodeId, token: Tag<'a>) -> Result<()> {
        let lookup =
            if !cfg!(feature = "baseline-parser") && token.attributes.len() > 8 { Some(token.attributes.iter().map(|a| (a.name.as_ref(), a.value.as_ref())).collect::<HashMap<_, _>>()) } else { None };
        let same = |f: &Formatting<'a>| {
            f.token.name == token.name
                && f.token.attributes.len() == token.attributes.len()
                && f.token.attributes.iter().all(|a| if let Some(map) = &lookup { map.get(a.name.as_ref()) == Some(&a.value.as_ref()) } else { token.attributes.contains(a) })
        };
        let matches: Vec<_> = self.formatting.iter().enumerate().rev().take_while(|(_, f)| f.is_some()).filter(|(_, f)| same(f.as_ref().unwrap())).map(|(at, _)| at).collect();
        if matches.len() >= 3 {
            self.formatting.remove(*matches.last().unwrap());
        }
        if self.formatting.len() >= self.options.formatting_limit {
            return Err(ParseError::FormattingLimit);
        }
        self.formatting.push(Some(Formatting { node, token }));
        Ok(())
    }
    fn adoption(&mut self, name: &str) -> Result<()> {
        if self.is(self.current(), name) && !self.formatting.iter().flatten().any(|f| f.node == self.current()) {
            self.stack.pop();
            return Ok(());
        }
        for _ in 0..8 {
            let position = self.formatting.iter().enumerate().rev().take_while(|(_, f)| f.is_some()).find(|(_, f)| f.as_ref().is_some_and(|f| f.token.name == name)).map(|(i, _)| i);
            let Some(position) = position else {
                self.generic_end(name);
                return Ok(());
            };
            let formatting = self.formatting[position].as_ref().unwrap().node;
            let Some(stack_at) = self.stack.iter().position(|&n| n == formatting) else {
                self.formatting.remove(position);
                return Ok(());
            };
            if !self.node_in_scope(formatting) {
                return Ok(());
            }
            let furthest = ((stack_at + 1)..self.stack.len()).find(|&i| self.special(self.stack[i]));
            let Some(furthest) = furthest else {
                self.stack.truncate(stack_at);
                self.formatting.remove(position);
                return Ok(());
            };
            let block = self.stack[furthest];
            let ancestor = self.stack[stack_at - 1];
            let mut bookmark = position;
            let mut last = block;
            let mut inner = furthest;
            let mut count = 0;
            loop {
                inner -= 1;
                let node = self.stack[inner];
                if node == formatting {
                    break;
                }
                count += 1;
                let mut active = self.formatting.iter().position(|f| f.as_ref().is_some_and(|f| f.node == node));
                if count > 3
                    && let Some(at) = active
                {
                    self.formatting.remove(at);
                    if at < bookmark {
                        bookmark -= 1;
                    }
                    active = None;
                }
                let Some(active) = active else {
                    self.stack.remove(inner);
                    continue;
                };
                let mut token = self.formatting[active].as_ref().unwrap().token.clone();
                let clone = self.create(&mut token)?;
                self.formatting[active].as_mut().unwrap().node = clone;
                self.stack[inner] = clone;
                if last == block {
                    bookmark = active + 1;
                }
                self.document.append_child(clone, last)?;
                last = clone;
            }
            let (parent, before) = self.insertion_location(ancestor)?;
            self.document.insert_before(parent, last, before)?;
            let mut token = self.formatting.iter().flatten().find(|f| f.node == formatting).unwrap().token.clone();
            let saved = token.clone();
            let new = self.create(&mut token)?;
            while let Some(child) = self.document.first_child(block)? {
                self.document.append_child(new, child)?;
            }
            self.document.append_child(block, new)?;
            let at = self.formatting.iter().position(|f| f.as_ref().is_some_and(|f| f.node == formatting)).unwrap();
            self.formatting.remove(at);
            if at < bookmark {
                bookmark -= 1;
            }
            self.formatting.insert(bookmark, Some(Formatting { node: new, token: saved }));
            let at = self.stack.iter().position(|&n| n == formatting).unwrap();
            self.stack.remove(at);
            let at = self.stack.iter().position(|&n| n == block).unwrap();
            self.stack.insert(at + 1, new);
        }
        Ok(())
    }

    fn process(&mut self, mut token: Option<Token<'a>>, tokenizer: &mut Tokenizer<'a>) -> Result<()> {
        if self.ignore_lf {
            self.ignore_lf = false;
            if let Some(Token::Text(text)) = &mut token
                && text.starts_with('\n')
            {
                take_prefix(text, 1);
                if text.is_empty() {
                    return Ok(());
                }
            }
        }
        if tokenizer.allows_cdata() && self.dispatch_foreign(&mut token)? {
            return Ok(());
        }
        for _ in 0..self.options.stack_limit.saturating_add(64) {
            self.check_cancelled()?;
            // Character runs can cross an insertion-mode boundary. Handle
            // their leading whitespace without splitting body text per word.
            if matches!(self.mode, Insertion::Initial | Insertion::BeforeHtml | Insertion::BeforeHead | Insertion::Head | Insertion::HeadNoscript | Insertion::AfterHead | Insertion::ColumnGroup)
                && let Some(Token::Text(text)) = &mut token
            {
                let n = text.bytes().take_while(|b| whitespace(*b)).count();
                if n > 0 {
                    let prefix = take_prefix(text, n);
                    if matches!(self.mode, Insertion::Head | Insertion::HeadNoscript | Insertion::AfterHead | Insertion::ColumnGroup) {
                        self.insert_text(prefix)?;
                    }
                    if text.is_empty() {
                        return Ok(());
                    }
                }
            }
            let parent = if matches!(self.mode, Insertion::Initial | Insertion::BeforeHtml | Insertion::AfterAfterBody | Insertion::AfterAfterFrameset) {
                self.document.root()
            } else {
                self.stack.last().copied().unwrap_or(self.document.root())
            };
            let parent = if self.mode == Insertion::AfterBody { self.stack[0] } else { parent };
            if self.miscellaneous(&mut token, parent)? {
                return Ok(());
            }
            let again = match self.mode {
                Insertion::Initial => {
                    if let Some(Token::Doctype(d)) = &mut token {
                        self.quirks = doctype_mode(d);
                        let node = self.document.create_doctype(d.name.as_deref().unwrap_or(""), d.public_id.take().unwrap_or_default(), d.system_id.take().unwrap_or_default())?;
                        self.document.append_child(self.document.root(), node)?;
                        self.mode = Insertion::BeforeHtml;
                        false
                    } else {
                        self.quirks = QuirksMode::Quirks;
                        self.mode = Insertion::BeforeHtml;
                        true
                    }
                }
                Insertion::BeforeHtml => match &mut token {
                    Some(Token::Doctype(_)) => false,
                    Some(Token::StartTag(tag)) if tag.name == "html" => {
                        self.insert(tag)?;
                        self.mode = Insertion::BeforeHead;
                        false
                    }
                    Some(Token::EndTag(tag)) if !matches!(tag.name.as_ref(), "head" | "body" | "html" | "br") => false,
                    _ => {
                        self.insert(&mut synthetic("html"))?;
                        self.mode = Insertion::BeforeHead;
                        true
                    }
                },
                Insertion::BeforeHead => match &mut token {
                    Some(Token::Doctype(_)) => false,
                    Some(Token::StartTag(tag)) if tag.name == "html" => {
                        self.merge_attributes(self.stack[0], tag)?;
                        false
                    }
                    Some(Token::StartTag(tag)) if tag.name == "head" => {
                        self.head = Some(self.insert(tag)?);
                        self.mode = Insertion::Head;
                        false
                    }
                    Some(Token::EndTag(tag)) if !matches!(tag.name.as_ref(), "head" | "body" | "html" | "br") => false,
                    _ => {
                        self.head = Some(self.insert(&mut synthetic("head"))?);
                        self.mode = Insertion::Head;
                        true
                    }
                },
                Insertion::Head => self.in_head(&mut token, tokenizer)?,
                Insertion::HeadNoscript => match &mut token {
                    Some(Token::EndTag(tag)) if tag.name == "noscript" => {
                        self.stack.pop();
                        self.mode = Insertion::Head;
                        false
                    }
                    Some(Token::Doctype(_)) => false,
                    Some(Token::StartTag(tag)) if tag.name == "html" => {
                        self.merge_attributes(self.stack[0], tag)?;
                        false
                    }
                    Some(Token::StartTag(tag)) if matches!(tag.name.as_ref(), "basefont" | "bgsound" | "link" | "meta" | "noframes" | "style") => self.in_head(&mut token, tokenizer)?,
                    Some(Token::StartTag(tag)) if matches!(tag.name.as_ref(), "head" | "noscript") => false,
                    Some(Token::EndTag(tag)) if tag.name != "br" => false,
                    _ => {
                        self.stack.pop();
                        self.mode = Insertion::Head;
                        true
                    }
                },
                Insertion::AfterHead => match &mut token {
                    Some(Token::Doctype(_)) => false,
                    Some(Token::StartTag(tag)) if tag.name == "html" => {
                        self.merge_attributes(self.stack[0], tag)?;
                        false
                    }
                    Some(Token::StartTag(tag)) if tag.name == "body" => {
                        self.insert(tag)?;
                        self.frameset_ok = false;
                        self.mode = Insertion::Body;
                        false
                    }
                    Some(Token::StartTag(tag)) if tag.name == "frameset" => {
                        self.insert(tag)?;
                        self.mode = Insertion::Frameset;
                        false
                    }
                    Some(Token::StartTag(tag)) if head_name(&tag.name) => {
                        let head = self.head.unwrap();
                        self.push(head)?;
                        let again = self.in_head(&mut token, tokenizer)?;
                        if let Some(i) = self.stack.iter().position(|&n| n == head) {
                            self.stack.remove(i);
                        }
                        again
                    }
                    Some(Token::StartTag(tag)) if tag.name == "head" => false,
                    Some(Token::EndTag(tag)) if tag.name == "template" => self.in_head(&mut token, tokenizer)?,
                    Some(Token::EndTag(tag)) if !matches!(tag.name.as_ref(), "body" | "html" | "br") => false,
                    _ => {
                        self.insert(&mut synthetic("body"))?;
                        self.frameset_ok = true;
                        self.mode = Insertion::Body;
                        true
                    }
                },
                Insertion::Body => self.in_body(&mut token, tokenizer)?,
                Insertion::Template => self.in_template(&mut token, tokenizer)?,
                Insertion::Text => match &mut token {
                    Some(Token::Text(text)) => {
                        self.insert_text(std::mem::replace(text, Cow::Borrowed("")))?;
                        false
                    }
                    _ => {
                        self.stack.pop();
                        self.mode = self.original_mode;
                        token.is_none()
                    }
                },
                Insertion::AfterBody => match &mut token {
                    Some(Token::Text(text)) if text.bytes().all(whitespace) => self.in_body(&mut token, tokenizer)?,
                    Some(Token::Doctype(_)) => false,
                    Some(Token::StartTag(tag)) if tag.name == "html" => {
                        self.merge_attributes(self.stack[0], tag)?;
                        false
                    }
                    Some(Token::EndTag(tag)) if tag.name == "html" => {
                        if self.fragment_context.is_none() {
                            self.mode = Insertion::AfterAfterBody;
                        }
                        false
                    }
                    None => false,
                    _ => {
                        self.mode = Insertion::Body;
                        true
                    }
                },
                Insertion::AfterAfterBody => match &mut token {
                    Some(Token::Doctype(_)) => false,
                    Some(Token::Text(text)) if text.bytes().all(whitespace) => self.in_body(&mut token, tokenizer)?,
                    Some(Token::StartTag(tag)) if tag.name == "html" => {
                        self.merge_attributes(self.stack[0], tag)?;
                        false
                    }
                    None => false,
                    _ => {
                        self.mode = Insertion::Body;
                        true
                    }
                },
                Insertion::Frameset | Insertion::AfterFrameset | Insertion::AfterAfterFrameset => self.in_frameset(&mut token, tokenizer)?,
                Insertion::Table | Insertion::TableText | Insertion::Caption | Insertion::ColumnGroup | Insertion::TableBody | Insertion::Row | Insertion::Cell => {
                    self.table_dispatch(&mut token, tokenizer)?
                }
            };
            if !again {
                return Ok(());
            }
        }
        Err(ParseError::ReprocessingLimit)
    }

    fn in_head(&mut self, token: &mut Option<Token<'a>>, tokenizer: &mut Tokenizer<'a>) -> Result<bool> {
        match token {
            Some(Token::Doctype(_)) => {}
            Some(Token::StartTag(tag)) => match tag.name.as_ref() {
                "html" => self.merge_attributes(self.stack[0], tag)?,
                "base" | "basefont" | "bgsound" | "link" | "meta" => {
                    self.insert(tag)?;
                    self.stack.pop();
                }
                "title" => self.start_text(tag, tokenizer, Mode::Rcdata("title".into()))?,
                "style" | "noframes" => {
                    let name = tag.name.to_string();
                    self.start_text(tag, tokenizer, Mode::RawText(name))?;
                }
                "noscript" if self.options.scripting => self.start_text(tag, tokenizer, Mode::RawText("noscript".into()))?,
                "noscript" => {
                    self.insert(tag)?;
                    self.mode = Insertion::HeadNoscript;
                }
                "script" => self.start_text(tag, tokenizer, Mode::ScriptData("script".into()))?,
                "template" => self.start_template(tag)?,
                "head" => {}
                _ => {
                    self.stack.pop();
                    self.mode = Insertion::AfterHead;
                    return Ok(true);
                }
            },
            Some(Token::EndTag(tag)) => match tag.name.as_ref() {
                "head" => {
                    self.stack.pop();
                    self.mode = Insertion::AfterHead;
                }
                "template" => self.close_template(),
                "body" | "html" | "br" => {
                    self.stack.pop();
                    self.mode = Insertion::AfterHead;
                    return Ok(true);
                }
                _ => {}
            },
            _ => {
                self.stack.pop();
                self.mode = Insertion::AfterHead;
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn in_body(&mut self, token: &mut Option<Token<'a>>, tokenizer: &mut Tokenizer<'a>) -> Result<bool> {
        match token {
            None if !self.template_modes.is_empty() => return self.in_template(token, tokenizer),
            None | Some(Token::Doctype(_)) => {}
            Some(Token::Text(text)) => {
                let mut text = std::mem::replace(text, Cow::Borrowed(""));
                if text.contains('\0') {
                    text.to_mut().retain(|c| c != '\0');
                }
                if text.is_empty() {
                    return Ok(false);
                }
                self.reconstruct()?;
                if !text.bytes().all(whitespace) {
                    self.frameset_ok = false;
                }
                self.insert_text(text)?;
            }
            Some(Token::StartTag(tag)) => {
                let name = tag.name.as_ref();
                if name == "html" {
                    if self.template_modes.is_empty() {
                        self.merge_attributes(self.stack[0], tag)?;
                    }
                } else if head_name(name) {
                    return self.in_head(token, tokenizer);
                } else if name == "body" {
                    if self.template_modes.is_empty() && self.stack.len() > 1 && self.is(self.stack[1], "body") {
                        self.frameset_ok = false;
                        self.merge_attributes(self.stack[1], tag)?;
                    }
                } else if name == "frameset" {
                    if self.frameset_ok && self.stack.len() > 1 && self.is(self.stack[1], "body") {
                        self.document.detach(self.stack[1])?;
                        self.stack.truncate(1);
                        self.insert(tag)?;
                        self.mode = Insertion::Frameset;
                    }
                } else if block(name) {
                    self.close_p()?;
                    self.insert(tag)?;
                } else if heading(name) {
                    self.close_p()?;
                    if heading(self.name(self.current())) && self.document.namespace(self.current())? == Some(Namespace::Html) {
                        self.stack.pop();
                    }
                    self.insert(tag)?;
                } else if matches!(name, "pre" | "listing") {
                    self.close_p()?;
                    self.insert(tag)?;
                    self.ignore_lf = true;
                    self.frameset_ok = false;
                } else if name == "form" {
                    if self.form.is_none() || !self.template_modes.is_empty() {
                        self.close_p()?;
                        let form = self.insert(tag)?;
                        if self.template_modes.is_empty() {
                            self.form = Some(form);
                        }
                    }
                } else if matches!(name, "li" | "dd" | "dt") {
                    self.frameset_ok = false;
                    for at in (0..self.stack.len()).rev() {
                        let current = self.name(self.stack[at]);
                        let matching = (if name == "li" { current == "li" } else { matches!(current, "dd" | "dt") }) && self.document.namespace(self.stack[at])? == Some(Namespace::Html);
                        if matching {
                            let except = current.to_owned();
                            self.implied(&except);
                            self.stack.truncate(at);
                            break;
                        }
                        if self.special(self.stack[at]) && !(self.document.namespace(self.stack[at])? == Some(Namespace::Html) && matches!(current, "address" | "div" | "p")) {
                            break;
                        }
                    }
                    self.close_p()?;
                    self.insert(tag)?;
                } else if name == "plaintext" {
                    self.close_p()?;
                    self.insert(tag)?;
                    tokenizer.set_mode(Mode::Plaintext);
                } else if name == "button" {
                    self.close_scoped("button", Scope::Normal);
                    self.reconstruct()?;
                    self.insert(tag)?;
                    self.frameset_ok = false;
                } else if formatting_name(name) {
                    if name == "a"
                        && let Some(node) = self.formatting.iter().rev().take_while(|f| f.is_some()).flatten().find(|f| f.token.name == "a").map(|f| f.node)
                    {
                        self.adoption("a")?;
                        self.formatting.retain(|f| f.as_ref().is_none_or(|f| f.node != node));
                        self.stack.retain(|&n| n != node);
                    }
                    self.reconstruct()?;
                    if tag.name == "nobr" && self.scope(&["nobr"], Scope::Normal).is_some() {
                        self.adoption("nobr")?;
                        self.reconstruct()?;
                    }
                    let saved = tag.clone();
                    let node = self.insert(tag)?;
                    self.add_formatting(node, saved)?;
                } else if matches!(name, "applet" | "marquee" | "object") {
                    self.reconstruct()?;
                    self.insert(tag)?;
                    self.formatting_marker()?;
                    self.frameset_ok = false;
                } else if name == "table" {
                    if self.quirks != QuirksMode::Quirks {
                        self.close_p()?;
                    }
                    self.insert(tag)?;
                    self.frameset_ok = false;
                    self.mode = Insertion::Table;
                } else if matches!(name, "area" | "br" | "embed" | "img" | "keygen" | "wbr" | "input" | "image") {
                    if name == "image" {
                        tag.name = Cow::Borrowed("img");
                    }
                    if tag.name == "input" && self.fragment_element().is_some_and(|n| self.is(n, "select")) {
                        return Ok(false);
                    }
                    if tag.name == "input"
                        && let Some(at) = self.scope(&["select"], Scope::Normal)
                    {
                        self.stack.truncate(at);
                    }
                    let hidden = tag.name == "input" && tag.attributes.iter().any(|a| a.name == "type" && a.value.eq_ignore_ascii_case("hidden"));
                    self.reconstruct()?;
                    self.insert(tag)?;
                    self.stack.pop();
                    if !hidden {
                        self.frameset_ok = false;
                    }
                } else if matches!(name, "param" | "source" | "track") {
                    self.insert(tag)?;
                    self.stack.pop();
                } else if name == "hr" {
                    self.close_p()?;
                    if self.scope(&["select"], Scope::Normal).is_some() {
                        self.implied("");
                    }
                    self.insert(tag)?;
                    self.stack.pop();
                    self.frameset_ok = false;
                } else if name == "textarea" {
                    self.start_text(tag, tokenizer, Mode::Rcdata("textarea".into()))?;
                    self.ignore_lf = true;
                    self.frameset_ok = false;
                } else if matches!(name, "xmp" | "iframe" | "noembed") || name == "noscript" && self.options.scripting {
                    if name == "xmp" {
                        self.close_p()?;
                        self.reconstruct()?;
                    }
                    if matches!(name, "xmp" | "iframe") {
                        self.frameset_ok = false;
                    }
                    let raw = tag.name.to_string();
                    self.start_text(tag, tokenizer, Mode::RawText(raw))?;
                } else if name == "select" {
                    if self.fragment_element().is_some_and(|n| self.is(n, "select")) {
                        return Ok(false);
                    }
                    if let Some(at) = self.scope(&["select"], Scope::Normal) {
                        self.stack.truncate(at);
                    } else {
                        self.reconstruct()?;
                        self.insert(tag)?;
                        self.frameset_ok = false;
                    }
                } else if matches!(name, "option" | "optgroup") {
                    if self.scope(&["select"], Scope::Normal).is_some() {
                        self.implied(if name == "option" { "optgroup" } else { "" });
                    } else if self.is(self.current(), "option") {
                        self.stack.pop();
                    }
                    self.reconstruct()?;
                    self.insert(tag)?;
                } else if matches!(name, "rb" | "rtc" | "rp" | "rt") {
                    if self.scope(&["ruby"], Scope::Normal).is_some() {
                        self.implied(if matches!(name, "rp" | "rt") { "rtc" } else { "" });
                    }
                    self.insert(tag)?;
                } else if matches!(name, "math" | "svg") {
                    self.reconstruct()?;
                    self.insert_foreign(tag, if name == "math" { Namespace::MathMl } else { Namespace::Svg })?;
                } else if matches!(name, "caption" | "col" | "colgroup" | "frame" | "head" | "tbody" | "td" | "tfoot" | "th" | "thead" | "tr") {
                } else {
                    self.reconstruct()?;
                    self.insert(tag)?;
                }
            }
            Some(Token::EndTag(tag)) => {
                let name = tag.name.as_ref();
                if name == "body" || name == "html" {
                    if self.scope(&["body"], Scope::Normal).is_some() {
                        self.mode = Insertion::AfterBody;
                        return Ok(name == "html");
                    }
                } else if name == "template" {
                    self.close_template();
                } else if name == "form" {
                    if !self.template_modes.is_empty() {
                        self.close_scoped("form", Scope::Normal);
                    } else if let Some(node) = self.form.take()
                        && self.node_in_scope(node)
                    {
                        self.implied("");
                        self.stack.retain(|&n| n != node);
                    }
                } else if name == "p" {
                    if self.scope(&["p"], Scope::Button).is_none() {
                        self.insert(&mut synthetic("p"))?;
                    }
                    self.close_p()?;
                } else if name == "li" {
                    self.close_scoped(name, Scope::List);
                } else if matches!(name, "dd" | "dt") {
                    self.close_scoped(name, Scope::Normal);
                } else if heading(name) {
                    if let Some(at) = self.scope(&["h1", "h2", "h3", "h4", "h5", "h6"], Scope::Normal) {
                        self.implied("");
                        self.stack.truncate(at);
                    }
                } else if formatting_name(name) {
                    self.adoption(name)?;
                } else if matches!(name, "applet" | "marquee" | "object") {
                    if self.scope(&[name], Scope::Normal).is_some() {
                        self.close_scoped(name, Scope::Normal);
                        self.clear_formatting();
                    }
                } else if name == "br" {
                    self.reconstruct()?;
                    self.insert(&mut synthetic("br"))?;
                    self.stack.pop();
                    self.frameset_ok = false;
                } else if block(name) || matches!(name, "button" | "listing" | "pre" | "select") {
                    self.close_scoped(name, Scope::Normal);
                } else {
                    self.generic_end(name);
                }
            }
            _ => unreachable!("miscellaneous token already handled"),
        }
        Ok(false)
    }

    // Initial form-element synchronization. Live selectedness, mutation
    // hooks and event dispatch remain separate unfinished DOM integration.
    fn initialize_selected_content(&mut self) -> Result<()> {
        let mut selected = HashMap::<NodeId, Option<NodeId>>::new();
        for target in std::mem::take(&mut self.selected_content) {
            self.check_cancelled()?;
            let mut nearest = None;
            let mut parent = self.document.parent(target)?;
            let mut disabled = false;
            while let Some(node) = parent {
                if self.is(node, "select") {
                    if nearest.is_some() {
                        disabled = true;
                        break;
                    }
                    nearest = Some(node);
                }
                if self.is(node, "option") || self.is(node, "selectedcontent") {
                    disabled = true;
                    break;
                }
                parent = self.document.parent(node)?;
            }
            if disabled {
                continue;
            }
            let Some(select) = nearest else { continue };
            if self.document.attribute(select, "multiple")?.is_some() {
                continue;
            }
            let option = if let Some(option) = selected.get(&select) {
                *option
            } else {
                let option = self.initial_selected_option(select)?;
                selected.insert(select, option);
                option
            };
            let fragment = self.document.create_fragment()?;
            if let Some(option) = option {
                let mut child = self.document.first_child(option)?;
                while let Some(node) = child {
                    child = self.document.next_sibling(node)?;
                    let copied = self.document.clone_subtree_cancellable(node, self.cancelled)?;
                    self.document.append_child(fragment, copied)?;
                }
            }
            while let Some(child) = self.document.first_child(target)? {
                self.document.destroy_subtree(child)?;
            }
            self.document.append_child(target, fragment)?;
            self.document.destroy_subtree(fragment)?;
        }
        Ok(())
    }

    fn initial_selected_option(&self, select: NodeId) -> Result<Option<NodeId>> {
        let mut first = None;
        let mut last = None;
        let mut pending: Vec<_> = self.document.children(select)?.map(|n| (n, false, false)).collect();
        pending.reverse();
        while let Some((node, in_group, disabled_group)) = pending.pop() {
            self.check_cancelled()?;
            let option = self.is(node, "option");
            if option {
                if self.document.attribute(node, "selected")?.is_some() {
                    last = Some(node);
                }
                if first.is_none() && !disabled_group && self.document.attribute(node, "disabled")?.is_none() {
                    first = Some(node);
                }
            }
            let group = self.is(node, "optgroup");
            if option || self.is(node, "select") || self.is(node, "hr") || self.is(node, "datalist") || group && in_group {
                continue;
            }
            let disabled = disabled_group || group && self.document.attribute(node, "disabled")?.is_some();
            let mut children: Vec<_> = self.document.children(node)?.map(|n| (n, in_group || group, disabled)).collect();
            children.reverse();
            pending.extend(children);
        }
        let size = self
            .document
            .attribute(select, "size")?
            .map(|s| {
                let s = s.trim_start_matches(|c: char| c.is_ascii() && whitespace(c as u8));
                let s = s.strip_prefix('+').unwrap_or(s);
                let number = s.bytes().take_while(u8::is_ascii_digit).fold(0u64, |n, c| n.saturating_mul(10).saturating_add((c - b'0') as u64));
                if number == 0 { 1 } else { number }
            })
            .unwrap_or(1);
        Ok(last.or(if size == 1 { first } else { None }))
    }

    fn check_cancelled(&self) -> Result<()> {
        if self.cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(ParseError::Tokenizer(super::Error::Cancelled)) } else { Ok(()) }
    }

    fn in_frameset(&mut self, token: &mut Option<Token<'a>>, tokenizer: &mut Tokenizer<'a>) -> Result<bool> {
        match token {
            Some(Token::Text(text)) => {
                let text = std::mem::replace(text, Cow::Borrowed(""));
                let text = if text.bytes().all(whitespace) { text } else { Cow::Owned(text.chars().filter(|c| c.is_ascii() && whitespace(*c as u8)).collect()) };
                self.insert_text(text)?;
            }
            Some(Token::StartTag(tag)) if tag.name == "html" => self.merge_attributes(self.stack[0], tag)?,
            Some(Token::StartTag(tag)) if tag.name == "noframes" => {
                self.in_head(token, tokenizer)?;
            }
            Some(Token::StartTag(tag)) if tag.name == "frameset" && self.mode == Insertion::Frameset => {
                self.insert(tag)?;
            }
            Some(Token::StartTag(tag)) if tag.name == "frame" && self.mode == Insertion::Frameset => {
                self.insert(tag)?;
                self.stack.pop();
            }
            Some(Token::EndTag(tag)) if tag.name == "frameset" && self.mode == Insertion::Frameset => {
                if self.stack.len() > 1 {
                    self.stack.pop();
                    if self.fragment_context.is_none() && !self.is(self.current(), "frameset") {
                        self.mode = Insertion::AfterFrameset;
                    }
                }
            }
            Some(Token::EndTag(tag)) if tag.name == "html" && self.mode == Insertion::AfterFrameset => self.mode = Insertion::AfterAfterFrameset,
            _ => {}
        }
        Ok(false)
    }
}

fn take_prefix<'a>(text: &mut Cow<'a, str>, n: usize) -> Cow<'a, str> {
    match text {
        Cow::Borrowed(s) => {
            let prefix = &s[..n];
            *s = &s[n..];
            Cow::Borrowed(prefix)
        }
        Cow::Owned(s) => Cow::Owned(s.drain(..n).collect()),
    }
}

fn doctype_mode(d: &Doctype<'_>) -> QuirksMode {
    let public = d.public_id.as_deref().unwrap_or("");
    let system = d.system_id.as_deref().unwrap_or("");
    let exact = ["-//W3O//DTD W3 HTML Strict 3.0//EN//", "-/W3C/DTD HTML 4.0 Transitional/EN", "HTML"];
    let prefixes = [
        "+//Silmaril//dtd html Pro v0r11 19970101//",
        "-//AS//DTD HTML 3.0 asWedit + extensions//",
        "-//AdvaSoft Ltd//DTD HTML 3.0 asWedit + extensions//",
        "-//IETF//DTD HTML 2.0 Level 1//",
        "-//IETF//DTD HTML 2.0 Level 2//",
        "-//IETF//DTD HTML 2.0 Strict Level 1//",
        "-//IETF//DTD HTML 2.0 Strict Level 2//",
        "-//IETF//DTD HTML 2.0 Strict//",
        "-//IETF//DTD HTML 2.0//",
        "-//IETF//DTD HTML 2.1E//",
        "-//IETF//DTD HTML 3.0//",
        "-//IETF//DTD HTML 3.2 Final//",
        "-//IETF//DTD HTML 3.2//",
        "-//IETF//DTD HTML 3//",
        "-//IETF//DTD HTML Level 0//",
        "-//IETF//DTD HTML Level 1//",
        "-//IETF//DTD HTML Level 2//",
        "-//IETF//DTD HTML Level 3//",
        "-//IETF//DTD HTML Strict Level 0//",
        "-//IETF//DTD HTML Strict Level 1//",
        "-//IETF//DTD HTML Strict Level 2//",
        "-//IETF//DTD HTML Strict Level 3//",
        "-//IETF//DTD HTML Strict//",
        "-//IETF//DTD HTML//",
        "-//Metrius//DTD Metrius Presentational//",
        "-//Microsoft//DTD Internet Explorer 2.0 HTML Strict//",
        "-//Microsoft//DTD Internet Explorer 2.0 HTML//",
        "-//Microsoft//DTD Internet Explorer 2.0 Tables//",
        "-//Microsoft//DTD Internet Explorer 3.0 HTML Strict//",
        "-//Microsoft//DTD Internet Explorer 3.0 HTML//",
        "-//Microsoft//DTD Internet Explorer 3.0 Tables//",
        "-//Netscape Comm. Corp.//DTD HTML//",
        "-//Netscape Comm. Corp.//DTD Strict HTML//",
        "-//O'Reilly and Associates//DTD HTML 2.0//",
        "-//O'Reilly and Associates//DTD HTML Extended 1.0//",
        "-//O'Reilly and Associates//DTD HTML Extended Relaxed 1.0//",
        "-//SQ//DTD HTML 2.0 HoTMetaL + extensions//",
        "-//SoftQuad Software//DTD HoTMetaL PRO 6.0::19990601::extensions to HTML 4.0//",
        "-//SoftQuad//DTD HoTMetaL PRO 4.0::19971010::extensions to HTML 4.0//",
        "-//Spyglass//DTD HTML 2.0 Extended//",
        "-//Sun Microsystems Corp.//DTD HotJava HTML//",
        "-//Sun Microsystems Corp.//DTD HotJava Strict HTML//",
        "-//W3C//DTD HTML 3 1995-03-24//",
        "-//W3C//DTD HTML 3.2 Draft//",
        "-//W3C//DTD HTML 3.2 Final//",
        "-//W3C//DTD HTML 3.2//",
        "-//W3C//DTD HTML 3.2S Draft//",
        "-//W3C//DTD HTML 4.0 Frameset//",
        "-//W3C//DTD HTML 4.0 Transitional//",
        "-//W3C//DTD HTML Experimental 19960712//",
        "-//W3C//DTD HTML Experimental 970421//",
        "-//W3C//DTD W3 HTML//",
        "-//W3O//DTD W3 HTML 3.0//",
        "-//WebTechs//DTD Mozilla HTML 2.0//",
        "-//WebTechs//DTD Mozilla HTML//",
    ];
    let starts = |value: &str, prefix: &str| value.as_bytes().get(..prefix.len()).is_some_and(|v| v.eq_ignore_ascii_case(prefix.as_bytes()));
    let html4 = ["-//W3C//DTD HTML 4.01 Frameset//", "-//W3C//DTD HTML 4.01 Transitional//"].iter().any(|p| starts(public, p));
    if d.force_quirks
        || d.name.as_deref() != Some("html")
        || exact.iter().any(|p| public.eq_ignore_ascii_case(p))
        || system.eq_ignore_ascii_case("http://www.ibm.com/data/dtd/v11/ibmxhtml1-transitional.dtd")
        || prefixes.iter().any(|p| starts(public, p))
        || system.is_empty() && html4
    {
        QuirksMode::Quirks
    } else if html4 || ["-//W3C//DTD XHTML 1.0 Frameset//", "-//W3C//DTD XHTML 1.0 Transitional//"].iter().any(|p| starts(public, p)) {
        QuirksMode::LimitedQuirks
    } else {
        QuirksMode::NoQuirks
    }
}
