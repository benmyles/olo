//! `--region` (⌘⇧4): the screen freezes, dims, and you drag out the part
//! you want; it is saved like a silent screenshot. Escape (or a click
//! without a drag) cancels. `--record --region` picks the part to record
//! the same way (`pick_region`).
//!
//! A bar at the top of each screen, as CleanShot X's: a shape the drag
//! keeps (Free, 16:9, 4:3, 1:1; keys 1 to 4; ⇧ makes a free drag
//! square), and sizes (1280 × 720, 1920 × 1080; keys 7 and 8) that place
//! a frame that many pixels big, which is dragged where it's wanted and
//! taken with Return or a click in it.
//!
//! Every output is captured first, then covered by an overlay showing its
//! picture one pixel to a device pixel, so what you select is exactly what
//! you saw.
//!
//! `--pick-color` (the Screenshot portal's PickColor) freezes the screens
//! the same way, undimmed, with a loupe beside the pointer: the pixels
//! around it magnified, the one under it outlined, its colour as a
//! swatch and in hex. A click picks it; Escape cancels.

use olo_sync::Mutex;
use std::{cell::Cell, path::PathBuf, rc::Rc, sync::Arc};

use olo_kit::app_kit::Messenger;
use olo_kit::{
    Bounds, ClickEvent, Corners, CursorStyle, Cx, Element, Font, FontWeight, Hsla, InteractiveElement as _, KeyDownEvent, LayerKeyboard, LayerShellLayer, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Painter, ParentElement as _, Pixels, Point, RenderImage, Styled as _, View, WindowOptions, canvas, div, metrics as m, point, px, rgb, size, style, theme, widgets::Button,
};

use crate::capture::{self, Settings, Shot};

const APP_ID: &str = "olo.screenshot";

/// A drag shorter than this (points) is a click: cancel.
const MIN_DRAG: f32 = 4.0;
/// The loupe: this many pixels a side, each this many points, this far
/// from the pointer.
const LOUPE_PIXELS: i64 = 9;
const LOUPE_CELL: f32 = 10.0;
const LOUPE_GAP: f32 = 20.0;

/// The shapes a drag may keep (width over height; None: any).
const SHAPES: [(&str, Option<f32>); 4] = [("Free", None), ("16:9", Some(16.0 / 9.0)), ("4:3", Some(4.0 / 3.0)), ("1:1", Some(1.0))];
/// Sizes a frame is placed at (pixels).
const SIZES: [(u32, u32); 2] = [(1280, 720), (1920, 1080)];

/// The corner `z` moved so the rectangle from `a` keeps `ratio` (the
/// longer way the pointer went sets the size).
fn keep_shape(a: Point<Pixels>, z: Point<Pixels>, ratio: f32) -> Point<Pixels> {
    let (dx, dy) = (f32::from(z.x - a.x), f32::from(z.y - a.y));
    let (w, h) = (dx.abs(), dy.abs());
    let (w, h) = if h == 0.0 || w / h > ratio { (h * ratio, h) } else { (w, w / ratio) };
    let (w, h) = if w < 1.0 && h < 1.0 { (dx.abs().max(dy.abs() * ratio), dx.abs().max(dy.abs() * ratio) / ratio) } else { (w, h) };
    point(a.x + px(w * dx.signum()), a.y + px(h * dy.signum()))
}

/// What the app's looper is told: an output's picture (and it as it's
/// drawn), or None if it couldn't be taken.
struct Frozen(Option<String>, Option<(image::RgbaImage, Arc<RenderImage>)>);

/// The part chosen: the output's picture and a rectangle in its pixels,
/// and which output that is.
type Cut = (Arc<image::RgbaImage>, [u32; 4], Option<String>);

/// What the overlays (one a screen, each on a looper of its own) share.
struct Shared {
    cut: Arc<Mutex<Option<Cut>>>,
    /// Picking a colour (`--pick-color`) rather than a region: the one
    /// picked.
    pick: Option<Arc<Mutex<Option<[u8; 3]>>>>,
    /// Every overlay, to tell what the others show too.
    overlays: Mutex<Vec<Messenger<Told>>>,
}

impl Shared {
    fn tell(&self, told: Told) {
        for o in self.overlays.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            o.send(told);
        }
    }

    /// The overlays go; the cut (if any) is saved once the app returns.
    fn finish(&self, cut: Option<Cut>) {
        *self.cut.lock().unwrap_or_else(|e| e.into_inner()) = cut;
        olo_kit::quit();
    }
}

/// What an overlay is told by another.
#[derive(Clone, Copy)]
enum Told {
    /// The shape drags keep (an index into `SHAPES`).
    Shape(usize),
    /// A frame was placed, or a drag begun, on screen `index`: there's
    /// one selection, not one a screen.
    Chosen(usize),
}

/// One output's overlay.
struct Overlay {
    shared: Arc<Shared>,
    index: usize,
    /// The output it is (None: the only one, unnamed).
    output: Option<String>,
    image: Arc<image::RgbaImage>,
    drawn: Arc<RenderImage>,
    /// The press and where the pointer is now.
    drag: Option<(Point<Pixels>, Point<Pixels>)>,
    /// The shape drags keep (an index into `SHAPES`).
    shape: usize,
    /// A frame placed by a size (its bounds in the window), and where in
    /// it a press is moving it from.
    placed: Option<Bounds<Pixels>>,
    moving: Option<Point<Pixels>>,
    /// Where the bar was drawn (presses there aren't drags).
    bar: Rc<Cell<Bounds<Pixels>>>,
    /// The pointer is on this screen, and where (the loupe's place).
    hovered: bool,
    pointer: Point<Pixels>,
}

/// `image` as it's drawn (BGRA), made on the worker that took it.
fn drawn(image: &image::RgbaImage) -> Option<Arc<RenderImage>> {
    let mut bgra = image.as_raw().clone();
    for p in bgra.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(image.width(), image.height(), bgra)?;
    Some(Arc::new(RenderImage::new([image::Frame::new(buffer)])))
}

impl Overlay {
    /// The screen in the window, from its top left: its picture's pixels
    /// at the window's scale, one to a device pixel. (Not the window's own
    /// size: at a fractional scale that's whole points, half a device
    /// pixel more or less than the screen, and a frame placed by a size
    /// came out a pixel too wide.)
    fn screen(&self, scale: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(0.0), px(0.0)), size(px(self.image.width() as f32 / scale), px(self.image.height() as f32 / scale)))
    }

    fn bounds(&self, cx: &Cx<Self>) -> Bounds<Pixels> {
        self.screen(cx.scale_factor())
    }

    /// A frame `w` × `h` pixels placed in the middle of the screen, as
    /// large as fits.
    fn place(&mut self, (w, h): (u32, u32), cx: &mut Cx<Self>) {
        let bounds = self.bounds(cx);
        let k = self.image.width() as f32 / f32::from(bounds.size.width).max(1.0);
        let (mut pw, mut ph) = (w as f32 / k, h as f32 / k);
        let fit = (f32::from(bounds.size.width) / pw).min(f32::from(bounds.size.height) / ph).min(1.0);
        (pw, ph) = (pw * fit, ph * fit);
        self.placed = Some(Bounds::centered_at(bounds.center(), size(px(pw), px(ph))));
        self.drag = None;
        self.shared.tell(Told::Chosen(self.index));
        cx.invalidate();
    }

    /// The frame placed, taken.
    fn take_placed(&mut self, cx: &mut Cx<Self>) {
        let Some(r) = self.placed.take() else { return };
        let cut = self.cut(self.bounds(cx), r.origin, r.bottom_right());
        self.shared.finish(cut);
    }

    /// The rectangle of the screen's picture under `a`–`b` (points in the
    /// window, `bounds`), in the picture's pixels.
    fn cut(&self, bounds: Bounds<Pixels>, a: Point<Pixels>, b: Point<Pixels>) -> Option<Cut> {
        let s = Bounds::from_corners(point(a.x.min(b.x), a.y.min(b.y)), point(a.x.max(b.x), a.y.max(b.y))).intersect(&bounds);
        if f32::from(s.size.width) < MIN_DRAG || f32::from(s.size.height) < MIN_DRAG {
            return None;
        }
        let k = self.image.width() as f32 / f32::from(bounds.size.width);
        let (iw, ih) = (self.image.width(), self.image.height());
        let x = ((f32::from(s.left() - bounds.left()) * k).round() as u32).min(iw - 1);
        let y = ((f32::from(s.top() - bounds.top()) * k).round() as u32).min(ih - 1);
        let w = ((f32::from(s.size.width) * k).round() as u32).clamp(1, iw - x);
        let h = ((f32::from(s.size.height) * k).round() as u32).clamp(1, ih - y);
        Some((self.image.clone(), [x, y, w, h], self.output.clone()))
    }

    fn pressed(&mut self, e: &MouseDownEvent, cx: &mut Cx<Self>) {
        // (The bar's own buttons take presses on it.)
        if self.bar.get().contains(&e.position) {
            return;
        }
        // In a frame placed: moving it (a click takes it).
        if let Some(r) = self.placed
            && r.contains(&e.position)
            && self.shared.pick.is_none()
        {
            self.moving = Some(e.position - r.origin);
            return;
        }
        self.placed = None;
        self.drag = Some((e.position, e.position));
        self.shared.tell(Told::Chosen(self.index));
        cx.invalidate();
    }

    fn moved(&mut self, e: &MouseMoveEvent, cx: &mut Cx<Self>) {
        let b = self.bounds(cx);
        self.pointer = e.position;
        let at = point(e.position.x.clamp(b.left(), b.right()), e.position.y.clamp(b.top(), b.bottom()));
        if let (Some(off), Some(r)) = (self.moving, self.placed.as_mut()) {
            // Moved, kept on its screen.
            let o = at - off;
            let x = o.x.clamp(b.left(), (b.right() - r.size.width).max(b.left()));
            let y = o.y.clamp(b.top(), (b.bottom() - r.size.height).max(b.top()));
            r.origin = point(x, y);
        } else if let Some((a, end)) = &mut self.drag {
            // The shape chosen kept (⇧: square, if free).
            let ratio = SHAPES[self.shape].1.or(e.modifiers.shift.then_some(1.0));
            *end = match ratio {
                Some(r) => {
                    let z = keep_shape(*a, at, r);
                    point(z.x.clamp(b.left(), b.right()), z.y.clamp(b.top(), b.bottom()))
                }
                None => at,
            };
        } else if self.shared.pick.is_none() {
            // (Nothing that's shown follows the pointer.)
            return;
        }
        cx.invalidate();
    }

    fn released(&mut self, e: &MouseUpEvent, cx: &mut Cx<Self>) {
        let b = self.bounds(cx);
        if let Some(pick) = self.shared.pick.clone() {
            if self.hovered && b.contains(&e.position) {
                let k = self.image.width() as f32 / f32::from(b.size.width);
                let (x, y) = ((f32::from(e.position.x - b.left()) * k).floor() as i64, (f32::from(e.position.y - b.top()) * k).floor() as i64);
                *pick.lock().unwrap_or_else(|e| e.into_inner()) = Some(pixel(&self.image, x, y));
            }
            return self.shared.finish(None);
        }
        // A frame placed: moved, or (a click in it) taken.
        if let Some(off) = self.moving.take() {
            let still = self.placed.is_some_and(|r| {
                let d = e.position - (r.origin + off);
                f32::from(d.x).abs() < MIN_DRAG && f32::from(d.y).abs() < MIN_DRAG
            });
            if still {
                self.take_placed(cx);
            }
            return;
        }
        let Some((a, z)) = self.drag.take() else { return };
        let cut = self.cut(b, a, z);
        self.shared.finish(cut);
    }

    fn key(&mut self, e: &KeyDownEvent, cx: &mut Cx<Self>) {
        let key = e.keystroke.key.as_str();
        let picking = self.shared.pick.is_some();
        match key {
            "escape" => self.shared.finish(None),
            "enter" if self.placed.is_some() => self.take_placed(cx),
            "1" | "2" | "3" | "4" if !picking => self.shared.tell(Told::Shape(key.parse::<usize>().unwrap_or(1) - 1)),
            "7" | "8" if !picking => self.place(SIZES[if key == "7" { 0 } else { 1 }], cx),
            _ => {}
        }
    }

    /// The screen's picture, and over it the loupe (picking a colour) or
    /// everything dimmed but the selection.
    fn paint(&self, g: &mut Painter, _: Bounds<Pixels>) {
        let b = self.screen(g.scale_factor());
        let _ = g.paint_image(b, Corners::default(), self.drawn.clone(), 0, false);
        let p = theme::palette();
        if self.shared.pick.is_some() {
            if self.hovered && b.contains(&self.pointer) {
                loupe(g, b, &self.image, self.pointer, &p);
            }
            return;
        }
        // Dim everything but the selection (being dragged, or a frame
        // placed).
        let dim = style::c(p.scrim);
        let Some((a, z)) = self.drag.or_else(|| self.placed.map(|r| (r.origin, r.bottom_right()))) else { return style::fill_rect(g, b, dim) };
        let s = style::snap(g, Bounds::from_corners(point(a.x.min(z.x), a.y.min(z.y)), point(a.x.max(z.x), a.y.max(z.y))));
        for r in [
            Bounds::from_corners(b.origin, point(b.right(), s.top())),
            Bounds::from_corners(point(b.left(), s.bottom()), b.bottom_right()),
            Bounds::from_corners(point(b.left(), s.top()), point(s.left(), s.bottom())),
            Bounds::from_corners(point(s.right(), s.top()), point(b.right(), s.bottom())),
        ] {
            style::fill_rect(g, r, dim);
        }
        // A line just outside it.
        let w = style::hairline(g);
        style::frame_rect(g, Bounds::from_corners(point(s.left() - w, s.top() - w), point(s.right() + w, s.bottom() + w)), style::c(p.desktop_text));
        // Its size in pixels, under the corner being dragged.
        let k = self.image.width() as f32 / f32::from(b.size.width);
        let text = format!("{} × {}", (f32::from(s.size.width) * k).round(), (f32::from(s.size.height) * k).round());
        let font = style::ui_font(FontWeight::NORMAL);
        let tw = style::text_width(text.clone(), style::PLAIN_SIZE, font.clone());
        let (lw, lh) = (tw + px(8.0), px(style::PLAIN_SIZE + 6.0));
        let mut at = point(s.right() + px(6.0), s.bottom() + px(6.0));
        at.x = at.x.min(b.right() - lw);
        at.y = at.y.min(b.bottom() - lh);
        let bg = style::snap(g, Bounds { origin: at, size: size(lw, lh) });
        g.paint_quad(olo_kit::fill(bg, style::c(p.tooltip_background)).corner_radii(px(3.0)));
        g.paint_quad(olo_kit::outline(bg, style::c(p.control_border), olo_kit::BorderStyle::Solid).corner_radii(px(3.0)));
        style::label(g, bg.left() + px(4.0), bg.center().y, style::Align::Left, text, style::PLAIN_SIZE, font, style::c(p.tooltip_text));
    }
}

/// The pixel of `image` at (x, y), clamped to it.
fn pixel(image: &image::RgbaImage, x: i64, y: i64) -> [u8; 3] {
    let (x, y) = (x.clamp(0, image.width() as i64 - 1) as u32, y.clamp(0, image.height() as i64 - 1) as u32);
    let p = image.get_pixel(x, y).0;
    [p[0], p[1], p[2]]
}

fn rgb_of([r, g, b]: [u8; 3]) -> Hsla {
    rgb((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)).into()
}

impl View for Overlay {
    type Message = Told;

    fn message(&mut self, told: Told, cx: &mut Cx<Self>) {
        match told {
            Told::Shape(shape) => self.shape = shape,
            Told::Chosen(index) if index != self.index => {
                self.placed = None;
                self.drag = None;
                self.moving = None;
            }
            Told::Chosen(_) => return,
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let screen = cx.canvas(|o: &Overlay, g, b| o.paint(g, b)).size_full();
        let root = div()
            .size_full()
            .relative()
            .cursor(CursorStyle::Crosshair)
            .child(screen)
            .on_hover(cx.listener(|o: &mut Overlay, hovered: &bool, cx| {
                o.hovered = *hovered;
                // (Where the pointer is, moved or not since the overlay
                // came: it may be there already.)
                o.pointer = cx.mouse_position();
                if o.shared.pick.is_some() {
                    cx.invalidate();
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|o: &mut Overlay, e: &MouseDownEvent, cx| o.pressed(e, cx)))
            .on_window_mouse_move(cx.listener(|o: &mut Overlay, e: &MouseMoveEvent, cx| o.moved(e, cx)))
            .on_window_mouse_up(cx.listener(|o: &mut Overlay, e: &MouseUpEvent, cx| {
                if e.button == MouseButton::Left {
                    o.released(e, cx)
                }
            }))
            .on_window_key_down(cx.listener(|o: &mut Overlay, e: &KeyDownEvent, cx| o.key(e, cx)));
        if self.shared.pick.is_some() {
            return root;
        }
        // The bar: shapes and sizes.
        let mut bar = div().flex().items_center().gap(px(m::TIGHT)).p(px(m::TIGHT)).rounded(px(m::CARD_RADIUS)).bg(style::c(theme::palette().panel_background));
        for (k, (name, _)) in SHAPES.iter().enumerate() {
            bar = bar.child(Button::new(*name).id(format!("shape-{k}")).small().pushed(self.shape == k).on_click(cx.listener(move |o: &mut Overlay, _: &ClickEvent, _| o.shared.tell(Told::Shape(k)))));
        }
        bar = bar.child(div().w(px(m::SPACING)));
        for (w, h) in SIZES {
            bar = bar.child(Button::new(format!("{w} × {h}")).small().on_click(cx.listener(move |o: &mut Overlay, _: &ClickEvent, cx| o.place((w, h), cx))));
        }
        // (Where it is: presses there aren't drags.)
        let at = self.bar.clone();
        let mark = canvas(move |_, b| at.set(b)).absolute().left(px(0.0)).top(px(0.0)).size_full();
        root.child(div().absolute().top(px(m::SPACING)).left(px(0.0)).right(px(0.0)).flex().justify_center().child(bar.relative().child(mark)))
    }
}

/// Picking a colour: the pixels round the pointer magnified below right of
/// it (above or left where they'd leave the screen), the one under it
/// outlined, and its swatch and hex under them, on device pixels.
fn loupe(g: &mut Painter, b: Bounds<Pixels>, image: &image::RgbaImage, p: Point<Pixels>, pal: &olo_kit::palette::Palette) {
    let k = image.width() as f32 / f32::from(b.size.width);
    let (cx_px, cy_px) = ((f32::from(p.x - b.left()) * k).floor() as i64, (f32::from(p.y - b.top()) * k).floor() as i64);
    let picked = pixel(image, cx_px, cy_px);
    let side = LOUPE_PIXELS as f32 * LOUPE_CELL;
    let label_h = 22.0;
    let mut origin = point(f32::from(p.x) + LOUPE_GAP, f32::from(p.y) + LOUPE_GAP);
    if origin.x + side > f32::from(b.right()) {
        origin.x = f32::from(p.x) - LOUPE_GAP - side;
    }
    if origin.y + side + label_h > f32::from(b.bottom()) {
        origin.y = f32::from(p.y) - LOUPE_GAP - side - label_h;
    }
    let origin = point(f32::from(style::snap_coord(g, px(origin.x))), f32::from(style::snap_coord(g, px(origin.y))));
    let frame = Bounds { origin: point(px(origin.x), px(origin.y)), size: size(px(side), px(side + label_h)) };
    let w = style::hairline(g);
    style::fill_rect(g, Bounds::from_corners(point(frame.left() - w, frame.top() - w), point(frame.right() + w, frame.bottom() + w)), style::c(pal.control_border));
    let half = LOUPE_PIXELS / 2;
    for j in 0..LOUPE_PIXELS {
        for i in 0..LOUPE_PIXELS {
            let cell = Bounds::from_corners(
                point(px(origin.x + i as f32 * LOUPE_CELL), px(origin.y + j as f32 * LOUPE_CELL)),
                point(px(origin.x + (i + 1) as f32 * LOUPE_CELL), px(origin.y + (j + 1) as f32 * LOUPE_CELL)),
            );
            style::fill_rect(g, cell, rgb_of(pixel(image, cx_px + i - half, cy_px + j - half)));
        }
    }
    // The pixel picked: outlined, light on dark and dark on light.
    let centre = Bounds { origin: point(px(origin.x + half as f32 * LOUPE_CELL), px(origin.y + half as f32 * LOUPE_CELL)), size: size(px(LOUPE_CELL), px(LOUPE_CELL)) };
    let luma = 0.299 * picked[0] as f32 + 0.587 * picked[1] as f32 + 0.114 * picked[2] as f32;
    style::frame_rect(g, centre, if luma > 128.0 { olo_kit::black() } else { olo_kit::white() });
    // Its swatch and hex, under the pixels.
    let bar = Bounds::from_corners(point(frame.left(), frame.top() + px(side)), frame.bottom_right());
    style::fill_rect(g, bar, style::c(pal.tooltip_background));
    let swatch = style::snap_centred(g, point(bar.left() + px(12.0), bar.center().y), 12.0, 12.0);
    style::fill_rect(g, swatch, rgb_of(picked));
    style::frame_rect(g, swatch, style::c(pal.control_border));
    let hex = format!("#{:02X}{:02X}{:02X}", picked[0], picked[1], picked[2]);
    let mono = Font { family: style::MONO_FONT.into(), ..style::ui_font(FontWeight::NORMAL) };
    style::label(g, swatch.right() + px(6.0), bar.center().y, style::Align::Left, hex, style::PLAIN_SIZE, mono, style::c(pal.tooltip_text));
}

/// Freezes every output and lets the person drag out a part, or pick a
/// colour (`pick`): runs until they're done.
fn freeze(cut: Arc<Mutex<Option<Cut>>>, pick: Option<Arc<Mutex<Option<[u8; 3]>>>>) {
    olo_kit::run(APP_ID, move |app| {
        let mut outputs: Vec<Option<String>> = olo_kit::display_names().into_iter().flatten().map(Some).collect();
        if outputs.is_empty() {
            outputs.push(None);
        }
        // With no window until the pictures are in.
        olo_kit::stay_resident();
        let shared = Arc::new(Shared { cut, pick, overlays: Mutex::new(Vec::new()) });
        let mut waiting = outputs.len();
        let mut frozen = Vec::new();
        olo_kit::message_handler(app, move |m, _| {
            let Ok(m) = m.downcast::<Frozen>() else { return };
            let Frozen(output, image) = *m;
            waiting = waiting.saturating_sub(1);
            if let Some((image, drawn)) = image {
                frozen.push((output, image, drawn));
            }
            if waiting > 0 {
                return;
            }
            if frozen.is_empty() {
                eprintln!("olo-screenshot: couldn't capture the screen");
                return olo_kit::quit();
            }
            // Now cover each screen with its picture.
            for (index, (output, image, drawn)) in std::mem::take(&mut frozen).into_iter().enumerate() {
                let on = output.clone();
                let options = WindowOptions::new("Screenshot", 1.0, 1.0).layer("screenshot-region", LayerShellLayer::Overlay, (true, true, true, true)).layer_options(|l| {
                    l.exclusive_zone = -1;
                    l.keyboard = LayerKeyboard::Exclusive;
                    l.output = on;
                });
                let shared = shared.clone();
                olo_kit::open_window(options, move |cx| {
                    shared.overlays.lock().unwrap_or_else(|e| e.into_inner()).push(cx.messenger());
                    Overlay { shared, index, output, image: Arc::new(image), drawn, drag: None, shape: 0, placed: None, moving: None, bar: Rc::default(), hovered: false, pointer: cx.mouse_position() }
                });
            }
        });
        for output in outputs {
            let proxy = olo_kit::proxy();
            std::thread::spawn(move || {
                let image = capture::take_output(output.clone()).map_err(|e| eprintln!("olo-screenshot: {e}")).ok().and_then(|image| drawn(&image).map(|d| (image, d)));
                proxy.send(Frozen(output, image));
            });
        }
    });
}

/// Run the region picker; save what was chosen. Returns the file, or None
/// if cancelled.
pub fn run() -> Result<Option<PathBuf>, String> {
    let cut: Arc<Mutex<Option<Cut>>> = Arc::default();
    freeze(cut.clone(), None);
    let Some((image, [x, y, w, h], _)) = cut.lock().unwrap_or_else(|e| e.into_inner()).take() else { return Ok(None) };
    let settings = Settings::load();
    let crop = image::imageops::crop_imm(image.as_ref(), x, y, w, h).to_image();
    let png = capture::scratch_path();
    crop.save_with_format(&png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    let folder = settings.folder();
    let path = folder.join(capture::file_name(&capture::next_name(&folder), settings.format));
    let r = capture::save(&Shot { png: png.clone(), image: Arc::new(crop) }, &path, settings.format);
    let _ = std::fs::remove_file(&png);
    r.map(|()| Some(path))
}

/// Drag out the part of the screen to record (`--record --region`): the
/// output it's on and the rectangle in its device pixels, or None if
/// cancelled.
pub fn pick_region() -> Option<(Option<String>, [u32; 4])> {
    let cut: Arc<Mutex<Option<Cut>>> = Arc::default();
    freeze(cut.clone(), None);
    let (_, rect, output) = cut.lock().unwrap_or_else(|e| e.into_inner()).take()?;
    Some((output, rect))
}

/// Pick a colour from the screen (`--pick-color`): the one clicked, or
/// None if cancelled.
pub fn pick_color() -> Result<Option<[u8; 3]>, String> {
    let picked: Arc<Mutex<Option<[u8; 3]>>> = Arc::default();
    freeze(Arc::default(), Some(picked.clone()));
    let colour = *picked.lock().unwrap_or_else(|e| e.into_inner());
    Ok(colour)
}
