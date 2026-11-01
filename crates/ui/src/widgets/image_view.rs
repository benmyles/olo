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
    collections::HashMap,
    rc::Rc,
    sync::Arc,
};

use gpui::{
    App, AppContext as _, Bounds, Context, CursorStyle, Entity, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render, RenderImage, ScrollDelta, ScrollWheelEvent, Size, Styled as _, Task, Window, canvas, div, point, px, size,
};
use olo_kit_base::metrics as m;

use crate::{agent, style, theme};

/// A picture's pixels for an [`ImageView`]: RGBA, premultiplied, row by
/// row (olo-preview's `Bitmap` is one).
pub trait Rgba: Send + Sync + 'static {
    /// Its size in pixels.
    fn size(&self) -> [usize; 2];
    fn rgba(&self) -> &[u8];
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
    /// The picture point at the view's middle (its own pixels).
    center: Point<f32>,
    fit: f32,
    view: Bounds<Pixels>,
    /// Where it was last drawn.
    shown: Option<Bounds<Pixels>>,
    exact: Option<Exact>,
    /// The resampling being made (picture, device size), and the next one
    /// wanted once it's done.
    making: Option<((u64, (u32, u32)), Task<()>)>,
    wanted: Option<(u64, (u32, u32))>,
    tiles: HashMap<(usize, usize), Arc<RenderImage>>,
    tiles_making: HashMap<(usize, usize), Task<()>>,
    checker: Option<(u32, Arc<RenderImage>)>,
    /// Images to take out of the window's atlas when next painted.
    dropped: Vec<Arc<RenderImage>>,
    /// The pointer while panning, or where a press that may drag the
    /// picture out went down.
    drag: Option<Point<Pixels>>,
    dragging_out: bool,
    on_drag_out: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    /// What agents read it as.
    name: gpui::SharedString,
}

/// A view of no picture yet; `zoomable` for the Viewer (Quick Look's
/// only fits).
pub fn image_view(cx: &mut App, zoomable: bool) -> Entity<ImageView> {
    cx.new(|_| ImageView {
        picture: None,
        full: size(1.0, 1.0),
        generation: 0,
        zoomable,
        still: false,
        zoom: None,
        center: point(0.0, 0.0),
        fit: 1.0,
        view: Bounds::default(),
        shown: None,
        exact: None,
        making: None,
        wanted: None,
        tiles: HashMap::new(),
        tiles_making: HashMap::new(),
        checker: None,
        dropped: Vec::new(),
        drag: None,
        dragging_out: false,
        on_drag_out: None,
        name: "Picture".into(),
    })
}

impl ImageView {
    /// Show `picture`, whose own size is `full` pixels (bigger than it if
    /// it was decoded smaller). A picture of the same size (an edit that
    /// flips it) stays where it was; another is fitted.
    pub fn set_picture(&mut self, picture: Arc<dyn Rgba>, full: [f32; 2], cx: &mut Context<Self>) {
        let full = size(full[0].max(1.0), full[1].max(1.0));
        if full != self.full {
            self.zoom = None;
        }
        self.full = full;
        self.picture = Some(picture);
        self.generation += 1;
        self.forget();
        cx.notify();
    }

    /// Show nothing.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.picture = None;
        self.generation += 1;
        self.forget();
        cx.notify();
    }

    /// What agents read it as ("Picture" until said).
    pub fn set_name(&mut self, name: impl Into<gpui::SharedString>) {
        self.name = name.into();
    }

    /// Lets go of what was made for the last picture (its tiles go from
    /// the atlas as the view next paints).
    fn forget(&mut self) {
        self.making = None;
        self.wanted = None;
        self.tiles_making.clear();
        // (The last resampling stands in until the new one comes.)
        self.dropped.extend(self.tiles.drain().map(|(_, t)| t));
    }

    pub fn set_still(&mut self, on: bool) {
        self.still = on;
    }

    /// Called when a drag starts on the picture where there's nothing to
    /// pan (it fits): the app drags its file out.
    pub fn on_drag_out(&mut self, f: impl Fn(&mut Window, &mut App) + 'static) {
        self.on_drag_out = Some(Rc::new(f));
    }

    /// The scale it was last shown at (1: a picture pixel a point).
    pub fn scale(&self) -> f32 {
        self.zoom.unwrap_or(self.fit)
    }

    /// Whether it's fitted to the view (no zoom chosen).
    pub fn fitted(&self) -> bool {
        self.zoom.is_none()
    }

    /// Where the picture was last drawn, and the view it was drawn in.
    pub fn picture_bounds(&self) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
        self.shown.map(|s| (s, self.view))
    }

    /// Zoom a step in (`Some(true)`) or out, or (`None`) back to fitting.
    pub fn zoom(&mut self, step: Option<bool>, cx: &mut Context<Self>) {
        let cur = self.scale();
        self.zoom = match step {
            None => None,
            Some(true) => Some(ZOOM_STEPS.iter().copied().find(|s| *s > cur + 1e-3).unwrap_or(MAX_ZOOM)),
            Some(false) => Some(ZOOM_STEPS.iter().rev().copied().find(|s| *s < cur - 1e-3).unwrap_or(MIN_ZOOM)),
        };
        cx.notify();
    }

    /// Show it at `k` points a pixel (1: Actual Size).
    pub fn set_scale(&mut self, k: f32, cx: &mut Context<Self>) {
        self.zoom = Some(k.clamp(MIN_ZOOM, MAX_ZOOM));
        cx.notify();
    }

    /// The picture point under `at` stays there as the scale goes to `to`.
    fn rescale(&mut self, to: f32, at: Point<Pixels>) {
        let k = self.scale();
        let to = to.clamp(MIN_ZOOM.min(self.fit), MAX_ZOOM);
        let c = self.view.center();
        let (dx, dy) = (f32::from(at.x - c.x), f32::from(at.y - c.y));
        if self.zoom.is_none() {
            self.center = point(self.full.width / 2.0, self.full.height / 2.0);
        }
        let under = point(self.center.x + dx / k, self.center.y + dy / k);
        self.center = point(under.x - dx / to, under.y - dy / to);
        self.zoom = Some(to);
    }

    /// Bigger than the view along either side (so it pans).
    fn bigger(&self) -> bool {
        let k = self.scale();
        self.full.width * k > f32::from(self.view.size.width) + 0.5 || self.full.height * k > f32::from(self.view.size.height) + 0.5
    }

    /// Keeps the picture over the view: centred along a side where it
    /// fits, edge to edge where it doesn't.
    fn keep_in(&mut self) {
        if self.zoom.is_none() {
            self.center = point(self.full.width / 2.0, self.full.height / 2.0);
            return;
        }
        let k = self.scale();
        let (vw, vh) = (f32::from(self.view.size.width), f32::from(self.view.size.height));
        for (c, len, view) in [(&mut self.center.x, self.full.width, vw), (&mut self.center.y, self.full.height, vh)] {
            let half = view / 2.0 / k;
            *c = if len * k <= view { len / 2.0 } else { c.clamp(half, len - half) };
        }
    }

    fn wheel(&mut self, e: &ScrollWheelEvent, cx: &mut Context<Self>) {
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
            let k = self.scale();
            self.center = point(self.center.x - d.x / k, self.center.y - d.y / k);
        } else {
            return;
        }
        self.keep_in();
        cx.notify();
    }

    fn press(&mut self, e: &MouseDownEvent, cx: &mut Context<Self>) {
        if self.picture.is_none() || !self.shown.is_some_and(|s| s.contains(&e.position)) {
            return;
        }
        if self.zoomable && !self.still && e.click_count == 2 {
            let k = self.scale();
            if (self.zoom.is_some_and(|z| (z - 1.0).abs() < 1e-3)) || (self.fit >= 1.0 && self.zoom.is_some()) {
                self.zoom = None;
            } else {
                let _ = k;
                self.rescale(1.0, e.position);
            }
            self.keep_in();
            cx.notify();
            return;
        }
        self.drag = Some(e.position);
        self.dragging_out = false;
    }

    fn moved(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(from) = self.drag else { return };
        if e.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        if self.zoomable && !self.still && self.bigger() {
            let k = self.scale();
            let d = e.position - from;
            self.center = point(self.center.x - f32::from(d.x) / k, self.center.y - f32::from(d.y) / k);
            self.drag = Some(e.position);
            self.keep_in();
            cx.notify();
        } else if !self.still && !self.dragging_out && {
            let d = e.position - from;
            f32::from(d.x).hypot(f32::from(d.y)) > 4.0
        } {
            self.dragging_out = true;
            if let Some(f) = self.on_drag_out.clone() {
                f(window, cx);
            }
        }
    }

    /// The resampling for device size `want` of this picture: asked of a
    /// worker unless it's being made (then it's next).
    fn want_exact(&mut self, want: (u32, u32), cx: &mut Context<Self>) {
        let key = (self.generation, want);
        if self.making.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        if self.making.is_some() {
            self.wanted = Some(key);
            return;
        }
        let Some(src) = self.picture.clone() else { return };
        let task = cx.spawn(async move |view, cx| {
            let made = cx.background_spawn(async move { resampled(&*src, want) }).await;
            let _ = view.update(cx, |v, cx| {
                if let Some(image) = made
                    && v.generation == key.0
                {
                    if let Some(old) = v.exact.replace(Exact { generation: key.0, size: want, image, bounds: Bounds::default() }) {
                        v.dropped.push(old.image);
                    }
                }
                v.making = None;
                if let Some((generation, next)) = v.wanted.take()
                    && generation == v.generation
                {
                    v.want_exact(next, cx);
                }
                cx.notify();
            });
        });
        self.making = Some((key, task));
    }

    fn want_tile(&mut self, at: (usize, usize), cx: &mut Context<Self>) {
        if self.tiles_making.contains_key(&at) {
            return;
        }
        let Some(src) = self.picture.clone() else { return };
        let generation = self.generation;
        let task = cx.spawn(async move |view, cx| {
            let made = cx.background_spawn(async move { tile(&*src, at) }).await;
            let _ = view.update(cx, |v, cx| {
                if v.generation == generation {
                    v.tiles_making.remove(&at);
                    if let Some(t) = made {
                        v.tiles.insert(at, t);
                    }
                    cx.notify();
                }
            });
        });
        self.tiles_making.insert(at, task);
    }

    fn paint(&mut self, b: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        for img in self.dropped.drain(..) {
            let _ = window.drop_image(img);
        }
        let p = theme::palette(cx);
        window.paint_quad(gpui::fill(b, style::c(p.panel_background)));
        self.view = b;
        let Some(src) = self.picture.clone() else {
            self.shown = None;
            return;
        };
        let s = window.scale_factor();
        let (vw, vh) = (f32::from(b.size.width), f32::from(b.size.height));
        self.fit = ((vw - 2.0 * m::SPACING) / self.full.width).min((vh - 2.0 * m::SPACING) / self.full.height).clamp(0.001, 1.0);
        self.keep_in();
        let k = self.scale();
        // Its device size and place, on whole device pixels.
        let (dw, dh) = ((self.full.width * k * s).round().max(1.0), (self.full.height * k * s).round().max(1.0));
        let c = b.center();
        let x0 = ((f32::from(c.x) - self.center.x * k) * s).round() / s;
        let y0 = ((f32::from(c.y) - self.center.y * k) * s).round() / s;
        let shown = Bounds::new(point(px(x0), px(y0)), size(px(dw / s), px(dh / s)));
        self.shown = Some(shown);
        let visible = shown.intersect(&b);
        agent::record(window, cx, agent::Node { role: "Image", name: Some(self.name.clone()), bounds: visible, enabled: true, ..Default::default() });
        window.with_content_mask(Some(gpui::ContentMask { bounds: b }), |window| {
            self.paint_checker(visible, s, window);
            let [sw, sh] = src.size();
            // Device pixels a source pixel.
            let d = dw / sw.max(1) as f32;
            let whole = (dw as u32, dh as u32);
            let view_area = vw * vh * s * s;
            if d <= 1.0 + 1e-3 && dw * dh <= 2.0 * view_area.max(1.0) {
                if self.exact.as_ref().is_none_or(|e| e.generation != self.generation || e.size != whole) {
                    self.want_exact(whole, cx);
                }
                if let Some(e) = &mut self.exact {
                    // The last picture's where it was, until this one's.
                    let at = if e.generation == self.generation { shown } else { e.bounds };
                    e.bounds = at;
                    let _ = window.paint_image(at, gpui::Corners::default(), e.image.clone(), 0, false);
                }
                return;
            }
            // Magnified (or minified past what a resampling is made for):
            // the tiles in view, the resampling under them meanwhile.
            if let Some(e) = &self.exact
                && e.generation == self.generation
            {
                let _ = window.paint_image(shown, gpui::Corners::default(), e.image.clone(), 0, false);
            }
            let per = f32::from(shown.size.width) / sw as f32;
            let small = sw.max(sh) <= SMALL;
            let (cols, rows) = (sw.div_ceil(TILE), sh.div_ceil(TILE));
            let mut keep = Vec::new();
            for ty in 0..rows {
                for tx in 0..cols {
                    let (ox, oy) = (tx * TILE, ty * TILE);
                    let (tw, th) = ((sw - ox).min(TILE), (sh - oy).min(TILE));
                    let tb = Bounds::new(point(shown.origin.x + px(ox as f32 * per), shown.origin.y + px(oy as f32 * per)), size(px(tw as f32 * per), px(th as f32 * per)));
                    if tb.intersect(&b).is_empty() {
                        continue;
                    }
                    keep.push((tx, ty));
                    match self.tiles.get(&(tx, ty)) {
                        Some(t) if small => {
                            let _ = window.paint_image_nearest(tb, t.clone());
                        }
                        Some(t) => {
                            let _ = window.paint_image(tb, gpui::Corners::default(), t.clone(), 0, false);
                        }
                        None => self.want_tile((tx, ty), cx),
                    }
                }
            }
            // Tiles far out of view go (made again if it comes back).
            if self.tiles.len() > keep.len() + 16 {
                let gone: Vec<_> = self.tiles.keys().filter(|k| !keep.contains(k)).copied().collect();
                for k in gone {
                    if let Some(t) = self.tiles.remove(&k) {
                        self.dropped.push(t);
                    }
                }
            }
        });
    }

    /// The light checkerboard under `r` (always light: dark artwork must
    /// show), from a tile made at the scale, a texture pixel to a device
    /// pixel.
    fn paint_checker(&mut self, r: Bounds<Pixels>, s: f32, window: &mut Window) {
        if r.is_empty() {
            return;
        }
        let cell = (CHECK * s).round().max(1.0) as u32;
        if self.checker.as_ref().is_none_or(|(c, _)| *c != cell) {
            if let Some((_, old)) = self.checker.take() {
                self.dropped.push(old);
            }
            self.checker = Some((cell, checker_tile(cell)));
        }
        let Some((cell, img)) = &self.checker else { return };
        let n = (2 * cell * 16) as f32 / s;
        let r = style::snap(window, r);
        window.with_content_mask(Some(gpui::ContentMask { bounds: r }), |window| {
            let mut y = f32::from(r.origin.y);
            while y < f32::from(r.bottom()) {
                let mut x = f32::from(r.origin.x);
                while x < f32::from(r.right()) {
                    let _ = window.paint_image(Bounds::new(point(px(x), px(y)), size(px(n), px(n))), gpui::Corners::default(), img.clone(), 0, false);
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

/// Premultiplied RGBA to straight BGRA (GPUI's images), in place.
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

/// `src` resampled to `(w, h)` device pixels (on its premultiplied
/// colours: Lanczos made smaller; Catmull-Rom made bigger, as sharp to
/// the eye at a third of the cost, 3 ms against 9 for a 1024 × 640
/// screen at 1.4×), as an image to draw a texel a device pixel (a
/// cover, a poster, an agent's screen; call it off the UI thread).
pub fn resampled(src: &dyn Rgba, (w, h): (u32, u32)) -> Option<Arc<RenderImage>> {
    use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer, images::{Image, ImageRef}};
    let [sw, sh] = src.size();
    let mut out = if (sw as u32, sh as u32) == (w, h) {
        src.rgba().to_vec()
    } else {
        let from = ImageRef::new(sw as u32, sh as u32, src.rgba(), PixelType::U8x4).ok()?;
        let mut to = Image::new(w, h, PixelType::U8x4);
        let filter = if w as usize > sw || h as usize > sh { FilterType::CatmullRom } else { FilterType::Lanczos3 };
        let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(filter)).use_alpha(false);
        Resizer::new().resize(&from, &mut to, &options).ok()?;
        to.into_vec()
    };
    to_bgra(&mut out);
    Some(Arc::new(RenderImage::new([image::Frame::new(image::RgbaImage::from_raw(w, h, out)?)])))
}

/// A picture from pixels already in GPUI's order, premultiplied BGRA
/// (`w` × `h`: a captured screen's frame, opaque), to draw as it is.
pub fn from_bgra(w: u32, h: u32, bgra: Vec<u8>) -> Option<Arc<RenderImage>> {
    Some(Arc::new(RenderImage::new([image::Frame::new(image::RgbaImage::from_raw(w, h, bgra)?)])))
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
    Some(Arc::new(RenderImage::new([image::Frame::new(image::RgbaImage::from_raw(tw as u32, th as u32, out)?)])))
}

impl Render for ImageView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let paint = me.clone();
        let cursor = if self.zoomable && !self.still && self.picture.is_some() && self.bigger() {
            if self.drag.is_some() { CursorStyle::ClosedHand } else { CursorStyle::OpenHand }
        } else {
            CursorStyle::Arrow
        };
        div()
            .id("image-view")
            .size_full()
            .cursor(cursor)
            .child(canvas(|_, _, _| {}, move |b, _, window, cx| paint.update(cx, |v, cx| v.paint(b, window, cx))).size_full())
            .on_scroll_wheel(cx.listener(|v, e: &ScrollWheelEvent, _, cx| v.wheel(e, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(|v, e: &MouseDownEvent, _, cx| v.press(e, cx)))
            .on_mouse_move(cx.listener(|v, e: &MouseMoveEvent, window, cx| v.moved(e, window, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|v, _: &MouseUpEvent, _, cx| {
                    if v.drag.take().is_some() {
                        cx.notify();
                    }
                }),
            )
    }
}

/// What resampling costs, by filter (`cargo test --release -p olo-ui
/// resample_timing -- --ignored --nocapture`).
#[cfg(test)]
mod resample_timing {
    #[test]
    #[ignore]
    fn filters() {
        use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer, images::{Image, ImageRef}};
        let (sw, sh) = (1024u32, 640u32);
        let src: Vec<u8> = (0..sw * sh * 4).map(|i| (i * 7 % 251) as u8).collect();
        for (to_w, to_h) in [(1421u32, 888u32), (710, 444)] {
            for f in [FilterType::Lanczos3, FilterType::CatmullRom, FilterType::Bilinear] {
                let from = ImageRef::new(sw, sh, &src, PixelType::U8x4).unwrap();
                let mut to = Image::new(to_w, to_h, PixelType::U8x4);
                let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(f)).use_alpha(false);
                let mut r = Resizer::new();
                let t = std::time::Instant::now();
                for _ in 0..10 {
                    r.resize(&from, &mut to, &options).unwrap();
                }
                eprintln!("{to_w}x{to_h} {f:?}: {:.2} ms", t.elapsed().as_secs_f64() * 100.0);
            }
        }
    }
}
