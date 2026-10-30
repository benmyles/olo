//! `--region` (⌘⇧4): the screen freezes, dims, and you drag out the part
//! you want; it is saved like a silent screenshot. Escape (or a click
//! without a drag) cancels.
//!
//! Every output is captured first, then covered by an overlay showing its
//! picture, so what you select is exactly what you saw.
//!
//! `--pick-color` (the Screenshot portal's PickColor) freezes the screens
//! the same way, undimmed, with a loupe beside the pointer: the pixels
//! around it magnified, the one under it outlined, its colour as a
//! swatch and in hex. A click picks it; Escape cancels.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use olo_kit::{
    App, Cx, Keyboard, Layer, LayerOptions, WindowId, WindowKind, WindowOptions,
    egui::{self, Color32, ColorImage, Pos2, Rect, Stroke, TextureHandle, TextureOptions, pos2, vec2},
    style,
};

use crate::capture::{self, Settings, Shot};

const APP_ID: &str = "olo.screenshot";

/// egui's largest texture side; pictures are drawn in tiles no bigger.
const TILE: u32 = 2048;
/// A drag shorter than this (points) is a click: cancel.
const MIN_DRAG: f32 = 4.0;
/// The loupe: this many pixels a side, each this many points, this far
/// from the pointer.
const LOUPE_PIXELS: i64 = 9;
const LOUPE_CELL: f32 = 10.0;
const LOUPE_GAP: f32 = 20.0;

enum Msg {
    /// An output's picture, or None if it couldn't be taken.
    Frozen(Option<String>, Option<image::RgbaImage>),
}

struct Screen {
    image: Arc<image::RgbaImage>,
    tiles: Vec<(TextureHandle, Rect)>,
}

struct Picker {
    /// Outputs still being captured.
    waiting: usize,
    frozen: Vec<(Option<String>, image::RgbaImage)>,
    screens: HashMap<WindowId, Screen>,
    /// Drag start and current point, on one window.
    drag: Option<(WindowId, Pos2, Pos2)>,
    cut: Arc<std::sync::Mutex<Option<Cut>>>,
    /// Picking a colour (`--pick-color`) rather than a region: the one
    /// picked.
    pick: Option<Arc<std::sync::Mutex<Option<[u8; 3]>>>>,
}

/// Split `image` into textures of at most `TILE` pixels, each with its
/// place in UV space (0..1).
fn tiles(ctx: &egui::Context, image: &image::RgbaImage) -> Vec<(TextureHandle, Rect)> {
    let (w, h) = (image.width(), image.height());
    let mut out = Vec::new();
    for ty in (0..h).step_by(TILE as usize) {
        for tx in (0..w).step_by(TILE as usize) {
            let (tw, th) = (TILE.min(w - tx), TILE.min(h - ty));
            let sub = image::imageops::crop_imm(image, tx, ty, tw, th).to_image();
            let ci = ColorImage::from_rgba_unmultiplied([tw as usize, th as usize], sub.as_raw());
            let tex = ctx.load_texture(format!("frozen-{tx}-{ty}"), ci, TextureOptions::LINEAR);
            let uv = Rect::from_min_max(
                pos2(tx as f32 / w as f32, ty as f32 / h as f32),
                pos2((tx + tw) as f32 / w as f32, (ty + th) as f32 / h as f32),
            );
            out.push((tex, uv));
        }
    }
    out
}

/// The part chosen: the output's picture and a rectangle in its pixels.
type Cut = (Arc<image::RgbaImage>, [u32; 4]);

impl Picker {
    /// Close the overlays; the cut (if any) is saved once the kit returns.
    fn finish(&mut self, cx: &mut Cx, cut: Option<Cut>) {
        *self.cut.lock().unwrap() = cut;
        let ids: Vec<WindowId> = self.screens.keys().copied().collect();
        for id in ids {
            cx.close_window(id);
        }
        cx.quit();
    }
}

impl App for Picker {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui) {
        let Some(screen) = self.screens.get_mut(&window) else { return };
        let ctx = ui.ctx().clone();
        let rect = ui.max_rect();
        if screen.tiles.is_empty() {
            screen.tiles = tiles(&ctx, &screen.image);
        }
        let painter = ui.painter();
        for (tex, uv) in &screen.tiles {
            let r = Rect::from_min_max(rect.lerp_inside(uv.min.to_vec2()), rect.lerp_inside(uv.max.to_vec2()));
            painter.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
        if self.pick.is_some() {
            return self.pick_ui(cx, window, ui, rect);
        }

        let (pressed, down, released, pos, escape) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.pointer.interact_pos().or(i.pointer.hover_pos()),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if escape {
            self.finish(cx, None);
            return;
        }
        if pressed && let Some(p) = pos {
            self.drag = Some((window, p, p));
        }
        if let Some((w, _, end)) = &mut self.drag
            && *w == window
            && down
            && let Some(p) = pos
        {
            *end = p.clamp(rect.min, rect.max);
        }

        // Dim everything but the selection.
        let p = style::palette(painter.ctx());
        let dim = style::c(p.scrim);
        let sel = self.drag.filter(|(w, _, _)| *w == window).map(|(_, a, b)| Rect::from_two_pos(a, b));
        match sel {
            Some(s) => {
                for r in [
                    Rect::from_min_max(rect.min, pos2(rect.right(), s.top())),
                    Rect::from_min_max(pos2(rect.left(), s.bottom()), rect.max),
                    Rect::from_min_max(pos2(rect.left(), s.top()), pos2(s.left(), s.bottom())),
                    Rect::from_min_max(pos2(s.right(), s.top()), pos2(rect.right(), s.bottom())),
                ] {
                    painter.rect_filled(r, 0.0, dim);
                }
                painter.rect_stroke(s, 0.0, Stroke::new(1.0, style::c(p.desktop_text)), egui::StrokeKind::Outside);
                // Its size in pixels, under the corner being dragged.
                let k = screen.image.width() as f32 / rect.width();
                let text = format!("{} × {}", (s.width() * k).round(), (s.height() * k).round());
                let at = s.right_bottom() + vec2(6.0, 6.0);
                let g = painter.layout_no_wrap(text, style::plain_font(), style::c(p.tooltip_text));
                let bg = Rect::from_min_size(at, g.size() + vec2(8.0, 4.0));
                let bg = bg.translate(vec2((rect.right() - bg.right()).min(0.0), (rect.bottom() - bg.bottom()).min(0.0)));
                painter.rect_filled(bg, 3.0, style::c(p.tooltip_background));
                painter.rect_stroke(bg, 3.0, Stroke::new(1.0, style::c(p.control_border)), egui::StrokeKind::Inside);
                painter.galley(bg.min + vec2(4.0, 2.0), g, style::c(p.tooltip_text));
            }
            None => {
                painter.rect_filled(rect, 0.0, dim);
            }
        }

        if released && let Some((w, a, b)) = self.drag.take() && w == window {
            let s = Rect::from_two_pos(a, b).intersect(rect);
            if s.width() < MIN_DRAG || s.height() < MIN_DRAG {
                self.finish(cx, None);
                return;
            }
            let k = screen.image.width() as f32 / rect.width();
            let (iw, ih) = (screen.image.width(), screen.image.height());
            let x = (((s.left() - rect.left()) * k).round() as u32).min(iw - 1);
            let y = (((s.top() - rect.top()) * k).round() as u32).min(ih - 1);
            let w = ((s.width() * k).round() as u32).clamp(1, iw - x);
            let h = ((s.height() * k).round() as u32).clamp(1, ih - y);
            let image = screen.image.clone();
            self.finish(cx, Some((image, [x, y, w, h])));
            return;
        }
        ctx.request_repaint();
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        self.on_message(cx, message);
    }
}

impl Picker {
    /// Picking a colour: the loupe by the pointer; a click picks.
    fn pick_ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui, rect: Rect) {
        let Some(screen) = self.screens.get(&window) else { return };
        let (clicked, pos, escape) = ui.input(|i| (i.pointer.primary_released(), i.pointer.hover_pos().or(i.pointer.interact_pos()), i.key_pressed(egui::Key::Escape)));
        if escape {
            return self.finish(cx, None);
        }
        let Some(p) = pos.filter(|p| rect.contains(*p)) else { return };
        let image = &screen.image;
        let k = image.width() as f32 / rect.width();
        let (cx_px, cy_px) = (((p.x - rect.left()) * k).floor() as i64, ((p.y - rect.top()) * k).floor() as i64);
        let at = |x: i64, y: i64| {
            let (x, y) = (x.clamp(0, image.width() as i64 - 1) as u32, y.clamp(0, image.height() as i64 - 1) as u32);
            let px = image.get_pixel(x, y).0;
            [px[0], px[1], px[2]]
        };
        let picked = at(cx_px, cy_px);
        if clicked {
            if let Some(pick) = &self.pick {
                *pick.lock().unwrap() = Some(picked);
            }
            return self.finish(cx, None);
        }
        // The loupe, below right of the pointer (above or left where it
        // would leave the screen), on device pixels.
        let pal = style::palette(ui.ctx());
        let ppp = ui.ctx().pixels_per_point();
        let snap = |v: f32| (v * ppp).round() / ppp;
        let side = LOUPE_PIXELS as f32 * LOUPE_CELL;
        let label_h = 22.0;
        let mut origin = p + vec2(LOUPE_GAP, LOUPE_GAP);
        if origin.x + side > rect.right() {
            origin.x = p.x - LOUPE_GAP - side;
        }
        if origin.y + side + label_h > rect.bottom() {
            origin.y = p.y - LOUPE_GAP - side - label_h;
        }
        let origin = pos2(snap(origin.x), snap(origin.y));
        let painter = ui.painter();
        let frame = Rect::from_min_size(origin, vec2(side, side + label_h));
        painter.rect_filled(frame.expand(1.0), 0.0, style::c(pal.control_border));
        let half = LOUPE_PIXELS / 2;
        for j in 0..LOUPE_PIXELS {
            for i in 0..LOUPE_PIXELS {
                let [r, g, b] = at(cx_px + i - half, cy_px + j - half);
                let cell = Rect::from_min_max(
                    pos2(snap(origin.x + i as f32 * LOUPE_CELL), snap(origin.y + j as f32 * LOUPE_CELL)),
                    pos2(snap(origin.x + (i + 1) as f32 * LOUPE_CELL), snap(origin.y + (j + 1) as f32 * LOUPE_CELL)),
                );
                painter.rect_filled(cell, 0.0, Color32::from_rgb(r, g, b));
            }
        }
        // The pixel picked: outlined, light on dark and dark on light.
        let centre = Rect::from_min_size(pos2(snap(origin.x + half as f32 * LOUPE_CELL), snap(origin.y + half as f32 * LOUPE_CELL)), vec2(LOUPE_CELL, LOUPE_CELL));
        let luma = 0.299 * picked[0] as f32 + 0.587 * picked[1] as f32 + 0.114 * picked[2] as f32;
        let ring = if luma > 128.0 { Color32::BLACK } else { Color32::WHITE };
        painter.rect_stroke(centre, 0.0, Stroke::new(1.0, ring), egui::StrokeKind::Inside);
        // Its swatch and hex, under the pixels.
        let bar = Rect::from_min_max(pos2(origin.x, origin.y + side), frame.max);
        painter.rect_filled(bar, 0.0, style::c(pal.tooltip_background));
        let swatch = Rect::from_center_size(pos2(bar.left() + 12.0, bar.center().y), vec2(12.0, 12.0));
        let swatch = Rect::from_min_size(pos2(snap(swatch.left()), snap(swatch.top())), swatch.size());
        painter.rect_filled(swatch, 0.0, Color32::from_rgb(picked[0], picked[1], picked[2]));
        painter.rect_stroke(swatch, 0.0, Stroke::new(1.0, style::c(pal.control_border)), egui::StrokeKind::Inside);
        let hex = format!("#{:02X}{:02X}{:02X}", picked[0], picked[1], picked[2]);
        style::label(painter, swatch.right() + 6.0, bar.center().y, egui::Align::Min, &hex, egui::FontId::monospace(style::PLAIN_SIZE), style::c(pal.tooltip_text));
        ui.ctx().request_repaint();
    }

    fn on_message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Frozen(output, image) => {
                self.waiting = self.waiting.saturating_sub(1);
                if let Some(image) = image {
                    self.frozen.push((output, image));
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
                for (output, image) in std::mem::take(&mut self.frozen) {
                    let id = cx.open_window(WindowOptions {
                        title: "Screenshot".into(),
                        size: (1, 1),
                        kind: WindowKind::Layer(LayerOptions {
                            layer: Layer::Overlay,
                            anchor: (true, true, true, true),
                            exclusive_zone: -1,
                            margin: (0, 0, 0, 0),
                            keyboard: Keyboard::Exclusive,
                            namespace: "screenshot-region".into(),
                            output,
                        }),
                        panel: false,
                        ..Default::default()
                    });
                    self.screens.insert(id, Screen { image: Arc::new(image), tiles: Vec::new() });
                }
            }
        }
    }
}

/// Run the region picker; save what was chosen. Returns the file, or None
/// if cancelled.
pub fn run() -> Result<Option<PathBuf>, String> {
    let cut: Arc<std::sync::Mutex<Option<Cut>>> = Arc::default();
    olo_kit::run(APP_ID, |cx| {
        let mut outputs: Vec<Option<String>> = cx.outputs().iter().cloned().map(Some).collect();
        if outputs.is_empty() {
            outputs.push(None);
        }
        let picker = Picker { waiting: outputs.len(), frozen: Vec::new(), screens: HashMap::new(), drag: None, cut: cut.clone(), pick: None };
        for output in outputs {
            let proxy = cx.proxy();
            std::thread::spawn(move || {
                let image = capture::take_output(output.clone()).map_err(|e| eprintln!("olo-screenshot: {e}")).ok();
                proxy.send(Msg::Frozen(output, image));
            });
        }
        picker
    });
    let Some((image, [x, y, w, h])) = cut.lock().unwrap().take() else { return Ok(None) };
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
    let picked: Arc<std::sync::Mutex<Option<[u8; 3]>>> = Arc::default();
    olo_kit::run(APP_ID, |cx| {
        let mut outputs: Vec<Option<String>> = cx.outputs().iter().cloned().map(Some).collect();
        if outputs.is_empty() {
            outputs.push(None);
        }
        let picker = Picker { waiting: outputs.len(), frozen: Vec::new(), screens: HashMap::new(), drag: None, cut: Arc::default(), pick: Some(picked.clone()) };
        for output in outputs {
            let proxy = cx.proxy();
            std::thread::spawn(move || {
                let image = capture::take_output(output.clone()).map_err(|e| eprintln!("olo-screenshot: {e}")).ok();
                proxy.send(Msg::Frozen(output, image));
            });
        }
        picker
    });
    let colour = *picked.lock().unwrap();
    Ok(colour)
}
