//! A picture to look at (the Viewer's, Quick Look's): fitted to the view
//! (never past a pixel a point, as a 72 dpi picture on a Mac), or zoomed
//! (⌘ and the wheel about the pointer, a double click between Actual Size
//! and fitting, [`ImageView::zoom`]) and panned (dragged, or scrolled)
//! when bigger than the view, on a light checkerboard so transparency
//! reads as such.
//!
//! Drawn exactly: shown smaller than it is, the picture is resampled on
//! a worker (Lanczos, its colours premultiplied) to the very device size
//! it's shown at and drawn a texture pixel to a device pixel, on whole
//! pixels; shown bigger, it's drawn from tiles of it made as they come
//! into view, a small picture's pixels kept as they are (nearest). While
//! a resampling is made (a zoom, a resize), the last one stands in.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
};

use olo_interface::{
    Bounds, Child, ContentMask, Corners, CursorStyle, Cx, Element, Gamut, InteractiveElement as _, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Painter, ParentElement as _, PinchEvent,
    Pixels, Point, RenderImage, ScrollDelta, ScrollWheelEvent, SharedString, Size, Styled as _, TouchPhase, View, WindowCx, div, fill, point, px, size,
};
use olo_kit_base::metrics as m;

use crate::{agent, style, theme};

/// A picture's pixels for an [`ImageView`]: RGBA, premultiplied, row by
/// row (olo-preview's `Bitmap` is one).
pub trait Rgba: Send + Sync + 'static {
    /// Its size in pixels.
    fn size(&self) -> [usize; 2];
    fn rgba(&self) -> &[u8];
    /// What its numbers are in (sRGB unless said): a Display P3 picture
    /// shows whole in a window drawing in P3, and
    /// converted into sRGB in any other.
    fn gamut(&self) -> Gamut {
        Gamut::Srgb
    }
}

pub const MIN_ZOOM: f32 = 0.05;
pub const MAX_ZOOM: f32 = 16.0;
const ZOOM_STEPS: [f32; 15] = [0.05, 0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 16.0];
/// Source pixels a side of a tile drawn magnified.
const TILE: usize = 512;
/// A picture this small (a side) is magnified with its pixels kept.
const SMALL: usize = 256;
/// The checkerboard's square, in points.
const CHECK: f32 = 8.0;

/// A resampling: of which picture, to what device size, the result, and
/// where it was last drawn (where it stands in for the next picture
/// until that one's is made: stepping through pictures doesn't flash).
struct Exact {
    generation: u64,
    size: (u32, u32),
    image: Arc<RenderImage>,
    bounds: Bounds<Pixels>,
}

/// What its drawing found it lacks, or a worker made.
pub enum Msg {
    /// The drawing wants a resampling or tiles made.
    Asked,
    Exact((u64, (u32, u32)), Option<Arc<RenderImage>>),
    Tile(u64, (usize, usize), Option<Arc<RenderImage>>),
}

/// What a drawing asks made, off the looper.
enum Ask {
    Exact((u32, u32)),
    Tile((usize, usize)),
}

pub struct ImageView {
    picture: Option<Arc<dyn Rgba>>,
    /// The picture's own size (it may have been decoded smaller).
    full: Size<f32>,
    /// Counts pictures shown, so what was made for one isn't shown for
    /// another.
    generation: u64,
    zoomable: bool,
    /// Pan and zoom by dragging are paused (a tool over it takes them).
    still: bool,
    /// Points per picture pixel; None fits it to the view.
    zoom: Option<f32>,
    /// The picture point at the view's middle (its own pixels), kept
    /// over the view as it's drawn.
    center: Cell<Point<f32>>,
    /// As last drawn: the scale that fits it, its view, and where it is.
    fit: Cell<f32>,
    view: Cell<Bounds<Pixels>>,
    shown: Cell<Option<Bounds<Pixels>>>,
    exact: RefCell<Option<Exact>>,
    /// The resampling being made (picture, device size), and the next one
    /// wanted once it's done.
    making: Option<(u64, (u32, u32))>,
    wanted: Option<(u64, (u32, u32))>,
    tiles: RefCell<HashMap<(usize, usize), Arc<RenderImage>>>,
    tiles_making: HashSet<(usize, usize)>,
    checker: RefCell<Option<(u32, Arc<RenderImage>)>>,
    /// What the last drawing lacked, for the view to have made (a
    /// drawing can only look).
    asks: RefCell<Vec<Ask>>,
    me: olo_interface::Messenger<Msg>,
    /// The pointer while panning, or where a press that may drag the
    /// picture out went down.
    drag: Option<Point<Pixels>>,
    dragging_out: bool,
    on_drag_out: Option<Rc<dyn Fn(&mut WindowCx)>>,
    /// What agents read it as.
    name: SharedString,
}

/// A view of no picture yet; `zoomable` for the Viewer (Quick Look's
/// only fits).
pub fn image_view<V: View>(cx: &mut Cx<V>, zoomable: bool) -> Child<ImageView> {
    cx.new_view(|cx| ImageView {
        picture: None,
        full: size(1.0, 1.0),
        generation: 0,
        zoomable,
        still: false,
        zoom: None,
        center: Cell::new(point(0.0, 0.0)),
        fit: Cell::new(1.0),
        view: Cell::default(),
        shown: Cell::new(None),
        exact: RefCell::new(None),
        making: None,
        wanted: None,
        tiles: RefCell::default(),
        tiles_making: HashSet::new(),
        checker: RefCell::new(None),
        asks: RefCell::default(),
        me: cx.messenger(),
        drag: None,
        dragging_out: false,
        on_drag_out: None,
        name: "Picture".into(),
    })
}

impl ImageView {
    /// What the picture's numbers are in (sRGB without one): a window
    /// showing a Display P3 picture may draw in P3.
    pub fn gamut(&self) -> Gamut {
        self.picture.as_ref().map_or(Gamut::Srgb, |p| p.gamut())
    }

    /// Show `picture`, whose own size is `full` pixels (bigger than it if
    /// it was decoded smaller). A picture of the same size (an edit that
    /// flips it) stays where it was; another is fitted.
    pub fn set_picture(&mut self, picture: Arc<dyn Rgba>, full: [f32; 2], cx: &mut Cx<Self>) {
        let full = size(full[0].max(1.0), full[1].max(1.0));
        if full != self.full {
            self.zoom = None;
        }
        self.full = full;
        self.picture = Some(picture);
        self.generation += 1;
        self.forget();
        cx.invalidate();
    }

    /// Show nothing.
    pub fn clear(&mut self, cx: &mut Cx<Self>) {
        self.picture = None;
        self.generation += 1;
        self.forget();
        cx.invalidate();
    }

    /// What agents read it as ("Picture" until said).
    pub fn set_name(&mut self, name: impl Into<SharedString>) {
        self.name = name.into();
    }

    /// Lets go of what was made for the last picture. (The last
    /// resampling stands in until the new one comes.)
    fn forget(&mut self) {
        self.making = None;
        self.wanted = None;
        self.tiles_making.clear();
        self.tiles.borrow_mut().clear();
        self.asks.borrow_mut().clear();
    }

    pub fn set_still(&mut self, on: bool) {
        self.still = on;
    }

    /// Called when a drag starts on the picture where there's nothing to
    /// pan (it fits): the app drags its file out.
    pub fn on_drag_out(&mut self, f: impl Fn(&mut WindowCx) + 'static) {
        self.on_drag_out = Some(Rc::new(f));
    }

    /// The scale it was last shown at (1: a picture pixel a point).
    pub fn scale(&self) -> f32 {
        self.zoom.unwrap_or(self.fit.get())
    }

    /// Whether it's fitted to the view (no zoom chosen).
    pub fn fitted(&self) -> bool {
        self.zoom.is_none()
    }

    /// Where the picture was last drawn, and the view it was drawn in.
    pub fn picture_bounds(&self) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
        self.shown.get().map(|s| (s, self.view.get()))
    }

    /// Zoom a step in (`Some(true)`) or out, or (`None`) back to fitting.
    pub fn zoom(&mut self, step: Option<bool>, cx: &mut Cx<Self>) {
        let cur = self.scale();
        self.zoom = match step {
            None => None,
            Some(true) => Some(ZOOM_STEPS.iter().copied().find(|s| *s > cur + 1e-3).unwrap_or(MAX_ZOOM)),
            Some(false) => Some(ZOOM_STEPS.iter().rev().copied().find(|s| *s < cur - 1e-3).unwrap_or(MIN_ZOOM)),
        };
        cx.invalidate();
    }

    /// Show it at `k` points a pixel (1: Actual Size).
    pub fn set_scale(&mut self, k: f32, cx: &mut Cx<Self>) {
        self.zoom = Some(k.clamp(MIN_ZOOM, MAX_ZOOM));
        cx.invalidate();
    }

    /// The picture point under `at` stays there as the scale goes to `to`.
    fn rescale(&mut self, to: f32, at: Point<Pixels>) {
        let k = self.scale();
        let to = to.clamp(MIN_ZOOM.min(self.fit.get()), MAX_ZOOM);
        let c = self.view.get().center();
        let (dx, dy) = (f32::from(at.x - c.x), f32::from(at.y - c.y));
        if self.zoom.is_none() {
            self.center.set(point(self.full.width / 2.0, self.full.height / 2.0));
        }
        let center = self.center.get();
        let under = point(center.x + dx / k, center.y + dy / k);
        self.center.set(point(under.x - dx / to, under.y - dy / to));
        self.zoom = Some(to);
    }

    /// Bigger than the view along either side (so it pans).
    fn bigger(&self) -> bool {
        let (k, view) = (self.scale(), self.view.get());
        self.full.width * k > f32::from(view.size.width) + 0.5 || self.full.height * k > f32::from(view.size.height) + 0.5
    }

    /// Keeps the picture over the view: centred along a side where it
    /// fits, edge to edge where it doesn't.
    fn keep_in(&self) {
        if self.zoom.is_none() {
            self.center.set(point(self.full.width / 2.0, self.full.height / 2.0));
            return;
        }
        let (k, view) = (self.scale(), self.view.get());
        let (vw, vh) = (f32::from(view.size.width), f32::from(view.size.height));
        let mut center = self.center.get();
        for (c, len, view) in [(&mut center.x, self.full.width, vw), (&mut center.y, self.full.height, vh)] {
            let half = view / 2.0 / k;
            *c = if len * k <= view { len / 2.0 } else { c.clamp(half, len - half) };
        }
        self.center.set(center);
    }

    fn wheel(&mut self, e: &ScrollWheelEvent, cx: &mut Cx<Self>) {
        if !self.zoomable || self.picture.is_none() {
            return;
        }
        let d = match e.delta {
            ScrollDelta::Pixels(p) => point(f32::from(p.x), f32::from(p.y)),
            ScrollDelta::Lines(l) => point(l.x * 40.0, l.y * 40.0),
        };
        if e.modifiers.control || e.modifiers.platform {
            let k = self.scale();
            self.rescale(k * 1.0025f32.powf(d.y), e.position);
        } else if self.bigger() {
            let (k, c) = (self.scale(), self.center.get());
            self.center.set(point(c.x - d.x / k, c.y - d.y / k));
        } else {
            return;
        }
        self.keep_in();
        cx.invalidate();
    }

    /// Two fingers pinching (a touchpad's, a touch screen's): zoomed by
    /// as much, about the fingers.
    fn pinch(&mut self, e: &PinchEvent, cx: &mut Cx<Self>) {
        if !self.zoomable || self.picture.is_none() || !matches!(e.phase, TouchPhase::Moved) || !e.step.is_finite() || e.step <= 0.0 {
            return;
        }
        let k = self.scale();
        self.rescale(k * e.step, e.position);
        self.keep_in();
        cx.stop_propagation();
        cx.invalidate();
    }

    fn press(&mut self, e: &MouseDownEvent, cx: &mut Cx<Self>) {
        if self.picture.is_none() || !self.shown.get().is_some_and(|s| s.contains(&e.position)) {
            return;
        }
        if self.zoomable && !self.still && e.click_count == 2 {
            if (self.zoom.is_some_and(|z| (z - 1.0).abs() < 1e-3)) || (self.fit.get() >= 1.0 && self.zoom.is_some()) {
                self.zoom = None;
            } else {
                self.rescale(1.0, e.position);
            }
            self.keep_in();
            cx.invalidate();
            return;
        }
        self.drag = Some(e.position);
        self.dragging_out = false;
    }

    fn moved(&mut self, e: &MouseMoveEvent, cx: &mut Cx<Self>) {
        let Some(from) = self.drag else { return };
        if e.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        if self.zoomable && !self.still && self.bigger() {
            let (k, c) = (self.scale(), self.center.get());
            let d = e.position - from;
            self.center.set(point(c.x - f32::from(d.x) / k, c.y - f32::from(d.y) / k));
            self.drag = Some(e.position);
            self.keep_in();
            cx.invalidate();
        } else if !self.still && !self.dragging_out && {
            let d = e.position - from;
            f32::from(d.x).hypot(f32::from(d.y)) > 4.0
        } {
            self.dragging_out = true;
            if let Some(f) = self.on_drag_out.clone() {
                f(cx);
            }
        }
    }

    /// The resampling for device size `want` of this picture: asked of a
    /// worker unless it's being made (then it's next).
    fn want_exact(&mut self, want: (u32, u32), cx: &mut Cx<Self>) {
        let key = (self.generation, want);
        if self.making == Some(key) {
            return;
        }
        if self.making.is_some() {
            self.wanted = Some(key);
            return;
        }
        let Some(src) = self.picture.clone() else { return };
        self.making = Some(key);
        cx.work(move || resampled(&*src, want), move |made| Msg::Exact(key, made));
    }

    fn want_tile(&mut self, at: (usize, usize), cx: &mut Cx<Self>) {
        if !self.tiles_making.insert(at) {
            return;
        }
        let Some(src) = self.picture.clone() else { return };
        let generation = self.generation;
        cx.work(move || tile(&*src, at), move |made| Msg::Tile(generation, at, made));
    }

    /// Asks the view (from its drawing, which can only look) to have
    /// something made.
    fn ask(&self, ask: Ask) {
        let mut asks = self.asks.borrow_mut();
        if asks.is_empty() {
            self.me.send(Msg::Asked);
        }
        asks.push(ask);
    }

    fn paint(&self, g: &mut Painter, b: Bounds<Pixels>) {
        let p = theme::palette();
        g.paint_quad(fill(b, style::c(p.panel_background)));
        self.view.set(b);
        let Some(src) = self.picture.clone() else {
            self.shown.set(None);
            return;
        };
        let s = g.scale_factor();
        let (vw, vh) = (f32::from(b.size.width), f32::from(b.size.height));
        self.fit.set(((vw - 2.0 * m::SPACING) / self.full.width).min((vh - 2.0 * m::SPACING) / self.full.height).clamp(0.001, 1.0));
        self.keep_in();
        let (k, center) = (self.scale(), self.center.get());
        // Its device size and place, on whole device pixels.
        let (dw, dh) = ((self.full.width * k * s).round().max(1.0), (self.full.height * k * s).round().max(1.0));
        let c = b.center();
        let x0 = ((f32::from(c.x) - center.x * k) * s).round() / s;
        let y0 = ((f32::from(c.y) - center.y * k) * s).round() / s;
        let shown = Bounds::new(point(px(x0), px(y0)), size(px(dw / s), px(dh / s)));
        self.shown.set(Some(shown));
        let visible = shown.intersect(&b);
        agent::record(g, agent::Node { role: "Image", name: Some(self.name.clone()), bounds: visible, enabled: true, ..Default::default() });
        g.with_content_mask(Some(ContentMask { bounds: b }), |g| {
            self.paint_checker(visible, s, g);
            let [sw, sh] = src.size();
            // Device pixels a source pixel.
            let d = dw / sw.max(1) as f32;
            let whole = (dw as u32, dh as u32);
            let view_area = vw * vh * s * s;
            if d <= 1.0 + 1e-3 && dw * dh <= 2.0 * view_area.max(1.0) {
                let mut exact = self.exact.borrow_mut();
                if exact.as_ref().is_none_or(|e| e.generation != self.generation || e.size != whole) {
                    self.ask(Ask::Exact(whole));
                }
                if let Some(e) = exact.as_mut() {
                    // The last picture's where it was, until this one's.
                    let at = if e.generation == self.generation { shown } else { e.bounds };
                    e.bounds = at;
                    let _ = g.paint_image(at, Corners::default(), e.image.clone(), 0, false);
                }
                return;
            }
            // Magnified (or minified past what a resampling is made for):
            // the tiles in view, the resampling under them meanwhile.
            if let Some(e) = self.exact.borrow().as_ref()
                && e.generation == self.generation
            {
                let _ = g.paint_image(shown, Corners::default(), e.image.clone(), 0, false);
            }
            let per = f32::from(shown.size.width) / sw as f32;
            let small = sw.max(sh) <= SMALL;
            let (cols, rows) = (sw.div_ceil(TILE), sh.div_ceil(TILE));
            let mut keep = Vec::new();
            let mut tiles = self.tiles.borrow_mut();
            for ty in 0..rows {
                for tx in 0..cols {
                    let (ox, oy) = (tx * TILE, ty * TILE);
                    let (tw, th) = ((sw - ox).min(TILE), (sh - oy).min(TILE));
                    let tb = Bounds::new(point(shown.origin.x + px(ox as f32 * per), shown.origin.y + px(oy as f32 * per)), size(px(tw as f32 * per), px(th as f32 * per)));
                    if tb.intersect(&b).is_empty() {
                        continue;
                    }
                    keep.push((tx, ty));
                    match tiles.get(&(tx, ty)) {
                        Some(t) if small => {
                            let _ = g.paint_image_nearest(tb, t.clone());
                        }
                        Some(t) => {
                            let _ = g.paint_image(tb, Corners::default(), t.clone(), 0, false);
                        }
                        None => self.ask(Ask::Tile((tx, ty))),
                    }
                }
            }
            // Tiles far out of view go (made again if it comes back).
            if tiles.len() > keep.len() + 16 {
                tiles.retain(|k, _| keep.contains(k));
            }
        });
    }

    /// The light checkerboard under `r` (always light: dark artwork must
    /// show), from a tile made at the scale, a texture pixel to a device
    /// pixel.
    fn paint_checker(&self, r: Bounds<Pixels>, s: f32, g: &mut Painter) {
        if r.is_empty() {
            return;
        }
        let cell = (CHECK * s).round().max(1.0) as u32;
        let mut checker = self.checker.borrow_mut();
        if checker.as_ref().is_none_or(|(c, _)| *c != cell) {
            *checker = Some((cell, checker_tile(cell)));
        }
        let Some((cell, img)) = checker.as_ref() else { return };
        let n = (2 * cell * 16) as f32 / s;
        let r = style::snap(g, r);
        g.with_content_mask(Some(ContentMask { bounds: r }), |g| {
            let mut y = f32::from(r.origin.y);
            while y < f32::from(r.bottom()) {
                let mut x = f32::from(r.origin.x);
                while x < f32::from(r.right()) {
                    let _ = g.paint_image(Bounds::new(point(px(x), px(y)), size(px(n), px(n))), Corners::default(), img.clone(), 0, false);
                    x += n;
                }
                y += n;
            }
        });
    }
}

/// 16 × 16 squares of `cell` device pixels, white and a light grey.
fn checker_tile(cell: u32) -> Arc<RenderImage> {
    let n = 2 * cell * 16;
    let mut bgra = vec![0u8; (n * n * 4) as usize];
    for (i, px) in bgra.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (i as u32 % n, i as u32 / n);
        let v = if (x / cell + y / cell).is_multiple_of(2) { 255 } else { 232 };
        *px = [v, v, v, 255];
    }
    Arc::new(RenderImage::new([image::Frame::new(image::RgbaImage::from_raw(n, n, bgra).expect("sized"))]))
}

/// Premultiplied RGBA to straight BGRA (the Interface Kit's images), in place.
fn to_bgra(px: &mut [u8]) {
    for p in px.as_chunks_mut::<4>().0 {
        let a = p[3];
        if a != 0 && a != 255 {
            let k = 255.0 / f32::from(a);
            for c in &mut p[..3] {
                *c = (f32::from(*c) * k).round().min(255.0) as u8;
            }
        }
        p.swap(0, 2);
    }
}

/// `src` resampled to `(w, h)` device pixels, to draw a texel a device
/// pixel (`pictures::resampled`, in its gamut).
pub fn resampled(src: &dyn Rgba, (w, h): (u32, u32)) -> Option<Arc<RenderImage>> {
    let [sw, sh] = src.size();
    let image = crate::pictures::resampled(src.rgba(), (sw as u32, sh as u32), (w, h))?;
    Some(match src.gamut() {
        Gamut::Srgb => image,
        // (Just made: nothing else holds it.)
        gamut => match Arc::try_unwrap(image) {
            Ok(image) => Arc::new(image.with_gamut(gamut)),
            Err(image) => image,
        },
    })
}

/// The tile at `(tx, ty)` (of [`TILE`] source pixels a side) as it is.
fn tile(src: &dyn Rgba, (tx, ty): (usize, usize)) -> Option<Arc<RenderImage>> {
    let [sw, sh] = src.size();
    let (ox, oy) = (tx * TILE, ty * TILE);
    if ox >= sw || oy >= sh {
        return None;
    }
    let (tw, th) = ((sw - ox).min(TILE), (sh - oy).min(TILE));
    let rgba = src.rgba();
    let mut out = Vec::with_capacity(tw * th * 4);
    for y in oy..oy + th {
        let row = (y * sw + ox) * 4;
        out.extend_from_slice(&rgba[row..row + tw * 4]);
    }
    to_bgra(&mut out);
    Some(Arc::new(RenderImage::new([image::Frame::new(image::RgbaImage::from_raw(tw as u32, th as u32, out)?)]).with_gamut(src.gamut())))
}

impl View for ImageView {
    type Message = Msg;

    fn message(&mut self, msg: Msg, cx: &mut Cx<Self>) {
        match msg {
            Msg::Asked => {
                let asks = std::mem::take(&mut *self.asks.borrow_mut());
                for ask in asks {
                    match ask {
                        Ask::Exact(size) => self.want_exact(size, cx),
                        Ask::Tile(at) => self.want_tile(at, cx),
                    }
                }
                return;
            }
            Msg::Exact(key, made) => {
                if let Some(image) = made
                    && self.generation == key.0
                {
                    *self.exact.borrow_mut() = Some(Exact { generation: key.0, size: key.1, image, bounds: Bounds::default() });
                }
                self.making = None;
                if let Some((generation, next)) = self.wanted.take()
                    && generation == self.generation
                {
                    self.want_exact(next, cx);
                }
            }
            Msg::Tile(generation, at, made) => {
                if generation != self.generation {
                    return;
                }
                self.tiles_making.remove(&at);
                if let Some(t) = made {
                    self.tiles.borrow_mut().insert(at, t);
                }
            }
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let cursor = if self.zoomable && !self.still && self.picture.is_some() && self.bigger() {
            if self.drag.is_some() { CursorStyle::ClosedHand } else { CursorStyle::OpenHand }
        } else {
            CursorStyle::Arrow
        };
        div()
            .id("image-view")
            .size_full()
            .cursor(cursor)
            .child(cx.canvas(|v: &ImageView, g, b| v.paint(g, b)).size_full())
            .on_scroll_wheel(cx.listener(|v: &mut Self, e: &ScrollWheelEvent, cx| v.wheel(e, cx)))
            .on_pinch(cx.listener(|v: &mut Self, e: &PinchEvent, cx| v.pinch(e, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(|v: &mut Self, e: &MouseDownEvent, cx| v.press(e, cx)))
            .on_mouse_move(cx.listener(|v: &mut Self, e: &MouseMoveEvent, cx| v.moved(e, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|v: &mut Self, _: &MouseUpEvent, cx| {
                    if v.drag.take().is_some() {
                        cx.invalidate();
                    }
                }),
            )
    }
}
