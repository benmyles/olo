//! Initial path-only inline SVG source. Worker-owned, retained vector geometry.
use super::{Path, PathJob};
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
}
pub struct Viewport {
    pub node: NodeId,
    pub shapes: Vec<Shape>,
    pub view_box: Option<[f64; 4]>,
    pub aspect: svgtypes::AspectRatio,
    pub visited: usize,
    pub work: usize,
}
impl Viewport {
    pub fn load(doc: &Document, styles: &StyleTree, root: NodeId, nodes_limit: usize, work_limit: usize, cancel: Option<&AtomicBool>) -> Result<Self> {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(Error::Cancelled);
        }
        styles.get(doc, root, styles.generation())?;
        if doc.namespace(root)? != Some(Namespace::Svg) || doc.name(root)? != Some("svg") {
            return Err(Error::Invalid);
        }
        let root_style = styles.snapshot(root).ok_or(Error::Stale)?;
        if [P::OverflowX, P::OverflowY].into_iter().any(|p| !matches!(root_style.value(p), Value::Keyword(K::Hidden | K::Clip))) {
            return Err(Error::Unsupported("unclipped SVG viewport"));
        }
        if [P::Width, P::Height].into_iter().all(|p| matches!(root_style.value(p), Value::Keyword(K::Auto))) {
            return Err(Error::Unsupported("auto-sized SVG viewport"));
        }
        let mut result = Self { node: root, shapes: Vec::new(), view_box: None, aspect: Default::default(), visited: 0, work: 0 };
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
                skip = matches!(s.value(P::Display), Value::Display(Display::None)) || matches!(name, "title" | "desc" | "metadata");
                if !skip {
                    for attr in ["transform", "clip-path", "mask", "filter"] {
                        if doc.attribute(node, attr)?.is_some() {
                            return Err(Error::Unsupported("SVG transform/clip/mask/filter"));
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
                            let mut job = PathJob::new(source, super::Limits { segments: 65536.min(work_limit.saturating_sub(work)), ..Default::default() }, cancel).map_err(path_error)?;
                            let steps = loop {
                                let p = job.step(128).map_err(path_error)?;
                                if p.finished {
                                    break p.segments;
                                }
                            };
                            work = work.checked_add(steps).ok_or(Error::Limit)?;
                            if work > 1_000_000.min(work_limit) {
                                return Err(Error::Limit);
                            }
                            let path = job.finish().map_err(path_error)?;
                            if path.malformed() {
                                return Err(Error::Unsupported("malformed SVG path"));
                            }
                            if matches!(s.value(P::Visibility), Value::Keyword(K::Visible)) && !matches!(s.value(P::Fill), Value::Keyword(K::None)) {
                                let mut color = s.used_color(P::Fill)?.srgb8_clipped();
                                color[3] = (f64::from(color[3]) * s.value(P::FillOpacity).scalar().ok_or(Error::Invalid)?).round() as u8;
                                result.shapes.push(Shape { node, path, color, evenodd: matches!(s.value(P::FillRule), Value::Keyword(K::Evenodd)) });
                            }
                        }
                        _ => return Err(Error::Unsupported("SVG shape/container type")),
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
        result.visited = visited;
        result.work = work;
        Ok(result)
    }
    pub fn payload_bytes(&self) -> usize {
        self.shapes.len() * size_of::<Shape>() + self.shapes.iter().map(|s| s.path.payload_bytes()).sum::<usize>()
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
