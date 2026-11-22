// todo("windows"): remove
#![cfg_attr(windows, allow(dead_code))]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AtlasTextureId, AtlasTile, Background, Bounds, ContentMask, Corners, Edges, Hsla, Pixels, Point, Radians, ScaledPixels, Size, bounds_tree::BoundsTree, point, size};
use std::{
    fmt::Debug,
    iter::Peekable,
    ops::{Add, Range, Sub},
    slice,
    sync::Arc,
};

#[allow(non_camel_case_types, unused)]
pub(crate) type PathVertex_ScaledPixels = PathVertex<ScaledPixels>;

pub(crate) type DrawOrder = u32;

/// What a scene's things are: a recording's orders each hold things
/// of one kind (`Scene::next_order`).
#[derive(Clone, Copy, Default, PartialEq)]
enum Kind {
    #[default]
    None,
    Shadow,
    Quad,
    Path,
    Underline,
    Segment,
    Sector,
    Mono,
    Poly,
    ImageTriangle,
    Surface,
}

/// One of a recording's orders: the kind of thing at it, what those
/// things reach, together, and the last of their textures as an order's
/// sprites are drawn (texture by texture; (0, 0) for what has none).
struct Order {
    kind: Kind,
    reach: Bounds<ScaledPixels>,
    last_texture: (u8, u32),
}

#[derive(Default)]
pub(crate) struct Scene {
    primitive_bounds: BoundsTree<ScaledPixels>,
    layer_stack: Vec<DrawOrder>,
    pub(crate) shadows: Vec<Shadow>,
    pub(crate) quads: Vec<Quad>,
    pub(crate) paths: Vec<Path<ScaledPixels>>,
    pub(crate) underlines: Vec<Underline>,
    pub(crate) segments: Vec<Segment>,
    pub(crate) sectors: Vec<Sector>,
    /// Olo: glyph sprites as painted, in runs of one order and texture
    /// (`mono_runs`): a line's glyphs share an order, so `finish` sorts a
    /// frame's lines, not each of its tens of thousands of glyphs, and a
    /// kept drawing (once sealed: `sealed`) is drawn from here. A frame
    /// draws its runs in their sorted order ([`Glyphs`]), from wherever
    /// they are, never copied into one sorted list first.
    mono_staging: Vec<MonochromeSprite>,
    /// Olo: a kept drawing's glyphs (`mono_staging`, once it's made,
    /// `seal`), shared with the frames that draw it: a frame's run of
    /// them says where they go, and the renderer puts each there as it
    /// copies it for the GPU (a frame never copies a drawing's glyphs
    /// into a list of its own: a window of text is 300 KB a frame).
    sealed: Option<Arc<Vec<MonochromeSprite>>>,
    /// Olo: how many things were left out, clipped away whole.
    pub(crate) culled: usize,
    mono_runs: Vec<SpriteRun>,
    pub(crate) polychrome_sprites: Vec<PolychromeSprite>,
    pub(crate) image_triangles: Vec<ImageTriangle>,
    pub(crate) surfaces: Vec<PaintSurface>,
    /// The pictures it shows, held while it does: a picture's tiles in
    /// the atlas go when the picture does (`RenderImage`'s drop), which
    /// is never while a frame or a kept drawing still draws from them.
    pub(crate) images: Vec<std::sync::Arc<crate::RenderImage>>,
    /// How many things it holds.
    count: usize,
    /// A view's drawing being kept (a display list), not a frame. What
    /// it paints is ordered among itself as it comes, with no looking at
    /// what overlaps what: things painted one after another of one kind
    /// share an order (and are drawn in the order they came), and the
    /// order goes up one at each change of kind. A frame that takes it
    /// ([`Scene::append`]) puts all of it above whatever it overlaps
    /// there, at once.
    recording: bool,
    /// (A recording's) each of its orders (`next_order`).
    orders: Vec<Order>,
    /// What a recording painted reaches (with each thing's own clip).
    extent: Option<Bounds<ScaledPixels>>,
}

impl Scene {
    /// A scene that keeps a view's drawing (see `recording`).
    pub fn recording() -> Self {
        Scene { recording: true, ..Default::default() }
    }

    /// Olo: where this recording differs from `was`, the one it replaces
    /// in the same place (a text area typed in: its line and its caret,
    /// not all of it): of each kind of thing, around what isn't the same
    /// at either end of that kind's list, where it was and where it is
    /// (what each shows: its bounds in its clip). Into `out`, in the
    /// coordinates they were painted in; false (and `out` as it was) when
    /// it can't tell where: among the things it only compares whole
    /// (paths, sectors, pictures' triangles, videos, shadows) something
    /// isn't as it was. Where those are all the same, as in a drawing made
    /// again of what it already showed (a view whose body ran and painted
    /// the same chart, the same shadowed card), the rest is compared and
    /// `out` is left as it was for a drawing that is the same throughout:
    /// nothing to show, so no frame. (It used to say it couldn't tell
    /// whenever there was a path or a shadow at all, and every such
    /// drawing made again was a frame.)
    pub(crate) fn changed_since(&self, was: &Scene, out: &mut Vec<Bounds<ScaledPixels>>) -> bool {
        if self.shadows != was.shadows
            || self.sectors != was.sectors
            || self.image_triangles != was.image_triangles
            || !same_paths(&self.paths, &was.paths)
            || !same_surfaces(&self.surfaces, &was.surfaces)
        {
            return false;
        }
        // (`same`: whether two stretches as long as each other are; the
        // ends are passed over a block at a time, then one by one.)
        fn kind<T>(a: &[T], b: &[T], same: impl Fn(&[T], &[T]) -> bool, shows: impl Fn(&T) -> Bounds<ScaledPixels>, out: &mut Vec<Bounds<ScaledPixels>>) {
            const BLOCK: usize = 32;
            let n = a.len().min(b.len());
            let mut start = 0;
            while start + BLOCK <= n && same(&a[start..start + BLOCK], &b[start..start + BLOCK]) {
                start += BLOCK;
            }
            while start < n && same(&a[start..start + 1], &b[start..start + 1]) {
                start += 1;
            }
            let (mut end_a, mut end_b) = (a.len(), b.len());
            while end_a >= start + BLOCK && end_b >= start + BLOCK && same(&a[end_a - BLOCK..end_a], &b[end_b - BLOCK..end_b]) {
                end_a -= BLOCK;
                end_b -= BLOCK;
            }
            while end_a > start && end_b > start && same(&a[end_a - 1..end_a], &b[end_b - 1..end_b]) {
                end_a -= 1;
                end_b -= 1;
            }
            let around = a[start..end_a].iter().chain(&b[start..end_b]).map(shows).filter(|r| !r.is_empty()).reduce(|r, s| r.union(&s));
            out.extend(around);
        }
        fn equal<T: PartialEq>(a: &[T], b: &[T]) -> bool {
            a == b
        }
        let len = out.len();
        kind(&was.quads, &self.quads, equal, |q| q.bounds.intersect(&q.content_mask.bounds), out);
        kind(&was.underlines, &self.underlines, equal, |u| u.bounds.intersect(&u.content_mask.bounds), out);
        kind(&was.segments, &self.segments, same_sprites, |g| g.bounds.intersect(&g.content_mask.bounds), out);
        self.glyphs_changed_since(was, out);
        kind(&was.polychrome_sprites, &self.polychrome_sprites, equal, |p| p.bounds.intersect(&p.content_mask.bounds), out);
        // (A picture let go and another in its tile: not the same.)
        if was.images.len() != self.images.len() || was.images.iter().zip(&self.images).any(|(a, b)| !std::sync::Arc::ptr_eq(a, b)) {
            out.truncate(len);
            return false;
        }
        true
    }

    /// The glyphs' part of [`Scene::changed_since`]: line by line first,
    /// as their runs were made (a page of text is a thousand and more
    /// glyphs, its lines made again from what was kept of each, but for
    /// the line that changed): what's the same at either end passed
    /// over, the rest glyph by glyph, by their bytes as they are in
    /// place.
    ///
    /// None is written out to be compared. A line put in whole
    /// (`kept`) holds its glyphs from the line's own corner, and each is
    /// put in place as it is looked at; what changed is bounded by the
    /// first and the last glyph that differ, and for a line all of whose
    /// glyphs lie between them it is the line's reach, which is exactly
    /// what they show (a line of one texture, at whole device pixels:
    /// `Placed::shows_onto`), with no glyph of it read.
    /// So a screen of a terminal's text scrolled a line (each line where
    /// the next one was) costs a look at its first glyph and its last,
    /// and its lines' reaches. (Every line between the ends was copied
    /// out of its kept run and put in place, twice a screen of glyphs
    /// written and then gone through at every line printed: a fifth of
    /// what printing a line cost.) What counts as the same, and what is
    /// said to have changed, are what the copies gave: the tests hold
    /// the two ways to each other.
    fn glyphs_changed_since(&self, was: &Scene, out: &mut Vec<Bounds<ScaledPixels>>) {
        let (ra, rb) = (&was.mono_runs, &self.mono_runs);
        let same_run = |x: &SpriteRun, y: &SpriteRun| {
            x.order == y.order
                && x.texture == y.texture
                && x.range.len() == y.range.len()
                && x.reach == y.reach
                && (x.made.is_some() && x.made == y.made
                    // (Two lines put in whole, put the same way, their kept
                    // glyphs the same: the same in place. Else in place,
                    // glyph by glyph.)
                    || x.kept_as(y)
                    || x.placed(was).zip(y.placed(self)).all(|(g, h)| same_sprites(slice::from_ref(&g), slice::from_ref(&h))))
        };
        let mut first = 0;
        while first < ra.len() && first < rb.len() && same_run(&ra[first], &rb[first]) {
            first += 1;
        }
        let (mut end_a, mut end_b) = (ra.len(), rb.len());
        while end_a > first && end_b > first && same_run(&ra[end_a - 1], &rb[end_b - 1]) {
            end_a -= 1;
            end_b -= 1;
        }
        // The runs between, each side's as one line of glyphs in place:
        // those the same at its start and at its end passed over.
        let (a, b) = (Placed { scene: was, runs: &ra[first..end_a] }, Placed { scene: self, runs: &rb[first..end_b] });
        let same = |(g, h): &(MonochromeSprite, MonochromeSprite)| same_sprites(slice::from_ref(g), slice::from_ref(h));
        let (len_a, len_b) = (a.len(), b.len());
        let start = a.forward().zip(b.forward()).take_while(same).count();
        let end = a.backward().zip(b.backward()).take(len_a.min(len_b) - start).take_while(same).count();
        // (One turned or moved by its matrix shows anywhere in its clip.)
        let mono = |m: &MonochromeSprite| if m.transformation == TransformationMatrix::unit() { m.bounds.intersect(&m.content_mask.bounds) } else { m.content_mask.bounds };
        // (What was, then what is, in one go: as the copies were gone through.)
        let mut around = None;
        a.shows_onto(start..len_a - end, mono, &mut around);
        b.shows_onto(start..len_b - end, mono, &mut around);
        out.extend(around);
    }

    /// The glyphs' part of [`Scene::changed_since`] as it was done before
    /// [`Scene::glyphs_changed_since`]: every run between the ends
    /// written out, in place, and the copies gone through. Kept for the
    /// tests, which hold the two to each other.
    #[cfg(test)]
    fn glyphs_changed_by_copies(&self, was: &Scene, out: &mut Vec<Bounds<ScaledPixels>>) {
        let (ra, rb) = (&was.mono_runs, &self.mono_runs);
        use std::borrow::Cow;
        fn glyphs<'a>(s: &'a Scene, runs: &[SpriteRun]) -> Cow<'a, [MonochromeSprite]> {
            match (runs.first(), runs.last()) {
                (Some(first), Some(last)) if runs.iter().all(|r| r.kept.is_none()) => Cow::Borrowed(&s.staged()[first.range.start as usize..last.range.end as usize]),
                _ => {
                    let mut all = Vec::with_capacity(runs.iter().map(|r| r.range.len()).sum());
                    for r in runs {
                        r.glyphs_onto(s, &mut all);
                    }
                    Cow::Owned(all)
                }
            }
        }
        let same_run = |x: &SpriteRun, y: &SpriteRun| {
            x.order == y.order
                && x.texture == y.texture
                && x.range.len() == y.range.len()
                && x.reach == y.reach
                && (x.made.is_some() && x.made == y.made || same_sprites(&glyphs(was, slice::from_ref(x)), &glyphs(self, slice::from_ref(y))))
        };
        let mut first = 0;
        while first < ra.len() && first < rb.len() && same_run(&ra[first], &rb[first]) {
            first += 1;
        }
        let (mut end_a, mut end_b) = (ra.len(), rb.len());
        while end_a > first && end_b > first && same_run(&ra[end_a - 1], &rb[end_b - 1]) {
            end_a -= 1;
            end_b -= 1;
        }
        let (a, b) = (glyphs(was, &ra[first..end_a]), glyphs(self, &rb[first..end_b]));
        let same = |i: usize, j: usize| same_sprites(&a[i..i + 1], &b[j..j + 1]);
        let n = a.len().min(b.len());
        let mut start = 0;
        while start < n && same(start, start) {
            start += 1;
        }
        let (mut ea, mut eb) = (a.len(), b.len());
        while ea > start && eb > start && same(ea - 1, eb - 1) {
            ea -= 1;
            eb -= 1;
        }
        let mono = |m: &MonochromeSprite| if m.transformation == TransformationMatrix::unit() { m.bounds.intersect(&m.content_mask.bounds) } else { m.content_mask.bounds };
        out.extend(a[start..ea].iter().chain(&b[start..eb]).map(mono).filter(|r| !r.is_empty()).reduce(|r, s| r.union(&s)));
    }

    /// Room for what `last` held, in a scene about to record the same
    /// view again: its lists are made once, the size they were, where
    /// they'd be grown from nothing as it paints (a row of text's
    /// glyphs: four, eight, sixteen, thirty-two, sixty-four, each a new
    /// list and a copy). A drawing that's kept can't hand its lists
    /// back to be drawn into again: the frame showing still holds it.
    pub(crate) fn reserve_like(&mut self, last: &Scene) {
        self.paths.reserve(last.paths.len());
        self.shadows.reserve(last.shadows.len());
        self.quads.reserve(last.quads.len());
        self.underlines.reserve(last.underlines.len());
        self.segments.reserve(last.segments.len());
        self.sectors.reserve(last.sectors.len());
        self.mono_staging.reserve(last.staged().len());
        self.mono_runs.reserve(last.mono_runs.len());
        self.polychrome_sprites.reserve(last.polychrome_sprites.len());
        self.image_triangles.reserve(last.image_triangles.len());
        self.surfaces.reserve(last.surfaces.len());
        self.images.reserve(last.images.len());
    }

    /// What a recording painted reaches, where it was painted.
    pub fn extent(&self) -> Bounds<ScaledPixels> {
        self.extent.unwrap_or_default()
    }

    /// Whether anything was painted.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Whether it keeps a view's drawing (see `recording`), not a frame.
    pub(crate) fn is_recording(&self) -> bool {
        self.recording
    }

    fn reach(&mut self, bounds: Bounds<ScaledPixels>) {
        self.extent = Some(match self.extent {
            Some(extent) => extent.union(&bounds),
            None => bounds,
        });
    }

    pub fn clear(&mut self) {
        self.extent = None;
        self.culled = 0;
        self.count = 0;
        self.orders.clear();
        self.primitive_bounds.clear();
        self.layer_stack.clear();
        self.paths.clear();
        self.shadows.clear();
        self.quads.clear();
        self.underlines.clear();
        self.segments.clear();
        self.sectors.clear();
        self.mono_staging.clear();
        self.sealed = None;
        self.mono_runs.clear();
        self.polychrome_sprites.clear();
        self.image_triangles.clear();
        self.surfaces.clear();
        self.images.clear();
    }

    /// How many things it holds.
    pub fn len(&self) -> usize {
        self.count
    }

    /// How many glyphs (and shapes' coverage tiles) a frame draws.
    pub(crate) fn glyph_count(&self) -> usize {
        self.mono_runs.iter().map(|run| run.range.len()).sum()
    }

    /// Whether a frame draws glyphs from atlas textures of `kind`.
    pub(crate) fn has_glyphs_of(&self, kind: crate::AtlasTextureKind) -> bool {
        self.mono_runs.iter().any(|run| run.texture.0 == kind as u8)
    }

    /// Its glyphs as painted: a kept drawing's shared list once made.
    fn staged(&self) -> &[MonochromeSprite] {
        self.sealed.as_deref().map_or(&self.mono_staging[..], |list| &list[..])
    }

    /// A drawing, made: its glyphs shared from now on (nothing copied:
    /// the list moves behind an `Arc`) with the frames that draw it.
    pub(crate) fn seal(&mut self) {
        if self.recording && self.sealed.is_none() {
            self.sealed = Some(Arc::new(std::mem::take(&mut self.mono_staging)));
        }
    }

    /// A finished frame's glyphs, in the order they're drawn (each at
    /// its run's order, as the GPU's copy never needs it).
    #[cfg(test)]
    pub(crate) fn drawn_glyphs(&self) -> Vec<MonochromeSprite> {
        let mut out = Vec::new();
        for b in self.batches() {
            if let PrimitiveBatch::MonochromeSprites { sprites, .. } = b {
                let mut orders = sprites.runs.iter().flat_map(|run| std::iter::repeat_n(run.order, run.range.len()));
                sprites.pieces(|piece| match piece {
                    Piece::AsIs(list) => out.extend(list.iter().map(|g| MonochromeSprite { order: orders.next().unwrap(), ..g.clone() })),
                    Piece::One(g) => out.push(MonochromeSprite { order: orders.next().unwrap(), ..g }),
                });
            }
        }
        out
    }

    /// How many orders a recording uses (its own run from zero).
    fn depth(&self) -> DrawOrder {
        if self.count == 0 { 0 } else { self.orders.len() as DrawOrder }
    }

    /// A recording's order for a thing of `kind` painted now, reaching
    /// `bounds` (sprites: of textures from `textures.0` to `.1`).
    ///
    /// Things at one order are drawn together, a kind at a time, so an
    /// order is a draw: the fewer the better. What matters is only that
    /// a thing is drawn after everything painted before it that it's
    /// over. So a thing goes to the lowest order of its own kind that's
    /// above everything it may be over (each order's things' reach,
    /// together, is kept: over none of an order's reach, it's over none
    /// of its things), a new order only when there's none. And it goes
    /// *with* what it's over, in its order, where that's drawn after it
    /// anyway: things of a kind are drawn as they were painted, and
    /// sprites texture by texture, then as painted.
    ///
    /// A list drawn row by row (an icon, a name, an arrow; an icon, a
    /// name, an arrow) is three orders so, whatever its length: with a
    /// new order at each change of kind, as it was, it was three a row,
    /// each a draw of its own (a Tracker window of 20 rows: 74 draws a
    /// frame, a quarter of the frame's time in the GPU's driver).
    fn next_order(&mut self, kind: Kind, textures: ((u8, u32), (u8, u32)), bounds: Bounds<ScaledPixels>) -> DrawOrder {
        // The highest order with something this may be over.
        let over = self.orders.iter().rposition(|order| order.reach.intersects(&bounds));
        let from = match over {
            // (With it: of its kind, and drawn after all of it.)
            Some(over) if self.orders[over].kind == kind && textures.0 >= self.orders[over].last_texture => over,
            Some(over) => over + 1,
            None => 0,
        };
        let top = self.orders.len();
        let order = (from..top).find(|&order| self.orders[order].kind == kind).unwrap_or(top);
        match self.orders.get_mut(order) {
            Some(at) => {
                at.reach = at.reach.union(&bounds);
                at.last_texture = at.last_texture.max(textures.1);
            }
            None => self.orders.push(Order { kind, reach: bounds, last_texture: textures.1 }),
        }
        order as DrawOrder
    }

    /// What's painted until [`Self::pop_layer`] shares one order, above
    /// what `bounds` overlaps (a line of text's glyphs). (Nothing to a
    /// recording: what it paints of one kind in a row shares an order
    /// anyway.)
    pub fn push_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        if self.recording {
            return;
        }
        let order = self.primitive_bounds.insert(bounds);
        self.layer_stack.push(order);
    }

    pub fn pop_layer(&mut self) {
        if self.recording {
            return;
        }
        self.layer_stack.pop();
    }

    pub fn insert_primitive(&mut self, primitive: impl Into<Primitive>) {
        let primitive = primitive.into();
        let clipped_bounds = primitive.reach().intersect(&primitive.content_mask().bounds);

        if clipped_bounds.is_empty() {
            self.culled += 1;
            return;
        }
        let kind = match &primitive {
            Primitive::Shadow(_) => Kind::Shadow,
            Primitive::Quad(_) => Kind::Quad,
            Primitive::Path(_) => Kind::Path,
            Primitive::Underline(_) => Kind::Underline,
            Primitive::Segment(_) => Kind::Segment,
            Primitive::Sector(_) => Kind::Sector,
            Primitive::MonochromeSprite(_) => Kind::Mono,
            Primitive::PolychromeSprite(_) => Kind::Poly,
            Primitive::ImageTriangle(_) => Kind::ImageTriangle,
            Primitive::Surface(_) => Kind::Surface,
        };
        let order = if self.recording {
            self.reach(clipped_bounds);
            let texture = match &primitive {
                Primitive::MonochromeSprite(sprite) => texture_key(&sprite.tile),
                Primitive::PolychromeSprite(sprite) => texture_key(&sprite.tile),
                _ => (0, 0),
            };
            self.next_order(kind, (texture, texture), clipped_bounds)
        } else {
            self.layer_stack.last().copied().unwrap_or_else(|| self.primitive_bounds.insert(clipped_bounds))
        };
        self.count += 1;
        match primitive {
            Primitive::Shadow(mut shadow) => {
                shadow.order = order;
                self.shadows.push(shadow);
            }
            Primitive::Quad(mut quad) => {
                quad.order = order;
                self.quads.push(quad);
            }
            Primitive::Path(mut path) => {
                path.order = order;
                path.id = PathId(self.paths.len());
                self.paths.push(path);
            }
            Primitive::Underline(mut underline) => {
                underline.order = order;
                self.underlines.push(underline);
            }
            Primitive::Segment(mut segment) => {
                segment.order = order;
                self.segments.push(segment);
            }
            Primitive::Sector(mut sector) => {
                sector.order = order;
                self.sectors.push(sector);
            }
            Primitive::MonochromeSprite(mut sprite) => {
                sprite.order = order;
                let start = self.mono_staging.len();
                self.mono_staging.push(sprite);
                self.record_runs(start, clipped_bounds);
            }
            Primitive::PolychromeSprite(mut sprite) => {
                sprite.order = order;
                self.polychrome_sprites.push(sprite);
            }
            Primitive::ImageTriangle(mut triangle) => {
                triangle.order = order;
                self.image_triangles.push(triangle);
            }
            Primitive::Surface(mut surface) => {
                surface.order = order;
                self.surfaces.push(surface);
            }
        }
    }

    /// Olo: many segments at once (`Painter::paint_segments`).
    pub fn insert_segments(&mut self, segments: impl IntoIterator<Item = Segment>) {
        let recording = self.recording;
        let layer = self.layer_stack.last().copied();
        let mut reach: Option<Bounds<ScaledPixels>> = None;
        // (Written in where they go, as they're made, then clipped where
        // they are: a curve of a thousand pieces, where a list of them was
        // made, gone over to clip them, again for their order and then
        // copied in, a fifth more of the time (`scene_bench`'s cycles).
        // Pushed one by one, each looking for room, was slower still.)
        let start = self.segments.len();
        self.segments.extend(segments);
        let mut gone = false;
        for segment in &mut self.segments[start..] {
            let clipped = segment.bounds.intersect(&segment.content_mask.bounds);
            if clipped.is_empty() {
                gone = true;
            } else if recording {
                reach = Some(reach.map_or(clipped, |r| r.union(&clipped)));
            } else {
                segment.order = layer.unwrap_or_else(|| self.primitive_bounds.insert(clipped));
            }
        }
        if gone {
            // (Those clipped away whole, left out: seldom any.)
            let mut kept = start;
            for i in start..self.segments.len() {
                let s = &self.segments[i];
                if !s.bounds.intersect(&s.content_mask.bounds).is_empty() {
                    self.segments.swap(kept, i);
                    kept += 1;
                }
            }
            self.segments.truncate(kept);
        }
        // (A recording's: one order for them all, by what they reach.)
        if let Some(reach) = reach {
            self.reach(reach);
            let order = self.next_order(Kind::Segment, ((0, 0), (0, 0)), reach);
            for segment in &mut self.segments[start..] {
                segment.order = order;
            }
        }
        self.count += self.segments.len() - start;
    }

    /// Olo: many glyphs' sprites at once (`Painter::paint_glyphs`, a
    /// line's): a page of text is tens of thousands, each written once,
    /// into `mono_staging`, as runs of one order and texture.
    pub fn insert_sprites(&mut self, sprites: impl IntoIterator<Item = MonochromeSprite>) {
        let mut sprites = sprites.into_iter().peekable();
        // (A line's glyphs share an order, whatever their textures: they
        // go texture by texture, `record_runs`.)
        let recording = self.recording;
        // (A recording's: the first and last of the line's textures, as
        // its glyphs are drawn.)
        let mut textures: Option<((u8, u32), (u8, u32))> = None;
        let layer = self.layer_stack.last().copied();
        let start = self.mono_staging.len();
        let mut reach: Option<Bounds<ScaledPixels>> = None;
        for mut sprite in sprites {
            let clipped = sprite.bounds.intersect(&sprite.content_mask.bounds);
            if clipped.is_empty() {
                self.culled += 1;
                continue;
            }
            if recording {
                reach = Some(reach.map_or(clipped, |r| r.union(&clipped)));
                let texture = texture_key(&sprite.tile);
                textures = Some(textures.map_or((texture, texture), |(first, last)| (first.min(texture), last.max(texture))));
            } else {
                sprite.order = layer.unwrap_or_else(|| self.primitive_bounds.insert(clipped));
            }
            self.mono_staging.push(sprite);
        }
        let end = self.mono_staging.len();
        if end == start {
            return;
        }
        // (A recording's: one order for the line, by what it reaches.)
        if let Some(reach) = reach {
            self.reach(reach);
            let order = self.next_order(Kind::Mono, textures.unwrap_or_default(), reach);
            for sprite in &mut self.mono_staging[start..end] {
                sprite.order = order;
            }
        }
        self.count += end - start;
        self.record_runs(start, reach.unwrap_or_default());
    }

    /// Olo: a kept line of glyphs (`Painter::paint_glyph_run`, a run
    /// painted again as it was), placed as `made` says, none cut by its
    /// clip: into a recording as [`Scene::insert_sprites`] would put
    /// them, shared, not copied, and without looking at each to cut it,
    /// cull it or see what it reaches (a screen of a terminal's text is a
    /// thousand and more glyphs, painted again whole when a line of it
    /// changes). `kept`: each glyph from the line's whole device pixel,
    /// sorted by texture; `parts`, each texture's and where its glyphs
    /// end; `reach`, what they show, together, placed.
    pub(crate) fn insert_kept_run(&mut self, kept: &Arc<[MonochromeSprite]>, parts: &[((u8, u32), u32)], reach: Bounds<ScaledPixels>, made: RunMade) {
        debug_assert!(self.recording, "a frame's sprites each take an order of their own");
        let (Some(first), Some(last)) = (parts.first(), parts.last()) else { return };
        if kept.is_empty() || reach.is_empty() {
            return;
        }
        debug_assert_eq!(last.1 as usize, kept.len());
        self.reach(reach);
        let order = self.next_order(Kind::Mono, (first.0, last.0), reach);
        self.count += kept.len();
        let mut from = 0;
        for (part, &(texture, end)) in parts.iter().enumerate() {
            let made = Some(RunMade { part: part as u32, ..made });
            self.mono_runs.push(SpriteRun { order, texture, range: from..end, reach, made, kept: Some(kept.clone()), drawing: None, put: Put::AsIs });
            from = end;
        }
    }

    /// Notes the sprites added to `mono_staging` from `start` as runs of
    /// one order and texture (those of a texture together, as `finish`
    /// sorted them before: a batch is one texture), each reaching no
    /// further than `reach` (what they all reach: a frame drawing a part
    /// away from it takes none of them, without looking at each).
    fn record_runs(&mut self, start: usize, reach: Bounds<ScaledPixels>) {
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
                self.mono_runs.push(SpriteRun { order: k.0, texture: k.1, range: from as u32..i as u32, reach, made: None, kept: None, drawing: None, put: Put::AsIs });
                (k, from) = (next, i);
            }
        }
        self.mono_runs.push(SpriteRun { order: k.0, texture: k.1, range: from as u32..self.mono_staging.len() as u32, reach, made: None, kept: None, drawing: None, put: Put::AsIs });
        if !sorted {
            // Glyphs of several textures in one line, interleaved: those
            // of each together, as one batch.
            self.mono_runs.truncate(first_run);
            self.mono_staging[start..].sort_by_key(key);
            self.record_runs(start, reach);
        }
    }

    /// Puts a view's kept drawing (`recorded`, a [`Scene::recording`])
    /// into this frame, moved by `by` (whole device pixels: glyphs and
    /// snapped edges stay on the grid) and clipped to `clip` (what holds
    /// the view, as it is now). Each thing's own clip moves with it.
    ///
    /// All of it goes above whatever it overlaps in the frame so far, as
    /// one thing: the frame looks at what overlaps what once for the
    /// drawing, not once for each thing in it, and drawings apart share
    /// their orders, so what's alike in them is drawn in one batch.
    pub(crate) fn append(&mut self, recorded: &Scene, by: Point<ScaledPixels>, clip: &Bounds<ScaledPixels>) {
        let Some(extent) = recorded.extent else { return };
        let still = by.x.0 == 0.0 && by.y.0 == 0.0;
        let mv = |b: Bounds<ScaledPixels>| if still { b } else { Bounds { origin: b.origin + by, size: b.size } };
        let reach = mv(extent).intersect(clip);
        if reach.is_empty() {
            return;
        }
        self.images.extend(recorded.images.iter().cloned());
        let base = self.primitive_bounds.insert_run(reach, recorded.depth());
        // Each thing moved, its clip with it and cut by `clip`; what's
        // clipped away whole is left out.
        // (`$reach`: what of it can show, a shadow's blur past its bounds
        // too, `Primitive::reach`.)
        macro_rules! take {
            ($x:ident) => {
                take!($x, $x.bounds)
            };
            ($x:ident, $reach:expr) => {{
                $x.bounds = mv($x.bounds);
                $x.content_mask.bounds = mv($x.content_mask.bounds).intersect(clip);
                $x.order += base;
                !$reach.intersect(&$x.content_mask.bounds).is_empty()
            }};
        }
        // All of it shows: `clip` cuts nothing of the drawing, so nothing
        // of any one thing in it (what a thing shows is within the
        // drawing's reach, which is what each showed when recorded, and
        // nothing recorded showed nothing). Each goes as it is, moved,
        // its own clip left as it was: no cutting and no looking at
        // whether it's still there, a thing at a time. Most drawings in
        // most frames: a row wholly in its list, a control in its window.
        let whole = reach == mv(extent);
        macro_rules! place {
            ($x:ident) => {{
                $x.bounds = mv($x.bounds);
                $x.content_mask.bounds = mv($x.content_mask.bounds);
                $x.order += base;
            }};
        }
        // (A list of one kind: placed, when all of it shows; else taken,
        // and those clipped away whole left out. `$moved`: what else of
        // it moves. Taken in a loop of its own, not by a filter's
        // closure: that was called for each thing, not put in place, and
        // each went through it and out as an `Option` before it was
        // pushed: a line of a screen saver's 1,500, drawn in part, took
        // 168 instructions more than one drawn whole; now 90
        // (`scene_bench`).)
        macro_rules! put {
            ($list:ident, |$x:ident| $moved:block) => {
                put!($list, |$x| $moved, $x.bounds)
            };
            ($list:ident, |$x:ident| $moved:block, $reach:expr) => {
                if whole {
                    self.$list.extend(recorded.$list.iter().cloned().map(|mut $x| {
                        $moved
                        place!($x);
                        $x
                    }));
                } else {
                    self.$list.reserve(recorded.$list.len());
                    for $x in &recorded.$list {
                        let mut $x = $x.clone();
                        $moved
                        if take!($x, $reach) {
                            self.$list.push($x);
                        }
                    }
                }
            };
        }
        let before = self.shadows.len() + self.quads.len() + self.underlines.len() + self.segments.len() + self.sectors.len() + self.polychrome_sprites.len() + self.surfaces.len();
        put!(shadows, |x| {}, x.reach());
        put!(quads, |x| {});
        put!(underlines, |x| {});
        put!(polychrome_sprites, |x| {});
        put!(surfaces, |x| {});
        put!(segments, |x| {
            (x.from, x.to, x.bottom) = (x.from + by, x.to + by, x.bottom + by.y);
        });
        put!(sectors, |x| {
            x.center = x.center + by;
        });
        self.count += self.shadows.len() + self.quads.len() + self.underlines.len() + self.segments.len() + self.sectors.len() + self.polychrome_sprites.len() + self.surfaces.len() - before;
        for path in &recorded.paths {
            // (Looked at before its vertices are copied.)
            let bounds = mv(path.bounds);
            if bounds.intersect(&mv(path.content_mask.bounds).intersect(clip)).is_empty() {
                continue;
            }
            let mut x = path.clone();
            x.bounds = bounds;
            x.content_mask.bounds = mv(x.content_mask.bounds).intersect(clip);
            x.order += base;
            x.id = PathId(self.paths.len());
            if !still {
                x.start = x.start + by;
                x.current = x.current + by;
            }
            for vertex in &mut x.vertices {
                if !still {
                    vertex.xy_position = vertex.xy_position + by;
                }
                vertex.content_mask.bounds = mv(vertex.content_mask.bounds).intersect(clip);
            }
            self.paths.push(x);
            self.count += 1;
        }
        for triangle in &recorded.image_triangles {
            let mut x = triangle.clone();
            if !still {
                for corner in &mut x.corners {
                    *corner = *corner + by;
                }
            }
            if take!(x) {
                self.image_triangles.push(x);
                self.count += 1;
            }
        }
        // Glyphs, run by run (each of one order and texture already). A
        // run that reaches nothing of what's drawn is passed over whole:
        // a frame drawing the line that changed on a page of text looks
        // at that line's glyphs, not at every glyph on the page.
        for run in &recorded.mono_runs {
            if !whole && !run.reach.is_empty() && mv(run.reach).intersect(clip).is_empty() {
                continue;
            }
            // A kept drawing's (sealed): the run is noted with where its
            // glyphs go, and they're put there as the renderer copies
            // them, not copied here first. (Those that end up clipped
            // away whole are drawn and show nothing: a line partly out
            // of its view's clip.)
            if let Some(list) = &recorded.sealed {
                let order = run.order + base;
                let (drawing, kept, put) = match &run.kept {
                    None => (Some(list.clone()), None, if still && whole { Put::AsIs } else { Put::Moved { by, clip: (!whole).then_some(*clip), order } }),
                    Some(kept) => {
                        // (Placed as it was made, then moved with the
                        // drawing, its clip cut by what holds it.)
                        let p = run.placing();
                        let moved = mv(p.clip);
                        let cut = if whole { moved } else { moved.intersect(clip) };
                        (None, Some(kept.clone()), Put::Placed(Placing { order, at: p.at + by, clip: cut, opacity: p.opacity }))
                    }
                };
                self.mono_runs.push(SpriteRun { order, texture: run.texture, range: run.range.clone(), reach: mv(run.reach).intersect(clip), made: None, kept, drawing, put });
                self.count += run.range.len();
                continue;
            }
            let start = self.mono_staging.len();
            let range = run.range.start as usize..run.range.end as usize;
            // (Each glyph as the recording holds it, then moved.)
            macro_rules! glyphs {
                ($glyphs:expr) => {
                    if whole {
                        self.mono_staging.extend($glyphs.map(|mut x| {
                            place!(x);
                            x
                        }));
                    } else {
                        self.mono_staging.extend($glyphs.filter_map(|mut x| take!(x).then_some(x)));
                    }
                };
            }
            match &run.kept {
                None => glyphs!(recorded.staged()[range].iter().cloned()),
                Some(kept) => {
                    // (A kept run's, put in place here: what a frame
                    // draws of a drawing is a line or two of it.)
                    let placing = run.placing();
                    glyphs!(kept[range].iter().map(|g| placing.place(g)));
                }
            }
            let end = self.mono_staging.len();
            if end > start {
                self.mono_runs.push(SpriteRun {
                    order: run.order + base,
                    texture: run.texture,
                    range: start as u32..end as u32,
                    reach: mv(run.reach).intersect(clip),
                    made: None,
                    kept: None,
                    drawing: None,
                    put: Put::AsIs,
                });
                self.count += end - start;
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
        // The glyphs go by their runs (`mono_runs`): the same order as
        // sorting each, for a sort of a frame's lines; and they stay
        // where they are, the renderer reading each batch's run by run
        // (a full window of text is 300 KB a frame not copied again).
        for run in &mut self.mono_runs {
            // (Kept runs painted into this scene itself, not through
            // `append`, which says where they go: put as they were made.)
            if run.kept.is_some() && matches!(run.put, Put::AsIs) {
                run.put = Put::Placed(run.placing());
            }
        }
        self.mono_runs.retain(|run| !run.range.is_empty());
        self.mono_runs.sort_by_key(|run| (run.order, run.texture));
        self.polychrome_sprites.sort_by_key(|sprite| (sprite.order, texture_key(&sprite.tile)));
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
        // (Notifications' History leaves its paper in 214 at 1×: a piece
        // each is a few bytes and four vertices.)
        const MOST_PIECES: usize = 512;
        let area = |b: &Bounds<ScaledPixels>| b.size.width.0.max(0.0) * b.size.height.0.max(0.0);
        let shown = |q: &Quad| q.bounds.intersect(&q.content_mask.bounds);
        // Drawn over all that's under it, edge to edge: one colour, no
        // round corners or border (whose edges are anti-aliased).
        let plain = |q: &Quad| q.background.tag == crate::BackgroundTag::Solid && q.corner_radii == Corners::default() && q.border_widths == Edges::default();
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
                    origin: point(ScaledPixels(c.origin.x.0 + 0.01), ScaledPixels(c.origin.y.0 + 0.01)),
                    size: size(ScaledPixels(c.size.width.0 - 0.02), ScaledPixels(c.size.height.0 - 0.02)),
                });
            }
            let bordered = q.border_widths != Edges::default();
            if bordered && (q.border_color.a < 1.0 || q.border_style != BorderStyle::Solid) {
                return None;
            }
            let r = q.corner_radii;
            let dy = r.top_left.0.max(r.top_right.0).max(r.bottom_left.0).max(r.bottom_right.0) + 1.0;
            let b = q.bounds;
            let band =
                Bounds { origin: point(ScaledPixels(b.origin.x.0 + 1.0), ScaledPixels(b.origin.y.0 + dy)), size: size(ScaledPixels(b.size.width.0 - 2.0), ScaledPixels(b.size.height.0 - 2.0 * dy)) };
            Some(band.intersect(&q.content_mask.bounds))
        };
        // Each quad's, found once. (The pieces a quad is cut into below
        // cover nothing: what's under them is done.)
        let mut covers: Vec<Option<Bounds<ScaledPixels>>> = self.quads.iter().map(|q| cover(q).filter(|c| c.size.width.0 > 0.0 && c.size.height.0 > 0.0)).collect();
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
            let left: Vec<_> = uncovered(a, &over).into_iter().filter(|r| r.size.width.0 >= 0.05 && r.size.height.0 >= 0.05).collect();
            if left.is_empty() {
                self.quads.remove(i);
                covers.remove(i);
                continue;
            }
            let kept: f32 = left.iter().map(area).sum();
            if flat && kept < 0.75 * area(&a) && left.len() <= MOST_PIECES {
                let q = self.quads[i].clone();
                let pieces: Vec<Quad> = left.into_iter().filter(|r| area(r) > 0.0).map(|r| Quad { bounds: r, ..q.clone() }).collect();
                let n = pieces.len();
                self.quads.splice(i..i + 1, pieces);
                covers.splice(i..i + 1, std::iter::repeat_n(None, n));
                i += n;
                continue;
            }
            i += 1;
        }
    }

    #[cfg_attr(all(any(target_os = "linux", target_os = "freebsd"), not(feature = "wayland")), allow(dead_code))]
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
            glyphs: &self.mono_staging,
            runs: &self.mono_runs,
            runs_start: 0,
            runs_iter: self.mono_runs.iter().peekable(),
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
#[cfg_attr(all(any(target_os = "linux", target_os = "freebsd"), not(feature = "wayland")), allow(dead_code))]
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

/// Olo: glyph sprites of one order and texture, together in a scene's
/// `mono_staging`, or in a kept run's own (`kept`).
struct SpriteRun {
    order: DrawOrder,
    texture: (u8, u32),
    /// Where they are in `mono_staging`, or in `kept`.
    range: Range<u32>,
    /// What they show reaches no further (a recording's; empty: not
    /// known, in a frame's own).
    reach: Bounds<ScaledPixels>,
    /// How they were made, if from a kept run of glyphs whole.
    made: Option<RunMade>,
    /// (A recording's, made from a kept run whole.) The kept run's
    /// glyphs, shared, not copied: each from the run's whole device
    /// pixel, put in place as `made` says only where a frame draws them
    /// (`Scene::append`). A drawing made again when a line of it changed
    /// holds each of its other lines so, its glyphs not written again (a
    /// screen of a terminal's text: 150 KB written at every line printed).
    kept: Option<Arc<[MonochromeSprite]>>,
    /// (A frame's.) The kept drawing's glyphs it's of (`Scene::sealed`),
    /// if not the scene's own or a kept run's.
    drawing: Option<Arc<Vec<MonochromeSprite>>>,
    /// (A frame's.) How its glyphs are put in place as they're drawn.
    put: Put,
}

/// Olo: how a frame's run of glyphs is put in place, glyph by glyph, as
/// the renderer copies them (`Glyphs::pieces`).
#[derive(Clone, Copy)]
enum Put {
    /// As they are: the scene's own, or a drawing's that didn't move and
    /// shows whole.
    AsIs,
    /// Each moved by `by` (whole device pixels), its own clip with it,
    /// cut by `clip` (none: all of the drawing shows).
    Moved { by: Point<ScaledPixels>, clip: Option<Bounds<ScaledPixels>>, order: DrawOrder },
    /// A kept run's (`Placing`).
    Placed(Placing),
}

impl SpriteRun {
    /// Its glyphs as held (`own`: its scene's own list).
    fn held<'a>(&'a self, own: &'a [MonochromeSprite]) -> &'a [MonochromeSprite] {
        let range = self.range.start as usize..self.range.end as usize;
        match (&self.drawing, &self.kept) {
            (Some(list), _) => &list[range],
            (None, Some(kept)) => &kept[range],
            (None, None) => &own[range],
        }
    }

    /// Its glyphs' placing (of a kept run's): at its order, as `made`.
    fn placing(&self) -> Placing {
        Placing { order: self.order, ..self.made.map_or_else(Placing::default, |m| m.placing()) }
    }

    /// Its glyphs as the recording holds them (`scene`'s), in place, one
    /// at a time and from either end: none written out (a kept run's are
    /// each put in place as they're asked for).
    fn placed<'a>(&'a self, scene: &'a Scene) -> impl DoubleEndedIterator<Item = MonochromeSprite> + ExactSizeIterator + 'a {
        let placing = self.placing();
        (self.range.start as usize..self.range.end as usize).map(move |i| match &self.kept {
            None => scene.staged()[i].clone(),
            Some(kept) => placing.place(&kept[i]),
        })
    }

    /// Whether this and `other` are lines put in whole, the same way
    /// (where, in what clip, how see-through), of kept glyphs that are
    /// the same: then they are the same in place (a glyph in place is
    /// its kept glyph and the placing, nothing else), and no glyph of
    /// either need be put in place to see it. Not the other way about:
    /// lines put differently, or of other kept glyphs, can still come to
    /// the same in place, and are compared there.
    fn kept_as(&self, other: &SpriteRun) -> bool {
        let (Some(a), Some(b), Some(ma), Some(mb)) = (&self.kept, &other.kept, self.made, other.made) else { return false };
        let (a, b) = (&a[self.range.start as usize..self.range.end as usize], &b[other.range.start as usize..other.range.end as usize]);
        self.order == other.order && (ma.at, ma.clip, ma.opacity) == (mb.at, mb.clip, mb.opacity) && (std::ptr::eq(a, b) || same_sprites(a, b))
    }

    /// What all its glyphs show together, if that is known without
    /// reading them: a line put in whole of one texture, whose reach is
    /// its glyphs' own, each within the line's clip (`insert_kept_run`:
    /// a line of several textures is a run for each, and each has the
    /// whole line's reach, more than its own glyphs').
    fn shows_whole(&self) -> Option<Bounds<ScaledPixels>> {
        let kept = self.kept.as_ref()?;
        (self.made.is_some_and(|m| m.part == 0) && self.range.start == 0 && self.range.end as usize == kept.len() && !self.reach.is_empty()).then_some(self.reach)
    }

    /// Its glyphs, as the recording holds them (`scene`'s), onto `into`
    /// (the tests': what compares drawings walks them in place, `placed`).
    #[cfg(test)]
    fn glyphs_onto(&self, scene: &Scene, into: &mut Vec<MonochromeSprite>) {
        let range = self.range.start as usize..self.range.end as usize;
        match &self.kept {
            None => into.extend_from_slice(&scene.staged()[range]),
            Some(kept) => {
                let placing = self.placing();
                into.extend(kept[range].iter().map(|g| placing.place(g)));
            }
        }
    }
}

/// Olo: runs of glyphs as one line of glyphs in place, none written
/// out (`Scene::glyphs_changed_since`).
struct Placed<'a> {
    scene: &'a Scene,
    runs: &'a [SpriteRun],
}

impl<'a> Placed<'a> {
    fn len(&self) -> usize {
        self.runs.iter().map(|r| r.range.len()).sum()
    }

    fn forward(&self) -> impl Iterator<Item = MonochromeSprite> + 'a {
        let scene = self.scene;
        self.runs.iter().flat_map(move |r| r.placed(scene))
    }

    fn backward(&self) -> impl Iterator<Item = MonochromeSprite> + 'a {
        let scene = self.scene;
        self.runs.iter().rev().flat_map(move |r| r.placed(scene).rev())
    }

    /// Adds to `all` what the glyphs `range` (counted along the whole
    /// line of them) show, each as `shows` says, one after another as
    /// they come (a union of bounds is sums in floats, and comes to the
    /// same in any order only where they are exact: so in the order the
    /// glyphs written out were gone through, and to the same bits). A
    /// run all in the range whose reach is what its glyphs show
    /// ([`SpriteRun::shows_whole`]) is added by its reach, its glyphs
    /// unread, where that is the same thing: `all` so far and the reach
    /// of whole numbers that add exactly ([`adds_exactly`]), as on a
    /// screen they are. Else glyph by glyph.
    fn shows_onto(&self, range: Range<usize>, shows: impl Fn(&MonochromeSprite) -> Bounds<ScaledPixels>, all: &mut Option<Bounds<ScaledPixels>>) {
        fn add(all: &mut Option<Bounds<ScaledPixels>>, r: Bounds<ScaledPixels>) {
            if !r.is_empty() {
                *all = Some(all.map_or(r, |a| a.union(&r)));
            }
        }
        let mut at = 0;
        for run in self.runs {
            let (from, to) = (at, at + run.range.len());
            at = to;
            let (lo, hi) = (range.start.max(from), range.end.min(to));
            if lo >= hi {
                continue;
            }
            match run.shows_whole().filter(|whole| (lo, hi) == (from, to) && adds_exactly(whole) && all.as_ref().is_none_or(adds_exactly)) {
                Some(whole) => add(all, whole),
                None => run.placed(self.scene).skip(lo - from).take(hi - lo).for_each(|g| add(all, shows(&g))),
            }
        }
    }
}

/// Olo: whether unions of bounds like `b` are exact, and so come to the
/// same in whatever order they are taken: its corner and size whole
/// numbers, small enough that their sums and differences are whole
/// numbers an f32 holds (every one to 2^24; these are at most 2^21
/// each). Bounds in whole device pixels on any screen are.
fn adds_exactly(b: &Bounds<ScaledPixels>) -> bool {
    [b.origin.x.0, b.origin.y.0, b.size.width.0, b.size.height.0].into_iter().all(|v| v.abs() <= 2_097_152.0 && v.fract() == 0.0)
}

/// Olo: where a kept run's glyphs go (`RunMade`): moved to its whole
/// device pixel, in its clip, at its opacity.
#[derive(Clone, Copy)]
struct Placing {
    order: DrawOrder,
    at: Point<ScaledPixels>,
    clip: Bounds<ScaledPixels>,
    opacity: f32,
}

impl Default for Placing {
    fn default() -> Self {
        Placing { order: 0, at: Point::default(), clip: Bounds::default(), opacity: 1.0 }
    }
}

impl Placing {
    /// A kept run's glyph, as the recording holds it.
    fn place(&self, glyph: &MonochromeSprite) -> MonochromeSprite {
        let mut x = glyph.clone();
        x.order = self.order;
        x.bounds.origin = x.bounds.origin + self.at;
        x.content_mask.bounds = self.clip;
        if self.opacity != 1.0 {
            x.color = x.color.opacity(self.opacity);
        }
        x
    }
}

/// Olo: how a recording's run of glyphs was made from a kept run
/// ([`crate::GlyphRun`], `Scene::insert_kept_run`). Two runs made the
/// same way, of the same order, are the same glyphs: a drawing made again
/// is compared with the last line by line, not glyph by glyph (a screen
/// of text is a thousand and more glyphs, two copies of them read from
/// memory at each line printed: 60 µs a frame).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct RunMade {
    /// The kept run's glyphs, as last found (new each time they are).
    pub found: u64,
    /// Its whole device pixel, its clip and its opacity (their bits).
    pub at: [u32; 2],
    pub clip: [u32; 4],
    pub opacity: u32,
    /// Which of the line's runs (one a texture, its glyphs of several).
    pub part: u32,
}

impl RunMade {
    fn placing(&self) -> Placing {
        let f = f32::from_bits;
        Placing {
            order: 0,
            at: Point { x: ScaledPixels(f(self.at[0])), y: ScaledPixels(f(self.at[1])) },
            clip: Bounds {
                origin: Point { x: ScaledPixels(f(self.clip[0])), y: ScaledPixels(f(self.clip[1])) },
                size: Size { width: ScaledPixels(f(self.clip[2])), height: ScaledPixels(f(self.clip[3])) },
            },
            opacity: f(self.opacity),
        }
    }
}

/// An atlas tile's texture, as sprites are batched by.
pub(crate) fn texture_key(t: &AtlasTile) -> (u8, u32) {
    (t.texture_id.kind as u8, t.texture_id.index)
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

    /// What drawing it can touch: its bounds, and a shadow's blur past
    /// them ([`Shadow::reach`]). A frame drawing only a part of the
    /// window goes by it, so a shadow falling past what casts it is
    /// drawn again with what it falls on.
    pub fn reach(&self) -> Bounds<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => shadow.reach(),
            _ => *self.bounds(),
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

#[cfg_attr(all(any(target_os = "linux", target_os = "freebsd"), not(feature = "wayland")), allow(dead_code))]
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
    glyphs: &'a [MonochromeSprite],
    runs: &'a [SpriteRun],
    runs_start: usize,
    runs_iter: Peekable<slice::Iter<'a, SpriteRun>>,
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
            (self.shadows_iter.peek().map(|s| s.order), PrimitiveKind::Shadow),
            (self.quads_iter.peek().map(|q| q.order), PrimitiveKind::Quad),
            (self.paths_iter.peek().map(|q| q.order), PrimitiveKind::Path),
            (self.sectors_iter.peek().map(|s| s.order), PrimitiveKind::Sector),
            (self.underlines_iter.peek().map(|u| u.order), PrimitiveKind::Underline),
            (self.segments_iter.peek().map(|s| s.order), PrimitiveKind::Segment),
            (self.runs_iter.peek().map(|r| r.order), PrimitiveKind::MonochromeSprite),
            (self.polychrome_sprites_iter.peek().map(|s| s.order), PrimitiveKind::PolychromeSprite),
            (self.image_triangles_iter.peek().map(|t| t.order), PrimitiveKind::ImageTriangle),
            (self.surfaces_iter.peek().map(|s| s.order), PrimitiveKind::Surface),
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
                while self.shadows_iter.next_if(|shadow| (shadow.order, batch_kind) < max_order_and_kind).is_some() {
                    shadows_end += 1;
                }
                self.shadows_start = shadows_end;
                Some(PrimitiveBatch::Shadows(&self.shadows[shadows_start..shadows_end]))
            }
            PrimitiveKind::Quad => {
                let quads_start = self.quads_start;
                let mut quads_end = quads_start + 1;
                self.quads_iter.next();
                while self.quads_iter.next_if(|quad| (quad.order, batch_kind) < max_order_and_kind).is_some() {
                    quads_end += 1;
                }
                self.quads_start = quads_end;
                Some(PrimitiveBatch::Quads(&self.quads[quads_start..quads_end]))
            }
            PrimitiveKind::Path => {
                let paths_start = self.paths_start;
                let mut paths_end = paths_start + 1;
                self.paths_iter.next();
                while self.paths_iter.next_if(|path| (path.order, batch_kind) < max_order_and_kind).is_some() {
                    paths_end += 1;
                }
                self.paths_start = paths_end;
                Some(PrimitiveBatch::Paths(&self.paths[paths_start..paths_end]))
            }
            PrimitiveKind::Underline => {
                let underlines_start = self.underlines_start;
                let mut underlines_end = underlines_start + 1;
                self.underlines_iter.next();
                while self.underlines_iter.next_if(|underline| (underline.order, batch_kind) < max_order_and_kind).is_some() {
                    underlines_end += 1;
                }
                self.underlines_start = underlines_end;
                Some(PrimitiveBatch::Underlines(&self.underlines[underlines_start..underlines_end]))
            }
            PrimitiveKind::Segment => {
                let segments_start = self.segments_start;
                let mut segments_end = segments_start + 1;
                self.segments_iter.next();
                while self.segments_iter.next_if(|segment| (segment.order, batch_kind) < max_order_and_kind).is_some() {
                    segments_end += 1;
                }
                self.segments_start = segments_end;
                Some(PrimitiveBatch::Segments(&self.segments[segments_start..segments_end]))
            }
            PrimitiveKind::Sector => {
                let sectors_start = self.sectors_start;
                let mut sectors_end = sectors_start + 1;
                self.sectors_iter.next();
                while self.sectors_iter.next_if(|sector| (sector.order, batch_kind) < max_order_and_kind).is_some() {
                    sectors_end += 1;
                }
                self.sectors_start = sectors_end;
                Some(PrimitiveBatch::Sectors(&self.sectors[sectors_start..sectors_end]))
            }
            PrimitiveKind::MonochromeSprite => {
                // (Run by run: each run's glyphs are of one order and
                // texture, and none is empty.)
                let first = self.runs_iter.next().unwrap();
                let texture_id = first.held(self.glyphs)[0].tile.texture_id;
                let start = self.runs_start;
                let mut end = start + 1;
                while self.runs_iter.next_if(|run| (run.order, batch_kind) < max_order_and_kind && run.texture == first.texture).is_some() {
                    end += 1;
                }
                self.runs_start = end;
                Some(PrimitiveBatch::MonochromeSprites { texture_id, sprites: Glyphs { own: self.glyphs, runs: &self.runs[start..end] } })
            }
            PrimitiveKind::PolychromeSprite => {
                let texture_id = self.polychrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.polychrome_sprites_start;
                let mut sprites_end = self.polychrome_sprites_start + 1;
                self.polychrome_sprites_iter.next();
                while self.polychrome_sprites_iter.next_if(|sprite| (sprite.order, batch_kind) < max_order_and_kind && sprite.tile.texture_id == texture_id).is_some() {
                    sprites_end += 1;
                }
                self.polychrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::PolychromeSprites { texture_id, sprites: &self.polychrome_sprites[sprites_start..sprites_end] })
            }
            PrimitiveKind::ImageTriangle => {
                let texture_id = self.image_triangles_iter.peek().unwrap().tile.texture_id;
                let start = self.image_triangles_start;
                let mut end = start + 1;
                self.image_triangles_iter.next();
                while self.image_triangles_iter.next_if(|triangle| (triangle.order, batch_kind) < max_order_and_kind && triangle.tile.texture_id == texture_id).is_some() {
                    end += 1;
                }
                self.image_triangles_start = end;
                Some(PrimitiveBatch::ImageTriangles { texture_id, triangles: &self.image_triangles[start..end] })
            }
            PrimitiveKind::Surface => {
                let surfaces_start = self.surfaces_start;
                let mut surfaces_end = surfaces_start + 1;
                self.surfaces_iter.next();
                while self.surfaces_iter.next_if(|surface| (surface.order, batch_kind) < max_order_and_kind).is_some() {
                    surfaces_end += 1;
                }
                self.surfaces_start = surfaces_end;
                Some(PrimitiveBatch::Surfaces(&self.surfaces[surfaces_start..surfaces_end]))
            }
        }
    }
}

#[derive(Debug)]
#[cfg_attr(all(any(target_os = "linux", target_os = "freebsd"), not(feature = "wayland")), allow(dead_code))]
pub(crate) enum PrimitiveBatch<'a> {
    Shadows(&'a [Shadow]),
    Quads(&'a [Quad]),
    Paths(&'a [Path<ScaledPixels>]),
    Underlines(&'a [Underline]),
    Segments(&'a [Segment]),
    Sectors(&'a [Sector]),
    MonochromeSprites { texture_id: AtlasTextureId, sprites: Glyphs<'a> },
    PolychromeSprites { texture_id: AtlasTextureId, sprites: &'a [PolychromeSprite] },
    ImageTriangles { texture_id: AtlasTextureId, triangles: &'a [ImageTriangle] },
    Surfaces(&'a [PaintSurface]),
}

/// Olo: a batch's glyphs, where they are: its runs (in the order
/// they're drawn), each a range of the frame's own list, a kept
/// drawing's or a kept run's, with where its glyphs go. The renderer
/// copies them into the GPU's buffer one after another, putting each in
/// place as it goes (`pieces`): what doesn't move is copied as it is.
#[derive(Clone, Copy)]
pub(crate) struct Glyphs<'a> {
    own: &'a [MonochromeSprite],
    runs: &'a [SpriteRun],
}

/// Olo: a piece of a batch's glyphs: some as they are, or one put in place.
pub(crate) enum Piece<'a> {
    AsIs(&'a [MonochromeSprite]),
    One(MonochromeSprite),
}

impl std::fmt::Debug for Glyphs<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Glyphs({} in {} runs)", self.len(), self.runs.len())
    }
}

impl<'a> Glyphs<'a> {
    pub(crate) fn len(&self) -> usize {
        self.runs.iter().map(|run| run.range.len()).sum()
    }

    /// Its glyphs in order, as `Piece`s: runs as they are one after
    /// another in a list as one piece (a page of a view's lines).
    pub(crate) fn pieces(&self, mut f: impl FnMut(Piece<'a>)) {
        let own = self.own;
        let mut i = 0;
        while let Some(run) = self.runs.get(i) {
            i += 1;
            match run.put {
                Put::AsIs => {
                    let held = run.held(own);
                    let (start, mut len) = (held.as_ptr(), held.len());
                    // (Those that follow it in the same list, as it is.)
                    while let Some(next) = self.runs.get(i).filter(|n| matches!(n.put, Put::AsIs)) {
                        let more = next.held(own);
                        if more.as_ptr() != start.wrapping_add(len) {
                            break;
                        }
                        len += more.len();
                        i += 1;
                    }
                    // SAFETY: `start..start + len` is runs of one list, one
                    // right after another (checked above).
                    f(Piece::AsIs(unsafe { std::slice::from_raw_parts(start, len) }));
                }
                Put::Moved { by, clip, order } => {
                    for g in run.held(own) {
                        let mut x = g.clone();
                        x.order = order;
                        x.bounds.origin = x.bounds.origin + by;
                        x.content_mask.bounds.origin = x.content_mask.bounds.origin + by;
                        if let Some(clip) = clip {
                            x.content_mask.bounds = x.content_mask.bounds.intersect(&clip);
                        }
                        f(Piece::One(x));
                    }
                }
                Put::Placed(placing) => {
                    for g in run.held(own) {
                        f(Piece::One(placing.place(g)));
                    }
                }
            }
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, PartialEq)]
#[repr(C)]
pub(crate) struct Shadow {
    pub order: DrawOrder,
    pub blur_radius: ScaledPixels,
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
}

impl Shadow {
    /// What it can touch: its bounds and its blur past them (the shader
    /// draws three blur radii out).
    pub fn reach(&self) -> Bounds<ScaledPixels> {
        self.bounds.dilate(ScaledPixels(3.0 * self.blur_radius.0))
    }
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
        Self { rotation_scale: [[1.0, 0.0], [0.0, 1.0]], translation: [0.0, 0.0] }
    }

    /// Move the origin by a given point
    pub fn translate(mut self, point: Point<ScaledPixels>) -> Self {
        self.compose(Self { rotation_scale: [[1.0, 0.0], [0.0, 1.0]], translation: [point.x.0, point.y.0] })
    }

    /// Clockwise rotation in radians around the origin
    pub fn rotate(self, angle: Radians) -> Self {
        self.compose(Self { rotation_scale: [[angle.0.cos(), -angle.0.sin()], [angle.0.sin(), angle.0.cos()]], translation: [0.0, 0.0] })
    }

    /// Scale around the origin
    pub fn scale(self, size: Size<f32>) -> Self {
        self.compose(Self { rotation_scale: [[size.width, 0.0], [0.0, size.height]], translation: [0.0, 0.0] })
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
                    self.rotation_scale[0][0] * other.rotation_scale[0][0] + self.rotation_scale[0][1] * other.rotation_scale[1][0],
                    self.rotation_scale[0][0] * other.rotation_scale[0][1] + self.rotation_scale[0][1] * other.rotation_scale[1][1],
                ],
                [
                    self.rotation_scale[1][0] * other.rotation_scale[0][0] + self.rotation_scale[1][1] * other.rotation_scale[1][0],
                    self.rotation_scale[1][0] * other.rotation_scale[0][1] + self.rotation_scale[1][1] * other.rotation_scale[1][1],
                ],
            ],
            translation: [
                self.translation[0] + self.rotation_scale[0][0] * other.translation[0] + self.rotation_scale[0][1] * other.translation[1],
                self.translation[1] + self.rotation_scale[1][0] * other.translation[0] + self.rotation_scale[1][1] * other.translation[1],
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

#[derive(Clone, Debug, PartialEq)]
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

// A sprite is four-byte numbers with nothing between them (as the GPU
// reads it): every byte of it is one of its numbers'.
const _: () = {
    use std::mem::size_of;
    assert!(size_of::<crate::AtlasTextureId>() == 2 * size_of::<u32>());
    assert!(size_of::<AtlasTile>() == size_of::<crate::AtlasTextureId>() + size_of::<crate::TileId>() + size_of::<u32>() + size_of::<Bounds<crate::DevicePixels>>());
    assert!(
        size_of::<MonochromeSprite>()
            == size_of::<DrawOrder>()
                + size_of::<u32>()
                + size_of::<Bounds<ScaledPixels>>()
                + size_of::<ContentMask<ScaledPixels>>()
                + size_of::<Hsla>()
                + size_of::<AtlasTile>()
                + size_of::<TransformationMatrix>()
    );
    assert!(size_of::<MonochromeSprite>() == 28 * 4);
    // (And a segment.)
    assert!(size_of::<Point<ScaledPixels>>() == 2 * size_of::<f32>() && size_of::<Bounds<ScaledPixels>>() == 4 * size_of::<f32>());
    assert!(
        size_of::<Segment>()
            == size_of::<DrawOrder>()
                + size_of::<u32>()
                + size_of::<Bounds<ScaledPixels>>()
                + size_of::<ContentMask<ScaledPixels>>()
                + size_of::<Hsla>()
                + 2 * size_of::<Point<ScaledPixels>>()
                + 2 * size_of::<ScaledPixels>()
    );
    assert!(size_of::<Segment>() == 20 * 4);
};

/// Olo: four-byte numbers with nothing between them (checked above), so
/// every byte of one is one of its numbers'.
///
/// # Safety
///
/// Only for a type with no padding.
unsafe trait Packed: Sized {}
// SAFETY: checked above.
unsafe impl Packed for MonochromeSprite {}
// SAFETY: checked above.
unsafe impl Packed for Segment {}

/// Olo: whether two stretches of glyph sprites (or segments) are the
/// same, byte for byte (a block of them one comparison of memory, where
/// comparing each field of each is a branch a number: a screen saver's
/// 1,500 lines compared with the last frame's, the same half the time,
/// were a quarter of its work). Two the same to the byte are the same;
/// two that differ only in a zero's sign are taken for different, which
/// draws a few pixels again.
/// Olo: whether two drawings' paths are the same to draw: where they are,
/// their fill, their triangles, and what they were built from (the same
/// one, or one built the same: its curves, how it is stroked or filled).
fn same_paths(a: &[Path<ScaledPixels>], b: &[Path<ScaledPixels>]) -> bool {
    fn same_outline(a: &Option<Arc<crate::outline::PathOutline>>, b: &Option<Arc<crate::outline::PathOutline>>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b) || (a.stroke == b.stroke && a.fill_rule == b.fill_rule && a.unmade == b.unmade && a.path.iter().eq(b.path.iter())),
            _ => false,
        }
    }
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| a.order == b.order && a.bounds == b.bounds && a.content_mask == b.content_mask && a.color == b.color && a.vertices == b.vertices && same_outline(&a.outline, &b.outline))
}

/// Olo: whether two drawings show the same videos' pictures in the same
/// places: the very same decoded frame, not one that looks like it.
fn same_surfaces(a: &[PaintSurface], b: &[PaintSurface]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            #[cfg(target_os = "macos")]
            let picture = a.image_buffer == b.image_buffer;
            #[cfg(not(target_os = "macos"))]
            let picture = Arc::ptr_eq(&a.frame, &b.frame);
            picture && a.order == b.order && a.bounds == b.bounds && a.content_mask == b.content_mask
        })
}

fn same_sprites<T: Packed>(a: &[T], b: &[T]) -> bool {
    fn bytes<T: Packed>(s: &[T]) -> &[u8] {
        // SAFETY: `T` has no padding (`Packed`), so all of its bytes are
        // initialized; u8 has no alignment to keep.
        unsafe { slice::from_raw_parts(s.as_ptr().cast::<u8>(), std::mem::size_of_val(s)) }
    }
    a.len() == b.len() && bytes(a) == bytes(b)
}

impl From<MonochromeSprite> for Primitive {
    fn from(sprite: MonochromeSprite) -> Self {
        Primitive::MonochromeSprite(sprite)
    }
}

#[derive(Clone, Debug, PartialEq)]
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
#[derive(Debug, Clone, PartialEq)]
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
    /// Olo: a video's picture, its planes as decoded (`Window::paint_yuv`).
    #[cfg(not(target_os = "macos"))]
    pub frame: std::sync::Arc<crate::YuvFrame>,
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
            bounds: Bounds { origin: start, size: Default::default() },
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
            vertices: self.vertices.iter().map(|vertex| vertex.scale(factor)).collect(),
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
            self.push_triangle((self.start, self.current, to), (point(0., 1.), point(0., 1.), point(0., 1.)));
        }
        self.current = to;
    }

    /// Draw a curve from the current point to the given point, using the given control point.
    pub fn curve_to(&mut self, to: Point<Pixels>, ctrl: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle((self.start, self.current, to), (point(0., 1.), point(0., 1.), point(0., 1.)));
        }

        self.push_triangle((self.current, ctrl, to), (point(0., 0.), point(0.5, 0.), point(1., 1.)));
        self.current = to;
    }

    /// Push a triangle to the Path.
    pub fn push_triangle(&mut self, xy: (Point<Pixels>, Point<Pixels>, Point<Pixels>), st: (Point<f32>, Point<f32>, Point<f32>)) {
        self.bounds =
            self.bounds.union(&Bounds { origin: xy.0, size: Default::default() }).union(&Bounds { origin: xy.1, size: Default::default() }).union(&Bounds { origin: xy.2, size: Default::default() });

        self.vertices.push(PathVertex { xy_position: xy.0, st_position: st.0, content_mask: Default::default() });
        self.vertices.push(PathVertex { xy_position: xy.1, st_position: st.1, content_mask: Default::default() });
        self.vertices.push(PathVertex { xy_position: xy.2, st_position: st.2, content_mask: Default::default() });
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

#[derive(Clone, Debug, PartialEq)]
#[repr(C)]
pub(crate) struct PathVertex<P: Clone + Debug + Default + PartialEq> {
    pub(crate) xy_position: Point<P>,
    pub(crate) st_position: Point<f32>,
    pub(crate) content_mask: ContentMask<P>,
}

impl PathVertex<Pixels> {
    pub fn scale(&self, factor: f32) -> PathVertex<ScaledPixels> {
        PathVertex { xy_position: self.xy_position.scale(factor), st_position: self.st_position, content_mask: self.content_mask.scale(factor) }
    }
}

/// Olo: what of `r` none of `covers` is over, in rectangles: `r` cut
/// into bands across at the covers' tops and bottoms, each band's
/// stretches between them, a stretch the bands below have too going on
/// down (a grid of cards on a paper leaves its gutters, a few pieces a
/// row).
fn uncovered(r: Bounds<ScaledPixels>, covers: &[Bounds<ScaledPixels>]) -> Vec<Bounds<ScaledPixels>> {
    let (rl, rt, rr, rb) = (r.origin.x.0, r.origin.y.0, r.origin.x.0 + r.size.width.0, r.origin.y.0 + r.size.height.0);
    // Each cover as it is over `r`: left, top, right, bottom. (Not one
    // too small to be worth its bands: a toolbar's button.)
    let least = (rr - rl) * (rb - rt) / 1000.0;
    let area = |c: &[f32; 4]| (c[2] - c[0]) * (c[3] - c[1]);
    let mut cs: Vec<[f32; 4]> = covers
        .iter()
        .map(|c| [c.origin.x.0.max(rl), c.origin.y.0.max(rt), (c.origin.x.0 + c.size.width.0).min(rr), (c.origin.y.0 + c.size.height.0).min(rb)])
        .filter(|c| c[0] < c[2] && c[1] < c[3] && area(c) >= least)
        .collect();
    // Less those inside a bigger one (the cards on a window's paper),
    // each a band or two more for nothing.
    cs.sort_by(|a, b| area(b).total_cmp(&area(a)));
    let mut kept = 0;
    for k in 0..cs.len() {
        let c = cs[k];
        if !cs[..kept].iter().any(|o| o[0] <= c[0] && o[1] <= c[1] && c[2] <= o[2] && c[3] <= o[3]) {
            cs[kept] = c;
            kept += 1;
        }
    }
    cs.truncate(kept);
    cs.sort_by(|a, b| a[1].total_cmp(&b[1]));
    let mut ys: Vec<f32> = cs.iter().flat_map(|c| [c[1], c[3]]).chain([rt, rb]).collect();
    ys.sort_by(f32::total_cmp);
    ys.dedup();
    let rect = |l: f32, t: f32, r: f32, b: f32| Bounds { origin: point(ScaledPixels(l), ScaledPixels(t)), size: size(ScaledPixels(r - l), ScaledPixels(b - t)) };
    let mut out = Vec::new();
    // The stretches open in the bands so far, left to right: left,
    // right, since where.
    let (mut open, mut going): (Vec<(f32, f32, f32)>, Vec<(f32, f32, f32)>) = (Vec::new(), Vec::new());
    let (mut next, mut active, mut spans, mut free): (usize, Vec<[f32; 4]>, Vec<(f32, f32)>, Vec<(f32, f32)>) = (0, Vec::new(), Vec::new(), Vec::new());
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
        Bounds { origin: point(ScaledPixels(x), ScaledPixels(y)), size: size(ScaledPixels(w), ScaledPixels(h)) }
    }

    fn quad(order: u32, b: Bounds<ScaledPixels>, alpha: f32) -> Quad {
        Quad { order, bounds: b, content_mask: ContentMask { bounds: rect(0.0, 0.0, 4000.0, 4000.0) }, background: solid_background(hsla(0.5, 0.5, 0.5, alpha)), ..Default::default() }
    }

    fn segment(x: f32, mask: Bounds<ScaledPixels>) -> Segment {
        let (from, to) = (point(ScaledPixels(x + 5.0), ScaledPixels(10.0)), point(ScaledPixels(x + 45.0), ScaledPixels(40.0)));
        Segment {
            order: 0,
            round: 1,
            bounds: rect(x, 0.0, 50.0, 50.0),
            content_mask: ContentMask { bounds: mask },
            color: hsla(0.5, 0.5, 0.5, 1.0),
            from,
            to,
            half_width: ScaledPixels(1.0),
            bottom: ScaledPixels(0.0),
        }
    }

    /// Segments clipped away whole are left out as they're recorded, the
    /// rest kept in order, one order for them all, reaching what they do.
    #[test]
    fn segments_recorded_leave_out_what_shows_nothing() {
        let mut rec = Scene::recording();
        let mask = rect(0.0, 0.0, 420.0, 1000.0);
        rec.insert_segments((0..10).map(|i| segment(i as f32 * 100.0, mask)));
        let xs: Vec<f32> = rec.segments.iter().map(|s| s.bounds.origin.x.0).collect();
        assert_eq!(xs, [0.0, 100.0, 200.0, 300.0, 400.0]);
        assert!(rec.segments.iter().all(|s| s.order == rec.segments[0].order));
        assert_eq!(rec.extent(), rect(0.0, 0.0, 420.0, 50.0));
        assert_eq!(rec.len(), 5);
        // None shows: nothing recorded.
        let mut none = Scene::recording();
        none.insert_segments((0..3).map(|i| segment(500.0 + i as f32 * 100.0, mask)));
        assert!(none.segments.is_empty() && none.is_empty());
    }

    /// A drawing put in a frame that draws part of the window: what
    /// shows of it kept, in order, moved, its clip cut by the frame's;
    /// the rest left out.
    #[test]
    fn a_drawing_put_in_part_keeps_what_shows() {
        let mut rec = Scene::recording();
        let everywhere = rect(0.0, 0.0, 4000.0, 4000.0);
        rec.insert_segments((0..10).map(|i| segment(i as f32 * 100.0, everywhere)));
        rec.quads = (0..10).map(|i| quad(0, rect(i as f32 * 100.0, 60.0, 50.0, 20.0), 1.0)).collect();
        let mut frame = Scene::default();
        frame.append(&rec, point(ScaledPixels(10.0), ScaledPixels(0.0)), &rect(0.0, 0.0, 420.0, 1000.0));
        let xs: Vec<f32> = frame.segments.iter().map(|s| s.bounds.origin.x.0).collect();
        assert_eq!(xs, [10.0, 110.0, 210.0, 310.0, 410.0]);
        assert!(frame.segments.iter().all(|s| s.content_mask.bounds == rect(10.0, 0.0, 410.0, 1000.0)));
        assert_eq!((frame.segments[4].from.x.0, frame.segments[4].to.x.0), (415.0, 455.0));
        let qs: Vec<f32> = frame.quads.iter().map(|q| q.bounds.origin.x.0).collect();
        assert_eq!(qs, [10.0, 110.0, 210.0, 310.0, 410.0]);
        // All of it showing: all of it, as it was, moved.
        let mut all = Scene::default();
        all.append(&rec, point(ScaledPixels(10.0), ScaledPixels(0.0)), &everywhere);
        assert_eq!(all.segments.len(), 10);
        assert_eq!(all.quads.len(), 10);
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
            quads.push(quad(2, rect(10.0 + k as f32 * 200.0, 50.0, 150.0, 32.0), 1.0));
        }
        quads.push(quad(2, rect(2.0, 101.0, 3128.0, 2033.0), 1.0));
        for i in 0..40 {
            let mut card = quad(3, rect(20.0 + (i % 5) as f32 * 600.0, 120.0 + (i / 5) as f32 * 250.0, 560.0, 230.0), 1.0);
            card.corner_radii = Corners::all(ScaledPixels(7.5));
            quads.push(card);
        }
        s.quads = quads;
        s.cull_hidden_quads();
        let screen = 3153.0 * 2163.0;
        let left = |order: u32| -> f32 { s.quads.iter().filter(|q| q.order == order).map(|q| q.bounds.size.width.0 * q.bounds.size.height.0).sum() };
        assert!(left(1) < screen * 0.1, "the ground left: {:.2} screens", left(1) / screen);
        // The paper only between the cards and by their corners (the
        // bands between their corners are 0.75 of it), in a few pieces a
        // row of cards.
        let paper = 3128.0 * 2033.0;
        let bars = 12.0 * 150.0 * 32.0 + 3153.0 * 39.0;
        let pieces = s.quads.iter().filter(|q| q.order == 2).count() - 13;
        assert!(((left(2) - bars) / paper - 0.25).abs() < 0.01, "the paper left: {:.3} of it", (left(2) - bars) / paper);
        assert!(pieces <= 8 * 8 + 2, "{pieces} pieces");
    }

    /// What's left and what covers it make the whole, without overlap,
    /// for rectangles anywhere (a grid of points checked).
    #[test]
    fn uncovered_is_the_rest_exactly() {
        let mut seed = 12345u64;
        let mut rand = |n: f32| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as f32 / (1u64 << 31) as f32 * n
        };
        for _ in 0..200 {
            let r = rect(rand(50.0).floor(), rand(50.0).floor(), 200.0 + rand(400.0).floor(), 200.0 + rand(400.0).floor());
            let covers: Vec<_> = (0..(rand(30.0) as usize)).map(|_| rect(rand(700.0).floor(), rand(700.0).floor(), 20.0 + rand(250.0).floor(), 20.0 + rand(250.0).floor())).collect();
            let least = r.size.width.0 * r.size.height.0 / 1000.0;
            let left = uncovered(r, &covers);
            for gx in 0..60 {
                for gy in 0..60 {
                    let p = point(ScaledPixels(r.origin.x.0 + (gx as f32 + 0.37) * r.size.width.0 / 60.0), ScaledPixels(r.origin.y.0 + (gy as f32 + 0.41) * r.size.height.0 / 60.0));
                    // (A cover too small to count may leave what's under it.)
                    let under = covers.iter().filter(|c| c.intersect(&r).size.width.0 * c.intersect(&r).size.height.0 >= least).any(|c| c.contains(&p));
                    let pieces = left.iter().filter(|b| b.contains(&p)).count();
                    assert_eq!(pieces, if under { 0 } else { 1 }, "{p:?} in {r:?} under {covers:?}: {left:?}");
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
        let under: Vec<_> = s.quads.iter().filter(|q| q.order == 0).map(|q| q.bounds).collect();
        let area: f32 = under.iter().map(|b| b.size.width.0 * b.size.height.0).sum();
        assert!((area - (500.0 * 5.0 + 1.0 * 495.0)).abs() < 1.0, "{under:?}");
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
        let b = Bounds { origin: point(ScaledPixels(x), ScaledPixels(0.0)), size: size(ScaledPixels(8.0), ScaledPixels(8.0)) };
        MonochromeSprite {
            order: 0,
            coverage: 0,
            bounds: b,
            content_mask: ContentMask { bounds: Bounds { origin: point(ScaledPixels(-1e4), ScaledPixels(-1e4)), size: size(ScaledPixels(2e4), ScaledPixels(2e4)) } },
            color: Hsla::default(),
            tile: AtlasTile {
                texture_id: AtlasTextureId { index: texture, kind: AtlasTextureKind::Monochrome },
                tile_id: TileId(id),
                padding: 0,
                bounds: Bounds { origin: point(DevicePixels(0), DevicePixels(0)), size: size(DevicePixels(8), DevicePixels(8)) },
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
            let bounds = Bounds { origin: point(ScaledPixels(x), ScaledPixels(0.0)), size: size(ScaledPixels(40.0), ScaledPixels(8.0)) };
            scene.push_layer(bounds);
            let glyphs: Vec<_> = (0..5).map(|g| next(x + g as f32, if line == 4 { g % 2 } else { 0 })).collect();
            scene.insert_sprites(glyphs.clone());
            painted.extend(glyphs.into_iter().map(|g| (g, *scene.layer_stack.last().unwrap())));
            scene.pop_layer();
            let alone = next(x + 3.0, 1);
            scene.insert_primitive(alone.clone());
            let order = scene.mono_staging.last().unwrap().order;
            painted.push((alone, order));
        }
        scene.finish();
        // (Each as it went in, with the order the scene gave it, sorted
        // by order and texture the way finish sorted every sprite.)
        let mut expected: Vec<_> = painted.into_iter().map(|(s, order)| (order, texture_key(&s.tile), s.tile.tile_id)).collect();
        expected.sort_by_key(|(order, texture, _)| (*order, *texture));
        let got: Vec<_> = scene.drawn_glyphs().iter().map(|s| (s.order, texture_key(&s.tile), s.tile.tile_id)).collect();
        assert_eq!(got, expected);
    }

    fn mask(x: f32, y: f32, w: f32, h: f32) -> ContentMask<ScaledPixels> {
        ContentMask { bounds: Bounds { origin: point(ScaledPixels(x), ScaledPixels(y)), size: size(ScaledPixels(w), ScaledPixels(h)) } }
    }

    fn quad(x: f32, y: f32, w: f32, h: f32) -> Quad {
        Quad { bounds: mask(x, y, w, h).bounds, content_mask: mask(-1e4, -1e4, 2e4, 2e4), ..Default::default() }
    }

    /// A kept drawing orders what it paints by itself: things of a kind
    /// painted in a row share an order; a thing of another kind goes to
    /// the lowest order of its kind above everything it may be over.
    #[test]
    fn a_recording_orders_by_what_is_over_what() {
        let mut kept = Scene::recording();
        kept.insert_primitive(quad(0.0, 0.0, 100.0, 20.0));
        kept.insert_primitive(quad(0.0, 0.0, 100.0, 1.0));
        kept.insert_sprites([sprite(4.0, 0, 1), sprite(12.0, 0, 2)]);
        // A rule over the ground and beside the glyphs: with the ground,
        // after it (quads are drawn as they were painted).
        kept.insert_primitive(quad(50.0, 0.0, 1.0, 20.0));
        assert_eq!(kept.quads.iter().map(|q| q.order).collect::<Vec<_>>(), vec![0, 0, 0]);
        assert_eq!(kept.mono_staging.iter().map(|s| s.order).collect::<Vec<_>>(), vec![1, 1]);
        assert_eq!(kept.depth(), 2);
        // One over the glyphs: above them.
        kept.insert_primitive(quad(6.0, 2.0, 4.0, 4.0));
        assert_eq!(kept.quads.last().unwrap().order, 2);
        // Glyphs beside everything but the ground: with the first.
        kept.insert_sprites([sprite(70.0, 0, 3)]);
        assert_eq!(kept.mono_staging.last().unwrap().order, 1);
        // Glyphs over the quad that's over glyphs: above it.
        kept.insert_sprites([sprite(5.0, 0, 4)]);
        assert_eq!(kept.mono_staging.last().unwrap().order, 3);
        assert_eq!(kept.depth(), 4);
        assert_eq!(kept.extent(), mask(0.0, 0.0, 100.0, 20.0).bounds);
    }

    fn picture(x: f32, y: f32, texture: u32) -> PolychromeSprite {
        PolychromeSprite {
            order: 0,
            nearest: 0,
            grayscale: false,
            opacity: 1.0,
            bounds: mask(x, y, 16.0, 16.0).bounds,
            content_mask: mask(-1e4, -1e4, 2e4, 2e4),
            corner_radii: Default::default(),
            tile: AtlasTile {
                texture_id: AtlasTextureId { index: texture, kind: AtlasTextureKind::Polychrome },
                tile_id: TileId(0),
                padding: 0,
                bounds: Bounds { origin: point(DevicePixels(0), DevicePixels(0)), size: size(DevicePixels(16), DevicePixels(16)) },
            },
        }
    }

    /// A list drawn row by row, each row a ground, an icon, a name and a
    /// rule, is as many orders as a row has kinds over one another,
    /// however many rows: every row's icons one draw, every row's names
    /// another.
    #[test]
    fn rows_drawn_one_by_one_share_their_orders() {
        let mut kept = Scene::recording();
        for row in 0..40 {
            let y = row as f32 * 20.0;
            // (Every other row has a ground of its own.)
            if row % 2 == 0 {
                kept.insert_primitive(quad(0.0, y, 300.0, 20.0));
            }
            kept.insert_primitive(picture(2.0, y + 2.0, row % 3));
            let mut name = [sprite(24.0, 0, row), sprite(32.0, 0, row), sprite(40.0, 1, row)];
            for glyph in &mut name {
                glyph.bounds.origin.y = ScaledPixels(y + 6.0);
            }
            kept.insert_sprites(name);
            // A rule at the row's right, over its ground.
            kept.insert_primitive(quad(290.0, y + 4.0, 1.0, 12.0));
        }
        // Grounds and rules; icons; names.
        assert_eq!(kept.depth(), 3);
        assert!(kept.quads.iter().all(|q| q.order == 0));
        assert!(kept.polychrome_sprites.iter().all(|p| p.order == 1));
        assert!(kept.mono_staging.iter().all(|g| g.order == 2));
        // And in a frame: six draws (the quads; the icons of each of
        // three textures; the glyphs of each of two).
        let mut frame = Scene::default();
        frame.append(&kept, point(ScaledPixels(0.0), ScaledPixels(0.0)), &mask(0.0, 0.0, 300.0, 800.0).bounds);
        frame.finish();
        assert_eq!(frame.batches().count(), 6);
    }

    /// Whatever is painted over something painted before is drawn after
    /// it: a higher order, or the same order where things of one order
    /// are drawn as they were painted (one kind, and for sprites one
    /// texture).
    #[test]
    fn what_is_painted_over_is_drawn_after() {
        // (A fixed sequence, the same every run.)
        let mut seed = 0x2545f4914f6cdd1du64;
        let mut next = |below: u32| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 33) as u32 % below
        };
        for round in 0..40 {
            let mut kept = Scene::recording();
            // (kind, texture, what it shows, its order)
            let mut painted: Vec<(u8, u32, Bounds<ScaledPixels>)> = Vec::new();
            let spread = if round % 2 == 0 { 400 } else { 60 };
            for _ in 0..120 {
                let (x, y) = (next(spread) as f32, next(spread) as f32);
                match next(4) {
                    0 | 1 => {
                        let (w, h) = (4.0 + next(60) as f32, 4.0 + next(30) as f32);
                        kept.insert_primitive(quad(x, y, w, h));
                        painted.push((0, 0, mask(x, y, w, h).bounds));
                    }
                    2 => {
                        let texture = next(2);
                        let mut glyphs = [sprite(x, texture, 1), sprite(x + 8.0, texture, 2)];
                        for glyph in &mut glyphs {
                            glyph.bounds.origin.y = ScaledPixels(y);
                        }
                        kept.insert_sprites(glyphs);
                        painted.push((1, texture, mask(x, y, 8.0, 8.0).bounds));
                        painted.push((1, texture, mask(x + 8.0, y, 8.0, 8.0).bounds));
                    }
                    _ => {
                        let texture = next(3);
                        kept.insert_primitive(picture(x, y, texture));
                        painted.push((2, texture, mask(x, y, 16.0, 16.0).bounds));
                    }
                }
            }
            let orders: Vec<DrawOrder> = {
                let (mut q, mut g, mut p) = (kept.quads.iter(), kept.mono_staging.iter(), kept.polychrome_sprites.iter());
                painted
                    .iter()
                    .map(|(kind, ..)| match kind {
                        0 => q.next().unwrap().order,
                        1 => g.next().unwrap().order,
                        _ => p.next().unwrap().order,
                    })
                    .collect()
            };
            for later in 0..painted.len() {
                for earlier in 0..later {
                    if !painted[later].2.intersects(&painted[earlier].2) {
                        continue;
                    }
                    let (a, b) = (orders[earlier], orders[later]);
                    // (Quads of an order: as painted. Sprites of an
                    // order: texture by texture, then as painted.)
                    let as_painted = painted[earlier].0 == painted[later].0 && (painted[later].0 == 0 || painted[later].1 >= painted[earlier].1);
                    assert!(b > a || (b == a && as_painted), "round {round}: {:?} at order {b} is over {:?} at order {a}", painted[later], painted[earlier]);
                }
            }
            assert!(kept.depth() as usize <= painted.len());
        }
    }

    /// A frame takes kept drawings whole: one above what it overlaps,
    /// those apart sharing their orders; moved, each thing's own clip
    /// with it, cut by what holds the drawing, what's cut away left out.
    #[test]
    fn a_frame_takes_drawings_whole() {
        let row = |text: u32| {
            let mut kept = Scene::recording();
            kept.insert_primitive(quad(0.0, 100.0, 400.0, 20.0));
            // Two cells, each clipped to its own.
            for (x, w) in [(0.0, 100.0), (100.0, 300.0)] {
                let mut glyph = sprite(x + 4.0, 0, text);
                glyph.bounds.origin.y = ScaledPixels(102.0);
                glyph.content_mask = mask(x, 100.0, w, 20.0);
                kept.insert_sprites([glyph]);
            }
            kept
        };
        let (ground, first, second) = (
            {
                let mut kept = Scene::recording();
                kept.insert_primitive(quad(0.0, 0.0, 400.0, 300.0));
                kept
            },
            row(1),
            row(2),
        );
        let list = mask(0.0, 0.0, 400.0, 300.0).bounds;
        let mut frame = Scene::default();
        frame.append(&ground, point(ScaledPixels(0.0), ScaledPixels(0.0)), &list);
        // One row moved up 95 pixels (still inside), the next 105 (its
        // top 5 cut by the list), a third out of sight.
        frame.append(&first, point(ScaledPixels(0.0), ScaledPixels(-95.0)), &list);
        frame.append(&second, point(ScaledPixels(0.0), ScaledPixels(-105.0)), &list);
        frame.append(&row(3), point(ScaledPixels(0.0), ScaledPixels(400.0)), &list);
        frame.finish();
        // The ground under both rows; the rows, overlapping each other,
        // one above the other: quads then glyphs, each row's above the
        // row before.
        assert_eq!(frame.quads.iter().map(|q| q.order).collect::<Vec<_>>(), vec![1, 2, 4]);
        assert_eq!(frame.drawn_glyphs().iter().map(|s| (s.order, s.tile.tile_id.0)).collect::<Vec<_>>(), vec![(3, 1), (3, 1), (5, 2), (5, 2)]);
        let moved: Vec<_> = frame.drawn_glyphs().iter().map(|g| (g.bounds.origin.y.0, g.content_mask.clone())).collect();
        assert_eq!(moved[..2], [(7.0, mask(0.0, 5.0, 100.0, 20.0)), (7.0, mask(100.0, 5.0, 300.0, 20.0))]);
        assert_eq!(moved[2..], [(-3.0, mask(0.0, 0.0, 100.0, 15.0)), (-3.0, mask(100.0, 0.0, 300.0, 15.0))]);
        assert_eq!(frame.len(), 7);

        // Drawings apart share their orders: what's alike in them is one
        // batch.
        let mut frame = Scene::default();
        frame.append(&first, point(ScaledPixels(0.0), ScaledPixels(0.0)), &list);
        frame.append(&second, point(ScaledPixels(0.0), ScaledPixels(20.0)), &list);
        assert_eq!(frame.quads.iter().map(|q| q.order).collect::<Vec<_>>(), vec![1, 1]);
        assert_eq!(frame.mono_staging.iter().map(|s| s.order).collect::<Vec<_>>(), vec![2, 2, 2, 2]);
    }

    /// Kept drawings, once made (sealed), aren't copied into the frame:
    /// its runs say where their glyphs go, and they're put there as
    /// they're drawn. What's drawn is what copying them drew: still and
    /// whole, moved, cut by what holds them, a glyph cut away whole
    /// drawn showing nothing (where the copy left it out).
    #[test]
    fn sealed_drawings_draw_as_copied_ones() {
        let row = |text: u32| {
            let mut kept = Scene::recording();
            kept.insert_primitive(quad(0.0, 100.0, 400.0, 20.0));
            // Three cells, each clipped to its own; one line's glyphs.
            let glyphs: Vec<_> = [(0.0, 100.0), (100.0, 300.0), (300.0, 100.0)]
                .into_iter()
                .map(|(x, w)| {
                    let mut glyph = sprite(x + 4.0, 0, text);
                    glyph.bounds.origin.y = ScaledPixels(102.0);
                    glyph.content_mask = mask(x, 100.0, w, 20.0);
                    glyph
                })
                .collect();
            kept.insert_sprites(glyphs);
            kept
        };
        // (What shows of each: where it is, cut by its clip.)
        let shown = |frame: &Scene| -> Vec<_> {
            frame.drawn_glyphs().into_iter().filter(|g| !g.bounds.intersect(&g.content_mask.bounds).is_empty()).map(|g| (g.order, g.tile.tile_id.0, g.bounds, g.content_mask.bounds, g.color)).collect()
        };
        let list = mask(0.0, 0.0, 250.0, 300.0).bounds;
        let places = [(0.0, 0.0), (0.0, -95.0), (0.0, -105.0), (-200.0, 20.0), (0.0, 400.0)];
        let (mut copied, mut sealed) = (Scene::default(), Scene::default());
        for (i, (x, y)) in places.into_iter().enumerate() {
            let kept = row(i as u32 + 1);
            copied.append(&kept, point(ScaledPixels(x), ScaledPixels(y)), &list);
            let mut kept = kept;
            kept.seal();
            assert!(kept.mono_staging.is_empty() && kept.sealed.is_some());
            sealed.append(&kept, point(ScaledPixels(x), ScaledPixels(y)), &list);
        }
        // Lines put in whole (kept runs: Terminal's, the editor's), one
        // at half its opacity, placed off their whole pixel's corner.
        let page = || {
            let mut kept = Scene::recording();
            for line in 0..6u32 {
                let glyphs: Vec<_> = (0..20).map(|i| sprite(i as f32 * 8.0, line % 2, 500 + line * 100 + i)).collect();
                let mut how = made(line as u64 + 1);
                how.at = [3.0f32.to_bits(), (130.0 + line as f32 * 20.0).to_bits()];
                if line == 2 {
                    how.opacity = 0.5f32.to_bits();
                }
                put_kept(&mut kept, glyphs, how);
            }
            kept
        };
        for (x, y) in [(0.0, 0.0), (10.0, -60.0), (0.0, 120.0)] {
            let kept = page();
            copied.append(&kept, point(ScaledPixels(x), ScaledPixels(y)), &list);
            let mut kept = kept;
            kept.seal();
            sealed.append(&kept, point(ScaledPixels(x), ScaledPixels(y)), &list);
        }
        // (A whole one, still, too: its glyphs as they are.)
        let mut still = row(9);
        let all = mask(0.0, 0.0, 1000.0, 1000.0).bounds;
        copied.append(&still, Point::default(), &all);
        still.seal();
        sealed.append(&still, Point::default(), &all);
        copied.finish();
        sealed.finish();
        assert!(sealed.mono_staging.is_empty(), "nothing copied into the frame");
        assert_eq!(shown(&sealed), shown(&copied));
        assert!(shown(&copied).len() >= 8 + 60);
    }

    /// A drawing that shows whole goes into the frame without its
    /// things' clips being cut by what holds it: each still shows
    /// exactly what it showed, moved. One that's cut shows what's left.
    #[test]
    fn a_drawing_that_shows_whole_shows_as_it_was() {
        let mut kept = Scene::recording();
        // A ground clipped by nothing near, a cell's ground clipped to
        // its cell, and glyphs: one within its cell, one cut by it.
        kept.insert_primitive(quad(10.0, 10.0, 200.0, 40.0));
        let mut cell = quad(20.0, 10.0, 300.0, 40.0);
        cell.content_mask = mask(20.0, 10.0, 80.0, 40.0);
        kept.insert_primitive(cell);
        for x in [24.0, 96.0] {
            let mut glyph = sprite(x, 0, 1);
            glyph.bounds.origin.y = ScaledPixels(20.0);
            glyph.content_mask = mask(20.0, 10.0, 80.0, 40.0);
            kept.insert_sprites([glyph]);
        }
        let shows = |b: &Bounds<ScaledPixels>, m: &ContentMask<ScaledPixels>| b.intersect(&m.bounds);
        let moved = |b: Bounds<ScaledPixels>, by: Point<ScaledPixels>| Bounds { origin: b.origin + by, size: b.size };
        let recorded: Vec<_> = kept.quads.iter().map(|q| shows(&q.bounds, &q.content_mask)).chain(kept.mono_staging.iter().map(|g| shows(&g.bounds, &g.content_mask))).collect();
        assert_eq!(recorded.len(), 4);
        for by in [point(ScaledPixels(0.0), ScaledPixels(0.0)), point(ScaledPixels(30.0), ScaledPixels(-5.0))] {
            // Held by something that cuts none of it (its reach, moved,
            // and room to spare on one side only).
            let hold = moved(kept.extent(), by);
            let hold = Bounds { origin: hold.origin, size: size(hold.size.width + ScaledPixels(50.0), hold.size.height) };
            let mut frame = Scene::default();
            frame.append(&kept, by, &hold);
            let got: Vec<_> = frame.quads.iter().map(|q| shows(&q.bounds, &q.content_mask)).chain(frame.mono_staging.iter().map(|g| shows(&g.bounds, &g.content_mask))).collect();
            assert_eq!(got, recorded.iter().map(|r| moved(*r, by)).collect::<Vec<_>>(), "moved by {by:?}");
            assert_eq!(frame.len(), 4);
            // Cut on the right, through the cell: what's left of each,
            // and the glyph past the cut gone.
            let cut = Bounds { origin: hold.origin, size: size(ScaledPixels(70.0), hold.size.height) };
            let mut frame = Scene::default();
            frame.append(&kept, by, &cut);
            let got: Vec<_> = frame.quads.iter().map(|q| shows(&q.bounds, &q.content_mask)).chain(frame.mono_staging.iter().map(|g| shows(&g.bounds, &g.content_mask))).collect();
            let left: Vec<_> = recorded.iter().map(|r| moved(*r, by).intersect(&cut)).filter(|r| !r.is_empty()).collect();
            assert_eq!(got, left, "cut, moved by {by:?}");
            assert_eq!(frame.len(), 3);
        }
    }

    /// A drawing made again with one thing changed (a key typed in a
    /// text area: a glyph and the caret) differs from the last only
    /// there; one holding things it doesn't compare (a shadow) says it
    /// can't tell.
    #[test]
    fn a_drawing_made_again_differs_only_where_it_changed() {
        let paint = |caret: f32, last: u32| {
            let mut kept = Scene::recording();
            kept.insert_primitive(quad(0.0, 0.0, 400.0, 300.0));
            for row in 0..10 {
                let mut line: Vec<_> = (0..8).map(|i| sprite(i as f32 * 8.0, 0, row * 10 + if row == 3 && i == 7 { last } else { i })).collect();
                for glyph in &mut line {
                    glyph.bounds.origin.y = ScaledPixels(row as f32 * 20.0);
                }
                kept.insert_sprites(line);
            }
            kept.insert_primitive(quad(caret, 60.0, 2.0, 16.0));
            kept
        };
        let was = paint(64.0, 7);
        let mut out = Vec::new();
        assert!(paint(64.0, 7).changed_since(&was, &mut out));
        assert!(out.is_empty(), "{out:?}");
        // A glyph typed at the end of row 3, the caret a glyph along.
        let now = paint(72.0, 99);
        assert!(now.changed_since(&was, &mut out));
        let around = out.iter().copied().reduce(|a, b| a.union(&b)).unwrap();
        assert!(around.contains(&point(ScaledPixels(57.0), ScaledPixels(61.0))) && around.contains(&point(ScaledPixels(73.0), ScaledPixels(75.0))), "{out:?}");
        assert!(out.iter().all(|r| r.origin.y.0 >= 59.0 && r.origin.y.0 + r.size.height.0 <= 77.0), "only row 3: {out:?}");
        // With a shadow that the last one hadn't: it can't tell where
        // (and says nothing).
        let shadow = |blur: f32| Shadow {
            order: 0,
            blur_radius: ScaledPixels(blur),
            bounds: mask(10.0, 10.0, 50.0, 20.0).bounds,
            corner_radii: Corners::default(),
            content_mask: mask(-1e4, -1e4, 2e4, 2e4),
            color: Hsla::default(),
        };
        let with = |caret: f32, last: u32, blur: f32| {
            let mut kept = paint(caret, last);
            kept.insert_primitive(shadow(blur));
            kept
        };
        out.clear();
        assert!(!with(72.0, 99, 4.0).changed_since(&was, &mut out));
        assert!(out.is_empty());
        // The same shadow in both: the rest is compared as ever. The
        // same drawing made again is the same (nothing to show), and one
        // with a glyph typed differs only there.
        let was = with(64.0, 7, 4.0);
        assert!(with(64.0, 7, 4.0).changed_since(&was, &mut out));
        assert!(out.is_empty(), "{out:?}");
        assert!(with(72.0, 99, 4.0).changed_since(&was, &mut out));
        assert!(!out.is_empty() && out.iter().all(|r| r.origin.y.0 >= 59.0 && r.origin.y.0 + r.size.height.0 <= 77.0), "only row 3: {out:?}");
        // The shadow itself changed: it can't tell where.
        out.clear();
        assert!(!with(64.0, 7, 5.0).changed_since(&was, &mut out));
        assert!(out.is_empty());
    }

    /// The things compared only whole: a drawing with a ring's slice, a
    /// path and a picture's triangle made again the same is the same; any
    /// one of them changed, and it can't tell where.
    #[test]
    fn a_drawing_with_paths_made_again_the_same_is_the_same() {
        let sector = |sweep: f32| Sector {
            order: 0,
            pad: 0,
            bounds: mask(0.0, 0.0, 40.0, 40.0).bounds,
            content_mask: mask(-1e4, -1e4, 2e4, 2e4),
            color: Hsla::default(),
            center: point(ScaledPixels(20.0), ScaledPixels(20.0)),
            inner: ScaledPixels(8.0),
            outer: ScaledPixels(18.0),
            start: 0.0,
            sweep,
        };
        let path = |to: f32| {
            let mut p = Path::new(point(crate::px(0.0), crate::px(0.0)));
            p.line_to(point(crate::px(to), crate::px(0.0)));
            p.line_to(point(crate::px(to), crate::px(30.0)));
            // (Clipped to nothing it would be left out of the drawing.)
            let mut p = p.scale(1.0);
            p.content_mask = mask(-1e4, -1e4, 2e4, 2e4);
            p
        };
        let paint = |sweep: f32, to: f32| {
            let mut kept = Scene::recording();
            kept.insert_primitive(quad(0.0, 0.0, 400.0, 300.0));
            kept.insert_primitive(sector(sweep));
            kept.insert_primitive(path(to));
            assert_eq!((kept.sectors.len(), kept.paths.len()), (1, 1));
            kept
        };
        let was = paint(1.0, 50.0);
        let mut out = Vec::new();
        assert!(paint(1.0, 50.0).changed_since(&was, &mut out));
        assert!(out.is_empty(), "{out:?}");
        assert!(!paint(1.5, 50.0).changed_since(&was, &mut out), "the slice went further round");
        assert!(!paint(1.0, 60.0).changed_since(&was, &mut out), "the path reaches further");
        assert!(out.is_empty());
        // One built the same from its curves, a path of its own.
        let outlined = |x: f32| {
            let mut b = crate::PathBuilder::fill();
            b.move_to(point(crate::px(0.0), crate::px(0.0)));
            b.line_to(point(crate::px(x), crate::px(0.0)));
            b.line_to(point(crate::px(x), crate::px(20.0)));
            b.close();
            let mut built = b.build().unwrap().scale(1.0);
            built.content_mask = mask(-1e4, -1e4, 2e4, 2e4);
            let mut kept = Scene::recording();
            kept.insert_primitive(built);
            assert_eq!(kept.paths.len(), 1);
            kept
        };
        let was = outlined(40.0);
        assert!(outlined(40.0).changed_since(&was, &mut out));
        assert!(out.is_empty());
        assert!(!outlined(41.0).changed_since(&was, &mut out));
    }

    /// A line painted again whole (`insert_kept_run`, what it reaches
    /// and its textures known) goes into a recording as it would glyph
    /// by glyph: the same orders, runs, reach and count.
    #[test]
    fn a_line_put_in_whole_is_as_one_put_in_glyph_by_glyph() {
        let page = |whole: bool| {
            let mut kept = Scene::recording();
            kept.insert_primitive(quad(0.0, 0.0, 400.0, 300.0));
            for row in 0..12 {
                let y = row as f32 * 20.0;
                // (Every third row: glyphs of two textures, interleaved.)
                let mut line: Vec<_> = (0..9).map(|i| sprite(i as f32 * 8.0, if row % 3 == 0 { i % 2 } else { 0 }, row * 10 + i)).collect();
                for glyph in &mut line {
                    glyph.bounds.origin.y = ScaledPixels(y);
                }
                if whole {
                    put_kept(&mut kept, line, made(row as u64 + 1));
                } else {
                    kept.insert_sprites(line);
                }
                // A rule over every other row: the next row's glyphs go
                // above it.
                if row % 2 == 0 {
                    kept.insert_primitive(quad(0.0, y + 4.0, 400.0, 1.0));
                }
            }
            kept
        };
        let (each, whole) = (page(false), page(true));
        let glyphs = |s: &Scene| {
            let mut all = Vec::new();
            for run in &s.mono_runs {
                run.glyphs_onto(s, &mut all);
            }
            all
        };
        assert!(same_sprites(&glyphs(&each), &glyphs(&whole)));
        // (Whole, the glyphs are the kept run's, shared, not copied.)
        assert!(whole.mono_staging.is_empty() && whole.mono_runs.iter().all(|r| r.kept.is_some()));
        let runs = |s: &Scene| s.mono_runs.iter().map(|r| (r.order, r.texture, r.range.len(), r.reach)).collect::<Vec<_>>();
        assert_eq!(runs(&each), runs(&whole));
        assert_eq!((each.depth(), each.extent(), each.len()), (whole.depth(), whole.extent(), whole.len()));
        assert_eq!(each.quads.iter().map(|q| q.order).collect::<Vec<_>>(), whole.quads.iter().map(|q| q.order).collect::<Vec<_>>());
        // Made whole, each run says how, a line's two textures each a
        // part of it; glyph by glyph, none does.
        let parts: Vec<_> = whole.mono_runs.iter().map(|r| r.made.map(|m| (m.found, m.part))).collect();
        assert_eq!(parts[..3], [Some((1, 0)), Some((1, 1)), Some((2, 0))]);
        assert!(each.mono_runs.iter().all(|r| r.made.is_none()));
    }

    /// Made from kept run `found`, at the window's corner, in the clip
    /// `sprite` gives each glyph.
    fn made(found: u64) -> RunMade {
        let c = sprite(0.0, 0, 0).content_mask.bounds;
        RunMade { found, at: [0, 0], clip: [c.origin.x.0.to_bits(), c.origin.y.0.to_bits(), c.size.width.0.to_bits(), c.size.height.0.to_bits()], opacity: 1.0f32.to_bits(), part: 0 }
    }

    /// A line's glyphs (each as `sprite` gives it) put in a recording as
    /// a kept run, as `Painter::paint_glyph_run` puts one.
    fn put_kept(kept: &mut Scene, mut line: Vec<MonochromeSprite>, made: RunMade) {
        let at = made.placing().at;
        let reach = line.iter().map(|g| Bounds { origin: g.bounds.origin + at, size: g.bounds.size }).reduce(|a, b| a.union(&b)).unwrap();
        line.sort_by_key(|g| texture_key(&g.tile));
        let mut parts: Vec<((u8, u32), u32)> = Vec::new();
        for (i, g) in line.iter().enumerate() {
            match parts.last_mut() {
                Some((t, end)) if *t == texture_key(&g.tile) => *end = i as u32 + 1,
                _ => parts.push((texture_key(&g.tile), i as u32 + 1)),
            }
        }
        kept.insert_kept_run(&line.into(), &parts, reach, made);
    }

    /// A drawing made again from kept runs is compared with the last
    /// line by line: a line made the same way is the same glyphs, unread;
    /// one made another way is compared glyph by glyph, and only what
    /// differs in it changed.
    #[test]
    fn lines_made_the_same_way_are_the_same_unread() {
        let page = |found: &dyn Fn(u32) -> u64, tile: &dyn Fn(u32, u32) -> u32| {
            let mut kept = Scene::recording();
            for row in 0..20 {
                let mut line: Vec<_> = (0..40).map(|i| sprite(i as f32 * 8.0, 0, tile(row, i))).collect();
                for glyph in &mut line {
                    glyph.bounds.origin.y = ScaledPixels(row as f32 * 20.0);
                }
                put_kept(&mut kept, line, made(found(row)));
            }
            kept
        };
        let tiles = |row: u32, i: u32| row * 100 + i;
        let was = page(&|row| row as u64 + 1, &tiles);
        let mut out = Vec::new();
        // Each line made again as it was: nothing changed.
        assert!(page(&|row| row as u64 + 1, &tiles).changed_since(&was, &mut out));
        assert!(out.is_empty(), "{out:?}");
        // Row 9 found again (its glyphs looked up anew) and the same:
        // compared, and nothing changed.
        assert!(page(&|row| if row == 9 { 99 } else { row as u64 + 1 }, &tiles).changed_since(&was, &mut out));
        assert!(out.is_empty(), "{out:?}");
        // Row 9 found again with its fifth glyph another: that glyph.
        let changed = |row: u32, i: u32| if (row, i) == (9, 4) { 7777 } else { tiles(row, i) };
        assert!(page(&|row| if row == 9 { 99 } else { row as u64 + 1 }, &changed).changed_since(&was, &mut out));
        assert_eq!(out, vec![mask(32.0, 180.0, 8.0, 8.0).bounds]);
        // A line said to be made the same way is taken at its word: its
        // glyphs aren't read (what makes it so is the kept run's).
        out.clear();
        assert!(page(&|row| row as u64 + 1, &changed).changed_since(&was, &mut out));
        assert!(out.is_empty(), "{out:?}");
    }

    /// Whether what two drawings show outside `out` (what is said to
    /// have changed) is the same: their glyphs in place, in the order
    /// they are drawn, each as often as it is, but for those that show
    /// nothing outside it (a glyph wholly inside what changed may be
    /// anything, and one that shows nothing too). What a comparison that
    /// can be trusted leaves true.
    fn same_outside(was: &Scene, now: &Scene, out: &[Bounds<ScaledPixels>]) -> bool {
        let outside = |s: &Scene| {
            let mut all = Vec::new();
            for run in &s.mono_runs {
                run.glyphs_onto(s, &mut all);
            }
            all.retain(|g| {
                // (One turned or moved by its matrix shows anywhere in its clip.)
                let shows = if g.transformation == TransformationMatrix::unit() { g.bounds.intersect(&g.content_mask.bounds) } else { g.content_mask.bounds };
                !(shows.is_empty() || out.iter().any(|r| shows.is_contained_within(r)))
            });
            all
        };
        let (a, b) = (outside(was), outside(now));
        a.len() == b.len() && same_sprites(&a, &b)
    }

    /// A screen of lines put in whole, with the lines `text` gives (each
    /// its length and what tells its glyphs from another line's), the
    /// first of them at the top: `from` is how far it has scrolled.
    fn screen(from: u32, rows: u32, text: &dyn Fn(u32) -> (u32, u32), whole: bool) -> Scene {
        let mut kept = Scene::recording();
        for row in 0..rows {
            let (length, id) = text(from + row);
            let mut line: Vec<_> = (0..length).map(|i| sprite(i as f32 * 8.0, 0, id * 1000 + i)).collect();
            for glyph in &mut line {
                glyph.bounds.origin.y = ScaledPixels(row as f32 * 20.0);
            }
            match whole {
                // (A line found once keeps its finding as it scrolls.)
                true => put_kept(&mut kept, line, made(u64::from(from + row) + 1)),
                false => kept.insert_sprites(line),
            }
        }
        kept
    }

    /// A screen of text scrolled a line (each line where the next was,
    /// a new one at the foot), its lines put in whole, is compared line
    /// with line as they are kept: what changed holds every glyph that
    /// went or came, as the same screen compared glyph by glyph says,
    /// and no more than the lines themselves reach.
    #[test]
    fn a_screen_scrolled_is_compared_line_with_line() {
        // Lines of many lengths, as a program prints them.
        let ragged = |n: u32| (10 + n * 7 % 30, n);
        let (was, now) = (screen(0, 24, &ragged, true), screen(1, 24, &ragged, true));
        let mut out = Vec::new();
        assert!(now.changed_since(&was, &mut out));
        assert!(same_outside(&was, &now, &out));
        // One piece, the lines' own reach: 39 glyphs at the widest, 24 rows.
        assert_eq!(out, vec![mask(0.0, 0.0, 39.0 * 8.0, 23.0 * 20.0 + 8.0).bounds]);
        // The same screens glyph by glyph (no line put in whole) say the
        // same of what changed, by reading every glyph.
        let (was_each, now_each) = (screen(0, 24, &ragged, false), screen(1, 24, &ragged, false));
        let mut each = Vec::new();
        assert!(now_each.changed_since(&was_each, &mut each));
        assert!(same_outside(&was_each, &now_each, &each));
        assert_eq!(out, each);
        // Not scrolled: nothing, either way.
        out.clear();
        assert!(screen(0, 24, &ragged, true).changed_since(&was, &mut out));
        assert!(out.is_empty(), "{out:?}");
        // As a terminal puts its lines: each line's glyphs from the
        // line's own corner, the line put at its row (so a line that
        // scrolls is the same kept glyphs, put a row higher).
        let rows = |from: u32| {
            let mut kept = Scene::recording();
            for row in 0..24u32 {
                let (length, id) = ragged(from + row);
                let line: Vec<_> = (0..length).map(|i| sprite(i as f32 * 8.0, 0, id * 1000 + i)).collect();
                let at = RunMade { at: [0f32.to_bits(), (row as f32 * 20.0).to_bits()], ..made(u64::from(from + row) + 1) };
                put_kept(&mut kept, line, at);
            }
            kept
        };
        let (was, now) = (rows(0), rows(1));
        out.clear();
        assert!(now.changed_since(&was, &mut out));
        assert!(same_outside(&was, &now, &out));
        assert_eq!(out, each);
        out.clear();
        assert!(rows(0).changed_since(&was, &mut out));
        assert!(out.is_empty(), "{out:?}");
    }

    /// What the glyphs' comparison says of two drawings, the way it is
    /// done and the way it was (every run between the ends written out
    /// and gone through): the same, which is what makes the first right.
    fn both_ways(was: &Scene, now: &Scene) -> Vec<Bounds<ScaledPixels>> {
        let (mut out, mut by_copies) = (Vec::new(), Vec::new());
        now.glyphs_changed_since(was, &mut out);
        now.glyphs_changed_by_copies(was, &mut by_copies);
        assert_eq!(out, by_copies, "walking the glyphs in place and writing them out disagree");
        out
    }

    /// A line put in whole as the painter puts one (`paint_glyph_run`):
    /// its glyphs (where each is, in the window; its texture; its tile;
    /// how see-through) kept from the line's corner `base`, what they
    /// reach taken there and then moved, and the line put at `base`,
    /// `opacity` over it all.
    fn put_line(s: &mut Scene, found: u64, base: (f32, f32), opacity: f32, glyphs: &[(f32, u32, u32, f32)]) {
        let at = RunMade { at: [base.0.to_bits(), base.1.to_bits()], opacity: opacity.to_bits(), ..made(found) };
        let mut line: Vec<_> = glyphs
            .iter()
            .map(|&(x, texture, id, alpha)| {
                let mut g = sprite(x - base.0, texture, id);
                g.color = crate::hsla(0.0, 0.5, 0.5, alpha);
                g.content_mask = ContentMask::default();
                g
            })
            .collect();
        let mut reach = line.iter().map(|g| g.bounds).reduce(|a, b| a.union(&b)).unwrap();
        reach.origin = reach.origin + at.placing().at;
        line.sort_by_key(|g| texture_key(&g.tile));
        let mut parts: Vec<((u8, u32), u32)> = Vec::new();
        for (i, g) in line.iter().enumerate() {
            match parts.last_mut() {
                Some((t, end)) if *t == texture_key(&g.tile) => *end = i as u32 + 1,
                _ => parts.push((texture_key(&g.tile), i as u32 + 1)),
            }
        }
        s.insert_kept_run(&line.into(), &parts, reach, at);
    }

    /// Lines of one length that differ in a few glyphs (a count going
    /// up, the rest of each line the same): what changed runs from the
    /// first glyph that differs to the last, as it did.
    #[test]
    fn lines_alike_in_place_differ_from_the_first_glyph_to_the_last() {
        // Each line 40 glyphs: the fifth is the line's own, the rest the
        // same in every line.
        let page = |from: u32| {
            let mut kept = Scene::recording();
            for row in 0..10u32 {
                let mut line: Vec<_> = (0..40).map(|i| sprite(i as f32 * 8.0, 0, if i == 4 { 5000 + from + row } else { i })).collect();
                for glyph in &mut line {
                    glyph.bounds.origin.y = ScaledPixels(row as f32 * 20.0);
                }
                put_kept(&mut kept, line, made(u64::from(from + row) + 1));
            }
            kept
        };
        let (was, now) = (page(0), page(1));
        let out = both_ways(&was, &now);
        assert!(same_outside(&was, &now, &out));
        assert_eq!(out, vec![mask(0.0, 0.0, 320.0, 9.0 * 20.0 + 8.0).bounds]);
        // One line of them changed: its fifth glyph, and no more.
        let one = |changed: bool| {
            let mut kept = Scene::recording();
            for row in 0..10u32 {
                let mut line: Vec<_> = (0..40).map(|i| sprite(i as f32 * 8.0, 0, if changed && (row, i) == (6, 4) { 9999 } else { row * 100 + i })).collect();
                for glyph in &mut line {
                    glyph.bounds.origin.y = ScaledPixels(row as f32 * 20.0);
                }
                put_kept(&mut kept, line, made(if changed && row == 6 { 77 } else { u64::from(row) + 1 }));
            }
            kept
        };
        assert_eq!(both_ways(&one(false), &one(true)), vec![mask(32.0, 120.0, 8.0, 8.0).bounds]);
    }

    /// Drawings that come to the same glyphs in place by another way
    /// are the same, and nothing is to be shown (a frame for nothing
    /// otherwise): a line's corner moved and its glyphs moved back; how
    /// see-through, in the glyph or over the line; a colour that the
    /// line's opacity brings to the same; the sign of a zero; the same
    /// glyphs in lines cut elsewhere. And a line with a glyph more, or
    /// one glyph of a second texture changed, has changed there, not
    /// along its whole length. (Astra's review of the first way this
    /// was done, 2026-11-22: each of these it got wrong.)
    #[test]
    fn the_same_in_place_by_another_way_is_the_same() {
        // (Each drawing's lines found anew, as lines of other glyphs
        // are: none is passed as made the same way, all are compared.)
        let found = std::cell::Cell::new(0u64);
        let anew = || {
            found.set(found.get() + 1);
            found.get()
        };
        let one = |base: f32, opacity: f32, alpha: f32| {
            let mut s = Scene::recording();
            put_line(&mut s, anew(), (base, 0.0), opacity, &[(15.0, 0, 7, alpha)]);
            s
        };
        let same = |was: Scene, now: Scene| {
            assert!(was.mono_runs.iter().zip(&now.mono_runs).all(|(a, b)| a.made != b.made));
            assert!(both_ways(&was, &now).is_empty());
        };
        // Its corner a pixel on, its glyph a pixel back.
        same(one(10.0, 1.0, 1.0), one(11.0, 1.0, 1.0));
        // Half see-through in the glyph, or over the line.
        same(one(10.0, 1.0, 0.5), one(10.0, 0.5, 1.0));
        // Nothing of it shows: whatever its own.
        same(one(10.0, 0.0, 0.5), one(10.0, 0.0, 1.0));
        // Two that the line's opacity rounds to one.
        same(one(10.0, 0.75, f32::from_bits(0x3f40_0002)), one(10.0, 0.75, f32::from_bits(0x3f40_0003)));
        // A corner at nothing, and at nothing less.
        let zero = |base: f32| {
            let mut s = Scene::recording();
            put_line(&mut s, anew(), (base, 0.0), 1.0, &[(15.0, 0, 7, 1.0)]);
            s
        };
        same(zero(0.0), zero(-0.0));
        // Three glyphs as two lines, cut after the second and after the first.
        let cut = |after: usize| {
            let glyphs = [(0.0, 0, 1, 1.0), (8.0, 0, 2, 1.0), (16.0, 0, 3, 1.0)];
            let mut s = Scene::recording();
            put_line(&mut s, anew(), (0.0, 0.0), 1.0, &glyphs[..after]);
            put_line(&mut s, anew(), (0.0, 0.0), 1.0, &glyphs[after..]);
            s
        };
        same(cut(2), cut(1));
        // A glyph more at a line's end: that glyph.
        let line = |n: usize| {
            let glyphs = [(0.0, 0, 1, 1.0), (8.0, 0, 2, 1.0), (16.0, 0, 3, 1.0)];
            let mut s = Scene::recording();
            put_line(&mut s, n as u64, (0.0, 0.0), 1.0, &glyphs[..n]);
            s
        };
        assert_eq!(both_ways(&line(2), &line(3)), vec![mask(16.0, 0.0, 8.0, 8.0).bounds]);
        // A line of two textures, the one glyph of its second another:
        // that glyph, not the line.
        let two = |id: u32| {
            let mut s = Scene::recording();
            put_line(&mut s, u64::from(id), (0.0, 0.0), 1.0, &[(0.0, 0, 1, 1.0), (8.0, 0, 2, 1.0), (128.0, 1, id, 1.0)]);
            s
        };
        assert_eq!(both_ways(&two(5), &two(6)), vec![mask(128.0, 0.0, 8.0, 8.0).bounds]);
    }

    /// Many drawings of lines, each a little other than the last in one
    /// of the ways drawings change (a line's glyph, its length, where
    /// it is, a line more or less, lines of two textures, lines put in
    /// whole and glyph by glyph): the comparison in place says of each
    /// pair what the comparison by copies says.
    #[test]
    fn in_place_and_by_copies_say_the_same_of_many_drawings() {
        // (A small generator of its own: the same drawings every run.)
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move |n: u32| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 33) as u32 % n
        };
        // A drawing: its lines (row, how many glyphs, what tells them
        // from another's, whether put in whole, whether of two textures,
        // and for one put glyph by glyph what is odd about it: nothing;
        // cut by its clip at a part of a pixel; a glyph of it millions
        // of pixels wide, cut likewise; one turned). The odd ones are
        // where sums of bounds round, and a union taken in another order
        // comes to other bits: the two ways must still agree to the bit.
        type Line = (u32, u32, u32, bool, bool, u32);
        let draw = |lines: &[Line]| {
            let mut s = Scene::recording();
            for (n, &(row, length, id, whole, two, odd)) in lines.iter().enumerate() {
                let y = row as f32 * 20.0;
                let glyphs: Vec<(f32, u32, u32, f32)> = (0..length).map(|i| (i as f32 * 8.0, u32::from(two && i % 5 == 4), id * 1000 + i, 1.0)).collect();
                if whole {
                    // (Some begin left of the rest: what changed grows
                    // that way too.)
                    let left = -((id % 4) as f32) * 24.0;
                    let glyphs: Vec<_> = glyphs.iter().map(|&(x, texture, tile, alpha)| (x + left, texture, tile, alpha)).collect();
                    put_line(&mut s, u64::from(id) * 100 + n as u64, (left, y), 1.0, &glyphs);
                } else {
                    s.insert_sprites(glyphs.iter().enumerate().map(|(i, &(x, texture, tile, _))| {
                        let mut g = sprite(x, texture, tile);
                        g.bounds.origin.y = ScaledPixels(y);
                        g.color = crate::hsla(0.0, 0.5, 0.5, 1.0);
                        match odd {
                            1 => g.content_mask = mask(2.0, y, 6.000005 + (id % 3) as f32 * 8.3, 8.0),
                            2 if i == 0 => {
                                let far = if id % 2 == 0 { 12_000_000.0 } else { 6_000_000.0 };
                                g.bounds = Bounds { origin: point(ScaledPixels(-far), ScaledPixels(y)), size: size(ScaledPixels(far + 16.0), ScaledPixels(8.0)) };
                                g.content_mask = mask(-2e7, y, 2e7 + 8.625, 8.0);
                            }
                            3 if i == 0 => {
                                g.transformation = TransformationMatrix::unit().translate(point(ScaledPixels(32.0), ScaledPixels(0.0)));
                                g.content_mask = mask(0.0, y, 64.5, 8.0);
                            }
                            _ => {}
                        }
                        g
                    }));
                }
            }
            s
        };
        let far = |lines: &[Line]| lines.iter().any(|l| !l.3 && l.5 == 2);
        let mut lines: Vec<Line> = (0..16).map(|row| (row, 6 + row * 5 % 23, row, true, row % 7 == 3, 0)).collect();
        let (mut changed, mut odd) = (0, 0);
        for step in 0..1200 {
            let was = (draw(&lines), lines.clone());
            match next(9) {
                // A line's text another.
                0 => {
                    let i = next(lines.len() as u32) as usize;
                    lines[i].2 = 500 + step;
                }
                // Longer or shorter.
                1 => {
                    let i = next(lines.len() as u32) as usize;
                    lines[i].1 = 1 + next(40);
                }
                // Scrolled a line: each where the next was, a new one at the foot.
                2 => {
                    let rows: Vec<u32> = lines.iter().map(|l| l.0).collect();
                    lines.remove(0);
                    lines.push((0, 3 + next(36), 700 + step, true, next(5) == 0, 0));
                    for (line, row) in lines.iter_mut().zip(rows) {
                        line.0 = row;
                    }
                }
                // A line more, or one less.
                3 if lines.len() < 30 => lines.push((lines.len() as u32, 1 + next(30), 900 + step, next(3) > 0, false, next(4))),
                4 if lines.len() > 2 => {
                    let i = next(lines.len() as u32) as usize;
                    lines.remove(i);
                }
                // Put in whole, or glyph by glyph; of two textures, or one.
                5 => {
                    let i = next(lines.len() as u32) as usize;
                    lines[i].3 = !lines[i].3;
                }
                6 => {
                    let i = next(lines.len() as u32) as usize;
                    lines[i].4 = !lines[i].4;
                }
                // Odd another way (a line put glyph by glyph).
                7 => {
                    let i = next(lines.len() as u32) as usize;
                    lines[i].5 = next(4);
                }
                // As it was.
                _ => {}
            }
            let now = draw(&lines);
            let out = both_ways(&was.0, &now);
            // (Bounds millions of pixels wide round as they are joined:
            // what is said of those is held to the old way's, above, not
            // to where each glyph is to the bit.)
            if !far(&was.1) && !far(&lines) {
                assert!(same_outside(&was.0, &now, &out), "step {step}");
            }
            changed += usize::from(!out.is_empty());
            odd += usize::from(lines.iter().any(|l| !l.3 && l.5 > 0));
        }
        assert!(changed > 400 && odd > 400, "{changed} of 1200 steps changed something, {odd} with a line odd in some way");
    }

    /// What changed is joined glyph by glyph in the order the glyphs
    /// written out were gone through, to the same bits, where joining
    /// rounds: a glyph cut by its clip at 8.000005, then a line put in
    /// whole with glyphs at -24 and at -56, and then the line alone.
    /// Joined one by one the right edge comes to 8; with the line's
    /// reach taken in one go, to 8.000008, a pixel more once rounded out
    /// (Astra's second review, 2026-11-22). A line is taken by its reach
    /// only where what is joined so far and the reach are whole numbers
    /// that add exactly; here it is read glyph by glyph.
    #[test]
    fn what_changed_is_joined_as_it_was_where_joining_rounds() {
        let draw = |tiles: u32, cut: Option<f32>| {
            let mut s = Scene::recording();
            if let Some(width) = cut {
                let mut glyph = sprite(2.0, 0, tiles);
                glyph.content_mask = mask(2.0, 0.0, width, 8.0);
                s.insert_sprites([glyph]);
            }
            put_line(&mut s, u64::from(tiles), (-56.0, 0.0), 1.0, &[(-24.0, 0, tiles + 1, 1.0), (-56.0, 0, tiles + 2, 1.0)]);
            s
        };
        let (was, now) = (draw(10, Some(6.000005)), draw(20, None));
        // (The line is one put in whole, of one texture, all of it changed.)
        assert!(was.mono_runs[1].shows_whole().is_some() && now.mono_runs[0].shows_whole().is_some());
        let out = both_ways(&was, &now);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].origin.x.0, out[0].origin.x.0 + out[0].size.width.0), (-56.0, 8.0));
        // After a glyph of whole numbers the line is taken by its reach,
        // and comes to the same as its glyphs.
        assert_eq!(both_ways(&draw(10, Some(6.0)), &draw(20, None)), vec![mask(-56.0, 0.0, 64.0, 8.0).bounds]);
    }

    /// What holds the comparison to its word sees the order glyphs are
    /// drawn in, and how many of each: two glyphs one over the other,
    /// drawn the other way round, are not the same with nothing said to
    /// have changed, nor is a glyph drawn twice where it was drawn once.
    #[test]
    fn what_checks_the_comparison_sees_order_and_count() {
        let pair = |reversed: bool, twice: bool| {
            let mut red = sprite(0.0, 0, 1);
            red.color = crate::hsla(0.0, 1.0, 0.5, 1.0);
            let blue = MonochromeSprite { color: crate::hsla(2.0 / 3.0, 1.0, 0.5, 1.0), ..red.clone() };
            let mut glyphs = vec![red, blue.clone()];
            if reversed {
                glyphs.reverse();
            }
            if twice {
                glyphs.push(blue);
            }
            let mut s = Scene::recording();
            s.insert_sprites(glyphs);
            s
        };
        let whole = [mask(0.0, 0.0, 8.0, 8.0).bounds];
        assert!(same_outside(&pair(false, false), &pair(false, false), &[]));
        assert!(!same_outside(&pair(false, false), &pair(true, false), &[]));
        assert!(!same_outside(&pair(false, false), &pair(false, true), &[]));
        assert!(same_outside(&pair(false, false), &pair(true, true), &whole));
        // And the comparison says so of each.
        assert_eq!(both_ways(&pair(false, false), &pair(true, false)), whole);
        assert_eq!(both_ways(&pair(false, false), &pair(false, true)), whole);
    }

    /// A line is put in whole only where what it reaches is exactly
    /// what its glyphs show: the reach is taken from the line's corner
    /// and moved, the glyphs are each moved, and far enough from the
    /// corner the two no longer come to the same (Astra's review,
    /// 2026-11-22: two glyphs at 0 and 64, their line's corner half a
    /// billion pixels off, reached 64 pixels and showed 72; a frame
    /// drawing the last 8 passed the line over). Such a line is put
    /// glyph by glyph, and a line on a screen whole, as ever.
    #[test]
    fn a_line_is_put_whole_only_where_its_reach_is_its_glyphs() {
        use crate::painter::{Gfx, Notes};
        use crate::{AtlasKey, FontId, GlyphId, GlyphRun, NoopTextSystem, Painter, PlatformAtlas, SvgRenderer, TextSystem, WindowTextSystem, px};
        use std::borrow::Cow;
        /// (No glyph is drawn into it: the one painted is found already.)
        struct NoAtlas;
        impl PlatformAtlas for NoAtlas {
            fn get_or_insert_with<'a>(&self, _: &AtlasKey, _: &mut dyn FnMut() -> anyhow::Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>) -> anyhow::Result<Option<AtlasTile>> {
                unreachable!("the glyph painted is one found already")
            }
            fn remove(&self, _: &AtlasKey) {}
        }
        let mut gfx = Gfx {
            scale_factor: 1.0,
            rem_size: px(16.0),
            text_rendering: Default::default(),
            text_system: Arc::new(WindowTextSystem::new(Arc::new(TextSystem::new(Arc::new(NoopTextSystem::new()))))),
            sprite_atlas: Arc::new(NoAtlas),
            svg_renderer: SvgRenderer::new(Arc::new(())),
            glyph_memo: Default::default(),
            outlines: Default::default(),
            text_style: Default::default(),
            ext: None,
        };
        // One glyph, 8 by 8, found.
        let tile = sprite(0.0, 0, 7).tile;
        gfx.glyph_memo.insert((0, 7, 12f32.to_bits(), 0, 0), Some((tile.bounds, tile)));
        // Two of it at 0 and 64, in a line whose corner is at `corner`.
        let mut paint = |corner: f32, alpha: f32| {
            let mut s = Scene::recording();
            s.insert_primitive(quad(0.0, 0.0, 800.0, 600.0));
            let (mut notes, mut state) = (Notes::default(), None);
            let mut p = Painter::new(&mut gfx, &mut s, &mut notes, &mut state, Bounds::default(), 1.0, vec![]);
            p.paint_glyph_run(&mut GlyphRun::default(), point(px(corner), px(16.0)), 0, |emit| {
                for x in [0.0, 64.0] {
                    emit(point(px(x - corner), px(0.0)), FontId(0), GlyphId(7), px(12.0), crate::hsla(0.0, 1.0, 0.5, alpha));
                }
            });
            s.seal();
            s
        };
        let both = mask(0.0, 16.0, 72.0, 8.0).bounds;
        let second = mask(64.0, 16.0, 8.0, 8.0).bounds;
        // What shows of a frame drawing only the second glyph's place (a
        // line put in whole is drawn whole, in the frame's clip).
        let drawn = |s: &Scene| {
            let mut frame = Scene::default();
            frame.append(s, Point::default(), &second);
            frame.finish();
            frame.drawn_glyphs().iter().map(|g| g.bounds.intersect(&g.content_mask.bounds)).filter(|shows| !shows.is_empty()).collect::<Vec<_>>()
        };
        for (corner, whole) in [(512.0, true), (4_194_304.0, true), (4_194_305.0, false), ((1u32 << 29) as f32, false)] {
            let (was, now) = (paint(corner, 1.0), paint(corner, 0.25));
            assert_eq!(now.mono_runs.len(), 1, "corner at {corner}");
            let run = &now.mono_runs[0];
            assert_eq!(run.kept.is_some(), whole, "corner at {corner}");
            // What it reaches is what its glyphs show, however it was put.
            let shown = run.placed(&now).map(|g| g.bounds.intersect(&g.content_mask.bounds)).reduce(|a, b| a.union(&b));
            assert_eq!(shown, Some(both), "corner at {corner}");
            assert_eq!(run.reach, both, "corner at {corner}");
            // Its glyphs a quarter as strong: both changed, and are said to have.
            let mut out = Vec::new();
            assert!(now.changed_since(&was, &mut out));
            assert_eq!(out, vec![both], "corner at {corner}");
            assert_eq!(both_ways(&was, &now), vec![both], "corner at {corner}");
            assert_eq!(drawn(&now), vec![second], "corner at {corner}");
        }
    }

    /// A line put in whole where a line glyph by glyph was (a drawing
    /// that began to keep its lines, or stopped): compared in place, as
    /// before, and right.
    #[test]
    fn a_line_kept_and_one_not_are_compared_in_place() {
        let ragged = |n: u32| (10 + n * 7 % 30, n);
        let (was, now) = (screen(0, 12, &ragged, false), screen(0, 12, &ragged, true));
        let mut out = Vec::new();
        // The same glyphs in the same places, however they were put in.
        assert!(now.changed_since(&was, &mut out));
        assert!(out.is_empty(), "{out:?}");
        let scrolled = screen(1, 12, &ragged, true);
        assert!(scrolled.changed_since(&was, &mut out));
        assert!(same_outside(&was, &scrolled, &out));
    }

    /// A frame drawing a part of a page of text takes the glyphs it
    /// would looking at each (the lines it reaches), passing over the
    /// lines away from it whole.
    #[test]
    fn a_frame_drawing_a_part_takes_only_the_lines_there() {
        // Lines put in glyph by glyph, and kept runs (each at its row's
        // pixel, every third at half its opacity) put in whole.
        for whole in [false, true] {
            let mut kept = Scene::recording();
            for row in 0..30 {
                let y = row as f32 * 20.0;
                let mut line: Vec<_> = (0..20).map(|i| sprite(i as f32 * 8.0, 0, row * 100 + i)).collect();
                for glyph in &mut line {
                    glyph.color.a = 1.0;
                }
                if whole {
                    let opacity = if row % 3 == 0 { 0.5f32 } else { 1.0 };
                    put_kept(&mut kept, line, RunMade { at: [0, y.to_bits()], opacity: opacity.to_bits(), ..made(row as u64 + 1) });
                } else {
                    for glyph in &mut line {
                        glyph.bounds.origin.y = ScaledPixels(y);
                    }
                    kept.insert_sprites(line);
                }
            }
            let mut recorded = Vec::new();
            for run in &kept.mono_runs {
                run.glyphs_onto(&kept, &mut recorded);
            }
            assert_eq!(recorded.len(), 600);
            assert_eq!(recorded[0].color.a, if whole { 0.5 } else { 1.0 });
            for (by, part) in [
                (0.0, mask(10.0, 61.0, 30.0, 10.0)),
                (5.0, mask(0.0, 0.0, 400.0, 3.0)),
                (-40.0, mask(150.0, 100.0, 100.0, 100.0)),
                (0.0, mask(0.0, 700.0, 10.0, 10.0)),
                (3.0, mask(0.0, 0.0, 400.0, 700.0)),
            ] {
                let by = point(ScaledPixels(0.0), ScaledPixels(by));
                let mut frame = Scene::default();
                frame.append(&kept, by, &part.bounds);
                // Each glyph moved and cut, kept if anything of it shows
                // (all of it showing: its own clip only moved).
                let all = kept.extent().is_contained_within(&Bounds { origin: part.bounds.origin - by, size: part.bounds.size });
                let shown: Vec<_> = recorded
                    .iter()
                    .filter_map(|g| {
                        let bounds = Bounds { origin: g.bounds.origin + by, size: g.bounds.size };
                        let moved = Bounds { origin: g.content_mask.bounds.origin + by, size: g.content_mask.bounds.size };
                        let clip = if all { moved } else { moved.intersect(&part.bounds) };
                        (!bounds.intersect(&clip).is_empty()).then_some((g.tile.tile_id, bounds, clip, g.color))
                    })
                    .collect();
                let got: Vec<_> = frame.mono_staging.iter().map(|g| (g.tile.tile_id, g.bounds, g.content_mask.bounds, g.color)).collect();
                assert_eq!(got, shown, "moved by {by:?}, part {part:?}, whole {whole}");
                assert_eq!(frame.len(), shown.len());
                assert!(frame.mono_runs.iter().all(|r| r.kept.is_none()));
            }
        }
    }

    /// Glyphs compared by their bytes find what changed as comparing
    /// each did: one glyph, or a line moved.
    #[test]
    fn glyphs_compared_by_their_bytes_find_what_changed() {
        let line = |row: u32, at: f32, last: u32| -> Vec<MonochromeSprite> {
            (0..40)
                .map(|i| {
                    let mut g = sprite(i as f32 * 8.0, 0, row * 100 + if i == 39 { last } else { i });
                    g.bounds.origin.y = ScaledPixels(at);
                    g
                })
                .collect()
        };
        let page = |changed: Option<(u32, f32, u32)>| {
            let mut kept = Scene::recording();
            for row in 0..20 {
                let (at, last) = match changed {
                    Some((r, at, last)) if r == row => (at, last),
                    _ => (row as f32 * 20.0, 39),
                };
                kept.insert_sprites(line(row, at, last));
            }
            kept
        };
        let was = page(None);
        let mut out = Vec::new();
        assert!(page(None).changed_since(&was, &mut out));
        assert!(out.is_empty(), "{out:?}");
        // The last glyph of row 7 another.
        assert!(page(Some((7, 140.0, 77))).changed_since(&was, &mut out));
        assert_eq!(out, vec![mask(312.0, 140.0, 8.0, 8.0).bounds]);
        // Row 12 moved down a pixel: where it was and where it is.
        out.clear();
        assert!(page(Some((12, 241.0, 39))).changed_since(&was, &mut out));
        assert_eq!(out, vec![mask(0.0, 240.0, 320.0, 9.0).bounds]);
        // And a zero's sign: taken for different.
        let mut signed = page(None);
        signed.mono_staging[5].bounds.origin.y = ScaledPixels(-0.0);
        out.clear();
        assert!(signed.changed_since(&was, &mut out));
        assert_eq!(out, vec![mask(40.0, 0.0, 8.0, 8.0).bounds]);
    }

    /// A shadow reaches three blur radii past its bounds: a recording
    /// holding one reaches that far, and a frame drawing only a part
    /// below what casts it (where the blur falls) keeps the shadow.
    #[test]
    fn a_shadow_is_drawn_where_its_blur_falls() {
        let mut kept = Scene::recording();
        let cast = mask(100.0, 100.0, 200.0, 40.0).bounds;
        kept.insert_primitive(Shadow { order: 0, blur_radius: ScaledPixels(6.0), bounds: cast, corner_radii: Corners::default(), content_mask: mask(-1e4, -1e4, 2e4, 2e4), color: Hsla::default() });
        let extent = kept.extent();
        assert_eq!(extent, cast.dilate(ScaledPixels(18.0)));
        // Just under it, within the blur.
        let mut frame = Scene::default();
        frame.append(&kept, point(ScaledPixels(0.0), ScaledPixels(0.0)), &mask(0.0, 145.0, 400.0, 10.0).bounds);
        assert_eq!(frame.shadows.len(), 1);
        // Past the blur: nothing.
        let mut frame = Scene::default();
        frame.append(&kept, point(ScaledPixels(0.0), ScaledPixels(0.0)), &mask(0.0, 160.0, 400.0, 10.0).bounds);
        assert!(frame.shadows.is_empty());
    }
}
