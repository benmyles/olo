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

use std::{collections::HashMap, sync::Arc, time::Duration};

use olo_kit::{
    ClickEvent, Cx, Element, InteractiveElement as _, KeyDownEvent, ParentElement as _, RenderImage, ScrollHandle, SharedString, Styled as _, View, WindowHandle, WindowOptions,
    app_kit::Messenger,
    div, metrics as m, px,
    widgets::{
        AlertFrame, Button, SCROLL_BAR, ScrollView, StringView, Thumbnail, ThumbnailPicker,
        alert::{ALERT_TEXT_X, alert_title, button_row, paragraph, paragraph_height},
    },
};

use olo_capture::{
    Source,
    thumbnails::{self, Live},
};

use crate::ui::{self, Answer};

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
    /// What it will control besides (the RemoteDesktop portal's devices:
    /// keyboard, pointer, touch screen; 0 for a screen share alone).
    pub control: u32,
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

/// A picture of an item, shrunk for its thumbnail (what the picker is
/// told, as each comes).
pub struct Thumb {
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

fn message(app: &str, types: u32, control: u32) -> String {
    let who = if app.is_empty() { "An app" } else { app };
    let what = match types & (SCREENS | WINDOWS) {
        WINDOWS => "a window",
        SCREENS => "your screen",
        _ => "your screen or a window",
    };
    if control != 0 {
        return format!(
            "{who} wants to see {what} and control your {}. Everything in the one you choose is shown, and it can type, click and move the pointer as you do, until you stop it.",
            crate::remote::devices_said(control)
        );
    }
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
fn layout(screens: &[Item], windows: &[Item], app: &str, types: u32, control: u32, text_height: impl Fn(&str, f32) -> f32) -> (f32, (f32, f32)) {
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
    let text = text_height(&message(app, types, control), column);
    let height = 2.0 * m::WINDOW_INSET + m::LABEL_ROW + m::TIGHT + text + m::SPACING * 2.0 + thumbs + m::SPACING * 2.0 + m::CONTROL_HEIGHT;
    let width = ALERT_TEXT_X + column + m::WINDOW_INSET;
    (column, (width.ceil(), height.ceil()))
}

/// One picker on screen.
pub struct Picker {
    answer: Answer<Option<Vec<String>>>,
    app: String,
    types: u32,
    control: u32,
    screens: Vec<Item>,
    windows: Vec<Item>,
    pictures: HashMap<String, Arc<RenderImage>>,
    /// The key of what is chosen.
    selected: Option<String>,
    column: f32,
    /// The thumbnails' sizes in device pixels (screens', windows'), which
    /// the worker shrinks to (told it again when the scale changes).
    pixels: ((u32, u32), (u32, u32)),
    /// The worker making them (`olo_capture::thumbnails`): stopped as
    /// the picker goes.
    live: Option<Live>,
    /// Whether its window was the active one when last drawn: its
    /// pictures are live only while it is.
    looking: bool,
    scroll: ScrollHandle,
}

/// Opens the picker for `ask`, choosing among `screens` and `windows`
/// (those the app may have).
pub fn open(ask: Ask, mut screens: Vec<Item>, mut windows: Vec<Item>) -> WindowHandle<Picker> {
    if ask.types & SCREENS == 0 {
        screens.clear();
    }
    if ask.types & WINDOWS == 0 {
        windows.clear();
    }
    let (column, (w, h)) = layout(&screens, &windows, &ask.app, ask.types, ask.control, paragraph_height);
    let options = ui::over(WindowOptions::alert(if ask.control != 0 { "Share Screen and Control" } else { "Share Screen" }, w, h), &ask.parent);
    ui::open(options, move |cx| {
        let pic = picture(&screens);
        let s = cx.scale_factor();
        let dev = |(w, h): (f32, f32)| ((w * s).round() as u32, (h * s).round() as u32);
        let picker = Picker {
            answer: Answer::new(ask.reply, None),
            app: ask.app,
            types: ask.types,
            control: ask.control,
            selected: screens.first().or(windows.first()).map(|i| i.key.clone()),
            pixels: (dev(pic), dev(WINDOW_PICTURE)),
            screens,
            windows,
            pictures: HashMap::new(),
            column,
            live: None,
            looking: true,
            scroll: ScrollHandle::new(),
        };
        // Each picture as it comes.
        let mut picker = picker;
        picker.live = Some(picker.watch(cx.messenger()));
        picker
    })
}

impl Picker {
    /// The worker: a capture of each item, shrunk for its thumbnail as
    /// often as it changes (a screen at most twice a second, a window
    /// every two seconds).
    fn watch(&self, to_ui: Messenger<Thumb>) -> Live {
        let (screens, windows) = self.pixels;
        let items = self
            .screens
            .iter()
            .map(|s| thumbnails::Item { key: s.key.clone(), source: s.source.clone(), every: Duration::from_millis(500), size: screens })
            .chain(self.windows.iter().map(|w| thumbnails::Item { key: w.key.clone(), source: w.source.clone(), every: Duration::from_secs(2), size: windows }))
            .collect();
        Live::start(items, move |p| {
            if let Some(image) = olo_kit::pictures::from_bgra(p.width, p.height, p.bgra) {
                to_ui.send(Thumb { key: p.key, image });
            }
        })
    }

    fn chosen(&self) -> Option<Vec<String>> {
        self.selected.clone().map(|k| vec![k])
    }

    /// Answered: shared (the choice) or cancelled; the window goes.
    fn finish(&mut self, share: bool, cx: &mut Cx<Self>) {
        let answer = if share { self.chosen() } else { None };
        if share && answer.is_none() {
            return;
        }
        self.answer.give(answer);
        cx.close_window();
    }

    fn thumbnails(&self, items: &[Item]) -> Vec<Thumbnail> {
        items.iter().map(|i| Thumbnail { picture: self.pictures.get(&i.key).cloned(), color: None, label: SharedString::from(i.label.clone()), key: None }).collect()
    }

    /// A grid of `items` to choose from: a click chooses, a double click
    /// shares.
    fn grid(&self, id: &'static str, items: &[Item], picture: (f32, f32), width: f32, cx: &mut Cx<Self>) -> ThumbnailPicker {
        let chosen = self.selected.as_ref().and_then(|k| items.iter().position(|i| &i.key == k));
        let keys: Vec<String> = items.iter().map(|i| i.key.clone()).collect();
        let (k1, k2) = (keys.clone(), keys);
        ThumbnailPicker::new(id, self.thumbnails(items), chosen)
            .picture_size(picture.0, picture.1)
            .width(width)
            .on_select(cx.listener(move |p: &mut Picker, i: &usize, cx| {
                p.selected = k1.get(*i).cloned();
                cx.invalidate();
            }))
            .on_open(cx.listener(move |p: &mut Picker, i: &usize, cx| {
                p.selected = k2.get(*i).cloned();
                p.finish(true, cx);
            }))
    }
}

impl View for Picker {
    type Message = Thumb;

    /// A picture came (the one it replaces goes, its texture with it: a
    /// screen's comes twice a second).
    fn message(&mut self, t: Thumb, cx: &mut Cx<Self>) {
        self.pictures.insert(t.key, t.image);
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        // The pictures' device sizes, for the worker (the scale may change).
        let s = cx.scale_factor();
        let dev = |(w, h): (f32, f32)| ((w * s).round() as u32, (h * s).round() as u32);
        let pic = picture(&self.screens);
        let now = (dev(pic), dev(WINDOW_PICTURE));
        if now != self.pixels {
            self.pixels = now;
            if let Some(live) = &self.live {
                for s in &self.screens {
                    live.set_size(&s.key, now.0);
                }
                for w in &self.windows {
                    live.set_size(&w.key, now.1);
                }
            }
        }
        // (Behind other windows, nobody is choosing: nothing is asked of
        // the screens and windows it shows until it is looked at again.)
        let looking = cx.is_window_active();
        if looking != self.looking {
            self.looking = looking;
            if let Some(live) = &self.live {
                live.set_live(looking);
            }
        }
        let headed = both(&self.screens, &self.windows);
        let column = self.column;
        let heading = |text: &'static str| div().mb(px(m::TIGHT)).child(StringView::new(text).bold(true).height(m::LABEL_ROW));
        let mut body = div().flex().flex_col().w(px(column));
        if self.screens.is_empty() && self.windows.is_empty() {
            body = body.child(StringView::new("There is nothing to share.").bold(true));
        }
        if !self.screens.is_empty() {
            if headed {
                body = body.child(heading("Screens"));
            }
            body = body.child(self.grid("screens", &self.screens, pic, column, cx));
        }
        if !self.windows.is_empty() {
            if !self.screens.is_empty() {
                body = body.child(div().h(px(m::SPACING * 2.0)));
            }
            if headed {
                body = body.child(heading("Windows"));
            }
            let fits = self.windows.len() <= COLUMNS * WINDOW_ROWS;
            // More than fit: they scroll, set in from the edges the view
            // clips at, so the chosen one's plate shows whole.
            let width = if fits { column } else { column - SCROLL_BAR - 4.0 };
            let grid = self.grid("windows", &self.windows, WINDOW_PICTURE, width, cx);
            body = if fits {
                body.child(grid)
            } else {
                body.child(div().h(px(windows_height(self.windows.len()) + 4.0)).child(ScrollView::new(&self.scroll).border(false).auto_hide(true).child(div().p(px(2.0)).child(grid))))
            };
        }
        let cancel = Button::new("Cancel").on_click(cx.listener(|p: &mut Picker, _: &ClickEvent, cx| p.finish(false, cx)));
        // (Return shares: the default button. Escape cancels.)
        let share = Button::new(if self.control != 0 { "Allow" } else { "Share" })
            .default(true)
            .enabled(self.selected.is_some())
            .on_click(cx.listener(|p: &mut Picker, _: &ClickEvent, cx| p.finish(true, cx)));
        div()
            .size_full()
            .on_window_key_down(cx.listener(|p: &mut Picker, e: &KeyDownEvent, cx| {
                if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
                    p.finish(false, cx);
                }
            }))
            .child(
                AlertFrame::new(ICON)
                    .child(alert_title(if self.control != 0 { "Share and Control Your Screen" } else { "Share Your Screen" }))
                    .child(div().mt(px(m::TIGHT)).child(paragraph(message(&self.app, self.types, self.control))))
                    .child(div().h(px(m::SPACING * 2.0)))
                    .child(body)
                    .child(div().flex_1())
                    .child(button_row().child(cancel).child(share)),
            )
    }
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
        let (column, (w, h)) = layout(&[screen(5120, 2880)], &[], "", SCREENS, 0, text);
        let (column3, (w3, _)) = layout(&[screen(5120, 2880), screen(5120, 2880), screen(5120, 2880)], &[], "Google Chrome", SCREENS, 0, text);
        // Room for two side by side even with one, three at most.
        assert!(column >= 2.0 * pic.0);
        assert!(column3 >= 3.0 * pic.0 && column3 < 4.0 * pic.0);
        assert!(w3 > w && h > 0.0);
        // Windows add their section, two rows of them at most (they scroll).
        let windows: Vec<Item> = (0..12).map(window).collect();
        let (_, (_, with)) = layout(&[screen(5120, 2880)], &windows[..2], "", SCREENS | WINDOWS, 0, text);
        let (_, (_, many)) = layout(&[screen(5120, 2880)], &windows, "", SCREENS | WINDOWS, 0, text);
        let (_, (_, six)) = layout(&[screen(5120, 2880)], &windows[..6], "", SCREENS | WINDOWS, 0, text);
        assert!(with > h && many > with && many == six);
    }
}
