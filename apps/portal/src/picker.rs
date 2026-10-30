//! The picker: what an app may share, asked of the person when it starts
//! a screen share (the portal's Start). A kit dialog over the app's window
//! (its exported handle, `WindowOptions::foreign_parent`): who is asking,
//! the screens as live thumbnails to choose from (a `ThumbnailPicker`,
//! so agents read and choose them as radio buttons), and Cancel / Share.
//! The thumbnails come from a worker with a capture of each screen: it
//! asks for a frame about twice a second, app_server answers once that
//! screen has changed, and the frame is shrunk to the picture's size in
//! device pixels (letterboxed, drawn one texture pixel to a device pixel)
//! before it reaches the UI.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use olo_kit::{
    Cx, Proxy, WindowId, WindowOptions,
    egui::{self, vec2},
    metrics as m,
    style::{self, c},
    widgets::{self, Thumbnail, ThumbnailPicker},
};

use crate::capture::Capture;

/// What the picker was asked for (from the portal's Start).
pub struct Ask {
    /// The portal request it answers (its handle), so it can be cancelled.
    pub request: String,
    /// Who is asking ("" when the portal couldn't tell).
    pub app: String,
    /// The window it opens over (`wayland:HANDLE`, or "").
    pub parent: String,
    /// The answer: the screens chosen (by connector), or None if
    /// cancelled.
    pub reply: async_channel::Sender<Option<Vec<String>>>,
}

/// A screen to choose.
pub struct Screen {
    pub output: String,
    pub label: String,
    /// Its size in pixels, for the thumbnail's shape.
    pub size: (u32, u32),
}

/// A picture of a screen, shrunk for its thumbnail.
pub struct Thumb {
    pub window: WindowId,
    pub output: String,
    pub image: egui::ColorImage,
}

/// The pictures' height, logical; their width follows the first screen's
/// shape (others are letterboxed). Both multiples of 4, so they are whole
/// device pixels at every scale from 1× to 2× in quarters.
const PICTURE_HEIGHT: f32 = 108.0;

/// The pictures' size for `screens`: the first's shape, 192×108 for 16:9.
fn picture(screens: &[Screen]) -> egui::Vec2 {
    let aspect = screens.first().filter(|s| s.size.1 > 0).map_or(16.0 / 9.0, |s| s.size.0 as f32 / s.size.1 as f32);
    let width = ((PICTURE_HEIGHT * aspect / 4.0).round() * 4.0).clamp(144.0, 224.0);
    vec2(width, PICTURE_HEIGHT)
}
/// At most this many to a row.
const COLUMNS: usize = 3;
const ICON: &str = "display";

/// One picker on screen.
pub struct Picker {
    ask: Ask,
    screens: Vec<Screen>,
    textures: HashMap<String, egui::TextureHandle>,
    selected: Option<usize>,
    /// The thumbnails' size in device pixels, which the worker shrinks to.
    pixels: Arc<Mutex<(u32, u32)>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Picker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn message(app: &str) -> String {
    let who = if app.is_empty() { "An app" } else { app };
    format!("{who} wants to share your screen. Everything on the screen you choose is shown until you stop sharing.")
}

/// The column the text and thumbnails share, and the window's size, for
/// `screens` of pictures `picture` big.
fn layout(screens: usize, picture: egui::Vec2, app: &str) -> (f32, (u32, u32)) {
    let cell = picture.x + 4.0;
    let columns = screens.clamp(2, COLUMNS) as f32;
    let column = columns * cell + (columns - 1.0) * m::SPACING;
    let rows = screens.div_ceil(COLUMNS).max(1) as f32;
    let thumbs = rows * (picture.y + 4.0 + m::TIGHT + m::LABEL_ROW) + (rows - 1.0) * m::SPACING;
    let text = style::plain_text_height(&message(app), column);
    let height = 2.0 * m::WINDOW_INSET + m::LABEL_ROW + m::TIGHT + text + m::SPACING * 2.0 + thumbs + m::SPACING * 2.0 + m::CONTROL_HEIGHT;
    let width = widgets::ALERT_TEXT_X + column + m::WINDOW_INSET;
    (column, (width.ceil() as u32, height.ceil() as u32))
}

impl Picker {
    /// Open the picker for `ask`, choosing among `screens`.
    pub fn open(cx: &mut Cx, ask: Ask, screens: Vec<Screen>) -> (WindowId, Picker) {
        let pic = picture(&screens);
        let (_, (w, h)) = layout(screens.len(), pic, &ask.app);
        let mut options = WindowOptions { dialog: true, ..WindowOptions::new("Share Screen", w, h).min_size(w, h).max_size(w, h) };
        if !ask.parent.is_empty() {
            options = options.foreign_parent(ask.parent.clone());
        }
        let id = cx.open_window(options);
        cx.activate(id);
        let picker = Picker {
            ask,
            selected: (!screens.is_empty()).then_some(0),
            screens,
            textures: HashMap::new(),
            pixels: Arc::new(Mutex::new((pic.x as u32, pic.y as u32))),
            stop: Arc::new(AtomicBool::new(false)),
        };
        picker.watch(id, cx.proxy());
        (id, picker)
    }

    pub fn request(&self) -> &str {
        &self.ask.request
    }

    /// Answer the request (None: cancelled); the caller closes the window.
    pub fn answer(&self, chosen: Option<Vec<String>>) {
        let _ = self.ask.reply.try_send(chosen);
    }

    pub fn thumb(&mut self, ctx: &egui::Context, t: Thumb) {
        let texture = ctx.load_texture(format!("screen-{}", t.output), t.image, egui::TextureOptions::NEAREST);
        self.textures.insert(t.output, texture);
    }

    /// The worker: a capture of each screen, shrunk for its thumbnail as
    /// often as it changes, at most twice a second.
    fn watch(&self, window: WindowId, proxy: Proxy) {
        let outputs: Vec<(String, (u32, u32))> = self.screens.iter().map(|s| (s.output.clone(), s.size)).collect();
        let (pixels, stop) = (self.pixels.clone(), self.stop.clone());
        let _ = std::thread::Builder::new().name("picker-thumbnails".into()).spawn(move || {
            let mut captures: Vec<(String, Option<Capture>, Instant)> = outputs
                .iter()
                .map(|(o, _)| {
                    let mut c = Capture::output(Some(o), false).ok();
                    if let Some(cap) = c.as_mut()
                        && cap.request().is_err()
                    {
                        c = None;
                    }
                    (o.clone(), c, Instant::now())
                })
                .collect();
            let mut shown: HashMap<String, (u32, u32)> = HashMap::new();
            while !stop.load(Ordering::Relaxed) {
                let want = *pixels.lock().unwrap();
                for (output, capture, next) in captures.iter_mut() {
                    let Some(cap) = capture.as_mut() else { continue };
                    match cap.poll() {
                        Ok(Some(Ok(()))) => {
                            if let Some(image) = shrink(cap, want) {
                                shown.insert(output.clone(), want);
                                proxy.send(Thumb { window, output: output.clone(), image });
                            }
                            *next = Instant::now() + Duration::from_millis(500);
                        }
                        Ok(Some(Err(_))) => *next = Instant::now() + Duration::from_millis(500),
                        Ok(None) => {}
                        Err(_) => *capture = None,
                    }
                    // The scale changed: this size again, from the last frame.
                    if shown.get(output).is_some_and(|s| *s != want)
                        && let Some(cap) = capture.as_ref()
                        && let Some(image) = shrink(cap, want)
                    {
                        shown.insert(output.clone(), want);
                        proxy.send(Thumb { window, output: output.clone(), image });
                    }
                    if let Some(cap) = capture.as_mut()
                        && !cap.waiting()
                        && Instant::now() >= *next
                    {
                        let _ = cap.request();
                    }
                }
                std::thread::sleep(Duration::from_millis(40));
            }
        });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Option<Vec<String>>> {
        let ppp = ui.ctx().pixels_per_point();
        let pic = picture(&self.screens);
        *self.pixels.lock().unwrap() = ((pic.x * ppp).round() as u32, (pic.y * ppp).round() as u32);
        let p = style::palette(ui.ctx());
        let col = widgets::alert_frame(ui, ICON);
        let mut y = col.top();
        style::label(ui.painter(), col.left(), y + style::PLAIN_SIZE / 2.0 + 2.0, egui::Align::Min, "Share Your Screen", style::bold_font(), c(p.panel_text));
        y += m::LABEL_ROW + m::TIGHT;
        y = widgets::alert_text(ui, col, y, &message(&self.ask.app)) + m::SPACING * 2.0;
        let items: Vec<Thumbnail> = self.screens.iter().map(|s| Thumbnail { texture: self.textures.get(&s.output).cloned(), color: None, label: s.label.clone() }).collect();
        let area = egui::Rect::from_min_max(egui::pos2(col.left(), y), egui::pos2(col.right(), col.bottom() - m::CONTROL_HEIGHT - m::SPACING));
        ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            if let Some(i) = ThumbnailPicker::new("screens", &items, self.selected).picture_size(pic).show(ui) {
                self.selected = Some(i);
            }
        });
        // Double-clicking one shares it at once, as a file dialog opens.
        let double = ui.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary)) && ui.input(|i| i.pointer.interact_pos()).is_some_and(|at| area.contains(at));
        match widgets::alert_buttons(ui, col, &["Cancel", "Share"], self.selected.is_some()) {
            Some(0) => Some(None),
            Some(_) => self.chosen().map(Some),
            None if double => self.chosen().map(Some),
            None => None,
        }
    }

    fn chosen(&self) -> Option<Vec<String>> {
        self.selected.and_then(|i| self.screens.get(i)).map(|s| vec![s.output.clone()])
    }
}

/// The last frame of `cap`, shrunk to fit `(w, h)` device pixels and
/// centred on transparency, as straight RGBA.
fn shrink(cap: &Capture, (w, h): (u32, u32)) -> Option<egui::ColorImage> {
    use fast_image_resize::{PixelType, ResizeAlg, ResizeOptions, Resizer, images::Image};
    let (sw, sh) = cap.size()?;
    let mut frame = vec![0u8; (sw * sh * 4) as usize];
    cap.copy_into(&mut frame, sw as usize * 4).ok()?;
    let fit = (w as f64 / sw as f64).min(h as f64 / sh as f64);
    let (tw, th) = (((sw as f64 * fit).round() as u32).clamp(1, w), ((sh as f64 * fit).round() as u32).clamp(1, h));
    let src = Image::from_vec_u8(sw, sh, frame, PixelType::U8x4).ok()?;
    let mut small = Image::new(tw, th, PixelType::U8x4);
    Resizer::new().resize(&src, &mut small, &ResizeOptions::new().resize_alg(ResizeAlg::SuperSampling(fast_image_resize::FilterType::Bilinear, 2))).ok()?;
    let (ox, oy) = ((w - tw) / 2, (h - th) / 2);
    let mut out = vec![0u8; (w * h * 4) as usize];
    for (y, row) in small.buffer().chunks_exact(tw as usize * 4).enumerate() {
        let at = (((oy + y as u32) * w + ox) * 4) as usize;
        // BGRx to RGBA.
        for (o, px) in out[at..at + tw as usize * 4].chunks_exact_mut(4).zip(row.chunks_exact(4)) {
            o.copy_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    Some(egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(w: u32, h: u32) -> Screen {
        Screen { output: "DP-1".into(), label: "Studio Display".into(), size: (w, h) }
    }

    #[test]
    fn pictures_take_the_screens_shape() {
        assert_eq!(picture(&[screen(5120, 2880)]), vec2(192.0, 108.0));
        assert_eq!(picture(&[screen(2560, 1600)]), vec2(172.0, 108.0));
        // Whole device pixels at every quarter scale.
        for s in [screen(3456, 2234), screen(1920, 1200), screen(1080, 1920)] {
            let p = picture(&[s]);
            assert_eq!(p.x % 4.0, 0.0);
        }
        assert_eq!(picture(&[]), vec2(192.0, 108.0));
    }

    #[test]
    fn the_window_fits_its_screens() {
        let pic = vec2(192.0, 108.0);
        let (column, (w, h)) = layout(1, pic, "");
        let (column3, (w3, _)) = layout(3, pic, "Google Chrome");
        // Room for two side by side even with one, three at most.
        assert!(column >= 2.0 * pic.x);
        assert!(column3 >= 3.0 * pic.x && column3 < 4.0 * pic.x);
        assert!(w3 > w && h > 0);
        let (_, (_, h4)) = layout(4, pic, "");
        assert!(h4 > h, "a second row of screens");
    }
}
