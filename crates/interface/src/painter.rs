//! Painting: what a view draws with.
//!
//! BeOS's `BView::Draw` drew straight to the screen each time app_server
//! asked. Here a view draws into a [`Painter`] only when what it shows
//! changed, and what it drew is kept (its display list): the window is
//! put together from those, and one that only moved (scrolled, or pushed
//! along by its neighbours) isn't drawn again at all.
//!
//! Everything is placed in the window's own coordinates, in points, and
//! lands on device pixels as Olo's GPUI put it there (the paint functions
//! are its own, from `Window`).

use std::{borrow::Cow, fmt::Debug, sync::Arc};

use anyhow::Result;
use collections::FxHashMap;

use crate::{
    AtlasTile, Background, BorderStyle, Bounds, BoxShadow, Corners, DevicePixels, Edges, FontId, GlyphId, Hsla, ImageTriangle, IsZero as _, MonochromeSprite, Path, Pixels, PlatformAtlas, Point,
    PolychromeSprite, Quad, RenderGlyphParams, RenderImage, RenderImageParams, RenderSvgParams, SUBPIXEL_VARIANTS_X, SUBPIXEL_VARIANTS_Y, ScaledPixels, Scene, Sector, Segment, Shadow, SharedString,
    StrikethroughStyle, SvgRenderer, TextRendering, TextStyle, TextStyleRefinement, TransformationMatrix, Underline, UnderlineStyle, WindowTextSystem, point, px, size, transparent_black,
};
use refineable::Refineable as _;

/// How much larger than its size on screen an SVG is rasterized, so its
/// edges stay smooth where it lands between pixels.
pub(crate) const SMOOTH_SVG_SCALE_FACTOR: f32 = 2.;

/// rendered. Currently, only rectangular content masks are supported, but we give the mask its own type
/// to leave room to support more complex shapes in the future.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct ContentMask<P: Clone + Debug + Default + PartialEq> {
    /// The bounds
    pub bounds: Bounds<P>,
}

impl ContentMask<Pixels> {
    /// Scale the content mask's pixel units by the given scaling factor.
    pub fn scale(&self, factor: f32) -> ContentMask<ScaledPixels> {
        ContentMask { bounds: self.bounds.scale(factor) }
    }

    /// Intersect the content mask with the given content mask.
    pub fn intersect(&self, other: &Self) -> Self {
        let bounds = self.bounds.intersect(&other.bounds);
        ContentMask { bounds }
    }
}

impl ContentMask<Pixels> {
    /// No clip at all: what a view's own drawing starts with (what holds
    /// it clips it when the window is put together).
    pub(crate) fn everything() -> Self {
        ContentMask { bounds: Bounds { origin: point(px(-8_000_000.), px(-8_000_000.)), size: size(px(16_000_000.), px(16_000_000.)) } }
    }
}

/// A glyph found in the atlas this pass: where its picture sits by its
/// origin, and its tile.
type FoundGlyph = Option<(Bounds<DevicePixels>, AtlasTile)>;

/// What a window keeps to paint with, on its looper's thread.
pub(crate) struct Gfx {
    pub(crate) scale_factor: f32,
    pub(crate) rem_size: Pixels,
    pub(crate) text_rendering: TextRendering,
    pub(crate) text_system: Arc<WindowTextSystem>,
    pub(crate) sprite_atlas: Arc<dyn PlatformAtlas>,
    pub(crate) svg_renderer: SvgRenderer,
    /// Glyphs looked up since the scale or text rendering last changed
    /// (font, glyph, size, subpixel place).
    pub(crate) glyph_memo: FxHashMap<(u32, u32, u32, u8, u8), FoundGlyph>,
    pub(crate) outlines: crate::outline::Outlines,
    pub(crate) text_style: TextStyle,
    /// Something of the kit's that drawing fills in while it's there
    /// (what a layout review is told of each thing drawn).
    pub(crate) ext: Option<Box<dyn std::any::Any>>,
}

/// What a view left for others as it drew, kept with its display list.
#[derive(Default)]
pub(crate) struct Notes {
    /// What was painted waits for something (a picture still loading):
    /// it's drawn again when that comes.
    pub(crate) unfinished: bool,
    /// The drawing looked at what can be seen of it: it's drawn again
    /// when that changes.
    pub(crate) by_visible: bool,
    /// What could be seen of it then.
    pub(crate) visible: Bounds<Pixels>,
    pub(crate) agent: Vec<crate::agent::Node>,
    pub(crate) cursors: Vec<(Bounds<Pixels>, crate::CursorStyle)>,
    /// Texts that don't show in full (cut short to fit, or cut by what
    /// they're in): each whole, and where what shows of it is; or
    /// (true) that are cut only if what holds the drawing cuts them at a
    /// side, where they are.
    pub(crate) cuts: Vec<(Bounds<Pixels>, SharedString, bool)>,
    /// It moves with time: drawn again within this many seconds
    /// (`Painter::paint_again_within`; taken by the window's clock as
    /// the drawing is kept).
    pub(crate) again: Option<f64>,
}

impl Notes {
    pub(crate) fn is_empty(&self) -> bool {
        !self.unfinished && !self.by_visible && self.agent.is_empty() && self.cursors.is_empty() && self.cuts.is_empty()
    }
}

/// An element's own state, of the type asked for (made if it isn't
/// there, or is another's).
pub(crate) fn node_state<T: Default + 'static>(slot: &mut Option<Box<dyn std::any::Any>>) -> &mut T {
    if !slot.as_ref().is_some_and(|s| s.is::<T>()) {
        *slot = Some(Box::new(T::default()));
    }
    slot.as_mut().and_then(|s| s.downcast_mut::<T>()).expect("made above")
}

/// What a view draws with: its drawing is recorded and kept until the
/// view changes.
pub struct Painter<'a> {
    pub(crate) gfx: &'a mut Gfx,
    pub(crate) scene: &'a mut Scene,
    pub(crate) notes: &'a mut Notes,
    content_mask_stack: Vec<ContentMask<Pixels>>,
    text_style_stack: Vec<TextStyleRefinement>,
    element_opacity: f32,
    /// What can be seen of the view being painted (in the window).
    visible: Bounds<Pixels>,
    /// What the element being painted keeps (`node_state`).
    state: &'a mut Option<Box<dyn std::any::Any>>,
}

impl<'a> Painter<'a> {
    pub(crate) fn new(
        gfx: &'a mut Gfx,
        scene: &'a mut Scene,
        notes: &'a mut Notes,
        state: &'a mut Option<Box<dyn std::any::Any>>,
        visible: Bounds<Pixels>,
        opacity: f32,
        text_style: Vec<TextStyleRefinement>,
    ) -> Self {
        Painter { gfx, scene, notes, content_mask_stack: Vec::new(), text_style_stack: text_style, element_opacity: opacity, visible, state }
    }

    /// What the element being painted keeps from one drawing to the
    /// next and between its handlers (`WindowCx::node_state`): what a
    /// control is in the middle of (a scroll bar's held arrow, a
    /// slider's drag). Made the first time it's asked for; one type an
    /// element.
    pub fn node_state<T: Default + 'static>(&mut self) -> &mut T {
        node_state(self.state)
    }

    /// Device pixels to a point on the window's display.
    pub fn scale_factor(&self) -> f32 {
        self.gfx.scale_factor
    }

    /// The size of a rem, in points.
    pub fn rem_size(&self) -> Pixels {
        self.gfx.rem_size
    }

    /// How text is drawn for the display the window is on.
    pub fn text_rendering(&self) -> TextRendering {
        self.gfx.text_rendering
    }

    /// The text system, for shaping and measuring.
    pub fn text_system(&self) -> &Arc<WindowTextSystem> {
        &self.gfx.text_system
    }

    /// The text style in force: the window's, as refined by what holds
    /// what's being painted.
    pub fn text_style(&self) -> TextStyle {
        let mut style = self.gfx.text_style.clone();
        for refinement in &self.text_style_stack {
            style.refine(refinement);
        }
        style
    }

    /// The line height of the text style in force.
    pub fn line_height(&self) -> Pixels {
        self.text_style().line_height_in_pixels(self.rem_size())
    }

    /// Paints with `style` refining the text style.
    pub fn with_text_style<R>(&mut self, style: Option<TextStyleRefinement>, f: impl FnOnce(&mut Self) -> R) -> R {
        if let Some(style) = style {
            self.text_style_stack.push(style);
            let result = f(self);
            self.text_style_stack.pop();
            result
        } else {
            f(self)
        }
    }

    /// The clip in force: what `with_content_mask` set, or none.
    pub fn content_mask(&self) -> ContentMask<Pixels> {
        self.content_mask_stack.last().cloned().unwrap_or_else(ContentMask::everything)
    }

    /// Paints clipped to `mask` (and to any clip already in force).
    pub fn with_content_mask<R>(&mut self, mask: Option<ContentMask<Pixels>>, f: impl FnOnce(&mut Self) -> R) -> R {
        if let Some(mask) = mask {
            let mask = mask.intersect(&self.content_mask());
            self.content_mask_stack.push(mask);
            let result = f(self);
            self.content_mask_stack.pop();
            result
        } else {
            f(self)
        }
    }

    /// What can be seen of the view being painted, in the window's
    /// coordinates: for a view that draws only what shows (a long text's
    /// lines in view). Asking makes the view drawn again whenever that
    /// changes, so ask only to draw less.
    pub fn visible_bounds(&mut self) -> Bounds<Pixels> {
        self.notes.by_visible = true;
        self.visible.intersect(&self.content_mask().bounds)
    }

    /// The opacity in force (what holds the view may be faded).
    pub fn element_opacity(&self) -> f32 {
        self.element_opacity
    }

    /// Paints faded to `opacity` (times any already in force).
    pub fn with_element_opacity<R>(&mut self, opacity: Option<f32>, f: impl FnOnce(&mut Self) -> R) -> R {
        let Some(opacity) = opacity else { return f(self) };
        let previous = self.element_opacity;
        self.element_opacity = previous * opacity;
        let result = f(self);
        self.element_opacity = previous;
        result
    }

    /// Something was left out of the drawing, wholly outside the clip.
    pub(crate) fn note_culled(&mut self) {
        self.scene.culled += 1;
    }

    /// Says what's being drawn is, for agents (a part of a drawing that
    /// stands for something: a key of a drawn keypad, a row of a drawn
    /// list). Kept with the drawing.
    pub fn agent(&mut self, mut node: crate::agent::Node) {
        // (Its own clip, if the drawing has one: what the window clips
        // of it is worked out when it's asked for, from where things
        // are then.)
        if node.clip.is_none() {
            node.clip = self.content_mask_stack.last().map(|mask| mask.bounds);
        }
        self.notes.agent.push(node);
    }

    /// `full` was drawn cut short in `bounds`: its whole is what agents
    /// read, and what a tip shows when the pointer rests on it.
    pub fn note_cut(&mut self, full: SharedString, bounds: Bounds<Pixels>) {
        let shown = bounds.intersect(&self.content_mask().bounds);
        if shown.size.width > px(0.0) && shown.size.height > px(0.0) && !full.trim().is_empty() {
            self.notes.cuts.push((shown, full, false));
        }
    }

    /// `full` was drawn whole in `bounds`: if what holds the drawing
    /// cuts it at a side (now, or as things are laid out later), its
    /// whole is what a tip shows and agents read. (A row scrolled half
    /// out of view, top or bottom, isn't cut.)
    pub fn note_if_cut(&mut self, full: SharedString, bounds: Bounds<Pixels>) {
        if full.trim().is_empty() {
            return;
        }
        // Cut by the drawing's own clip: so whatever holds it.
        let mask = self.content_mask().bounds;
        if bounds.left() < mask.left() - px(1.0) || bounds.right() > mask.right() + px(1.0) {
            if bounds.top() < mask.bottom() && bounds.bottom() > mask.top() {
                self.note_cut(full, bounds);
            }
            return;
        }
        self.notes.cuts.push((bounds, full, true));
    }

    /// What clips what's being painted now: what can be seen of the
    /// view, within any clip of its own. (Unlike
    /// [`Self::visible_bounds`], asking doesn't make the view drawn again
    /// when it changes: for notes on a drawing, not for what to draw.)
    pub fn clip(&self) -> Bounds<Pixels> {
        self.visible.intersect(&self.content_mask().bounds)
    }

    /// What the kit put with the window for drawings to fill in
    /// (`WindowCx::set_paint_ext`), if it's there and of this type.
    pub fn ext<T: 'static>(&mut self) -> Option<&mut T> {
        self.gfx.ext.as_mut()?.downcast_mut()
    }

    /// Whether anything is there to fill in ([`Self::ext`]).
    pub fn has_ext(&self) -> bool {
        self.gfx.ext.is_some()
    }

    /// Paints with `f`, none of whose text is noted as cut short: text
    /// that's a picture, not something to read whole in a tip (a screen
    /// saver's words running off the screen's edge).
    pub fn without_cut_notes<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let before = self.notes.cuts.len();
        let result = f(self);
        self.notes.cuts.truncate(before);
        result
    }

    /// What was painted waits for something still coming (a picture
    /// being decoded): the view is drawn again when it has.
    pub fn paint_unfinished(&mut self) {
        self.notes.unfinished = true;
    }

    /// What's being painted moves with time (a graph scrolling, a
    /// clock's hand): it's drawn again, alone, within `seconds` (how
    /// long until it would look different: a graph's samples a device
    /// pixel along). The window has one clock for all that ask: it
    /// ticks at the finest step asked, and each tick draws them all, so
    /// they move together in one frame. Each drawing asks for the next;
    /// what stops asking stops, and nothing runs while the window isn't
    /// shown (the clock waits for the frame its drawings went in).
    pub fn paint_again_within(&mut self, seconds: f64) {
        if seconds.is_finite() {
            self.notes.again = Some(self.notes.again.map_or(seconds, |s| s.min(seconds)).max(0.0));
        }
    }

    /// The pointer's shape while it's over `bounds`.
    pub fn set_cursor_style(&mut self, style: crate::CursorStyle, bounds: Bounds<Pixels>) {
        self.notes.cursors.push((bounds, style));
    }

    /// Creates a new painting layer for the specified bounds. A "layer" is a batch
    /// of geometry that are non-overlapping and have the same draw order. This is typically used
    /// for performance reasons.
    pub fn paint_layer<R>(&mut self, bounds: Bounds<Pixels>, f: impl FnOnce(&mut Self) -> R) -> R {
        let scale_factor = self.scale_factor();
        let content_mask = self.content_mask();
        let clipped_bounds = bounds.intersect(&content_mask.bounds);
        if !clipped_bounds.is_empty() {
            self.scene.push_layer(clipped_bounds.scale(scale_factor));
        }

        let result = f(self);

        if !clipped_bounds.is_empty() {
            self.scene.pop_layer();
        }

        result
    }

    /// Paint one or more drop shadows .
    pub fn paint_shadows(&mut self, bounds: Bounds<Pixels>, corner_radii: Corners<Pixels>, shadows: &[BoxShadow]) {
        let scale_factor = self.scale_factor();
        let content_mask = self.content_mask();
        let opacity = self.element_opacity();
        for shadow in shadows {
            let shadow_bounds = (bounds + shadow.offset).dilate(shadow.spread_radius);
            self.scene.insert_primitive(Shadow {
                order: 0,
                blur_radius: shadow.blur_radius.scale(scale_factor),
                bounds: shadow_bounds.scale(scale_factor),
                content_mask: content_mask.scale(scale_factor),
                corner_radii: corner_radii.scale(scale_factor),
                color: shadow.color.opacity(opacity),
            });
        }
    }

    /// Paint one or more quads .
    /// Quads are colored rectangular regions with an optional background, border, and corner radius.
    /// see [`fill`], [`outline`], and [`quad`] to construct this type.
    ///
    /// Note that the `quad.corner_radii` are allowed to exceed the bounds, creating sharp corners
    /// where the circular arcs meet. This will not display well when combined with dashed borders.
    /// Use `Corners::clamp_radii_for_quad_size` if the radii should fit within the bounds.
    pub fn paint_quad(&mut self, quad: PaintQuad) {
        let scale_factor = self.scale_factor();
        let content_mask = self.content_mask();
        let opacity = self.element_opacity();
        self.scene.insert_primitive(Quad {
            order: 0,
            bounds: quad.bounds.scale(scale_factor),
            content_mask: content_mask.scale(scale_factor),
            background: quad.background.opacity(opacity),
            border_color: quad.border_color.opacity(opacity),
            corner_radii: quad.corner_radii.scale(scale_factor),
            border_widths: quad.border_widths.scale(scale_factor),
            border_style: quad.border_style,
        });
    }

    /// Paint the given `Path` .
    pub fn paint_path(&mut self, mut path: Path<Pixels>, color: impl Into<Background>) {
        let scale_factor = self.scale_factor();
        let content_mask = self.content_mask();
        let opacity = self.element_opacity();
        let color: Background = color.into();
        // Olo: a small one in a plain colour, with exact coverage.
        if let Some(outline) = path.outline.take()
            && color.tag == crate::BackgroundTag::Solid
            && let Some(placed) = crate::outline::place(&outline, path.bounds, scale_factor)
            && let Ok(tile) = self.gfx.outlines.tile(self.gfx.sprite_atlas.as_ref(), &placed.key)
        {
            if let Some(tile) = tile {
                self.scene.insert_primitive(MonochromeSprite {
                    order: 0,
                    coverage: 1,
                    bounds: placed.bounds,
                    content_mask: content_mask.scale(scale_factor),
                    color: color.solid.opacity(opacity),
                    tile,
                    transformation: TransformationMatrix::unit(),
                });
            }
            return;
        }
        path.content_mask = content_mask;
        path.color = color.opacity(opacity);
        self.scene.insert_primitive(path.scale(scale_factor));
    }

    /// paints a straight line from `from` to `to`, `width` wide, its
    /// ends square (`round`: rounded) and its edges antialiased by each
    /// pixel's distance to it, drawn on the GPU (graphs, screen savers,
    /// strokes): where a path would be tessellated, one instance.
    pub fn paint_segment(&mut self, from: Point<Pixels>, to: Point<Pixels>, width: Pixels, round: bool, color: Hsla) {
        let scale = self.scale_factor();
        let (a, b) = (from.scale(scale), to.scale(scale));
        let half = width.scale(scale) / 2.0;
        let reach = half + ScaledPixels(1.0) + if round { half } else { ScaledPixels(0.0) };
        let min = point(a.x.min(b.x) - reach, a.y.min(b.y) - reach);
        let max = point(a.x.max(b.x) + reach, a.y.max(b.y) + reach);
        let opacity = self.element_opacity();
        let content_mask = self.content_mask().scale(scale);
        self.scene.insert_primitive(Segment {
            order: 0,
            round: round as u32,
            bounds: Bounds::from_corners(min, max),
            content_mask,
            color: color.opacity(opacity),
            from: a,
            to: b,
            half_width: half,
            bottom: ScaledPixels(0.0),
        });
    }

    /// paints straight lines, each piece `width` wide, their ends
    /// square (`round`: rounded, so the pieces of a polyline join), in
    /// one colour: [`Window::paint_segment`] for many, the clip, opacity
    /// and scale looked up once (a curve of a thousand pieces).
    pub fn paint_segments(&mut self, pieces: &[(Point<Pixels>, Point<Pixels>)], width: Pixels, round: bool, color: Hsla) {
        let scale = self.scale_factor();
        let half = width.scale(scale) / 2.0;
        let reach = half + ScaledPixels(1.0) + if round { half } else { ScaledPixels(0.0) };
        let color = color.opacity(self.element_opacity());
        let content_mask = self.content_mask().scale(scale);
        let segments = pieces
            .iter()
            .map(|(from, to)| {
                let (a, b) = (from.scale(scale), to.scale(scale));
                Segment {
                    order: 0,
                    round: round as u32,
                    bounds: Bounds::from_corners(point(a.x.min(b.x) - reach, a.y.min(b.y) - reach), point(a.x.max(b.x) + reach, a.y.max(b.y) + reach)),
                    content_mask: content_mask.clone(),
                    color,
                    from: a,
                    to: b,
                    half_width: half,
                    bottom: ScaledPixels(0.0),
                }
            })
            .collect();
        self.scene.insert_segments(segments);
    }

    /// paints what's under the line from `from` to `to` (left to
    /// right), down to `bottom`: a graph's fill, a column of it a
    /// segment. Drawn on the GPU in one instance, its slanted top
    /// antialiased by each pixel's distance to the line, and its sides
    /// taking the pixels whose middles they hold, so areas side by side
    /// share each pixel once (no seam, nothing drawn twice) — where a path
    /// would be tessellated, or filled multisampled through textures as
    /// large as the window.
    pub fn paint_area(&mut self, from: Point<Pixels>, to: Point<Pixels>, bottom: Pixels, color: Hsla) {
        let scale = self.scale_factor();
        let (a, b, bottom) = (from.scale(scale), to.scale(scale), bottom.scale(scale));
        if b.x <= a.x {
            return;
        }
        let top = a.y.min(b.y) - ScaledPixels(1.0);
        let opacity = self.element_opacity();
        let content_mask = self.content_mask().scale(scale);
        self.scene.insert_primitive(Segment {
            order: 0,
            round: 2,
            bounds: Bounds::from_corners(point(a.x, top), point(b.x, bottom.max(top))),
            content_mask,
            color: color.opacity(opacity),
            from: a,
            to: b,
            half_width: ScaledPixels(0.0),
            bottom,
        });
    }

    /// paints a slice of a ring around `center`, from radius `inner`
    /// to `outer` and from `start` radians clockwise from 12 o'clock
    /// `sweep` radians round (a whole turn or more: the whole ring; inner
    /// 0: a pie's slice, or a disc), its edges antialiased by each pixel's
    /// distance to them, drawn on the GPU: one instance, where a path
    /// would be tessellated or rasterised (ring charts, gauges, dials,
    /// round marks, a circle's outline as a ring a line wide).
    pub fn paint_sector(&mut self, center: Point<Pixels>, inner: Pixels, outer: Pixels, start: f32, sweep: f32, color: Hsla) {
        if !(sweep > 0.0) || !(outer > inner) {
            return;
        }
        let scale = self.scale_factor();
        let c = center.scale(scale);
        let (r0, r1) = (inner.scale(scale).max(ScaledPixels(0.0)), outer.scale(scale));
        // What it covers: its corners (the middle, with no hole), and the
        // ring's extremes at the quarter turns it passes, a pixel more for
        // the edges.
        let tau = std::f32::consts::TAU;
        let at = |r: ScaledPixels, a: f32| point(c.x + r * a.sin(), c.y - r * a.cos());
        let (mut min, mut max) = (point(ScaledPixels(f32::MAX), ScaledPixels(f32::MAX)), point(ScaledPixels(f32::MIN), ScaledPixels(f32::MIN)));
        let mut take = |p: Point<ScaledPixels>| {
            min = point(min.x.min(p.x), min.y.min(p.y));
            max = point(max.x.max(p.x), max.y.max(p.y));
        };
        if sweep >= tau {
            take(point(c.x - r1, c.y - r1));
            take(point(c.x + r1, c.y + r1));
        } else {
            let end = start + sweep;
            for a in [start, end] {
                take(at(r0, a));
                take(at(r1, a));
            }
            let quarter = tau / 4.0;
            let mut k = (start / quarter).ceil();
            while k * quarter <= end {
                take(at(r1, k * quarter));
                k += 1.0;
            }
        }
        let pad = ScaledPixels(1.0);
        let bounds = Bounds::from_corners(point(min.x - pad, min.y - pad), point(max.x + pad, max.y + pad));
        let opacity = self.element_opacity();
        let content_mask = self.content_mask().scale(scale);
        self.scene.insert_primitive(Sector { order: 0, pad: 0, bounds, content_mask, color: color.opacity(opacity), center: c, inner: r0, outer: r1, start, sweep: sweep.min(tau) });
    }

    /// Paint an underline .
    pub fn paint_underline(&mut self, origin: Point<Pixels>, width: Pixels, style: &UnderlineStyle) {
        let scale_factor = self.scale_factor();
        let height = if style.wavy { style.thickness * 3. } else { style.thickness };
        let bounds = Bounds { origin, size: size(width, height) };
        let content_mask = self.content_mask();
        let element_opacity = self.element_opacity();

        self.scene.insert_primitive(Underline {
            order: 0,
            pad: 0,
            bounds: bounds.scale(scale_factor),
            content_mask: content_mask.scale(scale_factor),
            color: style.color.unwrap_or_default().opacity(element_opacity),
            thickness: style.thickness.scale(scale_factor),
            wavy: if style.wavy { 1 } else { 0 },
        });
    }

    /// Paint a strikethrough .
    pub fn paint_strikethrough(&mut self, origin: Point<Pixels>, width: Pixels, style: &StrikethroughStyle) {
        let scale_factor = self.scale_factor();
        let height = style.thickness;
        let bounds = Bounds { origin, size: size(width, height) };
        let content_mask = self.content_mask();
        let opacity = self.element_opacity();

        self.scene.insert_primitive(Underline {
            order: 0,
            pad: 0,
            bounds: bounds.scale(scale_factor),
            content_mask: content_mask.scale(scale_factor),
            thickness: style.thickness.scale(scale_factor),
            color: style.color.unwrap_or_default().opacity(opacity),
            wavy: 0,
        });
    }

    /// Paints a monochrome (non-emoji) glyph .
    ///
    /// The y component of the origin is the baseline of the glyph.
    /// You should generally prefer to use the [`ShapedLine::paint`](crate::ShapedLine::paint) or
    /// [`WrappedLine::paint`](crate::WrappedLine::paint) methods in the [`TextSystem`](crate::TextSystem).
    /// This method is only useful if you need to paint a single glyph that has already been shaped.
    pub fn paint_glyph(&mut self, origin: Point<Pixels>, font_id: FontId, glyph_id: GlyphId, font_size: Pixels, color: Hsla) -> Result<()> {
        let element_opacity = self.element_opacity();
        let scale_factor = self.scale_factor();
        let glyph_origin = origin.scale(scale_factor);

        let subpixel_variant = Point { x: (glyph_origin.x.0.fract() * SUBPIXEL_VARIANTS_X as f32).floor() as u8, y: (glyph_origin.y.0.fract() * SUBPIXEL_VARIANTS_Y as f32).floor() as u8 };
        let key = (font_id.0 as u32, glyph_id.0, font_size.0.to_bits(), subpixel_variant.x, subpixel_variant.y);
        let found = match self.gfx.glyph_memo.get(&key) {
            Some(found) => found.clone(),
            None => {
                let params = RenderGlyphParams { font_id, glyph_id, font_size, subpixel_variant, scale_factor, is_emoji: false, rendering: self.gfx.text_rendering };
                let raster_bounds = self.gfx.text_system.raster_bounds(&params)?;
                let found = if raster_bounds.is_zero() {
                    None
                } else {
                    let tile = self
                        .gfx
                        .sprite_atlas
                        .get_or_insert_with(&params.clone().into(), &mut || {
                            // Olo: counted, so a face drawn as running text has
                            // the rest of its glyphs drawn ahead, off this thread.
                            self.gfx.text_system.drawn_first(params.font_id, params.font_size, params.scale_factor, params.rendering);
                            let (size, bytes) = self.gfx.text_system.rasterize_glyph(&params)?;
                            Ok(Some((size, Cow::Owned(bytes))))
                        })?
                        .expect("Callback above only errors or returns Some");
                    Some((raster_bounds, tile))
                };
                self.gfx.glyph_memo.insert(key, found.clone());
                found
            }
        };
        if let Some((raster_bounds, tile)) = found {
            let bounds = Bounds { origin: glyph_origin.map(|px| px.floor()) + raster_bounds.origin.map(Into::into), size: tile.bounds.size.map(Into::into) };
            let content_mask = self.content_mask().scale(scale_factor);
            self.scene.insert_primitive(MonochromeSprite { order: 0, coverage: 0, bounds, content_mask, color: color.opacity(element_opacity), tile, transformation: TransformationMatrix::unit() });
        }
        Ok(())
    }

    /// paints many glyphs at once, each at its baseline origin in
    /// its face, size and colour: as [`Window::paint_glyph`] each, but
    /// one paint operation for them all (a page of text).
    pub fn paint_glyphs(&mut self, glyphs: impl IntoIterator<Item = (Point<Pixels>, FontId, GlyphId, Pixels, Hsla)>) {
        let element_opacity = self.element_opacity();
        let content_mask = self.content_mask().scale(self.scale_factor());
        let mut sprites = Vec::new();
        for (origin, font_id, glyph_id, font_size, color) in glyphs {
            if let Some((bounds, tile)) = self.glyph_sprite(origin, font_id, glyph_id, font_size) {
                sprites.push(MonochromeSprite {
                    order: 0,
                    coverage: 0,
                    bounds,
                    content_mask: content_mask.clone(),
                    color: color.opacity(element_opacity),
                    tile,
                    transformation: TransformationMatrix::unit(),
                });
            }
        }
        self.scene.insert_sprites(sprites);
    }

    /// Paints `run`'s glyphs (as [`Painter::paint_glyphs`]), each at its
    /// offset from `origin`. The first time (and when the window's scale
    /// or text rendering, the caller's `stamp` for what `glyphs` gives,
    /// or where `origin` falls within a device pixel changes) they are
    /// looked up in the atlas; after that the run is painted from what
    /// was found, moved, without a lookup a glyph.
    pub fn paint_glyph_run(&mut self, run: &mut GlyphRun, origin: Point<Pixels>, stamp: u64, glyphs: impl FnOnce(&mut dyn FnMut(Point<Pixels>, FontId, GlyphId, Pixels, Hsla))) {
        let scale_factor = self.scale_factor();
        let device = origin.scale(scale_factor);
        // Its whole device pixel and where within it (in 64ths).
        let mut base = Point { x: device.x.0.floor(), y: device.y.0.floor() };
        let mut phase = (((device.x.0 - base.x) * 64.0).round() as i32, ((device.y.0 - base.y) * 64.0).round() as i32);
        if phase.0 == 64 {
            base.x += 1.0;
            phase.0 = 0;
        }
        if phase.1 == 64 {
            base.y += 1.0;
            phase.1 = 0;
        }
        let key = GlyphRunKey { scale: scale_factor.to_bits(), rendering: self.gfx.text_rendering, stamp, phase };
        if run.key != Some(key) {
            let cold = crate::stats::frame_phases().then(std::time::Instant::now);
            run.key = Some(key);
            run.sprites.clear();
            let mut found = Vec::new();
            glyphs(&mut |at, font_id, glyph_id, font_size, color| found.push((origin + at, font_id, glyph_id, font_size, color)));
            for (at, font_id, glyph_id, font_size, color) in found {
                if let Some((bounds, tile)) = self.glyph_sprite(at, font_id, glyph_id, font_size) {
                    let rel = Bounds { origin: Point { x: ScaledPixels(bounds.origin.x.0 - base.x), y: ScaledPixels(bounds.origin.y.0 - base.y) }, size: bounds.size };
                    run.sprites.push((rel, tile, color));
                }
            }
            if let Some(t) = cold {
                crate::stats::FRAME_WORK.note(&crate::stats::FRAME_WORK.runs, t);
            }
        }
        let element_opacity = self.element_opacity();
        let content_mask = self.content_mask().scale(scale_factor);
        let sprites = run.sprites.iter().map(|(rel, tile, color)| MonochromeSprite {
            order: 0,
            coverage: 0,
            bounds: Bounds { origin: Point { x: ScaledPixels(rel.origin.x.0 + base.x), y: ScaledPixels(rel.origin.y.0 + base.y) }, size: rel.size },
            content_mask: content_mask.clone(),
            color: color.opacity(element_opacity),
            tile: tile.clone(),
            transformation: TransformationMatrix::unit(),
        });
        self.scene.insert_sprites(sprites);
    }

    /// A glyph at `origin` (its baseline): where its sprite goes, in
    /// device pixels, and its tile (looked up once a frame a glyph).
    fn glyph_sprite(&mut self, origin: Point<Pixels>, font_id: FontId, glyph_id: GlyphId, font_size: Pixels) -> Option<(Bounds<ScaledPixels>, AtlasTile)> {
        let scale_factor = self.scale_factor();
        let glyph_origin = origin.scale(scale_factor);
        let subpixel_variant = Point { x: (glyph_origin.x.0.fract() * SUBPIXEL_VARIANTS_X as f32).floor() as u8, y: (glyph_origin.y.0.fract() * SUBPIXEL_VARIANTS_Y as f32).floor() as u8 };
        let key = (font_id.0 as u32, glyph_id.0, font_size.0.to_bits(), subpixel_variant.x, subpixel_variant.y);
        let found = match self.gfx.glyph_memo.get(&key) {
            Some(found) => found.clone(),
            None => {
                let params = RenderGlyphParams { font_id, glyph_id, font_size, subpixel_variant, scale_factor, is_emoji: false, rendering: self.gfx.text_rendering };
                let found = match self.gfx.text_system.raster_bounds(&params) {
                    Ok(b) if !b.is_zero() => self
                        .gfx
                        .sprite_atlas
                        .get_or_insert_with(&params.clone().into(), &mut || {
                            self.gfx.text_system.drawn_first(params.font_id, params.font_size, params.scale_factor, params.rendering);
                            let t = crate::stats::frame_phases().then(std::time::Instant::now);
                            let (size, bytes) = self.gfx.text_system.rasterize_glyph(&params)?;
                            if let Some(t) = t {
                                crate::stats::FRAME_WORK.note(&crate::stats::FRAME_WORK.rasterized, t);
                            }
                            Ok(Some((size, Cow::Owned(bytes))))
                        })
                        .ok()
                        .flatten()
                        .map(|tile| (b, tile)),
                    _ => None,
                };
                self.gfx.glyph_memo.insert(key, found.clone());
                found
            }
        };
        let (raster_bounds, tile) = found?;
        Some((Bounds { origin: glyph_origin.map(|px| px.floor()) + raster_bounds.origin.map(Into::into), size: tile.bounds.size.map(Into::into) }, tile))
    }

    /// Paints an emoji glyph .
    ///
    /// The y component of the origin is the baseline of the glyph.
    /// You should generally prefer to use the [`ShapedLine::paint`](crate::ShapedLine::paint) or
    /// [`WrappedLine::paint`](crate::WrappedLine::paint) methods in the [`TextSystem`](crate::TextSystem).
    /// This method is only useful if you need to paint a single emoji that has already been shaped.
    pub fn paint_emoji(&mut self, origin: Point<Pixels>, font_id: FontId, glyph_id: GlyphId, font_size: Pixels) -> Result<()> {
        let scale_factor = self.scale_factor();
        let glyph_origin = origin.scale(scale_factor);
        let params = RenderGlyphParams {
            font_id,
            glyph_id,
            font_size,
            // We don't render emojis with subpixel variants.
            subpixel_variant: Default::default(),
            scale_factor,
            is_emoji: true,
            rendering: TextRendering::default(),
        };

        let raster_bounds = self.gfx.text_system.raster_bounds(&params)?;
        if !raster_bounds.is_zero() {
            let tile = self
                .gfx
                .sprite_atlas
                .get_or_insert_with(&params.clone().into(), &mut || {
                    let (size, bytes) = self.gfx.text_system.rasterize_glyph(&params)?;
                    Ok(Some((size, Cow::Owned(bytes))))
                })?
                .expect("Callback above only errors or returns Some");

            let bounds = Bounds { origin: glyph_origin.map(|px| px.floor()) + raster_bounds.origin.map(Into::into), size: tile.bounds.size.map(Into::into) };
            let content_mask = self.content_mask().scale(scale_factor);
            let opacity = self.element_opacity();

            self.scene.insert_primitive(PolychromeSprite { order: 0, nearest: 0, grayscale: false, bounds, corner_radii: Default::default(), content_mask, tile, opacity });
        }
        Ok(())
    }

    /// Paint a monochrome SVG .
    pub fn paint_svg(&mut self, bounds: Bounds<Pixels>, path: SharedString, transformation: TransformationMatrix, color: Hsla) -> Result<()> {
        let element_opacity = self.element_opacity();
        let scale_factor = self.scale_factor();

        let bounds = bounds.scale(scale_factor);
        let params = RenderSvgParams { path, size: bounds.size.map(|pixels| DevicePixels::from((pixels.0 * SMOOTH_SVG_SCALE_FACTOR).ceil() as i32)) };

        let Some(tile) = self.gfx.sprite_atlas.get_or_insert_with(&params.clone().into(), &mut || {
            let Some((size, bytes)) = self.gfx.svg_renderer.render(&params)? else {
                return Ok(None);
            };
            Ok(Some((size, Cow::Owned(bytes))))
        })?
        else {
            return Ok(());
        };
        let content_mask = self.content_mask().scale(scale_factor);
        let svg_bounds = Bounds {
            origin: bounds.center()
                - Point::new(ScaledPixels(tile.bounds.size.width.0 as f32 / SMOOTH_SVG_SCALE_FACTOR / 2.), ScaledPixels(tile.bounds.size.height.0 as f32 / SMOOTH_SVG_SCALE_FACTOR / 2.)),
            size: tile.bounds.size.map(|value| ScaledPixels(value.0 as f32 / SMOOTH_SVG_SCALE_FACTOR)),
        };

        self.scene.insert_primitive(MonochromeSprite {
            order: 0,
            coverage: 0,
            bounds: svg_bounds.map_origin(|origin| origin.round()).map_size(|size| size.ceil()),
            content_mask,
            color: color.opacity(element_opacity),
            tile,
            transformation,
        });

        Ok(())
    }

    /// Paint an image .
    /// This method will panic if the frame_index is not valid
    pub fn paint_image(&mut self, bounds: Bounds<Pixels>, corner_radii: Corners<Pixels>, data: Arc<RenderImage>, frame_index: usize, grayscale: bool) -> Result<()> {
        self.paint_image_sampled(bounds, corner_radii, data, frame_index, grayscale, false)
    }

    /// [`Window::paint_image`], its texels shown as they are when
    /// magnified (nearest), not blended with their neighbours: a small
    /// picture (an icon) looked at closely keeps its pixels.
    pub fn paint_image_nearest(&mut self, bounds: Bounds<Pixels>, data: Arc<RenderImage>) -> Result<()> {
        self.paint_image_sampled(bounds, Corners::default(), data, 0, false, true)
    }

    /// paints triangles of an image, each corner a point in the
    /// window and where in the image (0–1 across it) it shows, the image
    /// tinted (white leaves it as it is): a picture bent in perspective
    /// (the Flip screen saver), a lens's magnified disc. On the GPU, one
    /// instance a triangle, sampled from the image's atlas tile.
    pub fn paint_image_triangles(&mut self, data: Arc<RenderImage>, triangles: &[[(Point<Pixels>, Point<f32>); 3]], tint: Hsla) -> Result<()> {
        let scale_factor = self.scale_factor();
        let params = RenderImageParams { image_id: data.id, frame_index: 0, deep: data.is_deep() };
        let tile = self
            .gfx
            .sprite_atlas
            .get_or_insert_with(&params.into(), &mut || Ok(Some((data.size(0), Cow::Borrowed(data.as_bytes(0).expect("an image has a first frame"))))))?
            .expect("Callback above only returns Some");
        let content_mask = self.content_mask().scale(scale_factor);
        let opacity = self.element_opacity();
        for t in triangles {
            let corners = [t[0].0.scale(scale_factor), t[1].0.scale(scale_factor), t[2].0.scale(scale_factor)];
            let min = point(corners[0].x.min(corners[1].x).min(corners[2].x), corners[0].y.min(corners[1].y).min(corners[2].y));
            let max = point(corners[0].x.max(corners[1].x).max(corners[2].x), corners[0].y.max(corners[1].y).max(corners[2].y));
            self.scene.insert_primitive(crate::ImageTriangle {
                order: 0,
                opacity,
                bounds: Bounds::from_corners(min, max),
                content_mask: content_mask.clone(),
                tile: tile.clone(),
                corners,
                uvs: [t[0].1, t[1].1, t[2].1],
                tint,
            });
        }
        Ok(())
    }

    fn paint_image_sampled(&mut self, bounds: Bounds<Pixels>, corner_radii: Corners<Pixels>, data: Arc<RenderImage>, frame_index: usize, grayscale: bool, nearest: bool) -> Result<()> {
        let scale_factor = self.scale_factor();
        let bounds = bounds.scale(scale_factor);
        let params = RenderImageParams { image_id: data.id, frame_index, deep: data.is_deep() };

        let tile = self
            .gfx
            .sprite_atlas
            .get_or_insert_with(&params.into(), &mut || Ok(Some((data.size(frame_index), Cow::Borrowed(data.as_bytes(frame_index).expect("It's the caller's job to pass a valid frame index"))))))?
            .expect("Callback above only returns Some");
        let content_mask = self.content_mask().scale(scale_factor);
        let corner_radii = corner_radii.scale(scale_factor);
        let opacity = self.element_opacity();

        self.scene.insert_primitive(PolychromeSprite {
            order: 0,
            // (Bit 1: the image is Display P3's numbers; bit 2, HDR10's.)
            nearest: u32::from(nearest)
                | match data.color_space {
                    crate::Gamut::DisplayP3 => 2,
                    crate::Gamut::Hdr10 => 4,
                    crate::Gamut::Srgb => 0,
                },
            grayscale,
            bounds: bounds.map_origin(|origin| origin.floor()).map_size(|size| size.ceil()),
            content_mask,
            corner_radii,
            tile,
            opacity,
        });
        Ok(())
    }

    /// paint a video's picture as decoded (Y'CbCr planes: `frame`)
    /// into `bounds`, turned into RGB by the GPU. A stream's frames keep
    /// its textures (`YuvFrame::stream`), filled again for each new one.
    pub fn paint_yuv(&mut self, bounds: Bounds<Pixels>, frame: std::sync::Arc<crate::YuvFrame>) {
        use crate::PaintSurface;

        let scale_factor = self.scale_factor();
        let bounds = bounds.scale(scale_factor);
        let content_mask = self.content_mask().scale(scale_factor);
        self.scene.insert_primitive(PaintSurface { order: 0, bounds, content_mask, frame });
    }
}

/// A rectangle to be rendered in the window at the given position and size.
/// Passed to [`Painter::paint_quad`].
#[derive(Clone)]
pub struct PaintQuad {
    /// The bounds of the quad within the window.
    pub bounds: Bounds<Pixels>,
    /// The radii of the quad's corners.
    pub corner_radii: Corners<Pixels>,
    /// The background color of the quad.
    pub background: Background,
    /// The widths of the quad's borders.
    pub border_widths: Edges<Pixels>,
    /// The color of the quad's borders.
    pub border_color: Hsla,
    /// The style of the quad's borders.
    pub border_style: BorderStyle,
}

impl PaintQuad {
    /// Sets the corner radii of the quad.
    pub fn corner_radii(self, corner_radii: impl Into<Corners<Pixels>>) -> Self {
        PaintQuad { corner_radii: corner_radii.into(), ..self }
    }

    /// Sets the border widths of the quad.
    pub fn border_widths(self, border_widths: impl Into<Edges<Pixels>>) -> Self {
        PaintQuad { border_widths: border_widths.into(), ..self }
    }

    /// Sets the border color of the quad.
    pub fn border_color(self, border_color: impl Into<Hsla>) -> Self {
        PaintQuad { border_color: border_color.into(), ..self }
    }

    /// Sets the background color of the quad.
    pub fn background(self, background: impl Into<Background>) -> Self {
        PaintQuad { background: background.into(), ..self }
    }
}

/// Creates a quad with the given parameters.
pub fn quad(
    bounds: Bounds<Pixels>,
    corner_radii: impl Into<Corners<Pixels>>,
    background: impl Into<Background>,
    border_widths: impl Into<Edges<Pixels>>,
    border_color: impl Into<Hsla>,
    border_style: BorderStyle,
) -> PaintQuad {
    PaintQuad { bounds, corner_radii: corner_radii.into(), background: background.into(), border_widths: border_widths.into(), border_color: border_color.into(), border_style }
}

/// Creates a filled quad with the given bounds and background color.
pub fn fill(bounds: impl Into<Bounds<Pixels>>, background: impl Into<Background>) -> PaintQuad {
    PaintQuad { bounds: bounds.into(), corner_radii: (0.).into(), background: background.into(), border_widths: (0.).into(), border_color: transparent_black(), border_style: BorderStyle::default() }
}

/// Creates a rectangle outline with the given bounds, border color, and a 1px border width
pub fn outline(bounds: impl Into<Bounds<Pixels>>, border_color: impl Into<Hsla>, border_style: BorderStyle) -> PaintQuad {
    PaintQuad { bounds: bounds.into(), corner_radii: (0.).into(), background: transparent_black().into(), border_widths: (1.).into(), border_color: border_color.into(), border_style }
}

/// A run of glyphs as [`Painter::paint_glyph_run`] last found them in
/// the atlas (keep one per run, from frame to frame).
#[derive(Clone, Default)]
pub struct GlyphRun {
    key: Option<GlyphRunKey>,
    /// Each glyph's sprite, from the run's whole device pixel, its tile
    /// and colour.
    sprites: Vec<(Bounds<ScaledPixels>, AtlasTile, Hsla)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct GlyphRunKey {
    scale: u32,
    rendering: TextRendering,
    stamp: u64,
    phase: (i32, i32),
}
