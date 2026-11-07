use super::*;
use crate::{
    css::{
        properties::{Display, Inner, Outer},
        style::{Keyword, Property, Style, StyleTree, Value, WritingMode},
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
    pub style_node: NodeId,
    pub kind: BoxKind,
    /// Absolutely positioned: laid out after its containing block, out of flow.
    pub absolute: bool,
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
    style: NodeId,
    close: bool,
    inline: bool,
}
struct Pending {
    container: BoxId,
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
    vector_nodes: std::collections::HashMap<NodeId, u32>,
    replaced_nodes: std::collections::HashMap<NodeId, [f32; 2]>,
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
            vector_nodes: Default::default(),
            replaced_nodes: Default::default(),
            vector_bytes: 0,
            vector_work: 0,
            fallbacks: Fallbacks::default(),
            document,
            styles,
            limits,
            cancel,
            boxes: vec![BoxNode {
                node: document.root(),
                style_node: document.root(),
                kind: BoxKind::Viewport,
                absolute: false,
                parent: None,
                first_child: None,
                last_child: None,
                next_sibling: None,
            }],
            sources: Vec::new(),
            inlines: Vec::new(),
            controls: Vec::new(),
            open: Vec::new(),
            stack: vec![Cursor { next: document.first_child(document.root())?, container: root, style: document.root(), close: false, inline: false }],
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
        let absolute = matches!(kind, BoxKind::Block { absolute: true, .. });
        self.boxes.push(BoxNode { node, style_node, kind, absolute, parent: Some(parent), first_child: None, last_child: None, next_sibling: None });
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
            && (self.styles.snapshot(self.boxes[p.container.index()].style_node).is_some_and(super::flow::is_flex)
                || p.spans.iter().all(|s| self.styles.snapshot(s.style_node).and_then(|s| WhiteSpace::for_style(s).ok()).is_some_and(|w| w.collapse == Collapse::Collapse)))
        {
            return Ok(());
        }
        let owner = p.spans.first().map_or(self.boxes[p.container.index()].style_node, |s| s.style_node);
        let source = u32::try_from(self.sources.len()).map_err(|_| Error::Limit)?;
        let text_box = self.add(self.boxes[p.container.index()].node, owner, BoxKind::InlineContent { source }, p.container)?;
        for &(at, kind) in &p.controls {
            if kind == ControlKind::Object {
                check(self.cancel)?;
                let span = p.spans.get(p.spans.partition_point(|s| s.range.end <= at)).filter(|s| s.range.start == at).ok_or(Error::Invalid)?;
                let kind = if let Some(&vector) = self.vector_nodes.get(&span.node) {
                    BoxKind::InlineVector { vector }
                } else if let Some(&natural) = self.replaced_nodes.get(&span.node) {
                    BoxKind::InlineReplaced { natural }
                } else {
                    let image = self.images.as_ref().and_then(|images| images.index(span.node)).ok_or(Error::Invalid)?;
                    BoxKind::InlineImage { image }
                };
                self.add(span.node, span.style_node, kind, text_box)?;
            }
        }
        let start = u32::try_from(self.inlines.len()).map_err(|_| Error::Limit)?;
        self.inlines.extend(p.inlines);
        let end = u32::try_from(self.inlines.len()).map_err(|_| Error::Limit)?;
        self.controls.extend(p.controls.into_iter().map(|(offset, kind)| Control { source, offset, kind }));
        self.sources.push(TextSource { container: p.container, source: Arc::from(p.source), spans: p.spans.into_boxed_slice(), inlines: start..end });
        Ok(())
    }
    fn pending(&mut self, container: BoxId) -> Result<&mut Pending> {
        if self.pending.is_none() {
            let mut inlines = Vec::new();
            for open in self.open.iter_mut().filter(|o| o.container == container) {
                open.index = inlines.len();
                inlines.push(InlineBox { node: open.node, range: 0..0, first: open.first, last: false });
            }
            self.pending = Some(Pending { container, source: String::new(), spans: Vec::new(), inlines, controls: Vec::new() });
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
    fn push(&mut self, node: NodeId, container: BoxId, style: NodeId, close: bool) -> Result<()> {
        if self.stack.len() >= self.limits.depth {
            return Err(Error::Limit);
        }
        self.stack.push(Cursor { next: self.document.first_child(node)?, container, style, close, inline: false });
        Ok(())
    }
    fn text(&mut self, node: NodeId, container: BoxId, style: NodeId) -> Result<()> {
        let value = self.document.value(node)?.ok_or(Error::Invalid)?;
        self.append(node, container, style, value, None)
    }
    fn append(&mut self, node: NodeId, container: BoxId, style: NodeId, value: &str, control: Option<ControlKind>) -> Result<()> {
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
                let close = frame.close;
                let inline = frame.inline;
                self.stack.pop();
                if inline {
                    self.close_inline()?;
                }
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
                        self.vector_nodes.insert(node, vector);
                        match display {
                            Display::Box { outer: Outer::Inline, .. } if !absolute => self.append(node, container, node, ControlKind::Object.carrier(), Some(ControlKind::Object))?,
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
                            Display::Box { outer: Outer::Inline, .. } if !absolute => self.append(node, container, node, ControlKind::Object.carrier(), Some(ControlKind::Object))?,
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
                        Display::Contents => self.push(node, container, node, false)?,
                        Display::Box { outer: Outer::Inline, inner, list_item } => {
                            // Inline-level boxes that are not plain inlines (inline-block,
                            // inline-flex, inline-grid, inline-table) flow their content
                            // inline until atomic inlines exist.
                            if inner != Inner::Flow || list_item {
                                self.note(
                                    match inner {
                                        Inner::FlowRoot => "inline-block as inline",
                                        Inner::Flex => "inline-flex as inline",
                                        Inner::Grid => "inline-grid as inline",
                                        Inner::Table => "inline-table as inline",
                                        Inner::Ruby => "ruby as inline",
                                        Inner::Flow => "inline list item as inline",
                                    },
                                    node,
                                );
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
                        }
                        Display::Box { outer: Outer::Block | Outer::RunIn, inner, list_item } => {
                            if matches!(display, Display::Box { outer: Outer::RunIn, .. }) {
                                self.note("run-in as block", node);
                            }
                            match inner {
                                Inner::Flow | Inner::FlowRoot | Inner::Flex => {}
                                Inner::Grid => self.note("grid as block", node),
                                Inner::Table => self.note("table as block", node),
                                Inner::Ruby => self.note("ruby as block", node),
                            }
                            if list_item {
                                self.note("list marker not drawn", node);
                            }
                            self.flush()?;
                            let root = self.boxes[container.index()].kind == BoxKind::Viewport;
                            let id = self.add(node, node, BoxKind::Block { flow_root: inner != Inner::Flow, root, absolute }, container)?;
                            self.push(node, id, node, true)?;
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
                                    self.note("table part as block", node);
                                    self.flush()?;
                                    let id = self.add(node, node, BoxKind::Block { flow_root: true, root: false, absolute }, container)?;
                                    self.push(node, id, node, true)?;
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
        Ok(Arc::new(Tree {
            images: self.images,
            vectors: self.vectors.into_boxed_slice(),
            text_bytes,
            document: self.document.id(),
            revision: self.document.revision(),
            styles: self.styles,
            inlines: self.inlines.into_boxed_slice(),
            controls: self.controls.into_boxed_slice(),
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
            Display::Box { outer: Outer::Inline, .. } if !absolute => {
                self.replaced_nodes.insert(node, natural);
                self.append(node, container, node, ControlKind::Object.carrier(), Some(ControlKind::Object))
            }
            _ => {
                self.flush()?;
                let id = self.add(node, node, BoxKind::BlockReplaced { natural }, container)?;
                self.boxes[id.index()].absolute = absolute;
                Ok(())
            }
        }
    }
}
