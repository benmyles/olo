//! The picker: what an app may share, asked of the person when it starts
//! a screen share (the portal's Start). A kit dialog over the app's window
//! (its exported handle, `WindowOptions::foreign_parent`): who is asking,
//! the screens and the windows it may have (as the app asked: either or
//! both) as live thumbnails to choose from (`ThumbnailPicker`s, so agents
//! read and choose them as radio buttons), and Cancel / Share. The
//! thumbnails come from a worker with a capture of each: it asks for a
//! frame now and then (a screen twice a second, a window every two),
//! app_server answers once it has changed, and the frame is shrunk to the
//! picture's size in device pixels (letterboxed, drawn one texture pixel
//! to a device pixel) before it reaches the UI.

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
    widgets::{self, ScrollView, Thumbnail, ThumbnailPicker},
};

use crate::capture::{Capture, Source};

/// Source types (bits), as the portal says them.
pub const SCREENS: u32 = 1;
pub const WINDOWS: u32 = 2;

/// What the picker was asked for (from the portal's Start).
pub struct Ask {
    /// The portal request it answers (its handle), so it can be cancelled.
    pub request: String,
    /// Who is asking ("" when the portal couldn't tell).
    pub app: String,
    /// The window it opens over (`wayland:HANDLE`, or "").
    pub parent: String,
    /// What it may share: `SCREENS`, `WINDOWS` or both.
    pub types: u32,
    /// The answer: the keys of what was chosen (`Item::key`), or None if
    /// cancelled.
    pub reply: async_channel::Sender<Option<Vec<String>>>,
}

/// Something to choose: a screen or a window.
pub struct Item {
    /// What the answer names it by: a screen's connector, `window:ID`.
    pub key: String,
    pub label: String,
    /// Its size (pixels or logical: only its shape counts here).
    pub size: (u32, u32),
    pub source: Source,
}

/// A picture of an item, shrunk for its thumbnail.
pub struct Thumb {
    pub window: WindowId,
    pub key: String,
    pub image: egui::ColorImage,
}

/// The pictures' height, logical; a screen's width follows the first
/// screen's shape (others are letterboxed), a window's is 16:10. Both
/// multiples of 4, so they are whole device pixels at every scale from 1×
/// to 2× in quarters.
const PICTURE_HEIGHT: f32 = 108.0;
const WINDOW_PICTURE: egui::Vec2 = vec2(160.0, 100.0);
/// At most this many to a row, and rows of windows shown before they
/// scroll.
const COLUMNS: usize = 3;
const WINDOW_ROWS: usize = 2;
const ICON: &str = "display";

/// The pictures' size for `screens`: the first's shape, 192×108 for 16:9.
fn picture(screens: &[Item]) -> egui::Vec2 {
    let aspect = screens.first().filter(|s| s.size.1 > 0).map_or(16.0 / 9.0, |s| s.size.0 as f32 / s.size.1 as f32);
    let width = ((PICTURE_HEIGHT * aspect / 4.0).round() * 4.0).clamp(144.0, 224.0);
    vec2(width, PICTURE_HEIGHT)
}

/// A row of thumbnails' height, for pictures `picture` high.
fn row(picture: egui::Vec2) -> f32 {
    picture.y + 4.0 + m::TIGHT + m::LABEL_ROW
}

/// The width `n` of these take side by side (at least two, at most three).
fn columns(n: usize, picture: egui::Vec2) -> f32 {
    let k = n.clamp(2, COLUMNS) as f32;
    k * (picture.x + 4.0) + (k - 1.0) * m::SPACING
}

/// One picker on screen.
pub struct Picker {
    ask: Ask,
    screens: Vec<Item>,
    windows: Vec<Item>,
    textures: HashMap<String, egui::TextureHandle>,
    /// The key of what is chosen.
    selected: Option<String>,
    /// The thumbnails' sizes in device pixels (screens', windows'), which
    /// the worker shrinks to.
    pixels: Arc<Mutex<((u32, u32), (u32, u32))>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Picker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn message(app: &str, types: u32) -> String {
    let who = if app.is_empty() { "An app" } else { app };
    let what = match types & (SCREENS | WINDOWS) {
        WINDOWS => "a window",
        SCREENS => "your screen",
        _ => "your screen or a window",
    };
    format!("{who} wants to share {what}. Everything in the one you choose is shown until you stop sharing.")
}

/// Whether both kinds are offered (each then under its heading).
fn both(screens: &[Item], windows: &[Item]) -> bool {
    !screens.is_empty() && !windows.is_empty()
}

/// The column the text and thumbnails share, and the window's size.
fn layout(screens: &[Item], windows: &[Item], app: &str, types: u32) -> (f32, (u32, u32)) {
    let pic = picture(screens);
    let mut column: f32 = 0.0;
    let mut thumbs = 0.0;
    let heading = if both(screens, windows) { m::LABEL_ROW + m::TIGHT } else { 0.0 };
    if !screens.is_empty() {
        column = column.max(columns(screens.len(), pic));
        let rows = screens.len().div_ceil(COLUMNS) as f32;
        thumbs += heading + rows * row(pic) + (rows - 1.0) * m::SPACING;
    }
    if !windows.is_empty() {
        column = column.max(columns(windows.len(), WINDOW_PICTURE) + widgets::SCROLL_BAR);
        let rows = windows.len().div_ceil(COLUMNS).min(WINDOW_ROWS) as f32;
        if !screens.is_empty() {
            thumbs += m::SPACING * 2.0;
        }
        thumbs += heading + rows * row(WINDOW_PICTURE) + (rows - 1.0) * m::SPACING;
    }
    if screens.is_empty() && windows.is_empty() {
        column = columns(2, pic);
        thumbs = m::LABEL_ROW;
    }
    let text = style::plain_text_height(&message(app, types), column);
    let height = 2.0 * m::WINDOW_INSET + m::LABEL_ROW + m::TIGHT + text + m::SPACING * 2.0 + thumbs + m::SPACING * 2.0 + m::CONTROL_HEIGHT;
    let width = widgets::ALERT_TEXT_X + column + m::WINDOW_INSET;
    (column, (width.ceil() as u32, height.ceil() as u32))
}

impl Picker {
    /// Open the picker for `ask`, choosing among `screens` and `windows`
    /// (those the app may have).
    pub fn open(cx: &mut Cx, ask: Ask, mut screens: Vec<Item>, mut windows: Vec<Item>) -> (WindowId, Picker) {
        if ask.types & SCREENS == 0 {
            screens.clear();
        }
        if ask.types & WINDOWS == 0 {
            windows.clear();
        }
        let (_, (w, h)) = layout(&screens, &windows, &ask.app, ask.types);
        let mut options = WindowOptions { dialog: true, ..WindowOptions::new("Share Screen", w, h).min_size(w, h).max_size(w, h) };
        if !ask.parent.is_empty() {
            options = options.foreign_parent(ask.parent.clone());
        }
        let id = cx.open_window(options);
        cx.activate(id);
        let pic = picture(&screens);
        let px = |v: egui::Vec2| (v.x as u32, v.y as u32);
        let picker = Picker {
            ask,
            selected: screens.first().or(windows.first()).map(|i| i.key.clone()),
            pixels: Arc::new(Mutex::new((px(pic), px(WINDOW_PICTURE)))),
            screens,
            windows,
            textures: HashMap::new(),
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
        let texture = ctx.load_texture(format!("share-{}", t.key), t.image, egui::TextureOptions::NEAREST);
        self.textures.insert(t.key, texture);
    }

    /// The worker: a capture of each item, shrunk for its thumbnail as
    /// often as it changes (a screen at most twice a second, a window
    /// every two seconds).
    fn watch(&self, window: WindowId, proxy: Proxy) {
        let items: Vec<(String, Source, bool)> = self.screens.iter().map(|s| (s.key.clone(), s.source.clone(), true)).chain(self.windows.iter().map(|w| (w.key.clone(), w.source.clone(), false))).collect();
        let (pixels, stop) = (self.pixels.clone(), self.stop.clone());
        let _ = std::thread::Builder::new().name("picker-thumbnails".into()).spawn(move || {
            let mut captures: Vec<(String, bool, Option<Capture>, Instant)> = items
                .into_iter()
                .map(|(key, source, screen)| {
                    let mut c = Capture::new(&source, false).ok();
                    if let Some(cap) = c.as_mut()
                        && cap.request().is_err()
                    {
                        c = None;
                    }
                    (key, screen, c, Instant::now())
                })
                .collect();
            let mut shown: HashMap<String, (u32, u32)> = HashMap::new();
            while !stop.load(Ordering::Relaxed) {
                let (screens, windows) = *pixels.lock().unwrap();
                for (key, screen, capture, next) in captures.iter_mut() {
                    let want = if *screen { screens } else { windows };
                    let every = Duration::from_millis(if *screen { 500 } else { 2000 });
                    let Some(cap) = capture.as_mut() else { continue };
                    if cap.stopped() {
                        *capture = None;
                        continue;
                    }
                    match cap.poll() {
                        Ok(Some(Ok(()))) => {
                            if let Some(image) = shrink(cap, want) {
                                shown.insert(key.clone(), want);
                                proxy.send(Thumb { window, key: key.clone(), image });
                            }
                            *next = Instant::now() + every;
                        }
                        Ok(Some(Err(_))) => *next = Instant::now() + every,
                        Ok(None) => {}
                        Err(_) => *capture = None,
                    }
                    // The scale changed: this size again, from the last frame.
                    if shown.get(key).is_some_and(|s| *s != want)
                        && let Some(cap) = capture.as_ref()
                        && let Some(image) = shrink(cap, want)
                    {
                        shown.insert(key.clone(), want);
                        proxy.send(Thumb { window, key: key.clone(), image });
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
        let dev = |v: egui::Vec2| ((v.x * ppp).round() as u32, (v.y * ppp).round() as u32);
        *self.pixels.lock().unwrap() = (dev(pic), dev(WINDOW_PICTURE));
        let p = style::palette(ui.ctx());
        let col = widgets::alert_frame(ui, ICON);
        let mut y = col.top();
        style::label(ui.painter(), col.left(), y + style::PLAIN_SIZE / 2.0 + 2.0, egui::Align::Min, "Share Your Screen", style::bold_font(), c(p.panel_text));
        y += m::LABEL_ROW + m::TIGHT;
        y = widgets::alert_text(ui, col, y, &message(&self.ask.app, self.ask.types)) + m::SPACING * 2.0;
        let headed = both(&self.screens, &self.windows);
        let heading = |ui: &egui::Ui, y: f32, text: &str| {
            style::label(ui.painter(), col.left(), y + m::LABEL_ROW / 2.0, egui::Align::Min, text, style::bold_font(), c(p.panel_text));
        };
        let mut double_area = egui::Rect::NOTHING;
        if self.screens.is_empty() && self.windows.is_empty() {
            heading(ui, y, "There is nothing to share.");
        }
        if !self.screens.is_empty() {
            if headed {
                heading(ui, y, "Screens");
                y += m::LABEL_ROW + m::TIGHT;
            }
            let rows = self.screens.len().div_ceil(COLUMNS) as f32;
            let area = egui::Rect::from_min_size(egui::pos2(col.left(), y), vec2(col.width(), rows * row(pic) + (rows - 1.0) * m::SPACING));
            let items: Vec<Thumbnail> = self.screens.iter().map(|s| Thumbnail { texture: self.textures.get(&s.key).cloned(), color: None, label: s.label.clone() }).collect();
            let on = self.selected.as_ref().and_then(|k| self.screens.iter().position(|s| &s.key == k));
            ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                if let Some(i) = ThumbnailPicker::new("screens", &items, on).picture_size(pic).show(ui) {
                    self.selected = Some(self.screens[i].key.clone());
                }
            });
            double_area = double_area.union(area);
            y = area.bottom() + m::SPACING * 2.0;
        }
        if !self.windows.is_empty() {
            if headed {
                heading(ui, y, "Windows");
                y += m::LABEL_ROW + m::TIGHT;
            }
            let rows = self.windows.len().div_ceil(COLUMNS).min(WINDOW_ROWS) as f32;
            let area = egui::Rect::from_min_size(egui::pos2(col.left(), y), vec2(col.width(), rows * row(WINDOW_PICTURE) + (rows - 1.0) * m::SPACING));
            let items: Vec<Thumbnail> = self.windows.iter().map(|w| Thumbnail { texture: self.textures.get(&w.key).cloned(), color: None, label: w.label.clone() }).collect();
            let on = self.selected.as_ref().and_then(|k| self.windows.iter().position(|w| &w.key == k));
            let mut chose = None;
            let picker = ThumbnailPicker::new("windows", &items, on).picture_size(WINDOW_PICTURE);
            if self.windows.len() <= COLUMNS * WINDOW_ROWS {
                ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| chose = picker.show(ui));
            } else {
                // More than fit: they scroll, set in from the edges the
                // view clips at, so the chosen one's plate shows whole.
                ScrollView::new("windows").border(false).auto_hide(true).show_in(ui, area.expand2(vec2(0.0, 2.0)), |ui, _| {
                    ui.add_space(2.0);
                    let inner = egui::Rect::from_min_size(ui.cursor().min + vec2(2.0, 0.0), vec2(ui.available_width() - 4.0, 0.0));
                    ui.scope_builder(egui::UiBuilder::new().max_rect(inner.with_max_y(f32::INFINITY)), |ui| chose = picker.show(ui));
                    ui.add_space(2.0);
                });
            }
            if let Some(i) = chose {
                self.selected = Some(self.windows[i].key.clone());
            }
            double_area = double_area.union(area);
        }
        // Double-clicking one shares it at once, as a file dialog opens.
        let double = ui.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary)) && ui.input(|i| i.pointer.interact_pos()).is_some_and(|at| double_area.contains(at));
        match widgets::alert_buttons(ui, col, &["Cancel", "Share"], self.selected.is_some()) {
            Some(0) => Some(None),
            Some(_) => self.chosen().map(Some),
            None if double => self.chosen().map(Some),
            None => None,
        }
    }

    fn chosen(&self) -> Option<Vec<String>> {
        self.selected.clone().map(|k| vec![k])
    }
}

/// The last frame of `cap`, shrunk to fit `(w, h)` device pixels and
/// centred on transparency, as straight RGBA.
fn shrink(cap: &Capture, (w, h): (u32, u32)) -> Option<egui::ColorImage> {
    use fast_image_resize::{PixelType, ResizeAlg, ResizeOptions, Resizer, images::Image};
    let (sw, sh) = cap.frame_size()?;
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

    fn screen(w: u32, h: u32) -> Item {
        Item { key: "DP-1".into(), label: "Studio Display".into(), size: (w, h), source: Source::Output(Some("DP-1".into())) }
    }

    fn window(id: u64) -> Item {
        Item { key: format!("window:{id}"), label: "Terminal".into(), size: (800, 500), source: Source::Window { id, app_id: "olo.terminal".into(), title: "Terminal".into() } }
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
    fn the_window_fits_what_it_offers() {
        let pic = vec2(192.0, 108.0);
        let (column, (w, h)) = layout(&[screen(5120, 2880)], &[], "", SCREENS);
        let (column3, (w3, _)) = layout(&[screen(5120, 2880), screen(5120, 2880), screen(5120, 2880)], &[], "Google Chrome", SCREENS);
        // Room for two side by side even with one, three at most.
        assert!(column >= 2.0 * pic.x);
        assert!(column3 >= 3.0 * pic.x && column3 < 4.0 * pic.x);
        assert!(w3 > w && h > 0);
        // Windows add their section, two rows of them at most (they scroll).
        let windows: Vec<Item> = (0..12).map(window).collect();
        let (_, (_, with)) = layout(&[screen(5120, 2880)], &windows[..2], "", SCREENS | WINDOWS);
        let (_, (_, many)) = layout(&[screen(5120, 2880)], &windows, "", SCREENS | WINDOWS);
        let (_, (_, six)) = layout(&[screen(5120, 2880)], &windows[..6], "", SCREENS | WINDOWS);
        assert!(with > h && many > with && many == six);
    }
}

