//! Initial path-only inline SVG source. Worker-owned, retained vector geometry.
use super::{Path, PathJob, clip};
use crate::{
    css::style::{Display, Keyword as K, Property as P, StyleTree, Value},
    dom::{Document, Namespace, NodeId, NodeType},
    layout::{Error, Result},
};
use std::sync::atomic::{AtomicBool, Ordering};
pub struct Shape {
    pub node: NodeId,
    pub path: Path,
    pub color: [u8; 4],
    pub evenodd: bool,
    pub clip: u32,
}
pub struct Viewport {
    pub node: NodeId,
    pub shapes: Vec<Shape>,
    clips: Option<triomphe::ThinArc<(), clip::Clip>>,
    pub view_box: Option<[f64; 4]>,
    pub aspect: svgtypes::AspectRatio,
    /// CSS viewport clipping, independently per axis.
    pub clip: [bool; 2],
    /// Conservative control-point bounds, cached once during path compilation.
    pub ink_bounds: Option<tiny_skia::Rect>,
    pub visited: usize,
    pub work: usize,
}
impl Viewport {
    pub fn load(doc: &Document, styles: &StyleTree, root: NodeId, nodes_limit: usize, work_limit: usize, cancel: Option<&AtomicBool>) -> Result<Self> {
        Self::load_with_sources(doc, styles, root, nodes_limit, work_limit, 64 * 1024 * 1024, &mut clip::Sources::default(), cancel)
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn load_with_sources<'a>(
        doc: &'a Document,
        styles: &StyleTree,
        root: NodeId,
        nodes_limit: usize,
        work_limit: usize,
        bytes_limit: usize,
        sources: &mut clip::Sources<'a>,
        cancel: Option<&AtomicBool>,
    ) -> Result<Self> {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(Error::Cancelled);
        }
        styles.get(doc, root, styles.generation())?;
        if doc.namespace(root)? != Some(Namespace::Svg) || doc.name(root)? != Some("svg") {
            return Err(Error::Invalid);
        }
        let root_style = styles.snapshot(root).ok_or(Error::Stale)?;
        let mut clip = [false; 2];
        for (axis, p) in [P::OverflowX, P::OverflowY].into_iter().enumerate() {
            clip[axis] = match root_style.value(p) {
                Value::Keyword(K::Hidden | K::Clip) => true,
                Value::Keyword(K::Visible) => false,
                _ => return Err(Error::Unsupported("scrollable SVG viewport")),
            };
        }
        if [P::Width, P::Height].into_iter().all(|p| matches!(root_style.value(p), Value::Keyword(K::Auto))) {
            return Err(Error::Unsupported("auto-sized SVG viewport"));
        }
        let mut result = Self { node: root, shapes: Vec::new(), clips: None, view_box: None, aspect: Default::default(), clip, ink_bounds: None, visited: 0, work: 0 };
        if let Some(value) = doc.attribute(root, "viewBox")? {
            if value.len() > 1024 * 1024 {
                return Err(Error::Limit);
            }
            let mut values = svgtypes::NumberListParser::from(value);
            let mut numbers = [0.; 4];
            for number in &mut numbers {
                *number = values.next().ok_or(Error::Unsupported("invalid SVG viewBox"))?.map_err(|_| Error::Unsupported("invalid SVG viewBox"))?;
            }
            if values.next().is_some() || numbers.iter().any(|x| !x.is_finite()) || numbers[2] < 0. || numbers[3] < 0. {
                return Err(Error::Unsupported("invalid SVG viewBox"));
            }
            result.view_box = Some(numbers);
        }
        if let Some(value) = doc.attribute(root, "preserveAspectRatio")? {
            if value.len() > 4096 {
                return Err(Error::Limit);
            }
            result.aspect = value.parse().map_err(|_| Error::Unsupported("invalid SVG preserveAspectRatio"))?;
        }
        let mut next = Some(root);
        let mut clips = Vec::new();
        let mut inherited_clips = std::collections::HashMap::new();
        let mut visited = 0;
        let mut bytes = 0usize;
        let mut work = 0usize;
        while let Some(node) = next {
            if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Err(Error::Cancelled);
            }
            visited += 1;
            if visited > 65536.min(nodes_limit) {
                return Err(Error::Limit);
            }
            let mut skip = false;
            if doc.node_type(node)? == NodeType::Element {
                if doc.namespace(node)? != Some(Namespace::Svg) {
                    return Err(Error::Unsupported("SVG foreign content"));
                }
                let s = styles.snapshot(node).ok_or(Error::Stale)?;
                let name = doc.name(node)?.ok_or(Error::Invalid)?;
                // Definitions stay in the DOM/style tree for future references,
                // but never enter the directly painted subtree. Unsupported
                // uses (paint servers, clip-path, use) still reject explicitly.
                skip = matches!(s.value(P::Display), Value::Display(Display::None)) || matches!(name, "defs" | "clipPath" | "title" | "desc" | "metadata");
                if !skip {
                    let mut current_clip = if inherited_clips.is_empty() { clip::NO_CLIP } else { doc.parent(node)?.and_then(|p| inherited_clips.get(&p).copied()).unwrap_or(clip::NO_CLIP) };
                    match s.value(P::ClipPath) {
                        Value::Keyword(K::None) => {}
                        Value::LocalUrl(url) => {
                            if let Some(definition) = sources.get(doc, styles, url.as_str(), work_limit.saturating_sub(work), bytes_limit, cancel)? {
                                if node == root {
                                    return Err(Error::Unsupported("SVG root clip decoration"));
                                }
                                let index = u32::try_from(clips.len()).map_err(|_| Error::Limit)?;
                                clips.push(clip::Clip { parent: current_clip, definition });
                                current_clip = index;
                            }
                        }
                        _ => return Err(Error::Unsupported("SVG basic shape clip")),
                    }
                    if current_clip != clip::NO_CLIP {
                        inherited_clips.insert(node, current_clip);
                        if inherited_clips.capacity().saturating_mul(size_of::<(NodeId, u32)>() + 1) > bytes_limit {
                            return Err(Error::Limit);
                        }
                    }
                    for (attr, diagnostic) in [("transform", "SVG transform"), ("mask", "SVG mask"), ("filter", "SVG filter")] {
                        if doc.attribute(node, attr)?.is_some() {
                            return Err(Error::Unsupported(diagnostic));
                        }
                    }
                    if s.value(P::Opacity).scalar() != Some(1.) {
                        return Err(Error::Unsupported("SVG group opacity"));
                    }
                    match name {
                        "svg" if node == root => {}
                        "g" => {}
                        "path" => {
                            if !matches!(s.value(P::Stroke), Value::Keyword(K::None)) {
                                return Err(Error::Unsupported("SVG strokes"));
                            }
                            let source = doc.attribute(node, "d")?.unwrap_or("");
                            bytes = bytes.checked_add(source.len()).ok_or(Error::Limit)?;
                            if bytes > 16 * 1024 * 1024 {
                                return Err(Error::Limit);
                            }
                            let mut job = PathJob::new(source, super::Limits { segments: 65536.min(work_limit.saturating_sub(work).saturating_sub(sources.work)), ..Default::default() }, cancel)
                                .map_err(path_error)?;
                            let steps = loop {
                                let p = job.step(128).map_err(path_error)?;
                                if p.finished {
                                    break p.segments;
                                }
                            };
                            work = work.checked_add(steps).ok_or(Error::Limit)?;
                            if work > 1_000_000.min(work_limit) || work.saturating_add(sources.work) > work_limit {
                                return Err(Error::Limit);
                            }
                            let path = job.finish().map_err(path_error)?;
                            if path.malformed() {
                                return Err(Error::Unsupported("malformed SVG path"));
                            }
                            if matches!(s.value(P::Visibility), Value::Keyword(K::Visible)) && !matches!(s.value(P::Fill), Value::Keyword(K::None)) {
                                let mut color = s.used_color(P::Fill)?.srgb8_clipped();
                                color[3] = (f64::from(color[3]) * s.value(P::FillOpacity).scalar().ok_or(Error::Invalid)?).round() as u8;
                                if let Some(geometry) = path.geometry() {
                                    let b = geometry.bounds();
                                    result.ink_bounds = Some(result.ink_bounds.map_or(Some(b), |old| old.join(&b)).ok_or(Error::Unsupported("SVG overflow bounds precision"))?);
                                }
                                result.shapes.push(Shape { node, path, color, evenodd: matches!(s.value(P::FillRule), Value::Keyword(K::Evenodd)), clip: current_clip });
                            }
                        }
                        _ => return Err(Error::UnsupportedForeign(Box::new(crate::layout::ForeignElement { node, namespace: Namespace::Svg, name: name.into() }))),
                    }
                }
            } else if doc.node_type(node)? == NodeType::Text && doc.value(node)?.is_some_and(|t| !t.trim().is_empty()) {
                return Err(Error::Unsupported("SVG text"));
            }
            next = if !skip { doc.first_child(node)? } else { None };
            if next.is_none() {
                let mut at = node;
                loop {
                    if at == root {
                        break;
                    }
                    if let Some(sibling) = doc.next_sibling(at)? {
                        next = Some(sibling);
                        break;
                    }
                    at = doc.parent(at)?.ok_or(Error::Invalid)?;
                }
            }
        }
        if !clips.is_empty() {
            result.clips = Some(triomphe::ThinArc::from_header_and_iter((), clips.into_iter()));
        }
        result.visited = visited;
        result.work = work;
        Ok(result)
    }
    pub fn payload_bytes(&self) -> usize {
        self.clips.as_ref().map_or(0, |c| 2 * size_of::<usize>() + size_of_val(&c.slice)) + self.shapes.len() * size_of::<Shape>() + self.shapes.iter().map(|s| s.path.payload_bytes()).sum::<usize>()
    }
    pub(crate) fn clip_nodes(&self) -> &[clip::Clip] {
        self.clips.as_ref().map_or(&[], |c| &c.slice)
    }
    /// Materialize geometry once for this layout, never per raster tile.
    pub(crate) fn resolve_clips(&self, viewports: &std::collections::HashMap<NodeId, [f64; 2]>, segments: usize, bytes: usize, cancel: Option<&AtomicBool>) -> Result<Vec<clip::Resolved>> {
        let mut left = bytes.checked_sub(self.clip_nodes().len() * size_of::<clip::Resolved>()).ok_or(Error::Limit)?;
        let mut work = segments;
        let mut paths = std::collections::HashMap::new();
        let mut out = Vec::with_capacity(self.clip_nodes().len());
        for c in self.clip_nodes() {
            if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Err(Error::Cancelled);
            }
            let key = std::sync::Arc::as_ptr(&c.definition) as usize;
            let path = if let Some(path) = paths.get(&key) {
                std::sync::Arc::clone(path)
            } else {
                // A verb uses at most three f32 points. Bound growth before
                // allocating the path, then charge actual retained payload.
                let viewport = *viewports.get(&c.definition.viewport).ok_or(Error::Unsupported("SVG clip definition viewport"))?;
                let path = c.definition.resolve(viewport, work.min(left / 32), cancel)?;
                let amount = size_of::<Option<tiny_skia::Path>>() + 2 * size_of::<usize>() + path.as_ref().map_or(0, |p| size_of_val(p.verbs()) + size_of_val(p.points()));
                left = left.checked_sub(amount).ok_or(Error::Limit)?;
                work = work.checked_sub(path.as_ref().map_or(0, |p| p.verbs().len())).ok_or(Error::Limit)?;
                let path = std::sync::Arc::new(path);
                paths.insert(key, path.clone());
                path
            };
            out.push(clip::Resolved { parent: c.parent, path });
        }
        Ok(out)
    }
    /// Initial default object size with viewBox ratio. Complete SVG intrinsic
    /// percentage/default sizing remains subject to independent layout checks.
    pub fn natural(&self) -> (f64, f64) {
        match self.view_box {
            Some([_, _, w, h]) if w > 0. && h > 0. => {
                let k = (300. / w).min(150. / h);
                (w * k, h * k)
            }
            _ => (300., 150.),
        }
    }
    /// scale-x, scale-y, translate-x, translate-y in the CSS viewport.
    pub fn transform(&self, width: f64, height: f64) -> Option<[f64; 4]> {
        use svgtypes::Align::*;
        if width <= 0. || height <= 0. {
            return Option::None;
        }
        let Some([x, y, w, h]) = self.view_box else {
            return Some([1., 1., 0., 0.]);
        };
        if w == 0. || h == 0. {
            return Option::None;
        }
        let (mut sx, mut sy) = (width / w, height / h);
        if self.aspect.align != None {
            let k = if self.aspect.slice { sx.max(sy) } else { sx.min(sy) };
            sx = k;
            sy = k;
        }
        let ax = match self.aspect.align {
            XMidYMin | XMidYMid | XMidYMax => 0.5,
            XMaxYMin | XMaxYMid | XMaxYMax => 1.,
            _ => 0.,
        };
        let ay = match self.aspect.align {
            XMinYMid | XMidYMid | XMaxYMid => 0.5,
            XMinYMax | XMidYMax | XMaxYMax => 1.,
            _ => 0.,
        };
        Some([sx, sy, -x * sx + (width - w * sx) * ax, -y * sy + (height - h * sy) * ay])
    }
}
fn path_error(error: super::Error) -> Error {
    match error {
        super::Error::Cancelled => Error::Cancelled,
        super::Error::Limit => Error::Limit,
        _ => Error::Unsupported("SVG path precision"),
    }
}
