// todo("windows"): remove
#![cfg_attr(windows, allow(dead_code))]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    AtlasTextureId, AtlasTile, Background, Bounds, ContentMask, Corners, Edges, Hsla, Pixels,
    Point, Radians, ScaledPixels, Size, bounds_tree::BoundsTree, point, size,
};
use std::{
    fmt::Debug,
    iter::Peekable,
    ops::{Add, Range, Sub},
    slice,
};

#[allow(non_camel_case_types, unused)]
pub(crate) type PathVertex_ScaledPixels = PathVertex<ScaledPixels>;

pub(crate) type DrawOrder = u32;

#[derive(Default)]
pub(crate) struct Scene {
    pub(crate) paint_operations: Vec<PaintOperation>,
    primitive_bounds: BoundsTree<ScaledPixels>,
    layer_stack: Vec<DrawOrder>,
    pub(crate) shadows: Vec<Shadow>,
    pub(crate) quads: Vec<Quad>,
    pub(crate) paths: Vec<Path<ScaledPixels>>,
    pub(crate) underlines: Vec<Underline>,
    pub(crate) segments: Vec<Segment>,
    pub(crate) sectors: Vec<Sector>,
    /// Sorted by order and texture (`finish`), from `mono_staging`.
    pub(crate) monochrome_sprites: Vec<MonochromeSprite>,
    /// Olo: glyph sprites as painted, in runs of one order and texture
    /// (`mono_runs`): a line's glyphs share its layer's order, so
    /// `finish` sorts a frame's lines, not each of its tens of thousands
    /// of glyphs, and a replayed view copies its ranges from here.
    mono_staging: Vec<MonochromeSprite>,
    /// Olo: how many primitives were left out, clipped away whole (a
    /// paint memo of a row painted out of sight holds none of its text:
    /// it can't be moved into sight, `Window::paint_memo`).
    pub(crate) culled: usize,
    mono_runs: Vec<SpriteRun>,
    pub(crate) polychrome_sprites: Vec<PolychromeSprite>,
    pub(crate) image_triangles: Vec<ImageTriangle>,
    pub(crate) surfaces: Vec<PaintSurface>,
    pub(crate) backdrops: Vec<Backdrop>,
}

impl Scene {
    pub fn clear(&mut self) {
        self.paint_operations.clear();
        self.primitive_bounds.clear();
        self.layer_stack.clear();
        self.paths.clear();
        self.shadows.clear();
        self.quads.clear();
        self.underlines.clear();
        self.segments.clear();
        self.sectors.clear();
        self.monochrome_sprites.clear();
        self.mono_staging.clear();
        self.mono_runs.clear();
        self.polychrome_sprites.clear();
        self.image_triangles.clear();
        self.surfaces.clear();
        self.backdrops.clear();
    }

    pub fn len(&self) -> usize {
        self.paint_operations.len()
    }

    pub fn push_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        let order = self.primitive_bounds.insert(bounds);
        self.layer_stack.push(order);
        self.paint_operations
            .push(PaintOperation::StartLayer(bounds));
    }

    pub fn pop_layer(&mut self) {
        self.layer_stack.pop();
        self.paint_operations.push(PaintOperation::EndLayer);
    }

    pub fn insert_primitive(&mut self, primitive: impl Into<Primitive>) {
        let mut primitive = primitive.into();
        let clipped_bounds = primitive
            .bounds()
            .intersect(&primitive.content_mask().bounds);

        if clipped_bounds.is_empty() {
            self.culled += 1;
            return;
        }

        let order = self
            .layer_stack
            .last()
            .copied()
            .unwrap_or_else(|| self.primitive_bounds.insert(clipped_bounds));
        match &mut primitive {
            Primitive::Shadow(shadow) => {
                shadow.order = order;
                self.shadows.push(shadow.clone());
            }
            Primitive::Quad(quad) => {
                quad.order = order;
                self.quads.push(quad.clone());
            }
            Primitive::Path(path) => {
                path.order = order;
                path.id = PathId(self.paths.len());
                self.paths.push(path.clone());
            }
            Primitive::Underline(underline) => {
                underline.order = order;
                self.underlines.push(underline.clone());
            }
            Primitive::Segment(segment) => {
                segment.order = order;
                self.segments.push(segment.clone());
            }
            Primitive::Sector(sector) => {
                sector.order = order;
                self.sectors.push(sector.clone());
            }
            Primitive::MonochromeSprite(sprite) => {
                sprite.order = order;
                let start = self.mono_staging.len();
                self.mono_staging.push(sprite.clone());
                self.record_runs(start);
            }
            Primitive::PolychromeSprite(sprite) => {
                sprite.order = order;
                self.polychrome_sprites.push(sprite.clone());
            }
            Primitive::ImageTriangle(triangle) => {
                triangle.order = order;
                self.image_triangles.push(triangle.clone());
            }
            Primitive::Surface(surface) => {
                surface.order = order;
                self.surfaces.push(surface.clone());
            }
            Primitive::Backdrop(backdrop) => {
                backdrop.order = order;
                self.backdrops.push(backdrop.clone());
            }
        }
        self.paint_operations
            .push(PaintOperation::Primitive(primitive));
    }

    /// Olo: many segments at once (`Window::paint_segments`): one paint
    /// operation for them all, not an enum and two copies each.
    pub fn insert_segments(&mut self, mut segments: Vec<Segment>) {
        let layer = self.layer_stack.last().copied();
        segments.retain_mut(|segment| {
            let clipped = segment.bounds.intersect(&segment.content_mask.bounds);
            if clipped.is_empty() {
                return false;
            }
            segment.order = layer.unwrap_or_else(|| self.primitive_bounds.insert(clipped));
            true
        });
        if segments.is_empty() {
            return;
        }
        self.segments.extend_from_slice(&segments);
        self.paint_operations
            .push(PaintOperation::Segments(segments));
    }

    /// Olo: many glyphs' sprites at once (`Window::paint_glyphs`, a
    /// line's): a page of text is tens of thousands, each written once,
    /// into `mono_staging`, as runs of one order and texture.
    pub fn insert_sprites(&mut self, sprites: impl IntoIterator<Item = MonochromeSprite>) {
        let layer = self.layer_stack.last().copied();
        let start = self.mono_staging.len();
        for mut sprite in sprites {
            let clipped = sprite.bounds.intersect(&sprite.content_mask.bounds);
            if clipped.is_empty() {
                self.culled += 1;
                continue;
            }
            sprite.order = layer.unwrap_or_else(|| self.primitive_bounds.insert(clipped));
            self.mono_staging.push(sprite);
        }
        let end = self.mono_staging.len();
        if end == start {
            return;
        }
        self.record_runs(start);
        self.paint_operations
            .push(PaintOperation::Sprites(start..end));
    }

    /// Notes the sprites added to `mono_staging` from `start` as runs of
    /// one order and texture (those of a texture together, as `finish`
    /// sorted them before: a batch is one texture).
    fn record_runs(&mut self, start: usize) {
        let key = |s: &MonochromeSprite| (s.order, texture_key(&s.tile));
        let first_run = self.mono_runs.len();
        // (One pass: a line's glyphs are nearly always one run.)
        let mut k = key(&self.mono_staging[start]);
        let mut from = start;
        let mut sorted = true;
        for i in start + 1..self.mono_staging.len() {
            let next = key(&self.mono_staging[i]);
            if next != k {
                sorted &= next > k;
                self.mono_runs.push(SpriteRun {
                    order: k.0,
                    texture: k.1,
                    range: from as u32..i as u32,
                });
                (k, from) = (next, i);
            }
        }
        self.mono_runs.push(SpriteRun {
            order: k.0,
            texture: k.1,
            range: from as u32..self.mono_staging.len() as u32,
        });
        if !sorted {
            // Glyphs of several textures in one line, interleaved: those
            // of each together, as one batch.
            self.mono_runs.truncate(first_run);
            self.mono_staging[start..].sort_by_key(key);
            self.record_runs(start);
        }
    }

    pub fn replay(&mut self, range: Range<usize>, prev_scene: &Scene) {
        for operation in &prev_scene.paint_operations[range] {
            match operation {
                PaintOperation::Primitive(primitive) => self.insert_primitive(primitive.clone()),
                PaintOperation::Segments(segments) => self.insert_segments(segments.clone()),
                PaintOperation::Sprites(range) => {
                    self.insert_sprites(prev_scene.mono_staging[range.clone()].iter().cloned())
                }
                PaintOperation::StartLayer(bounds) => self.push_layer(*bounds),
                PaintOperation::EndLayer => self.pop_layer(),
            }
        }
    }

    /// Olo: whether what `range` painted can be moved ([`Scene::replay_moved`]),
    /// sideways and up or down: nothing but quads, sprites and the like
    /// (no paths), each clipped by `mask` (what it's painted in) or by a
    /// clip of its own inside it (a list's cell) that `mask` didn't cut
    /// that way: such a clip moves with what it clips, then cut by `mask`
    /// again.
    pub(crate) fn movable(&self, range: Range<usize>, mask: &ContentMask<ScaledPixels>) -> Movable {
        let mut can = Movable { x: true, y: true };
        let mut by_mask = |m: &ContentMask<ScaledPixels>| {
            if m != mask {
                let (b, o) = (&m.bounds, &mask.bounds);
                can.x &= b.left() > o.left() && b.right() < o.right();
                can.y &= b.top() > o.top() && b.bottom() < o.bottom();
            }
        };
        for op in &self.paint_operations[range] {
            match op {
                PaintOperation::Primitive(p) => match p {
                    Primitive::Shadow(x) => by_mask(&x.content_mask),
                    Primitive::Quad(x) => by_mask(&x.content_mask),
                    Primitive::Underline(x) => by_mask(&x.content_mask),
                    Primitive::Segment(x) => by_mask(&x.content_mask),
                    Primitive::Sector(x) => by_mask(&x.content_mask),
                    Primitive::MonochromeSprite(x) => by_mask(&x.content_mask),
                    Primitive::PolychromeSprite(x) => by_mask(&x.content_mask),
                    Primitive::Path(_)
                    | Primitive::ImageTriangle(_)
                    | Primitive::Surface(_)
                    | Primitive::Backdrop(_) => {
                        return Movable { x: false, y: false };
                    }
                },
                PaintOperation::Segments(segments) => {
                    segments.iter().for_each(|x| by_mask(&x.content_mask))
                }
                PaintOperation::Sprites(sprites) => self.mono_staging[sprites.clone()]
                    .iter()
                    .for_each(|x| by_mask(&x.content_mask)),
                PaintOperation::StartLayer(_) | PaintOperation::EndLayer => {}
            }
        }
        can
    }

    /// Olo: [`Scene::replay`], everything moved by `by` (whole device
    /// pixels: glyphs and snapped edges stay on the grid), what `range`
    /// painted being [`Scene::movable`].
    /// Whether `mask` cut anything moved (then, it isn't to be moved
    /// again before it's painted afresh: what was cut is gone from it).
    pub(crate) fn replay_moved(
        &mut self,
        range: Range<usize>,
        prev_scene: &Scene,
        by: Point<ScaledPixels>,
        mask: &ContentMask<ScaledPixels>,
    ) -> bool {
        let mv = |b: Bounds<ScaledPixels>| Bounds {
            origin: b.origin + by,
            size: b.size,
        };
        let culled = self.culled;
        let cut = std::cell::Cell::new(false);
        // (A clip of its own moves with it, cut by what it's painted in.)
        let clip = |m: &mut ContentMask<ScaledPixels>| {
            if m != mask {
                let moved = mv(m.bounds);
                m.bounds = moved.intersect(&mask.bounds);
                if m.bounds != moved {
                    cut.set(true);
                }
            }
        };
        for operation in &prev_scene.paint_operations[range] {
            match operation {
                PaintOperation::Primitive(primitive) => {
                    let mut p = primitive.clone();
                    match &mut p {
                        Primitive::Shadow(x) => {
                            (x.bounds, _) = (mv(x.bounds), clip(&mut x.content_mask))
                        }
                        Primitive::Quad(x) => {
                            (x.bounds, _) = (mv(x.bounds), clip(&mut x.content_mask))
                        }
                        Primitive::Underline(x) => {
                            (x.bounds, _) = (mv(x.bounds), clip(&mut x.content_mask))
                        }
                        Primitive::Segment(x) => {
                            x.bounds = mv(x.bounds);
                            (x.from, x.to, x.bottom) = (x.from + by, x.to + by, x.bottom + by.y);
                            clip(&mut x.content_mask);
                        }
                        Primitive::Sector(x) => {
                            (x.bounds, x.center) = (mv(x.bounds), x.center + by);
                            clip(&mut x.content_mask);
                        }
                        Primitive::MonochromeSprite(x) => {
                            (x.bounds, _) = (mv(x.bounds), clip(&mut x.content_mask))
                        }
                        Primitive::PolychromeSprite(x) => {
                            (x.bounds, _) = (mv(x.bounds), clip(&mut x.content_mask))
                        }
                        Primitive::Path(_)
                        | Primitive::ImageTriangle(_)
                        | Primitive::Surface(_)
                        | Primitive::Backdrop(_) => {}
                    }
                    self.insert_primitive(p);
                }
                PaintOperation::Segments(segments) => self.insert_segments(
                    segments
                        .iter()
                        .map(|x| {
                            let mut x = x.clone();
                            x.bounds = mv(x.bounds);
                            (x.from, x.to, x.bottom) = (x.from + by, x.to + by, x.bottom + by.y);
                            clip(&mut x.content_mask);
                            x
                        })
                        .collect(),
                ),
                PaintOperation::Sprites(sprites) => {
                    self.insert_sprites(prev_scene.mono_staging[sprites.clone()].iter().map(|x| {
                        let mut x = x.clone();
                        x.bounds = mv(x.bounds);
                        clip(&mut x.content_mask);
                        x
                    }))
                }
                PaintOperation::StartLayer(bounds) => self.push_layer(mv(*bounds)),
                PaintOperation::EndLayer => self.pop_layer(),
            }
        }
        cut.get() || self.culled != culled
    }

    pub fn finish(&mut self) {
        self.shadows.sort_by_key(|shadow| shadow.order);
        self.quads.sort_by_key(|quad| quad.order);
        self.cull_hidden_quads();
        self.paths.sort_by_key(|path| path.order);
        self.underlines.sort_by_key(|underline| underline.order);
        self.segments.sort_by_key(|segment| segment.order);
        self.sectors.sort_by_key(|sector| sector.order);
        // Olo: by the atlas texture in each place in the order (what a
        // batch is), not each tile's own id: a page of glyphs is mostly
        // one texture and in order already, which the sort finds in a
        // pass, where sorting by tile shuffled them all every frame.
        // The glyphs go by their runs (`mono_runs`): the same order as
        // sorting each, for a sort of a frame's lines.
        self.mono_runs.sort_by_key(|run| (run.order, run.texture));
        self.monochrome_sprites.clear();
        self.monochrome_sprites.reserve(self.mono_staging.len());
        for run in &self.mono_runs {
            self.monochrome_sprites.extend_from_slice(
                &self.mono_staging[run.range.start as usize..run.range.end as usize],
            );
        }
        self.polychrome_sprites
            .sort_by_key(|sprite| (sprite.order, texture_key(&sprite.tile)));
        self.image_triangles.sort_by_key(|triangle| triangle.order);
        self.surfaces.sort_by_key(|surface| surface.order);
        self.backdrops.sort_by_key(|backdrop| backdrop.order);
    }

    /// Olo: a big quad wholly under opaque quads drawn after it isn't
    /// drawn, and a plain one mostly under them only where it shows (a
    /// window's ground under its views' own grounds filled the window
    /// two or three times over a frame, the GPU's biggest cost).
    fn cull_hidden_quads(&mut self) {
        // (Small quads aren't worth it: what one costs is its pixels.)
        const BIG: f32 = 256.0 * 256.0;
        // (Notifications' History leaves its paper in 214 at 1×: a piece
        // each is a few bytes and four vertices.)
        const MOST_PIECES: usize = 512;
        let area = |b: &Bounds<ScaledPixels>| b.size.width.0.max(0.0) * b.size.height.0.max(0.0);
        let shown = |q: &Quad| q.bounds.intersect(&q.content_mask.bounds);
        // Drawn over all that's under it, edge to edge: one colour, no
        // round corners or border (whose edges are anti-aliased).
        let plain = |q: &Quad| {
            q.background.tag == crate::BackgroundTag::Solid
                && q.corner_radii == Corners::default()
                && q.border_widths == Edges::default()
        };
        // What an opaque quad hides of what's under it: a plain one all of
        // it (to a hair in from its edges: what's cut stays drawn up to
        // them, never a gap); a rounded or bordered one (cards) the band
        // between its corners, a pixel in from its sides, whose edges are
        // anti-aliased (a border opaque and solid). Notifications'
        // History drew its card paper whole under its cards: 6.8 Mpx a
        // frame at 1×, as much as the cards themselves.
        let cover = |q: &Quad| -> Option<Bounds<ScaledPixels>> {
            if q.background.tag != crate::BackgroundTag::Solid || q.background.solid.a < 1.0 {
                return None;
            }
            let c = shown(q);
            if plain(q) {
                return Some(Bounds {
                    origin: point(
                        ScaledPixels(c.origin.x.0 + 0.01),
                        ScaledPixels(c.origin.y.0 + 0.01),
                    ),
                    size: size(
                        ScaledPixels(c.size.width.0 - 0.02),
                        ScaledPixels(c.size.height.0 - 0.02),
                    ),
                });
            }
            let bordered = q.border_widths != Edges::default();
            if bordered && (q.border_color.a < 1.0 || q.border_style != BorderStyle::Solid) {
                return None;
            }
            let r = q.corner_radii;
            let dy = r
                .top_left
                .0
                .max(r.top_right.0)
                .max(r.bottom_left.0)
                .max(r.bottom_right.0)
                + 1.0;
            let b = q.bounds;
            let band = Bounds {
                origin: point(
                    ScaledPixels(b.origin.x.0 + 1.0),
                    ScaledPixels(b.origin.y.0 + dy),
                ),
                size: size(
                    ScaledPixels(b.size.width.0 - 2.0),
                    ScaledPixels(b.size.height.0 - 2.0 * dy),
                ),
            };
            Some(band.intersect(&q.content_mask.bounds))
        };
        // Each quad's, found once. (The pieces a quad is cut into below
        // cover nothing: what's under them is done.)
        let mut covers: Vec<Option<Bounds<ScaledPixels>>> = self
            .quads
            .iter()
            .map(|q| cover(q).filter(|c| c.size.width.0 > 0.0 && c.size.height.0 > 0.0))
            .collect();
        let mut over = Vec::new();
        let mut i = 0;
        while i < self.quads.len() {
            let a = shown(&self.quads[i]);
            if area(&a) < BIG {
                i += 1;
                continue;
            }
            // What covers it, all at once (`uncovered`): a window's ground
            // is mostly under one view's (Notifications' card paper) and
            // cut by a menu bar's and toolbar's little quads, and a paper
            // by its grid of cards. (Cut one cover at a time, a ground came
            // apart into more pieces than were worth drawing before the
            // paper's turn, and was drawn whole: a screen of overdraw.)
            over.clear();
            let mut most = 0.0;
            for c in covers[i + 1..].iter().flatten() {
                let o = c.intersect(&a);
                if o.size.width.0 > 0.0 && o.size.height.0 > 0.0 {
                    most += area(&o);
                    over.push(o);
                }
            }
            // (Not looked into when the covers couldn't hide what's worth
            // it, overlaps and all: all of one that isn't plain, which
            // can't be cut (a card under its own buttons), or a quarter.)
            let flat = plain(&self.quads[i]);
            if most < if flat { 0.25 } else { 0.99 } * area(&a) {
                i += 1;
                continue;
            }
            // (Less the slivers the hair leaves where a cover's edge is one
            // of the quad's: it draws those pixels itself.)
            let left: Vec<_> = uncovered(a, &over)
                .into_iter()
                .filter(|r| r.size.width.0 >= 0.05 && r.size.height.0 >= 0.05)
                .collect();
            if left.is_empty() {
                self.quads.remove(i);
                covers.remove(i);
                continue;
            }
            let kept: f32 = left.iter().map(area).sum();
            if flat && kept < 0.75 * area(&a) && left.len() <= MOST_PIECES {
                let q = self.quads[i].clone();
                let pieces: Vec<Quad> = left
                    .into_iter()
                    .filter(|r| area(r) > 0.0)
                    .map(|r| Quad {
                        bounds: r,
                        ..q.clone()
                    })
                    .collect();
                let n = pieces.len();
                self.quads.splice(i..i + 1, pieces);
                covers.splice(i..i + 1, std::iter::repeat_n(None, n));
                i += n;
                continue;
            }
            i += 1;
        }
    }

    #[cfg_attr(
        all(
            any(target_os = "linux", target_os = "freebsd"),
            not(feature = "wayland")
        ),
        allow(dead_code)
    )]
    pub(crate) fn batches(&self) -> impl Iterator<Item = PrimitiveBatch<'_>> {
        BatchIterator {
            shadows: &self.shadows,
            shadows_start: 0,
            shadows_iter: self.shadows.iter().peekable(),
            quads: &self.quads,
            quads_start: 0,
            quads_iter: self.quads.iter().peekable(),
            paths: &self.paths,
            paths_start: 0,
            paths_iter: self.paths.iter().peekable(),
            underlines: &self.underlines,
            underlines_start: 0,
            underlines_iter: self.underlines.iter().peekable(),
            segments: &self.segments,
            segments_start: 0,
            segments_iter: self.segments.iter().peekable(),
            sectors: &self.sectors,
            sectors_start: 0,
            sectors_iter: self.sectors.iter().peekable(),
            monochrome_sprites: &self.monochrome_sprites,
            monochrome_sprites_start: 0,
            monochrome_sprites_iter: self.monochrome_sprites.iter().peekable(),
            polychrome_sprites: &self.polychrome_sprites,
            polychrome_sprites_start: 0,
            polychrome_sprites_iter: self.polychrome_sprites.iter().peekable(),
            image_triangles: &self.image_triangles,
            image_triangles_start: 0,
            image_triangles_iter: self.image_triangles.iter().peekable(),
            surfaces: &self.surfaces,
            surfaces_start: 0,
            surfaces_iter: self.surfaces.iter().peekable(),
            backdrops: &self.backdrops,
            backdrops_start: 0,
            backdrops_iter: self.backdrops.iter().peekable(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Default)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(feature = "wayland")
    ),
    allow(dead_code)
)]
pub(crate) enum PrimitiveKind {
    Shadow,
    #[default]
    Quad,
    Path,
    Sector,
    Underline,
    Segment,
    MonochromeSprite,
    PolychromeSprite,
    ImageTriangle,
    Surface,
    Backdrop,
}

/// Olo: which ways what a range painted can be moved ([`Scene::movable`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Movable {
    pub x: bool,
    pub y: bool,
}

/// Olo: glyph sprites of one order and texture, together in a scene's
/// `mono_staging`.
struct SpriteRun {
    order: DrawOrder,
    texture: (u8, u32),
    range: Range<u32>,
}

/// An atlas tile's texture, as sprites are batched by.
fn texture_key(t: &AtlasTile) -> (u8, u32) {
    (t.texture_id.kind as u8, t.texture_id.index)
}

pub(crate) enum PaintOperation {
    Primitive(Primitive),
    /// Olo: a polyline's pieces (`Scene::insert_segments`).
    Segments(Vec<Segment>),
    /// Olo: a run of glyphs (`Scene::insert_sprites`), where they are in
    /// the scene's `mono_staging`.
    Sprites(Range<usize>),
    StartLayer(Bounds<ScaledPixels>),
    EndLayer,
}

#[derive(Clone)]
pub(crate) enum Primitive {
    Shadow(Shadow),
    Quad(Quad),
    Path(Path<ScaledPixels>),
    Underline(Underline),
    Segment(Segment),
    Sector(Sector),
    MonochromeSprite(MonochromeSprite),
    PolychromeSprite(PolychromeSprite),
    ImageTriangle(ImageTriangle),
    Surface(PaintSurface),
    Backdrop(Backdrop),
}

impl Primitive {
    pub fn bounds(&self) -> &Bounds<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.bounds,
            Primitive::Quad(quad) => &quad.bounds,
            Primitive::Path(path) => &path.bounds,
            Primitive::Underline(underline) => &underline.bounds,
            Primitive::Segment(segment) => &segment.bounds,
            Primitive::Sector(sector) => &sector.bounds,
            Primitive::MonochromeSprite(sprite) => &sprite.bounds,
            Primitive::PolychromeSprite(sprite) => &sprite.bounds,
            Primitive::ImageTriangle(triangle) => &triangle.bounds,
            Primitive::Surface(surface) => &surface.bounds,
            Primitive::Backdrop(backdrop) => &backdrop.bounds,
        }
    }

    pub fn content_mask(&self) -> &ContentMask<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.content_mask,
            Primitive::Quad(quad) => &quad.content_mask,
            Primitive::Path(path) => &path.content_mask,
            Primitive::Underline(underline) => &underline.content_mask,
            Primitive::Segment(segment) => &segment.content_mask,
            Primitive::Sector(sector) => &sector.content_mask,
            Primitive::MonochromeSprite(sprite) => &sprite.content_mask,
            Primitive::PolychromeSprite(sprite) => &sprite.content_mask,
            Primitive::ImageTriangle(triangle) => &triangle.content_mask,
            Primitive::Surface(surface) => &surface.content_mask,
            Primitive::Backdrop(backdrop) => &backdrop.content_mask,
        }
    }
}

#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(feature = "wayland")
    ),
    allow(dead_code)
)]
struct BatchIterator<'a> {
    shadows: &'a [Shadow],
    shadows_start: usize,
    shadows_iter: Peekable<slice::Iter<'a, Shadow>>,
    quads: &'a [Quad],
    quads_start: usize,
    quads_iter: Peekable<slice::Iter<'a, Quad>>,
    paths: &'a [Path<ScaledPixels>],
    paths_start: usize,
    paths_iter: Peekable<slice::Iter<'a, Path<ScaledPixels>>>,
    underlines: &'a [Underline],
    underlines_start: usize,
    underlines_iter: Peekable<slice::Iter<'a, Underline>>,
    segments: &'a [Segment],
    segments_start: usize,
    segments_iter: Peekable<slice::Iter<'a, Segment>>,
    sectors: &'a [Sector],
    sectors_start: usize,
    sectors_iter: Peekable<slice::Iter<'a, Sector>>,
    monochrome_sprites: &'a [MonochromeSprite],
    monochrome_sprites_start: usize,
    monochrome_sprites_iter: Peekable<slice::Iter<'a, MonochromeSprite>>,
    polychrome_sprites: &'a [PolychromeSprite],
    polychrome_sprites_start: usize,
    polychrome_sprites_iter: Peekable<slice::Iter<'a, PolychromeSprite>>,
    image_triangles: &'a [ImageTriangle],
    image_triangles_start: usize,
    image_triangles_iter: Peekable<slice::Iter<'a, ImageTriangle>>,
    surfaces: &'a [PaintSurface],
    surfaces_start: usize,
    surfaces_iter: Peekable<slice::Iter<'a, PaintSurface>>,
    backdrops: &'a [Backdrop],
    backdrops_start: usize,
    backdrops_iter: Peekable<slice::Iter<'a, Backdrop>>,
}

impl<'a> Iterator for BatchIterator<'a> {
    type Item = PrimitiveBatch<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut orders_and_kinds = [
            (
                self.shadows_iter.peek().map(|s| s.order),
                PrimitiveKind::Shadow,
            ),
            (self.quads_iter.peek().map(|q| q.order), PrimitiveKind::Quad),
            (self.paths_iter.peek().map(|q| q.order), PrimitiveKind::Path),
            (
                self.sectors_iter.peek().map(|s| s.order),
                PrimitiveKind::Sector,
            ),
            (
                self.underlines_iter.peek().map(|u| u.order),
                PrimitiveKind::Underline,
            ),
            (
                self.segments_iter.peek().map(|s| s.order),
                PrimitiveKind::Segment,
            ),
            (
                self.monochrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::MonochromeSprite,
            ),
            (
                self.polychrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::PolychromeSprite,
            ),
            (
                self.image_triangles_iter.peek().map(|t| t.order),
                PrimitiveKind::ImageTriangle,
            ),
            (
                self.surfaces_iter.peek().map(|s| s.order),
                PrimitiveKind::Surface,
            ),
            (
                self.backdrops_iter.peek().map(|b| b.order),
                PrimitiveKind::Backdrop,
            ),
        ];
        orders_and_kinds.sort_by_key(|(order, kind)| (order.unwrap_or(u32::MAX), *kind));

        let first = orders_and_kinds[0];
        let second = orders_and_kinds[1];
        let (batch_kind, max_order_and_kind) = if first.0.is_some() {
            (first.1, (second.0.unwrap_or(u32::MAX), second.1))
        } else {
            return None;
        };

        match batch_kind {
            PrimitiveKind::Shadow => {
                let shadows_start = self.shadows_start;
                let mut shadows_end = shadows_start + 1;
                self.shadows_iter.next();
                while self
                    .shadows_iter
                    .next_if(|shadow| (shadow.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    shadows_end += 1;
                }
                self.shadows_start = shadows_end;
                Some(PrimitiveBatch::Shadows(
                    &self.shadows[shadows_start..shadows_end],
                ))
            }
            PrimitiveKind::Quad => {
                let quads_start = self.quads_start;
                let mut quads_end = quads_start + 1;
                self.quads_iter.next();
                while self
                    .quads_iter
                    .next_if(|quad| (quad.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    quads_end += 1;
                }
                self.quads_start = quads_end;
                Some(PrimitiveBatch::Quads(&self.quads[quads_start..quads_end]))
            }
            PrimitiveKind::Path => {
                let paths_start = self.paths_start;
                let mut paths_end = paths_start + 1;
                self.paths_iter.next();
                while self
                    .paths_iter
                    .next_if(|path| (path.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    paths_end += 1;
                }
                self.paths_start = paths_end;
                Some(PrimitiveBatch::Paths(&self.paths[paths_start..paths_end]))
            }
            PrimitiveKind::Underline => {
                let underlines_start = self.underlines_start;
                let mut underlines_end = underlines_start + 1;
                self.underlines_iter.next();
                while self
                    .underlines_iter
                    .next_if(|underline| (underline.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    underlines_end += 1;
                }
                self.underlines_start = underlines_end;
                Some(PrimitiveBatch::Underlines(
                    &self.underlines[underlines_start..underlines_end],
                ))
            }
            PrimitiveKind::Segment => {
                let segments_start = self.segments_start;
                let mut segments_end = segments_start + 1;
                self.segments_iter.next();
                while self
                    .segments_iter
                    .next_if(|segment| (segment.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    segments_end += 1;
                }
                self.segments_start = segments_end;
                Some(PrimitiveBatch::Segments(
                    &self.segments[segments_start..segments_end],
                ))
            }
            PrimitiveKind::Sector => {
                let sectors_start = self.sectors_start;
                let mut sectors_end = sectors_start + 1;
                self.sectors_iter.next();
                while self
                    .sectors_iter
                    .next_if(|sector| (sector.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    sectors_end += 1;
                }
                self.sectors_start = sectors_end;
                Some(PrimitiveBatch::Sectors(
                    &self.sectors[sectors_start..sectors_end],
                ))
            }
            PrimitiveKind::MonochromeSprite => {
                let texture_id = self.monochrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.monochrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.monochrome_sprites_iter.next();
                while self
                    .monochrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.monochrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::MonochromeSprites {
                    texture_id,
                    sprites: &self.monochrome_sprites[sprites_start..sprites_end],
                })
            }
            PrimitiveKind::PolychromeSprite => {
                let texture_id = self.polychrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.polychrome_sprites_start;
                let mut sprites_end = self.polychrome_sprites_start + 1;
                self.polychrome_sprites_iter.next();
                while self
                    .polychrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.polychrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::PolychromeSprites {
                    texture_id,
                    sprites: &self.polychrome_sprites[sprites_start..sprites_end],
                })
            }
            PrimitiveKind::ImageTriangle => {
                let texture_id = self.image_triangles_iter.peek().unwrap().tile.texture_id;
                let start = self.image_triangles_start;
                let mut end = start + 1;
                self.image_triangles_iter.next();
                while self
                    .image_triangles_iter
                    .next_if(|triangle| {
                        (triangle.order, batch_kind) < max_order_and_kind
                            && triangle.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    end += 1;
                }
                self.image_triangles_start = end;
                Some(PrimitiveBatch::ImageTriangles {
                    texture_id,
                    triangles: &self.image_triangles[start..end],
                })
            }
            PrimitiveKind::Surface => {
                let surfaces_start = self.surfaces_start;
                let mut surfaces_end = surfaces_start + 1;
                self.surfaces_iter.next();
                while self
                    .surfaces_iter
                    .next_if(|surface| (surface.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    surfaces_end += 1;
                }
                self.surfaces_start = surfaces_end;
                Some(PrimitiveBatch::Surfaces(
                    &self.surfaces[surfaces_start..surfaces_end],
                ))
            }
            // One at a time: each blurs what was drawn before it.
            PrimitiveKind::Backdrop => {
                let start = self.backdrops_start;
                self.backdrops_iter.next();
                self.backdrops_start = start + 1;
                Some(PrimitiveBatch::Backdrops(&self.backdrops[start..start + 1]))
            }
        }
    }
}

#[derive(Debug)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(feature = "wayland")
    ),
    allow(dead_code)
)]
pub(crate) enum PrimitiveBatch<'a> {
    Shadows(&'a [Shadow]),
    Quads(&'a [Quad]),
    Paths(&'a [Path<ScaledPixels>]),
    Underlines(&'a [Underline]),
    Segments(&'a [Segment]),
    Sectors(&'a [Sector]),
    MonochromeSprites {
        texture_id: AtlasTextureId,
        sprites: &'a [MonochromeSprite],
    },
    PolychromeSprites {
        texture_id: AtlasTextureId,
        sprites: &'a [PolychromeSprite],
    },
    ImageTriangles {
        texture_id: AtlasTextureId,
        triangles: &'a [ImageTriangle],
    },
    Surfaces(&'a [PaintSurface]),
    Backdrops(&'a [Backdrop]),
}

#[derive(Default, Debug, Clone)]
#[repr(C)]
pub(crate) struct Quad {
    pub order: DrawOrder,
    pub border_style: BorderStyle,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub background: Background,
    pub border_color: Hsla,
    pub corner_radii: Corners<ScaledPixels>,
    pub border_widths: Edges<ScaledPixels>,
}

impl From<Quad> for Primitive {
    fn from(quad: Quad) -> Self {
        Primitive::Quad(quad)
    }
}

#[derive(Debug, Clone)]
#[repr(C)]
pub(crate) struct Underline {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub thickness: ScaledPixels,
    pub wavy: u32,
}

impl From<Underline> for Primitive {
    fn from(underline: Underline) -> Self {
        Primitive::Underline(underline)
    }
}

/// Olo: a straight line of any angle and width, antialiased by each
/// pixel's distance to it (`Window::paint_segment`).
#[derive(Debug, Clone)]
#[repr(C)]
pub(crate) struct Segment {
    pub order: DrawOrder,
    /// 1 for round ends, 0 for butt ends; 2 for what's under it, down
    /// to `bottom` (`Window::paint_area`).
    pub round: u32,
    /// What it covers (for ordering and clipping).
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub from: Point<ScaledPixels>,
    pub to: Point<ScaledPixels>,
    pub half_width: ScaledPixels,
    /// How far down what's under it goes (an area's).
    pub bottom: ScaledPixels,
}

impl From<Segment> for Primitive {
    fn from(segment: Segment) -> Self {
        Primitive::Segment(segment)
    }
}

/// Olo: a ring's slice (`Window::paint_sector`): between two radii and
/// two angles, antialiased by each pixel's distance to its edges. A disc
/// is one from radius 0 all the way round, a circle's outline a thin
/// ring.
#[derive(Debug, Clone)]
#[repr(C)]
pub(crate) struct Sector {
    pub order: DrawOrder,
    pub pad: u32,
    /// What it covers (for ordering and clipping, and the quad drawn).
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub center: Point<ScaledPixels>,
    pub inner: ScaledPixels,
    pub outer: ScaledPixels,
    /// Radians clockwise from 12 o'clock: where it starts, and how far
    /// round it goes (a whole turn or more: all the way).
    pub start: f32,
    pub sweep: f32,
}

impl From<Sector> for Primitive {
    fn from(sector: Sector) -> Self {
        Primitive::Sector(sector)
    }
}

#[derive(Debug, Clone)]
#[repr(C)]
pub(crate) struct Shadow {
    pub order: DrawOrder,
    pub blur_radius: ScaledPixels,
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
}

impl From<Shadow> for Primitive {
    fn from(shadow: Shadow) -> Self {
        Primitive::Shadow(shadow)
    }
}

/// The style of a border.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub enum BorderStyle {
    /// A solid border.
    #[default]
    Solid = 0,
    /// A dashed border.
    Dashed = 1,
}

/// A data type representing a 2 dimensional transformation that can be applied to an element.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct TransformationMatrix {
    /// 2x2 matrix containing rotation and scale,
    /// stored row-major
    pub rotation_scale: [[f32; 2]; 2],
    /// translation vector
    pub translation: [f32; 2],
}

impl Eq for TransformationMatrix {}

impl TransformationMatrix {
    /// The unit matrix, has no effect.
    pub fn unit() -> Self {
        Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [0.0, 0.0],
        }
    }

    /// Move the origin by a given point
    pub fn translate(mut self, point: Point<ScaledPixels>) -> Self {
        self.compose(Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [point.x.0, point.y.0],
        })
    }

    /// Clockwise rotation in radians around the origin
    pub fn rotate(self, angle: Radians) -> Self {
        self.compose(Self {
            rotation_scale: [
                [angle.0.cos(), -angle.0.sin()],
                [angle.0.sin(), angle.0.cos()],
            ],
            translation: [0.0, 0.0],
        })
    }

    /// Scale around the origin
    pub fn scale(self, size: Size<f32>) -> Self {
        self.compose(Self {
            rotation_scale: [[size.width, 0.0], [0.0, size.height]],
            translation: [0.0, 0.0],
        })
    }

    /// Perform matrix multiplication with another transformation
    /// to produce a new transformation that is the result of
    /// applying both transformations: first, `other`, then `self`.
    #[inline]
    pub fn compose(self, other: TransformationMatrix) -> TransformationMatrix {
        if other == Self::unit() {
            return self;
        }
        // Perform matrix multiplication
        TransformationMatrix {
            rotation_scale: [
                [
                    self.rotation_scale[0][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][0],
                    self.rotation_scale[0][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][1],
                ],
                [
                    self.rotation_scale[1][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][0],
                    self.rotation_scale[1][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][1],
                ],
            ],
            translation: [
                self.translation[0]
                    + self.rotation_scale[0][0] * other.translation[0]
                    + self.rotation_scale[0][1] * other.translation[1],
                self.translation[1]
                    + self.rotation_scale[1][0] * other.translation[0]
                    + self.rotation_scale[1][1] * other.translation[1],
            ],
        }
    }

    /// Apply transformation to a point, mainly useful for debugging
    pub fn apply(&self, point: Point<Pixels>) -> Point<Pixels> {
        let input = [point.x.0, point.y.0];
        let mut output = self.translation;
        for (i, output_cell) in output.iter_mut().enumerate() {
            for (k, input_cell) in input.iter().enumerate() {
                *output_cell += self.rotation_scale[i][k] * *input_cell;
            }
        }
        Point::new(output[0].into(), output[1].into())
    }
}

impl Default for TransformationMatrix {
    fn default() -> Self {
        Self::unit()
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
pub(crate) struct MonochromeSprite {
    pub order: DrawOrder,
    /// Olo: 1 when the tile is a shape's exact coverage, used as it is
    /// (`crate::outline`); 0 for a glyph's, gamma-corrected as text.
    /// (It also keeps the fields after it 8-byte aligned.)
    pub coverage: u32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub tile: AtlasTile,
    pub transformation: TransformationMatrix,
}

impl From<MonochromeSprite> for Primitive {
    fn from(sprite: MonochromeSprite) -> Self {
        Primitive::MonochromeSprite(sprite)
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
pub(crate) struct PolychromeSprite {
    pub order: DrawOrder,
    /// Olo: 1 to sample its texels as they are (nearest), else
    /// filtered: a small picture magnified keeps its pixels.
    pub nearest: u32,
    pub grayscale: bool,
    pub opacity: f32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub tile: AtlasTile,
}

impl From<PolychromeSprite> for Primitive {
    fn from(sprite: PolychromeSprite) -> Self {
        Primitive::PolychromeSprite(sprite)
    }
}

/// Olo: a triangle of an image (`Window::paint_image_triangles`): each
/// corner a point in the window and where in the image (0–1) it shows,
/// the image tinted. A picture bent in perspective, a lens's disc.
#[derive(Debug, Clone)]
#[repr(C)]
pub(crate) struct ImageTriangle {
    pub order: DrawOrder,
    pub opacity: f32,
    /// What it covers (for ordering and clipping).
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub tile: AtlasTile,
    pub corners: [Point<ScaledPixels>; 3],
    pub uvs: [Point<f32>; 3],
    pub tint: Hsla,
}

impl From<ImageTriangle> for Primitive {
    fn from(triangle: ImageTriangle) -> Self {
        Primitive::ImageTriangle(triangle)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PaintSurface {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    /// Olo: its corners rounded.
    pub corner_radii: Corners<ScaledPixels>,
    /// Olo: its opacity (a video fading in, inside an opacity group).
    pub opacity: f32,
    #[cfg(target_os = "macos")]
    pub image_buffer: core_video::pixel_buffer::CVPixelBuffer,
    /// Olo: a video's picture, its planes as decoded (`Window::paint_yuv`).
    #[cfg(not(target_os = "macos"))]
    pub frame: std::sync::Arc<crate::YuvFrame>,
}

/// Olo: what was drawn under `bounds` before it, shown blurred and
/// saturated (CSS `backdrop-filter: blur() saturate()`), rounded by its
/// corners (`Window::paint_backdrop`).
#[derive(Default, Debug, Clone)]
pub(crate) struct Backdrop {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    /// The blur's standard deviation, in device pixels.
    pub sigma: f32,
    /// 1 leaves colours as they are, 0 grey.
    pub saturation: f32,
    /// Saturated before it is blurred (`saturate() blur()`), else after.
    pub saturate_first: bool,
}

impl From<Backdrop> for Primitive {
    fn from(backdrop: Backdrop) -> Self {
        Primitive::Backdrop(backdrop)
    }
}

impl From<PaintSurface> for Primitive {
    fn from(surface: PaintSurface) -> Self {
        Primitive::Surface(surface)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PathId(pub(crate) usize);

/// A line made up of a series of vertices and control points.
#[derive(Clone, Debug)]
pub struct Path<P: Clone + Debug + Default + PartialEq> {
    pub(crate) id: PathId,
    pub(crate) order: DrawOrder,
    pub(crate) bounds: Bounds<P>,
    pub(crate) content_mask: ContentMask<P>,
    pub(crate) vertices: Vec<PathVertex<P>>,
    pub(crate) color: Background,
    start: Point<P>,
    current: Point<P>,
    contour_count: usize,
    /// Olo: what it was built from (`PathBuilder`), for drawing it with
    /// exact coverage (`crate::outline`).
    pub(crate) outline: Option<std::sync::Arc<crate::outline::PathOutline>>,
}

impl Path<Pixels> {
    /// Create a new path with the given starting point.
    pub fn new(start: Point<Pixels>) -> Self {
        Self {
            id: PathId(0),
            order: DrawOrder::default(),
            vertices: Vec::new(),
            start,
            current: start,
            bounds: Bounds {
                origin: start,
                size: Default::default(),
            },
            content_mask: Default::default(),
            color: Default::default(),
            contour_count: 0,
            outline: None,
        }
    }

    /// Scale this path by the given factor.
    pub fn scale(&self, factor: f32) -> Path<ScaledPixels> {
        Path {
            id: self.id,
            order: self.order,
            bounds: self.bounds.scale(factor),
            content_mask: self.content_mask.scale(factor),
            vertices: self
                .vertices
                .iter()
                .map(|vertex| vertex.scale(factor))
                .collect(),
            start: self.start.map(|start| start.scale(factor)),
            current: self.current.scale(factor),
            contour_count: self.contour_count,
            color: self.color,
            outline: None,
        }
    }

    /// Move the start, current point to the given point.
    pub fn move_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        self.start = to;
        self.current = to;
    }

    /// Draw a straight line from the current point to the given point.
    pub fn line_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }
        self.current = to;
    }

    /// Draw a curve from the current point to the given point, using the given control point.
    pub fn curve_to(&mut self, to: Point<Pixels>, ctrl: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }

        self.push_triangle(
            (self.current, ctrl, to),
            (point(0., 0.), point(0.5, 0.), point(1., 1.)),
        );
        self.current = to;
    }

    /// Push a triangle to the Path.
    pub fn push_triangle(
        &mut self,
        xy: (Point<Pixels>, Point<Pixels>, Point<Pixels>),
        st: (Point<f32>, Point<f32>, Point<f32>),
    ) {
        self.bounds = self
            .bounds
            .union(&Bounds {
                origin: xy.0,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.1,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.2,
                size: Default::default(),
            });

        self.vertices.push(PathVertex {
            xy_position: xy.0,
            st_position: st.0,
            content_mask: Default::default(),
        });
        self.vertices.push(PathVertex {
            xy_position: xy.1,
            st_position: st.1,
            content_mask: Default::default(),
        });
        self.vertices.push(PathVertex {
            xy_position: xy.2,
            st_position: st.2,
            content_mask: Default::default(),
        });
    }
}

impl<T> Path<T>
where
    T: Clone + Debug + Default + PartialEq + PartialOrd + Add<T, Output = T> + Sub<Output = T>,
{
    #[allow(unused)]
    pub(crate) fn clipped_bounds(&self) -> Bounds<T> {
        self.bounds.intersect(&self.content_mask.bounds)
    }
}

impl From<Path<ScaledPixels>> for Primitive {
    fn from(path: Path<ScaledPixels>) -> Self {
        Primitive::Path(path)
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
pub(crate) struct PathVertex<P: Clone + Debug + Default + PartialEq> {
    pub(crate) xy_position: Point<P>,
    pub(crate) st_position: Point<f32>,
    pub(crate) content_mask: ContentMask<P>,
}

impl PathVertex<Pixels> {
    pub fn scale(&self, factor: f32) -> PathVertex<ScaledPixels> {
        PathVertex {
            xy_position: self.xy_position.scale(factor),
            st_position: self.st_position,
            content_mask: self.content_mask.scale(factor),
        }
    }
}

/// Olo: what of `r` none of `covers` is over, in rectangles: `r` cut
/// into bands across at the covers' tops and bottoms, each band's
/// stretches between them, a stretch the bands below have too going on
/// down (a grid of cards on a paper leaves its gutters, a few pieces a
/// row).
fn uncovered(
    r: Bounds<ScaledPixels>,
    covers: &[Bounds<ScaledPixels>],
) -> Vec<Bounds<ScaledPixels>> {
    let (rl, rt, rr, rb) = (
        r.origin.x.0,
        r.origin.y.0,
        r.origin.x.0 + r.size.width.0,
        r.origin.y.0 + r.size.height.0,
    );
    // Each cover as it is over `r`: left, top, right, bottom. (Not one
    // too small to be worth its bands: a toolbar's button.)
    let least = (rr - rl) * (rb - rt) / 1000.0;
    let area = |c: &[f32; 4]| (c[2] - c[0]) * (c[3] - c[1]);
    let mut cs: Vec<[f32; 4]> = covers
        .iter()
        .map(|c| {
            [
                c.origin.x.0.max(rl),
                c.origin.y.0.max(rt),
                (c.origin.x.0 + c.size.width.0).min(rr),
                (c.origin.y.0 + c.size.height.0).min(rb),
            ]
        })
        .filter(|c| c[0] < c[2] && c[1] < c[3] && area(c) >= least)
        .collect();
    // Less those inside a bigger one (the cards on a window's paper),
    // each a band or two more for nothing.
    cs.sort_by(|a, b| area(b).total_cmp(&area(a)));
    let mut kept = 0;
    for k in 0..cs.len() {
        let c = cs[k];
        if !cs[..kept]
            .iter()
            .any(|o| o[0] <= c[0] && o[1] <= c[1] && c[2] <= o[2] && c[3] <= o[3])
        {
            cs[kept] = c;
            kept += 1;
        }
    }
    cs.truncate(kept);
    cs.sort_by(|a, b| a[1].total_cmp(&b[1]));
    let mut ys: Vec<f32> = cs
        .iter()
        .flat_map(|c| [c[1], c[3]])
        .chain([rt, rb])
        .collect();
    ys.sort_by(f32::total_cmp);
    ys.dedup();
    let rect = |l: f32, t: f32, r: f32, b: f32| Bounds {
        origin: point(ScaledPixels(l), ScaledPixels(t)),
        size: size(ScaledPixels(r - l), ScaledPixels(b - t)),
    };
    let mut out = Vec::new();
    // The stretches open in the bands so far, left to right: left,
    // right, since where.
    let (mut open, mut going): (Vec<(f32, f32, f32)>, Vec<(f32, f32, f32)>) =
        (Vec::new(), Vec::new());
    let (mut next, mut active, mut spans, mut free): (
        usize,
        Vec<[f32; 4]>,
        Vec<(f32, f32)>,
        Vec<(f32, f32)>,
    ) = (0, Vec::new(), Vec::new(), Vec::new());
    for band in ys.windows(2) {
        let (y0, y1) = (band[0], band[1]);
        active.retain(|c| c[3] > y0);
        while next < cs.len() && cs[next][1] <= y0 {
            if cs[next][3] > y0 {
                active.push(cs[next]);
            }
            next += 1;
        }
        // The band's open stretches, left to right: `r`'s width less its
        // covers'.
        spans.clear();
        spans.extend(active.iter().filter(|c| c[3] >= y1).map(|c| (c[0], c[2])));
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        free.clear();
        let mut x = rl;
        for &(l, r) in &spans {
            if l > x {
                free.push((x, l));
            }
            x = x.max(r);
        }
        if x < rr {
            free.push((x, rr));
        }
        // A stretch open above goes on down while this band has room for
        // it (a gutter between columns of cards, past the gaps beside
        // it); one that hasn't is done. What's left of the band's open
        // stretches starts new ones.
        going.clear();
        let mut k = 0;
        for &(fl, fr) in &free {
            let mut x = fl;
            while k < open.len() && open[k].0 < fr {
                let o = open[k];
                if fl <= o.0 && o.1 <= fr {
                    if o.0 > x {
                        going.push((x, o.0, y0));
                    }
                    going.push(o);
                    x = o.1;
                } else {
                    out.push(rect(o.0, o.2, o.1, y0));
                }
                k += 1;
            }
            if x < fr {
                going.push((x, fr, y0));
            }
        }
        out.extend(open[k..].iter().map(|o| rect(o.0, o.2, o.1, y0)));
        std::mem::swap(&mut open, &mut going);
    }
    out.extend(open.into_iter().map(|(l, r, top)| rect(l, top, r, rb)));
    out
}

#[cfg(test)]
mod cull_tests {
    use super::*;
    use crate::{hsla, solid_background};

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
        Bounds {
            origin: point(ScaledPixels(x), ScaledPixels(y)),
            size: size(ScaledPixels(w), ScaledPixels(h)),
        }
    }

    fn quad(order: u32, b: Bounds<ScaledPixels>, alpha: f32) -> Quad {
        Quad {
            order,
            bounds: b,
            content_mask: ContentMask {
                bounds: rect(0.0, 0.0, 4000.0, 4000.0),
            },
            background: solid_background(hsla(0.5, 0.5, 0.5, alpha)),
            ..Default::default()
        }
    }

    #[test]
    fn a_ground_under_others_is_cut_to_what_shows() {
        let mut s = Scene::default();
        // The window's ground, a bar across its top, a view's ground under.
        s.quads = vec![
            quad(0, rect(0.0, 0.0, 1000.0, 800.0), 1.0),
            quad(1, rect(0.0, 0.0, 1000.0, 50.0), 1.0),
            quad(2, rect(0.0, 50.0, 1000.0, 700.0), 1.0),
        ];
        s.cull_hidden_quads();
        // The ground is left only below the view (50 + 700 to 800).
        let grounds: Vec<_> = s.quads.iter().filter(|q| q.order == 0).collect();
        assert_eq!(grounds.len(), 1);
        let b = grounds[0].bounds;
        assert!(
            (b.origin.y.0 - 750.0).abs() < 0.02
                && (b.origin.y.0 + b.size.height.0 - 800.0).abs() < 0.02,
            "{b:?}"
        );
        assert_eq!(s.quads.len(), 3);
    }

    #[test]
    fn hidden_wholly_goes_and_see_through_covers_nothing() {
        let mut s = Scene::default();
        s.quads = vec![
            quad(0, rect(0.0, 0.0, 500.0, 500.0), 1.0),
            quad(1, rect(0.0, 0.0, 600.0, 600.0), 1.0),
        ];
        s.cull_hidden_quads();
        assert_eq!(s.quads.len(), 1);
        assert_eq!(s.quads[0].order, 1);
        // Over it half see-through: it stays whole.
        let mut s = Scene::default();
        s.quads = vec![
            quad(0, rect(0.0, 0.0, 500.0, 500.0), 1.0),
            quad(1, rect(0.0, 0.0, 600.0, 600.0), 0.5),
        ];
        s.cull_hidden_quads();
        assert_eq!(s.quads.len(), 2);
        assert_eq!(s.quads[0].bounds, rect(0.0, 0.0, 500.0, 500.0));
    }

    /// Notifications' history at 1.5×: the window's ground, the card
    /// paper over most of it, rounded cards on the paper.
    #[test]
    fn a_window_ground_under_a_scroll_views_paper() {
        let mut s = Scene::default();
        let mut quads = vec![quad(1, rect(0.0, 0.0, 3153.0, 2163.0), 1.0)];
        // (A menu bar and a toolbar's controls first, as painted: each
        // cuts the ground, before the paper's turn.)
        quads.push(quad(2, rect(0.0, 0.0, 3153.0, 39.0), 1.0));
        for k in 0..12 {
            quads.push(quad(
                2,
                rect(10.0 + k as f32 * 200.0, 50.0, 150.0, 32.0),
                1.0,
            ));
        }
        quads.push(quad(2, rect(2.0, 101.0, 3128.0, 2033.0), 1.0));
        for i in 0..40 {
            let mut card = quad(
                3,
                rect(
                    20.0 + (i % 5) as f32 * 600.0,
                    120.0 + (i / 5) as f32 * 250.0,
                    560.0,
                    230.0,
                ),
                1.0,
            );
            card.corner_radii = Corners::all(ScaledPixels(7.5));
            quads.push(card);
        }
        s.quads = quads;
        s.cull_hidden_quads();
        let screen = 3153.0 * 2163.0;
        let left = |order: u32| -> f32 {
            s.quads
                .iter()
                .filter(|q| q.order == order)
                .map(|q| q.bounds.size.width.0 * q.bounds.size.height.0)
                .sum()
        };
        assert!(
            left(1) < screen * 0.1,
            "the ground left: {:.2} screens",
            left(1) / screen
        );
        // The paper only between the cards and by their corners (the
        // bands between their corners are 0.75 of it), in a few pieces a
        // row of cards.
        let paper = 3128.0 * 2033.0;
        let bars = 12.0 * 150.0 * 32.0 + 3153.0 * 39.0;
        let pieces = s.quads.iter().filter(|q| q.order == 2).count() - 13;
        assert!(
            ((left(2) - bars) / paper - 0.25).abs() < 0.01,
            "the paper left: {:.3} of it",
            (left(2) - bars) / paper
        );
        assert!(pieces <= 8 * 8 + 2, "{pieces} pieces");
    }

    /// What's left and what covers it make the whole, without overlap,
    /// for rectangles anywhere (a grid of points checked).
    #[test]
    fn uncovered_is_the_rest_exactly() {
        let mut seed = 12345u64;
        let mut rand = |n: f32| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as f32 / (1u64 << 31) as f32 * n
        };
        for _ in 0..200 {
            let r = rect(
                rand(50.0).floor(),
                rand(50.0).floor(),
                200.0 + rand(400.0).floor(),
                200.0 + rand(400.0).floor(),
            );
            let covers: Vec<_> = (0..(rand(30.0) as usize))
                .map(|_| {
                    rect(
                        rand(700.0).floor(),
                        rand(700.0).floor(),
                        20.0 + rand(250.0).floor(),
                        20.0 + rand(250.0).floor(),
                    )
                })
                .collect();
            let least = r.size.width.0 * r.size.height.0 / 1000.0;
            let left = uncovered(r, &covers);
            for gx in 0..60 {
                for gy in 0..60 {
                    let p = point(
                        ScaledPixels(r.origin.x.0 + (gx as f32 + 0.37) * r.size.width.0 / 60.0),
                        ScaledPixels(r.origin.y.0 + (gy as f32 + 0.41) * r.size.height.0 / 60.0),
                    );
                    // (A cover too small to count may leave what's under it.)
                    let under = covers
                        .iter()
                        .filter(|c| {
                            c.intersect(&r).size.width.0 * c.intersect(&r).size.height.0 >= least
                        })
                        .any(|c| c.contains(&p));
                    let pieces = left.iter().filter(|b| b.contains(&p)).count();
                    assert_eq!(
                        pieces,
                        if under { 0 } else { 1 },
                        "{p:?} in {r:?} under {covers:?}: {left:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_rounded_cover_hides_its_middle() {
        let mut s = Scene::default();
        let mut over = quad(1, rect(0.0, 0.0, 600.0, 600.0), 1.0);
        over.corner_radii = Corners::all(ScaledPixels(4.0));
        s.quads = vec![quad(0, rect(0.0, 0.0, 500.0, 500.0), 1.0), over];
        s.cull_hidden_quads();
        // Left: above its corners' band, and a pixel at its side.
        let under: Vec<_> = s
            .quads
            .iter()
            .filter(|q| q.order == 0)
            .map(|q| q.bounds)
            .collect();
        let area: f32 = under.iter().map(|b| b.size.width.0 * b.size.height.0).sum();
        assert!(
            (area - (500.0 * 5.0 + 1.0 * 495.0)).abs() < 1.0,
            "{under:?}"
        );
        // A see-through border, or a dashed one, hides nothing.
        for (alpha, style) in [(0.5, BorderStyle::Solid), (1.0, BorderStyle::Dashed)] {
            let mut s = Scene::default();
            let mut over = quad(1, rect(0.0, 0.0, 600.0, 600.0), 1.0);
            over.corner_radii = Corners::all(ScaledPixels(4.0));
            over.border_widths = Edges::all(ScaledPixels(1.0));
            over.border_color = hsla(0.0, 0.0, 0.0, alpha);
            over.border_style = style;
            s.quads = vec![quad(0, rect(0.0, 0.0, 500.0, 500.0), 1.0), over];
            s.cull_hidden_quads();
            assert_eq!(s.quads.len(), 2);
            assert_eq!(s.quads[0].bounds, rect(0.0, 0.0, 500.0, 500.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AtlasTextureId, AtlasTextureKind, DevicePixels, TileId, TransformationMatrix};

    fn sprite(x: f32, texture: u32, id: u32) -> MonochromeSprite {
        let b = Bounds {
            origin: point(ScaledPixels(x), ScaledPixels(0.0)),
            size: size(ScaledPixels(8.0), ScaledPixels(8.0)),
        };
        MonochromeSprite {
            order: 0,
            coverage: 0,
            bounds: b,
            content_mask: ContentMask {
                bounds: Bounds {
                    origin: point(ScaledPixels(-1e4), ScaledPixels(-1e4)),
                    size: size(ScaledPixels(2e4), ScaledPixels(2e4)),
                },
            },
            color: Hsla::default(),
            tile: AtlasTile {
                texture_id: AtlasTextureId {
                    index: texture,
                    kind: AtlasTextureKind::Monochrome,
                },
                tile_id: TileId(id),
                padding: 0,
                bounds: Bounds {
                    origin: point(DevicePixels(0), DevicePixels(0)),
                    size: size(DevicePixels(8), DevicePixels(8)),
                },
            },
            transformation: TransformationMatrix::unit(),
        }
    }

    /// Olo: glyphs go by runs (`mono_runs`), in the order sorting each
    /// sprite by order and texture gave: lines in layers, overlapping or
    /// not, glyphs of two textures interleaved, and sprites alone.
    #[test]
    fn sprites_in_runs_keep_the_order_sorting_each_gave() {
        let mut scene = Scene::default();
        let mut painted = Vec::new();
        let mut id = 0;
        let mut next = |x: f32, t: u32| {
            id += 1;
            sprite(x, t, id)
        };
        for line in 0..6 {
            let x = (line % 3) as f32 * 30.0;
            let bounds = Bounds {
                origin: point(ScaledPixels(x), ScaledPixels(0.0)),
                size: size(ScaledPixels(40.0), ScaledPixels(8.0)),
            };
            scene.push_layer(bounds);
            let glyphs: Vec<_> = (0..5)
                .map(|g| next(x + g as f32, if line == 4 { g % 2 } else { 0 }))
                .collect();
            scene.insert_sprites(glyphs.clone());
            painted.extend(
                glyphs
                    .into_iter()
                    .map(|g| (g, *scene.layer_stack.last().unwrap())),
            );
            scene.pop_layer();
            let alone = next(x + 3.0, 1);
            scene.insert_primitive(alone.clone());
            let order = scene.mono_staging.last().unwrap().order;
            painted.push((alone, order));
        }
        scene.finish();
        // (Each as it went in, with the order the scene gave it, sorted
        // by order and texture the way finish sorted every sprite.)
        let mut expected: Vec<_> = painted
            .into_iter()
            .map(|(s, order)| (order, texture_key(&s.tile), s.tile.tile_id))
            .collect();
        expected.sort_by_key(|(order, texture, _)| (*order, *texture));
        let got: Vec<_> = scene
            .monochrome_sprites
            .iter()
            .map(|s| (s.order, texture_key(&s.tile), s.tile.tile_id))
            .collect();
        assert_eq!(got, expected);
    }

    fn mask(x: f32, y: f32, w: f32, h: f32) -> ContentMask<ScaledPixels> {
        ContentMask {
            bounds: Bounds {
                origin: point(ScaledPixels(x), ScaledPixels(y)),
                size: size(ScaledPixels(w), ScaledPixels(h)),
            },
        }
    }

    /// Olo: a list's row, its cells clipped each to its own, moves with
    /// its clips (scrolled up or down) unless the list's clip cut a cell
    /// that way, and is cut by the list's clip again where it lands.
    #[test]
    fn a_row_moves_with_its_cells_clips() {
        let list = mask(0.0, 0.0, 400.0, 300.0);
        let mut scene = Scene::default();
        // A row inside the list: two cells, the first at its left edge.
        for (x, w) in [(0.0, 100.0), (100.0, 300.0)] {
            let mut glyph = sprite(x + 4.0, 0, 1);
            glyph.bounds.origin.y = ScaledPixels(102.0);
            glyph.content_mask = mask(x, 100.0, w, 20.0);
            scene.insert_sprites([glyph]);
        }
        let n = scene.paint_operations.len();
        assert_eq!(scene.movable(0..n, &list), Movable { x: false, y: true });
        // Moved up 95 pixels: still inside, nothing cut.
        let mut next = Scene::default();
        assert!(!next.replay_moved(
            0..n,
            &scene,
            point(ScaledPixels(0.0), ScaledPixels(-95.0)),
            &list
        ));
        let moved: Vec<_> = next
            .mono_staging
            .iter()
            .map(|g| (g.bounds.origin.y.0, g.content_mask.clone()))
            .collect();
        assert_eq!(
            moved,
            vec![
                (7.0, mask(0.0, 5.0, 100.0, 20.0)),
                (7.0, mask(100.0, 5.0, 300.0, 20.0))
            ]
        );
        // Moved up 105: its top 5 cut by the list's clip (and so not to be
        // moved again till it's painted afresh).
        let mut next = Scene::default();
        assert!(
            next.replay_moved(
                0..n,
                &scene,
                point(ScaledPixels(0.0), ScaledPixels(-105.0)),
                &list
            ),
            "cut where it landed"
        );
        let moved: Vec<_> = next
            .mono_staging
            .iter()
            .map(|g| (g.bounds.origin.y.0, g.content_mask.clone()))
            .collect();
        assert_eq!(
            moved,
            vec![
                (-3.0, mask(0.0, 0.0, 100.0, 15.0)),
                (-3.0, mask(100.0, 0.0, 300.0, 15.0))
            ]
        );
        // A row the list's clip cut (at its top) is drawn again instead.
        let mut cut = Scene::default();
        let mut glyph = sprite(4.0, 0, 1);
        glyph.content_mask = mask(0.0, 0.0, 100.0, 12.0);
        cut.insert_sprites([glyph]);
        let n = cut.paint_operations.len();
        assert_eq!(cut.movable(0..n, &list), Movable { x: false, y: false });
    }
}
