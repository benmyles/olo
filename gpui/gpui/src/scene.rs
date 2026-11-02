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
    pub(crate) monochrome_sprites: Vec<MonochromeSprite>,
    pub(crate) polychrome_sprites: Vec<PolychromeSprite>,
    pub(crate) image_triangles: Vec<ImageTriangle>,
    pub(crate) surfaces: Vec<PaintSurface>,
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
        self.polychrome_sprites.clear();
        self.image_triangles.clear();
        self.surfaces.clear();
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
                self.monochrome_sprites.push(sprite.clone());
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
        self.paint_operations.push(PaintOperation::Segments(segments));
    }

    /// Olo: many glyphs' sprites at once (`Window::paint_glyphs`), as
    /// [`Scene::insert_segments`]: a page of text is tens of thousands.
    pub fn insert_sprites(&mut self, mut sprites: Vec<MonochromeSprite>) {
        let layer = self.layer_stack.last().copied();
        sprites.retain_mut(|sprite| {
            let clipped = sprite.bounds.intersect(&sprite.content_mask.bounds);
            if clipped.is_empty() {
                return false;
            }
            sprite.order = layer.unwrap_or_else(|| self.primitive_bounds.insert(clipped));
            true
        });
        if sprites.is_empty() {
            return;
        }
        self.monochrome_sprites.extend_from_slice(&sprites);
        self.paint_operations.push(PaintOperation::Sprites(sprites));
    }

    pub fn replay(&mut self, range: Range<usize>, prev_scene: &Scene) {
        for operation in &prev_scene.paint_operations[range] {
            match operation {
                PaintOperation::Primitive(primitive) => self.insert_primitive(primitive.clone()),
                PaintOperation::Segments(segments) => self.insert_segments(segments.clone()),
                PaintOperation::Sprites(sprites) => self.insert_sprites(sprites.clone()),
                PaintOperation::StartLayer(bounds) => self.push_layer(*bounds),
                PaintOperation::EndLayer => self.pop_layer(),
            }
        }
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
        let texture = |t: &crate::AtlasTile| (t.texture_id.kind as u8, t.texture_id.index);
        self.monochrome_sprites
            .sort_by_key(|sprite| (sprite.order, texture(&sprite.tile)));
        self.polychrome_sprites
            .sort_by_key(|sprite| (sprite.order, texture(&sprite.tile)));
        self.image_triangles.sort_by_key(|triangle| triangle.order);
        self.surfaces.sort_by_key(|surface| surface.order);
    }

    /// Olo: a big quad wholly under opaque quads drawn after it isn't
    /// drawn, and a plain one mostly under them only where it shows (a
    /// window's ground under its views' own grounds filled the window
    /// two or three times over a frame, the GPU's biggest cost).
    fn cull_hidden_quads(&mut self) {
        // (Small quads aren't worth it: what one costs is its pixels.)
        const BIG: f32 = 256.0 * 256.0;
        const MOST_PIECES: usize = 8;
        let area = |b: &Bounds<ScaledPixels>| b.size.width.0.max(0.0) * b.size.height.0.max(0.0);
        let shown = |q: &Quad| q.bounds.intersect(&q.content_mask.bounds);
        // Drawn over all that's under it, edge to edge: one colour, no
        // round corners or border (whose edges are anti-aliased).
        let plain = |q: &Quad| {
            q.background.tag == crate::BackgroundTag::Solid
                && q.corner_radii == Corners::default()
                && q.border_widths == Edges::default()
        };
        let covers = |q: &Quad| plain(q) && q.background.solid.a >= 1.0;
        let mut i = 0;
        while i < self.quads.len() {
            let a = shown(&self.quads[i]);
            if area(&a) < BIG {
                i += 1;
                continue;
            }
            let mut left = vec![a];
            for b in &self.quads[i + 1..] {
                if !covers(b) {
                    continue;
                }
                // (A hair in from its edges: what's cut stays drawn up to
                // them, never a gap.)
                let c = shown(b);
                let c = Bounds { origin: point(ScaledPixels(c.origin.x.0 + 0.01), ScaledPixels(c.origin.y.0 + 0.01)), size: size(ScaledPixels(c.size.width.0 - 0.02), ScaledPixels(c.size.height.0 - 0.02)) };
                if area(&c) <= 0.0 || !left.iter().any(|r| r.intersects(&c)) {
                    continue;
                }
                // (Less the slivers that hair leaves where its edge is one
                // of the quad's: it draws those pixels itself.)
                left = left.into_iter().flat_map(|r| subtract(r, c)).filter(|r| r.size.width.0 >= 0.05 && r.size.height.0 >= 0.05).collect();
                if left.is_empty() || left.len() > MOST_PIECES {
                    break;
                }
            }
            if left.is_empty() {
                self.quads.remove(i);
                continue;
            }
            let kept: f32 = left.iter().map(area).sum();
            if left.len() <= MOST_PIECES && plain(&self.quads[i]) && kept < 0.75 * area(&a) {
                let q = self.quads[i].clone();
                let pieces: Vec<Quad> = left.into_iter().filter(|r| area(r) > 0.0).map(|r| Quad { bounds: r, ..q.clone() }).collect();
                let n = pieces.len();
                self.quads.splice(i..i + 1, pieces);
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
}

pub(crate) enum PaintOperation {
    Primitive(Primitive),
    /// Olo: a polyline's pieces (`Scene::insert_segments`).
    Segments(Vec<Segment>),
    /// Olo: a run of glyphs (`Scene::insert_sprites`).
    Sprites(Vec<MonochromeSprite>),
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
                Some(PrimitiveBatch::Segments(&self.segments[segments_start..segments_end]))
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
                Some(PrimitiveBatch::Sectors(&self.sectors[sectors_start..sectors_end]))
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
    #[cfg(target_os = "macos")]
    pub image_buffer: core_video::pixel_buffer::CVPixelBuffer,
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

/// Olo: `r` less `c`: what of it is outside `c`, in up to four rectangles.
fn subtract(r: Bounds<ScaledPixels>, c: Bounds<ScaledPixels>) -> Vec<Bounds<ScaledPixels>> {
    let (rl, rt, rr, rb) = (r.origin.x.0, r.origin.y.0, r.origin.x.0 + r.size.width.0, r.origin.y.0 + r.size.height.0);
    let (cl, ct, cr, cb) = (c.origin.x.0.max(rl), c.origin.y.0.max(rt), (c.origin.x.0 + c.size.width.0).min(rr), (c.origin.y.0 + c.size.height.0).min(rb));
    if cl >= cr || ct >= cb {
        return vec![r];
    }
    let rect = |l: f32, t: f32, r: f32, b: f32| Bounds { origin: point(ScaledPixels(l), ScaledPixels(t)), size: size(ScaledPixels(r - l), ScaledPixels(b - t)) };
    let mut out = Vec::with_capacity(4);
    if ct > rt {
        out.push(rect(rl, rt, rr, ct));
    }
    if cb < rb {
        out.push(rect(rl, cb, rr, rb));
    }
    if cl > rl {
        out.push(rect(rl, ct, cl, cb));
    }
    if cr < rr {
        out.push(rect(cr, ct, rr, cb));
    }
    out
}

#[cfg(test)]
mod cull_tests {
    use super::*;
    use crate::{hsla, solid_background};

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
        Bounds { origin: point(ScaledPixels(x), ScaledPixels(y)), size: size(ScaledPixels(w), ScaledPixels(h)) }
    }

    fn quad(order: u32, b: Bounds<ScaledPixels>, alpha: f32) -> Quad {
        Quad { order, bounds: b, content_mask: ContentMask { bounds: rect(0.0, 0.0, 4000.0, 4000.0) }, background: solid_background(hsla(0.5, 0.5, 0.5, alpha)), ..Default::default() }
    }

    #[test]
    fn a_ground_under_others_is_cut_to_what_shows() {
        let mut s = Scene::default();
        // The window's ground, a bar across its top, a view's ground under.
        s.quads = vec![quad(0, rect(0.0, 0.0, 1000.0, 800.0), 1.0), quad(1, rect(0.0, 0.0, 1000.0, 50.0), 1.0), quad(2, rect(0.0, 50.0, 1000.0, 700.0), 1.0)];
        s.cull_hidden_quads();
        // The ground is left only below the view (50 + 700 to 800).
        let grounds: Vec<_> = s.quads.iter().filter(|q| q.order == 0).collect();
        assert_eq!(grounds.len(), 1);
        let b = grounds[0].bounds;
        assert!((b.origin.y.0 - 750.0).abs() < 0.02 && (b.origin.y.0 + b.size.height.0 - 800.0).abs() < 0.02, "{b:?}");
        assert_eq!(s.quads.len(), 3);
    }

    #[test]
    fn hidden_wholly_goes_and_see_through_covers_nothing() {
        let mut s = Scene::default();
        s.quads = vec![quad(0, rect(0.0, 0.0, 500.0, 500.0), 1.0), quad(1, rect(0.0, 0.0, 600.0, 600.0), 1.0)];
        s.cull_hidden_quads();
        assert_eq!(s.quads.len(), 1);
        assert_eq!(s.quads[0].order, 1);
        // Over it half see-through: it stays whole.
        let mut s = Scene::default();
        s.quads = vec![quad(0, rect(0.0, 0.0, 500.0, 500.0), 1.0), quad(1, rect(0.0, 0.0, 600.0, 600.0), 0.5)];
        s.cull_hidden_quads();
        assert_eq!(s.quads.len(), 2);
        assert_eq!(s.quads[0].bounds, rect(0.0, 0.0, 500.0, 500.0));
    }

    #[test]
    fn a_rounded_cover_hides_nothing() {
        let mut s = Scene::default();
        let mut over = quad(1, rect(0.0, 0.0, 600.0, 600.0), 1.0);
        over.corner_radii = Corners::all(ScaledPixels(4.0));
        s.quads = vec![quad(0, rect(0.0, 0.0, 500.0, 500.0), 1.0), over];
        s.cull_hidden_quads();
        assert_eq!(s.quads.len(), 2);
    }
}
