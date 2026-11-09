//! Inline SVG source. Worker-owned, retained vector geometry. Elements and
//! effects the engine does not draw yet are skipped and recorded, never fatal:
//! an icon with one unknown filter still shows its paths.
use super::{Path, PathJob, clip};
use crate::{
    css::style::{Display, Keyword as K, Property as P, Style, StyleTree, Value},
    dom::{Document, Namespace, NodeId, NodeType},
    layout::{Error, Fallbacks, Result},
};
use std::sync::atomic::{AtomicBool, Ordering};
pub struct Shape {
    pub node: NodeId,
    pub(crate) geometry: Geometry,
    /// Its colour; a gradient's first stop when it has `gradient`.
    pub color: [u8; 4],
    pub evenodd: bool,
    /// The gradient it is painted with, when its paint names one.
    pub gradient: Option<std::sync::Arc<super::gradient::Gradient>>,
}
pub(crate) enum Geometry {
    Path(Path),
    /// Resolved per layout (percentages against the viewport), then transformed.
    Rectangle {
        slot: u32,
        source: Box<super::rect::Rectangle>,
        transform: Option<tiny_skia::Transform>,
    },
    /// Shaped when the layout is prepared (`Viewport::texts[slot]`).
    Text {
        slot: u32,
    },
}
impl Geometry {
    pub fn path<'a>(&'a self, used: Option<&'a super::used::Geometry>) -> Option<&'a tiny_skia::Path> {
        match self {
            Self::Path(path) => path.geometry(),
            Self::Rectangle { slot, .. } => used.and_then(|u| u.rectangles.get(*slot as usize)).and_then(Option::as_ref),
            Self::Text { slot } => used.and_then(|u| u.texts.get(*slot as usize)).and_then(Option::as_ref),
        }
    }
    fn payload_bytes(&self) -> usize {
        match self {
            Self::Path(path) => path.payload_bytes(),
            Self::Rectangle { .. } => size_of::<super::rect::Rectangle>(),
            Self::Text { .. } => 0,
        }
    }
}
/// No mask on a layer.
pub const NO_MASK: u32 = u32::MAX;
/// A `<mask>`'s content: shapes drawn on a transparent surface whose
/// luminance masks the layer. Its rectangles have slots like the viewport's.
pub struct MaskDef {
    pub shapes: Vec<Shape>,
}
/// Preorder isolated effects over a contiguous range of retained shapes.
/// Bounds are in SVG user space; raster surfaces cover only their tile intersection.
pub struct Layer {
    pub first: u32,
    pub end: u32,
    pub end_layer: u32,
    pub opacity: f32,
    pub clip: u32,
    pub mask: u32,
    pub bounds: Option<tiny_skia::Rect>,
}
pub struct Viewport {
    pub node: NodeId,
    pub shapes: Vec<Shape>,
    pub(crate) rectangles: u32,
    /// Texts to shape, by `Geometry::Text` slot.
    pub(crate) texts: Vec<super::text::Source>,
    pub(crate) masks: Vec<MaskDef>,
    clips: Option<triomphe::ThinArc<(), clip::Clip>>,
    layers: Option<triomphe::ThinArc<(), Layer>>,
    pub view_box: Option<[f64; 4]>,
    pub aspect: svgtypes::AspectRatio,
    /// CSS viewport clipping, independently per axis.
    pub clip: [bool; 2],
    /// Conservative control-point bounds, cached once during path compilation.
    pub ink_bounds: Option<tiny_skia::Rect>,
    pub visited: usize,
    pub work: usize,
    /// A hash of everything drawn (shapes, colours, layers, masks, texts):
    /// two loads of the same SVG fingerprint alike, so a painted tile over
    /// it can be kept across layouts.
    pub fingerprint: u64,
}
impl Viewport {
    fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::hash::DefaultHasher::new();
        let shape = |s: &Shape, h: &mut std::hash::DefaultHasher| {
            s.node.hash(h);
            s.color.hash(h);
            s.evenodd.hash(h);
            match &s.geometry {
                Geometry::Path(p) => {
                    0u8.hash(h);
                    if let Some(g) = p.geometry() {
                        g.verbs().len().hash(h);
                        for pt in g.points() {
                            pt.x.to_bits().hash(h);
                            pt.y.to_bits().hash(h);
                        }
                    }
                }
                Geometry::Rectangle { slot, transform, .. } => {
                    1u8.hash(h);
                    slot.hash(h);
                    if let Some(t) = transform {
                        for v in [t.sx, t.kx, t.ky, t.sy, t.tx, t.ty] {
                            v.to_bits().hash(h);
                        }
                    }
                }
                Geometry::Text { slot } => {
                    2u8.hash(h);
                    slot.hash(h);
                }
            }
        };
        self.shapes.len().hash(&mut h);
        for s in &self.shapes {
            shape(s, &mut h);
        }
        for m in &self.masks {
            m.shapes.len().hash(&mut h);
            for s in &m.shapes {
                shape(s, &mut h);
            }
        }
        for t in &self.texts {
            t.text.hash(&mut h);
            t.x.to_bits().hash(&mut h);
            t.y.to_bits().hash(&mut h);
            (t.anchor as u8).hash(&mut h);
            (t.baseline as u8).hash(&mut h);
        }
        for l in self.layers() {
            l.first.hash(&mut h);
            l.end.hash(&mut h);
            l.opacity.to_bits().hash(&mut h);
            l.clip.hash(&mut h);
            l.mask.hash(&mut h);
        }
        self.clip_nodes().len().hash(&mut h);
        self.view_box.map(|v| v.map(f64::to_bits)).hash(&mut h);
        self.clip.hash(&mut h);
        h.finish()
    }
    pub fn load(doc: &Document, styles: &StyleTree, root: NodeId, nodes_limit: usize, work_limit: usize, cancel: Option<&AtomicBool>) -> Result<Self> {
        Self::load_with_sources(doc, styles, root, nodes_limit, work_limit, 64 * 1024 * 1024, &mut clip::Sources::default(), &mut Fallbacks::default(), cancel)
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
        fallbacks: &mut Fallbacks,
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
            // Scrollable overflow on an SVG viewport clips like hidden.
            clip[axis] = !matches!(root_style.value(p), Value::Keyword(K::Visible));
        }
        let mut result = Self {
            node: root,
            shapes: Vec::new(),
            rectangles: 0,
            texts: Vec::new(),
            masks: Vec::new(),
            clips: None,
            layers: None,
            view_box: None,
            aspect: Default::default(),
            clip,
            ink_bounds: None,
            visited: 0,
            work: 0,
            fingerprint: 0,
        };
        if let Some(value) = doc.attribute(root, "viewBox")? {
            if value.len() > 1024 * 1024 {
                return Err(Error::Limit);
            }
            // An invalid viewBox is ignored, as if absent (SVG 2 §8.8).
            let mut values = svgtypes::NumberListParser::from(value);
            let mut numbers = [0.; 4];
            let mut valid = true;
            for number in &mut numbers {
                match values.next() {
                    Some(Ok(v)) => *number = v,
                    _ => valid = false,
                }
            }
            if valid && values.next().is_none() && numbers.iter().all(|x| x.is_finite()) && numbers[2] >= 0. && numbers[3] >= 0. {
                result.view_box = Some(numbers);
            } else {
                fallbacks.note("invalid SVG viewBox ignored", Some(root));
            }
        }
        if let Some(value) = doc.attribute(root, "preserveAspectRatio")? {
            if value.len() > 4096 {
                return Err(Error::Limit);
            }
            match value.parse() {
                Ok(aspect) => result.aspect = aspect,
                Err(_) => fallbacks.note("invalid SVG preserveAspectRatio ignored", Some(root)),
            }
        }
        let mut next = Some(root);
        let mut clips = Vec::new();
        let mut layers: Vec<Layer> = Vec::new();
        let mut open: Vec<(NodeId, usize)> = Vec::new();
        // Current transformation matrices of open transformed ancestors.
        let mut ctms: Vec<(NodeId, tiny_skia::Transform)> = Vec::new();
        // `use` elements being instantiated: (the use, its target); the
        // target's subtree is walked as if it were the use's child.
        let mut uses: Vec<(NodeId, NodeId)> = Vec::new();
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
                    fallbacks.note("foreign content in SVG skipped", Some(node));
                    skip = true;
                } else {
                    let s = styles.snapshot(node).ok_or(Error::Stale)?;
                    let name = doc.name(node)?.ok_or(Error::Invalid)?;
                    // Definitions stay in the DOM/style tree for references, but
                    // never enter the directly painted subtree.
                    let used_target = uses.last().is_some_and(|&(_, t)| t == node);
                    skip = matches!(s.value(P::Display), Value::Display(Display::None))
                        || (!used_target
                            && matches!(
                                name,
                                "defs" | "clipPath" | "title" | "desc" | "metadata" | "linearGradient" | "radialGradient" | "pattern" | "mask" | "filter" | "marker" | "symbol" | "style" | "script"
                            ));
                    if !skip {
                        let mut own_clip = clip::NO_CLIP;
                        match s.value(P::ClipPath) {
                            Value::Keyword(K::None) => {}
                            Value::LocalUrl(url) => {
                                if let Some(definition) = sources.get(doc, styles, url.as_str(), work_limit.saturating_sub(work), bytes_limit, cancel)? {
                                    if node == root {
                                        fallbacks.note("clip-path on the SVG root ignored", Some(node));
                                    } else {
                                        own_clip = u32::try_from(clips.len()).map_err(|_| Error::Limit)?;
                                        clips.push(clip::Clip { definition });
                                    }
                                } else {
                                    fallbacks.note("SVG clip-path not resolved", Some(node));
                                }
                            }
                            _ => fallbacks.note("SVG basic-shape clip-path ignored", Some(node)),
                        }
                        let opacity = s.value(P::Opacity).scalar().ok_or(Error::Invalid)? as f32;
                        // A mask="url(#id)" attribute: the mask's content, built here.
                        let mut own_mask = NO_MASK;
                        if let Some(value) = doc.attribute(node, "mask")?
                            && node != root
                        {
                            let inner = value.trim().strip_prefix("url(").and_then(|r| r.split(')').next()).unwrap_or("").trim().trim_matches(['"', '\'']);
                            let target = match inner.strip_prefix('#') {
                                Some(id) => sources.lookup(doc, id, work_limit.saturating_sub(work), bytes_limit, cancel)?,
                                None => None,
                            };
                            match target {
                                Some(mask_node) if doc.name(mask_node)? == Some("mask") => {
                                    let parent_ctm = ctms.last().map(|(_, t)| *t);
                                    let shapes = mask_content(
                                        doc,
                                        styles,
                                        sources,
                                        mask_node,
                                        parent_ctm,
                                        &mut result.rectangles,
                                        &mut result.texts,
                                        work_limit.saturating_sub(work),
                                        bytes_limit,
                                        fallbacks,
                                        cancel,
                                    )?;
                                    own_mask = u32::try_from(result.masks.len()).map_err(|_| Error::Limit)?;
                                    result.masks.push(MaskDef { shapes });
                                }
                                _ => fallbacks.note("SVG mask not resolved", Some(node)),
                            }
                        }
                        if (opacity != 1. || own_clip != clip::NO_CLIP || own_mask != NO_MASK) && node != root {
                            if open.len() >= 128 || (layers.len() + 1).saturating_mul(size_of::<Layer>()) > bytes_limit {
                                return Err(Error::Limit);
                            }
                            open.push((node, layers.len()));
                            layers.push(Layer { first: result.shapes.len() as u32, end: 0, end_layer: 0, opacity: (opacity * 255.).round() / 255., clip: own_clip, mask: own_mask, bounds: None });
                        } else if opacity != 1. && node == root {
                            fallbacks.note("opacity on the SVG root painted opaque", Some(node));
                        }
                        if doc.attribute(node, "filter")?.is_some() {
                            fallbacks.note("SVG filter ignored", Some(node));
                        }
                        if let Some(value) = doc.attribute(node, "transform")?
                            && node != root
                        {
                            let parent = ctms.last().map_or(tiny_skia::Transform::identity(), |(_, t)| *t);
                            match value.parse::<svgtypes::Transform>() {
                                Ok(t) => {
                                    let own = tiny_skia::Transform::from_row(t.a as f32, t.b as f32, t.c as f32, t.d as f32, t.e as f32, t.f as f32);
                                    ctms.push((node, own.post_concat(parent)));
                                }
                                Err(_) => fallbacks.note("invalid SVG transform ignored", Some(node)),
                            }
                        }
                        let ctm = ctms.last().map(|(_, t)| *t).filter(|t| !t.is_identity());
                        match name {
                            "svg" if node == root => {}
                            "svg" => fallbacks.note("nested SVG viewport drawn in its parent's space", Some(node)),
                            "g" | "a" | "switch" | "symbol" => {}
                            "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" => {
                                let geometry = match name {
                                    "rect" => {
                                        work = work.checked_add(10).ok_or(Error::Limit)?;
                                        if work.saturating_add(sources.work) > work_limit {
                                            return Err(Error::Limit);
                                        }
                                        match super::rect::Rectangle::read(s) {
                                            Ok(source) => Some(Geometry::Rectangle { slot: result.rectangles, source: Box::new(source), transform: ctm }),
                                            Err(_) => {
                                                fallbacks.note("SVG rectangle geometry not resolved", Some(node));
                                                None
                                            }
                                        }
                                    }
                                    "path" => {
                                        let source = doc.attribute(node, "d")?.unwrap_or("");
                                        bytes = bytes.checked_add(source.len()).ok_or(Error::Limit)?;
                                        if bytes > 16 * 1024 * 1024 {
                                            return Err(Error::Limit);
                                        }
                                        let mut job =
                                            PathJob::new(source, super::Limits { segments: 65536.min(work_limit.saturating_sub(work).saturating_sub(sources.work)), ..Default::default() }, cancel)
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
                                            // SVG renders the valid prefix of malformed path data.
                                            fallbacks.note("malformed SVG path drawn up to its error", Some(node));
                                        }
                                        Some(Geometry::Path(path.transformed(ctm)))
                                    }
                                    _ => {
                                        work = work.checked_add(10).ok_or(Error::Limit)?;
                                        if work.saturating_add(sources.work) > work_limit {
                                            return Err(Error::Limit);
                                        }
                                        match basic_shape(doc, node, name, ctm)? {
                                            Ok(path) => Some(Geometry::Path(path)),
                                            Err(what) => {
                                                fallbacks.note(what, Some(node));
                                                None
                                            }
                                        }
                                    }
                                };
                                if let Some(geometry) = geometry
                                    && matches!(s.value(P::Visibility), Value::Keyword(K::Visible))
                                {
                                    let fill = paint(doc, styles, sources, s, node, P::Fill, P::FillOpacity, "fill", work_limit.saturating_sub(work), bytes_limit, fallbacks, cancel)?;
                                    let stroke = paint(doc, styles, sources, s, node, P::Stroke, P::StrokeOpacity, "stroke", work_limit.saturating_sub(work), bytes_limit, fallbacks, cancel)?;
                                    let stroked = match (&stroke, &geometry) {
                                        (Some(_), Geometry::Path(path)) => stroke_outline(doc, node, path, ctm, fallbacks)?,
                                        (Some(_), Geometry::Rectangle { .. }) => {
                                            fallbacks.note("SVG rectangle stroke not drawn", Some(node));
                                            None
                                        }
                                        _ => None,
                                    };
                                    let evenodd = matches!(s.value(P::FillRule), Value::Keyword(K::Evenodd));
                                    // Gradients (in the viewport's space, by the shape's bounds).
                                    let viewport_size = result.view_box.map_or((100., 100.), |v| (v[2] as f32, v[3] as f32));
                                    let mut gradient_for = |attribute: &str, bounds: Option<tiny_skia::Rect>| -> Result<Option<std::sync::Arc<super::gradient::Gradient>>> {
                                        match paint_reference(doc, node, attribute)? {
                                            Some(reference) => super::gradient::resolve(
                                                doc,
                                                styles,
                                                sources,
                                                reference,
                                                ctm.unwrap_or_default(),
                                                bounds,
                                                viewport_size,
                                                work_limit.saturating_sub(work),
                                                bytes_limit,
                                                cancel,
                                            ),
                                            None => Ok(None),
                                        }
                                    };
                                    // (A rectangle's box, at the viewBox's size for its
                                    // percentages: it is resolved per layout.)
                                    let bounds = match &geometry {
                                        Geometry::Rectangle { source, transform, .. } => rectangle_bounds(source, *transform, viewport_size, cancel)?,
                                        other => other.path(None).map(|p| p.bounds()),
                                    };
                                    let fill_gradient = if fill.is_some() { gradient_for("fill", bounds)? } else { None };
                                    let stroke_gradient = match (&stroke, &stroked) {
                                        (Some(_), Some(outline)) => gradient_for("stroke", outline.geometry().map(|p| p.bounds()))?,
                                        _ => None,
                                    };
                                    for (attribute, painted, gradient) in [("fill", fill.is_some(), &fill_gradient), ("stroke", stroke.is_some() && stroked.is_some(), &stroke_gradient)] {
                                        if painted && gradient.is_none() && paint_reference(doc, node, attribute)?.is_some() {
                                            fallbacks.note("SVG gradient painted as its first stop", Some(node));
                                        }
                                    }
                                    let mut add = |geometry: Geometry, color: [u8; 4], evenodd: bool, gradient: Option<std::sync::Arc<super::gradient::Gradient>>| -> Result<()> {
                                        if matches!(geometry, Geometry::Rectangle { .. }) {
                                            result.rectangles = result.rectangles.checked_add(1).ok_or(Error::Limit)?;
                                        }
                                        if let Some(g) = geometry.path(None) {
                                            let b = g.bounds();
                                            result.ink_bounds = result.ink_bounds.map_or(Some(b), |old| old.join(&b)).or(result.ink_bounds);
                                            if let Some(&(_, i)) = open.last() {
                                                layers[i].bounds = layers[i].bounds.map_or(Some(b), |old| old.join(&b)).or(layers[i].bounds);
                                            }
                                        }
                                        result.shapes.push(Shape { node, geometry, color, evenodd, gradient });
                                        Ok(())
                                    };
                                    if let Some(color) = fill {
                                        add(geometry, color, evenodd, fill_gradient)?;
                                    }
                                    if let (Some(color), Some(outline)) = (stroke, stroked) {
                                        add(Geometry::Path(outline), color, false, stroke_gradient)?;
                                    }
                                }
                            }
                            "use" => {
                                // The referenced element is walked as the use's child, in
                                // the use's space moved by x/y (a symbol's viewBox scaled
                                // to the use's size).
                                let href = doc.attribute(node, "href")?.or(doc.attribute(node, "xlink:href")?);
                                let target = match href.and_then(|h| h.trim().strip_prefix('#')) {
                                    Some(id) => sources.lookup(doc, id, work_limit.saturating_sub(work), bytes_limit, cancel)?,
                                    None => None,
                                };
                                match target {
                                    Some(target) if uses.len() < 16 && !uses.iter().any(|&(_, t)| t == target) && target != node && doc.node_type(target)? == NodeType::Element => {
                                        let x = user_length(doc, node, "x")?.ok().flatten().unwrap_or(0.);
                                        let y = user_length(doc, node, "y")?.ok().flatten().unwrap_or(0.);
                                        let mut own = tiny_skia::Transform::from_translate(x, y);
                                        if doc.name(target)? == Some("symbol")
                                            && let Some(vb) = doc.attribute(target, "viewBox")?
                                        {
                                            let mut values = svgtypes::NumberListParser::from(vb).flatten();
                                            let (vx, vy, vw, vh) = (values.next().unwrap_or(0.), values.next().unwrap_or(0.), values.next().unwrap_or(0.), values.next().unwrap_or(0.));
                                            let w = user_length(doc, node, "width")?.ok().flatten().or(user_length(doc, target, "width")?.ok().flatten());
                                            let h = user_length(doc, node, "height")?.ok().flatten().or(user_length(doc, target, "height")?.ok().flatten());
                                            if let (Some(w), Some(h)) = (w, h)
                                                && vw > 0.
                                                && vh > 0.
                                            {
                                                let scale = (f64::from(w) / vw).min(f64::from(h) / vh) as f32;
                                                let (ox, oy) = ((w - vw as f32 * scale) / 2., (h - vh as f32 * scale) / 2.);
                                                own = tiny_skia::Transform::from_row(scale, 0., 0., scale, ox - vx as f32 * scale, oy - vy as f32 * scale).post_concat(own);
                                            }
                                        }
                                        let parent = ctms.last().map_or(tiny_skia::Transform::identity(), |(_, t)| *t);
                                        ctms.push((target, own.post_concat(parent)));
                                        uses.push((node, target));
                                        next = Some(target);
                                        continue;
                                    }
                                    Some(_) => fallbacks.note("SVG use reference skipped", Some(node)),
                                    None => fallbacks.note("SVG use not resolved", Some(node)),
                                }
                            }
                            "text" => {
                                if matches!(s.value(P::Visibility), Value::Keyword(K::Visible))
                                    && let Some(color) = paint(doc, styles, sources, s, node, P::Fill, P::FillOpacity, "fill", work_limit.saturating_sub(work), bytes_limit, fallbacks, cancel)?
                                    && let Some(source) = text_source(doc, node, ctm, bytes_limit, cancel)?
                                {
                                    if paint_reference(doc, node, "fill")?.is_some() {
                                        fallbacks.note("SVG gradient painted as its first stop", Some(node));
                                    }
                                    let slot = u32::try_from(result.texts.len()).map_err(|_| Error::Limit)?;
                                    result.texts.push(source);
                                    result.shapes.push(Shape { node, geometry: Geometry::Text { slot }, color, evenodd: false, gradient: None });
                                }
                                skip = true;
                            }
                            "tspan" | "textPath" => {
                                fallbacks.note("SVG text span outside text skipped", Some(node));
                                skip = true;
                            }
                            "image" => fallbacks.note("SVG image not drawn", Some(node)),
                            "foreignObject" => {
                                fallbacks.note("SVG foreignObject not drawn", Some(node));
                                skip = true;
                            }
                            _ => fallbacks.note("unknown SVG element skipped", Some(node)),
                        }
                    }
                }
            }
            next = if !skip { doc.first_child(node)? } else { None };
            if next.is_none() {
                let mut at = node;
                loop {
                    if open.last().is_some_and(|&(n, _)| n == at) {
                        let (_, i) = open.pop().ok_or(Error::Invalid)?;
                        layers[i].end = result.shapes.len() as u32;
                        layers[i].end_layer = layers.len() as u32;
                        if let (Some(b), Some(&(_, parent))) = (layers[i].bounds, open.last()) {
                            layers[parent].bounds = layers[parent].bounds.map_or(Some(b), |old| old.join(&b)).or(layers[parent].bounds);
                        }
                    }
                    while ctms.last().is_some_and(|&(n, _)| n == at) {
                        ctms.pop();
                    }
                    if at == root {
                        break;
                    }
                    // A use's target done: continue from the use itself (its
                    // own transform and layer close as any element's).
                    if uses.last().is_some_and(|&(_, t)| t == at) {
                        let (use_node, _) = uses.pop().ok_or(Error::Invalid)?;
                        at = use_node;
                        continue;
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
        if !layers.is_empty() {
            result.layers = Some(triomphe::ThinArc::from_header_and_iter((), layers.into_iter()));
        }
        result.visited = visited;
        result.work = work;
        result.fingerprint = result.fingerprint();
        Ok(result)
    }
    pub fn payload_bytes(&self) -> usize {
        self.layers.as_ref().map_or(0, |l| 2 * size_of::<usize>() + size_of_val(&l.slice))
            + self.clips.as_ref().map_or(0, |c| 2 * size_of::<usize>() + size_of_val(&c.slice))
            + self.shapes.len() * size_of::<Shape>()
            + self.shapes.iter().map(|s| s.geometry.payload_bytes()).sum::<usize>()
    }
    pub(crate) fn layers(&self) -> &[Layer] {
        self.layers.as_ref().map_or(&[], |l| &l.slice)
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
            out.push(clip::Resolved { path });
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
/// A length attribute in user units; "px" is the only unit SVG user space
/// takes as is. Percentages and other units need the viewport (recorded).
fn user_length(doc: &Document, node: NodeId, name: &str) -> Result<std::result::Result<Option<f32>, &'static str>> {
    let Some(value) = doc.attribute(node, name)? else { return Ok(Ok(None)) };
    let value = value.trim();
    if value.len() > 64 {
        return Ok(Err("oversized SVG length ignored"));
    }
    let number = value.strip_suffix("px").unwrap_or(value).trim();
    match number.parse::<f32>() {
        Ok(v) if v.is_finite() => Ok(Ok(Some(v))),
        _ if value.ends_with('%') => Ok(Err("percentage SVG shape length not resolved")),
        _ => Ok(Err("invalid SVG shape length ignored")),
    }
}
/// circle, ellipse, line, polyline and polygon as paths in user space.
fn basic_shape(doc: &Document, node: NodeId, name: &str, ctm: Option<tiny_skia::Transform>) -> Result<std::result::Result<Path, &'static str>> {
    let length = |attribute: &str| -> Result<std::result::Result<f32, &'static str>> { Ok(user_length(doc, node, attribute)?.map(|v| v.unwrap_or(0.))) };
    let path = match name {
        "circle" => {
            let (cx, cy, r) = match (length("cx")?, length("cy")?, length("r")?) {
                (Ok(cx), Ok(cy), Ok(r)) => (cx, cy, r),
                (Err(e), ..) | (_, Err(e), _) | (_, _, Err(e)) => return Ok(Err(e)),
            };
            if r <= 0. {
                return Ok(Err("empty SVG circle"));
            }
            tiny_skia::PathBuilder::from_circle(cx, cy, r)
        }
        "ellipse" => {
            let (cx, cy, rx, ry) = match (length("cx")?, length("cy")?, length("rx")?, length("ry")?) {
                (Ok(cx), Ok(cy), Ok(rx), Ok(ry)) => (cx, cy, rx, ry),
                _ => return Ok(Err("SVG ellipse geometry not resolved")),
            };
            // auto takes the other radius (SVG 2 §10.4).
            let rx = if doc.attribute(node, "rx")?.is_none_or(|v| v.trim() == "auto") { ry } else { rx };
            let ry = if doc.attribute(node, "ry")?.is_none_or(|v| v.trim() == "auto") { rx } else { ry };
            if rx <= 0. || ry <= 0. {
                return Ok(Err("empty SVG ellipse"));
            }
            tiny_skia::Rect::from_xywh(cx - rx, cy - ry, 2. * rx, 2. * ry).and_then(tiny_skia::PathBuilder::from_oval)
        }
        "line" => {
            let (x1, y1, x2, y2) = match (length("x1")?, length("y1")?, length("x2")?, length("y2")?) {
                (Ok(a), Ok(b), Ok(c), Ok(d)) => (a, b, c, d),
                _ => return Ok(Err("SVG line geometry not resolved")),
            };
            let mut b = tiny_skia::PathBuilder::new();
            b.move_to(x1, y1);
            b.line_to(x2, y2);
            b.finish()
        }
        _ => {
            let points = doc.attribute(node, "points")?.unwrap_or("");
            if points.len() > 1024 * 1024 {
                return Err(Error::Limit);
            }
            let mut b = tiny_skia::PathBuilder::new();
            let mut first = true;
            for (i, (x, y)) in svgtypes::PointsParser::from(points).enumerate() {
                if i > 65536 {
                    return Err(Error::Limit);
                }
                let (x, y) = (x as f32, y as f32);
                if !x.is_finite() || !y.is_finite() {
                    return Ok(Err("SVG points not finite"));
                }
                if first {
                    b.move_to(x, y);
                    first = false;
                } else {
                    b.line_to(x, y);
                }
            }
            if name == "polygon" && !first {
                b.close();
            }
            b.finish()
        }
    };
    Ok(Ok(Path::from_geometry(path).transformed(ctm)))
}
/// The outline of a stroke as a fill path, from the stroke presentation
/// attributes (stylesheet stroke widths are recorded as not applied).
fn stroke_outline(doc: &Document, node: NodeId, path: &Path, ctm: Option<tiny_skia::Transform>, fallbacks: &mut Fallbacks) -> Result<Option<Path>> {
    let Some(geometry) = path.geometry() else { return Ok(None) };
    let width = match user_length(doc, node, "stroke-width")? {
        Ok(Some(w)) => w,
        Ok(None) => 1.,
        Err(what) => {
            fallbacks.note(what, Some(node));
            1.
        }
    };
    if width <= 0. {
        return Ok(None);
    }
    let mut stroke = tiny_skia::Stroke { width, ..Default::default() };
    stroke.line_cap = match doc.attribute(node, "stroke-linecap")?.map(str::trim) {
        Some("round") => tiny_skia::LineCap::Round,
        Some("square") => tiny_skia::LineCap::Square,
        _ => tiny_skia::LineCap::Butt,
    };
    stroke.line_join = match doc.attribute(node, "stroke-linejoin")?.map(str::trim) {
        Some("round") => tiny_skia::LineJoin::Round,
        Some("bevel") => tiny_skia::LineJoin::Bevel,
        _ => tiny_skia::LineJoin::Miter,
    };
    if let Some(limit) = doc.attribute(node, "stroke-miterlimit")?.and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite() && *v >= 1.) {
        stroke.miter_limit = limit;
    }
    // Dashes (SVG 2 §13.5.4): an odd list repeated to an even one, a
    // negative length making the stroke solid, as does a list of zeros.
    if let Some(list) = doc.attribute(node, "stroke-dasharray")?.map(str::trim).filter(|v| !v.is_empty() && *v != "none") {
        let lengths: Option<Vec<f32>> = list
            .split(|c: char| c == ',' || c.is_ascii_whitespace())
            .filter(|t| !t.is_empty())
            .take(1024)
            .map(|t| {
                let t = t.trim_end_matches("px");
                // (A percentage is of the viewport's diagonal; taken as
                // user units until that is passed here.)
                t.trim_end_matches('%').parse::<f32>().ok().filter(|v| v.is_finite() && *v >= 0.)
            })
            .collect();
        match lengths {
            Some(mut lengths) if lengths.iter().any(|v| *v > 0.) => {
                if lengths.len() % 2 == 1 {
                    lengths.extend_from_within(..);
                }
                let offset = doc.attribute(node, "stroke-dashoffset")?.and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(0.);
                stroke.dash = tiny_skia::StrokeDash::new(lengths, offset);
            }
            Some(_) => {}
            None => fallbacks.note("SVG stroke-dasharray not understood: drawn solid", Some(node)),
        }
    }
    // The path already carries the CTM; stroke the untransformed outline so a
    // scaled group scales its stroke width too, as SVG does.
    let (source, scale) = match ctm {
        Some(t) => match t.invert() {
            Some(inverse) => (geometry.clone().transform(inverse), ((t.sx * t.sy - t.kx * t.ky).abs()).sqrt().max(1e-3)),
            None => (None, 1.),
        },
        None => (Some(geometry.clone()), 1.),
    };
    let Some(source) = source else { return Ok(None) };
    // (Stroking a path doesn't dash it: it is cut into its dashes first.)
    let source = match &stroke.dash {
        Some(dash) => match source.dash(dash, scale) {
            Some(dashed) => dashed,
            None => return Ok(None),
        },
        None => source,
    };
    let outline = source.stroke(&stroke, scale);
    Ok(outline.map(|o| Path::from_geometry(Some(o)).transformed(ctm)))
}
/// A `<text>` element's content and placement: the text of its descendant
/// text nodes with whitespace collapsed, x/y/dx/dy (first values), its
/// anchor and dominant baseline (attributes, as presentation attributes).
fn text_source(doc: &Document, node: NodeId, ctm: Option<tiny_skia::Transform>, bytes_limit: usize, cancel: Option<&AtomicBool>) -> Result<Option<super::text::Source>> {
    let mut text = String::new();
    let mut next = doc.first_child(node)?;
    let mut depth = 0usize;
    let mut at = node;
    while let Some(n) = next {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(Error::Cancelled);
        }
        if doc.node_type(n)? == NodeType::Text
            && let Some(value) = doc.value(n)?
        {
            text.push_str(value);
            if text.len() > bytes_limit.min(1 << 16) {
                return Err(Error::Limit);
            }
        }
        let descend = doc.node_type(n)? == NodeType::Element && depth < 8;
        next = if descend { doc.first_child(n)? } else { None };
        if next.is_some() {
            depth += 1;
            at = n;
            continue;
        }
        let mut here = n;
        loop {
            if let Some(sibling) = doc.next_sibling(here)? {
                next = Some(sibling);
                break;
            }
            if here == node {
                next = None;
                break;
            }
            here = doc.parent(here)?.ok_or(Error::Invalid)?;
            depth = depth.saturating_sub(1);
            if here == node {
                next = None;
                break;
            }
        }
        let _ = at;
    }
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return Ok(None);
    }
    // x/y may be percentages of the viewport (resolved later); dx/dy add.
    let coordinate = |name: &str| -> Result<(f32, bool)> {
        let Some(value) = doc.attribute(node, name)? else { return Ok((0., false)) };
        let value = value.trim();
        if let Some(pct) = value.strip_suffix('%') {
            return Ok((pct.trim().parse::<f32>().ok().filter(|v| v.is_finite()).unwrap_or(0.), true));
        }
        Ok((user_length(doc, node, name)?.ok().flatten().unwrap_or(0.), false))
    };
    let number = |name: &str| -> Result<f32> { Ok(user_length(doc, node, name)?.ok().flatten().unwrap_or(0.)) };
    let (x, x_percent) = coordinate("x")?;
    let (y, y_percent) = coordinate("y")?;
    let (x, y) = if x_percent || y_percent { (x, y) } else { (x + number("dx")?, y + number("dy")?) };
    if (x_percent || y_percent) && (doc.attribute(node, "dx")?.is_some() || doc.attribute(node, "dy")?.is_some()) {
        // dx/dy with percentage positions: added in user units below.
    }
    let anchor = match doc.attribute(node, "text-anchor")?.map(str::trim) {
        Some("middle") => super::text::Anchor::Middle,
        Some("end") => super::text::Anchor::End,
        _ => super::text::Anchor::Start,
    };
    let baseline = match doc.attribute(node, "dominant-baseline")?.map(str::trim) {
        Some("central") => super::text::Baseline::Central,
        Some("middle") => super::text::Baseline::Middle,
        Some("hanging") => super::text::Baseline::Hanging,
        Some("text-top" | "text-before-edge") => super::text::Baseline::TextTop,
        Some("text-bottom" | "text-after-edge" | "ideographic") => super::text::Baseline::TextBottom,
        _ => super::text::Baseline::Alphabetic,
    };
    let mut source = super::text::Source { node, text: std::sync::Arc::from(collapsed), x, y, x_percent, y_percent, anchor, baseline, transform: ctm };
    if x_percent || y_percent {
        // dx/dy in user units ride along in the transform.
        let (dx, dy) = (number("dx")?, number("dy")?);
        if dx != 0. || dy != 0. {
            let shift = tiny_skia::Transform::from_translate(dx, dy);
            source.transform = Some(ctm.map_or(shift, |c| shift.post_concat(c)));
        }
    }
    Ok(Some(source))
}
/// A `<mask>`'s content as shapes (fills of rectangles, paths, basic shapes
/// and text; groups' transforms applied), in the masked element's user space.
#[allow(clippy::too_many_arguments)]
fn mask_content<'a>(
    doc: &'a Document,
    styles: &StyleTree,
    sources: &mut clip::Sources<'a>,
    mask: NodeId,
    base: Option<tiny_skia::Transform>,
    rectangles: &mut u32,
    texts: &mut Vec<super::text::Source>,
    work_limit: usize,
    bytes_limit: usize,
    fallbacks: &mut Fallbacks,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<Shape>> {
    if doc.attribute(mask, "maskContentUnits")?.is_some_and(|v| v.trim() == "objectBoundingBox") {
        fallbacks.note("SVG mask in bounding-box units drawn in user space", Some(mask));
    }
    let mut shapes = Vec::new();
    let mut ctms: Vec<(NodeId, tiny_skia::Transform)> = Vec::new();
    let mut next = doc.first_child(mask)?;
    let mut visited = 0usize;
    while let Some(node) = next {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(Error::Cancelled);
        }
        visited += 1;
        if visited > 4096 {
            return Err(Error::Limit);
        }
        let mut skip = false;
        if doc.node_type(node)? == NodeType::Element && doc.namespace(node)? == Some(Namespace::Svg) {
            let s = styles.snapshot(node).ok_or(Error::Stale)?;
            let name = doc.name(node)?.ok_or(Error::Invalid)?;
            skip = matches!(s.value(P::Display), Value::Display(Display::None)) || matches!(name, "defs" | "clipPath" | "mask" | "title" | "desc" | "metadata" | "style" | "script");
            if !skip {
                if let Some(value) = doc.attribute(node, "transform")?
                    && let Ok(t) = value.parse::<svgtypes::Transform>()
                {
                    let parent = ctms.last().map_or(base.unwrap_or_default(), |(_, t)| *t);
                    let own = tiny_skia::Transform::from_row(t.a as f32, t.b as f32, t.c as f32, t.d as f32, t.e as f32, t.f as f32);
                    ctms.push((node, own.post_concat(parent)));
                }
                let ctm = ctms.last().map(|(_, t)| *t).or(base).filter(|t| !t.is_identity());
                let visible = matches!(s.value(P::Visibility), Value::Keyword(K::Visible));
                let geometry = match name {
                    "rect" => match super::rect::Rectangle::read(s) {
                        Ok(source) => {
                            let slot = *rectangles;
                            *rectangles = rectangles.checked_add(1).ok_or(Error::Limit)?;
                            Some(Geometry::Rectangle { slot, source: Box::new(source), transform: ctm })
                        }
                        Err(_) => None,
                    },
                    "path" => {
                        let source = doc.attribute(node, "d")?.unwrap_or("");
                        let mut job = PathJob::new(source, super::Limits { segments: 65536.min(work_limit), ..Default::default() }, cancel).map_err(path_error)?;
                        while !job.step(128).map_err(path_error)?.finished {}
                        Some(Geometry::Path(job.finish().map_err(path_error)?.transformed(ctm)))
                    }
                    "circle" | "ellipse" | "line" | "polyline" | "polygon" => match basic_shape(doc, node, name, ctm)? {
                        Ok(path) => Some(Geometry::Path(path)),
                        Err(what) => {
                            fallbacks.note(what, Some(node));
                            None
                        }
                    },
                    "text" => {
                        if visible
                            && let Some(color) = paint(doc, styles, sources, s, node, P::Fill, P::FillOpacity, "fill", work_limit, bytes_limit, fallbacks, cancel)?
                            && let Some(source) = text_source(doc, node, ctm, bytes_limit, cancel)?
                        {
                            if paint_reference(doc, node, "fill")?.is_some() {
                                fallbacks.note("SVG gradient painted as its first stop", Some(node));
                            }
                            let slot = u32::try_from(texts.len()).map_err(|_| Error::Limit)?;
                            texts.push(source);
                            shapes.push(Shape { node, geometry: Geometry::Text { slot }, color, evenodd: false, gradient: None });
                        }
                        skip = true;
                        None
                    }
                    "g" | "a" | "svg" | "switch" => None,
                    _ => {
                        fallbacks.note("SVG element in mask skipped", Some(node));
                        None
                    }
                };
                if let Some(geometry) = geometry
                    && visible
                    && let Some(color) = paint(doc, styles, sources, s, node, P::Fill, P::FillOpacity, "fill", work_limit, bytes_limit, fallbacks, cancel)?
                {
                    if paint_reference(doc, node, "fill")?.is_some() {
                        fallbacks.note("SVG gradient painted as its first stop", Some(node));
                    }
                    let evenodd = matches!(s.value(P::FillRule), Value::Keyword(K::Evenodd));
                    shapes.push(Shape { node, geometry, color, evenodd, gradient: None });
                }
            }
        } else {
            skip = true;
        }
        next = if !skip { doc.first_child(node)? } else { None };
        if next.is_none() {
            let mut at = node;
            loop {
                while ctms.last().is_some_and(|&(n, _)| n == at) {
                    ctms.pop();
                }
                if at == mask {
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
    Ok(shapes)
}
/// The solid colour a fill or stroke paints with: its colour times its
/// opacity, or a gradient's first stop. None paints nothing.
#[allow(clippy::too_many_arguments)]
fn paint<'a>(
    doc: &'a Document,
    styles: &StyleTree,
    sources: &mut clip::Sources<'a>,
    s: &Style,
    node: NodeId,
    property: P,
    opacity: P,
    attribute: &str,
    work_limit: usize,
    bytes_limit: usize,
    fallbacks: &mut Fallbacks,
    cancel: Option<&AtomicBool>,
) -> Result<Option<[u8; 4]>> {
    let alpha = s.value(opacity).scalar().ok_or(Error::Invalid)?.clamp(0., 1.);
    // A paint-server reference the property grammar dropped leaves the
    // inherited value behind (a colour, or a root's `fill="none"`: MDN's
    // browser logos drew nothing); the element's own attribute or inline
    // style still says so.
    let referenced = paint_reference(doc, node, attribute)?;
    let mut color = match s.value(property) {
        Value::Keyword(K::None) if referenced.is_none() => return Ok(None),
        Value::Keyword(K::None) | Value::Color(_) => {
            match referenced {
                Some(reference) => match first_stop(doc, styles, sources, reference, work_limit, bytes_limit, cancel)? {
                    // (Drawn as the gradient where its shape takes one; else
                    // its first stop, said where so.)
                    Some(stop) => stop,
                    None => {
                        fallbacks.note("SVG paint server not resolved", Some(node));
                        return Ok(None);
                    }
                },
                None => s.used_color(property)?.srgb8_clipped(),
            }
        }
        _ => return Ok(None),
    };
    color[3] = (f64::from(color[3]) * alpha).round() as u8;
    Ok((color[3] != 0).then_some(color))
}
/// A rectangle's box (its percentages of `viewport`), under its transform.
fn rectangle_bounds(source: &super::rect::Rectangle, transform: Option<tiny_skia::Transform>, viewport: (f32, f32), cancel: Option<&AtomicBool>) -> Result<Option<tiny_skia::Rect>> {
    let mut builder = tiny_skia::PathBuilder::new();
    let mut scratch = crate::css::math::Scratch::default();
    source.append(&mut builder, [f64::from(viewport.0), f64::from(viewport.1)], &mut scratch, 64, cancel)?;
    let path = match transform {
        Some(t) => builder.finish().and_then(|p| p.transform(t)),
        None => builder.finish(),
    };
    Ok(path.map(|p| p.bounds()))
}
/// The paint server (`url(#…)`) an element's own `fill` or `stroke` names,
/// in its inline style or its attribute (the property grammar drops it).
fn paint_reference<'a>(doc: &'a Document, node: NodeId, attribute: &str) -> Result<Option<&'a str>> {
    let style_reference = doc
        .attribute(node, "style")?
        .and_then(|style| style.split(';').filter_map(|d| d.split_once(':')).find(|(k, _)| k.trim().eq_ignore_ascii_case(attribute)).map(|(_, v)| v.trim()).filter(|v| v.starts_with("url(")));
    Ok(style_reference.or(doc.attribute(node, attribute)?.map(str::trim).filter(|v| v.starts_with("url("))))
}
/// The first stop colour of a referenced gradient, with its stop-opacity.
fn first_stop<'a>(
    doc: &'a Document,
    styles: &StyleTree,
    sources: &mut clip::Sources<'a>,
    reference: &str,
    work_limit: usize,
    bytes_limit: usize,
    cancel: Option<&AtomicBool>,
) -> Result<Option<[u8; 4]>> {
    let inner = reference.strip_prefix("url(").and_then(|r| r.split(')').next()).unwrap_or("").trim().trim_matches(['"', '\'']);
    let Some(id) = inner.strip_prefix('#') else { return Ok(None) };
    let mut target = sources.lookup(doc, id, work_limit, bytes_limit, cancel)?;
    // Gradients may inherit their stops from another gradient through href.
    for _ in 0..8 {
        let Some(gradient) = target else { return Ok(None) };
        if !matches!(doc.name(gradient)?, Some("linearGradient" | "radialGradient")) {
            return Ok(None);
        }
        let mut child = doc.first_child(gradient)?;
        while let Some(stop) = child {
            if doc.node_type(stop)? == NodeType::Element && doc.name(stop)? == Some("stop") {
                let s = styles.snapshot(stop).ok_or(Error::Stale)?;
                let mut color = s.used_color(P::StopColor)?.srgb8_clipped();
                color[3] = (f64::from(color[3]) * s.value(P::StopOpacity).scalar().unwrap_or(1.).clamp(0., 1.)).round() as u8;
                return Ok(Some(color));
            }
            child = doc.next_sibling(stop)?;
        }
        let href = doc.attribute(gradient, "href")?.or(doc.attribute(gradient, "xlink:href")?);
        target = match href.and_then(|h| h.trim().strip_prefix('#')) {
            Some(id) => sources.lookup(doc, id, work_limit, bytes_limit, cancel)?,
            None => return Ok(None),
        };
    }
    Ok(None)
}
