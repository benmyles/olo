//! The picker: what an app may share, asked of the person when it starts
//! a screen share (the portal's Start). A dialog over the app's window
//! (its exported handle, xdg-foreign): who is asking, the screens and the
//! windows it may have (as the app asked: either or both) as live
//! thumbnails to choose from (`ThumbnailPicker`s, so agents read and
//! choose them as radio buttons), and Cancel / Share. The thumbnails come
//! from a worker with a capture of each: it asks for a frame now and then
//! (a screen twice a second, a window every two), app_server answers once
//! it has changed, and the frame is shrunk to the picture's size in device
//! pixels (drawn one texture pixel to a device pixel) before it reaches
//! the UI.

use olo_sync::Mutex;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use olo_ui::{
    Frame, WindowOptions,
    gpui::{App, Context, FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _, Render, RenderImage, ScrollHandle, SharedString, Styled as _, Window, WindowHandle, div, px},
    metrics as m,
    widgets::{
        AlertFrame, Button, SCROLL_BAR, ScrollView, StringView, Thumbnail, ThumbnailPicker, alert,
        alert::{ALERT_TEXT_X, alert_title, button_row, paragraph, paragraph_height},
    },
};

use crate::{
    capture::{Capture, Source},
    ui::{self, Answer},
};

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
struct Thumb {
    key: String,
    image: Arc<RenderImage>,
}

/// The pictures' height, logical; a screen's width follows the first
/// screen's shape (others are letterboxed), a window's is 16:10. Both
/// multiples of 4, so they are whole device pixels at every scale from 1×
/// to 2× in quarters.
const PICTURE_HEIGHT: f32 = 108.0;
const WINDOW_PICTURE: (f32, f32) = (160.0, 100.0);
/// At most this many to a row, and rows of windows shown before they
/// scroll.
const COLUMNS: usize = 3;
const WINDOW_ROWS: usize = 2;
const ICON: &str = "display";

/// The pictures' size for `screens`: the first's shape, 192×108 for 16:9.
fn picture(screens: &[Item]) -> (f32, f32) {
    let aspect = screens.first().filter(|s| s.size.1 > 0).map_or(16.0 / 9.0, |s| s.size.0 as f32 / s.size.1 as f32);
    let width = ((PICTURE_HEIGHT * aspect / 4.0).round() * 4.0).clamp(144.0, 224.0);
    (width, PICTURE_HEIGHT)
}

/// A row of thumbnails' height, for pictures `picture` high.
fn row(picture: (f32, f32)) -> f32 {
    picture.1 + 4.0 + m::TIGHT + m::LABEL_ROW
}

/// The width `n` of these take side by side (at least two, at most three).
fn columns(n: usize, picture: (f32, f32)) -> f32 {
    let k = n.clamp(2, COLUMNS) as f32;
    k * (picture.0 + 4.0) + (k - 1.0) * m::SPACING
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

/// The windows' area: at most two rows (more scroll).
fn windows_height(n: usize) -> f32 {
    let rows = n.div_ceil(COLUMNS).min(WINDOW_ROWS) as f32;
    rows * row(WINDOW_PICTURE) + (rows - 1.0) * m::SPACING
}

/// The column the text and thumbnails share, and the window's size
/// (`text_height` measures the message wrapped to a width).
fn layout(screens: &[Item], windows: &[Item], app: &str, types: u32, text_height: impl Fn(&str, f32) -> f32) -> (f32, (f32, f32)) {
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
        column = column.max(columns(windows.len(), WINDOW_PICTURE) + SCROLL_BAR);
        if !screens.is_empty() {
            thumbs += m::SPACING * 2.0;
        }
        thumbs += heading + windows_height(windows.len());
    }
    if screens.is_empty() && windows.is_empty() {
        column = columns(2, pic);
        thumbs = m::LABEL_ROW;
    }
    let text = text_height(&message(app, types), column);
    let height = 2.0 * m::WINDOW_INSET + m::LABEL_ROW + m::TIGHT + text + m::SPACING * 2.0 + thumbs + m::SPACING * 2.0 + m::CONTROL_HEIGHT;
    let width = ALERT_TEXT_X + column + m::WINDOW_INSET;
    (column, (width.ceil(), height.ceil()))
}

/// One picker on screen.
pub struct Picker {
    answer: Answer<Option<Vec<String>>>,
    app: String,
    types: u32,
    screens: Vec<Item>,
    windows: Vec<Item>,
    pictures: HashMap<String, Arc<RenderImage>>,
    /// The key of what is chosen.
    selected: Option<String>,
    column: f32,
    /// The thumbnails' sizes in device pixels (screens', windows'), which
    /// the worker shrinks to.
    pixels: Arc<Mutex<((u32, u32), (u32, u32))>>,
    stop: Arc<AtomicBool>,
    scroll: ScrollHandle,
    focus: FocusHandle,
}

impl Drop for Picker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Opens the picker for `ask`, choosing among `screens` and `windows`
/// (those the app may have).
pub fn open(cx: &mut App, ask: Ask, mut screens: Vec<Item>, mut windows: Vec<Item>) -> Option<WindowHandle<Frame>> {
    if ask.types & SCREENS == 0 {
        screens.clear();
    }
    if ask.types & WINDOWS == 0 {
        windows.clear();
    }
    let (column, (w, h)) = layout(&screens, &windows, &ask.app, ask.types, |t, width| paragraph_height(cx, t, width));
    let options = ui::over(WindowOptions::alert("Share Screen", w, h), &ask.parent);
    ui::open(cx, options, move |window, cx| {
        let (tx, rx) = async_channel::unbounded::<Thumb>();
        let pic = picture(&screens);
        let s = window.scale_factor();
        let dev = |(w, h): (f32, f32)| ((w * s).round() as u32, (h * s).round() as u32);
        let picker = Picker {
            answer: Answer::new(ask.reply, None),
            app: ask.app,
            types: ask.types,
            selected: screens.first().or(windows.first()).map(|i| i.key.clone()),
            pixels: Arc::new(Mutex::new((dev(pic), dev(WINDOW_PICTURE)))),
            screens,
            windows,
            pictures: HashMap::new(),
            column,
            stop: Arc::new(AtomicBool::new(false)),
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
        };
        window.focus(&picker.focus);
        picker.watch(tx);
        // Its pictures leave the GPU's atlas with it.
        cx.on_release(|p: &mut Picker, cx| {
            for (_, img) in p.pictures.drain() {
                cx.drop_image(img, None);
            }
        })
        .detach();
        // Each picture as it comes.
        cx.spawn(async move |me: olo_ui::gpui::WeakEntity<Picker>, cx| {
            while let Ok(t) = rx.recv().await {
                if me
                    .update(cx, |p, cx| {
                        // (The one it replaces leaves the GPU's atlas: a
                        // screen's comes twice a second.)
                        if let Some(old) = p.pictures.insert(t.key, t.image) {
                            cx.drop_image(old, None);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        picker
    })
}

impl Picker {
    /// The worker: a capture of each item, shrunk for its thumbnail as
    /// often as it changes (a screen at most twice a second, a window
    /// every two seconds).
    fn watch(&self, to_ui: async_channel::Sender<Thumb>) {
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
            // A frame's pixels, read into the same buffer each time.
            let mut frame = Vec::new();
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
                            if let Some(image) = shrink(cap, want, &mut frame) {
                                shown.insert(key.clone(), want);
                                let _ = to_ui.send_blocking(Thumb { key: key.clone(), image });
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
                        && let Some(image) = shrink(cap, want, &mut frame)
                    {
                        shown.insert(key.clone(), want);
                        let _ = to_ui.send_blocking(Thumb { key: key.clone(), image });
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
            // A screen's frames (16 MB each at 2560×1600) were this
            // thread's: its heap goes back, not kept by the allocator
            // for a thread that's ending.
            drop((captures, frame));
            // SAFETY: glibc's malloc_trim takes no pointers.
            unsafe { libc::malloc_trim(0) };
        });
    }

    fn chosen(&self) -> Option<Vec<String>> {
        self.selected.clone().map(|k| vec![k])
    }

    /// Answered: shared (the choice) or cancelled; the window goes.
    fn finish(&mut self, share: bool, window: &mut Window) {
        let answer = if share { self.chosen() } else { None };
        if share && answer.is_none() {
            return;
        }
        self.answer.give(answer);
        window.remove_window();
    }

    fn thumbnails(&self, items: &[Item]) -> Vec<Thumbnail> {
        items.iter().map(|i| Thumbnail { picture: self.pictures.get(&i.key).cloned(), color: None, label: SharedString::from(i.label.clone()) }).collect()
    }
}

impl Render for Picker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The pictures' device sizes, for the worker (the scale may change).
        let s = window.scale_factor();
        let dev = |(w, h): (f32, f32)| ((w * s).round() as u32, (h * s).round() as u32);
        let pic = picture(&self.screens);
        *self.pixels.lock().unwrap() = (dev(pic), dev(WINDOW_PICTURE));
        let me = cx.entity();
        let headed = both(&self.screens, &self.windows);
        let column = self.column;
        let heading = |text: &'static str| div().mb(px(m::TIGHT)).child(StringView::new(text).bold(true).height(m::LABEL_ROW));
        let mut body = div().flex().flex_col().w(px(column));
        if self.screens.is_empty() && self.windows.is_empty() {
            body = body.child(StringView::new("There is nothing to share.").bold(true));
        }
        let pick = |items: &[Item], key: &Option<String>| key.as_ref().and_then(|k| items.iter().position(|i| &i.key == k));
        if !self.screens.is_empty() {
            if headed {
                body = body.child(heading("Screens"));
            }
            let keys: Vec<String> = self.screens.iter().map(|i| i.key.clone()).collect();
            let (k1, k2) = (keys.clone(), keys);
            let (m1, m2) = (me.clone(), me.clone());
            body = body.child(
                ThumbnailPicker::new("screens", self.thumbnails(&self.screens), pick(&self.screens, &self.selected))
                    .picture_size(pic.0, pic.1)
                    .width(column)
                    .on_select(move |i, _, cx| {
                        let k = k1[i].clone();
                        m1.update(cx, |p, cx| {
                            p.selected = Some(k);
                            cx.notify();
                        })
                    })
                    .on_open(move |i, window, cx| {
                        let k = k2[i].clone();
                        m2.update(cx, |p, _| {
                            p.selected = Some(k);
                            p.finish(true, window);
                        })
                    }),
            );
        }
        if !self.windows.is_empty() {
            if !self.screens.is_empty() {
                body = body.child(div().h(px(m::SPACING * 2.0)));
            }
            if headed {
                body = body.child(heading("Windows"));
            }
            let keys: Vec<String> = self.windows.iter().map(|i| i.key.clone()).collect();
            let (k1, k2) = (keys.clone(), keys);
            let (m1, m2) = (me.clone(), me.clone());
            let fits = self.windows.len() <= COLUMNS * WINDOW_ROWS;
            // More than fit: they scroll, set in from the edges the view
            // clips at, so the chosen one's plate shows whole.
            let width = if fits { column } else { column - SCROLL_BAR - 4.0 };
            let grid = ThumbnailPicker::new("windows", self.thumbnails(&self.windows), pick(&self.windows, &self.selected))
                .picture_size(WINDOW_PICTURE.0, WINDOW_PICTURE.1)
                .width(width)
                .on_select(move |i, _, cx| {
                    let k = k1[i].clone();
                    m1.update(cx, |p, cx| {
                        p.selected = Some(k);
                        cx.notify();
                    })
                })
                .on_open(move |i, window, cx| {
                    let k = k2[i].clone();
                    m2.update(cx, |p, _| {
                        p.selected = Some(k);
                        p.finish(true, window);
                    })
                });
            body = if fits {
                body.child(grid)
            } else {
                body.child(
                    div().h(px(windows_height(self.windows.len()) + 4.0)).child(
                        ScrollView::new("windows", &self.scroll).border(false).auto_hide(true).child(div().p(px(2.0)).child(grid)),
                    ),
                )
            };
        }
        let cancel = Button::new("Cancel").on_click({
            let me = me.clone();
            move |window, cx| me.update(cx, |p, _| p.finish(false, window))
        });
        let share = Button::new("Share").default(true).enabled(self.selected.is_some()).on_click({
            let me = me.clone();
            move |window, cx| me.update(cx, |p, _| p.finish(true, window))
        });
        div()
            .size_full()
            .key_context(alert::CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|p, _: &alert::Default, window, _| p.finish(true, window)))
            .on_action(cx.listener(|p, _: &alert::Cancel, window, _| p.finish(false, window)))
            .child(
                AlertFrame::new(ICON)
                    .child(alert_title("Share Your Screen"))
                    .child(div().mt(px(m::TIGHT)).child(paragraph(message(&self.app, self.types))))
                    .child(div().h(px(m::SPACING * 2.0)))
                    .child(body)
                    .child(div().flex_1())
                    .child(button_row().child(cancel).child(share)),
            )
    }
}

/// The last frame of `cap`, shrunk to fit `(w, h)` device pixels, in
/// its own proportions, as GPUI draws it (BGRA: the frame's own order).
fn shrink(cap: &Capture, (w, h): (u32, u32), frame: &mut Vec<u8>) -> Option<Arc<RenderImage>> {
    use fast_image_resize::{PixelType, ResizeAlg, ResizeOptions, Resizer, images::{Image, ImageRef}};
    let (sw, sh) = cap.frame_size()?;
    frame.resize((sw * sh * 4) as usize, 0);
    cap.copy_into(frame, sw as usize * 4).ok()?;
    let fit = (w as f64 / sw as f64).min(h as f64 / sh as f64);
    let (tw, th) = (((sw as f64 * fit).round() as u32).clamp(1, w), ((sh as f64 * fit).round() as u32).clamp(1, h));
    let src = ImageRef::new(sw, sh, frame, PixelType::U8x4).ok()?;
    let mut small = Image::new(tw, th, PixelType::U8x4);
    Resizer::new().resize(&src, &mut small, &ResizeOptions::new().resize_alg(ResizeAlg::SuperSampling(fast_image_resize::FilterType::Bilinear, 2))).ok()?;
    // BGRx: opaque.
    let mut out = small.into_vec();
    for px in out.chunks_exact_mut(4) {
        px[3] = 255;
    }
    olo_ui::widgets::image_view::from_bgra(tw, th, out)
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

    /// Two lines of text, whatever the width (enough to lay it out by).
    fn text(_: &str, _: f32) -> f32 {
        34.0
    }

    #[test]
    fn pictures_take_the_screens_shape() {
        assert_eq!(picture(&[screen(5120, 2880)]), (192.0, 108.0));
        assert_eq!(picture(&[screen(2560, 1600)]), (172.0, 108.0));
        // Whole device pixels at every quarter scale.
        for s in [screen(3456, 2234), screen(1920, 1200), screen(1080, 1920)] {
            let p = picture(&[s]);
            assert_eq!(p.0 % 4.0, 0.0);
        }
        assert_eq!(picture(&[]), (192.0, 108.0));
    }

    #[test]
    fn the_window_fits_what_it_offers() {
        let pic = (192.0, 108.0);
        let (column, (w, h)) = layout(&[screen(5120, 2880)], &[], "", SCREENS, text);
        let (column3, (w3, _)) = layout(&[screen(5120, 2880), screen(5120, 2880), screen(5120, 2880)], &[], "Google Chrome", SCREENS, text);
        // Room for two side by side even with one, three at most.
        assert!(column >= 2.0 * pic.0);
        assert!(column3 >= 3.0 * pic.0 && column3 < 4.0 * pic.0);
        assert!(w3 > w && h > 0.0);
        // Windows add their section, two rows of them at most (they scroll).
        let windows: Vec<Item> = (0..12).map(window).collect();
        let (_, (_, with)) = layout(&[screen(5120, 2880)], &windows[..2], "", SCREENS | WINDOWS, text);
        let (_, (_, many)) = layout(&[screen(5120, 2880)], &windows, "", SCREENS | WINDOWS, text);
        let (_, (_, six)) = layout(&[screen(5120, 2880)], &windows[..6], "", SCREENS | WINDOWS, text);
        assert!(with > h && many > with && many == six);
    }
}

