//! `--region` (⌘⇧4): the screen freezes, dims, and you drag out the part
//! you want; it is saved like a silent screenshot. Escape (or a click
//! without a drag) cancels.
//!
//! Every output is captured first, then covered by an overlay showing its
//! picture, so what you select is exactly what you saw.

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
        let dim = Color32::from_black_alpha(90);
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
                painter.rect_stroke(s, 0.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Outside);
                // Its size in pixels, under the corner being dragged.
                let k = screen.image.width() as f32 / rect.width();
                let text = format!("{} × {}", (s.width() * k).round(), (s.height() * k).round());
                let at = s.right_bottom() + vec2(6.0, 6.0);
                let g = painter.layout_no_wrap(text, style::plain_font(), Color32::WHITE);
                let bg = Rect::from_min_size(at, g.size() + vec2(8.0, 4.0));
                let bg = bg.translate(vec2((rect.right() - bg.right()).min(0.0), (rect.bottom() - bg.bottom()).min(0.0)));
                painter.rect_filled(bg, 3.0, Color32::from_black_alpha(160));
                painter.galley(bg.min + vec2(4.0, 2.0), g, Color32::WHITE);
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
        let picker = Picker { waiting: outputs.len(), frozen: Vec::new(), screens: HashMap::new(), drag: None, cut: cut.clone() };
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
