//! Open in which window? (Ben, 2026-11-17: "when something is opening
//! with the default app, e.g. a url opening in chrome, and that app has >
//! 1 windows open and the url would normally open as a tab in that app,
//! then i want olo to show me a dialog where i can choose which window to
//! open it in -- each window should have a live thumbnail in it to pick
//! from (as well as the window title or some subpart of it) and it should
//! also have a kb shortcut for each one so i can just hit a letter to
//! choose if i want (or click it if i'd rather)".)
//!
//! A link handed to a Chrome running with two browser windows or more
//! (app_server's windows of its browser process: not Slack's or another
//! app's, not DevTools or a picture in picture) asks first: the windows as
//! live thumbnails (`olo_capture::thumbnails`, as Share Screen's) with
//! their titles (" - Google Chrome" left off) and a letter each, A, B, …
//! (a key cap on each, as keyboard hints and Twitcher letter what they
//! offer). Its letter or a click opens the link there; Return, in the one
//! chosen (the one used last, first); New Window, in a new one; Escape or
//! Cancel, nowhere. The window chosen is brought forward
//! (`Request::Activate`), and once app_server has it in front Olo's
//! extension opens the link in a new tab of the window Chrome last had
//! the focus in: that one (`open_in_front`). Chrome's own hand-over of a
//! link won't do: it opens it in the window it last opened one in,
//! whichever is in front.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use olo_capture::{
    Source,
    thumbnails::{self, Live},
};
use olo_kit::{
    ClickEvent, Cx, Element, InteractiveElement as _, KeyDownEvent, ParentElement as _, RenderImage, SharedString, Styled as _, View, WindowOptions, div, metrics as m, px,
    widgets::{
        AlertFrame, Button, Thumbnail, ThumbnailPicker,
        alert::{alert_title, button_row, paragraph},
        thumbs,
    },
};
use olo_protocol::{Client, Reply, Request, WindowInfo};
use olo_sync::Mutex;

/// What was chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    Window(u64),
    NewWindow,
    Cancel,
}

/// The app ids Chrome's browser windows have (its channels', Chromium's).
const BROWSERS: &[&str] = &["google-chrome", "google-chrome-beta", "google-chrome-unstable", "chromium", "chromium-browser"];

/// A window's picture (16:10, as Share Screen's windows: whole device
/// pixels at every quarter scale), and how many go side by side.
const PICTURE: (f32, f32) = (160.0, 100.0);
const COLUMNS: usize = 3;

/// The web link `args` ask to open, if they ask only that: Chrome's own
/// options aside, one http or https link, and nothing said of a new
/// window. (A file isn't asked about: the extension can't open one in a
/// tab, so the window chosen couldn't be honoured.)
pub fn link_of(args: &[String]) -> Option<&str> {
    if args.iter().any(|a| matches!(a.as_str(), "--new-window" | "--incognito" | "--app") || a.starts_with("--app=")) {
        return None;
    }
    let mut links = args.iter().filter(|a| !a.starts_with('-'));
    let link = links.next()?;
    let web = ["http://", "https://"].iter().any(|p| link.get(..p.len()).is_some_and(|s| s.eq_ignore_ascii_case(p)));
    (web && links.next().is_none()).then_some(link.as_str())
}

/// Chrome's browser windows on this display, the one used last first: of
/// its browser process (`pid`, when known), not DevTools or a picture in
/// picture.
pub fn windows(pid: Option<u32>) -> Vec<WindowInfo> {
    let Ok(mut c) = Client::connect() else { return Vec::new() };
    let Ok(Reply::Windows { windows }) = c.request(&Request::ListWindows) else { return Vec::new() };
    browsers(windows, pid)
}

fn browsers(windows: Vec<WindowInfo>, pid: Option<u32>) -> Vec<WindowInfo> {
    let mut w: Vec<WindowInfo> = windows
        .into_iter()
        .filter(|w| BROWSERS.contains(&w.app_id.as_str()) && pid.is_none_or(|p| w.pid == Some(p as i32)) && !w.title.starts_with("DevTools") && w.title != "Picture in picture")
        .collect();
    // (The front one first: it's the one Chrome would have used.)
    w.sort_by_key(|w| (w.minimized, w.stack.unwrap_or(u32::MAX), w.id));
    w
}

/// A window's title without its app's name.
fn short_title(title: &str) -> String {
    for tail in [" - Google Chrome", " – Google Chrome", " - Chromium"] {
        if let Some(t) = title.strip_suffix(tail) {
            return t.to_owned();
        }
    }
    title.to_owned()
}

/// The letter for the `i`th window: A to Z, then none.
fn letter(i: usize) -> Option<String> {
    (i < 26).then(|| char::from(b'A' + i as u8).to_string())
}

/// Bring window `id` forward, and wait (a second at most) until app_server
/// has it in front, then a moment for Chrome to have taken it as its
/// active window.
pub fn bring_forward(id: u64) {
    let Ok(mut c) = Client::connect() else { return };
    if c.request(&Request::Activate { id }).is_err() {
        return;
    }
    let until = Instant::now() + Duration::from_secs(1);
    while Instant::now() < until {
        if let Ok(Reply::Windows { windows }) = c.request(&Request::ListWindows)
            && windows.iter().any(|w| w.id == id && w.focused)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(CHROME_TAKES);
}

/// How long Chrome is given to take a window brought forward as the one
/// it has the focus in (only what tells two windows alike apart).
const CHROME_TAKES: Duration = Duration::from_millis(150);

/// Have Chrome (its browser process `pid`) open `link` in a new tab of
/// `window` (the one just brought forward), through Olo's extension (the
/// host's `open-in-front`): whether it was asked. The extension knows
/// Chrome's windows by its own ids, so it's told this one's title and
/// size, as Agent Screens pairs them; of two alike, the one Chrome has
/// the focus in. Chrome's own hand-over of a link opens it in a window of
/// its choosing, whichever is in front (tried: always the window it last
/// opened a link in).
pub fn open_in_front(pid: u32, link: &str, window: &WindowInfo) -> bool {
    let (width, height) = window.content.as_ref().map_or((0, 0), |c| (c.width, c.height));
    let ask = serde_json::json!({ "url": link, "title": window.title, "width": width, "height": height });
    olo_kit_base::commands::invoke(pid, &format!("open-in-front {ask}"))
}

/// Ask which of `windows` to open `link` in, in a dialog that's this
/// program's only window: the answer once it's gone.
pub fn ask(link: &str, windows: Vec<WindowInfo>) -> Choice {
    let out = Arc::new(Mutex::new(Choice::Cancel));
    let (answer, link) = (out.clone(), link.to_owned());
    olo_kit::run(crate::APP_ID, move |_| {
        let k = windows.len().clamp(1, COLUMNS);
        let width = olo_kit::widgets::alert::ALERT_TEXT_X + grid_width(k) + m::WINDOW_INSET;
        olo_kit::open_window(WindowOptions::alert("Open Link", width.ceil(), 300.0), move |cx| {
            cx.activate_window();
            Chooser::new(link, windows, answer, cx)
        });
    });
    out.lock().unwrap().clone()
}

/// The grid's width with `k` pictures side by side.
fn grid_width(k: usize) -> f32 {
    let cell = thumbs::cell_size(PICTURE).0;
    k as f32 * cell + (k.saturating_sub(1)) as f32 * m::SPACING
}

/// A window's picture, come.
pub struct Thumb {
    id: u64,
    image: Arc<RenderImage>,
}

struct Chooser {
    link: String,
    windows: Vec<WindowInfo>,
    pictures: HashMap<u64, Arc<RenderImage>>,
    selected: usize,
    answer: Arc<Mutex<Choice>>,
    /// The pictures' size in device pixels, and the worker making them.
    pixels: (u32, u32),
    live: Option<Live>,
}

impl Chooser {
    fn new(link: String, windows: Vec<WindowInfo>, answer: Arc<Mutex<Choice>>, cx: &mut Cx<Self>) -> Chooser {
        let s = cx.scale_factor();
        let pixels = ((PICTURE.0 * s).round() as u32, (PICTURE.1 * s).round() as u32);
        let items = windows
            .iter()
            .map(|w| thumbnails::Item { key: w.id.to_string(), source: Source::Window { id: w.id, app_id: w.app_id.clone(), title: w.title.clone() }, every: Duration::from_secs(1), size: pixels })
            .collect();
        let to_ui = cx.messenger();
        let live = Live::start(items, move |p| {
            if let (Ok(id), Some(image)) = (p.key.parse(), olo_kit::pictures::from_bgra(p.width, p.height, p.bgra)) {
                to_ui.send(Thumb { id, image });
            }
        });
        Chooser { link, windows, pictures: HashMap::new(), selected: 0, answer, pixels, live: Some(live) }
    }

    /// Answered: the window goes (and with it the program, which then acts
    /// on the answer).
    fn finish(&mut self, choice: Choice, cx: &mut Cx<Self>) {
        *self.answer.lock().unwrap() = choice;
        self.live = None;
        cx.close_window();
    }

    fn open_in(&mut self, i: usize, cx: &mut Cx<Self>) {
        if let Some(w) = self.windows.get(i) {
            let id = w.id;
            self.finish(Choice::Window(id), cx);
        }
    }

    /// What the dialog says of the link: the address shortened (a long one
    /// to its start and its site).
    fn said(&self) -> String {
        let mut shown: String = self.link.chars().take(80).collect();
        if self.link.chars().count() > 80 {
            shown.push('…');
        }
        format!("Chrome has {} windows. Choose the one to open “{shown}” in: press its letter, or click it.", self.windows.len())
    }
}

impl View for Chooser {
    type Message = Thumb;

    fn message(&mut self, t: Thumb, cx: &mut Cx<Self>) {
        self.pictures.insert(t.id, t.image);
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        // (The pictures' device size again, the scale having changed.)
        let s = cx.scale_factor();
        let now = ((PICTURE.0 * s).round() as u32, (PICTURE.1 * s).round() as u32);
        if now != self.pixels {
            self.pixels = now;
            if let Some(live) = &self.live {
                for w in &self.windows {
                    live.set_size(&w.id.to_string(), now);
                }
            }
        }
        let items: Vec<Thumbnail> = self
            .windows
            .iter()
            .enumerate()
            .map(|(i, w)| Thumbnail { picture: self.pictures.get(&w.id).cloned(), color: None, label: SharedString::from(short_title(&w.title)), key: letter(i).map(SharedString::from) })
            .collect();
        let k = self.windows.len().clamp(1, COLUMNS);
        let grid = ThumbnailPicker::new("windows", items, Some(self.selected))
            .picture_size(PICTURE.0, PICTURE.1)
            .width(grid_width(k))
            .click_opens()
            .on_select(cx.listener(|c: &mut Chooser, i: &usize, cx| {
                c.selected = *i;
                cx.invalidate();
            }))
            .on_open(cx.listener(|c: &mut Chooser, i: &usize, cx| c.open_in(*i, cx)));
        let cancel = Button::new("Cancel").on_click(cx.listener(|c: &mut Chooser, _: &ClickEvent, cx| c.finish(Choice::Cancel, cx)));
        let new_window = Button::new("New Window").on_click(cx.listener(|c: &mut Chooser, _: &ClickEvent, cx| c.finish(Choice::NewWindow, cx)));
        // (Return opens it in the one chosen: the default button.)
        let open = Button::new("Open").default(true).on_click(cx.listener(|c: &mut Chooser, _: &ClickEvent, cx| {
            let i = c.selected;
            c.open_in(i, cx);
        }));
        div()
            .size_full()
            .on_window_key_down(cx.listener(|c: &mut Chooser, e: &KeyDownEvent, cx| {
                let k = &e.keystroke;
                if k.modifiers.modified() {
                    return;
                }
                if k.key == "escape" {
                    c.finish(Choice::Cancel, cx);
                    return;
                }
                // A window's letter, wherever the keyboard is.
                if let [b] = k.key.as_bytes()
                    && b.is_ascii_lowercase()
                {
                    let i = usize::from(b - b'a');
                    if i < c.windows.len() {
                        c.open_in(i, cx);
                    }
                }
            }))
            .child(
                AlertFrame::new("google-chrome")
                    .child(alert_title("Open the Link in Which Window?"))
                    .child(div().mt(px(m::TIGHT)).child(paragraph(self.said())))
                    .child(div().h(px(m::SPACING * 2.0)))
                    .child(grid)
                    .child(div().h(px(m::SPACING * 2.0)))
                    .child(div().flex_1())
                    .child(button_row().child(cancel).child(new_window).child(open)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: u64, app_id: &str, title: &str, pid: i32, stack: Option<u32>) -> WindowInfo {
        serde_json::from_value(serde_json::json!({"id": id, "app_id": app_id, "title": title, "pid": pid, "workspace": 0, "minimized": false, "focused": false, "stack": stack})).unwrap()
    }

    #[test]
    fn the_link_and_the_windows() {
        assert_eq!(link_of(&["https://example.com".into()]), Some("https://example.com"));
        assert_eq!(link_of(&["--ozone-platform=wayland".into(), "https://example.com".into()]), Some("https://example.com"));
        assert_eq!(link_of(&["--new-window".into(), "https://example.com".into()]), None);
        assert_eq!(link_of(&[]), None);
        assert_eq!(link_of(&["file:///tmp/a.html".into()]), None, "a file goes to Chrome as it is");
        assert_eq!(link_of(&["/home/me/a.html".into()]), None);
        assert_eq!(link_of(&["https://a.example".into(), "https://b.example".into()]), None);
        let all = vec![
            window(1, "google-chrome", "Inbox - Google Chrome", 40, Some(3)),
            window(2, "google-chrome", "Docs - Google Chrome", 40, Some(0)),
            window(3, "olo.slack", "Slack", 40, Some(1)),
            window(4, "google-chrome", "DevTools - example.com", 40, Some(2)),
            window(5, "google-chrome", "Other profile - Google Chrome", 41, Some(4)),
        ];
        let got: Vec<u64> = browsers(all, Some(40)).iter().map(|w| w.id).collect();
        assert_eq!(got, [2, 1], "the browser's windows, the front one first");
        assert_eq!(short_title("Docs - Google Chrome"), "Docs");
        assert_eq!((letter(0).as_deref(), letter(25).as_deref(), letter(26)), (Some("A"), Some("Z"), None));
    }
}
