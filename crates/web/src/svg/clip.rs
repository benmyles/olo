//! Worker-local SVG clip definitions. Lookup is indexed once per document;
//! computed geometry is retained and materialized once per laid-out viewport.
use crate::{
    css::{
        math,
        style::{Display, Keyword as K, Property as P, StyleTree, Value},
    },
    dom::{Document, Namespace, NodeId, NodeType},
    layout::{Error, Result},
};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub const NO_CLIP: u32 = u32::MAX;
pub struct Clip {
    pub definition: Arc<Definition>,
}
pub struct Definition {
    pub viewport: NodeId,
    rectangles: Vec<super::rect::Rectangle>,
}
/// All coordinates remain in SVG user space. None is a valid empty clip.
pub struct Resolved {
    pub path: Arc<Option<tiny_skia::Path>>,
}
pub fn resolved_bytes(clips: &[Resolved]) -> usize {
    let mut seen = std::collections::HashSet::new();
    size_of_val(clips)
        + clips
            .iter()
            .filter(|c| seen.insert(Arc::as_ptr(&c.path) as usize))
            .map(|c| size_of::<Option<tiny_skia::Path>>() + 2 * size_of::<usize>() + c.path.as_ref().as_ref().map_or(0, |p| size_of_val(p.verbs()) + size_of_val(p.points())))
            .sum::<usize>()
}
#[derive(Default)]
pub struct Sources<'a> {
    document: Option<(u64, u64)>,
    ids: HashMap<&'a str, NodeId>,
    definitions: HashMap<NodeId, Option<Arc<Definition>>>,
    pub work: usize,
    pub bytes: usize,
    failed: bool,
}
fn check(cancel: Option<&AtomicBool>) -> Result<()> {
    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
impl<'a> Sources<'a> {
    fn tick(&mut self, amount: usize, work_limit: usize, cancel: Option<&AtomicBool>) -> Result<()> {
        check(cancel)?;
        self.work = self.work.checked_add(amount).ok_or(Error::Limit)?;
        if self.work > work_limit {
            return Err(Error::Limit);
        }
        Ok(())
    }
    fn index(&mut self, doc: &'a Document, work_limit: usize, bytes_limit: usize, cancel: Option<&AtomicBool>) -> Result<()> {
        if let Some(version) = self.document {
            return if version == (doc.id(), doc.revision()) { Ok(()) } else { Err(Error::Stale) };
        }
        // Rebuild only after success; an errored owner poisons its whole job.
        let mut next = Some(doc.root());
        while let Some(node) = next {
            self.tick(1, work_limit, cancel)?;
            if doc.node_type(node)? == NodeType::Element
                && let Some(id) = doc.attribute(node, "id")?
            {
                self.tick(id.len(), work_limit, cancel)?;
                if !id.is_empty() {
                    self.ids.entry(id).or_insert(node);
                }
                self.bytes = self.ids.capacity() * (size_of::<(&str, NodeId)>() + 1);
                if self.bytes > bytes_limit {
                    return Err(Error::Limit);
                }
            }
            next = doc.first_child(node)?;
            if next.is_none() {
                let mut at = node;
                while at != doc.root() {
                    self.tick(1, work_limit, cancel)?;
                    if let Some(sibling) = doc.next_sibling(at)? {
                        next = Some(sibling);
                        break;
                    }
                    at = doc.parent(at)?.ok_or(Error::Invalid)?;
                }
            }
        }
        self.document = Some((doc.id(), doc.revision()));
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub fn get(&mut self, doc: &'a Document, styles: &StyleTree, url: &str, work_limit: usize, bytes_limit: usize, cancel: Option<&AtomicBool>) -> Result<Option<Arc<Definition>>> {
        if self.failed {
            return Err(Error::Failed);
        }
        let result = self.get_inner(doc, styles, url, work_limit, bytes_limit, cancel);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    #[allow(clippy::too_many_arguments)]
    fn get_inner(&mut self, doc: &'a Document, styles: &StyleTree, url: &str, work_limit: usize, bytes_limit: usize, cancel: Option<&AtomicBool>) -> Result<Option<Arc<Definition>>> {
        self.index(doc, work_limit, bytes_limit, cancel)?;
        self.tick(url.len(), work_limit, cancel)?;
        let fragment = url.strip_prefix('#').ok_or(Error::Invalid)?;
        let decoded;
        let id = if fragment.as_bytes().contains(&b'%') {
            let mut out = Vec::with_capacity(fragment.len());
            let mut i = 0;
            let bytes = fragment.as_bytes();
            while i < bytes.len() {
                if i % 256 == 0 {
                    check(cancel)?;
                }
                if bytes[i] == b'%'
                    && i + 2 < bytes.len()
                    && let (Some(a), Some(b)) = ((bytes[i + 1] as char).to_digit(16), (bytes[i + 2] as char).to_digit(16))
                {
                    out.push((a * 16 + b) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            decoded = String::from_utf8_lossy(&out).into_owned();
            decoded.as_str()
        } else {
            fragment
        };
        let Some(&node) = self.ids.get(id) else {
            return Ok(None);
        };
        if doc.namespace(node)? != Some(Namespace::Svg) || doc.name(node)? != Some("clipPath") {
            return Ok(None);
        }
        if let Some(definition) = self.definitions.get(&node) {
            return Ok(definition.clone());
        }
        // Chromium treats a display:none definition/ancestor as an unavailable
        // resource. This compatibility behavior differs from CSS Masking1 §6.1;
        // the independent pixel corpus pins both direct and ancestor cases.
        let mut ancestor = Some(node);
        let mut viewport = None;
        while let Some(at) = ancestor {
            self.tick(1, work_limit, cancel)?;
            if styles.snapshot(at).is_some_and(|s| matches!(s.value(P::Display), Value::Display(Display::None))) {
                self.cache(node, None, bytes_limit)?;
                return Ok(None);
            }
            if viewport.is_none() && doc.namespace(at)? == Some(Namespace::Svg) && doc.name(at)? == Some("svg") {
                viewport = Some(at);
            }
            ancestor = doc.parent(at)?;
        }
        if doc.attribute(node, "clipPathUnits")?.is_some_and(|v| v != "userSpaceOnUse") {
            return Err(Error::Unsupported("SVG object-bounding-box clip"));
        }
        effects(doc, styles, node)?;
        let mut rectangles = Vec::new();
        let mut child = doc.first_child(node)?;
        while let Some(node) = child {
            self.tick(1, work_limit, cancel)?;
            child = doc.next_sibling(node)?;
            if doc.node_type(node)? != NodeType::Element {
                continue;
            }
            if doc.namespace(node)? != Some(Namespace::Svg) {
                return Err(Error::Unsupported("SVG clip foreign content"));
            }
            let name = doc.name(node)?.ok_or(Error::Invalid)?;
            if matches!(name, "title" | "desc" | "metadata") {
                continue;
            }
            let style = styles.snapshot(node).ok_or(Error::Stale)?;
            if matches!(style.value(P::Display), Value::Display(Display::None)) || !matches!(style.value(P::Visibility), Value::Keyword(K::Visible)) {
                continue;
            }
            effects(doc, styles, node)?;
            if name != "rect" {
                return Err(Error::Unsupported("SVG clip child shape"));
            }
            rectangles.push(super::rect::Rectangle::read(style)?);
            let payload = rectangles.capacity() * size_of::<super::rect::Rectangle>();
            if self.bytes.saturating_add(payload) > bytes_limit {
                return Err(Error::Limit);
            }
        }
        let definition = Arc::new(Definition { viewport: viewport.ok_or(Error::Unsupported("SVG clip without viewport"))?, rectangles });
        self.bytes = self.bytes.checked_add(definition.payload_bytes() + size_of::<Definition>() + 2 * size_of::<usize>()).ok_or(Error::Limit)?;
        if self.bytes > bytes_limit {
            return Err(Error::Limit);
        }
        self.cache(node, Some(definition.clone()), bytes_limit)?;
        Ok(Some(definition))
    }
    fn cache(&mut self, node: NodeId, definition: Option<Arc<Definition>>, bytes_limit: usize) -> Result<()> {
        let capacity = self.definitions.capacity();
        self.definitions.insert(node, definition);
        self.bytes = self.bytes.checked_add((self.definitions.capacity() - capacity) * (size_of::<(NodeId, Option<Arc<Definition>>)>() + 1)).ok_or(Error::Limit)?;
        if self.bytes > bytes_limit {
            return Err(Error::Limit);
        }
        Ok(())
    }
}
fn effects(doc: &Document, styles: &StyleTree, node: NodeId) -> Result<()> {
    if doc.attribute(node, "transform")?.is_some() {
        return Err(Error::Unsupported("SVG clip transform"));
    }
    if !matches!(styles.snapshot(node).ok_or(Error::Stale)?.value(P::ClipPath), Value::Keyword(K::None)) {
        return Err(Error::Unsupported("SVG recursively clipped definition"));
    }
    Ok(())
}
impl Definition {
    pub fn payload_bytes(&self) -> usize {
        self.rectangles.capacity() * size_of::<super::rect::Rectangle>()
    }
    pub fn resolve(&self, viewport: [f64; 2], segments: usize, cancel: Option<&AtomicBool>) -> Result<Option<tiny_skia::Path>> {
        let mut path = tiny_skia::PathBuilder::new();
        let mut scratch = math::Scratch::default();
        for rectangle in &self.rectangles {
            rectangle.append(&mut path, viewport, &mut scratch, segments, cancel)?;
        }
        Ok(path.finish())
    }
}
