use super::*;
use crate::{
    css::{
        properties::{Display, Inner, Outer},
        selectors::PseudoElement,
        style::{Keyword, Property, Style, StyleKey, StyleTree, Value, WritingMode},
    },
    dom::{Document, NodeId, NodeType},
    text::prepare::{Collapse, ControlKind, WhiteSpace},
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
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BoxKind {
    Viewport,
    Block {
        flow_root: bool,
        root: bool,
        absolute: bool,
    },
    InlineContent {
        source: u32,
    },
    InlineImage {
        image: u32,
    },
    BlockImage {
        image: u32,
    },
    InlineVector {
        vector: u32,
    },
    BlockVector {
        vector: u32,
    },
    /// A replaced element the engine cannot render yet (a form control, a
    /// frame, a canvas, a broken image): it takes its natural CSS-pixel size
    /// and paints only its own decoration.
    InlineReplaced {
        natural: [f32; 2],
    },
    BlockReplaced {
        natural: [f32; 2],
    },
}
impl BoxKind {
    pub fn is_inline_level(self) -> bool {
        matches!(self, Self::InlineContent { .. } | Self::InlineImage { .. } | Self::InlineVector { .. } | Self::InlineReplaced { .. })
    }
    /// Images, vectors and placeholders: boxes sized by content they do not lay out.
    pub fn is_replaced(self) -> bool {
        matches!(self, Self::InlineImage { .. } | Self::BlockImage { .. } | Self::InlineVector { .. } | Self::BlockVector { .. } | Self::InlineReplaced { .. } | Self::BlockReplaced { .. })
    }
}
#[derive(Clone, Debug)]
pub struct BoxNode {
    pub node: NodeId,
    pub style_node: StyleKey,
    pub kind: BoxKind,
    /// Absolutely positioned: laid out after its containing block, out of flow.
    pub absolute: bool,
    /// A table cell's column and row spans (1 elsewhere).
    pub span: [u16; 2],
    pub parent: Option<BoxId>,
    pub first_child: Option<BoxId>,
    pub next_sibling: Option<BoxId>,
    last_child: Option<BoxId>,
}
#[derive(Clone, Debug)]
pub struct SourceSpan {
    pub range: Range<u32>,
    pub node: NodeId,
    pub style_node: StyleKey,
}
/// A non-replaced inline's source coverage in one anonymous text box. A block
/// descendant can split it into several parts; only its actual ends carry edges.
#[derive(Clone, Debug)]
pub struct InlineBox {
    pub node: NodeId,
    pub range: Range<u32>,
    pub first: bool,
    pub last: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct Control {
    source: u32,
    pub offset: u32,
    pub kind: ControlKind,
}
pub struct TextSource {
    pub container: BoxId,
    pub source: Arc<str>,
    pub spans: Box<[SourceSpan]>,
    inlines: Range<u32>,
}
impl TextSource {
    pub fn span_at(&self, byte: u32) -> Option<&SourceSpan> {
        self.spans.get(self.spans.partition_point(|s| s.range.end <= byte)).filter(|s| s.range.contains(&byte))
    }
}
pub struct Tree {
    vectors: Box<[crate::svg::Viewport]>,
    images: Option<Arc<crate::image::Set>>,
    pub(super) boxes: Box<[BoxNode]>,
    pub(super) sources: Box<[TextSource]>,
    pub(super) styles: Arc<StyleTree>,
    inlines: Box<[InlineBox]>,
    controls: Box<[Control]>,
    document: u64,
    revision: u64,
    text_bytes: usize,
}
impl Tree {
    pub fn vector(&self, index: u32) -> Option<&crate::svg::Viewport> {
        self.vectors.get(index as usize)
    }
    pub(super) fn natural(&self, id: BoxId) -> Result<(f64, f64)> {
        match self.get(id).ok_or(Error::Invalid)?.kind {
            BoxKind::InlineImage { image } | BoxKind::BlockImage { image } => {
                let image = self.image(image).ok_or(Error::Invalid)?;
                Ok((f64::from(image.width()), f64::from(image.height())))
            }
            BoxKind::InlineVector { vector } | BoxKind::BlockVector { vector } => Ok(self.vector(vector).ok_or(Error::Invalid)?.natural()),
            BoxKind::InlineReplaced { natural } | BoxKind::BlockReplaced { natural } => Ok((f64::from(natural[0]), f64::from(natural[1]))),
            _ => Err(Error::Invalid),
        }
    }
    pub fn images(&self) -> Option<&Arc<crate::image::Set>> {
        self.images.as_ref()
    }
    pub fn image(&self, index: u32) -> Option<&Arc<crate::image::Image>> {
        self.images.as_ref()?.image(index)
    }
    pub fn text_bytes(&self) -> usize {
        self.text_bytes
    }
    pub fn boxes(&self) -> &[BoxNode] {
        &self.boxes
    }
    pub fn iter(&self) -> impl Iterator<Item = (BoxId, &BoxNode)> {
        self.boxes.iter().enumerate().map(|(i, b)| (BoxId::new(i).unwrap(), b))
    }
    /// The box at a preorder index, without walking the ones before it.
    pub fn at(&self, index: usize) -> Option<(BoxId, &BoxNode)> {
        self.boxes.get(index).map(|b| (BoxId::new(index).unwrap(), b))
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
    pub fn inline_boxes(&self, source: u32) -> Option<&[InlineBox]> {
        let range = &self.text_source(source)?.inlines;
        self.inlines.get(range.start as usize..range.end as usize)
    }
    pub fn controls(&self, source: u32) -> &[Control] {
        let lo = self.controls.partition_point(|c| c.source < source);
        let hi = self.controls.partition_point(|c| c.source <= source);
        &self.controls[lo..hi]
    }
    pub fn styles(&self) -> &Arc<StyleTree> {
        &self.styles
    }
    pub fn validate(&self, document: &Document) -> Result<()> {
        if self.document != document.id() || self.revision != document.revision() { Err(Error::Stale) } else { Ok(()) }
    }
    pub fn storage_payload_bytes(&self) -> usize {
        let mut seen = std::collections::HashSet::new();
        let definitions: usize = self
            .vectors
            .iter()
            .flat_map(|v| v.clip_nodes())
            .filter(|c| seen.insert(Arc::as_ptr(&c.definition) as usize))
            .map(|c| c.definition.payload_bytes() + size_of::<crate::svg::clip::Definition>() + 2 * size_of::<usize>())
            .sum();
        definitions
            + self.vectors.len() * size_of::<crate::svg::Viewport>()
            + self.vectors.iter().map(|v| v.payload_bytes()).sum::<usize>()
            + self.images.as_ref().map_or(0, |images| images.payload_bytes())
            + self.boxes.len() * size_of::<BoxNode>()
            + self.inlines.len() * size_of::<InlineBox>()
            + self.sources.len() * size_of::<Range<u32>>()
            + self.controls.len() * size_of::<Control>()
            + self.sources.iter().map(|s| s.source.len() + s.spans.len() * size_of::<SourceSpan>()).sum::<usize>()
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
    pub inline_boxes: usize,
    pub vector_bytes: usize,
    pub vector_work: usize,
}
impl Default for BuildLimits {
    fn default() -> Self {
        Self {
            nodes: 2_000_000,
            boxes: 1_000_000,
            depth: 1024,
            text_bytes: 16 * 1024 * 1024,
            paragraph_bytes: 65536,
            spans: 262144,
            inline_boxes: 262144,
            vector_bytes: 64 * 1024 * 1024,
            vector_work: 4_000_000,
        }
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
    style: StyleKey,
    close: bool,
    inline: bool,
    /// The element whose `::after` content comes once its children are built.
    after: Option<NodeId>,
    /// The paragraph an atomic inline interrupted, resumed when its content is built.
    restore: Option<Box<Pending>>,
}
struct Pending {
    container: BoxId,
    /// The anonymous text box, created when the paragraph starts so atomic
    /// inlines and replaced elements can be its children in document order.
    text_box: BoxId,
    source: String,
    spans: Vec<SourceSpan>,
    inlines: Vec<InlineBox>,
    controls: Vec<(u32, ControlKind)>,
}
struct OpenInline {
    node: NodeId,
    container: BoxId,
    first: bool,
    index: usize,
}
pub struct BuildJob<'a> {
    clip_sources: crate::svg::clip::Sources<'a>,
    vectors: Vec<crate::svg::Viewport>,
    vector_bytes: usize,
    vector_work: usize,
    images: Option<Arc<crate::image::Set>>,
    fallbacks: Fallbacks,
    document: &'a Document,
    styles: Arc<StyleTree>,
    limits: BuildLimits,
    cancel: Option<&'a AtomicBool>,
    boxes: Vec<BoxNode>,
    sources: Vec<TextSource>,
    inlines: Vec<InlineBox>,
    controls: Vec<Control>,
    open: Vec<OpenInline>,
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
            images: None,
            clip_sources: Default::default(),
            vectors: Vec::new(),
            vector_bytes: 0,
            vector_work: 0,
            fallbacks: Fallbacks::default(),
            document,
            styles,
            limits,
            cancel,
            boxes: vec![BoxNode {
                node: document.root(),
                style_node: document.root().into(),
                kind: BoxKind::Viewport,
                absolute: false,
                span: [1, 1],
                parent: None,
                first_child: None,
                last_child: None,
                next_sibling: None,
            }],
            sources: Vec::new(),
            inlines: Vec::new(),
            controls: Vec::new(),
            open: Vec::new(),
            stack: vec![Cursor { next: document.first_child(document.root())?, container: root, style: document.root().into(), close: false, inline: false, after: None, restore: None }],
            pending: None,
            progress: BuildProgress { boxes: 1, ..Default::default() },
            failed: false,
        })
    }
    pub fn with_images(mut self, images: Arc<crate::image::Set>) -> Result<Self> {
        check(self.cancel)?;
        if self.failed || self.progress.visited != 0 || self.images.is_some() {
            return Err(Error::Invalid);
        }
        images.validate(self.document).map_err(|_| Error::Stale)?;
        self.images = Some(images);
        Ok(self)
    }
    pub fn progress(&self) -> BuildProgress {
        self.progress
    }
    /// What the engine substituted while building this tree.
    pub fn fallbacks(&self) -> &Fallbacks {
        &self.fallbacks
    }
    fn note(&mut self, what: &'static str, node: NodeId) {
        self.fallbacks.note(what, Some(node));
    }
    fn add(&mut self, node: NodeId, style_node: impl Into<StyleKey>, kind: BoxKind, parent: BoxId) -> Result<BoxId> {
        let style_node = style_node.into();
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
        let absolute = matches!(kind, BoxKind::Block { absolute: true, .. });
        self.boxes.push(BoxNode { node, style_node, kind, absolute, span: [1, 1], parent: Some(parent), first_child: None, last_child: None, next_sibling: None });
        self.progress.boxes = self.boxes.len();
        Ok(id)
    }
    fn flush(&mut self) -> Result<()> {
        let Some(mut p) = self.pending.take() else { return Ok(()) };
        for open in self.open.iter_mut().filter(|o| o.container == p.container) {
            p.inlines[open.index].range.end = p.source.len() as u32;
            open.first = false;
        }
        // Collapsible indentation between blocks generates no anonymous line box.
        if p.source.bytes().all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
            && p.inlines.is_empty()
            && p.controls.is_empty()
            && (self.styles.snapshot(self.boxes[p.container.index()].style_node).is_some_and(|s| super::flow::is_flex(s) || super::flow::is_grid(s))
                || p.spans.iter().all(|s| self.styles.snapshot(s.style_node).and_then(|s| WhiteSpace::for_style(s).ok()).is_some_and(|w| w.collapse == Collapse::Collapse)))
        {
            // Its text box and source slot were made when the paragraph
            // started; it has no children (no controls), and nothing came after.
            self.remove_last(p.text_box)?;
            let BoxKind::InlineContent { source } = self.boxes.get(p.text_box.index()).map_or(BoxKind::InlineContent { source: self.sources.len() as u32 - 1 }, |b| b.kind) else {
                return Err(Error::Invalid);
            };
            if source as usize + 1 != self.sources.len() {
                return Err(Error::Invalid);
            }
            self.sources.pop();
            return Ok(());
        }
        let owner = p.spans.first().map_or(self.boxes[p.container.index()].style_node, |s| s.style_node);
        let text_box = p.text_box;
        let BoxKind::InlineContent { source } = self.boxes[text_box.index()].kind else { return Err(Error::Invalid) };
        self.boxes[text_box.index()].style_node = owner;
        let start = u32::try_from(self.inlines.len()).map_err(|_| Error::Limit)?;
        self.inlines.extend(p.inlines);
        let end = u32::try_from(self.inlines.len()).map_err(|_| Error::Limit)?;
        self.controls.extend(p.controls.into_iter().map(|(offset, kind)| Control { source, offset, kind }));
        let slot = self.sources.get_mut(source as usize).ok_or(Error::Invalid)?;
        *slot = TextSource { container: p.container, source: Arc::from(p.source), spans: p.spans.into_boxed_slice(), inlines: start..end };
        Ok(())
    }
    /// Unlink and drop the box just added, which has no children.
    fn remove_last(&mut self, id: BoxId) -> Result<()> {
        if id.index() + 1 != self.boxes.len() || self.boxes[id.index()].first_child.is_some() {
            return Err(Error::Invalid);
        }
        let parent = self.boxes[id.index()].parent.ok_or(Error::Invalid)?;
        let mut previous = None;
        let mut at = self.boxes[parent.index()].first_child;
        while let Some(child) = at {
            if child == id {
                break;
            }
            previous = Some(child);
            at = self.boxes[child.index()].next_sibling;
        }
        match previous {
            Some(previous) => self.boxes[previous.index()].next_sibling = None,
            None => self.boxes[parent.index()].first_child = None,
        }
        self.boxes[parent.index()].last_child = previous;
        self.boxes.pop();
        self.progress.boxes = self.boxes.len();
        Ok(())
    }
    fn pending(&mut self, container: BoxId) -> Result<&mut Pending> {
        if self.pending.is_none() {
            let mut inlines = Vec::new();
            for open in self.open.iter_mut().filter(|o| o.container == container) {
                open.index = inlines.len();
                inlines.push(InlineBox { node: open.node, range: 0..0, first: open.first, last: false });
            }
            // Reserve the source slot now: an atomic inline's own paragraph
            // can finish before this one and must not take its index.
            let source = u32::try_from(self.sources.len()).map_err(|_| Error::Limit)?;
            self.sources.push(TextSource { container, source: Arc::from(""), spans: Box::new([]), inlines: 0..0 });
            let container_node = self.boxes[container.index()].node;
            let style_node = self.boxes[container.index()].style_node;
            let text_box = self.add(container_node, style_node, BoxKind::InlineContent { source }, container)?;
            self.pending = Some(Pending { container, text_box, source: String::new(), spans: Vec::new(), inlines, controls: Vec::new() });
        }
        let p = self.pending.as_mut().unwrap();
        if p.inlines.len() > self.limits.inline_boxes.saturating_sub(self.inlines.len()) {
            return Err(Error::Limit);
        }
        if p.container != container {
            return Err(Error::Invalid);
        }
        Ok(p)
    }
    fn open_inline(&mut self, node: NodeId, container: BoxId) -> Result<()> {
        let remaining = self.limits.inline_boxes.saturating_sub(self.inlines.len());
        let p = self.pending(container)?;
        if p.inlines.len() >= remaining {
            return Err(Error::Limit);
        }
        let index = p.inlines.len();
        let at = p.source.len() as u32;
        p.inlines.push(InlineBox { node, range: at..at, first: true, last: false });
        self.open.push(OpenInline { node, container, first: true, index });
        Ok(())
    }
    fn close_inline(&mut self) -> Result<()> {
        let container = self.open.last().ok_or(Error::Invalid)?.container;
        self.pending(container)?;
        let open = self.open.pop().ok_or(Error::Invalid)?;
        let p = self.pending.as_mut().unwrap();
        p.inlines[open.index].range.end = p.source.len() as u32;
        p.inlines[open.index].last = true;
        Ok(())
    }
    fn push(&mut self, node: NodeId, container: BoxId, style: impl Into<StyleKey>, close: bool) -> Result<()> {
        if self.stack.len() >= self.limits.depth {
            return Err(Error::Limit);
        }
        self.stack.push(Cursor { next: self.document.first_child(node)?, container, style: style.into(), close, inline: false, after: Some(node), restore: None });
        Ok(())
    }
    /// An atomic inline (inline-block, inline-flex, inline-grid, inline-table):
    /// a block formatting context that sits in its paragraph as one object.
    /// Its own content is built with the paragraph set aside, then resumed.
    fn atomic(&mut self, node: NodeId, container: BoxId, flex_or_grid: bool) -> Result<()> {
        self.append_object(node, container, node, BoxKind::Block { flow_root: true, root: false, absolute: false })?;
        let p = self.pending.as_ref().ok_or(Error::Invalid)?;
        let block = self.boxes[p.text_box.index()].last_child.ok_or(Error::Invalid)?;
        if self.stack.len() >= self.limits.depth {
            return Err(Error::Limit);
        }
        let _ = flex_or_grid;
        let restore = self.pending.take().map(Box::new);
        self.stack.push(Cursor { next: self.document.first_child(node)?, container: block, style: node.into(), close: true, inline: false, after: Some(node), restore });
        self.generated(node, block, PseudoElement::Before)?;
        Ok(())
    }
    /// The element's `::before` or `::after` content, if it has any: text in
    /// the pseudo-element's style, as an inline run of the paragraph, an
    /// atomic inline, or a block of its own (CSS Generated Content 3 §2).
    fn generated(&mut self, node: NodeId, container: BoxId, pseudo: PseudoElement) -> Result<()> {
        let styles = self.styles.clone();
        let Some(s) = styles.pseudo(node, pseudo) else { return Ok(()) };
        let key = StyleKey::pseudo(node, pseudo);
        let mut text = String::new();
        if let Value::Content(content) = s.value(Property::Content) {
            for part in content.0.iter() {
                match part {
                    crate::css::properties::ContentPart::Text(t) => text.push_str(t),
                    crate::css::properties::ContentPart::Attr(name) => text.push_str(self.document.attribute(node, name)?.unwrap_or("")),
                }
            }
        }
        let Value::Display(display) = s.value(Property::Display) else { return Err(Error::Invalid) };
        let absolute = matches!(s.value(Property::Position), Value::Keyword(Keyword::Absolute | Keyword::Fixed));
        match *display {
            Display::None => {}
            Display::Box { outer: Outer::Inline, inner: Inner::Flow, .. } if !absolute => self.append(node, container, key, &text, None)?,
            Display::Box { outer: Outer::Inline, inner: Inner::FlowRoot | Inner::Flex | Inner::Grid | Inner::Table, .. } if !absolute => {
                // An atomic inline: its text is its own paragraph, built now.
                self.append_object(node, container, key, BoxKind::Block { flow_root: true, root: false, absolute: false })?;
                let p = self.pending.as_ref().ok_or(Error::Invalid)?;
                let block = self.boxes[p.text_box.index()].last_child.ok_or(Error::Invalid)?;
                let restore = self.pending.take();
                if !text.is_empty() {
                    self.pending(block)?;
                    self.append(node, block, key, &text, None)?;
                }
                self.flush()?;
                self.pending = restore;
            }
            _ => {
                self.flush()?;
                let id = self.add(node, key, BoxKind::Block { flow_root: true, root: false, absolute }, container)?;
                self.boxes[id.index()].absolute = absolute;
                if !text.is_empty() {
                    self.pending(id)?;
                    self.append(node, id, key, &text, None)?;
                    self.flush()?;
                }
            }
        }
        Ok(())
    }
    /// A replaced element or atomic inline in its paragraph: the object
    /// control in the text and the box as the text box's next child.
    fn append_object(&mut self, node: NodeId, container: BoxId, style: impl Into<StyleKey>, kind: BoxKind) -> Result<()> {
        let style = style.into();
        self.append(node, container, style, ControlKind::Object.carrier(), Some(ControlKind::Object))?;
        let text_box = self.pending.as_ref().ok_or(Error::Invalid)?.text_box;
        self.add(node, style, kind, text_box)?;
        Ok(())
    }
    fn text(&mut self, node: NodeId, container: BoxId, style: impl Into<StyleKey>) -> Result<()> {
        let value = self.document.value(node)?.ok_or(Error::Invalid)?;
        self.append(node, container, style, value, None)
    }
    fn append(&mut self, node: NodeId, container: BoxId, style: impl Into<StyleKey>, value: &str, control: Option<ControlKind>) -> Result<()> {
        let style = style.into();
        if value.is_empty() {
            return Ok(());
        }
        if self.styles.snapshot(style).is_none() {
            if !value.bytes().all(|b| b.is_ascii_whitespace()) {
                self.note("text outside a styled element dropped", node);
            }
            return Ok(());
        }
        self.progress.source_bytes = self.progress.source_bytes.checked_add(value.len()).ok_or(Error::Limit)?;
        self.progress.spans += 1;
        if self.progress.source_bytes > self.limits.text_bytes || self.progress.spans > self.limits.spans {
            return Err(Error::Limit);
        }
        let max_bytes = self.limits.paragraph_bytes;
        let p = self.pending(container)?;
        let from = p.source.len();
        if from.checked_add(value.len()).is_none_or(|n| n > max_bytes || n > u32::MAX as usize) {
            return Err(Error::Limit);
        }
        p.source.push_str(value);
        p.spans.push(SourceSpan { range: from as u32..p.source.len() as u32, node, style_node: style });
        if let Some(kind) = control {
            p.controls.push((from as u32, kind));
        }
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
                if let Some(after) = frame.after.take() {
                    let container = frame.container;
                    self.generated(after, container, PseudoElement::After)?;
                    continue;
                }
                let close = frame.close;
                let inline = frame.inline;
                let restore = frame.restore.take();
                self.stack.pop();
                if inline {
                    self.close_inline()?;
                }
                if close {
                    self.flush()?
                }
                if restore.is_some() {
                    // The atomic inline's content is built; its paragraph goes on.
                    if self.pending.is_some() {
                        return Err(Error::Invalid);
                    }
                    self.pending = restore.map(|p| *p);
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
                    let styles = self.styles.clone();
                    let s = styles.snapshot(node).ok_or(Error::Stale)?;
                    let Value::Display(display) = s.value(Property::Display) else { return Err(Error::Invalid) };
                    let display = *display;
                    if display == Display::None {
                        continue;
                    }
                    let name = self.document.name(node)?;
                    let namespace = self.document.namespace(node)?;
                    if s.axes().writing_mode != WritingMode::HorizontalTb {
                        self.note("vertical writing mode laid out horizontally", node);
                    }
                    // Fixed boxes follow the viewport's containing block; at the
                    // initial scroll that is where absolute places them. Sticky
                    // boxes sit where relative would until they would scroll off.
                    let position = match s.value(Property::Position) {
                        Value::Keyword(Keyword::Absolute) => Keyword::Absolute,
                        Value::Keyword(Keyword::Fixed) => {
                            self.note("position: fixed as absolute", node);
                            Keyword::Absolute
                        }
                        Value::Keyword(Keyword::Relative) => Keyword::Relative,
                        Value::Keyword(Keyword::Sticky) => {
                            self.note("position: sticky as relative", node);
                            Keyword::Relative
                        }
                        _ => Keyword::Static,
                    };
                    let absolute = position == Keyword::Absolute;
                    if !matches!(s.value(Property::Float), Value::Keyword(Keyword::None)) {
                        self.note("float in normal flow", node);
                    }
                    if !matches!(s.value(Property::Clear), Value::Keyword(Keyword::None)) {
                        self.note("clear ignored", node);
                    }
                    if namespace == Some(crate::dom::Namespace::Svg) && name == Some("svg") {
                        if display == Display::Contents {
                            continue;
                        }
                        if matches!(display, Display::Box { outer: Outer::Inline, .. }) && !matches!(s.value(Property::VerticalAlign), Value::Keyword(Keyword::Baseline | Keyword::Middle)) {
                            self.note("vertical-align on SVG", node);
                        }
                        if position == Keyword::Relative {
                            self.note("relative offset on SVG ignored", node);
                        }
                        let vector = u32::try_from(self.vectors.len()).map_err(|_| Error::Limit)?;
                        let loaded = crate::svg::Viewport::load_with_sources(
                            self.document,
                            &self.styles,
                            node,
                            self.limits.nodes.saturating_sub(self.progress.visited) + 1,
                            self.limits.vector_work.saturating_sub(self.vector_work),
                            self.limits.vector_bytes.saturating_sub(self.vector_bytes),
                            &mut self.clip_sources,
                            &mut self.fallbacks,
                            self.cancel,
                        );
                        let source = match loaded {
                            Ok(source) => source,
                            Err(e @ (Error::Cancelled | Error::Limit | Error::Stale | Error::Dom(_))) => return Err(e),
                            Err(_) => {
                                // Keep its space: the viewport's attribute size, else SVG's default object size.
                                self.note("SVG not rendered", node);
                                let natural = replaced_natural(self.document, node, [300., 150.])?;
                                self.replaced(node, display, container, natural, absolute)?;
                                continue;
                            }
                        };
                        self.vector_bytes = self.vector_bytes.checked_add(source.payload_bytes()).ok_or(Error::Limit)?;
                        if self.vector_bytes.saturating_add(self.clip_sources.bytes) > self.limits.vector_bytes {
                            return Err(Error::Limit);
                        }
                        self.vector_work += source.work;
                        self.progress.visited += source.visited.saturating_sub(1);
                        self.vectors.push(source);
                        match display {
                            Display::Box { outer: Outer::Inline, .. } if !absolute => self.append_object(node, container, node, BoxKind::InlineVector { vector })?,
                            _ => {
                                self.flush()?;
                                let id = self.add(node, node, BoxKind::BlockVector { vector }, container)?;
                                self.boxes[id.index()].absolute = absolute;
                            }
                        }
                        continue;
                    }
                    if matches!(namespace, Some(crate::dom::Namespace::Svg | crate::dom::Namespace::MathMl)) {
                        // SVG children outside a root and MathML have no boxes yet.
                        self.note(if namespace == Some(crate::dom::Namespace::MathMl) { "MathML skipped" } else { "SVG element outside svg skipped" }, node);
                        continue;
                    }
                    if !matches!(s.value(Property::ClipPath), Value::Keyword(Keyword::None)) {
                        self.note("clip-path on HTML ignored", node);
                    }
                    if let Some(name @ ("br" | "wbr")) = name {
                        if display == Display::Contents {
                            continue;
                        }
                        if !matches!(display, Display::Box { inner: Inner::Flow, list_item: false, .. })
                            || position != Keyword::Static
                            || !matches!(s.value(Property::VerticalAlign), Value::Keyword(Keyword::Baseline))
                            || [Property::MarginLeft, Property::MarginRight, Property::PaddingLeft, Property::PaddingRight]
                                .into_iter()
                                .any(|p| !matches!(s.value(p), Value::Keyword(Keyword::Auto)) && s.value(p).scalar() != Some(0.))
                            || [Property::BorderLeftWidth, Property::BorderRightWidth, Property::BorderTopWidth, Property::BorderBottomWidth]
                                .into_iter()
                                .any(|p| s.used_border_width(p).unwrap_or(0.) != 0.)
                        {
                            self.note("styled line break", node);
                        }
                        let kind = if name == "br" { ControlKind::Break } else { ControlKind::Opportunity };
                        self.append(node, container, node, kind.carrier(), Some(kind))?;
                        continue;
                    }
                    if name == Some("img") {
                        if display == Display::Contents {
                            continue;
                        }
                        if !matches!(s.value(Property::VerticalAlign), Value::Keyword(Keyword::Baseline)) {
                            self.note("vertical-align on image", node);
                        }
                        if position == Keyword::Relative {
                            self.note("relative offset on image ignored", node);
                        }
                        let Some(image) = self.images.as_ref().and_then(|images| images.index(node)) else {
                            // Not decoded (blocked, failed, unsupported format): Chrome
                            // reserves the attribute size and otherwise shows nothing.
                            self.note("image not decoded", node);
                            let natural = replaced_natural(self.document, node, [0., 0.])?;
                            self.replaced(node, display, container, natural, absolute)?;
                            continue;
                        };
                        match display {
                            Display::Box { outer: Outer::Inline, .. } if !absolute => self.append_object(node, container, node, BoxKind::InlineImage { image })?,
                            _ => {
                                self.flush()?;
                                let id = self.add(node, node, BoxKind::BlockImage { image }, container)?;
                                self.boxes[id.index()].absolute = absolute;
                            }
                        }
                        continue;
                    }
                    if let Some(name @ ("input" | "textarea" | "select" | "video" | "audio" | "iframe" | "object" | "embed" | "canvas")) = name {
                        if display == Display::Contents {
                            continue;
                        }
                        self.note(
                            match name {
                                "input" | "textarea" | "select" => "form control as an empty box",
                                "video" | "audio" => "media as an empty box",
                                "iframe" | "object" | "embed" => "embedded document as an empty box",
                                _ => "canvas as an empty box",
                            },
                            node,
                        );
                        let default = match name {
                            "input" => match self.document.attribute(node, "type")?.map(str::trim) {
                                Some(t) if t.eq_ignore_ascii_case("checkbox") || t.eq_ignore_ascii_case("radio") => [13., 13.],
                                Some(t) if t.eq_ignore_ascii_case("hidden") => [0., 0.],
                                Some(t) if t.eq_ignore_ascii_case("submit") || t.eq_ignore_ascii_case("button") || t.eq_ignore_ascii_case("reset") => [64., 21.],
                                Some(t) if t.eq_ignore_ascii_case("range") => [129., 21.],
                                Some(t) if t.eq_ignore_ascii_case("color") => [50., 27.],
                                _ => [153., 21.],
                            },
                            "textarea" => [184., 54.],
                            "select" => [120., 19.],
                            "audio" => [300., 54.],
                            _ => [300., 150.],
                        };
                        let natural = if name == "input" || name == "textarea" || name == "select" { default } else { replaced_natural(self.document, node, default)? };
                        self.replaced(node, display, container, natural, absolute)?;
                        continue;
                    }
                    match display {
                        Display::Contents => {
                            self.push(node, container, node, false)?;
                            self.generated(node, container, PseudoElement::Before)?;
                        }
                        Display::Box { outer: Outer::Inline, inner: inner @ (Inner::FlowRoot | Inner::Flex | Inner::Grid | Inner::Table), .. } => {
                            if inner == Inner::Table {
                                self.note("inline-table as inline-block", node);
                            }
                            if position != Keyword::Static {
                                self.note("positioned inline", node);
                            }
                            if !matches!(s.value(Property::VerticalAlign), Value::Keyword(Keyword::Baseline | Keyword::Middle)) {
                                self.note("vertical-align on an atomic inline", node);
                            }
                            self.atomic(node, container, matches!(inner, Inner::Flex | Inner::Grid))?;
                        }
                        Display::Box { outer: Outer::Inline, inner, list_item } => {
                            if inner == Inner::Ruby {
                                self.note("ruby as inline", node);
                            }
                            if list_item {
                                self.note("inline list item as inline", node);
                            }
                            if position != Keyword::Static {
                                self.note("positioned inline", node);
                            }
                            if !matches!(s.value(Property::VerticalAlign), Value::Keyword(Keyword::Baseline)) {
                                self.note("vertical-align on inline", node);
                            }
                            // Block-axis padding does not change line fitting
                            // or line height. Its decoration is handled by paint.
                            for property in [Property::BorderLeftWidth, Property::BorderRightWidth, Property::BorderTopWidth, Property::BorderBottomWidth] {
                                if s.used_border_width(property)? != 0. {
                                    self.note("inline borders ignored", node);
                                    break;
                                }
                            }
                            self.open_inline(node, container)?;
                            self.push(node, container, node, false)?;
                            self.stack.last_mut().unwrap().inline = true;
                            self.generated(node, container, PseudoElement::Before)?;
                        }
                        Display::Box { outer: Outer::Block | Outer::RunIn, inner, list_item } => {
                            if matches!(display, Display::Box { outer: Outer::RunIn, .. }) {
                                self.note("run-in as block", node);
                            }
                            match inner {
                                Inner::Flow | Inner::FlowRoot | Inner::Flex | Inner::Grid | Inner::Table => {}
                                Inner::Ruby => self.note("ruby as block", node),
                            }
                            if list_item {
                                self.note("list marker not drawn", node);
                            }
                            self.flush()?;
                            let root = self.boxes[container.index()].kind == BoxKind::Viewport;
                            let id = self.add(node, node, BoxKind::Block { flow_root: inner != Inner::Flow, root, absolute }, container)?;
                            self.push(node, id, node, true)?;
                            self.generated(node, id, PseudoElement::Before)?;
                        }
                        Display::Internal(internal) => {
                            use crate::css::properties::Internal as I;
                            match internal {
                                I::TableColumn | I::TableColumnGroup => continue, // No box of their own.
                                I::RubyBase | I::RubyText | I::RubyBaseContainer | I::RubyTextContainer => {
                                    self.note("ruby as inline", node);
                                    self.open_inline(node, container)?;
                                    self.push(node, container, node, false)?;
                                    self.stack.last_mut().unwrap().inline = true;
                                }
                                _ => {
                                    self.flush()?;
                                    let id = self.add(node, node, BoxKind::Block { flow_root: true, root: false, absolute }, container)?;
                                    if internal == I::TableCell {
                                        let span = |name: &str| -> Result<u16> {
                                            Ok(self.document.attribute(node, name)?.and_then(|v| v.trim().parse::<u16>().ok()).filter(|n| *n >= 1).unwrap_or(1).min(1000))
                                        };
                                        self.boxes[id.index()].span = [span("colspan")?, span("rowspan")?];
                                    }
                                    self.push(node, id, node, true)?;
                                    self.generated(node, id, PseudoElement::Before)?;
                                }
                            }
                        }
                        Display::None => unreachable!(),
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
        // Nested paragraphs (atomic inlines) finish before their parents; the
        // control index needs source order.
        let mut controls = self.controls;
        controls.sort_by_key(|c| (c.source, c.offset));
        Ok(Arc::new(Tree {
            images: self.images,
            vectors: self.vectors.into_boxed_slice(),
            text_bytes,
            document: self.document.id(),
            revision: self.document.revision(),
            styles: self.styles,
            inlines: self.inlines.into_boxed_slice(),
            controls: controls.into_boxed_slice(),
            boxes: self.boxes.into_boxed_slice(),
            sources: self.sources.into_boxed_slice(),
        }))
    }
}
/// The natural CSS-pixel size of a replaced element the engine does not
/// render: its width/height attributes (whole CSS pixels, as HTML's
/// dimension rules read them) over the given default.
fn replaced_natural(document: &Document, node: NodeId, default: [f32; 2]) -> Result<[f32; 2]> {
    let mut natural = default;
    for (i, attribute) in ["width", "height"].into_iter().enumerate() {
        let Some(value) = document.attribute(node, attribute)? else { continue };
        let value = value.trim_start();
        let digits: String = value.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        if digits.is_empty() || value[digits.len()..].trim_start().starts_with('%') {
            continue;
        }
        if let Ok(v) = digits.parse::<f32>()
            && v.is_finite()
            && v >= 0.
        {
            natural[i] = v.min(1e6);
        }
    }
    Ok(natural)
}
impl BuildJob<'_> {
    /// Add an unrendered replaced element with the given natural size as an
    /// atomic inline or a block, following its display.
    fn replaced(&mut self, node: NodeId, display: Display, container: BoxId, natural: [f32; 2], absolute: bool) -> Result<()> {
        match display {
            Display::Box { outer: Outer::Inline, .. } if !absolute => self.append_object(node, container, node, BoxKind::InlineReplaced { natural }),
            _ => {
                self.flush()?;
                let id = self.add(node, node, BoxKind::BlockReplaced { natural }, container)?;
                self.boxes[id.index()].absolute = absolute;
                Ok(())
            }
        }
    }
}
