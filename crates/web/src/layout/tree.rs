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
        /// A checkbox or radio button: on the baseline by its border box's
        /// bottom, its margins aside (as Chrome lines them up).
        toggle: bool,
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
    /// A list item's outside marker: positioned before its item's content.
    pub marker: bool,
    /// A one-line field's or a select's box: its text centred in its height,
    /// as Chrome centres it (whatever its `appearance`).
    pub centred: bool,
    /// A select's box: its text is every option's label, the chosen one
    /// first, a line each; it is as wide as the widest and as tall as the
    /// first (the rest clipped), as Chrome sizes a select by its options.
    pub first_line: bool,
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
    viewport_elements: super::overflow::ViewportElements,
    vectors: Arc<[crate::svg::Viewport]>,
    images: Option<Arc<crate::image::Set>>,
    /// Shared with the trees restyled from this one (`with_styles`, every
    /// frame of a live page): the same boxes, never copied.
    pub(super) boxes: Arc<[BoxNode]>,
    pub(super) sources: Arc<[TextSource]>,
    pub(super) styles: Arc<StyleTree>,
    inlines: Arc<[InlineBox]>,
    controls: Arc<[Control]>,
    document: u64,
    revision: u64,
    text_bytes: usize,
}
impl Tree {
    /// This tree with other styles for the same boxes (a restyle that
    /// moved nothing), for the document as it is now (its revision moved
    /// on; the styles were made against it) and with images rebound to
    /// it (a redrawn canvas is the same box with new pixels).
    pub fn with_styles(&self, styles: Arc<StyleTree>, document: &Document, images: Option<Arc<crate::image::Set>>) -> Self {
        Self {
            viewport_elements: self.viewport_elements,
            vectors: self.vectors.clone(),
            images: images.or_else(|| self.images.clone()),
            boxes: self.boxes.clone(),
            sources: self.sources.clone(),
            styles,
            inlines: self.inlines.clone(),
            controls: self.controls.clone(),
            document: self.document,
            revision: if document.id() == self.document { document.revision() } else { self.revision },
            text_bytes: self.text_bytes,
        }
    }
    /// This tree with the text the document has now for `texts` (text
    /// nodes whose data changed) and `holders` (inline elements whose
    /// children, all text, came or went), in place: the source range each
    /// covers rewritten, the spans, inline boxes and controls after it
    /// moved along. None when one can't be (no span or inline box for it,
    /// an element among the children, a box split by a block, text that
    /// was or becomes only whitespace, whose boxes depend on where it is):
    /// built again.
    pub fn with_text(&self, document: &Document, texts: &[NodeId], holders: &[NodeId]) -> Option<Self> {
        let blank = |t: &str| t.bytes().all(|b| b.is_ascii_whitespace());
        let mut sources: Vec<TextSource> = self.sources.iter().map(|t| TextSource { container: t.container, source: t.source.clone(), spans: t.spans.clone(), inlines: t.inlines.clone() }).collect();
        let mut inlines: Vec<InlineBox> = self.inlines.to_vec();
        let mut controls: Vec<Control> = self.controls.to_vec();
        let mut text_bytes = self.text_bytes;
        // Rewrite `range` of source `si` with `text`, its spans `replaced`.
        let mut rewrite = |sources: &mut Vec<TextSource>, inlines: &mut Vec<InlineBox>, si: usize, range: Range<u32>, text: &str, replaced: Vec<SourceSpan>| -> Option<()> {
            let source = &mut sources[si];
            let delta = text.len() as i64 - (range.end - range.start) as i64;
            let mut out = String::with_capacity((source.source.len() as i64 + delta).max(0) as usize);
            out.push_str(&source.source[..range.start as usize]);
            out.push_str(text);
            out.push_str(&source.source[range.end as usize..]);
            if out.len() > u32::MAX as usize {
                return None;
            }
            let shift = |x: u32| (i64::from(x) + delta) as u32;
            // (The range is never empty: blank text is built again.)
            if range.start >= range.end {
                return None;
            }
            let mut spans: Vec<SourceSpan> = Vec::with_capacity(source.spans.len() + replaced.len());
            let mut placed = false;
            for s in source.spans.iter() {
                if s.range.end <= range.start {
                    spans.push(s.clone());
                } else if s.range.start >= range.end {
                    if !placed {
                        spans.extend(replaced.iter().cloned());
                        placed = true;
                    }
                    spans.push(SourceSpan { range: shift(s.range.start)..shift(s.range.end), ..s.clone() });
                } else if s.range.start < range.start || s.range.end > range.end {
                    // A span across the range's edge: not this tree's to patch.
                    return None;
                }
            }
            if !placed {
                spans.extend(replaced);
            }
            for b in &mut inlines[source.inlines.start as usize..source.inlines.end as usize] {
                if b.range.start >= range.end && b.range.start > range.start {
                    b.range.start = shift(b.range.start);
                }
                if b.range.end >= range.end {
                    b.range.end = shift(b.range.end);
                }
            }
            for c in controls.iter_mut().filter(|c| c.source as usize == si && c.offset >= range.end) {
                c.offset = shift(c.offset);
            }
            text_bytes = (text_bytes as i64 + delta).max(0) as usize;
            source.source = Arc::from(out);
            source.spans = spans.into_boxed_slice();
            Some(())
        };
        for &node in texts {
            // Gone: its parent's children changed too, rewritten there.
            if !document.is_connected(node).unwrap_or(false) {
                continue;
            }
            let value = document.value(node).ok()??;
            let (si, k) = sources.iter().enumerate().find_map(|(si, t)| t.spans.iter().position(|span| span.node == node).map(|k| (si, k)))?;
            if sources[si].spans.iter().filter(|span| span.node == node).count() != 1 {
                return None;
            }
            let span = sources[si].spans[k].clone();
            let old = &sources[si].source[span.range.start as usize..span.range.end as usize];
            if blank(old) || blank(value) {
                return None;
            }
            if old == value {
                continue;
            }
            let end = span.range.start + value.len() as u32;
            rewrite(&mut sources, &mut inlines, si, span.range.clone(), value, vec![SourceSpan { range: span.range.start..end, ..span }])?;
        }
        for &holder in holders {
            // The range its content covers: its inline box, whole, when it has
            // one; else its text's spans (styled by it), one run of them.
            let key = StyleKey::from(holder);
            let mut found = inlines.iter().enumerate().filter(|(_, b)| b.node == holder);
            let (si, range) = match found.next() {
                Some((bi, b)) => {
                    if found.next().is_some() || !(b.first && b.last) {
                        return None;
                    }
                    let range = b.range.clone();
                    (sources.iter().position(|t| (t.inlines.start as usize..t.inlines.end as usize).contains(&bi))?, range)
                }
                None => {
                    let si = sources.iter().position(|t| t.spans.iter().any(|s| s.style_node == key))?;
                    let at: Vec<usize> = sources[si].spans.iter().enumerate().filter(|(_, s)| s.style_node == key).map(|(i, _)| i).collect();
                    if sources.iter().skip(si + 1).any(|t| t.spans.iter().any(|s| s.style_node == key)) || at.windows(2).any(|w| w[1] != w[0] + 1) {
                        return None;
                    }
                    let spans = &sources[si].spans;
                    (si, spans[at[0]].range.start..spans[*at.last()?].range.end)
                }
            };
            // What was there: text spans of its own children only.
            let olds: Vec<&SourceSpan> = sources[si].spans.iter().filter(|s| s.range.start >= range.start && s.range.end <= range.end).collect();
            let style = olds.first().map(|s| s.style_node)?;
            if olds.iter().any(|s| s.style_node != style || document.node_type(s.node).ok() != Some(NodeType::Text)) || blank(&sources[si].source[range.start as usize..range.end as usize]) {
                return None;
            }
            let mut text = String::new();
            let mut spans = Vec::new();
            for child in document.children(holder).ok()? {
                if document.node_type(child).ok()? != NodeType::Text {
                    return None;
                }
                let value = document.value(child).ok()??;
                if value.is_empty() {
                    continue;
                }
                let start = range.start + text.len() as u32;
                text.push_str(value);
                spans.push(SourceSpan { range: start..range.start + text.len() as u32, node: child, style_node: style });
            }
            if blank(&text) {
                return None;
            }
            rewrite(&mut sources, &mut inlines, si, range, &text, spans)?;
        }
        Some(Self {
            viewport_elements: self.viewport_elements,
            vectors: self.vectors.clone(),
            images: self.images.clone(),
            boxes: self.boxes.clone(),
            sources: sources.into(),
            styles: self.styles.clone(),
            inlines: inlines.into(),
            controls: controls.into(),
            document: self.document,
            revision: if document.id() == self.document { document.revision() } else { self.revision },
            text_bytes,
        })
    }
    /// Where this tree differs from `fresh`, one built anew from the same
    /// document and styles (`OLO_LAYOUT_CHECK`, for trees kept or patched
    /// in place): None when they are the same.
    pub fn differs_from(&self, fresh: &Tree) -> Option<String> {
        if self.boxes.len() != fresh.boxes.len() {
            return Some(format!("{} boxes, fresh {}", self.boxes.len(), fresh.boxes.len()));
        }
        for (i, (a, b)) in self.boxes.iter().zip(fresh.boxes.iter()).enumerate() {
            if a.node != b.node || a.style_node != b.style_node || a.absolute != b.absolute || a.span != b.span || std::mem::discriminant(&a.kind) != std::mem::discriminant(&b.kind) {
                return Some(format!("box {i}"));
            }
        }
        if self.sources.len() != fresh.sources.len() {
            return Some(format!("{} sources, fresh {}", self.sources.len(), fresh.sources.len()));
        }
        for (i, (a, b)) in self.sources.iter().zip(fresh.sources.iter()).enumerate() {
            if a.source != b.source
                || a.container != b.container
                || a.inlines != b.inlines
                || a.spans.len() != b.spans.len()
                || a.spans.iter().zip(b.spans.iter()).any(|(x, y)| x.range != y.range || x.node != y.node || x.style_node != y.style_node)
            {
                return Some(format!("source {i}: {:?} / fresh {:?}", a.source.chars().take(60).collect::<String>(), b.source.chars().take(60).collect::<String>()));
            }
        }
        if self.inlines.len() != fresh.inlines.len() || self.inlines.iter().zip(fresh.inlines.iter()).any(|(a, b)| a.node != b.node || a.range != b.range || a.first != b.first || a.last != b.last) {
            return Some("inline boxes".into());
        }
        if self.controls.len() != fresh.controls.len() || self.controls.iter().zip(fresh.controls.iter()).any(|(a, b)| a.source != b.source || a.offset != b.offset) {
            return Some("controls".into());
        }
        (self.text_bytes != fresh.text_bytes).then(|| "text bytes".into())
    }
    /// The identity of its image set (a set with new pictures is another).
    pub fn images_identity(&self) -> usize {
        self.images.as_ref().map_or(0, |i| Arc::as_ptr(i) as *const u8 as usize)
    }
    pub fn vector(&self, index: u32) -> Option<&crate::svg::Viewport> {
        self.vectors.get(index as usize)
    }
    pub fn vectors(&self) -> &[crate::svg::Viewport] {
        &self.vectors
    }
    /// A box with a natural ratio and no natural size: an SVG with a
    /// viewBox (its width and height attributes, when given, are CSS).
    pub(super) fn ratio_only(&self, id: BoxId) -> bool {
        matches!(self.get(id).map(|b| b.kind), Some(BoxKind::InlineVector { vector } | BoxKind::BlockVector { vector }) if self.vector(vector).is_some_and(|v| v.view_box.is_some_and(|b| b[2] > 0. && b[3] > 0.)))
    }
    pub(super) fn natural(&self, id: BoxId) -> Result<(f64, f64)> {
        match self.get(id).ok_or(Error::Invalid)?.kind {
            BoxKind::InlineImage { image } | BoxKind::BlockImage { image } => {
                let image = self.image(image).ok_or(Error::Invalid)?;
                Ok(image.natural())
            }
            BoxKind::InlineVector { vector } | BoxKind::BlockVector { vector } => Ok(self.vector(vector).ok_or(Error::Invalid)?.natural()),
            BoxKind::InlineReplaced { natural, .. } | BoxKind::BlockReplaced { natural } => Ok((f64::from(natural[0]), f64::from(natural[1]))),
            _ => Err(Error::Invalid),
        }
    }
    pub fn images(&self) -> Option<&Arc<crate::image::Set>> {
        self.images.as_ref()
    }
    pub fn image(&self, index: u32) -> Option<&Arc<crate::image::Image>> {
        self.images.as_ref()?.image(index)
    }
    /// A `background-image` by the URL as written in the stylesheet.
    pub fn background(&self, url: &str) -> Option<&Arc<crate::image::Image>> {
        self.images.as_ref()?.background(url)
    }
    pub fn text_bytes(&self) -> usize {
        self.text_bytes
    }
    pub fn boxes(&self) -> &[BoxNode] {
        &self.boxes
    }
    /// The boxes a document node generated, in tree order.
    pub fn boxes_of(&self, node: NodeId) -> impl Iterator<Item = BoxId> + '_ {
        self.boxes.iter().enumerate().filter(move |(_, b)| b.node == node).filter_map(|(i, _)| BoxId::new(i).ok())
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
    pub fn viewport_overflow(&self) -> super::ViewportOverflow {
        self.viewport_elements.resolve(&self.styles)
    }
    /// The styles to change in place, when nothing else holds them, for
    /// the document as it is now (see `with_styles`).
    pub(super) fn styles_mut(&mut self, document: &Document) -> Option<&mut StyleTree> {
        if self.document == document.id() {
            self.revision = document.revision();
        }
        Arc::get_mut(&mut self.styles)
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
    /// The enclosing list box, carried through wrappers in constant time.
    list_owner: Option<NodeId>,
    close: bool,
    inline: bool,
    /// The element whose `::after` content comes once its children are built.
    after: Option<NodeId>,
    /// The paragraph an atomic inline interrupted, resumed when its content is built.
    restore: Option<Box<Pending>>,
}
#[derive(Clone, Copy)]
struct ListCounter {
    value: i64,
    step: i32,
}
struct Counter {
    name: Arc<str>,
    value: i32,
    origin_depth: usize,
    // A reset shadowing an ancestor ends with its own element. Without
    // an ancestor of that name, following siblings inherit the counter.
    end_depth: usize,
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
    /// The last ordinal given in each list (by the list's node).
    list_counters: crate::dom::NodeMap<ListCounter>,
    /// Active CSS counters, with their originating and ending scopes.
    counters: Vec<Counter>,
    counter_max_end: usize,
    /// How deep quotes are nested (open-quote and close-quote).
    quote_depth: usize,
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
                marker: false,
                centred: false,
                first_line: false,
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
            list_counters: Default::default(),
            counters: Vec::new(),
            counter_max_end: 0,
            quote_depth: 0,
            stack: vec![Cursor {
                next: document.composed_first_child(document.root())?,
                container: root,
                style: document.root().into(),
                list_owner: None,
                close: false,
                inline: false,
                after: None,
                restore: None,
            }],
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
        self.boxes.push(BoxNode {
            node,
            style_node,
            kind,
            absolute,
            marker: false,
            centred: false,
            first_line: false,
            span: [1, 1],
            parent: Some(parent),
            first_child: None,
            last_child: None,
            next_sibling: None,
        });
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
        let list_owner = self.child_list_owner(node)?;
        self.stack.push(Cursor { next: self.document.composed_first_child(node)?, container, style: style.into(), list_owner, close, inline: false, after: Some(node), restore: None });
        Ok(())
    }
    /// An atomic inline (inline-block, inline-flex, inline-grid, inline-table):
    /// a block formatting context that sits in its paragraph as one object.
    /// Its own content is built with the paragraph set aside, then resumed.
    fn atomic(&mut self, node: NodeId, container: BoxId, style: &Style, absolute: bool) -> Result<()> {
        self.append_object(node, container, node, BoxKind::Block { flow_root: true, root: false, absolute })?;
        let p = self.pending.as_ref().ok_or(Error::Invalid)?;
        let block = self.boxes[p.text_box.index()].last_child.ok_or(Error::Invalid)?;
        self.boxes[block.index()].absolute = absolute;
        if self.stack.len() >= self.limits.depth {
            return Err(Error::Limit);
        }
        let owner = self.stack.last().and_then(|f| f.list_owner);
        let restore = self.pending.take().map(Box::new);
        let list_owner = self.child_list_owner(node)?;
        self.stack.push(Cursor { next: self.document.composed_first_child(node)?, container: block, style: node.into(), list_owner, close: true, inline: false, after: Some(node), restore });
        if matches!(style.value(Property::Display), Value::Display(Display::Box { list_item: true, .. })) {
            self.marker(node, block, style, owner)?;
        }
        self.generated(node, block, PseudoElement::Before)?;
        Ok(())
    }
    fn is_list_owner(&self, node: NodeId) -> Result<bool> {
        Ok(self.document.node_type(node)? == NodeType::Element
            && self.document.namespace(node)? == Some(crate::dom::Namespace::Html)
            && matches!(self.document.name(node)?, Some("ol" | "ul" | "menu"))
            && self.styles.snapshot(node).is_some_and(|s| !matches!(s.value(Property::Display), Value::Display(Display::None | Display::Contents))))
    }
    fn child_list_owner(&self, node: NodeId) -> Result<Option<NodeId>> {
        Ok(if self.is_list_owner(node)? { Some(node) } else { self.stack.last().and_then(|f| f.list_owner) })
    }
    /// Count only rendered items owned by this list. A reversed list needs
    /// this once, not a sibling scan for every item. Nested list boxes own
    /// their descendants; display:contents wrappers do not. The traversal
    /// is iterative, bounded and cancellable like the main tree builder.
    fn reversed_items(&self, owner: NodeId) -> Result<i32> {
        let mut stack = vec![self.document.composed_first_child(owner)?];
        let (mut visited, mut count) = (0usize, 0i32);
        while let Some(next) = stack.last_mut() {
            check(self.cancel)?;
            let Some(node) = *next else {
                stack.pop();
                continue;
            };
            *next = self.document.composed_next_sibling(node)?;
            visited += 1;
            if visited > self.limits.nodes || stack.len() > self.limits.depth {
                return Err(Error::Limit);
            }
            if closed_details_hides(self.document, node)? {
                continue;
            }
            if self.document.node_type(node)? == NodeType::Element {
                let s = self.styles.snapshot(node).ok_or(Error::Stale)?;
                if matches!(s.value(Property::Display), Value::Display(Display::None)) {
                    continue;
                }
                if matches!(s.value(Property::Display), Value::Display(Display::Box { list_item: true, .. })) {
                    count = count.saturating_add(1);
                }
                if self.is_list_owner(node)? {
                    continue;
                }
            }
            stack.push(self.document.composed_first_child(node)?);
        }
        Ok(count)
    }
    fn list_ordinal(&mut self, node: NodeId, s: &Style, owner: Option<NodeId>) -> Result<i32> {
        let owner = owner.or(self.document.composed_parent(node)?).unwrap_or(self.document.root());
        if !self.list_counters.contains_key(&owner) {
            if self.list_counters.len() >= self.limits.nodes {
                return Err(Error::Limit);
            }
            let ordered = self.document.node_type(owner)? == NodeType::Element && self.document.namespace(owner)? == Some(crate::dom::Namespace::Html) && self.document.name(owner)? == Some("ol");
            let reversed = ordered && self.document.attribute(owner, "reversed")?.is_some();
            let step = if reversed { -1 } else { 1 };
            let start = if ordered { self.document.integer_attribute(owner, "start")? } else { None };
            let value = match start {
                Some(n) => i64::from(n) - i64::from(step),
                None if reversed => i64::from(self.reversed_items(owner)?) + 1,
                None => 0,
            };
            self.list_counters.insert(owner, ListCounter { value, step });
        }
        let change = |property| match s.value(property) {
            Value::Content(c) => Self::counter_changes(&c.0)
                .filter(|(name, _)| name.as_ref() == "list-item")
                .fold(None::<i32>, |previous, (_, value)| Some(if property == Property::CounterIncrement { previous.unwrap_or(0).saturating_add(value) } else { value }))
                .map(i64::from),
            _ => None,
        };
        let explicit = if self.document.namespace(node)? == Some(crate::dom::Namespace::Html) && self.document.name(node)? == Some("li") {
            self.document.integer_attribute(node, "value")?.map(i64::from)
        } else {
            None
        };
        let counter = self.list_counters.get_mut(&owner).ok_or(Error::Invalid)?;
        let increment = change(Property::CounterIncrement);
        let value = match change(Property::CounterSet).or(explicit) {
            Some(value) => value,
            None => counter.value.saturating_add(increment.unwrap_or(i64::from(counter.step))),
        };
        // Blink's list ordinal is a saturating signed 32-bit value. In
        // particular, an adversarial value cannot wrap or panic here.
        counter.value = value.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
        Ok(counter.value as i32)
    }
    /// A list item's marker (CSS Lists 3 §3): the counter or bullet for its
    /// `list-style-type`, as a positioned block before the item's content
    /// (`outside`) or the first run of its text (`inside`).
    fn marker(&mut self, node: NodeId, item: BoxId, s: &Style, owner: Option<NodeId>) -> Result<()> {
        // An item advances the list even when its marker is suppressed.
        let ordinal = self.list_ordinal(node, s, owner)?;
        let Value::Keyword(kind) = s.value(Property::ListStyleType) else { return Ok(()) };
        if *kind == Keyword::None {
            return Ok(());
        }
        if !matches!(s.value(Property::ListStyleImage), Value::Keyword(Keyword::None)) {
            self.note("list-style-image as the type's marker", node);
        }
        let text = marker_text(*kind, i64::from(ordinal));
        // (Its text in its `::marker` style when rules give it one: their
        // colour and font.)
        let text_style = if self.styles.pseudo(node, PseudoElement::Marker).is_some() { StyleKey::pseudo(node, PseudoElement::Marker) } else { node.into() };
        if matches!(s.value(Property::ListStylePosition), Value::Keyword(Keyword::Inside))
            || matches!(s.value(Property::Display), Value::Display(Display::Box { outer: Outer::Inline, inner: Inner::Flow, .. }))
        {
            self.pending(item)?;
            return self.append(node, item, text_style, &text, None);
        }
        let marker = self.add(node, node, BoxKind::Block { flow_root: true, root: false, absolute: true }, item)?;
        self.boxes[marker.index()].absolute = true;
        self.boxes[marker.index()].marker = true;
        self.pending(marker)?;
        self.append(node, marker, text_style, &text, None)?;
        self.flush()
    }
    fn add_counter(&mut self, name: Arc<str>, value: i32, depth: usize, end_depth: usize) -> Result<()> {
        if self.counters.len() >= 1024 {
            return Err(Error::Limit);
        }
        self.counter_max_end = self.counter_max_end.max(end_depth);
        self.counters.push(Counter { name, value, origin_depth: depth, end_depth });
        Ok(())
    }
    fn change_counter(&mut self, name: &Arc<str>, value: i32, add: bool, depth: usize) -> Result<()> {
        if let Some(c) = self.counters.iter_mut().rev().find(|c| &c.name == name) {
            c.value = if add { c.value.checked_add(value).unwrap_or(c.value) } else { value };
            Ok(())
        } else {
            self.add_counter(name.clone(), value, depth, depth)
        }
    }
    fn end_counters(&mut self, depth: usize) {
        // Most visited nodes end no counter scope. Only scan the active
        // counters when one actually expires, not on every visited node.
        if self.counter_max_end > depth {
            self.counters.retain(|c| c.end_depth <= depth);
            self.counter_max_end = self.counters.iter().map(|c| c.end_depth).max().unwrap_or(0);
        }
    }
    fn counter_changes(parts: &[crate::css::properties::ContentPart]) -> impl Iterator<Item = (&Arc<str>, i32)> {
        parts.iter().filter_map(|part| match part {
            crate::css::properties::ContentPart::Change(name, value) => Some((name, (*value).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32)),
            _ => None,
        })
    }
    /// Reset, increment, then set. Duplicate increments compound with
    /// signed 32-bit saturation; adding that increment to the counter
    /// leaves its old value when the result would overflow (as Blink).
    fn apply_counters(&mut self, s: &Style, depth: usize) -> Result<()> {
        use crate::css::properties::ContentPart as C;
        let list = |p: Property| -> &[C] {
            match s.value(p) {
                Value::Content(c) => &c.0,
                _ => &[],
            }
        };
        for (name, value) in Self::counter_changes(list(Property::CounterReset)) {
            if let Some(i) = self.counters.iter().rposition(|c| &c.name == name && c.origin_depth == depth) {
                self.counters.remove(i);
            }
            let shadowed = self.counters.iter().any(|c| &c.name == name);
            self.add_counter(name.clone(), value, depth, depth + usize::from(shadowed))?;
        }
        let parts = list(Property::CounterIncrement);
        if parts.len() == 1 {
            if let Some((name, value)) = Self::counter_changes(parts).next() {
                self.change_counter(name, value, true, depth)?;
            }
        } else if !parts.is_empty() {
            let mut increments = std::collections::HashMap::<&Arc<str>, i32>::new();
            for (name, value) in Self::counter_changes(parts) {
                let increment = increments.entry(name).or_default();
                *increment = increment.saturating_add(value);
            }
            for (name, value) in increments {
                self.change_counter(name, value, true, depth)?;
            }
        }
        for (name, value) in Self::counter_changes(list(Property::CounterSet)) {
            self.change_counter(name, value, false, depth)?;
        }
        Ok(())
    }
    /// The element's `::before` or `::after` content, if it has any: text in
    /// the pseudo-element's style, as an inline run of the paragraph, an
    /// atomic inline, or a block of its own (CSS Generated Content 3 §2).
    fn generated(&mut self, node: NodeId, container: BoxId, pseudo: PseudoElement) -> Result<()> {
        let styles = self.styles.clone();
        let Some(s) = styles.pseudo(node, pseudo) else { return Ok(()) };
        let Value::Display(display) = s.value(Property::Display) else { return Err(Error::Invalid) };
        if *display == Display::None {
            return Ok(());
        }
        let depth = self.stack.len();
        let key = StyleKey::pseudo(node, pseudo);
        let mut text = String::new();
        // Its own counters first (a ::before counting itself), within the
        // element.
        if *display != Display::Contents {
            self.apply_counters(s, depth)?;
        }
        if let Value::Content(content) = s.value(Property::Content) {
            use crate::css::properties::ContentPart as C;
            for part in content.0.iter() {
                match part {
                    C::Text(t) => text.push_str(t),
                    C::Attr(name) => text.push_str(self.document.attribute(node, name)?.unwrap_or("")),
                    C::Counter(name, style) => text.push_str(&counter_text(*style, self.counters.iter().rev().find(|c| c.name == *name).map_or(0, |c| i64::from(c.value)))),
                    C::Counters(name, separator, style) => {
                        let values: Vec<String> = self.counters.iter().filter(|c| c.name == *name).map(|c| counter_text(*style, i64::from(c.value))).collect();
                        text.push_str(&if values.is_empty() { counter_text(*style, 0) } else { values.join(separator) });
                    }
                    C::Quote(open, shows) => {
                        // `quotes`: its pairs, or the language's (en: curly
                        // double, then single) for auto.
                        let pairs: Vec<(Arc<str>, Arc<str>)> = match s.value(Property::Quotes) {
                            Value::Content(q) => q.0.iter().filter_map(|p| if let C::Pair(a, b) = p { Some((a.clone(), b.clone())) } else { None }).collect(),
                            Value::Keyword(Keyword::None) => Vec::new(),
                            _ => vec![(Arc::from("\u{201c}"), Arc::from("\u{201d}")), (Arc::from("\u{2018}"), Arc::from("\u{2019}"))],
                        };
                        let level = if *open {
                            let l = self.quote_depth;
                            self.quote_depth += 1;
                            Some(l)
                        } else if self.quote_depth > 0 {
                            self.quote_depth -= 1;
                            Some(self.quote_depth)
                        } else {
                            None
                        };
                        if *shows
                            && let Some(l) = level
                            && let Some((a, b)) = pairs.get(l.min(pairs.len().saturating_sub(1)))
                        {
                            text.push_str(if *open { a } else { b });
                        }
                    }
                    C::Change(..) | C::Pair(..) => {}
                }
            }
        }
        let absolute = matches!(s.value(Property::Position), Value::Keyword(Keyword::Absolute | Keyword::Fixed));
        match *display {
            Display::None => {}
            Display::Contents => self.append(node, container, key, &text, None)?,
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
        self.end_counters(depth);
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
    /// An absolutely positioned box that was inline-level, in a block
    /// container's flow: it stays in its paragraph, as an object taking no
    /// room, so its static position is where it would have been in its
    /// line (CSS 2.2 §10.3.7; the paragraph isn't split around it).
    fn in_line(&self, s: &Style, absolute: bool, container: BoxId) -> bool {
        absolute
            && s.inline_origin()
            && self.boxes.get(container.index()).is_some_and(|b| matches!(b.kind, BoxKind::Block { .. }))
            && self
                .styles
                .snapshot(self.boxes[container.index()].style_node)
                .is_some_and(|c| matches!(c.value(Property::Display), Value::Display(Display::Box { inner: Inner::Flow | Inner::FlowRoot, .. })))
    }
    /// An absolute box of `kind` as an object in its paragraph (`in_line`).
    fn absolute_object(&mut self, node: NodeId, container: BoxId, kind: BoxKind) -> Result<BoxId> {
        self.append_object(node, container, node, kind)?;
        let text_box = self.pending.as_ref().ok_or(Error::Invalid)?.text_box;
        let id = self.boxes[text_box.index()].last_child.ok_or(Error::Invalid)?;
        self.boxes[id.index()].absolute = true;
        Ok(id)
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
            let depth = self.stack.len();
            self.end_counters(depth);
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
                // The counters its elements made end with it.
                let depth = self.stack.len();
                self.end_counters(depth);
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
            let (container, parent_style, list_owner) = (frame.container, frame.style, frame.list_owner);
            frame.next = self.document.composed_next_sibling(node)?;
            self.progress.visited += 1;
            if self.progress.visited > self.limits.nodes {
                return Err(Error::Limit);
            }
            // A closed <details> shows its first <summary> alone (HTML
            // §15.5.15: the rest is in a slot it hides, whatever their own
            // display says).
            if closed_details_hides(self.document, node)? {
                continue;
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
                    if display != Display::Contents {
                        self.apply_counters(s, self.stack.len())?;
                    }
                    let name = self.document.name(node)?;
                    let namespace = self.document.namespace(node)?;
                    if s.axes().writing_mode != WritingMode::HorizontalTb {
                        self.note("vertical writing mode laid out horizontally", node);
                    }
                    // Fixed boxes follow the viewport's containing block: laid
                    // out as absolute at the initial scroll, painted as a layer
                    // the scroll leaves alone. Sticky boxes sit in the flow (no
                    // offsets: `Used::relative` reads the style's own
                    // position) and are painted shifted as the scroll has them
                    // (`Layout::sticky`).
                    let position = match s.value(Property::Position) {
                        Value::Keyword(Keyword::Absolute | Keyword::Fixed) => Keyword::Absolute,
                        Value::Keyword(Keyword::Relative | Keyword::Sticky) => Keyword::Relative,
                        _ => Keyword::Static,
                    };
                    let absolute = position == Keyword::Absolute;
                    // A float is block-level (CSS 2.2 §9.7): an inline-level
                    // box floated becomes a block formatting context.
                    let floated = !absolute && !matches!(s.value(Property::Float), Value::Keyword(Keyword::None));
                    let display = match display {
                        Display::Box { outer: Outer::Inline, inner, list_item } if floated => {
                            Display::Box { outer: Outer::Block, inner: if inner == Inner::Flow { Inner::FlowRoot } else { inner }, list_item }
                        }
                        other => other,
                    };
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
                                self.replaced(node, display, container, natural, absolute, self.in_line(s, absolute, container))?;
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
                            _ if self.in_line(s, absolute, container) => {
                                self.absolute_object(node, container, BoxKind::BlockVector { vector })?;
                            }
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
                    if !matches!(s.value(Property::ClipPath), Value::Keyword(Keyword::None) | Value::Inset(_)) {
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
                    // A canvas the scripts drew on is an image of its pixels; a
                    // video showing a picture, an image of that picture.
                    // An iframe whose document was rendered, an image of it.
                    let drawn_canvas = matches!(name, Some("canvas" | "video" | "iframe")) && self.images.as_ref().is_some_and(|images| images.index(node).is_some());
                    // A video showing nothing yet (no poster, no frame): its
                    // room (its attributes' size, else 300 × 150), as Chrome.
                    if name == Some("video") && !drawn_canvas && display != Display::Contents && matches!(namespace, Some(crate::dom::Namespace::Html)) {
                        let natural = replaced_natural(self.document, node, [300., 150.])?;
                        self.replaced(node, display, container, natural, absolute, self.in_line(s, absolute, container))?;
                        continue;
                    }
                    if name == Some("img") || drawn_canvas {
                        if display == Display::Contents {
                            continue;
                        }
                        if !matches!(s.value(Property::VerticalAlign), Value::Keyword(Keyword::Baseline | Keyword::Middle | Keyword::Top | Keyword::Bottom)) {
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
                            self.replaced(node, display, container, natural, absolute, self.in_line(s, absolute, container))?;
                            continue;
                        };
                        match display {
                            Display::Box { outer: Outer::Inline, .. } if !absolute && !floated => self.append_object(node, container, node, BoxKind::InlineImage { image })?,
                            _ if self.in_line(s, absolute, container) => {
                                self.absolute_object(node, container, BoxKind::BlockImage { image })?;
                            }
                            _ => {
                                self.flush()?;
                                let id = self.add(node, node, BoxKind::BlockImage { image }, container)?;
                                self.boxes[id.index()].absolute = absolute;
                            }
                        }
                        continue;
                    }
                    // Text fields, text areas, selects and buttons show their
                    // text: a block (inline-block by the user-agent sheet)
                    // holding its value, else its placeholder (in the
                    // `::placeholder` style).
                    if let Some(control @ ("input" | "textarea" | "select")) = name
                        && display != Display::Contents
                        && let Some((text, placeholder)) = control_text(self.document, node, control)?
                    {
                        let key: StyleKey =
                            if placeholder && self.styles.pseudo(node, PseudoElement::Placeholder).is_some() { StyleKey::pseudo(node, PseudoElement::Placeholder) } else { node.into() };
                        // (Its `::placeholder` opacity applies when painted.)

                        // An empty field or button is still a line tall.
                        let text = if text.is_empty() && control != "textarea" { "\u{200b}".to_owned() } else { text };
                        match display {
                            Display::Box { outer: Outer::Inline, .. } if !absolute && !floated => {
                                self.append_object(node, container, node, BoxKind::Block { flow_root: true, root: false, absolute: false })?;
                                let p = self.pending.as_ref().ok_or(Error::Invalid)?;
                                let block = self.boxes[p.text_box.index()].last_child.ok_or(Error::Invalid)?;
                                self.boxes[block.index()].centred = control != "textarea";
                                self.boxes[block.index()].first_line = control == "select";
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
                                let id = self.add(node, node, BoxKind::Block { flow_root: true, root: false, absolute }, container)?;
                                self.boxes[id.index()].absolute = absolute;
                                self.boxes[id.index()].centred = control != "textarea";
                                self.boxes[id.index()].first_line = control == "select";
                                if !text.is_empty() {
                                    self.pending(id)?;
                                    self.append(node, id, key, &text, None)?;
                                    self.flush()?;
                                }
                            }
                        }
                        continue;
                    }
                    if let Some(name @ ("input" | "textarea" | "select" | "video" | "audio" | "iframe" | "object" | "embed" | "canvas")) = name {
                        if display == Display::Contents {
                            continue;
                        }
                        let toggle = name == "input" && self.document.attribute(node, "type")?.map(str::trim).is_some_and(|t| t.eq_ignore_ascii_case("checkbox") || t.eq_ignore_ascii_case("radio"));
                        // (Checkboxes and radio buttons are painted, as Chrome's.)
                        if !toggle {
                            self.note(
                                match name {
                                    "input" | "textarea" | "select" => "form control as an empty box",
                                    "video" | "audio" => "media as an empty box",
                                    "iframe" | "object" | "embed" => "embedded document as an empty box",
                                    _ => "canvas as an empty box",
                                },
                                node,
                            );
                        }
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
                        self.replaced(node, display, container, natural, absolute, self.in_line(s, absolute, container))?;
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
                            if !matches!(s.value(Property::VerticalAlign), Value::Keyword(Keyword::Baseline | Keyword::Middle | Keyword::Top | Keyword::Bottom)) {
                                self.note("vertical-align on an atomic inline", node);
                            }
                            self.atomic(node, container, s, false)?;
                        }
                        Display::Box { outer: Outer::Inline, inner, list_item } => {
                            if inner == Inner::Ruby {
                                self.note("ruby as inline", node);
                            }
                            if position != Keyword::Static {
                                self.note("positioned inline", node);
                            }
                            // super, sub and lengths shift the span's baseline in
                            // its paragraph (`baseline_shift`); the others are not
                            // taken into account.
                            if !matches!(s.value(Property::VerticalAlign), Value::Keyword(Keyword::Baseline | Keyword::Super | Keyword::Sub) | Value::Numeric(_)) {
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
                            if list_item {
                                self.marker(node, container, s, list_owner)?;
                            }
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
                            if !list_item && self.in_line(s, absolute, container) {
                                self.atomic(node, container, s, true)?;
                                continue;
                            }
                            self.flush()?;
                            let root = self.boxes[container.index()].kind == BoxKind::Viewport;
                            let id = self.add(node, node, BoxKind::Block { flow_root: inner != Inner::Flow, root, absolute }, container)?;
                            self.push(node, id, node, true)?;
                            if list_item {
                                self.marker(node, id, s, list_owner)?;
                            }
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
            viewport_elements: super::overflow::ViewportElements::new(self.document),
            images: self.images,
            vectors: Arc::from(self.vectors),
            text_bytes,
            document: self.document.id(),
            revision: self.document.revision(),
            styles: self.styles,
            inlines: Arc::from(self.inlines),
            controls: Arc::from(controls),
            boxes: Arc::from(self.boxes),
            sources: Arc::from(self.sources),
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
    fn replaced(&mut self, node: NodeId, display: Display, container: BoxId, natural: [f32; 2], absolute: bool, in_line: bool) -> Result<()> {
        match display {
            Display::Box { outer: Outer::Inline, .. } if !absolute => {
                let toggle = self.document.name(node)? == Some("input")
                    && self.document.attribute(node, "type")?.map(str::trim).is_some_and(|t| t.eq_ignore_ascii_case("checkbox") || t.eq_ignore_ascii_case("radio"));
                self.append_object(node, container, node, BoxKind::InlineReplaced { natural, toggle })
            }
            _ if in_line => self.absolute_object(node, container, BoxKind::BlockReplaced { natural }).map(|_| ()),
            _ => {
                self.flush()?;
                let id = self.add(node, node, BoxKind::BlockReplaced { natural }, container)?;
                self.boxes[id.index()].absolute = absolute;
                Ok(())
            }
        }
    }
}

/// A marker's text for a `list-style-type` and ordinal (CSS Counter Styles 3
/// §6): the bullet or the counter with its suffix, a space after.
/// A counter's value as its style shows it (`counter()`): the list
/// marker's text without its suffix.
fn counter_text(kind: Keyword, value: i64) -> String {
    match kind {
        Keyword::None => String::new(),
        Keyword::Disc | Keyword::Circle | Keyword::Square => marker_text(kind, value).trim_end_matches('\u{a0}').to_owned(),
        _ => marker_text(kind, value).trim_end_matches('\u{a0}').trim_end_matches('.').to_owned(),
    }
}
fn marker_text(kind: Keyword, ordinal: i64) -> String {
    fn alpha(mut n: i64, upper: bool) -> String {
        if n <= 0 {
            return n.to_string();
        }
        let mut out = Vec::new();
        while n > 0 {
            let r = ((n - 1) % 26) as u8;
            out.push((if upper { b'A' } else { b'a' }) + r);
            n = (n - 1) / 26;
        }
        out.reverse();
        String::from_utf8(out).unwrap_or_default()
    }
    fn roman(n: i64, upper: bool) -> String {
        if !(1..=3999).contains(&n) {
            return n.to_string();
        }
        let mut n = n;
        let mut out = String::new();
        for (value, numeral) in [(1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"), (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")] {
            while n >= value {
                out.push_str(numeral);
                n -= value;
            }
        }
        if upper { out.to_ascii_uppercase() } else { out }
    }
    match kind {
        Keyword::Disc => "\u{2022}\u{a0}".into(),
        Keyword::Circle => "\u{25e6}\u{a0}".into(),
        Keyword::Square => "\u{25aa}\u{a0}".into(),
        Keyword::DecimalLeadingZero => format!("{}{ordinal}.\u{a0}", if (0..10).contains(&ordinal) { "0" } else { "" }),
        Keyword::LowerAlpha | Keyword::LowerLatin => format!("{}.\u{a0}", alpha(ordinal, false)),
        Keyword::UpperAlpha | Keyword::UpperLatin => format!("{}.\u{a0}", alpha(ordinal, true)),
        Keyword::LowerRoman => format!("{}.\u{a0}", roman(ordinal, false)),
        Keyword::UpperRoman => format!("{}.\u{a0}", roman(ordinal, true)),
        _ => format!("{ordinal}.\u{a0}"),
    }
}

/// What a text-showing control shows, and whether it is its placeholder:
/// a text field's value (a password's as bullets), a text area's, a
/// select's chosen option, a button input's label; None for the controls
/// drawn as boxes (checkboxes, radios, ranges, colours, files, images).
fn control_text(doc: &Document, node: NodeId, name: &str) -> Result<Option<(String, bool)>> {
    let kind = doc.attribute(node, "type")?.unwrap_or("").trim().to_ascii_lowercase();
    match name {
        "input" => {
            let label = |default: &str| -> Result<Option<(String, bool)>> { Ok(Some((doc.attribute(node, "value")?.map_or_else(|| default.to_owned(), str::to_owned), false))) };
            match kind.as_str() {
                "submit" => return label("Submit"),
                "reset" => return label("Reset"),
                "button" => return label(""),
                "checkbox" | "radio" | "range" | "color" | "file" | "image" | "hidden" => return Ok(None),
                _ => {}
            }
        }
        "select" => {
            // The selected option's text, else the first option's; then the
            // others', a line each (see `BoxNode::first_line`).
            let mut options = Vec::new();
            let mut first = None;
            let mut chosen = None;
            let mut stack: Vec<(NodeId, bool)> = doc.children(node)?.map(|c| (c, false)).collect();
            stack.reverse();
            while let Some((n, grouped)) = stack.pop() {
                match doc.name(n)? {
                    Some("option") => {
                        if first.is_none() {
                            first = Some(n);
                        }
                        if options.len() < 256 {
                            options.push((n, grouped));
                        }
                        if doc.attribute(n, "selected")?.is_some() && chosen.is_none() {
                            chosen = Some(n);
                        }
                    }
                    Some("optgroup") => {
                        let mut kids: Vec<(NodeId, bool)> = doc.children(n)?.map(|c| (c, true)).collect();
                        kids.reverse();
                        stack.extend(kids);
                    }
                    _ => {}
                }
            }
            let Some(option) = chosen.or(first) else { return Ok(Some((String::new(), false))) };
            let label = |option: NodeId| -> Result<String> {
                if let Some(l) = doc.attribute(option, "label")?.filter(|l| !l.trim().is_empty()) {
                    return Ok(l.split_whitespace().collect::<Vec<_>>().join(" "));
                }
                let mut text = String::new();
                let mut stack = vec![option];
                while let Some(n) = stack.pop() {
                    if doc.node_type(n)? == crate::dom::NodeType::Text
                        && let Some(v) = doc.value(n)?
                    {
                        text.push_str(v);
                    }
                    let mut kids: Vec<NodeId> = doc.children(n)?.collect();
                    kids.reverse();
                    stack.extend(kids);
                    if text.len() > 4096 {
                        break;
                    }
                }
                Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
            };
            let mut text = label(option)?;
            // (The chosen one again when in a group: its indented width counts.)
            for &(other, grouped) in options.iter().filter(|(o, grouped)| *o != option || *grouped) {
                if text.len() > 16 * 1024 {
                    break;
                }
                text.push('\n');
                // Chrome's popup indents a group's options: they size it so.
                if grouped {
                    text.push_str("    ");
                }
                text.push_str(&label(other)?);
            }
            return Ok(Some((text, false)));
        }
        _ => {}
    }
    let value = doc.control_value(node)?;
    if value.is_empty() {
        let placeholder = doc.attribute(node, "placeholder")?.unwrap_or("");
        return Ok(Some((placeholder.replace(['\r', '\n'], " "), !placeholder.is_empty())));
    }
    Ok(Some((
        if kind == "password" {
            "\u{2022}".repeat(value.chars().count())
        } else if name == "input" {
            value.replace(['\r', '\n'], " ")
        } else {
            value.into_owned()
        },
        false,
    )))
}

/// Whether `node` is content a closed `<details>` hides: a child of one
/// without `open`, other than its first `<summary>`.
fn closed_details_hides(document: &Document, node: NodeId) -> Result<bool> {
    let Some(parent) = document.parent(node)? else { return Ok(false) };
    if document.node_type(parent)? != NodeType::Element
        || document.name(parent)? != Some("details")
        || document.namespace(parent)? != Some(crate::dom::Namespace::Html)
        || document.attribute(parent, "open")?.is_some()
    {
        return Ok(false);
    }
    let first_summary = document.children(parent)?.find(|&c| document.node_type(c).ok() == Some(NodeType::Element) && document.name(c).ok().flatten() == Some("summary"));
    Ok(first_summary != Some(node))
}
