use super::*;
use crate::{
    css::style::{Keyword as K, Property as P, Style, Value},
    dom::Document,
    layout::{BoxId, BoxKind, Point, Rect},
    text::shaping::Direction,
};
use std::{borrow::Cow, ops::Range, sync::Arc};
const NONE: u32 = u32::MAX;
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub canvas: [u8; 4],
}
impl Default for Options {
    fn default() -> Self {
        Self { canvas: [255; 4] }
    }
}
#[derive(Clone, Debug)]
pub enum Primitive {
    Image {
        owner: BoxId,
        image: u32,
    },
    Vector {
        owner: BoxId,
        vector: u32,
        origin: Point,
    },
    Fill {
        color: [u8; 4],
    },
    Border {
        owner: BoxId,
        colors: [[u8; 4]; 4],
    },
    /// Shadows, backgrounds (colour, gradients) and borders with rounded
    /// corners, painted from the owner's style; `outer` is its border box.
    Decoration {
        owner: BoxId,
        outer: Rect,
    },
    Text {
        owner: BoxId,
        fragment: u32,
        glyphs: Range<u32>,
        origin: Point,
        color: [u8; 4],
    },
}
#[derive(Clone, Debug)]
pub struct Command {
    pub bounds: Rect,
    pub primitive: Primitive,
    pub(super) clip: u32,
    /// The innermost opacity group this command paints inside, or NONE.
    pub(super) group: u32,
    /// The cumulative CSS transform this command paints through (an index
    /// into the scene's transforms), or NONE. `bounds` and the primitive's
    /// geometry are untransformed; `extent` applies it.
    pub(super) transform: u32,
}
/// The axis-aligned bounds of `r` mapped through `m`.
pub(super) fn aabb(m: [f64; 6], r: Rect) -> Rect {
    let corners = [(r.x, r.y), (r.x + r.width, r.y), (r.x, r.y + r.height), (r.x + r.width, r.y + r.height)].map(|(x, y)| crate::css::properties::apply_transform(m, x, y));
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for (x, y) in corners {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 }
}
/// A command's bounds in document space, its transform applied.
pub(super) fn extent(c: &Command, transforms: &[[f64; 6]]) -> Rect {
    if c.transform == NONE { c.bounds } else { aabb(transforms[c.transform as usize], c.bounds) }
}
/// A box painted as a whole at an opacity (CSS Color 4 §2.2): its commands
/// are contiguous in paint order and composite through one layer.
#[derive(Clone, Copy, Debug)]
pub struct Group {
    pub parent: u32,
    pub opacity: f32,
}
/// A clip for descendants of an overflow-clipping box: its padding box,
/// rounded when the box has corner radii.
#[derive(Clone, Debug)]
pub struct Clip {
    pub rect: Rect,
    pub radii: Option<decoration::Radii>,
    /// The transform the clipping box paints through, or NONE.
    pub transform: u32,
}
pub struct Scene {
    pub(super) layout: Arc<layout::Layout>,
    pub(super) commands: Box<[Command]>,
    pub(super) clips: Box<[Clip]>,
    pub(super) groups: Box<[Group]>,
    /// Cumulative affine matrices of transformed boxes, document space to
    /// document space (CSS Transforms 1 §6).
    pub(super) transforms: Box<[[f64; 6]]>,
    pub(super) vector_geometry: std::collections::HashMap<u32, crate::svg::used::Geometry>,
    pub(super) vector_clips: std::collections::HashMap<u32, Vec<crate::svg::clip::Resolved>>,
    pub(super) index: index::Index,
    pub(super) base: [u8; 4],
    pub(super) canvas: [u8; 4],
    extent: Rect,
}
impl Scene {
    pub fn layout(&self) -> &Arc<layout::Layout> {
        &self.layout
    }
    pub fn node(&self, command: u32) -> Option<crate::dom::NodeId> {
        if let Primitive::Image { owner, .. } | Primitive::Vector { owner, .. } | Primitive::Decoration { owner, .. } = self.commands.get(command as usize)?.primitive {
            self.layout.cache().tree().get(owner).map(|b| b.node)
        } else {
            self.text_node(command)
        }
    }
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }
    /// Resolve a text paint segment back to its stable document node without
    /// duplicating full DOM handles in every command.
    pub fn text_node(&self, command: u32) -> Option<crate::dom::NodeId> {
        let Primitive::Text { owner, fragment, glyphs, .. } = &self.commands.get(command as usize)?.primitive else {
            return None;
        };
        let tree = self.layout.cache().tree();
        let BoxKind::InlineContent { source } = tree.get(*owner)?.kind else {
            return None;
        };
        let source_id = source;
        let source = tree.text_source(source)?;
        let text = self.layout.text_layout(*owner)?;
        let fragment = text.fragments().get(*fragment as usize)?;
        let run = text.run(fragment)?;
        let glyph = run.glyphs.get(glyphs.start as usize)?;
        let prepared = text.text().prepared();
        let mapping = prepared.mapping_at_output(glyph.cluster)?;
        let stop = if run.direction == Direction::RightToLeft { run.glyphs.partition_point(|g| g.cluster > glyph.cluster) } else { run.glyphs.partition_point(|g| g.cluster <= glyph.cluster) };
        let end =
            if run.direction == Direction::RightToLeft { stop.checked_sub(1).map_or(run.range.end, |i| run.glyphs[i].cluster) } else { run.glyphs.get(stop).map_or(run.range.end, |g| g.cluster) };
        let last = prepared.mapping_at_output(end.saturating_sub(1))?;
        let input_end = if last.kind == crate::text::prepare::MappingKind::Copy { last.input.start + (end - last.output.start) } else { last.input.end };
        let span = source.spans.get(mapping.span as usize)?;
        if tree.controls(source_id).binary_search_by_key(&span.range.start, |c| c.offset).is_ok() {
            return None;
        }
        (input_end <= span.range.end).then_some(span.node)
    }
    /// Visible text for one paint command, excluding formatting carriers owned
    /// by elements such as wbr. Actual Unicode in DOM text is preserved.
    pub fn text_content(&self, command: u32) -> Option<Cow<'_, str>> {
        let Primitive::Text { owner, fragment, glyphs, .. } = &self.commands.get(command as usize)?.primitive else {
            return None;
        };
        let tree = self.layout.cache().tree();
        let BoxKind::InlineContent { source } = tree.get(*owner)?.kind else {
            return None;
        };
        let text = self.layout.text_layout(*owner)?;
        let f = text.fragments().get(*fragment as usize)?;
        let run = text.run(f)?;
        let gs = run.glyphs.get(glyphs.start as usize..glyphs.end as usize)?;
        let start = gs.iter().map(|g| g.cluster).min()?;
        let last = gs.iter().map(|g| g.cluster).max()?;
        // Glyph clusters are monotone in the run's direction. Do not rescan
        // an entire long paragraph for every small visible paint segment.
        let stop = if run.direction == Direction::RightToLeft { run.glyphs.partition_point(|g| g.cluster > last) } else { run.glyphs.partition_point(|g| g.cluster <= last) };
        let end =
            if run.direction == Direction::RightToLeft { stop.checked_sub(1).map_or(run.range.end, |i| run.glyphs[i].cluster) } else { run.glyphs.get(stop).map_or(run.range.end, |g| g.cluster) }
                .min(f.source.end);
        if start >= end {
            return None;
        }
        let value = run.source.get(start as usize..end as usize)?;
        let controls = tree.controls(source);
        if controls.is_empty() {
            return Some(Cow::Borrowed(value));
        }
        let prepared = text.text().prepared();
        let a = prepared.mapping_at_output(start)?;
        let b = prepared.mapping_at_output(end - 1)?;
        let raw_start = if a.kind == crate::text::prepare::MappingKind::Copy { a.input.start + start - a.output.start } else { a.input.start };
        let raw_end = if b.kind == crate::text::prepare::MappingKind::Copy { b.input.start + end - b.output.start } else { b.input.end };
        let lo = controls.partition_point(|c| c.offset < raw_start);
        let hi = controls.partition_point(|c| c.offset < raw_end);
        if lo == hi {
            return Some(Cow::Borrowed(value));
        }
        let mut result = String::with_capacity(value.len());
        let mut cursor = start;
        for c in &controls[lo..hi] {
            let m = prepared.mapping_at_source(c.offset)?;
            // Semantic carriers are copied intact by text preparation.
            let at = m.output.start + c.offset - m.input.start;
            result.push_str(run.source.get(cursor as usize..at as usize)?);
            cursor = at.checked_add(c.kind.carrier().len() as u32)?;
        }
        result.push_str(run.source.get(cursor as usize..end as usize)?);
        Some(Cow::Owned(result))
    }
    /// Clipped ink bounds for structured text. Empty clips never become actions.
    pub fn visible_bounds(&self, command: u32) -> Option<Rect> {
        let c = self.commands.get(command as usize)?;
        let bounds = if c.clip == NONE { c.bounds } else { intersection(c.bounds, self.clips[c.clip as usize].rect) };
        (bounds.width > 0. && bounds.height > 0.).then_some(bounds)
    }
    pub fn content_extent(&self) -> Rect {
        self.extent
    }
    pub fn device_scale(&self) -> f64 {
        self.layout.device_scale()
    }
    pub fn command_payload_bytes(&self) -> usize {
        self.commands.len() * size_of::<Command>()
            + self.clips.len() * size_of::<Clip>()
            + self.index.payload_bytes()
            + self.vector_geometry.capacity() * (size_of::<(u32, crate::svg::used::Geometry)>() + 1)
            + self.vector_geometry.values().map(|g| g.payload_bytes()).sum::<usize>()
            + self.vector_clips.capacity() * (size_of::<(u32, Vec<crate::svg::clip::Resolved>)>() + 1)
            + self.vector_clips.values().map(|v| crate::svg::clip::resolved_bytes(v)).sum::<usize>()
    }
    pub fn query(&self, region: Rect, out: &mut Vec<u32>, cancel: Option<&AtomicBool>) -> Result<()> {
        if !valid(region) {
            return Err(Error::Invalid);
        }
        self.index.query(&self.commands, &self.transforms, region, out, cancel)?;
        out.retain(|&id| {
            let c = &self.commands[id as usize];
            c.clip == NONE || {
                let clip = &self.clips[c.clip as usize];
                intersects(if clip.transform == NONE { clip.rect } else { aabb(self.transforms[clip.transform as usize], clip.rect) }, region)
            }
        });
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    pub boxes: usize,
    pub glyphs: usize,
    pub commands: usize,
    pub finished: bool,
}
struct State {
    origin: Point,
    context: u32,
    clip: u32,
    group: u32,
    transform: u32,
}
/// One CSS stacking context (CSS 2.2 Appendix E): the root, positioned boxes
/// (z-index: auto ones painted as if they were contexts), opacity groups and
/// isolated boxes. Each paints at one point in its parent's order, decided
/// by its z-index and tree position.
struct Context {
    parent: u32,
    z: i32,
    /// Where the context's own content falls in its parent's order.
    path: Vec<u32>,
}
struct Draft {
    context: u32,
    /// 0: the context root's own background and borders; 2: in-flow content.
    /// Negative child contexts take slot 1 and the rest slot 3 in `path`.
    slot: u8,
    text: bool,
    command: Command,
}
pub struct BuildJob<'a> {
    doc: &'a Document,
    layout: Arc<layout::Layout>,
    engine: &'a mut Engine,
    limits: Limits,
    cancel: Option<&'a AtomicBool>,
    options: Options,
    canvas: [u8; 4],
    canvas_owner: Option<BoxId>,
    extent: Rect,
    states: Vec<State>,
    draft: Vec<Draft>,
    clips: Vec<Clip>,
    groups: Vec<Group>,
    current_group: u32,
    transforms: Vec<[f64; 6]>,
    current_transform: u32,
    vector_geometry: std::collections::HashMap<u32, crate::svg::used::Geometry>,
    vector_clips: std::collections::HashMap<u32, Vec<crate::svg::clip::Resolved>>,
    vector_clip_bytes: usize,
    svg_viewports: Option<std::collections::HashMap<crate::dom::NodeId, [f64; 2]>>,
    next: usize,
    contexts: Vec<Context>,
    progress: Progress,
    fallbacks: layout::Fallbacks,
    failed: bool,
    /// Text decoration metrics per font (see `line_metrics`).
    font_lines: std::collections::HashMap<usize, (f64, f64, f64, f64)>,
}
fn color(s: &Style, p: P) -> Result<[u8; 4]> {
    Ok(s.used_color(p)?.srgb8_clipped())
}
fn visible(s: &Style) -> bool {
    matches!(s.value(P::Visibility), Value::Keyword(K::Visible))
}
fn intersection(a: Rect, b: Rect) -> Rect {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    Rect { x, y, width: ((a.x + a.width).min(b.x + b.width) - x).max(0.), height: ((a.y + a.height).min(b.y + b.height) - y).max(0.) }
}
impl<'a> BuildJob<'a> {
    pub fn new(doc: &'a Document, layout: Arc<layout::Layout>, engine: &'a mut Engine, options: Options, limits: Limits, cancel: Option<&'a AtomicBool>) -> Result<Self> {
        check(cancel)?;
        layout.cache().tree().validate(doc)?;
        if options.canvas[3] != 255 {
            return Err(Error::Invalid);
        }
        if layout.cache().tree().boxes().len() > limits.boxes {
            return Err(Error::Limit);
        }
        let tree = layout.cache().tree();
        let mut canvas = [0; 4];
        let mut owner = None;
        if let Some((root, _)) = tree.iter().find(|(_, b)| matches!(b.kind, BoxKind::Block { root: true, .. })) {
            let c = color(tree.style(root).ok_or(Error::Invalid)?, P::BackgroundColor)?;
            canvas = c;
            owner = Some(root);
            if c[3] == 0
                && let Some((body, _)) = tree.iter().find(|(_, b)| matches!(b.kind, BoxKind::Block { .. }) && doc.name(b.node).ok() == Some(Some("body")))
            {
                canvas = color(tree.style(body).ok_or(Error::Invalid)?, P::BackgroundColor)?;
                owner = Some(body)
            }
        }
        Ok(Self {
            doc,
            layout,
            engine,
            limits,
            cancel,
            options,
            canvas,
            canvas_owner: owner,
            extent: Rect::default(),
            states: Vec::new(),
            draft: Vec::new(),
            clips: Vec::new(),
            groups: Vec::new(),
            current_group: NONE,
            transforms: Vec::new(),
            current_transform: NONE,
            vector_geometry: Default::default(),
            vector_clips: Default::default(),
            vector_clip_bytes: 0,
            svg_viewports: None,
            next: 0,
            contexts: vec![Context { parent: NONE, z: 0, path: Vec::new() }],
            progress: Progress::default(),
            fallbacks: layout::Fallbacks::default(),
            failed: false,
            font_lines: Default::default(),
        })
    }
    pub fn progress(&self) -> Progress {
        self.progress
    }
    /// What this paint substituted (dashed borders solid, stacking in tree order, …).
    pub fn fallbacks(&self) -> &layout::Fallbacks {
        &self.fallbacks
    }
    fn note(&mut self, what: &'static str, id: BoxId) {
        let node = self.layout.cache().tree().get(id).map(|b| b.node);
        self.fallbacks.note(what, node);
    }
    fn emit(&mut self, (context, slot): (u32, u8), text: bool, bounds: Rect, primitive: Primitive, clip: u32) -> Result<()> {
        if !valid(bounds) {
            return Err(Error::Invalid);
        }
        if bounds.width == 0. || bounds.height == 0. {
            return Ok(());
        }
        if self.draft.len() >= self.limits.commands {
            return Err(Error::Limit);
        }
        let command = Command { bounds, primitive, clip, group: self.current_group, transform: self.current_transform };
        let reach = extent(&command, &self.transforms);
        self.extent = union(self.extent, if clip == NONE { reach } else { intersection(reach, self.clip_extent(clip)) });
        self.draft.push(Draft { context, slot, text, command });
        Ok(())
    }
    fn clip_extent(&self, clip: u32) -> Rect {
        let c = &self.clips[clip as usize];
        if c.transform == NONE { c.rect } else { aabb(self.transforms[c.transform as usize], c.rect) }
    }
    /// The box's `transform` about its `transform-origin`, composed with its
    /// parent's: the matrix its subtree paints through.
    fn transform_of(&mut self, s: &Style, outer: Rect, parent: u32) -> Result<u32> {
        let Value::Transforms(t) = s.value(P::Transform) else { return Ok(parent) };
        let local = t.matrix(outer.width, outer.height);
        let (ox, oy) = match s.value(P::TransformOrigin) {
            Value::Positions(p) if !p.0.is_empty() => (decoration::resolve_any(&p.0[0].0, outer.width), decoration::resolve_any(&p.0[0].1, outer.height)),
            _ => (outer.width / 2., outer.height / 2.),
        };
        let (ox, oy) = (outer.x + ox, outer.y + oy);
        let about = crate::css::properties::multiply_transforms([1., 0., 0., 1., ox, oy], crate::css::properties::multiply_transforms(local, [1., 0., 0., 1., -ox, -oy]));
        let m = if parent == NONE { about } else { crate::css::properties::multiply_transforms(self.transforms[parent as usize], about) };
        if !m.iter().all(|v| v.is_finite()) {
            return Err(Error::Invalid);
        }
        if crate::css::properties::is_identity(m) && parent == NONE {
            return Ok(NONE);
        }
        if self.transforms.len() >= self.limits.commands {
            return Err(Error::Limit);
        }
        self.transforms.push(m);
        Ok(self.transforms.len() as u32 - 1)
    }
    #[allow(clippy::too_many_arguments)]
    fn decorate(&mut self, id: BoxId, s: &Style, g: layout::Geometry, outer: Rect, group: (u32, u8), clip: u32, text: bool) -> Result<()> {
        let radii = decoration::radii(s, outer);
        let images = matches!(s.value(P::BackgroundImage), Value::Images(_));
        let shadows = matches!(s.value(P::BoxShadow), Value::Shadows(_));
        if decoration::rounded(&radii) || images || shadows {
            if let Value::Images(list) = s.value(P::BackgroundImage)
                && list.0.iter().flatten().any(|i| matches!(i, crate::css::properties::Image::Url(url) if self.layout.cache().tree().background(url).is_none()))
            {
                self.note("background image not loaded", id);
            }
            if matches!(s.value(P::BackgroundClip), Value::Keyword(K::Text)) {
                self.note("background-clip: text not painted", id);
            }
            if let Value::Shadows(list) = s.value(P::BoxShadow)
                && list.0.iter().any(|s| s.inset)
            {
                self.note("inset box-shadow not painted", id);
            }
            if decoration::rounded(&radii) {
                let colors: Vec<[u8; 4]> = [P::BorderTopColor, P::BorderRightColor, P::BorderBottomColor, P::BorderLeftColor].iter().map(|p| color(s, *p)).collect::<Result<_>>()?;
                let widths = [g.border_width.top, g.border_width.right, g.border_width.bottom, g.border_width.left];
                let visible: Vec<usize> = (0..4).filter(|&i| widths[i] > 0. && colors[i][3] != 0).collect();
                if !visible.is_empty() && !(visible.len() == 4 && visible.iter().all(|&i| colors[i] == colors[0])) {
                    self.note("rounded border with differing sides painted square", id);
                }
            }
            let reach = decoration::shadow_extent(s);
            let bounds = Rect { x: outer.x - reach.left, y: outer.y - reach.top, width: outer.width + reach.left + reach.right, height: outer.height + reach.top + reach.bottom };
            if !valid(bounds) {
                return Err(Error::Invalid);
            }
            return self.emit(group, text, bounds, Primitive::Decoration { owner: id, outer }, clip);
        }
        let bg = color(s, P::BackgroundColor)?;
        if self.canvas_owner != Some(id) && bg[3] != 0 {
            self.emit(group, text, outer, Primitive::Fill { color: bg }, clip)?;
        }
        let widths = [g.border_width.top, g.border_width.right, g.border_width.bottom, g.border_width.left];
        let mut colors = [[0; 4]; 4];
        for i in 0..4 {
            if widths[i] > 0. {
                let p = [P::BorderTopStyle, P::BorderRightStyle, P::BorderBottomStyle, P::BorderLeftStyle][i];
                if !matches!(s.value(p), Value::Keyword(K::Solid)) {
                    self.note("border style painted solid", id);
                }
                colors[i] = color(s, [P::BorderTopColor, P::BorderRightColor, P::BorderBottomColor, P::BorderLeftColor][i])?;
            }
        }
        if widths.iter().any(|w| *w > 0.) {
            self.emit(group, text, outer, Primitive::Border { owner: id, colors }, clip)?;
        }
        Ok(())
    }
    fn inline_replaced(&mut self, id: BoxId, at: Point, group: (u32, u8), clip: u32) -> Result<()> {
        let layout = self.layout.clone();
        let s = layout.cache().tree().style(id).ok_or(Error::Invalid)?;
        let primitive = match layout.cache().tree().get(id).ok_or(Error::Invalid)?.kind {
            BoxKind::InlineImage { image } => {
                if !visible(s) {
                    return Ok(());
                }
                Some(Primitive::Image { owner: id, image })
            }
            BoxKind::InlineVector { vector } => Some(Primitive::Vector { owner: id, vector, origin: Point::default() }),
            BoxKind::InlineReplaced { .. } => None,
            _ => return Err(Error::Invalid),
        };
        if s.value(P::Opacity).scalar() != Some(1.)
            || !matches!(s.value(P::Isolation), Value::Keyword(K::Auto))
            || !matches!(s.value(P::ZIndex), Value::Keyword(K::Auto)) && s.value(P::ZIndex).scalar() != Some(0.)
        {
            self.note("opacity/z-index on an atomic inline ignored", id);
        }
        let g = *layout.geometry(id).ok_or(Error::Invalid)?;
        let outer = Rect { x: at.x + g.border_box.x, y: at.y + g.border_box.y, ..g.border_box };
        if visible(s) {
            self.decorate(id, s, g, outer, group, clip, true)?;
        }
        match primitive {
            Some(Primitive::Vector { vector, .. }) => self.vector(id, vector, outer, group, clip),
            Some(primitive) => self.emit(group, true, outer, primitive, clip),
            None => Ok(()),
        }
    }
    /// Keep the raster transform anchored to the CSS viewport while the spatial
    /// index includes visible ink outside it. No path traversal during repaint.
    fn vector(&mut self, id: BoxId, vector: u32, outer: Rect, group: (u32, u8), clip: u32) -> Result<()> {
        let g = self.layout.geometry(id).ok_or(Error::Invalid)?;
        let origin = Point { x: outer.x + g.border_width.left + g.padding.left, y: outer.y + g.border_width.top + g.padding.top };
        let content = g.content_box();
        let source = self.layout.cache().tree().vector(vector).ok_or(Error::Invalid)?;
        if source.rectangles != 0 && !self.vector_geometry.contains_key(&vector) {
            let viewport = source.view_box.map_or([content.width, content.height], |v| [v[2], v[3]]);
            let used = crate::svg::used::Geometry::resolve(source, viewport, self.limits.path_segments, self.limits.path_bytes.saturating_sub(self.vector_clip_bytes), self.cancel)?;
            self.vector_clip_bytes = self.vector_clip_bytes.checked_add(used.payload_bytes()).ok_or(Error::Limit)?;
            if self.vector_clip_bytes > self.limits.path_bytes {
                return Err(Error::Limit);
            }
            self.vector_geometry.insert(vector, used);
        }
        let ink_bounds = self.vector_geometry.get(&vector).map_or(source.ink_bounds, |g| g.ink_bounds);
        let mut bounds = outer;
        if source.clip != [true, true]
            && let (Some(b), Some([sx, sy, tx, ty])) = (ink_bounds, source.transform(content.width, content.height))
        {
            let mut ink = Rect { x: origin.x + tx + f64::from(b.left()) * sx, y: origin.y + ty + f64::from(b.top()) * sy, width: f64::from(b.width()) * sx, height: f64::from(b.height()) * sy };
            if source.clip[0] {
                ink.x = origin.x;
                ink.width = content.width;
            }
            if source.clip[1] {
                ink.y = origin.y;
                ink.height = content.height;
            }
            if valid(ink) {
                bounds = union(bounds, ink);
            }
        }
        if !source.clip_nodes().is_empty() && !self.vector_clips.contains_key(&vector) {
            if self.svg_viewports.is_none() {
                let tree = self.layout.cache().tree();
                let mut viewports = std::collections::HashMap::new();
                for (id, node) in tree.iter() {
                    check(self.cancel)?;
                    if let BoxKind::BlockVector { vector } | BoxKind::InlineVector { vector } = node.kind {
                        let svg = tree.vector(vector).ok_or(Error::Invalid)?;
                        let content = self.layout.geometry(id).ok_or(Error::Invalid)?.content_box();
                        viewports.insert(svg.node, svg.view_box.map_or([content.width, content.height], |v| [v[2], v[3]]));
                    }
                }
                self.svg_viewports = Some(viewports);
            }
            let clips = source.resolve_clips(self.svg_viewports.as_ref().unwrap(), self.limits.path_segments, self.limits.path_bytes.saturating_sub(self.vector_clip_bytes), self.cancel)?;
            self.vector_clip_bytes = self.vector_clip_bytes.checked_add(crate::svg::clip::resolved_bytes(&clips)).ok_or(Error::Limit)?;
            if self.vector_clip_bytes > self.limits.path_bytes {
                return Err(Error::Limit);
            }
            self.vector_clips.insert(vector, clips);
        }
        self.emit(group, true, bounds, Primitive::Vector { owner: id, vector, origin }, clip)
    }
    fn text<const HAS_IMAGES: bool>(&mut self, id: BoxId, source: u32, at: Point, group: (u32, u8), clip: u32) -> Result<()> {
        // Layout is held outside self's mutable command builder borrow.
        let layout = self.layout.clone();
        let text = layout.text_layout(id).ok_or(Error::Invalid)?;
        let controls = layout.cache().tree().controls(source);
        // Include empty/nested boxes: checking only glyph ownership misses
        // a visible background on an empty padded span or an outer inline.
        for b in layout.cache().tree().inline_boxes(source).ok_or(Error::Invalid)? {
            check(self.cancel)?;
            let s = layout.cache().tree().styles().snapshot(b.node).ok_or(Error::Stale)?;
            if visible(s) && color(s, P::BackgroundColor)?[3] != 0 {
                self.fallbacks.note("inline background not painted", Some(b.node));
                break;
            }
        }
        let source = layout.cache().tree().text_source(source).ok_or(Error::Invalid)?;
        let prepared = text.text().prepared();
        // Styles are immutable for this build. Consecutive glyphs usually
        // belong to the same span: resolve/convert its paint color once.
        let mut paint_color: Option<(u32, [u8; 4], Option<([u8; 4], K)>)> = None;
        let mut image_owners = Vec::new();
        if HAS_IMAGES {
            let mut child = layout.cache().tree().get(id).ok_or(Error::Invalid)?.first_child;
            while let Some(id) = child {
                let b = layout.cache().tree().get(id).ok_or(Error::Invalid)?;
                child = b.next_sibling;
                // Atomic inlines (blocks under a text box) paint themselves.
                if matches!(b.kind, BoxKind::InlineImage { .. } | BoxKind::InlineVector { .. } | BoxKind::InlineReplaced { .. }) {
                    image_owners.push((b.node.slot(), id));
                }
            }
            image_owners.sort_unstable_by_key(|(node, _)| *node);
        }

        for (li, line) in text.lines().iter().enumerate() {
            check(self.cancel)?;
            // Native text has subpixel x placement and a device-pixel baseline.
            // Retain the painted baseline in the scene so its index and agent
            // bounds agree with the actual glyph placement.
            let scale = layout.device_scale();
            let baseline = ((at.y + text.baseline(li).ok_or(Error::Invalid)?) * scale).round() / scale;
            for fi in line.fragments.clone() {
                let f = &text.fragments()[fi as usize];
                if HAS_IMAGES && f.glyphs.is_empty() && !image_owners.is_empty() {
                    let mapping = prepared.mapping_at_output(f.source.start).ok_or(Error::Invalid)?;
                    let node = source.spans.get(mapping.span as usize).ok_or(Error::Invalid)?.node;
                    if let Ok(i) = image_owners.binary_search_by_key(&node.slot(), |(node, _)| *node) {
                        self.inline_replaced(image_owners[i].1, at, group, clip)?;
                    }
                }
                let run = text.run(f).ok_or(Error::Invalid)?;
                if !matches!(run.direction, Direction::LeftToRight | Direction::RightToLeft) {
                    self.note("vertical glyphs not painted", id);
                    continue;
                }
                let factor = run.size / f64::from(run.font.units_per_em());
                // A shifted run (superscript) draws above the line's baseline.
                let baseline = baseline - (text.shift(f) * scale).round() / scale;
                let mut pen = 0i64;
                let mut segment: Option<(u32, Point, [u8; 4], Rect, u32, Option<([u8; 4], K)>)> = None;
                for gi in f.glyphs.clone() {
                    check(self.cancel)?;
                    self.progress.glyphs += 1;
                    if self.progress.glyphs > self.limits.glyph_work {
                        return Err(Error::Limit);
                    }
                    let g = run.glyphs[gi as usize];
                    let mapping = prepared.mapping_at_output(g.cluster).ok_or(Error::Invalid)?;
                    let span = source.spans.get(mapping.span as usize).ok_or(Error::Invalid)?;
                    let (c, deco) = match paint_color {
                        Some((owner, color, deco)) if owner == mapping.span => (color, deco),
                        _ => {
                            let s = layout.cache().tree().styles().snapshot(span.style_node).ok_or(Error::Stale)?;
                            let mut c = color(s, P::Color)?;
                            if !visible(s) {
                                c[3] = 0
                            }
                            // text-decoration-line, in text-decoration-color.
                            let deco = match s.value(P::TextDecorationLine) {
                                Value::Keyword(k @ (K::Underline | K::Overline | K::LineThrough)) if c[3] != 0 => Some((s.used_color(P::TextDecorationColor)?.srgb8_clipped(), *k)),
                                _ => None,
                            };
                            paint_color = Some((mapping.span, c, deco));
                            (c, deco)
                        }
                    };
                    // A cluster shared across different paint styles needs caret clipping.
                    // Never silently color the entire ligature from its first source span.
                    let stop = if run.direction == Direction::RightToLeft { run.glyphs.partition_point(|n| n.cluster > g.cluster) } else { run.glyphs.partition_point(|n| n.cluster <= g.cluster) };
                    let end = if run.direction == Direction::RightToLeft {
                        stop.checked_sub(1).map_or(run.range.end, |i| run.glyphs[i].cluster)
                    } else {
                        run.glyphs.get(stop).map_or(run.range.end, |g| g.cluster)
                    };
                    let project_end = prepared.mapping_at_output(end.saturating_sub(1)).ok_or(Error::Invalid)?;
                    let input_end = if project_end.kind == crate::text::prepare::MappingKind::Copy { project_end.input.start + (end - project_end.output.start) } else { project_end.input.end };
                    let input_start = if mapping.kind == crate::text::prepare::MappingKind::Copy { mapping.input.start + (g.cluster - mapping.output.start) } else { mapping.input.start };
                    let semantic_control = !controls.is_empty() && controls.binary_search_by_key(&span.range.start, |c| c.offset).is_ok();
                    if semantic_control && input_end <= span.range.end {
                        if let Some((start, origin, color, bounds, _, deco)) = segment.take() {
                            let end_x = at.x + f.x + pen as f64 * factor;
                            self.segment(group, clip, id, fi, start..gi, origin, color, bounds, deco, end_x, baseline, &run.font, run.size)?;
                        }
                        pen += i64::from(g.x_advance);
                        continue;
                    }
                    let lo = source.spans.partition_point(|s| s.range.end <= input_start);
                    let hi = source.spans.partition_point(|s| s.range.start < input_end);
                    // A shared cluster spanning DOM nodes needs glyph caret
                    // ownership. Keep it readable, but do not invent a click target.
                    // A ligature across two colours takes its first span's colour.
                    let hit_span = if hi - lo == 1 { mapping.span } else { u32::MAX };
                    let origin = Point { x: at.x + f.x + pen as f64 * factor, y: baseline };
                    let ink = self.engine.ink(&run.font, g.id, self.cancel)?;
                    pen += i64::from(g.x_advance);
                    let bounds = ink.bounds.map(|r| Rect {
                        x: origin.x + (f64::from(g.x_offset) + r.x) * factor,
                        y: baseline - (f64::from(g.y_offset) + r.y + r.height) * factor,
                        width: r.width * factor,
                        height: r.height * factor,
                    });
                    if let Some((start, start_at, old, b, span_index, old_deco)) = segment.take() {
                        if old == c && span_index == hit_span && old_deco == deco && gi - start < 64 {
                            segment = Some((start, start_at, c, bounds.map_or(b, |n| union(b, n)), span_index, deco));
                        } else {
                            self.segment(group, clip, id, fi, start..gi, start_at, old, b, old_deco, origin.x, baseline, &run.font, run.size)?;
                        }
                    }
                    if segment.is_none() && (bounds.is_some() || deco.is_some()) && c[3] != 0 {
                        segment = Some((gi, origin, c, bounds.unwrap_or(Rect { x: origin.x, y: baseline, width: 0., height: 0. }), hit_span, deco));
                    }
                }
                if let Some((start, origin, c, b, _, deco)) = segment {
                    let end_x = at.x + f.x + pen as f64 * factor;
                    self.segment(group, clip, id, fi, start..f.glyphs.end, origin, c, b, deco, end_x, baseline, &run.font, run.size)?;
                }
            }
        }
        Ok(())
    }
    /// One run of glyphs in one colour, with its text decoration under it
    /// (CSS Text Decoration 3 §2): the line from the font's underline
    /// metrics, or a tenth of an em below the baseline and a fourteenth
    /// thick without them; line-through through the middle of the
    /// x-height; overline at the ascent.
    #[allow(clippy::too_many_arguments)]
    fn segment(
        &mut self,
        group: (u32, u8),
        clip: u32,
        id: BoxId,
        fragment: u32,
        glyphs: Range<u32>,
        origin: Point,
        color: [u8; 4],
        bounds: Rect,
        deco: Option<([u8; 4], K)>,
        end_x: f64,
        baseline: f64,
        font: &Arc<crate::text::shaping::GlyphFont>,
        size: f64,
    ) -> Result<()> {
        if let Some((line_color, line)) = deco
            && line_color[3] != 0
            && end_x > origin.x
        {
            let (position, thickness, x_height, ascent) = self.line_metrics(font);
            let factor = size / f64::from(font.units_per_em());
            let scale = self.layout.device_scale();
            let thickness = ((thickness * factor * scale).round() / scale).max(1. / scale);
            let y = match line {
                K::Underline => baseline - position * factor,
                K::LineThrough => baseline - x_height * factor / 2. - thickness / 2.,
                _ => baseline - ascent * factor,
            };
            let y = (y * scale).round() / scale;
            self.emit(group, false, Rect { x: origin.x, y, width: end_x - origin.x, height: thickness }, Primitive::Fill { color: line_color }, clip)?;
        }
        if bounds.width > 0. && bounds.height > 0. {
            self.emit(group, true, bounds, Primitive::Text { owner: id, fragment, glyphs, origin, color }, clip)?;
        }
        Ok(())
    }
    /// A font's underline position (above the baseline, usually negative)
    /// and thickness, x-height and ascent, in font units, kept per font.
    fn line_metrics(&mut self, font: &Arc<crate::text::shaping::GlyphFont>) -> (f64, f64, f64, f64) {
        let key = Arc::as_ptr(font) as usize;
        if let Some(m) = self.font_lines.get(&key) {
            return *m;
        }
        let upem = f64::from(font.units_per_em());
        let m = match font.selection().parsed() {
            Ok(face) => {
                let (position, thickness) = face.underline_metrics().map_or((-upem / 10., upem / 14.), |u| (f64::from(u.position), f64::from(u.thickness).max(1.)));
                let x_height = face.x_height().map_or(upem / 2., f64::from);
                (position, thickness, x_height, f64::from(face.ascender()))
            }
            Err(_) => (-upem / 10., upem / 14., upem / 2., upem * 0.8),
        };
        if self.font_lines.len() < 256 {
            self.font_lines.insert(key, m);
        }
        m
    }
    pub fn step(&mut self, budget: usize) -> Result<Progress> {
        if self.failed {
            return Err(Error::Failed);
        }
        let r = self.advance(budget);
        if r.is_err() {
            self.failed = true
        }
        r
    }
    fn advance(&mut self, budget: usize) -> Result<Progress> {
        check(self.cancel)?;
        for _ in 0..budget {
            check(self.cancel)?;
            let layout = self.layout.clone();
            let tree = layout.cache().tree();
            let Some((id, b)) = tree.at(self.next) else {
                self.progress.finished = true;
                break;
            };
            self.next += 1;
            let (parent, mut context, mut clip, parent_group, parent_transform) = if let Some(parent) = b.parent {
                let p = &self.states[parent.index()];
                (p.origin, p.context, p.clip, p.group, p.transform)
            } else {
                (Point::default(), 0, NONE, NONE, NONE)
            };
            self.current_group = parent_group;
            let mut children_group = parent_group;
            self.current_transform = parent_transform;
            let mut children_transform = parent_transform;
            // In-flow content paints in slot 2 of its context; a box that
            // starts a context paints its own decoration in slot 0 of the new one.
            let mut group = (context, 2);
            if b.absolute {
                let mut cb = b.parent.ok_or(Error::Invalid)?;
                loop {
                    check(self.cancel)?;
                    if cb == tree.root() || !matches!(tree.style(cb).ok_or(Error::Invalid)?.value(P::Position), Value::Keyword(K::Static)) {
                        break;
                    }
                    cb = tree.get(cb).ok_or(Error::Invalid)?.parent.ok_or(Error::Invalid)?;
                }
                clip = self.states[cb.index()].clip;
            }
            let g = *layout.geometry(id).ok_or(Error::Invalid)?;
            let mut outer = g.border_box;
            outer.x += parent.x;
            outer.y += parent.y;
            if !valid(outer) {
                return Err(Error::Invalid);
            }
            let local = g.content_box();
            let origin = Point { x: parent.x + local.x, y: parent.y + local.y };
            let mut children_clip = clip;
            match b.kind {
                BoxKind::Viewport => {}
                BoxKind::Block { .. } | BoxKind::BlockImage { .. } | BoxKind::BlockVector { .. } | BoxKind::BlockReplaced { .. } => {
                    let s = tree.style(id).ok_or(Error::Invalid)?;
                    // A transformed box paints its subtree through its matrix
                    // and is a stacking context (CSS Transforms 1 §3).
                    let transformed = self.transform_of(s, outer, parent_transform)?;
                    if transformed != parent_transform {
                        self.current_transform = transformed;
                        children_transform = transformed;
                    }
                    let opacity = s.value(P::Opacity).scalar().unwrap_or(1.).clamp(0., 1.);
                    if opacity < 1. {
                        if self.groups.len() >= self.limits.commands {
                            return Err(Error::Limit);
                        }
                        let index = self.groups.len() as u32;
                        self.groups.push(Group { parent: parent_group, opacity: opacity as f32 });
                        self.current_group = index;
                        children_group = index;
                    }
                    let positioned = !matches!(s.value(P::Position), Value::Keyword(K::Static));
                    let z = match s.value(P::ZIndex) {
                        Value::Keyword(K::Auto) => None,
                        value => Some(value.scalar().unwrap_or(0.).clamp(i32::MIN as f64, i32::MAX as f64) as i32),
                    };
                    let isolated = !matches!(s.value(P::Isolation), Value::Keyword(K::Auto));
                    // z-index applies to positioned boxes (and flex/grid items; those
                    // are blocks here, so their parent's display decides).
                    let item = tree.get(id).and_then(|b| b.parent).and_then(|p| tree.style(p)).is_some_and(|p| {
                        matches!(p.value(P::Display), Value::Display(crate::css::properties::Display::Box { inner: crate::css::properties::Inner::Flex | crate::css::properties::Inner::Grid, .. }))
                    });
                    let z = if positioned || item { z } else { None };
                    if positioned || z.is_some() || opacity < 1. || isolated || transformed != parent_transform {
                        if self.contexts.len() >= self.limits.commands {
                            return Err(Error::Limit);
                        }
                        let index = self.contexts.len() as u32;
                        self.contexts.push(Context { parent: context, z: z.unwrap_or(0), path: Vec::new() });
                        context = index;
                        group = (context, 0);
                    }
                    if visible(s) {
                        self.decorate(id, s, g, outer, group, clip, false)?;
                        if let BoxKind::BlockImage { image } = b.kind {
                            self.emit(group, true, outer, Primitive::Image { owner: id, image }, clip)?;
                        }
                    }
                    if let BoxKind::BlockVector { vector } = b.kind {
                        self.vector(id, vector, outer, group, clip)?;
                    }
                    let x = !matches!(s.value(P::OverflowX), Value::Keyword(K::Visible));
                    let y = !matches!(s.value(P::OverflowY), Value::Keyword(K::Visible));
                    if x || y {
                        let p = Rect {
                            x: outer.x + g.border_width.left,
                            y: outer.y + g.border_width.top,
                            width: (outer.width - g.border_width.left - g.border_width.right).max(0.),
                            height: (outer.height - g.border_width.top - g.border_width.bottom).max(0.),
                        };
                        let old = if clip == NONE { Rect { x: -1e30, y: -1e30, width: 2e30, height: 2e30 } } else { self.clips[clip as usize].rect };
                        let clip_rect = intersection(
                            old,
                            Rect { x: if x { p.x } else { old.x }, y: if y { p.y } else { old.y }, width: if x { p.width } else { old.width }, height: if y { p.height } else { old.height } },
                        );
                        // Rounded boxes clip to their rounded padding box (one axis
                        // clipped keeps the curve, which is what Chrome does too).
                        let radii = decoration::radii(s, outer);
                        let radii = if x && y && decoration::rounded(&radii) && clip_rect == p { Some(decoration::inset_radii(&radii, g.border_width)) } else { None };
                        children_clip = self.clips.len() as u32;
                        self.clips.push(Clip { rect: clip_rect, radii, transform: children_transform });
                    }
                }
                BoxKind::InlineImage { .. } | BoxKind::InlineVector { .. } | BoxKind::InlineReplaced { .. } => {}
                BoxKind::InlineContent { source } => {
                    let at = Point { x: outer.x, y: outer.y };
                    if layout.text_layout(id).ok_or(Error::Invalid)?.text().object_count() == 0 {
                        self.text::<false>(id, source, at, group, clip)?;
                    } else {
                        self.text::<true>(id, source, at, group, clip)?;
                    }
                }
            }
            self.states.push(State { origin, context, clip: children_clip, group: children_group, transform: children_transform });
            self.progress.boxes += 1;
            self.progress.commands = self.draft.len();
        }
        Ok(self.progress)
    }
    pub fn finish(mut self) -> Result<Arc<Scene>> {
        if self.failed {
            return Err(Error::Failed);
        }
        check(self.cancel)?;
        if !self.progress.finished {
            return Err(Error::Incomplete);
        }
        self.layout.cache().tree().validate(self.doc)?;
        // CSS painting order: each context's own decoration, then its negative
        // children, its in-flow content (block backgrounds before inline
        // content), then its zero and positive children by z-index and tree
        // order. Parents precede children, so one pass builds every path.
        for i in 1..self.contexts.len() {
            check(self.cancel)?;
            let parent = self.contexts[i].parent as usize;
            let z = self.contexts[i].z;
            let mut path = Vec::with_capacity(self.contexts[parent].path.len() + 3);
            path.extend_from_slice(&self.contexts[parent].path);
            path.extend([if z < 0 { 1 } else { 3 }, (z as i64 - i64::from(i32::MIN)) as u32, i as u32]);
            self.contexts[i].path = path;
        }
        let contexts = &self.contexts;
        let key = |d: &Draft| contexts[d.context as usize].path.iter().copied().chain(std::iter::once(u32::from(d.slot)));
        self.draft.sort_by(|a, b| key(a).cmp(key(b)).then(a.text.cmp(&b.text)));
        let commands: Box<[Command]> = self.draft.into_iter().map(|d| d.command).collect();
        let index = index::Index::new(&commands, &self.transforms, self.cancel)?;
        Ok(Arc::new(Scene {
            extent: self.extent,
            layout: self.layout,
            commands,
            clips: self.clips.into_boxed_slice(),
            groups: self.groups.into_boxed_slice(),
            transforms: self.transforms.into_boxed_slice(),
            vector_geometry: self.vector_geometry,
            vector_clips: self.vector_clips,
            index,
            base: self.options.canvas,
            canvas: self.canvas,
        }))
    }
}
