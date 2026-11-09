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
    /// The person prefers dark: what a `color-scheme` allowing both uses.
    pub dark: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self { canvas: [255; 4], dark: false }
    }
}
/// What a form control drawn by the engine (`appearance: auto`) is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ControlKind {
    Checkbox {
        checked: bool,
    },
    Radio {
        checked: bool,
    },
    /// A select's arrow: a chevron filling its bounds.
    Arrow,
    /// An unstyled select's frame (its background and rounded border, in
    /// place of its CSS ones) in its bounds.
    Menulist,
    /// An unstyled text field's or button's frame: its background in a
    /// thin rounded border (Chrome's), in place of its CSS bevel.
    Field,
    Button,
}
/// A form control drawn as Chrome draws it: its kind and colours (its
/// background, border, accent, and the mark on it: a check, an arrow).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ControlLook {
    pub kind: ControlKind,
    pub fill: [u8; 4],
    pub border: [u8; 4],
    pub accent: [u8; 4],
    pub mark: [u8; 4],
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
        /// The box it is part of (a background, a text's line), for hit tests.
        owner: BoxId,
        /// Its corners' radii (top left, top right, bottom right, bottom
        /// left; CSS px): an inline box's rounded background.
        radii: [f32; 4],
    },
    Border {
        owner: BoxId,
        colors: [[u8; 4]; 4],
        /// Each side's style (top, right, bottom, left).
        styles: [super::border::Style; 4],
    },
    /// A wavy text decoration across its bounds: its line `thickness`
    /// thick, `wavelength` long a wave (CSS px), about the bounds' middle.
    Wave {
        owner: BoxId,
        color: [u8; 4],
        thickness: f32,
        wavelength: f32,
    },
    /// Shadows, backgrounds (colour, gradients) and borders with rounded
    /// corners, painted from the owner's style; `outer` is its border box.
    Decoration {
        owner: BoxId,
        outer: Rect,
    },
    /// A form control the engine draws (`appearance: auto`) in its bounds.
    Control {
        owner: BoxId,
        look: ControlLook,
    },
    Text {
        owner: BoxId,
        fragment: u32,
        glyphs: Range<u32>,
        origin: Point,
        color: [u8; 4],
        /// Under a box with `background-clip: text`: its backgrounds
        /// paint through these glyphs (an index into the scene's
        /// `text_fills`), or NONE.
        fill: u32,
    },
    /// One glyph of the font of a text's fragment's run, at its size, not
    /// among the text's own (`text-overflow: ellipsis`'s "…").
    Glyph {
        owner: BoxId,
        fragment: u32,
        glyph: u16,
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
    /// Under a `position: fixed` box: anchored to the viewport, painted
    /// over the scrolled page (its bounds are viewport coordinates, which
    /// are the document's at the initial scroll).
    pub(super) fixed: bool,
    /// Under a `position: sticky` box (that box): painted apart, moved by
    /// the scroll (see `layout::Sticky`).
    pub(super) sticky: Option<BoxId>,
    /// A live surface's image (see `image::Image::live`): shown through a
    /// layer of its own when nothing bends, fades or cuts it.
    pub(super) surface: bool,
    /// Under an animated, transformed box (that box): painted apart in the
    /// box's own space, shown through its matrix (see `Moved`).
    pub(super) moved: Option<BoxId>,
    /// How far past its bounds it may show (CSS px): a `filter: blur()`
    /// of a group it is in spreads it (what finds it by area looks that
    /// much further).
    pub(super) reach: f32,
    /// Under a box with a `backdrop-filter` on the page (that box): painted
    /// in a layer of its own, over what is under it blurred.
    pub(super) backdrop: Option<BoxId>,
}
/// A text's decoration as its style says: line, colour, style, and its
/// thickness and an underline's offset from the baseline (CSS px) when
/// given (else the font's).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Deco {
    line: K,
    color: [u8; 4],
    style: K,
    thickness: Option<f64>,
    offset: Option<f64>,
}
impl Deco {
    fn of(s: &Style, line: K, color: [u8; 4]) -> Self {
        let font_size = s.value(P::FontSize).scalar().unwrap_or(16.);
        // A length (px when computed), or a percentage of the font size.
        let length = |p: P| match s.value(p) {
            Value::Numeric(n) => n.expression().constant().map(|q| if q.ty.kind() == Some(crate::css::math::Dimension::Percentage) { q.value / 100. * font_size } else { q.value }),
            _ => None,
        };
        let style = match s.value(P::TextDecorationStyle) {
            Value::Keyword(k) => *k,
            _ => K::Solid,
        };
        Deco { line, color, style, thickness: length(P::TextDecorationThickness).map(|t| t.max(0.)), offset: length(P::TextUnderlineOffset) }
    }
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
    extent_of(c, transforms)
}
fn extent_of(c: &Command, transforms: &[[f64; 6]]) -> Rect {
    let r = if c.transform == NONE { c.bounds } else { aabb(transforms[c.transform as usize], c.bounds) };
    if c.reach > 0. {
        let m = f64::from(c.reach);
        Rect { x: r.x - m, y: r.y - m, width: r.width + 2. * m, height: r.height + 2. * m }
    } else {
        r
    }
}
/// A box painted as a whole at an opacity (CSS Color 4 §2.2): its commands
/// are contiguous in paint order and composite through one layer.
#[derive(Clone, Copy, Debug)]
pub struct Group {
    pub parent: u32,
    pub opacity: f32,
    /// A `mask-image` the group composites through (CSS Masking 1 §7).
    pub mask: Option<MaskRef>,
    /// Its `filter`'s colour functions as one matrix (see
    /// `decoration::color_matrix`), applied to what the group painted
    /// before it composites.
    pub filter: Option<[f32; 20]>,
    /// Its `filter`'s blur (the standard deviation, CSS px; several
    /// compose as one), applied before the matrix.
    pub blur: f32,
    /// How far its content and its enclosing groups' blurs spread it
    /// (CSS px): its commands' reach.
    pub reach: f32,
    /// Its `mix-blend-mode`, as it composites onto what is under it (0
    /// normal; see `raster::blend_mode`).
    pub blend: u8,
    /// Its `filter`'s `drop-shadow()`: the group's alpha in a colour, moved
    /// and blurred, under it.
    pub shadow: Option<DropShadow>,
    /// The colour functions after its `drop-shadow()`, applied to the group
    /// and its shadow (`filter` holds those before).
    pub after: Option<[f32; 20]>,
    /// The clip it paints under (its ancestors' overflow and clip-path),
    /// or NONE. A blur or shadow spreads what the group paints before
    /// that clip cuts it (CSS Filter Effects 1 §3: the filter, then the
    /// ancestors' clipping), so the raster redraws a filtered group's
    /// content without it and clips the result.
    pub clip: u32,
}
/// The hasher of tile signatures: foldhash's quality one, fixed (a
/// signature is compared within the process), a fraction of SipHash's cost.
pub type SignatureHasher = foldhash::quality::FoldHasher<'static>;
pub fn signature_hasher() -> SignatureHasher {
    use std::hash::BuildHasher;
    foldhash::quality::FixedState::with_seed(0x6f6c_6f5f_7469_6c65).build_hasher()
}
/// A `drop-shadow()`: its offset and blur (CSS px; the blur a standard
/// deviation, as Chrome takes it) and its colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropShadow {
    pub dx: f32,
    pub dy: f32,
    pub sigma: f32,
    pub color: [u8; 4],
}
/// The box whose mask images, over its border box, mask a group.
#[derive(Clone, Copy, Debug)]
pub struct MaskRef {
    pub owner: BoxId,
    pub outer: Rect,
    pub transform: u32,
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
/// Where a surface shows (document coordinates): its image's whole
/// rectangle, the rectangle it is cut to, and the circular radii it is
/// rounded by (top left, top right, bottom right, bottom left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceShown {
    pub bounds: Rect,
    pub clip: Option<Rect>,
    pub radii: Option<[f64; 4]>,
    /// The opacity it shows at: its opacity groups' (each holding nothing
    /// but it), multiplied.
    pub opacity: f32,
}
/// How a surface at `b` shows within a clip `k` rounded by `r`: unrounded
/// (Some(None)) where it keeps away from the corners, rounded by circular
/// radii (Some(Some)) where the clip is its own rectangle, and None where
/// neither (it is painted with the page then).
/// A surface's clip in document coordinates: its rectangle, moved by its
/// transform when that only translates; none under any other.
fn surface_clip(transforms: &[[f64; 6]], clip: &Clip) -> Option<Rect> {
    if clip.transform == NONE {
        return Some(clip.rect);
    }
    let m = transforms[clip.transform as usize];
    (m[0] == 1. && m[1] == 0. && m[2] == 0. && m[3] == 1.).then(|| Rect { x: clip.rect.x + m[4], y: clip.rect.y + m[5], ..clip.rect })
}
fn surface_corners(b: Rect, k: Rect, r: &decoration::Radii) -> Option<Option<[f64; 4]>> {
    let inside = b.x >= k.x && b.y >= k.y && b.x + b.width <= k.x + k.width && b.y + b.height <= k.y + k.height;
    let (left, right) = (r[0][0].max(r[3][0]), r[1][0].max(r[2][0]));
    let (top, bottom) = (r[0][1].max(r[1][1]), r[2][1].max(r[3][1]));
    if inside && ((b.x >= k.x + left && b.x + b.width <= k.x + k.width - right) || (b.y >= k.y + top && b.y + b.height <= k.y + k.height - bottom)) {
        return Some(None);
    }
    let same = |a: f64, b: f64| (a - b).abs() < 0.01;
    let own = same(b.x, k.x) && same(b.y, k.y) && same(b.width, k.width) && same(b.height, k.height);
    (own && r.iter().all(|c| same(c[0], c[1]))).then(|| Some([r[0][0], r[1][0], r[2][0], r[3][0]]))
}
/// What a stratum holds: a run of the page's own commands, the fixed
/// layer, or a sticky box's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerKind {
    Page,
    Fixed,
    Sticky(BoxId),
    /// A live surface (a canvas scripts keep drawing on), shown with the
    /// page from its image as it is, not through tiles: the box's.
    Surface(BoxId),
    /// An animated box with a `transform` (a marquee, a carousel, a
    /// spinner): its subtree painted in its own space and shown through
    /// the matrix, so a frame that only moves, turns or scales it repaints
    /// nothing.
    Moved(BoxId),
    /// A box with a `backdrop-filter` scrolled with the page, and its
    /// subtree: a layer of the page's of its own, so what is painted under
    /// it can be blurred (on the GPU) before it (the box's).
    Backdrop(BoxId),
}
/// A moved layer (see `LayerKind::Moved`): its box, its matrix now (its
/// transform about its origin, CSS px: a translation for a marquee, a
/// rotation for a spinner), the clip of its ancestors it is shown within
/// (document coordinates), and its border box (what `transform-origin`
/// resolves against).
#[derive(Clone, Copy, Debug)]
pub struct Moved {
    pub root: BoxId,
    pub matrix: [f64; 6],
    pub clip: Option<Rect>,
    outer: Rect,
}
impl Moved {
    /// Its translation, when that is all its matrix does.
    pub fn translation(&self) -> Option<[f64; 2]> {
        is_translation(self.matrix).then_some([self.matrix[4], self.matrix[5]])
    }
    /// A point of its own space where the layer shows it.
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let m = self.matrix;
        (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
    }
    /// The point of its own space shown at (`x`, `y`).
    pub fn unapply(&self, x: f64, y: f64) -> (f64, f64) {
        let m = self.matrix;
        let det = m[0] * m[3] - m[1] * m[2];
        let (x, y) = (x - m[4], y - m[5]);
        ((m[3] * x - m[2] * y) / det, (m[0] * y - m[1] * x) / det)
    }
    /// The rectangle of its own space that `shown` (where it shows) holds.
    pub fn unapply_rect(&self, shown: Rect) -> Rect {
        let corners = [(shown.x, shown.y), (shown.x + shown.width, shown.y), (shown.x, shown.y + shown.height), (shown.x + shown.width, shown.y + shown.height)].map(|(x, y)| self.unapply(x, y));
        let (x0, x1) = corners.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), c| (a.min(c.0), b.max(c.0)));
        let (y0, y1) = corners.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), c| (a.min(c.1), b.max(c.1)));
        Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 }
    }
}
/// A matrix a layer can be shown through: finite and invertible, not
/// flattened to a line.
fn invertible(m: [f64; 6]) -> bool {
    m.iter().all(|v| v.is_finite()) && (m[0] * m[3] - m[1] * m[2]).abs() > 1e-6
}
/// A scroll container (`overflow` other than `visible` and `clip`): its
/// box and node, its padding box as painted (document coordinates, or the
/// viewport's under a fixed box), its offset and the size of what it
/// holds from its padding box's corner, and which axes a person can
/// scroll (`auto` and `scroll`; `hidden` scrolls only by script).
#[derive(Clone, Copy, Debug)]
pub struct Scroller {
    pub root: BoxId,
    pub node: crate::dom::NodeId,
    pub rect: Rect,
    pub fixed: bool,
    pub sticky: Option<BoxId>,
    pub offset: [f64; 2],
    pub content: [f64; 2],
    pub user: [bool; 2],
    /// Its end padding (right, bottom), which what it holds ends with.
    pad: [f64; 2],
}
impl Scroller {
    /// The farthest it scrolls, each axis.
    pub fn max(&self) -> [f64; 2] {
        [(self.content[0] - self.rect.width).max(0.), (self.content[1] - self.rect.height).max(0.)]
    }
}
/// How a transformed box's matrix was made: its box, the matrix of the
/// transformed box above it (or NONE), its border box and its own matrix
/// about its origin; so a changed `transform` patches the table without
/// building the scene again.
#[derive(Clone, Copy, Debug)]
pub(super) struct TransformEntry {
    owner: BoxId,
    parent: u32,
    outer: Rect,
    about: [f64; 6],
}
/// A box's `transform` about its `transform-origin`, as a matrix in the
/// document's space; None without a transform.
fn about_of(s: &Style, outer: Rect) -> Option<[f64; 6]> {
    let Value::Transforms(t) = s.value(P::Transform) else { return None };
    let local = t.matrix(outer.width, outer.height);
    let (ox, oy) = match s.value(P::TransformOrigin) {
        Value::Positions(p) if !p.0.is_empty() => (decoration::resolve_any(&p.0[0].0, outer.width), decoration::resolve_any(&p.0[0].1, outer.height)),
        _ => (outer.width / 2., outer.height / 2.),
    };
    let (ox, oy) = (outer.x + ox, outer.y + oy);
    Some(crate::css::properties::multiply_transforms([1., 0., 0., 1., ox, oy], crate::css::properties::multiply_transforms(local, [1., 0., 0., 1., -ox, -oy])))
}
static COMMAND_SERIALS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
/// A matrix that only moves (no scale, rotation or skew).
fn is_translation(m: [f64; 6]) -> bool {
    (m[0] - 1.).abs() < 1e-9 && m[1].abs() < 1e-9 && m[2].abs() < 1e-9 && (m[3] - 1.).abs() < 1e-9 && m[4].is_finite() && m[5].is_finite()
}
/// A run of commands in paint order that are shown the same way.
/// A scene's paint inputs as an identity, and its matrices
/// (`Scene::paint_state`).
pub struct PaintState {
    serial: u64,
    rest: u64,
    transforms: Box<[[f64; 6]]>,
    images: Vec<(u32, u64)>,
}
pub struct Stratum {
    pub kind: LayerKind,
    pub(super) index: index::Index,
}
pub struct Scene {
    pub(super) layout: Arc<layout::Layout>,
    /// Shared with the scenes patched from this one (`with_transforms`): the
    /// same commands, which `same_commands` tells.
    pub(super) commands: Arc<[Command]>,
    /// The commands' identity: unique to each list built (an address is
    /// not: the next list may land where a dropped one was).
    commands_serial: u64,
    pub(super) clips: Box<[Clip]>,
    pub(super) groups: Box<[Group]>,
    /// Cumulative affine matrices of transformed boxes, document space to
    /// document space (CSS Transforms 1 §6).
    pub(super) transforms: Box<[[f64; 6]]>,
    pub(super) transform_entries: Box<[TransformEntry]>,
    pub(super) moved: Box<[Moved]>,
    pub(super) scrollers: Box<[Scroller]>,
    /// The box each opacity group is that box's (`opacity` below 1), by
    /// group: an animated opacity patches its group (`with_transforms`).
    pub(super) group_owners: Box<[(BoxId, u32)]>,
    /// The boxes with `background-clip: text` and their border boxes
    /// (see `Primitive::Text::fill`).
    pub(super) text_fills: Arc<[(BoxId, Rect)]>,
    pub(super) vector_geometry: Arc<std::collections::HashMap<u32, crate::svg::used::Geometry>>,
    pub(super) vector_clips: Arc<std::collections::HashMap<u32, Vec<crate::svg::clip::Resolved>>>,
    /// The commands in paint order cut into strata: runs of the page's own
    /// commands, and the fixed and sticky boxes' layers between them, each
    /// indexed by place. The first is always a page run (the base, with
    /// the page's background); the rest are shown over it in order, the
    /// page runs scrolled, the fixed layer not, a sticky layer shifted.
    pub(super) strata: Box<[Stratum]>,
    pub(super) base: [u8; 4],
    pub(super) canvas: [u8; 4],
    extent: Rect,
    /// The page's scrollable overflow from its boxes, painted or not.
    layout_extent: Rect,
    /// What painting substituted (a dashed border drawn solid): kept by
    /// the scenes patched from this one, which paint the same.
    fallbacks: Arc<layout::Fallbacks>,
}
impl Scene {
    /// What painting this scene substituted.
    pub fn fallbacks(&self) -> &layout::Fallbacks {
        &self.fallbacks
    }
    pub fn layout(&self) -> &Arc<layout::Layout> {
        &self.layout
    }
    pub fn node(&self, command: u32) -> Option<crate::dom::NodeId> {
        if let Primitive::Image { owner, .. }
        | Primitive::Vector { owner, .. }
        | Primitive::Decoration { owner, .. }
        | Primitive::Border { owner, .. }
        | Primitive::Fill { owner, .. }
        | Primitive::Wave { owner, .. }
        | Primitive::Control { owner, .. }
        | Primitive::Glyph { owner, .. } = self.commands.get(command as usize)?.primitive
        {
            self.layout.cache().tree().get(owner).map(|b| b.node)
        } else {
            self.text_node(command)
        }
    }
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }
    /// Whether `other` paints the very same commands (it was patched from
    /// this scene, or this one from it): only matrices or layers moved.
    pub fn same_commands(&self, other: &Scene) -> bool {
        Arc::ptr_eq(&self.commands, &other.commands)
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
        let input_end = if last.kind != crate::text::prepare::MappingKind::Removed { last.input.start + (end - last.output.start) } else { last.input.end };
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
        let raw_start = if a.kind != crate::text::prepare::MappingKind::Removed { a.input.start + start - a.output.start } else { a.input.start };
        let raw_end = if b.kind != crate::text::prepare::MappingKind::Removed { b.input.start + end - b.output.start } else { b.input.end };
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
    /// How far the page reaches: what it paints, and its boxes (an empty
    /// one makes a page as tall as it, as in Chrome).
    pub fn content_extent(&self) -> Rect {
        if self.layout_extent.width <= 0. && self.layout_extent.height <= 0. { self.extent } else { union(self.extent, self.layout_extent) }
    }
    pub fn device_scale(&self) -> f64 {
        self.layout.device_scale()
    }
    pub fn command_payload_bytes(&self) -> usize {
        self.commands.len() * size_of::<Command>()
            + self.clips.len() * size_of::<Clip>()
            + self.strata.iter().map(|s| s.index.payload_bytes()).sum::<usize>()
            + self.vector_geometry.capacity() * (size_of::<(u32, crate::svg::used::Geometry)>() + 1)
            + self.vector_geometry.values().map(|g| g.payload_bytes()).sum::<usize>()
            + self.vector_clips.capacity() * (size_of::<(u32, Vec<crate::svg::clip::Resolved>)>() + 1)
            + self.vector_clips.values().map(|v| crate::svg::clip::resolved_bytes(v)).sum::<usize>()
    }
    /// What painting `region` depends on, as a hash: the commands over it
    /// (their geometry, paint and clips), the identities of the images,
    /// styles, texts and vectors they draw from. A tile whose signature is
    /// unchanged between scenes paints the same pixels and can be kept.
    pub fn tile_signature(&self, region: Rect, cancel: Option<&AtomicBool>) -> Result<u64> {
        Ok(self.tile_signature_parts(region, cancel)?.0)
    }
    /// The signature and, for checking why a tile repaints, each command's
    /// own hash with what it is.
    pub fn tile_signature_parts(&self, region: Rect, cancel: Option<&AtomicBool>) -> Result<(u64, Vec<(u64, &'static str, [u64; 5], u32)>)> {
        let mut hits = Vec::new();
        self.query(region, &mut hits, cancel)?;
        self.signature_parts(&hits, cancel)
    }
    /// The signature of these commands (in order), and each one's part.
    fn signature_parts(&self, hits: &[u32], cancel: Option<&AtomicBool>) -> Result<(u64, Vec<(u64, &'static str, [u64; 5], u32)>)> {
        use std::hash::{Hash, Hasher};
        let mut parts = Vec::new();
        let tree = self.layout.cache().tree();
        let mut h = signature_hasher();
        self.base.hash(&mut h);
        self.canvas.hash(&mut h);
        self.layout.device_scale().to_bits().hash(&mut h);
        hits.len().hash(&mut h);
        let rect = |r: Rect, h: &mut SignatureHasher| {
            r.x.to_bits().hash(h);
            r.y.to_bits().hash(h);
            r.width.to_bits().hash(h);
            r.height.to_bits().hash(h);
        };
        let matrix = |t: u32, h: &mut SignatureHasher| {
            (t != NONE).hash(h);
            if t != NONE {
                for v in self.transforms[t as usize] {
                    v.to_bits().hash(h);
                }
            }
        };
        let style_identity = |owner: BoxId, h: &mut SignatureHasher| {
            if let Some(s) = tree.style(owner) {
                s.identity().hash(h);
            }
        };
        let backgrounds = tree.images().map_or(0, |i| i.backgrounds_identity());
        let whole = h.finish();
        let mut combined = signature_hasher();
        whole.hash(&mut combined);
        for &id in hits {
            check(cancel)?;
            let c = &self.commands[id as usize];
            let mut h = signature_hasher();
            let mut sub = [0u64; 5];
            rect(c.bounds, &mut h);
            sub[0] = h.clone().finish();
            matrix(c.transform, &mut h);
            sub[1] = h.clone().finish();
            (c.clip != NONE).hash(&mut h);
            if c.clip != NONE {
                let clip = &self.clips[c.clip as usize];
                rect(clip.rect, &mut h);
                clip.radii.is_some().hash(&mut h);
                if let Some(r) = &clip.radii {
                    for corner in r {
                        for v in corner {
                            v.to_bits().hash(&mut h);
                        }
                    }
                }
                matrix(clip.transform, &mut h);
            }
            sub[2] = h.clone().finish();
            let mut group = c.group;
            while group != NONE {
                let g = &self.groups[group as usize];
                g.opacity.to_bits().hash(&mut h);
                if let Some(m) = g.mask {
                    rect(m.outer, &mut h);
                    style_identity(m.owner, &mut h);
                }
                group = g.parent;
            }
            sub[3] = h.clone().finish();
            match &c.primitive {
                Primitive::Image { image, .. } => {
                    0u8.hash(&mut h);
                    tree.image(*image).map_or(0, |i| i.serial()).hash(&mut h);
                }
                Primitive::Vector { vector, origin, .. } => {
                    1u8.hash(&mut h);
                    origin.x.to_bits().hash(&mut h);
                    origin.y.to_bits().hash(&mut h);
                    tree.vector(*vector).map_or(0, |v| v.fingerprint).hash(&mut h);
                }
                Primitive::Fill { color, radii, .. } => {
                    2u8.hash(&mut h);
                    color.hash(&mut h);
                    radii.map(f32::to_bits).hash(&mut h);
                }
                Primitive::Border { owner, colors, styles } => {
                    3u8.hash(&mut h);
                    colors.hash(&mut h);
                    styles.hash(&mut h);
                    style_identity(*owner, &mut h);
                }
                Primitive::Wave { color, thickness, wavelength, .. } => {
                    7u8.hash(&mut h);
                    color.hash(&mut h);
                    thickness.to_bits().hash(&mut h);
                    wavelength.to_bits().hash(&mut h);
                }
                Primitive::Control { look, .. } => {
                    8u8.hash(&mut h);
                    look.hash(&mut h);
                }
                Primitive::Decoration { owner, outer } => {
                    4u8.hash(&mut h);
                    rect(*outer, &mut h);
                    style_identity(*owner, &mut h);
                    backgrounds.hash(&mut h);
                }
                Primitive::Text { owner, fragment, glyphs, origin, color, fill } => {
                    5u8.hash(&mut h);
                    if let Some(&(box_, outer)) = self.text_fills.get(*fill as usize) {
                        rect(outer, &mut h);
                        style_identity(box_, &mut h);
                        backgrounds.hash(&mut h);
                    }
                    glyphs.start.hash(&mut h);
                    glyphs.end.hash(&mut h);
                    origin.x.to_bits().hash(&mut h);
                    origin.y.to_bits().hash(&mut h);
                    color.hash(&mut h);
                    self.layout.text_layout(*owner).and_then(|t| t.fragment_identity(*fragment)).hash(&mut h);
                }
                Primitive::Glyph { owner, fragment, glyph, origin, color } => {
                    6u8.hash(&mut h);
                    glyph.hash(&mut h);
                    origin.x.to_bits().hash(&mut h);
                    origin.y.to_bits().hash(&mut h);
                    color.hash(&mut h);
                    self.layout.text_layout(*owner).and_then(|t| t.fragment_identity(*fragment)).hash(&mut h);
                }
            }
            let part = h.finish();
            sub[4] = part;
            parts.push((
                part,
                match &c.primitive {
                    Primitive::Image { .. } => "image",
                    Primitive::Vector { .. } => "vector",
                    Primitive::Fill { .. } => "fill",
                    Primitive::Border { .. } => "border",
                    Primitive::Wave { .. } => "wave",
                    Primitive::Decoration { .. } => "decoration",
                    Primitive::Control { .. } => "control",
                    Primitive::Text { .. } => "text",
                    Primitive::Glyph { .. } => "glyph",
                },
                sub,
                id,
            ));
            part.hash(&mut combined);
        }
        Ok((combined.finish(), parts))
    }
    pub fn strata(&self) -> &[Stratum] {
        &self.strata
    }
    /// Stratum `i`'s commands over `region` (document coordinates, as laid
    /// out: a fixed layer's are viewport coordinates, a sticky layer's its
    /// flow position), in paint order.
    pub fn query_stratum(&self, i: usize, region: Rect, out: &mut Vec<u32>, cancel: Option<&AtomicBool>) -> Result<()> {
        if !valid(region) {
            return Err(Error::Invalid);
        }
        let stratum = self.strata.get(i).ok_or(Error::Invalid)?;
        stratum.index.query(&self.commands, &self.transforms, region, out, cancel)?;
        out.sort_unstable();
        Ok(())
    }
    /// What stratum `i`'s commands cover (clips applied), or None without any.
    pub fn stratum_extent(&self, i: usize) -> Option<Rect> {
        let stratum = self.strata.get(i)?;
        let mut out: Option<Rect> = None;
        for &id in stratum.index.ids() {
            let c = &self.commands[id as usize];
            let mut reach = extent(c, &self.transforms);
            if c.clip != NONE {
                let clip = &self.clips[c.clip as usize];
                let clip_rect = if clip.transform == NONE { clip.rect } else { aabb(self.transforms[clip.transform as usize], clip.rect) };
                reach = intersection(reach, clip_rect);
            }
            if reach.width <= 0. || reach.height <= 0. {
                continue;
            }
            out = Some(out.map_or(reach, |o| union(o, reach)));
        }
        out
    }
    /// The signature of all of stratum `i` (see `tile_signature`).
    pub fn stratum_signature(&self, i: usize, cancel: Option<&AtomicBool>) -> Result<u64> {
        let stratum = self.strata.get(i).ok_or(Error::Invalid)?;
        let mut hits: Vec<u32> = stratum.index.ids().to_vec();
        hits.sort_unstable();
        Ok(self.signature_parts(&hits, cancel)?.0)
    }
    /// The signature of stratum `i` over a tile (see `tile_signature`), and
    /// whether anything of it is there at all.
    pub fn stratum_tile_signature(&self, i: usize, region: Rect, cancel: Option<&AtomicBool>) -> Result<(u64, bool)> {
        let mut hits = Vec::new();
        self.query_stratum(i, region, &mut hits, cancel)?;
        Ok((self.signature_parts(&hits, cancel)?.0, !hits.is_empty()))
    }
    /// The moved layer a stratum holds, if it is one.
    pub fn moved(&self, i: usize) -> Option<&Moved> {
        let LayerKind::Moved(root) = self.strata.get(i)?.kind else { return None };
        self.moved.iter().find(|m| m.root == root)
    }
    /// What its tiles are painted from, as an identity: its commands (shared
    /// by the scenes patched from one another) and its matrices' values. Two
    /// scenes with one identity paint every tile the same (moved layers
    /// placed apart): a frame that only moves layers needs no tile looked at.
    pub fn paint_identity(&self) -> (usize, u64) {
        use std::hash::{Hash, Hasher};
        let mut h = signature_hasher();
        for m in self.transforms.iter() {
            for v in m {
                v.to_bits().hash(&mut h);
            }
        }
        self.strata.len().hash(&mut h);
        // New pictures (a frame swapped in) paint other tiles, and so do
        // groups' new opacities.
        self.layout.cache().tree().images_identity().hash(&mut h);
        for g in self.groups.iter() {
            g.opacity.to_bits().hash(&mut h);
        }
        (self.commands_serial as usize, h.finish())
    }
    /// What its tiles are painted from, kept to tell a next scene what it
    /// changed (`changed_region`).
    pub fn paint_state(&self) -> PaintState {
        use std::hash::{Hash, Hasher};
        let mut h = signature_hasher();
        self.strata.len().hash(&mut h);
        let tree = self.layout.cache().tree();
        tree.images().map_or(0, |set| set.backgrounds_identity()).hash(&mut h);
        for g in self.groups.iter() {
            g.opacity.to_bits().hash(&mut h);
        }
        // The pictures image commands show, by command (a canvas's next
        // frame changes its own tiles only).
        let images = self
            .commands
            .iter()
            .enumerate()
            .filter_map(|(i, c)| if let Primitive::Image { image, .. } = c.primitive { Some((i as u32, tree.image(image).map_or(0, |im| im.serial()))) } else { None })
            .collect();
        PaintState { serial: self.commands_serial, rest: h.finish(), transforms: self.transforms.clone(), images }
    }
    /// Where (document space) this scene paints otherwise than the scene
    /// `old` was the state of, when it has the same commands and only
    /// some matrices moved (an animated transform): the commands under
    /// those, where they were and where they are. None when it may differ
    /// anywhere (other commands, pictures, opacities; a moved command
    /// fixed, sticky or in a layer apart, whose tiles are elsewhere).
    pub fn changed_region(&self, old: &PaintState) -> Option<Vec<Rect>> {
        let now = self.paint_state();
        if now.serial != old.serial || now.rest != old.rest || now.transforms.len() != old.transforms.len() {
            return None;
        }
        let changed: Vec<bool> = now.transforms.iter().zip(old.transforms.iter()).map(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.to_bits() != y.to_bits())).collect();
        let mut region = Vec::new();
        // New pictures: where their commands show them.
        if now.images.len() != old.images.len() {
            return None;
        }
        for (&(i, a), &(j, b)) in now.images.iter().zip(old.images.iter()) {
            if i != j {
                return None;
            }
            if a != b {
                let c = &self.commands[i as usize];
                if c.fixed || c.sticky.is_some() || c.moved.is_some() {
                    return None;
                }
                region.push(extent_of(c, &now.transforms));
                if c.transform != NONE {
                    region.push(extent_of(c, &old.transforms));
                }
            }
        }
        if !changed.iter().any(|&c| c) {
            return Some(region);
        }
        // A clip moved: what it cuts may show anywhere it was or is.
        if self.clips.iter().any(|c| c.transform != NONE && changed[c.transform as usize]) {
            return None;
        }
        for c in self.commands.iter() {
            if c.transform == NONE || !changed[c.transform as usize] {
                continue;
            }
            if c.fixed || c.sticky.is_some() || c.moved.is_some() || region.len() >= 256 {
                return None;
            }
            region.push(extent_of(c, &now.transforms));
            region.push(extent_of(c, &old.transforms));
        }
        Some(region)
    }
    /// How many surface layers (live canvases shown apart) it has.
    pub fn surface_count(&self) -> usize {
        self.strata.iter().filter(|s| matches!(s.kind, LayerKind::Surface(_))).count()
    }
    /// Every moved layer.
    /// The scroll containers, in paint order.
    pub fn scrollers(&self) -> &[Scroller] {
        &self.scrollers
    }
    pub fn moved_layers(&self) -> &[Moved] {
        &self.moved
    }
    /// A surface stratum's image and where it shows (document coordinates):
    /// the live image of the one command it holds.
    pub fn surface(&self, i: usize) -> Option<(SurfaceShown, &Arc<crate::image::Image>)> {
        let stratum = self.strata.get(i)?;
        let LayerKind::Surface(_) = stratum.kind else { return None };
        let c = self.commands.get(*stratum.index.ids().first()? as usize)?;
        let Primitive::Image { image, .. } = c.primitive else { return None };
        let image = self.layout.cache().tree().image(image)?;
        let bounds = if c.transform == NONE { c.bounds } else { aabb(self.transforms[c.transform as usize], c.bounds) };
        let (clip, radii) = match self.clips.get(c.clip as usize) {
            None => (None, None),
            Some(clip) => {
                let rect = surface_clip(&self.transforms, clip)?;
                (Some(rect), clip.radii.and_then(|r| surface_corners(bounds, rect, &r).flatten()))
            }
        };
        let opacity = std::iter::successors((c.group != NONE).then_some(c.group), |&g| (self.groups[g as usize].parent != NONE).then(|| self.groups[g as usize].parent))
            .map(|g| self.groups[g as usize].opacity)
            .product();
        Some((SurfaceShown { bounds, clip, radii, opacity }, image))
    }

    /// This scene over `layout` (the same boxes, with new styles) with the
    /// matrices of the `changed` boxes made again from their styles, and
    /// those under them composed again: a changed `transform` without
    /// building the scene. None when a box's transform came or went
    /// (the scene's structure would differ): build it then.
    pub fn with_transforms(&self, layout: Arc<layout::Layout>, changed: &[BoxId], cancel: Option<&AtomicBool>) -> Result<Option<Arc<Scene>>> {
        let tree = layout.cache().tree();
        let mut entries = self.transform_entries.to_vec();
        let mut touched = vec![false; entries.len()];
        let mut moved = self.moved.to_vec();
        let mut groups = self.groups.to_vec();
        for &id in changed {
            check(cancel)?;
            let s = tree.style(id).ok_or(Error::Invalid)?;
            // An opacity that stays a group's (strictly between 0 and 1):
            // that group's alpha, the same commands. One that reaches 0
            // or 1 (the group goes, or the box stops painting) or starts
            // needs the scene built.
            let opacity = s.value(P::Opacity).scalar().unwrap_or(1.).clamp(0., 1.) as f32;
            let refused = |why: &str| {
                if crate::env_on!("OLO_WEB_TIMING") {
                    eprintln!("  scene patch refused: {why} (opacity {opacity})");
                }
            };
            match self.group_owners.iter().find(|(o, _)| *o == id) {
                Some(&(_, g)) => {
                    if !(opacity > 0. && opacity < 1.) {
                        refused("a group's opacity reached 0 or 1");
                        return Ok(None);
                    }
                    groups[g as usize].opacity = opacity;
                }
                // A box the build gives no group whatever its opacity (an
                // element's line boxes, an inline image: their element's
                // main box has the group): built again, it would be the same.
                None if opacity < 1.
                    && !tree.get(id).is_some_and(|b| matches!(b.kind, BoxKind::InlineContent { .. } | BoxKind::InlineImage { .. } | BoxKind::InlineVector { .. } | BoxKind::InlineReplaced { .. })) =>
                {
                    refused("an opacity below 1 without a group");
                    return Ok(None);
                }
                None => {}
            }
            // A moved layer's root: its translation, nothing to repaint.
            if let Some(m) = moved.iter_mut().find(|m| m.root == id) {
                let Some(about) = about_of(s, m.outer).filter(|a| invertible(*a)) else {
                    refused("a moved layer's transform is no longer invertible");
                    return Ok(None);
                };
                m.matrix = about;
                continue;
            }
            let Some(i) = entries.iter().position(|e| e.owner == id) else {
                // Never transformed; one that is now needs the scene built.
                // (An identity transform, which the build gave no matrix,
                // is no change; nor is one on an element's line boxes, which
                // its main box's transform carries.)
                if matches!(s.value(P::Transform), Value::Transforms(_))
                    && !tree.get(id).is_some_and(|b| matches!(b.kind, BoxKind::InlineContent { .. }))
                    && !layout.bounds(id).and_then(|outer| about_of(s, outer)).is_some_and(crate::css::properties::is_identity)
                {
                    refused("a transform on a box that had none");
                    return Ok(None);
                }
                continue;
            };
            let Some(about) = about_of(s, entries[i].outer) else {
                refused("a transform went");
                return Ok(None);
            };
            if entries[i].parent == NONE && crate::css::properties::is_identity(about) {
                refused("a transform became the identity");
                return Ok(None);
            }
            entries[i].about = about;
            touched[i] = true;
        }
        if !touched.iter().any(|&t| t) {
            // Nothing transformed changed: the same scene over the new layout.
            return Ok(Some(Arc::new(Scene {
                extent: self.extent,
                layout_extent: self.layout_extent,
                fallbacks: self.fallbacks.clone(),
                layout,
                commands: self.commands.clone(),
                commands_serial: self.commands_serial,
                clips: self.clips.clone(),
                groups: groups.clone().into_boxed_slice(),
                group_owners: self.group_owners.clone(),
                text_fills: self.text_fills.clone(),
                transforms: self.transforms.clone(),
                transform_entries: self.transform_entries.clone(),
                moved: moved.clone().into_boxed_slice(),
                scrollers: self.scrollers.clone(),
                vector_geometry: self.vector_geometry.clone(),
                vector_clips: self.vector_clips.clone(),
                strata: self.strata.iter().map(|s| Stratum { kind: s.kind, index: s.index.clone() }).collect(),
                base: self.base,
                canvas: self.canvas,
            })));
        }
        let mut transforms = self.transforms.to_vec();
        for i in 0..entries.len() {
            let parent = entries[i].parent;
            if parent != NONE && touched[parent as usize] {
                touched[i] = true;
            }
            if touched[i] {
                let m = if parent == NONE { entries[i].about } else { crate::css::properties::multiply_transforms(transforms[parent as usize], entries[i].about) };
                if !m.iter().all(|v| v.is_finite()) {
                    return Err(Error::Invalid);
                }
                transforms[i] = m;
            }
        }
        let mut extent = Rect::default();
        for c in self.commands.iter() {
            let reach = extent_of(c, &transforms);
            extent = union(extent, if c.clip == NONE { reach } else { intersection(reach, self.clip_extent_with(c.clip, &transforms)) });
        }
        let mut strata = Vec::with_capacity(self.strata.len());
        for stratum in &self.strata {
            check(cancel)?;
            strata.push(Stratum { kind: stratum.kind, index: index::Index::over(&self.commands, &transforms, stratum.index.ids().to_vec(), cancel)? });
        }
        Ok(Some(Arc::new(Scene {
            extent,
            layout_extent: self.layout_extent,
            fallbacks: self.fallbacks.clone(),
            layout,
            commands: self.commands.clone(),
            commands_serial: self.commands_serial,
            clips: self.clips.clone(),
            groups: groups.into_boxed_slice(),
            group_owners: self.group_owners.clone(),
            text_fills: self.text_fills.clone(),
            transforms: transforms.into_boxed_slice(),
            transform_entries: entries.into_boxed_slice(),
            moved: moved.into_boxed_slice(),
            scrollers: self.scrollers.clone(),
            vector_geometry: self.vector_geometry.clone(),
            vector_clips: self.vector_clips.clone(),
            strata: strata.into_boxed_slice(),
            base: self.base,
            canvas: self.canvas,
        })))
    }
    fn clip_extent_with(&self, clip: u32, transforms: &[[f64; 6]]) -> Rect {
        let c = &self.clips[clip as usize];
        if c.transform == NONE { c.rect } else { aabb(transforms[c.transform as usize], c.rect) }
    }
    /// The node at document point (x, y) with the page scrolled by `scroll`
    /// in a viewport `height` tall: the topmost command there in paint
    /// order, strata from the top down, each tested where it is shown
    /// (the fixed layer at the viewport point, a sticky layer shifted),
    /// skipping boxes with `pointer-events: none` or hidden.
    pub fn hit_test(&self, x: f64, y: f64, scroll: f64, height: f64, stickies: &[layout::Sticky]) -> Option<crate::dom::NodeId> {
        let tree = self.layout.cache().tree();
        let mut hits = Vec::new();
        for (i, stratum) in self.strata.iter().enumerate().rev() {
            let (x, py) = match stratum.kind {
                LayerKind::Page | LayerKind::Surface(_) | LayerKind::Backdrop(_) => (x, y),
                LayerKind::Fixed => (x, y - scroll),
                LayerKind::Sticky(root) => (x, y - stickies.iter().find(|s| s.root == root).map_or(0., |s| s.offset(scroll, height))),
                LayerKind::Moved(_) => {
                    let Some(m) = self.moved(i) else { continue };
                    if m.clip.is_some_and(|c| x < c.x || x >= c.x + c.width || y < c.y || y >= c.y + c.height) {
                        continue;
                    }
                    m.unapply(x, y)
                }
            };
            let region = Rect { x: x - 0.5, y: py - 0.5, width: 1., height: 1. };
            if self.query_stratum(i, region, &mut hits, None).is_err() {
                continue;
            }
            for &id in hits.iter().rev() {
                let c = &self.commands[id as usize];
                let reach = extent(c, &self.transforms);
                if x < reach.x || x >= reach.x + reach.width || py < reach.y || py >= reach.y + reach.height {
                    continue;
                }
                if c.clip != NONE {
                    let clip = &self.clips[c.clip as usize];
                    let clip_rect = if clip.transform == NONE { clip.rect } else { aabb(self.transforms[clip.transform as usize], clip.rect) };
                    if x < clip_rect.x || x >= clip_rect.x + clip_rect.width || py < clip_rect.y || py >= clip_rect.y + clip_rect.height {
                        continue;
                    }
                }
                let Some(node) = self.node(id) else { continue };
                let takes_none = |style: &Style| matches!(style.value(P::PointerEvents), Value::Keyword(K::None)) || matches!(style.value(P::Visibility), Value::Keyword(K::Hidden));
                let ignored = |node| tree.styles().snapshot(node).is_some_and(takes_none);
                // The painting box's own style: a pseudo-element's (apple's
                // `body::before` scrim over its nav takes no pointer events).
                let owner = match c.primitive {
                    Primitive::Image { owner, .. }
                    | Primitive::Vector { owner, .. }
                    | Primitive::Decoration { owner, .. }
                    | Primitive::Border { owner, .. }
                    | Primitive::Fill { owner, .. }
                    | Primitive::Control { owner, .. }
                    | Primitive::Wave { owner, .. } => Some(owner),
                    Primitive::Glyph { owner, .. } => Some(owner),
                    Primitive::Text { .. } => None,
                };
                if ignored(node) || owner.and_then(|o| tree.style(o)).is_some_and(takes_none) {
                    // A graphic or text that takes no pointer events passes
                    // the point to the boxes it is drawn in (up to its
                    // positioned one), as Chrome: apple.com's nav links are
                    // `pointer-events: none` SVGs in links painted over an
                    // open flyout, which took their hovers.
                    if c.transform == NONE
                        && let Primitive::Vector { owner, .. } | Primitive::Image { owner, .. } | Primitive::Text { owner, .. } = c.primitive
                        && let Some(found) = self.box_under(owner, x, py, &takes_none)
                    {
                        return Some(found);
                    }
                    continue;
                }
                // A box hit through its background or border: a box in it
                // that paints nothing there (a transparent button over its
                // row) is still what is under the point, as in Chrome.
                // One that takes no pointer events (`pointer-events: none`,
                // hidden) passes the point to the box it is in: apple.com's
                // nav links hold a `pointer-events: none` span, and the
                // nav itself took their hovers.
                if let Primitive::Decoration { owner, .. } | Primitive::Border { owner, .. } | Primitive::Fill { owner, .. } = c.primitive
                    && c.transform == NONE
                    && let Some(mut inner) = self.deepest_box(owner, x, py)
                {
                    for _ in 0..4096 {
                        let Some(b) = tree.get(inner) else { break };
                        if inner == owner {
                            break;
                        }
                        if !ignored(b.node) && !matches!(b.kind, BoxKind::InlineContent { .. }) {
                            return Some(b.node);
                        }
                        let Some(parent) = b.parent else { break };
                        inner = parent;
                    }
                }
                return Some(node);
            }
        }
        // Nothing painted there: the innermost box in flow under the point,
        // as Chrome finds it (a row without a background, a page's bare body).
        let root = tree.get(tree.root())?.first_child?;
        let inner = self.deepest_box(root, x, y).unwrap_or(root);
        let node = tree.get(inner)?.node;
        let hidden =
            tree.styles().snapshot(node).is_some_and(|style| matches!(style.value(P::PointerEvents), Value::Keyword(K::None)) || matches!(style.value(P::Visibility), Value::Keyword(K::Hidden)));
        (!hidden).then_some(node)
    }
    /// The innermost in-flow box under `owner` whose border box holds the
    /// point (layout coordinates), later siblings winning (painted above);
    /// None when no box inside it does. Only boxes holding the point are
    /// entered, so a hit costs the depth times the siblings on the way.
    /// Positioned, transformed and clipped-away boxes are left to their own
    /// paint.
    /// The nearest box around `from` (itself, for a text's box) under the
    /// point that takes pointer events, no further out than its first
    /// positioned or transformed ancestor (a layer of its own above what
    /// is under it).
    fn box_under(&self, from: BoxId, x: f64, y: f64, takes_none: &dyn Fn(&Style) -> bool) -> Option<crate::dom::NodeId> {
        let tree = self.layout.cache().tree();
        let mut at = from;
        for _ in 0..256 {
            let b = tree.get(at)?;
            let s = tree.style(at)?;
            if at != from && !matches!(b.kind, BoxKind::InlineContent { .. }) {
                let r = self.layout.bounds(at)?;
                let shift = self.scroll_shift(at);
                let (rx, ry) = (r.x - shift[0], r.y - shift[1]);
                if !takes_none(s) && x >= rx && y >= ry && x < rx + r.width && y < ry + r.height {
                    return Some(b.node);
                }
                if matches!(s.value(P::Transform), Value::Transforms(_)) || !matches!(s.value(P::Position), Value::Keyword(K::Static)) {
                    return None;
                }
            }
            at = b.parent?;
        }
        None
    }
    fn deepest_box(&self, owner: BoxId, x: f64, y: f64) -> Option<BoxId> {
        let layout = &self.layout;
        let tree = layout.cache().tree();
        let start = layout.bounds(owner)?;
        let shift = self.scroll_shift(owner);
        let mut origin = (start.x - layout.geometry(owner)?.border_box.x - shift[0], start.y - layout.geometry(owner)?.border_box.y - shift[1]);
        let (mut at, mut found) = (owner, None);
        for _ in 0..4096 {
            let g = layout.geometry(at)?;
            let content = g.content_box();
            let offset = self.scroll_offset_of(at);
            let inner = (origin.0 + content.x - offset[0], origin.1 + content.y - offset[1]);
            let mut next = None;
            let mut child = tree.get(at)?.first_child;
            while let Some(id) = child {
                let b = tree.get(id)?;
                child = b.next_sibling;
                if b.absolute || b.marker || matches!(b.kind, BoxKind::Viewport) {
                    continue;
                }
                if tree.style(id).is_some_and(|s| matches!(s.value(P::Transform), Value::Transforms(_)) || !matches!(s.value(P::Position), Value::Keyword(K::Static | K::Relative))) {
                    continue;
                }
                let r = layout.geometry(id)?.border_box;
                let (rx, ry) = (inner.0 + r.x, inner.1 + r.y);
                if x >= rx && y >= ry && x < rx + r.width && y < ry + r.height {
                    next = Some(id);
                }
            }
            let Some(id) = next else { break };
            found = Some(id);
            origin = inner;
            at = id;
        }
        found
    }
    /// A scroll container's offset (zero for any other box).
    fn scroll_offset_of(&self, id: BoxId) -> [f64; 2] {
        if self.scrollers.iter().all(|s| s.offset == [0., 0.]) {
            return [0., 0.];
        }
        self.scrollers.iter().find(|s| s.root == id).map_or([0., 0.], |s| s.offset)
    }
    /// How far the scroll containers around a box moved it.
    pub fn scroll_shift(&self, id: BoxId) -> [f64; 2] {
        let mut shift = [0., 0.];
        if self.scrollers.iter().all(|s| s.offset == [0., 0.]) {
            return shift;
        }
        let tree = self.layout.cache().tree();
        let mut at = tree.get(id).and_then(|b| b.parent);
        while let Some(p) = at {
            let o = self.scroll_offset_of(p);
            shift[0] += o[0];
            shift[1] += o[1];
            at = tree.get(p).and_then(|b| b.parent);
        }
        shift
    }
    /// The page's own commands over `region` (every page stratum), in
    /// paint order: not the fixed or sticky layers'.
    pub fn query(&self, region: Rect, out: &mut Vec<u32>, cancel: Option<&AtomicBool>) -> Result<()> {
        if !valid(region) {
            return Err(Error::Invalid);
        }
        out.clear();
        let mut part = Vec::new();
        for stratum in self.strata.iter().filter(|s| matches!(s.kind, LayerKind::Page | LayerKind::Backdrop(_))) {
            stratum.index.query(&self.commands, &self.transforms, region, &mut part, cancel)?;
            out.extend_from_slice(&part);
        }
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
    /// Under `opacity: 0`: nothing of it paints (its boxes are walked for
    /// their states and nothing else).
    hidden: bool,
    /// Under a `position: fixed` box (see `Command::fixed`).
    fixed: bool,
    /// Under a `position: sticky` box (see `Command::sticky`).
    sticky: Option<BoxId>,
    /// Under a moved layer (see `Command::moved`).
    moved: Option<BoxId>,
    /// Under a backdrop layer (see `Command::backdrop`).
    backdrop: Option<BoxId>,
    /// The scroll container whose content this is (index into
    /// `scrollers`), or NONE.
    scroller: u32,
    /// How far the scroll containers around moved this box's content
    /// (what a box positioned against a block outside them takes back).
    shift: Point,
    /// Under a box with `background-clip: text` (see
    /// `Primitive::Text::fill`), or NONE.
    text_fill: u32,
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
    /// A positioned box with `z-index: auto` (and nothing else making it a
    /// stacking context): painted as if it were one, but its positioned
    /// descendants and the stacking contexts inside it belong to the
    /// nearest real one (CSS 2 Appendix E), so a `z-index: 2` inside it
    /// rises above a later sibling's positioned box.
    pseudo: bool,
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
    layout_extent: Rect,
    states: Vec<State>,
    draft: Vec<Draft>,
    clips: Vec<Clip>,
    groups: Vec<Group>,
    current_group: u32,
    transforms: Vec<[f64; 6]>,
    transform_entries: Vec<TransformEntry>,
    current_transform: u32,
    /// The box being painted is fixed to the viewport (or under one).
    current_fixed: bool,
    /// The sticky box the one being painted is under, if any.
    current_sticky: Option<BoxId>,
    /// The command being emitted is a live surface's image.
    current_surface: bool,
    /// The moved layer the box being painted is under.
    current_moved: Option<BoxId>,
    current_backdrop: Option<BoxId>,
    /// Nodes whose `transform` animates: each translated one becomes a
    /// moved layer.
    animated: std::collections::HashSet<crate::dom::NodeId>,
    moved: Vec<Moved>,
    scrollers: Vec<Scroller>,
    group_owners: Vec<(BoxId, u32)>,
    text_fills: Vec<(BoxId, Rect)>,
    current_text_fill: u32,
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
    font_lines: std::collections::HashMap<usize, (f64, f64, f64, f64, f64)>,
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
            layout_extent: Rect::default(),
            states: Vec::new(),
            draft: Vec::new(),
            clips: Vec::new(),
            groups: Vec::new(),
            current_group: NONE,
            transforms: Vec::new(),
            current_transform: NONE,
            current_fixed: false,
            current_sticky: None,
            current_surface: false,
            current_moved: None,
            current_backdrop: None,
            animated: Default::default(),
            moved: Vec::new(),
            scrollers: Vec::new(),
            group_owners: Vec::new(),
            text_fills: Vec::new(),
            current_text_fill: NONE,
            vector_geometry: Default::default(),
            vector_clips: Default::default(),
            transform_entries: Vec::new(),
            vector_clip_bytes: 0,
            svg_viewports: None,
            next: 0,
            contexts: vec![Context { parent: NONE, z: 0, path: Vec::new(), pseudo: false }],
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
        let command = Command {
            bounds,
            primitive,
            clip,
            group: self.current_group,
            transform: self.current_transform,
            fixed: self.current_fixed,
            sticky: self.current_sticky,
            surface: self.current_surface,
            moved: self.current_moved,
            backdrop: self.current_backdrop,
            reach: if self.current_group == NONE { 0. } else { self.groups[self.current_group as usize].reach },
        };
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
    fn transform_of(&mut self, id: BoxId, s: &Style, outer: Rect, parent: u32) -> Result<u32> {
        let Some(about) = about_of(s, outer) else { return Ok(parent) };
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
        self.transform_entries.push(TransformEntry { owner: id, parent, outer, about });
        Ok(self.transforms.len() as u32 - 1)
    }
    #[allow(clippy::too_many_arguments)]
    fn decorate(&mut self, id: BoxId, s: &Style, g: layout::Geometry, outer: Rect, group: (u32, u8), clip: u32, text: bool) -> Result<()> {
        let look = self.control_look(id, s)?;
        match look {
            // A checkbox or radio drawn by the engine: its CSS background
            // and border are not (as Chrome ignores them there).
            Some(look @ ControlLook { kind: ControlKind::Checkbox { .. } | ControlKind::Radio { .. }, .. }) => {
                return self.emit(group, text, outer, Primitive::Control { owner: id, look }, clip);
            }
            // A field or a button the page has not styled (its UA border and
            // background): drawn as Chrome's theme draws it.
            Some(look @ ControlLook { kind: ControlKind::Field | ControlKind::Button, .. }) => {
                let near = |a: [u8; 4], b: [u8; 4]| a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 1);
                let field = look.kind == ControlKind::Field;
                let style = if field { K::Inset } else { K::Outset };
                let sides =
                    [(P::BorderTopStyle, P::BorderTopColor), (P::BorderRightStyle, P::BorderRightColor), (P::BorderBottomStyle, P::BorderBottomColor), (P::BorderLeftStyle, P::BorderLeftColor)];
                let first = color(s, P::BorderTopColor)?;
                let unstyled = [g.border_width.top, g.border_width.right, g.border_width.bottom, g.border_width.left].iter().all(|&w| (w - 2.).abs() < 0.01)
                    && sides.iter().all(|&(st, c)| matches!(s.value(st), Value::Keyword(k) if *k == style) && color(s, c).is_ok_and(|c| c == first))
                    // The UA's colours: a field's #767676 (#858585 dark), a button's
                    // its text colour; and its background the UA's.
                    && (if field { near(first, [0x76, 0x76, 0x76, 255]) || near(first, [0x85, 0x85, 0x85, 255]) } else { near(first, color(s, P::Color)?) })
                    && {
                        // Field, ButtonFace (light, dark), or a disabled field's faded one.
                        let bg = color(s, P::BackgroundColor)?;
                        let ua: &[[u8; 4]] = if field { &[[255; 4], [0x3b, 0x3b, 0x3b, 255], [239, 239, 239, 77], [59, 59, 59, 77]] } else { &[[0xef, 0xef, 0xef, 255], [0x6b, 0x6b, 0x6b, 255]] };
                        ua.iter().any(|&u| near(bg, u))
                    }
                    && !decoration::rounded(&decoration::radii(s, outer))
                    && !matches!(s.value(P::BackgroundImage), Value::Images(_))
                    && !matches!(s.value(P::BoxShadow), Value::Shadows(_));
                if unstyled {
                    return self.emit(group, text, outer, Primitive::Control { owner: id, look }, clip);
                }
                return self.decorate_box(id, s, g, outer, group, clip, text);
            }
            Some(look) => {
                // A select the page has not styled is drawn as Chrome's theme
                // draws it: a field's background in a rounded frame.
                // (The UA sheet's colours, a disabled one's faded to 30%.)
                let dark = look.fill[0] < 0x80;
                let mut ua = if dark { ([0x6b, 0x6b, 0x6b, 255], [0x85, 0x85, 0x85, 255]) } else { ([0xef, 0xef, 0xef, 255], [0x76, 0x76, 0x76, 255]) };
                if look.border == look.accent {
                    ua = (if dark { [59, 59, 59, 77] } else { [239, 239, 239, 77] }, [ua.1[0], ua.1[1], ua.1[2], 77]);
                }
                let near = |a: [u8; 4], b: [u8; 4]| a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 1);
                let unstyled = near(color(s, P::BackgroundColor)?, ua.0)
                    && [P::BorderTopColor, P::BorderRightColor, P::BorderBottomColor, P::BorderLeftColor].iter().all(|&p| color(s, p).is_ok_and(|c| near(c, ua.1)))
                    && [g.border_width.top, g.border_width.right, g.border_width.bottom, g.border_width.left].iter().all(|&w| w == g.border_width.top)
                    && !decoration::rounded(&decoration::radii(s, outer))
                    && !matches!(s.value(P::BackgroundImage), Value::Images(_))
                    && !matches!(s.value(P::BoxShadow), Value::Shadows(_));
                if unstyled {
                    self.emit(group, text, outer, Primitive::Control { owner: id, look: ControlLook { kind: ControlKind::Menulist, ..look } }, clip)?;
                } else {
                    self.decorate_box(id, s, g, outer, group, clip, text)?;
                }
                // The arrow: a chevron centred in the select's end padding, its
                // size following the font (as Chrome's, measured at 1× and 2×).
                let k = s.value(P::FontSize).scalar().map_or(1., |f| f / (40. / 3.));
                let (half, rise) = (4.4 * k, 7.6 * k);
                let rtl = matches!(s.value(P::Direction), Value::Keyword(K::Rtl));
                let cx = if rtl { outer.x + g.border_width.left + 7.9 * k } else { outer.x + outer.width - g.border_width.right - 7.9 * k };
                let top = outer.y + outer.height / 2. - 5. * k;
                let bounds = Rect { x: cx - half - 1.2 * k, y: top, width: 2. * (half + 1.2 * k), height: rise };
                return self.emit(group, text, bounds, Primitive::Control { owner: id, look }, clip);
            }
            None => {}
        }
        self.decorate_box(id, s, g, outer, group, clip, text)
    }
    /// The look of a form control this box draws itself (`appearance` not
    /// `none`): a checkbox, a radio button, a select's arrow.
    fn control_look(&self, id: BoxId, s: &Style) -> Result<Option<ControlLook>> {
        if matches!(s.value(P::Appearance), Value::Keyword(K::None)) {
            return Ok(None);
        }
        let node = self.layout.cache().tree().get(id).ok_or(Error::Invalid)?.node;
        if self.doc.namespace(node)? != Some(crate::dom::Namespace::Html) {
            return Ok(None);
        }
        let kind = match self.doc.name(node)? {
            Some("input") => match self.doc.attribute(node, "type")?.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
                Some("checkbox") => ControlKind::Checkbox { checked: self.doc.control_checked(node)? },
                Some("radio") => ControlKind::Radio { checked: self.doc.control_checked(node)? },
                Some("submit" | "button" | "reset") => ControlKind::Button,
                Some("hidden" | "image" | "range" | "color" | "file") => return Ok(None),
                _ => ControlKind::Field,
            },
            Some("button") => ControlKind::Button,
            // A drop-down (not a list box: `multiple`, or a size above one).
            Some("select") if self.doc.attribute(node, "multiple")?.is_none() && self.doc.attribute(node, "size")?.and_then(|v| v.trim().parse::<u32>().ok()).is_none_or(|n| n <= 1) => {
                ControlKind::Arrow
            }
            _ => return Ok(None),
        };
        let disabled = self.doc.attribute(node, "disabled")?.is_some();
        let preferred = if self.options.dark { crate::css::color::Scheme::Dark } else { crate::css::color::Scheme::Light };
        let dark = match s.value(P::ColorScheme) {
            Value::Keyword(k) => crate::css::style::used_scheme(*k, preferred) == crate::css::color::Scheme::Dark,
            _ => false,
        };
        // Chrome's colours (light: #fff, #767676, #0075ff; dark: #3b3b3b,
        // #858585, #99c8ff), disabled ones as its are, faded.
        let (mut fill, mut border, mut accent) =
            if dark { ([0x3b, 0x3b, 0x3b, 255], [0x85, 0x85, 0x85, 255], [0x99, 0xc8, 0xff, 255]) } else { ([255; 4], [0x76, 0x76, 0x76, 255], [0x00, 0x75, 0xff, 255]) };
        if let Value::Color(_) = s.value(P::AccentColor) {
            accent = color(s, P::AccentColor)?;
        }
        // The check on the accent: white on a dark one, dark on a light one.
        let luma = |c: [u8; 4]| 0.2126 * f64::from(c[0]) + 0.7152 * f64::from(c[1]) + 0.0722 * f64::from(c[2]);
        let mut mark = if luma(accent) > 160. { [0x3b, 0x3b, 0x3b, 255] } else { [255; 4] };
        if disabled {
            if dark {
                (fill, border, accent, mark) = ([0x36, 0x36, 0x36, 255], [0x4f, 0x4f, 0x4f, 255], [0x4f, 0x4f, 0x4f, 255], [0x36, 0x36, 0x36, 255]);
            } else {
                (fill, border, accent, mark) = ([0xf8, 0xf8, 0xf8, 255], [0xd1, 0xd1, 0xd1, 255], [0xd1, 0xd1, 0xd1, 255], [0xed, 0xed, 0xed, 255]);
            }
        }
        if matches!(kind, ControlKind::Field | ControlKind::Button) {
            // Their background as the page's style has it (Field, ButtonFace,
            // or faded when disabled); their border Chrome's, faded likewise.
            fill = color(s, P::BackgroundColor)?;
            border = if dark { [0x85, 0x85, 0x85, 255] } else { [0x76, 0x76, 0x76, 255] };
            if disabled {
                border[3] = 77;
            }
        }
        if kind == ControlKind::Arrow {
            mark = color(s, P::Color)?;
            // A disabled select's frame, as Chrome's theme fades it.
            if disabled && !dark {
                (fill, border) = ([0xfb, 0xfb, 0xfb, 255], [0xe0, 0xe0, 0xe0, 255]);
            }
        }
        Ok(Some(ControlLook { kind, fill, border, accent, mark }))
    }
    #[allow(clippy::too_many_arguments)]
    fn decorate_box(&mut self, id: BoxId, s: &Style, g: layout::Geometry, outer: Rect, group: (u32, u8), clip: u32, text: bool) -> Result<()> {
        let radii = decoration::radii(s, outer);
        let images = matches!(s.value(P::BackgroundImage), Value::Images(_));
        let shadows = matches!(s.value(P::BoxShadow), Value::Shadows(_));
        // A colour clipped to less than the border box (or to the text).
        let clipped = !matches!(s.value(P::BackgroundClip), Value::Keyword(K::BorderBox)) && self.canvas_owner != Some(id) && color(s, P::BackgroundColor)?[3] != 0;
        if decoration::rounded(&radii) || images || shadows || clipped {
            if let Value::Images(list) = s.value(P::BackgroundImage)
                && list.0.iter().flatten().any(|i| matches!(i, crate::css::properties::Image::Url(url) if self.layout.cache().tree().background(url).is_none()))
            {
                self.note("background image not loaded", id);
            }
            if let Value::Shadows(list) = s.value(P::BoxShadow)
                && list.0.iter().any(|s| s.inset)
                && self.current_transform != NONE
            {
                self.note("inset box-shadow under a transform not painted", id);
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
            self.emit(group, text, outer, Primitive::Fill { color: bg, owner: id, radii: [0.; 4] }, clip)?;
        }
        let widths = [g.border_width.top, g.border_width.right, g.border_width.bottom, g.border_width.left];
        let mut colors = [[0; 4]; 4];
        let mut styles = [super::border::Style::Solid; 4];
        for i in 0..4 {
            if widths[i] > 0. {
                let p = [P::BorderTopStyle, P::BorderRightStyle, P::BorderBottomStyle, P::BorderLeftStyle][i];
                colors[i] = color(s, [P::BorderTopColor, P::BorderRightColor, P::BorderBottomColor, P::BorderLeftColor][i])?;
                match s.value(p) {
                    Value::Keyword(K::Solid) => {}
                    Value::Keyword(K::Dashed) => styles[i] = super::border::Style::Dashed,
                    Value::Keyword(K::Dotted) => styles[i] = super::border::Style::Dotted,
                    // Double needs room for its two lines and the gap.
                    Value::Keyword(K::Double) if widths[i] >= 3. => styles[i] = super::border::Style::Double,
                    Value::Keyword(K::Double) => {}
                    // Shaded: the top and left darker for inset (lighter for
                    // outset), as Chrome shades them (an iframe's border);
                    // groove and ridge likewise, as two-tone bevels.
                    Value::Keyword(k @ (K::Inset | K::Outset)) => colors[i] = decoration::bevel(colors[i], *k == K::Inset, i == 0 || i == 3),
                    Value::Keyword(k @ (K::Groove | K::Ridge)) => colors[i] = decoration::bevel(colors[i], *k == K::Groove, i == 0 || i == 3),
                    _ => self.note("border style painted solid", id),
                }
            }
        }
        if widths.iter().any(|w| *w > 0.) {
            self.emit(group, text, outer, Primitive::Border { owner: id, colors, styles }, clip)?;
        }
        Ok(())
    }
    /// The group a box and its content paint in, if they need one: its
    /// opacity, mask, filter (colours, blur, drop shadow) and blend mode,
    /// under `clip`, through `transformed` (the box's matrix, its
    /// ancestors' with it). Block boxes and atomic inlines alike.
    #[allow(clippy::too_many_arguments)]
    fn group_of(&mut self, id: BoxId, s: &Style, outer: Rect, inside_border: bool, opacity: f64, transformed: u32, parent_group: u32, clip: u32) -> Result<Option<u32>> {
        let layout = self.layout.clone();
        let tree = layout.cache().tree();
        let extent = decoration::shadow_extent(s);
        let unshadowed = extent.left <= 0. && extent.right <= 0. && extent.top <= 0. && extent.bottom <= 0.;
        let mask = match s.value(P::MaskImage) {
            Value::Images(_) if inside_border && unshadowed && decoration::mask_is_opaque(s) => None,
            Value::Images(list) => {
                let urls = list.0.iter().flatten().filter_map(|i| if let crate::css::properties::Image::Url(u) = i { Some(u) } else { None });
                let mut loaded = false;
                let mut missing = false;
                for url in urls {
                    if tree.background(url).is_some() {
                        loaded = true;
                    } else {
                        missing = true;
                    }
                }
                let gradient = list.0.iter().flatten().any(|i| !matches!(i, crate::css::properties::Image::Url(_)));
                if missing {
                    self.note("mask-image not loaded", id);
                }
                (loaded || gradient).then_some(MaskRef { owner: id, outer, transform: self.current_transform })
            }
            _ => None,
        };
        // filter: the box and its content through its colour
        // functions and its blur, as one group, its drop shadow
        // under it (one; url() not yet).
        let mut shadow = None;
        let mut after = None;
        let (filter, blur) = match s.value(P::Filter) {
            Value::Filters(f) => {
                if f.0.iter().any(|f| matches!(f, crate::css::properties::Filter::Other)) {
                    self.note("filter url() not drawn", id);
                }
                let current = s.used_color(P::Color)?;
                let mut split = f.0.len();
                for (i, f) in f.0.iter().enumerate() {
                    if let crate::css::properties::Filter::DropShadowComputed(d) = f {
                        if shadow.is_some() {
                            self.note("filter with more than one drop-shadow(): the first drawn", id);
                            break;
                        }
                        let constant = |n: &crate::css::properties::Numeric| n.expression().constant().map_or(0., |q| q.value) as f32;
                        let color = d.color.as_ref().map_or(current, |c| c.used(current)).srgb8_clipped();
                        shadow = Some(DropShadow { dx: constant(&d.x), dy: constant(&d.y), sigma: constant(&d.blur).clamp(0., 1000.), color });
                        split = i;
                    }
                }
                // The colour functions after the shadow colour it too.
                after = crate::css::properties::color_matrix(&f.0[split.min(f.0.len())..]);
                let sigma2: f64 = f.0.iter().map(|f| if let crate::css::properties::Filter::Blur(n) = f { n.expression().constant().map_or(0., |q| q.value.max(0.)).powi(2) } else { 0. }).sum();
                (crate::css::properties::color_matrix(&f.0[..split]), sigma2.sqrt().min(1000.) as f32)
            }
            _ => (None, 0.),
        };
        // The filter acts in the box's own space, its transform
        // after (Filter Effects 1 §3): a blur widened by the
        // matrix's scale (Reddit's backdrops are scale(1.2)
        // blur(24px), 28.8 px on screen), a shadow's offset
        // turned with it.
        let (blur, shadow) = if transformed == NONE {
            (blur, shadow)
        } else {
            let m = self.transforms[transformed as usize];
            let k = (m[0] * m[3] - m[1] * m[2]).abs().sqrt();
            let k = if k.is_finite() { k as f32 } else { 1. };
            let shadow = shadow.map(|d| DropShadow {
                dx: (m[0] * f64::from(d.dx) + m[2] * f64::from(d.dy)) as f32,
                dy: (m[1] * f64::from(d.dx) + m[3] * f64::from(d.dy)) as f32,
                sigma: (d.sigma * k).min(1000.),
                ..d
            });
            ((blur * k).min(1000.), shadow)
        };
        // mix-blend-mode: the box and its content composite onto
        // what is under it through the mode, as one group.
        let blend = match s.value(P::MixBlendMode) {
            Value::Keyword(k) => [
                K::Normal,
                K::Multiply,
                K::Screen,
                K::Overlay,
                K::Darken,
                K::Lighten,
                K::ColorDodge,
                K::ColorBurn,
                K::HardLight,
                K::SoftLight,
                K::Difference,
                K::Exclusion,
                K::Hue,
                K::Saturation,
                K::ColorBlend,
                K::Luminosity,
                K::PlusLighter,
            ]
            .iter()
            .position(|m| m == k)
            .unwrap_or(0) as u8,
            _ => 0,
        };
        if opacity < 1. || mask.is_some() || filter.is_some() || blur > 0. || blend != 0 || shadow.is_some() || after.is_some() {
            if self.groups.len() >= self.limits.commands {
                return Err(Error::Limit);
            }
            let index = self.groups.len() as u32;
            let parent_reach = if parent_group == NONE { 0. } else { self.groups[parent_group as usize].reach };
            // (A shadow reaches as far as it is moved and blurred.)
            let shadow_reach = shadow.map_or(0., |d| d.dx.abs().max(d.dy.abs()) + 3. * d.sigma);
            self.groups.push(Group { parent: parent_group, opacity: opacity as f32, mask, filter, blur, reach: parent_reach + (3. * blur).max(shadow_reach), blend, shadow, after, clip });
            if mask.is_none() && filter.is_none() && blur == 0. && blend == 0 && shadow.is_none() && after.is_none() {
                self.group_owners.push((id, index));
            }
            return Ok(Some(index));
        }
        Ok(None)
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
        if !matches!(s.value(P::Isolation), Value::Keyword(K::Auto)) || !matches!(s.value(P::ZIndex), Value::Keyword(K::Auto)) && s.value(P::ZIndex).scalar() != Some(0.) {
            self.note("z-index on an atomic inline ignored", id);
        }
        let g = *layout.geometry(id).ok_or(Error::Invalid)?;
        let outer = Rect { x: at.x + g.border_box.x, y: at.y + g.border_box.y, ..g.border_box };
        // Its transform, opacity, filter, mask and blend, as a block's:
        // painted where it is in the line (not raised to the stacking
        // context's later layers, as Chrome paints a translucent or
        // transformed inline: the same unless something overlaps it).
        let opacity = s.value(P::Opacity).scalar().unwrap_or(1.).clamp(0., 1.);
        if opacity == 0. {
            return Ok(());
        }
        let (parent_transform, parent_group) = (self.current_transform, self.current_group);
        let transformed = self.transform_of(id, s, outer, parent_transform)?;
        self.current_transform = transformed;
        let painted = (|| {
            if let Some(index) = self.group_of(id, s, outer, matches!(primitive, Some(Primitive::Image { .. })), opacity, transformed, parent_group, clip)? {
                self.current_group = index;
            }
            if visible(s) {
                self.decorate(id, s, g, outer, group, clip, true)?;
            }
            match primitive {
                Some(Primitive::Vector { vector, .. }) => self.vector(id, vector, outer, group, clip),
                Some(primitive) => self.emit(group, true, outer, primitive, clip),
                None => Ok(()),
            }
        })();
        self.current_transform = parent_transform;
        self.current_group = parent_group;
        painted
    }
    /// Keep the raster transform anchored to the CSS viewport while the spatial
    /// index includes visible ink outside it. No path traversal during repaint.
    fn vector(&mut self, id: BoxId, vector: u32, outer: Rect, group: (u32, u8), clip: u32) -> Result<()> {
        let g = self.layout.geometry(id).ok_or(Error::Invalid)?;
        let origin = Point { x: outer.x + g.border_width.left + g.padding.left, y: outer.y + g.border_width.top + g.padding.top };
        let content = g.content_box();
        let source = self.layout.cache().tree().vector(vector).ok_or(Error::Invalid)?;
        if (source.rectangles != 0 || !source.texts.is_empty()) && !self.vector_geometry.contains_key(&vector) {
            let viewport = source.view_box.map_or([content.width, content.height], |v| [v[2], v[3]]);
            let texts = self.layout.cache().svg_texts(vector);
            let used = crate::svg::used::Geometry::resolve(source, viewport, texts, self.limits.path_segments, self.limits.path_bytes.saturating_sub(self.vector_clip_bytes), self.cancel)?;
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
        let inline_boxes = layout.cache().tree().inline_boxes(source).ok_or(Error::Invalid)?;
        let source = layout.cache().tree().text_source(source).ok_or(Error::Invalid)?;
        let prepared = text.text().prepared();
        // Inline boxes with a background colour (`<code>`, `<mark>`, a
        // highlighted span): each line's piece of their content area, grown
        // by their padding (horizontally at their first and last piece),
        // painted under their glyphs (CSS 2.2 §10.6.1, Appendix E step 7).
        let to_output = |byte: u32| -> u32 {
            match prepared.mapping_at_source(byte) {
                Some(m) if m.kind != crate::text::prepare::MappingKind::Removed => m.output.start + (byte - m.input.start).min(m.output.end - m.output.start),
                Some(m) => m.output.start,
                None => prepared.text().len() as u32,
            }
        };
        // Inline elements' `opacity` (output ranges and opacities, those
        // below 1): what they paint (glyphs, decorations, backgrounds) at
        // the product of the opacities around it, as their groups would
        // composite it where it doesn't overlap itself (apple.com swaps
        // words by fading one out over the other).
        let faded: Vec<(Range<u32>, f64)> = inline_boxes
            .iter()
            .filter_map(|b| {
                let o = layout.cache().tree().styles().snapshot(b.node)?.value(P::Opacity).scalar().unwrap_or(1.).clamp(0., 1.);
                (o < 1.).then(|| (to_output(b.range.start)..to_output(b.range.end), o))
            })
            .collect();
        let opacity_at = |at: u32| -> f64 { faded.iter().filter(|(r, _)| r.contains(&at)).map(|(_, o)| *o).product() };
        let fade = |c: [u8; 4], o: f64| if o < 1. { [c[0], c[1], c[2], (f64::from(c[3]) * o).round() as u8] } else { c };
        // (An inline's border: each side's width and colour, drawn on each
        // line's piece, its left and right only where it starts and ends.)
        type InlineBorder = ([f64; 4], [[u8; 4]; 4]);
        let mut backgrounds: Vec<(Range<u32>, [u8; 4], [f64; 4], bool, bool, crate::dom::NodeId, Option<InlineBorder>)> = Vec::new();
        for b in inline_boxes {
            check(self.cancel)?;
            let s = layout.cache().tree().styles().snapshot(b.node).ok_or(Error::Stale)?;
            let c = fade(color(s, P::BackgroundColor)?, opacity_at(to_output(b.range.start)));
            let widths = [P::BorderTopWidth, P::BorderRightWidth, P::BorderBottomWidth, P::BorderLeftWidth].map(|p| s.used_border_width(p).unwrap_or(0.));
            let border = if widths.iter().any(|w| *w > 0.) {
                let mut colors = [[0u8; 4]; 4];
                for (i, p) in [P::BorderTopColor, P::BorderRightColor, P::BorderBottomColor, P::BorderLeftColor].into_iter().enumerate() {
                    colors[i] = fade(color(s, p)?, opacity_at(to_output(b.range.start)));
                }
                if [P::BorderTopStyle, P::BorderRightStyle, P::BorderBottomStyle, P::BorderLeftStyle].iter().any(|p| !matches!(s.value(*p), Value::Keyword(K::Solid | K::None | K::Hidden))) {
                    self.fallbacks.note("inline border style painted solid", Some(b.node));
                }
                Some((widths, colors))
            } else {
                None
            };
            if !visible(s) || (c[3] == 0 && border.is_none()) {
                continue;
            }
            if matches!(s.value(P::BackgroundImage), Value::Images(_)) {
                self.fallbacks.note("inline background image not painted", Some(b.node));
            }
            if decoration::radii(s, Rect { x: 0., y: 0., width: 100., height: 100. }).iter().any(|r| (r[0] - r[1]).abs() > 0.01) {
                self.fallbacks.note("elliptical inline background corners drawn circular", Some(b.node));
            }
            let pad = |p| s.value(p).scalar().unwrap_or(0.).max(0.);
            let range = to_output(b.range.start)..to_output(b.range.end);
            if range.is_empty() {
                if pad(P::PaddingLeft) + pad(P::PaddingRight) > 0. {
                    self.fallbacks.note("empty inline background not painted", Some(b.node));
                }
                continue;
            }
            // Its edges lie before its first glyph and after its last by its
            // padding and by the margins and paddings of the boxes in it
            // that start (or end) there too.
            let (mut lead, mut trail) = (pad(P::PaddingLeft), pad(P::PaddingRight));
            for d in inline_boxes {
                if std::ptr::eq(d, b) || d.range.start < b.range.start || d.range.end > b.range.end {
                    continue;
                }
                let Some(ds) = layout.cache().tree().styles().snapshot(d.node) else { continue };
                let edge = |p| ds.value(p).scalar().unwrap_or(0.);
                if d.range.start == b.range.start && d.first {
                    lead += edge(P::MarginLeft) + edge(P::PaddingLeft).max(0.) + ds.used_border_width(P::BorderLeftWidth).unwrap_or(0.);
                }
                if d.range.end == b.range.end && d.last {
                    trail += edge(P::MarginRight) + edge(P::PaddingRight).max(0.) + ds.used_border_width(P::BorderRightWidth).unwrap_or(0.);
                }
            }
            backgrounds.push((range, c, [pad(P::PaddingTop), trail, pad(P::PaddingBottom), lead], b.first, b.last, b.node, border));
        }
        // Styles are immutable for this build. Consecutive glyphs usually
        // belong to the same span: resolve/convert its paint color once.
        let mut paint_color: Option<(u32, [u8; 4], Option<Deco>, bool)> = None;
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

        // `text-overflow: ellipsis` on the block holding this text, when it
        // clips: a line running past its end loses the glyphs that would
        // cross "…", which goes there in the block's colour (CSS Overflow 3
        // §3.1; GitHub's file list's commit messages).
        let ellipsis = {
            let parent = layout.cache().tree().get(id).and_then(|b| b.parent).and_then(|p| layout.cache().tree().style(p));
            match parent {
                Some(ps)
                    if matches!(ps.value(P::TextOverflow), Value::Keyword(K::Ellipsis))
                        && !matches!(ps.value(P::OverflowX), Value::Keyword(K::Visible))
                        && ps.axes().direction != crate::css::style::Direction::Rtl =>
                {
                    Some((at.x + layout.geometry(id).ok_or(Error::Invalid)?.border_box.width, color(ps, P::Color)?))
                }
                _ => None,
            }
        };
        for (li, line) in text.lines().iter().enumerate() {
            check(self.cancel)?;
            // Where this line's glyphs stop for its ellipsis: the "…" of the
            // font of its first run (fragment, glyph, advance), and the x no
            // kept glyph may pass.
            let mut cut: Option<(u32, u16, usize, f64, f64, [u8; 4])> = None;
            if let Some((right, ec)) = ellipsis {
                let end = line
                    .fragments
                    .clone()
                    .map(|fi| {
                        let f = &text.fragments()[fi as usize];
                        at.x + f.x + f.advance
                    })
                    .fold(f64::NEG_INFINITY, f64::max);
                if end > right + 0.5
                    && let Some(fi) = line.fragments.clone().find(|&fi| !text.fragments()[fi as usize].glyphs.is_empty())
                    && let Some(run) = text.run(&text.fragments()[fi as usize])
                    && let Ok(face) = run.font.selection().parsed()
                    // A font without "…" gives three periods, as browsers do.
                    && let Some((gid, count)) = face.glyph_index('\u{2026}').map(|g| (g, 1)).or_else(|| face.glyph_index('.').map(|g| (g, 3)))
                {
                    let advance = f64::from(face.glyph_hor_advance(gid).unwrap_or(0)) * run.size / f64::from(run.font.units_per_em());
                    cut = Some((fi, gid.0, count, advance, right - advance * count as f64, ec));
                }
            }
            let mut cut_x: Option<f64> = None;
            // Native text has subpixel x placement and a device-pixel baseline.
            // Retain the painted baseline in the scene so its index and agent
            // bounds agree with the actual glyph placement.
            let scale = layout.device_scale();
            let baseline = ((at.y + text.baseline(li).ok_or(Error::Invalid)?) * scale).round() / scale;
            for (range, c, pad, first, last, node, border) in &backgrounds {
                let (mut x0, mut x1, mut ascent, mut descent) = (f64::INFINITY, f64::NEG_INFINITY, 0f64, 0f64);
                let (mut starts, mut ends) = (false, false);
                for fi in line.fragments.clone() {
                    let f = &text.fragments()[fi as usize];
                    if f.source.end <= range.start || f.source.start >= range.end {
                        continue;
                    }
                    let run = text.run(f).ok_or(Error::Invalid)?;
                    let factor = run.size / f64::from(run.font.units_per_em());
                    let mut pen = 0i64;
                    for gi in f.glyphs.clone() {
                        let g = run.glyphs[gi as usize];
                        let gx = at.x + f.x + pen as f64 * factor;
                        pen += i64::from(g.x_advance);
                        if range.contains(&g.cluster) {
                            x0 = x0.min(gx);
                            x1 = x1.max(at.x + f.x + pen as f64 * factor);
                        }
                    }
                    // (Its font's content area on the device grid, as Chrome
                    // paints an inline's background and border.)
                    let (a, d) = text.content_area(f);
                    ascent = ascent.max(a + text.shift(f));
                    descent = descent.max(d - text.shift(f));
                    starts |= f.source.start <= range.start;
                    ends |= f.source.end >= range.end;
                }
                if x0 < x1 {
                    if starts && *first {
                        x0 -= pad[3];
                    }
                    if ends && *last {
                        x1 += pad[1];
                    }
                    let snap = |v: f64| (v * scale).round() / scale;
                    let (y0, y1) = (snap(baseline - ascent - pad[0]), snap(baseline + descent + pad[2]));
                    let piece = Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 };
                    // Its corners rounded where the box starts and ends
                    // (`box-decoration-break: slice`: the pieces between
                    // lines are square), by its radii against this piece.
                    let mut radii = [0f32; 4];
                    if let Some(s) = layout.cache().tree().styles().snapshot(*node) {
                        let r = decoration::radii(s, piece);
                        let circular = |i: usize| r[i][0].min(r[i][1]).max(0.) as f32;
                        if starts && *first {
                            (radii[0], radii[3]) = (circular(0), circular(3));
                        }
                        if ends && *last {
                            (radii[1], radii[2]) = (circular(1), circular(2));
                        }
                    }
                    if c[3] != 0 {
                        self.emit(group, true, piece, Primitive::Fill { color: *c, owner: id, radii }, clip)?;
                    }
                    if let Some((w, colors)) = border {
                        let (left, right) = (if starts && *first { w[3] } else { 0. }, if ends && *last { w[1] } else { 0. });
                        let outer = Rect { x: piece.x - left, y: piece.y - w[0], width: piece.width + left + right, height: piece.height + w[0] + w[2] };
                        let sides = [
                            Rect { x: outer.x, y: outer.y, width: outer.width, height: w[0] },
                            Rect { x: outer.x + outer.width - right, y: outer.y, width: right, height: outer.height },
                            Rect { x: outer.x, y: outer.y + outer.height - w[2], width: outer.width, height: w[2] },
                            Rect { x: outer.x, y: outer.y, width: left, height: outer.height },
                        ];
                        for (side, color) in sides.into_iter().zip(colors) {
                            if side.width > 0. && side.height > 0. && color[3] != 0 {
                                self.emit(group, true, side, Primitive::Fill { color: *color, owner: id, radii: [0.; 4] }, clip)?;
                            }
                        }
                    }
                }
            }
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
                let mut segment: Option<(u32, Point, [u8; 4], Rect, u32, Option<Deco>)> = None;
                for gi in f.glyphs.clone() {
                    check(self.cancel)?;
                    self.progress.glyphs += 1;
                    if self.progress.glyphs > self.limits.glyph_work {
                        return Err(Error::Limit);
                    }
                    let g = run.glyphs[gi as usize];
                    if let Some((_, _, _, _, limit, _)) = cut {
                        let left = at.x + f.x + pen as f64 * factor;
                        if cut_x.is_some() || left + f64::from(g.x_advance) * factor > limit + 0.01 {
                            if let Some((start, start_at, old, b, _, old_deco)) = segment.take() {
                                self.segment(group, clip, id, fi, start..gi, start_at, old, b, old_deco, left, baseline, &run.font, run.size)?;
                            }
                            cut_x.get_or_insert(left.min(limit));
                            pen += i64::from(g.x_advance);
                            continue;
                        }
                    }
                    let mapping = prepared.mapping_at_output(g.cluster).ok_or(Error::Invalid)?;
                    let span = source.spans.get(mapping.span as usize).ok_or(Error::Invalid)?;
                    let (c, deco, shown) = match paint_color {
                        Some((owner, color, deco, shown)) if owner == mapping.span => (color, deco, shown),
                        _ => {
                            let s = layout.cache().tree().styles().snapshot(span.style_node).ok_or(Error::Stale)?;
                            let opacity = opacity_at(g.cluster);
                            let mut c = fade(color(s, P::Color)?, opacity);
                            if !visible(s) {
                                c[3] = 0
                            }
                            // A field's placeholder at its `::placeholder` opacity
                            // (GitHub's shows only while the field has the focus).
                            if span.style_node.pseudo == Some(crate::css::selectors::PseudoElement::Placeholder) {
                                let o = s.value(P::Opacity).scalar().unwrap_or(1.).clamp(0., 1.);
                                c[3] = (f64::from(c[3]) * o).round() as u8;
                            }
                            // text-decoration-line, in text-decoration-color, its
                            // style, thickness and (an underline's) offset.
                            let deco = match s.value(P::TextDecorationLine) {
                                Value::Keyword(k @ (K::Underline | K::Overline | K::LineThrough)) if c[3] != 0 => {
                                    Some(Deco::of(s, *k, fade(s.used_color(P::TextDecorationColor)?.srgb8_clipped(), opacity)))
                                }
                                _ => None,
                            };
                            // Under `background-clip: text`, transparent
                            // glyphs still show the background through them.
                            let shown = c[3] != 0 || self.current_text_fill != NONE && visible(s);
                            paint_color = Some((mapping.span, c, deco, shown));
                            (c, deco, shown)
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
                    let input_end = if project_end.kind != crate::text::prepare::MappingKind::Removed { project_end.input.start + (end - project_end.output.start) } else { project_end.input.end };
                    let input_start = if mapping.kind != crate::text::prepare::MappingKind::Removed { mapping.input.start + (g.cluster - mapping.output.start) } else { mapping.input.start };
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
                    if segment.is_none() && (bounds.is_some() || deco.is_some()) && shown {
                        segment = Some((gi, origin, c, bounds.unwrap_or(Rect { x: origin.x, y: baseline, width: 0., height: 0. }), hit_span, deco));
                    }
                }
                if let Some((start, origin, c, b, _, deco)) = segment {
                    let end_x = at.x + f.x + pen as f64 * factor;
                    self.segment(group, clip, id, fi, start..f.glyphs.end, origin, c, b, deco, end_x, baseline, &run.font, run.size)?;
                }
            }
            if let (Some((fi, glyph, count, advance, _, ec)), Some(x)) = (cut, cut_x)
                && ec[3] != 0
            {
                let run_size = text.run(&text.fragments()[fi as usize]).map_or(16., |r| r.size);
                for k in 0..count {
                    let x = x + advance * k as f64;
                    let bounds = Rect { x, y: baseline - run_size, width: advance, height: run_size * 1.25 };
                    self.emit(group, true, bounds, Primitive::Glyph { owner: id, fragment: fi, glyph, origin: Point { x, y: baseline }, color: ec }, clip)?;
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
        deco: Option<Deco>,
        end_x: f64,
        baseline: f64,
        font: &Arc<crate::text::shaping::GlyphFont>,
        size: f64,
    ) -> Result<()> {
        if let Some(d) = deco
            && d.color[3] != 0
            && end_x > origin.x
        {
            let (position, font_thickness, x_height, ascent, _) = self.line_metrics(font);
            let factor = size / f64::from(font.units_per_em());
            let scale = self.layout.device_scale();
            let thickness = ((d.thickness.unwrap_or(font_thickness * factor) * scale).round() / scale).max(1. / scale);
            let y = match d.line {
                // text-underline-offset from the baseline when given, else the font's.
                K::Underline => match d.offset {
                    Some(o) => baseline + o,
                    None => baseline - position * factor,
                },
                K::LineThrough => baseline - x_height * factor / 2. - thickness / 2.,
                _ => baseline - ascent * factor,
            };
            let y = (y * scale).round() / scale;
            let width = end_x - origin.x;
            let fill = |x: f64, y: f64, w: f64, h: f64, round: bool| {
                (Rect { x, y, width: w, height: h }, Primitive::Fill { color: d.color, owner: id, radii: if round { [(h / 2.) as f32; 4] } else { [0.; 4] } })
            };
            let mut lines: Vec<(Rect, Primitive)> = Vec::new();
            match d.style {
                K::Double => {
                    // Two lines, the gap between them as thick as each.
                    lines.push(fill(origin.x, y, width, thickness, false));
                    lines.push(fill(origin.x, y + 2. * thickness, width, thickness, false));
                }
                K::Dotted => {
                    // Round dots the thickness across, two apart.
                    let step = 2. * thickness;
                    let mut x = origin.x;
                    while x + thickness <= end_x + 0.01 && lines.len() < 2048 {
                        lines.push(fill(x, y, thickness, thickness, true));
                        x += step;
                    }
                }
                K::Dashed => {
                    // Dashes three thick long, two apart.
                    let (dash, gap) = (3. * thickness, 2. * thickness);
                    let mut x = origin.x;
                    while x < end_x && lines.len() < 2048 {
                        lines.push(fill(x, y, dash.min(end_x - x), thickness, false));
                        x += dash + gap;
                    }
                }
                K::Wavy => {
                    // A wave about the line: a 9-thickness wavelength, rising
                    // two thicknesses each way (Chrome's at 1px: 9 and 4).
                    let amplitude = (2. * thickness).max(1.5);
                    // (Hanging from the line, as Chrome draws it: its crests at
                    // the underline's position.)
                    let bounds = Rect { x: origin.x, y: y - thickness / 2., width, height: 2. * (amplitude + thickness) };
                    lines.push((bounds, Primitive::Wave { owner: id, color: d.color, thickness: thickness as f32, wavelength: (9. * thickness).max(6.) as f32 }));
                }
                _ => lines.push(fill(origin.x, y, width, thickness, false)),
            }
            for (bounds, primitive) in lines {
                self.emit(group, false, bounds, primitive, clip)?;
            }
        }
        if bounds.width > 0. && bounds.height > 0. {
            self.emit(group, true, bounds, Primitive::Text { owner: id, fragment, glyphs, origin, color, fill: self.current_text_fill }, clip)?;
        }
        Ok(())
    }
    /// A font's underline position (above the baseline, usually negative)
    /// and thickness, x-height and ascent, in font units, kept per font.
    fn line_metrics(&mut self, font: &Arc<crate::text::shaping::GlyphFont>) -> (f64, f64, f64, f64, f64) {
        let key = Arc::as_ptr(font) as usize;
        if let Some(m) = self.font_lines.get(&key) {
            return *m;
        }
        let upem = f64::from(font.units_per_em());
        let m = match font.selection().parsed() {
            Ok(face) => {
                let (position, thickness) = face.underline_metrics().map_or((-upem / 10., upem / 14.), |u| (f64::from(u.position), f64::from(u.thickness).max(1.)));
                let x_height = face.x_height().map_or(upem / 2., f64::from);
                (position, thickness, x_height, f64::from(face.ascender()), -f64::from(face.descender()))
            }
            Err(_) => (-upem / 10., upem / 14., upem / 2., upem * 0.8, upem * 0.2),
        };
        if self.font_lines.len() < 256 {
            self.font_lines.insert(key, m);
        }
        m
    }
    /// Nodes whose `transform` animates (see `LayerKind::Moved`).
    pub fn with_animated(mut self, nodes: std::collections::HashSet<crate::dom::NodeId>) -> Self {
        self.animated = nodes;
        self
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
            let (mut parent, mut context, mut clip, parent_group, parent_transform, parent_hidden, parent_fixed, parent_sticky, parent_moved, mut parent_scroller, mut parent_shift) =
                if let Some(parent) = b.parent {
                    let p = &self.states[parent.index()];
                    (p.origin, p.context, p.clip, p.group, p.transform, p.hidden, p.fixed, p.sticky, p.moved, p.scroller, p.shift)
                } else {
                    (Point::default(), 0, NONE, NONE, NONE, false, false, None, None, NONE, Point::default())
                };
            let parent_backdrop = b.parent.and_then(|p| self.states[p.index()].backdrop);
            if parent_hidden {
                self.states.push(State {
                    origin: parent,
                    context,
                    clip,
                    group: parent_group,
                    transform: parent_transform,
                    hidden: true,
                    fixed: parent_fixed,
                    sticky: parent_sticky,
                    moved: parent_moved,
                    backdrop: parent_backdrop,
                    scroller: parent_scroller,
                    shift: parent_shift,
                    text_fill: NONE,
                });
                self.progress.boxes += 1;
                continue;
            }
            self.current_fixed = parent_fixed;
            let mut children_fixed = parent_fixed;
            self.current_sticky = parent_sticky;
            let mut children_sticky = parent_sticky;
            self.current_group = parent_group;
            let mut children_group = parent_group;
            self.current_transform = parent_transform;
            let mut children_transform = parent_transform;
            self.current_moved = parent_moved;
            let mut children_moved = parent_moved;
            self.current_backdrop = parent_backdrop;
            let mut children_backdrop = parent_backdrop;
            let parent_text_fill = b.parent.map_or(NONE, |p| self.states[p.index()].text_fill);
            self.current_text_fill = parent_text_fill;
            let mut children_text_fill = parent_text_fill;
            // In-flow content paints in slot 2 of its context; a box that
            // starts a context paints its own decoration in slot 0 of the new one.
            let mut group = (context, 2);
            if b.absolute {
                let mut cb = b.parent.ok_or(Error::Invalid)?;
                loop {
                    check(self.cancel)?;
                    if cb == tree.root() || !matches!(tree.style(cb).ok_or(Error::Invalid)?.value(P::Position), Value::Keyword(K::Static)) || parent_moved == Some(cb) {
                        break;
                    }
                    cb = tree.get(cb).ok_or(Error::Invalid)?.parent.ok_or(Error::Invalid)?;
                }
                clip = self.states[cb.index()].clip;
                // Positioned against a block outside the scroll containers
                // around it (a fixed box: the viewport's): it doesn't move
                // with them, nor count in what they hold.
                let fixed_here = parent_transform == NONE && tree.style(id).is_some_and(|s| matches!(s.value(P::Position), Value::Keyword(K::Fixed)));
                let (scroller, shift) = if fixed_here { (NONE, Point::default()) } else { (self.states[cb.index()].scroller, self.states[cb.index()].shift) };
                if scroller != parent_scroller || shift.x != parent_shift.x || shift.y != parent_shift.y {
                    parent.x += parent_shift.x - shift.x;
                    parent.y += parent_shift.y - shift.y;
                    parent_scroller = scroller;
                    parent_shift = shift;
                }
            }
            let g = *layout.geometry(id).ok_or(Error::Invalid)?;
            let mut outer = g.border_box;
            outer.x += parent.x;
            outer.y += parent.y;
            if !valid(outer) {
                return Err(Error::Invalid);
            }
            let local = g.content_box();
            let mut origin = Point { x: parent.x + local.x, y: parent.y + local.y };
            let mut children_clip = clip;
            // What a scroll container holds reaches as far as its boxes do
            // (a nested scroll container's own box, not what it holds).
            let mut children_scroller = parent_scroller;
            let mut children_shift = parent_shift;
            if parent_scroller != NONE {
                let sc = &mut self.scrollers[parent_scroller as usize];
                // Margin boxes, then the container's end padding (as Chrome).
                sc.content[0] = sc.content[0].max(outer.x + outer.width + g.margin.right.max(0.) + sc.pad[0] + sc.offset[0] - sc.rect.x);
                sc.content[1] = sc.content[1].max(outer.y + outer.height + g.margin.bottom.max(0.) + sc.pad[1] + sc.offset[1] - sc.rect.y);
            }
            // The page's own scrollable overflow (CSS Overflow 3 §2.2): a box
            // no ancestor clips, not fixed, reaches it (its margin box's end)
            // whether it paints or not.
            if parent_scroller == NONE
                && clip == NONE
                && !parent_fixed
                && !matches!(b.kind, BoxKind::Viewport)
                && !tree.style(id).is_some_and(|s| matches!(s.value(P::Position), Value::Keyword(K::Fixed)))
            {
                let reach = Rect { width: outer.width + g.margin.right.max(0.), height: outer.height + g.margin.bottom.max(0.), ..outer };
                self.layout_extent = if self.layout_extent.width <= 0. && self.layout_extent.height <= 0. { reach } else { union(self.layout_extent, reach) };
            }
            match b.kind {
                BoxKind::Viewport => {}
                BoxKind::Block { .. } | BoxKind::BlockImage { .. } | BoxKind::BlockVector { .. } | BoxKind::BlockReplaced { .. } => {
                    let s = tree.style(id).ok_or(Error::Invalid)?;
                    // Fixed to the viewport (unless a transformed ancestor is
                    // its containing block: then it scrolls with that): its
                    // subtree paints over the scrolled page.
                    if !parent_fixed && parent_transform == NONE && matches!(s.value(P::Position), Value::Keyword(K::Fixed)) {
                        self.current_fixed = true;
                        children_fixed = true;
                    }
                    // Sticky: its subtree paints apart, moved by the scroll
                    // (a sticky box under another follows the outer one).
                    if !parent_fixed && parent_sticky.is_none() && matches!(s.value(P::Position), Value::Keyword(K::Sticky)) {
                        self.current_sticky = Some(id);
                        children_sticky = Some(id);
                    }
                    // A transformed box paints its subtree through its matrix
                    // and is a stacking context (CSS Transforms 1 §3).
                    // An animated box with a transform, on the page itself (no
                    // fixed or sticky box, matrix, group or other layer above
                    // it, its ancestors' clip a plain rectangle): a moved
                    // layer, painted in its own space once and shown through
                    // its matrix (moved by whole pixels, or rotated and
                    // scaled on the GPU, as Chrome composites a spinner).
                    let mut layered = false;
                    if parent_moved.is_none()
                        && !parent_fixed
                        && parent_sticky.is_none()
                        && parent_transform == NONE
                        && parent_group == NONE
                        && self.animated.contains(&b.node)
                        && let Some(about) = about_of(s, outer).filter(|a| invertible(*a))
                        && (clip == NONE || self.clips[clip as usize].radii.is_none() && self.clips[clip as usize].transform == NONE)
                    {
                        self.moved.push(Moved { root: id, matrix: about, clip: (clip != NONE).then(|| self.clips[clip as usize].rect), outer });
                        self.current_moved = Some(id);
                        children_moved = Some(id);
                        // Its content is clipped only within it; the window
                        // shows the layer within its ancestors' clip.
                        clip = NONE;
                        children_clip = NONE;
                        layered = true;
                    }
                    // A box with a backdrop-filter on the page itself (not
                    // fixed, sticky, moved, in a group or another backdrop's
                    // layer): it and its subtree in a layer of their own, so
                    // what is under it shows blurred through it.
                    if parent_backdrop.is_none()
                        && parent_moved.is_none()
                        && children_moved.is_none()
                        && !children_fixed
                        && children_sticky.is_none()
                        && parent_group == NONE
                        && let Value::Filters(f) = s.value(P::BackdropFilter)
                        && f.0.iter().any(|f| match f {
                            crate::css::properties::Filter::Blur(n) => n.expression().constant().is_some_and(|q| q.value > 0.),
                            crate::css::properties::Filter::Saturate(v) => (v - 1.).abs() > 1e-6,
                            _ => false,
                        })
                    {
                        self.current_backdrop = Some(id);
                        children_backdrop = Some(id);
                    }
                    let transformed = if layered { parent_transform } else { self.transform_of(id, s, outer, parent_transform)? };
                    if transformed != parent_transform {
                        // OLO_TRANSFORM_DEBUG: which boxes paint through a matrix.
                        if crate::env_on!("OLO_TRANSFORM_DEBUG") {
                            eprintln!(
                                "  transform on <{} class={:?}>: {:?}",
                                self.doc.name(b.node).ok().flatten().unwrap_or("?"),
                                self.doc.attribute(b.node, "class").ok().flatten().unwrap_or("").chars().take(60).collect::<String>(),
                                self.transforms[transformed as usize]
                            );
                        }
                        self.current_transform = transformed;
                        children_transform = transformed;
                    }
                    let opacity = s.value(P::Opacity).scalar().unwrap_or(1.).clamp(0., 1.);
                    // Fully transparent: nothing of it or its content paints
                    // (a fallback picture under a canvas that drew, say).
                    if opacity == 0. {
                        self.states.push(State {
                            origin,
                            context,
                            clip: children_clip,
                            group: children_group,
                            transform: children_transform,
                            hidden: true,
                            fixed: children_fixed,
                            sticky: children_sticky,
                            moved: children_moved,
                            backdrop: children_backdrop,
                            scroller: children_scroller,
                            shift: children_shift,
                            text_fill: NONE,
                        });
                        self.progress.boxes += 1;
                        continue;
                    }
                    // mask-image: the box and its content composite through
                    // the mask's alpha, as one group.
                    // A mask that changes nothing (see `mask_is_opaque`) where
                    // nothing of the box paints outside its border box: a
                    // replaced element, or a box clipping its overflow, with
                    // no outer shadow. No group, nothing offscreen.
                    let inside_border =
                        matches!(b.kind, BoxKind::BlockImage { .. }) || !matches!(s.value(P::OverflowX), Value::Keyword(K::Visible)) && !matches!(s.value(P::OverflowY), Value::Keyword(K::Visible));
                    let mut masked = false;
                    if let Some(index) = self.group_of(id, s, outer, inside_border, opacity, transformed, parent_group, clip)? {
                        self.current_group = index;
                        children_group = index;
                        masked = self.groups[index as usize].mask.is_some();
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
                    // (A filter other than none makes one, as it would whatever
                    // it does to colours.)
                    let filtered = !matches!(s.value(P::Filter), Value::Keyword(K::None))
                        || !matches!(s.value(P::BackdropFilter), Value::Keyword(K::None))
                        || !matches!(s.value(P::MixBlendMode), Value::Keyword(K::Normal));
                    let stacking = z.is_some() || opacity < 1. || isolated || transformed != parent_transform || layered || masked || filtered;
                    if positioned || stacking {
                        if self.contexts.len() >= self.limits.commands {
                            return Err(Error::Limit);
                        }
                        let index = self.contexts.len() as u32;
                        let mut parent_context = context;
                        while self.contexts[parent_context as usize].pseudo {
                            parent_context = self.contexts[parent_context as usize].parent;
                        }
                        self.contexts.push(Context { parent: parent_context, z: z.unwrap_or(0), path: Vec::new(), pseudo: !stacking });
                        context = index;
                        group = (context, 0);
                    }
                    // clip-path: inset(): the box and its subtree paint within
                    // the reference box inset by the offsets (CSS Masking 1 §6).
                    let clip = if let Value::Inset(inset) = s.value(P::ClipPath) {
                        let reference = match inset.reference_box() {
                            crate::css::properties::GeometryBox::Padding => Rect {
                                x: outer.x + g.border_width.left,
                                y: outer.y + g.border_width.top,
                                width: (outer.width - g.border_width.horizontal()).max(0.),
                                height: (outer.height - g.border_width.vertical()).max(0.),
                            },
                            crate::css::properties::GeometryBox::Content => g.content_box(),
                            crate::css::properties::GeometryBox::Margin => Rect {
                                x: outer.x - g.margin.left,
                                y: outer.y - g.margin.top,
                                width: (outer.width + g.margin.horizontal()).max(0.),
                                height: (outer.height + g.margin.vertical()).max(0.),
                            },
                            _ => outer,
                        };
                        let [top, right, bottom, left] = inset.offsets();
                        let (top, bottom) = (decoration::resolve_any(top, reference.height), decoration::resolve_any(bottom, reference.height));
                        let (left, right) = (decoration::resolve_any(left, reference.width), decoration::resolve_any(right, reference.width));
                        let rect = Rect { x: reference.x + left, y: reference.y + top, width: (reference.width - left - right).max(0.), height: (reference.height - top - bottom).max(0.) };
                        let old = if clip == NONE { Rect { x: -1e30, y: -1e30, width: 2e30, height: 2e30 } } else { self.clips[clip as usize].rect };
                        if self.clips.len() >= self.limits.commands {
                            return Err(Error::Limit);
                        }
                        if inset.radii().is_some() {
                            self.note("clip-path inset corners drawn square", id);
                        }
                        let index = self.clips.len() as u32;
                        self.clips.push(Clip { rect: intersection(old, rect), radii: None, transform: children_transform });
                        index
                    } else {
                        clip
                    };
                    children_clip = clip;
                    // A backdrop layer's box paints at least nothing at its
                    // place, so the layer is there for what is under it to
                    // show through blurred (one with neither background
                    // nor border has nothing else).
                    if self.current_backdrop == Some(id) {
                        self.emit(group, false, outer, Primitive::Fill { color: [0; 4], owner: id, radii: [0.; 4] }, clip)?;
                    }
                    if visible(s) {
                        self.decorate(id, s, g, outer, group, clip, false)?;
                        // Backgrounds clipped to the text: painted with the
                        // glyphs inside (the innermost such box's).
                        if matches!(s.value(P::BackgroundClip), Value::Keyword(K::Text)) && self.text_fills.len() < self.limits.commands {
                            children_text_fill = self.text_fills.len() as u32;
                            self.text_fills.push((id, outer));
                        }
                        if let BoxKind::BlockImage { image } = b.kind {
                            // A live surface (a canvas being animated) goes
                            // in a layer of its own: what is around it is
                            // kept while it changes.
                            self.current_surface = tree.image(image).is_some_and(|i| i.live());
                            let emitted = self.emit(group, true, outer, Primitive::Image { owner: id, image }, clip);
                            self.current_surface = false;
                            emitted?;
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
                        // A scroll container (not `clip`): its content painted
                        // moved by its offset, within its padding box.
                        let scrolls = |p| !matches!(s.value(p), Value::Keyword(K::Visible | K::Clip));
                        // The root's and body's overflow is the viewport's (CSS
                        // Overflow 3 §3.3): the page scrolls, not them.
                        let viewport_owner = matches!(self.doc.name(b.node).ok().flatten(), Some("html" | "body"));
                        if !viewport_owner && (scrolls(P::OverflowX) || scrolls(P::OverflowY)) && self.scrollers.len() < self.limits.commands {
                            let user = |p| matches!(s.value(p), Value::Keyword(K::Auto | K::Scroll));
                            let offset = self.doc.scroll_offset(b.node);
                            children_scroller = self.scrollers.len() as u32;
                            self.scrollers.push(Scroller {
                                root: id,
                                node: b.node,
                                rect: p,
                                fixed: children_fixed,
                                sticky: children_sticky,
                                offset,
                                content: [p.width, p.height],
                                user: [user(P::OverflowX), user(P::OverflowY)],
                                pad: [g.padding.right, g.padding.bottom],
                            });
                            origin.x -= offset[0];
                            origin.y -= offset[1];
                            children_shift = Point { x: parent_shift.x + offset[0], y: parent_shift.y + offset[1] };
                        }
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
            self.states.push(State {
                origin,
                context,
                clip: children_clip,
                group: children_group,
                transform: children_transform,
                hidden: false,
                fixed: children_fixed,
                sticky: children_sticky,
                moved: children_moved,
                backdrop: children_backdrop,
                scroller: children_scroller,
                shift: children_shift,
                text_fill: children_text_fill,
            });
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
        let timing = std::env::var_os("OLO_WEB_TIMING").map(|_| std::time::Instant::now());
        let contexts = &self.contexts;
        let key = |d: &Draft| contexts[d.context as usize].path.iter().copied().chain(std::iter::once(u32::from(d.slot)));
        self.draft.sort_by(|a, b| key(a).cmp(key(b)).then(a.text.cmp(&b.text)));
        let commands: Arc<[Command]> = self.draft.into_iter().map(|d| d.command).collect();
        let sorted_ms = timing.map_or(0., |t| t.elapsed().as_secs_f64() * 1000.);
        // Strata: runs of commands shown the same way, in paint order; the
        // first always a page run, for the base with the background.
        let mut runs: Vec<(LayerKind, Vec<u32>)> = vec![(LayerKind::Page, Vec::new())];
        // How many commands each opacity group holds (its nested groups'
        // too): a surface shows through groups that hold it alone, at
        // their opacity (GitHub's videos fade in and out; painted with the
        // page meanwhile, each picture was made RGB on the processor).
        let mut members = vec![0u32; self.groups.len()];
        for c in commands.iter() {
            let mut g = c.group;
            while g != NONE {
                members[g as usize] += 1;
                g = self.groups[g as usize].parent;
            }
        }
        let alone = |mut g: u32| {
            while g != NONE {
                let group = &self.groups[g as usize];
                if members[g as usize] != 1 || group.mask.is_some() || group.filter.is_some() || group.blur > 0. || group.blend != 0 {
                    return false;
                }
                g = group.parent;
            }
            true
        };
        for i in 0..commands.len() as u32 {
            let c = &commands[i as usize];
            // A surface's image is a layer of its own where it is shown
            // as it is: not under a rotating or skewing transform, an
            // opacity group, a fixed or sticky box, nor a transformed clip.
            // A straight clip cuts it; a rounded one rounds it where the
            // clip is its own rectangle, else it keeps away from the corners.
            // Through a scale and translation (GitHub's hero WebGL canvas,
            // in a scaled box): shown at its transformed place.
            let matrix = (c.transform != NONE).then(|| self.transforms[c.transform as usize]);
            let axis_aligned = matrix.is_none_or(|m| m[1] == 0. && m[2] == 0. && m[0] > 0. && m[3] > 0.);
            let shown = matrix.map_or(c.bounds, |m| aabb(m, c.bounds));
            let surface = c.surface
                && c.moved.is_none()
                && !c.fixed
                && c.sticky.is_none()
                && axis_aligned
                && alone(c.group)
                && (c.clip == NONE || {
                    let clip = &self.clips[c.clip as usize];
                    surface_clip(&self.transforms, clip).is_some_and(|rect| clip.radii.is_none_or(|r| surface_corners(shown, rect, &r).is_some()))
                });
            if c.surface && !surface && crate::env_on!("OLO_SURFACE_DEBUG") {
                let clip = (c.clip != NONE).then(|| (self.clips[c.clip as usize].rect, self.clips[c.clip as usize].transform, self.clips[c.clip as usize].radii.is_some()));
                let groups: Vec<(f32, bool)> = std::iter::successors((c.group != NONE).then_some(c.group), |&g| (self.groups[g as usize].parent != NONE).then(|| self.groups[g as usize].parent))
                    .map(|g| (self.groups[g as usize].opacity, self.groups[g as usize].mask.is_some()))
                    .collect();
                eprintln!("surface refused: bounds {:?} shown {shown:?} matrix {matrix:?} moved {:?} fixed {} sticky {:?} groups {groups:?} clip {clip:?}", c.bounds, c.moved, c.fixed, c.sticky);
            }
            let kind = if let Some(root) = c.moved {
                LayerKind::Moved(root)
            } else if let (true, Primitive::Image { owner, .. }) = (surface, &c.primitive) {
                LayerKind::Surface(*owner)
            } else if c.fixed {
                LayerKind::Fixed
            } else if let Some(root) = c.sticky {
                LayerKind::Sticky(root)
            } else if let Some(root) = c.backdrop {
                LayerKind::Backdrop(root)
            } else {
                LayerKind::Page
            };
            match runs.last_mut() {
                Some((k, ids)) if *k == kind && !matches!(kind, LayerKind::Surface(_)) => ids.push(i),
                _ => runs.push((kind, vec![i])),
            }
        }
        if runs.len() > self.limits.commands {
            return Err(Error::Limit);
        }
        let mut strata = Vec::with_capacity(runs.len());
        for (kind, ids) in runs {
            check(self.cancel)?;
            strata.push(Stratum { kind, index: index::Index::over(&commands, &self.transforms, ids, self.cancel)? });
        }
        if let Some(t) = timing {
            eprintln!(
                "scene finish: sort {sorted_ms:.2} ms, strata {:.2} ms ({} commands, {} contexts, {} strata)",
                t.elapsed().as_secs_f64() * 1000. - sorted_ms,
                commands.len(),
                self.contexts.len(),
                strata.len()
            );
        }
        Ok(Arc::new(Scene {
            extent: self.extent,
            layout_extent: self.layout_extent,
            fallbacks: Arc::new(self.fallbacks),
            layout: self.layout,
            commands,
            commands_serial: COMMAND_SERIALS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            clips: self.clips.into_boxed_slice(),
            groups: self.groups.into_boxed_slice(),
            transforms: self.transforms.into_boxed_slice(),
            transform_entries: self.transform_entries.into_boxed_slice(),
            moved: self.moved.into_boxed_slice(),
            scrollers: self.scrollers.into_boxed_slice(),
            group_owners: self.group_owners.into_boxed_slice(),
            text_fills: self.text_fills.into(),
            vector_geometry: Arc::new(self.vector_geometry),
            vector_clips: Arc::new(self.vector_clips),
            strata: strata.into_boxed_slice(),
            base: self.options.canvas,
            canvas: self.canvas,
        }))
    }
}
