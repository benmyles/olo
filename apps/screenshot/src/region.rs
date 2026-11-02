//! `--region` (⌘⇧4): the screen freezes, dims, and you drag out the part
//! you want; it is saved like a silent screenshot. Escape (or a click
//! without a drag) cancels.
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
use std::{path::PathBuf, sync::Arc};

use olo_ui::{
    WindowOptions,
    gpui::{
        self, App, AppContext as _, Bounds, Context, Corners, CursorStyle, Entity, FocusHandle, Font, FontWeight, Hitbox, HitboxBehavior, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, LayerKeyboard, LayerShellLayer, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render, RenderImage, Styled as _, Window, canvas, div, point, px, rgb, size,
    },
    style, theme,
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

enum Msg {
    /// An output's picture (and it as GPUI draws it), or None if it
    /// couldn't be taken.
    Frozen(Option<String>, Option<(image::RgbaImage, Arc<RenderImage>)>),
}

/// The part chosen: the output's picture and a rectangle in its pixels.
type Cut = (Arc<image::RgbaImage>, [u32; 4]);

struct Screen {
    image: Arc<image::RgbaImage>,
    drawn: Arc<RenderImage>,
}

struct Picker {
    /// Outputs still being captured.
    waiting: usize,
    frozen: Vec<(Option<String>, image::RgbaImage, Arc<RenderImage>)>,
    screens: Vec<Screen>,
    /// The press and where the pointer is now, on one screen.
    drag: Option<(usize, Point<Pixels>, Point<Pixels>)>,
    cut: Arc<Mutex<Option<Cut>>>,
    /// Picking a colour (`--pick-color`) rather than a region: the one
    /// picked.
    pick: Option<Arc<Mutex<Option<[u8; 3]>>>>,
}

/// One output's overlay.
struct Overlay {
    picker: Entity<Picker>,
    index: usize,
    focus: FocusHandle,
}

/// `image` as GPUI draws it (BGRA), made on the worker that took it.
fn drawn(image: &image::RgbaImage) -> Option<Arc<RenderImage>> {
    let mut bgra = image.as_raw().clone();
    for p in bgra.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(image.width(), image.height(), bgra)?;
    Some(Arc::new(RenderImage::new([image::Frame::new(buffer)])))
}

impl Picker {
    /// The overlays go; the cut (if any) is saved once the app returns.
    fn finish(&mut self, cut: Option<Cut>, cx: &mut Context<Self>) {
        *self.cut.lock().unwrap_or_else(|e| e.into_inner()) = cut;
        cx.quit();
    }

    fn message(&mut self, msg: Msg, cx: &mut Context<Self>) {
        let Msg::Frozen(output, image) = msg;
        self.waiting = self.waiting.saturating_sub(1);
        if let Some((image, drawn)) = image {
            self.frozen.push((output, image, drawn));
        }
        if self.waiting > 0 {
            return;
        }
        if self.frozen.is_empty() {
            eprintln!("olo-screenshot: couldn't capture the screen");
            cx.quit();
            return;
        }
        // Now cover each screen with its picture.
        let me = cx.entity();
        for (output, image, drawn) in std::mem::take(&mut self.frozen) {
            let index = self.screens.len();
            self.screens.push(Screen { image: Arc::new(image), drawn });
            let picker = me.clone();
            cx.defer(move |cx| {
                let options = WindowOptions::new("Screenshot", 1.0, 1.0).layer("screenshot-region", LayerShellLayer::Overlay, (true, true, true, true)).layer_options(|l| {
                    l.exclusive_zone = -1;
                    l.keyboard = LayerKeyboard::Exclusive;
                    l.output = output;
                });
                olo_ui::open_window(cx, options, move |window, cx| {
                    let focus = cx.focus_handle();
                    window.focus(&focus);
                    cx.new(|cx| {
                        cx.observe(&picker, |_, _, cx| cx.notify()).detach();
                        Overlay { picker, index, focus }
                    })
                });
            });
        }
    }

    /// The rectangle of screen `index`'s picture under `a`–`b` (points in
    /// a window `bounds`), in the picture's pixels.
    fn cut(&self, index: usize, bounds: Bounds<Pixels>, a: Point<Pixels>, b: Point<Pixels>) -> Option<Cut> {
        let screen = self.screens.get(index)?;
        let s = Bounds::from_corners(point(a.x.min(b.x), a.y.min(b.y)), point(a.x.max(b.x), a.y.max(b.y))).intersect(&bounds);
        if f32::from(s.size.width) < MIN_DRAG || f32::from(s.size.height) < MIN_DRAG {
            return None;
        }
        let k = screen.image.width() as f32 / f32::from(bounds.size.width);
        let (iw, ih) = (screen.image.width(), screen.image.height());
        let x = ((f32::from(s.left() - bounds.left()) * k).round() as u32).min(iw - 1);
        let y = ((f32::from(s.top() - bounds.top()) * k).round() as u32).min(ih - 1);
        let w = ((f32::from(s.size.width) * k).round() as u32).clamp(1, iw - x);
        let h = ((f32::from(s.size.height) * k).round() as u32).clamp(1, ih - y);
        Some((screen.image.clone(), [x, y, w, h]))
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

impl Render for Overlay {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (picker, index) = (self.picker.clone(), self.index);
        let escape = picker.clone();
        let screen = canvas(
            move |b, window, _| window.insert_hitbox(b, HitboxBehavior::Normal),
            move |b, hitbox: Hitbox, window, cx| {
                window.set_cursor_style(CursorStyle::Crosshair, &hitbox);
                let state = picker.read(cx);
                let Some(screen) = state.screens.get(index) else { return };
                let (image, drawn) = (screen.image.clone(), screen.drawn.clone());
                let _ = window.paint_image(b, Corners::default(), drawn, 0, false);
                let p = theme::palette(cx);
                let picking = state.pick.is_some();
                if picking {
                    // Where the pointer is, moved or not since the
                    // overlay came (it may be there already).
                    let at = window.mouse_position();
                    if hitbox.is_hovered(window) && b.contains(&at) {
                        loupe(window, cx, b, &image, at, &p);
                    }
                } else {
                    // Dim everything but the selection.
                    let dim = style::c(p.scrim);
                    match state.drag.filter(|(i, _, _)| *i == index) {
                        Some((_, a, z)) => {
                            let s = style::snap(window, Bounds::from_corners(point(a.x.min(z.x), a.y.min(z.y)), point(a.x.max(z.x), a.y.max(z.y))));
                            for r in [
                                Bounds::from_corners(b.origin, point(b.right(), s.top())),
                                Bounds::from_corners(point(b.left(), s.bottom()), b.bottom_right()),
                                Bounds::from_corners(point(b.left(), s.top()), point(s.left(), s.bottom())),
                                Bounds::from_corners(point(s.right(), s.top()), point(b.right(), s.bottom())),
                            ] {
                                style::fill_rect(window, r, dim);
                            }
                            // A line just outside it.
                            let w = style::hairline(window);
                            style::frame_rect(window, Bounds::from_corners(point(s.left() - w, s.top() - w), point(s.right() + w, s.bottom() + w)), style::c(p.desktop_text));
                            // Its size in pixels, under the corner being
                            // dragged.
                            let k = image.width() as f32 / f32::from(b.size.width);
                            let text = format!("{} × {}", (f32::from(s.size.width) * k).round(), (f32::from(s.size.height) * k).round());
                            let font = style::ui_font(FontWeight::NORMAL);
                            let tw = style::text_width(window, text.clone(), style::PLAIN_SIZE, font.clone());
                            let (lw, lh) = (tw + px(8.0), px(style::PLAIN_SIZE + 6.0));
                            let mut at = point(s.right() + px(6.0), s.bottom() + px(6.0));
                            at.x = at.x.min(b.right() - lw);
                            at.y = at.y.min(b.bottom() - lh);
                            let bg = style::snap(window, Bounds { origin: at, size: size(lw, lh) });
                            window.paint_quad(gpui::fill(bg, style::c(p.tooltip_background)).corner_radii(px(3.0)));
                            window.paint_quad(gpui::outline(bg, style::c(p.control_border), gpui::BorderStyle::Solid).corner_radii(px(3.0)));
                            style::label(window, cx, bg.left() + px(4.0), bg.center().y, style::Align::Left, text, style::PLAIN_SIZE, font, style::c(p.tooltip_text));
                        }
                        None => style::fill_rect(window, b, dim),
                    }
                }

                let (m, hb) = (picker.clone(), hitbox.clone());
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if phase.bubble() && e.button == MouseButton::Left && hb.is_hovered(window) {
                        m.update(cx, |s, cx| {
                            s.drag = Some((index, e.position, e.position));
                            cx.notify();
                        });
                    }
                });
                let m = picker.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                    if !phase.capture() {
                        return;
                    }
                    m.update(cx, |s, cx| {
                        let at = point(e.position.x.clamp(b.left(), b.right()), e.position.y.clamp(b.top(), b.bottom()));
                        if let Some((i, _, end)) = &mut s.drag
                            && *i == index
                        {
                            *end = at;
                        }
                        cx.notify();
                    });
                });
                let m = picker.clone();
                window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                    if !phase.capture() || e.button != MouseButton::Left {
                        return;
                    }
                    m.update(cx, |s, cx| {
                        if let Some(pick) = s.pick.clone() {
                            if let Some(screen) = s.screens.get(index)
                                && b.contains(&e.position)
                            {
                                let k = screen.image.width() as f32 / f32::from(b.size.width);
                                let (x, y) = ((f32::from(e.position.x - b.left()) * k).floor() as i64, (f32::from(e.position.y - b.top()) * k).floor() as i64);
                                *pick.lock().unwrap_or_else(|e| e.into_inner()) = Some(pixel(&screen.image, x, y));
                            }
                            return s.finish(None, cx);
                        }
                        let Some((i, a, _)) = s.drag.take().filter(|(i, _, _)| *i == index) else { return };
                        let cut = s.cut(i, b, a, e.position);
                        s.finish(cut, cx);
                    });
                });
            },
        )
        .size_full();
        div().size_full().track_focus(&self.focus).child(screen).on_key_down(move |e: &KeyDownEvent, _, cx| {
            if e.keystroke.key == "escape" {
                escape.update(cx, |s, cx| s.finish(None, cx));
            }
        })
    }
}

/// Picking a colour: the pixels round the pointer magnified below right of
/// it (above or left where they'd leave the screen), the one under it
/// outlined, and its swatch and hex under them, on device pixels.
fn loupe(window: &mut Window, cx: &mut App, b: Bounds<Pixels>, image: &image::RgbaImage, p: Point<Pixels>, pal: &olo_ui::palette::Palette) {
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
    let snap = |v: f32| f32::from(style::snap_coord(window, px(v)));
    let origin = point(snap(origin.x), snap(origin.y));
    let frame = Bounds { origin: point(px(origin.x), px(origin.y)), size: size(px(side), px(side + label_h)) };
    let w = style::hairline(window);
    style::fill_rect(window, Bounds::from_corners(point(frame.left() - w, frame.top() - w), point(frame.right() + w, frame.bottom() + w)), style::c(pal.control_border));
    let half = LOUPE_PIXELS / 2;
    for j in 0..LOUPE_PIXELS {
        for i in 0..LOUPE_PIXELS {
            let cell = Bounds::from_corners(
                point(px(origin.x + i as f32 * LOUPE_CELL), px(origin.y + j as f32 * LOUPE_CELL)),
                point(px(origin.x + (i + 1) as f32 * LOUPE_CELL), px(origin.y + (j + 1) as f32 * LOUPE_CELL)),
            );
            style::fill_rect(window, cell, rgb_of(pixel(image, cx_px + i - half, cy_px + j - half)));
        }
    }
    // The pixel picked: outlined, light on dark and dark on light.
    let centre = Bounds { origin: point(px(origin.x + half as f32 * LOUPE_CELL), px(origin.y + half as f32 * LOUPE_CELL)), size: size(px(LOUPE_CELL), px(LOUPE_CELL)) };
    let luma = 0.299 * picked[0] as f32 + 0.587 * picked[1] as f32 + 0.114 * picked[2] as f32;
    style::frame_rect(window, centre, if luma > 128.0 { gpui::black() } else { gpui::white() });
    // Its swatch and hex, under the pixels.
    let bar = Bounds::from_corners(point(frame.left(), frame.top() + px(side)), frame.bottom_right());
    style::fill_rect(window, bar, style::c(pal.tooltip_background));
    let swatch = style::snap_centred(window, point(bar.left() + px(12.0), bar.center().y), 12.0, 12.0);
    style::fill_rect(window, swatch, rgb_of(picked));
    style::frame_rect(window, swatch, style::c(pal.control_border));
    let hex = format!("#{:02X}{:02X}{:02X}", picked[0], picked[1], picked[2]);
    let mono = Font { family: style::MONO_FONT.into(), ..style::ui_font(FontWeight::NORMAL) };
    style::label(window, cx, swatch.right() + px(6.0), bar.center().y, style::Align::Left, hex, style::PLAIN_SIZE, mono, style::c(pal.tooltip_text));
}

/// Freezes every output and lets the person drag out a part, or pick a
/// colour (`pick`): runs until they're done.
fn freeze(cut: Arc<Mutex<Option<Cut>>>, pick: Option<Arc<Mutex<Option<[u8; 3]>>>>) {
    olo_ui::run(APP_ID, move |cx| {
        let mut outputs: Vec<Option<String>> = cx.displays().iter().filter_map(|d| d.name()).map(Some).collect();
        if outputs.is_empty() {
            outputs.push(None);
        }
        // With no window until the pictures are in.
        olo_ui::stay_resident(cx);
        let picker = cx.new(|_| Picker { waiting: outputs.len(), frozen: Vec::new(), screens: Vec::new(), drag: None, cut, pick });
        let messages = picker.clone();
        olo_ui::message_handler(cx, move |m, cx| {
            if let Ok(m) = m.downcast::<Msg>() {
                messages.update(cx, |p, cx| p.message(*m, cx));
            }
        });
        for output in outputs {
            let proxy = olo_ui::proxy(cx);
            std::thread::spawn(move || {
                let image = capture::take_output(output.clone()).map_err(|e| eprintln!("olo-screenshot: {e}")).ok().and_then(|image| drawn(&image).map(|d| (image, d)));
                proxy.send(Msg::Frozen(output, image));
            });
        }
    });
}

/// Run the region picker; save what was chosen. Returns the file, or None
/// if cancelled.
pub fn run() -> Result<Option<PathBuf>, String> {
    let cut: Arc<Mutex<Option<Cut>>> = Arc::default();
    freeze(cut.clone(), None);
    let Some((image, [x, y, w, h])) = cut.lock().unwrap_or_else(|e| e.into_inner()).take() else { return Ok(None) };
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

/// Pick a colour from the screen (`--pick-color`): the one clicked, or
/// None if cancelled.
pub fn pick_color() -> Result<Option<[u8; 3]>, String> {
    let picked: Arc<Mutex<Option<[u8; 3]>>> = Arc::default();
    freeze(Arc::default(), Some(picked.clone()));
    let colour = *picked.lock().unwrap_or_else(|e| e.into_inner());
    Ok(colour)
}
