use super::*;
use crate::{
    css::{
        properties::{Display, Inner, Outer},
        style::{Keyword, Property, Style, StyleTree, Value, WritingMode},
    },
    dom::{Document, NodeId, NodeType},
    text::prepare::{Collapse, WhiteSpace},
};
use std::{
    num::NonZeroU32,
    ops::Range,
    sync::{Arc, atomic::AtomicBool},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BoxId(NonZeroU32);
impl BoxId {
    pub fn index(self) -> usize {
        self.0.get() as usize - 1
    }
    fn new(i: usize) -> Result<Self> {
        Ok(Self(NonZeroU32::new(u32::try_from(i + 1).map_err(|_| Error::Limit)?).ok_or(Error::Limit)?))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxKind {
    Viewport,
    Block { flow_root: bool, root: bool },
    InlineContent { source: u32 },
}
#[derive(Clone, Debug)]
pub struct BoxNode {
    pub node: NodeId,
    pub style_node: NodeId,
    pub kind: BoxKind,
    pub parent: Option<BoxId>,
    pub first_child: Option<BoxId>,
    pub next_sibling: Option<BoxId>,
    last_child: Option<BoxId>,
}
#[derive(Clone, Debug)]
pub struct SourceSpan {
    pub range: Range<u32>,
    pub node: NodeId,
    pub style_node: NodeId,
}
pub struct TextSource {
    pub container: BoxId,
    pub source: Arc<str>,
    pub spans: Box<[SourceSpan]>,
}
impl TextSource {
    pub fn span_at(&self, byte: u32) -> Option<&SourceSpan> {
        self.spans.get(self.spans.partition_point(|s| s.range.end <= byte)).filter(|s| s.range.contains(&byte))
    }
}
pub struct Tree {
    pub(super) boxes: Box<[BoxNode]>,
    pub(super) sources: Box<[TextSource]>,
    pub(super) styles: Arc<StyleTree>,
    document: u64,
    revision: u64,
    text_bytes: usize,
}
impl Tree {
    pub fn text_bytes(&self) -> usize {
        self.text_bytes
    }
    pub fn boxes(&self) -> &[BoxNode] {
        &self.boxes
    }
    pub fn iter(&self) -> impl Iterator<Item = (BoxId, &BoxNode)> {
        self.boxes.iter().enumerate().map(|(i, b)| (BoxId::new(i).unwrap(), b))
    }
    pub fn root(&self) -> BoxId {
        BoxId::new(0).unwrap()
    }
    pub fn get(&self, id: BoxId) -> Option<&BoxNode> {
        self.boxes.get(id.index())
    }
    pub fn style(&self, id: BoxId) -> Option<&Style> {
        self.get(id).and_then(|b| self.styles.snapshot(b.style_node))
    }
    pub fn text_source(&self, index: u32) -> Option<&TextSource> {
        self.sources.get(index as usize)
    }
    pub fn styles(&self) -> &Arc<StyleTree> {
        &self.styles
    }
    pub fn validate(&self, document: &Document) -> Result<()> {
        if self.document != document.id() || self.revision != document.revision() { Err(Error::Stale) } else { Ok(()) }
    }
    pub fn storage_payload_bytes(&self) -> usize {
        self.boxes.len() * size_of::<BoxNode>() + self.sources.iter().map(|s| s.source.len() + s.spans.len() * size_of::<SourceSpan>()).sum::<usize>()
    }
}
#[derive(Clone, Copy, Debug)]
pub struct BuildLimits {
    pub nodes: usize,
    pub boxes: usize,
    pub depth: usize,
    pub text_bytes: usize,
    pub paragraph_bytes: usize,
    pub spans: usize,
}
impl Default for BuildLimits {
    fn default() -> Self {
        Self { nodes: 2_000_000, boxes: 1_000_000, depth: 1024, text_bytes: 16 * 1024 * 1024, paragraph_bytes: 65536, spans: 262144 }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct BuildProgress {
    pub visited: usize,
    pub boxes: usize,
    pub source_bytes: usize,
    pub spans: usize,
    pub finished: bool,
}
struct Cursor {
    next: Option<NodeId>,
    container: BoxId,
    style: NodeId,
    close: bool,
}
struct Pending {
    container: BoxId,
    source: String,
    spans: Vec<SourceSpan>,
}
pub struct BuildJob<'a> {
    document: &'a Document,
    styles: Arc<StyleTree>,
    limits: BuildLimits,
    cancel: Option<&'a AtomicBool>,
    boxes: Vec<BoxNode>,
    sources: Vec<TextSource>,
    stack: Vec<Cursor>,
    pending: Option<Pending>,
    progress: BuildProgress,
    failed: bool,
}
impl<'a> BuildJob<'a> {
    pub fn new(document: &'a Document, styles: Arc<StyleTree>, limits: BuildLimits, cancel: Option<&'a AtomicBool>) -> Result<Self> {
        check(cancel)?;
        styles.get(document, document.root(), styles.generation())?;
        if limits.boxes == 0 || limits.depth == 0 {
            return Err(Error::Limit);
        }
        let root = BoxId::new(0)?;
        Ok(Self {
            document,
            styles,
            limits,
            cancel,
            boxes: vec![BoxNode { node: document.root(), style_node: document.root(), kind: BoxKind::Viewport, parent: None, first_child: None, last_child: None, next_sibling: None }],
            sources: Vec::new(),
            stack: vec![Cursor { next: document.first_child(document.root())?, container: root, style: document.root(), close: false }],
            pending: None,
            progress: BuildProgress { boxes: 1, ..Default::default() },
            failed: false,
        })
    }
    pub fn progress(&self) -> BuildProgress {
        self.progress
    }
    fn add(&mut self, node: NodeId, style_node: NodeId, kind: BoxKind, parent: BoxId) -> Result<BoxId> {
        if self.boxes.len() >= self.limits.boxes {
            return Err(Error::Limit);
        }
        let id = BoxId::new(self.boxes.len())?;
        if let Some(last) = self.boxes[parent.index()].last_child {
            self.boxes[last.index()].next_sibling = Some(id)
        } else {
            self.boxes[parent.index()].first_child = Some(id)
        }
        self.boxes[parent.index()].last_child = Some(id);
        self.boxes.push(BoxNode { node, style_node, kind, parent: Some(parent), first_child: None, last_child: None, next_sibling: None });
        self.progress.boxes = self.boxes.len();
        Ok(id)
    }
    fn flush(&mut self) -> Result<()> {
        let Some(p) = self.pending.take() else { return Ok(()) };
        // Collapsible indentation between blocks generates no anonymous line box.
        if p.source.bytes().all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
            && p.spans.iter().all(|s| self.styles.snapshot(s.style_node).and_then(|s| WhiteSpace::for_style(s).ok()).is_some_and(|w| w.collapse == Collapse::Collapse))
        {
            return Ok(());
        }
        let owner = p.spans.first().ok_or(Error::Invalid)?.style_node;
        let source = u32::try_from(self.sources.len()).map_err(|_| Error::Limit)?;
        self.add(self.boxes[p.container.index()].node, owner, BoxKind::InlineContent { source }, p.container)?;
        self.sources.push(TextSource { container: p.container, source: Arc::from(p.source), spans: p.spans.into_boxed_slice() });
        Ok(())
    }
    fn push(&mut self, node: NodeId, container: BoxId, style: NodeId, close: bool) -> Result<()> {
        if self.stack.len() >= self.limits.depth {
            return Err(Error::Limit);
        }
        self.stack.push(Cursor { next: self.document.first_child(node)?, container, style, close });
        Ok(())
    }
    fn text(&mut self, node: NodeId, container: BoxId, style: NodeId) -> Result<()> {
        let value = self.document.value(node)?.ok_or(Error::Invalid)?;
        if value.is_empty() {
            return Ok(());
        }
        if self.styles.snapshot(style).is_none() {
            return if value.bytes().all(|b| b.is_ascii_whitespace()) { Ok(()) } else { Err(Error::Unsupported("text outside a styled element")) };
        }
        self.progress.source_bytes = self.progress.source_bytes.checked_add(value.len()).ok_or(Error::Limit)?;
        self.progress.spans += 1;
        if self.progress.source_bytes > self.limits.text_bytes || self.progress.spans > self.limits.spans {
            return Err(Error::Limit);
        }
        let p = self.pending.get_or_insert_with(|| Pending { container, source: String::new(), spans: Vec::new() });
        if p.container != container {
            return Err(Error::Invalid);
        }
        let from = p.source.len();
        if from.checked_add(value.len()).is_none_or(|n| n > self.limits.paragraph_bytes || n > u32::MAX as usize) {
            return Err(Error::Limit);
        }
        p.source.push_str(value);
        p.spans.push(SourceSpan { range: from as u32..p.source.len() as u32, node, style_node: style });
        Ok(())
    }
    pub fn step(&mut self, budget: usize) -> Result<BuildProgress> {
        if self.failed {
            return Err(Error::Failed);
        }
        let r = self.advance(budget);
        if r.is_err() {
            self.failed = true
        }
        r
    }
    fn advance(&mut self, budget: usize) -> Result<BuildProgress> {
        check(self.cancel)?;
        for _ in 0..budget {
            check(self.cancel)?;
            let Some(frame) = self.stack.last_mut() else {
                self.flush()?;
                self.progress.finished = true;
                break;
            };
            let Some(node) = frame.next else {
                let close = frame.close;
                self.stack.pop();
                if close {
                    self.flush()?
                }
                continue;
            };
            let (container, parent_style) = (frame.container, frame.style);
            frame.next = self.document.next_sibling(node)?;
            self.progress.visited += 1;
            if self.progress.visited > self.limits.nodes {
                return Err(Error::Limit);
            }
            match self.document.node_type(node)? {
                NodeType::Element => {
                    let s = self.styles.snapshot(node).ok_or(Error::Stale)?;
                    let Value::Display(display) = s.value(Property::Display) else { return Err(Error::Invalid) };
                    if *display == Display::None {
                        continue;
                    }
                    if s.axes().writing_mode != WritingMode::HorizontalTb {
                        return Err(Error::Unsupported("vertical box layout"));
                    }
                    if !matches!(s.value(Property::Position), Value::Keyword(Keyword::Static | Keyword::Relative))
                        || !matches!(s.value(Property::Float), Value::Keyword(Keyword::None))
                        || !matches!(s.value(Property::Clear), Value::Keyword(Keyword::None))
                    {
                        return Err(Error::Unsupported("positioned/floating/clearance layout"));
                    }
                    if self.document.namespace(node)? != Some(crate::dom::Namespace::Html) {
                        return Err(Error::Unsupported("foreign box layout"));
                    }
                    if matches!(self.document.name(node)?, Some("img" | "input" | "textarea" | "select" | "button" | "video" | "audio" | "iframe" | "object" | "embed" | "canvas" | "br" | "wbr")) {
                        return Err(Error::Unsupported("replaced/control/forced-break inline object"));
                    }
                    match *display {
                        Display::Contents => self.push(node, container, node, false)?,
                        Display::Box { outer: Outer::Inline, inner: Inner::Flow, list_item: false } => {
                            if !matches!(s.value(Property::Position), Value::Keyword(Keyword::Static)) || !matches!(s.value(Property::VerticalAlign), Value::Keyword(Keyword::Baseline)) {
                                return Err(Error::Unsupported("positioned/vertically aligned inline box"));
                            }
                            for property in [Property::MarginLeft, Property::MarginRight, Property::PaddingLeft, Property::PaddingRight, Property::PaddingTop, Property::PaddingBottom] {
                                if s.value(property).scalar() != Some(0.) {
                                    return Err(Error::Unsupported("inline box edges"));
                                }
                            }
                            for property in [Property::BorderLeftWidth, Property::BorderRightWidth, Property::BorderTopWidth, Property::BorderBottomWidth] {
                                if s.used_border_width(property)? != 0. {
                                    return Err(Error::Unsupported("inline box borders"));
                                }
                            }
                            self.push(node, container, node, false)?;
                        }
                        Display::Box { outer: Outer::Block, inner: inner @ (Inner::Flow | Inner::FlowRoot), list_item: false } => {
                            self.flush()?;
                            let root = self.boxes[container.index()].kind == BoxKind::Viewport;
                            let id = self.add(node, node, BoxKind::Block { flow_root: inner == Inner::FlowRoot, root }, container)?;
                            self.push(node, id, node, true)?;
                        }
                        _ => return Err(Error::Unsupported("non-flow box formatting")),
                    }
                }
                NodeType::Text => self.text(node, container, parent_style)?,
                NodeType::Document | NodeType::Fragment => return Err(Error::Invalid),
                _ => {}
            }
        }
        Ok(self.progress)
    }
    pub fn finish(self) -> Result<Arc<Tree>> {
        if self.failed {
            return Err(Error::Failed);
        }
        check(self.cancel)?;
        if !self.progress.finished {
            return Err(Error::Incomplete);
        }
        let text_bytes = self.sources.iter().map(|s| s.source.len()).sum();
        Ok(Arc::new(Tree {
            text_bytes,
            document: self.document.id(),
            revision: self.document.revision(),
            styles: self.styles,
            boxes: self.boxes.into_boxed_slice(),
            sources: self.sources.into_boxed_slice(),
        }))
    }
}
