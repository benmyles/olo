//! What follows a person's recording (`olo-screenshot --after PATH`,
//! which `--toggle-record` starts once the file is whole, in the saved
//! notification's place): a card in the screen's lower left, as CleanShot
//! X's quick-access overlay. The recording's picture, its name, how long
//! and how big it is, and Show in Tracker, Copy and Delete, with a close
//! box; the picture dragged out is the file (to a folder, a chat, a
//! mail). It goes by itself `STAYS` after the pointer left it (over it,
//! it stays), when closed, or once something was done with it. Copied,
//! the window goes and the process stays only to serve the clipboard,
//! until something else is copied.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use olo_kit::{
    Bounds, BoxShadow, ClickEvent, Corners, Cx, DragAction, Element, InteractiveElement as _, LayerKeyboard, LayerShellLayer, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels,
    Point, RenderImage, Styled as _, View, WindowOptions, canvas, div, dnd, held_clipboard, hsla, metrics as m, point, px, size, style, theme,
    widgets::{Button, SMALL_BUTTON_HEIGHT, StringView},
};

const APP_ID: &str = "olo.screenshot-after";
const NAMESPACE: &str = "screenshot-after";
/// The picture's frame (points): this wide, as tall as the video's shape
/// asks, at most `PICTURE_MOST_H` (narrower then).
const PICTURE_W: f32 = 280.0;
const PICTURE_MOST_H: f32 = 160.0;
/// From the screen's edges to the card.
const MARGIN: f32 = 16.0;
/// Room round the card for its shadow.
const SHADOW: f32 = 12.0;
/// How long it stays once the pointer has left it.
const STAYS: Duration = Duration::from_secs(20);
/// How far a press moves before it's a drag (points).
const DRAG_AFTER: f32 = 4.0;

enum Msg {
    /// The video's picture, and its size (device pixels) and scale made
    /// for; or none (it couldn't be read).
    Picture(Option<(f32, Arc<RenderImage>)>),
    /// The original picture, to make again at another scale.
    Original(Arc<image::RgbaImage>),
    /// `STAYS` since the pointer last came or went (the wait numbered
    /// so: an older one is dropped).
    Leave(u64),
    /// The file dragged out was dropped as this (None: nowhere).
    Dropped(Option<DragAction>),
}

struct After {
    path: PathBuf,
    name: String,
    /// "0:12 · 1280 × 800 · 4.2 MB".
    details: String,
    /// The video's shape (width over height).
    aspect: f32,
    original: Option<Arc<image::RgbaImage>>,
    /// The picture as made, for which scale.
    picture: Option<(f32, Arc<RenderImage>)>,
    /// A picture being made for this scale.
    asked: Option<f32>,
    close_lit: bool,
    /// The pointer is over it.
    hovered: bool,
    /// The wait for its going that counts (`Msg::Leave`).
    leaving: u64,
    /// Where a press on the picture began (points in the window).
    press: Option<Point<Pixels>>,
    /// The size the window was given (points).
    size: (f32, f32),
    /// Copied: the window is gone, the process serves the clipboard.
    holding: bool,
}

/// `m:ss` (or `h:mm:ss`).
fn length(seconds: f64) -> String {
    let s = seconds.round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// "4.2 MB".
fn bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / f64::from(1u32 << 30)),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / f64::from(1u32 << 20)),
        n if n >= 1 << 10 => format!("{} KB", n >> 10),
        n => format!("{n} bytes"),
    }
}

/// The picture's frame for `aspect` (points): `PICTURE_W` wide, or
/// narrower if it would be taller than `PICTURE_MOST_H`; whole points.
fn picture_size(aspect: f32) -> (f32, f32) {
    let aspect = if aspect.is_finite() && aspect > 0.0 { aspect } else { 16.0 / 10.0 };
    let h = (PICTURE_W / aspect).min(PICTURE_MOST_H);
    ((h * aspect).round().min(PICTURE_W), h.round())
}

/// The card's size (points), its parts' positions following from it.
fn card_size(aspect: f32) -> (f32, f32) {
    let (_, ph) = picture_size(aspect);
    (PICTURE_W + 2.0 * m::CARD_INSET, m::CARD_INSET + ph + m::TIGHT + 2.0 * m::LABEL_ROW + m::TIGHT + SMALL_BUTTON_HEIGHT + m::CARD_INSET)
}

/// Move `path` to the Trash (freedesktop's: Tracker restores it).
pub(crate) fn to_trash(path: &Path) -> std::io::Result<PathBuf> {
    let (target, info) = olo_xdg::trash::trash_slot(path)?;
    let moved = std::fs::rename(path, &target).or_else(|_| std::fs::copy(path, &target).and_then(|_| std::fs::remove_file(path)));
    if moved.is_err() {
        let _ = std::fs::remove_file(&info);
    }
    moved.map(|()| target)
}

impl After {
    /// The picture made for `scale` on a worker (exact device pixels).
    fn ask_picture(&mut self, scale: f32, cx: &mut Cx<Self>) {
        let Some(original) = self.original.clone() else { return };
        if self.asked == Some(scale) || self.picture.as_ref().is_some_and(|(s, _)| *s == scale) {
            return;
        }
        self.asked = Some(scale);
        let (w, h) = picture_size(self.aspect);
        // (Inside the frame's 2-point bevel.)
        let (dw, dh) = (((w - 4.0) * scale).round() as u32, ((h - 4.0) * scale).round() as u32);
        cx.work(move || crate::preview_of(&original, dw, dh).map(|i| (scale, i)), Msg::Picture);
    }

    /// It goes `STAYS` after the pointer left: asked then whether the
    /// pointer is over it, and again later if it is.
    fn hold(&mut self, cx: &mut Cx<Self>) {
        self.leaving += 1;
        cx.after(STAYS, Msg::Leave(self.leaving));
    }

    fn show(&mut self) {
        let _ = olo_kit::tracker::open(&self.path);
        olo_kit::quit();
    }

    /// The file on the clipboard (as a file manager copies it); the window
    /// goes, the process serving it till something else is copied.
    fn copy(&mut self, cx: &mut Cx<Self>) {
        self.holding = true;
        // (No window from now on, and still running.)
        olo_kit::stay_resident();
        let clip = held_clipboard::files(std::slice::from_ref(&self.path), false);
        std::thread::spawn(move || {
            if let Err(e) = held_clipboard::hold(clip) {
                eprintln!("olo-screenshot: couldn't copy the recording: {e}");
            }
            olo_kit::quit();
        });
        cx.close_window();
    }

    /// Edit: the recording in the Viewer's editor (`olo-viewer --edit`).
    fn edit(&mut self) {
        match olo_xdg::find_program("olo-viewer") {
            Some(viewer) => {
                let argv = [viewer.into_os_string(), "--edit".into(), self.path.clone().into_os_string()];
                if let Err(e) = olo_xdg::open::spawn(&argv) {
                    eprintln!("olo-screenshot: couldn't open the editor: {e}");
                }
            }
            None => eprintln!("olo-screenshot: no Viewer to edit it in"),
        }
        olo_kit::quit();
    }

    fn delete(&mut self) {
        if let Err(e) = to_trash(&self.path) {
            eprintln!("olo-screenshot: couldn't move {} to the Trash: {e}", self.path.display());
        }
        olo_kit::quit();
    }

    /// A press on the picture moved far enough: the file dragged out.
    fn drag(&mut self, cx: &mut Cx<Self>) {
        self.press = None;
        let mut drag = dnd::files(std::slice::from_ref(&self.path), true, false);
        drag.icon = dnd::picture(cx, "video-x-generic", 48.0, Some(&self.name), point(px(24.0), px(24.0)));
        dnd::start(cx, drag, Msg::Dropped);
    }
}

impl View for After {
    type Message = Msg;

    fn message(&mut self, m: Msg, cx: &mut Cx<Self>) {
        match m {
            Msg::Picture(made) => {
                self.asked = None;
                self.picture = made;
                cx.invalidate();
            }
            Msg::Original(original) => {
                self.original = Some(original);
                self.ask_picture(cx.scale_factor(), cx);
            }
            Msg::Leave(n) if n == self.leaving && !self.holding => {
                if self.hovered {
                    self.hold(cx);
                } else {
                    olo_kit::quit();
                }
            }
            Msg::Leave(_) => {}
            // Dropped somewhere: done with.
            Msg::Dropped(Some(_)) => olo_kit::quit(),
            // (A drag takes the pointer without its leaving being told:
            // it isn't over the card now, wherever it was let go.)
            Msg::Dropped(None) => {
                self.hovered = false;
                self.hold(cx);
            }
        }
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let scale = cx.scale_factor();
        self.ask_picture(scale, cx);
        let (cw, ch) = card_size(self.aspect);
        let (w, h) = (cw + 2.0 * SHADOW, ch + 2.0 * SHADOW);
        if (w, h) != self.size {
            self.size = (w, h);
            cx.resize_window(size(px(w), px(h)));
        }
        let (pw, ph) = picture_size(self.aspect);
        let card_at = point(px(SHADOW), px(SHADOW * 0.6));
        let frame_at = card_at + point(px(m::CARD_INSET + (PICTURE_W - pw) / 2.0), px(m::CARD_INSET));
        let text_y = SHADOW * 0.6 + m::CARD_INSET + ph + m::TIGHT;
        let close = 16.0;
        let close_at = point(card_at.x + px(cw - m::CARD_INSET - close + 4.0), px(text_y + (m::LABEL_ROW - close) / 2.0));
        let ink = style::card_ink(&p);
        let back = cx
            .canvas(move |a: &After, g, b| {
                let p = theme::palette();
                let card = Bounds::new(b.origin + card_at, size(px(cw), px(ch)));
                // Over whatever is on the screen: a shadow that lifts it,
                // then the card's own.
                let radius = Corners::all(px(m::CARD_RADIUS));
                g.paint_shadows(card, radius, &[BoxShadow { color: hsla(0.0, 0.0, 0.0, 0.28), offset: point(px(0.0), px(3.0)), blur_radius: px(SHADOW * 0.7), spread_radius: px(0.0) }]);
                style::card(g, card, &p, false, false);
                let frame = style::snap(g, Bounds::new(b.origin + frame_at, size(px(pw), px(ph))));
                match a.picture.as_ref().filter(|(s, _)| *s == g.scale_factor()) {
                    Some((_, image)) => {
                        let inner = style::inset(g, frame, 2.0);
                        let _ = g.paint_image(inner, Corners::default(), image.clone(), 0, false);
                    }
                    None => style::fill_rect(g, frame, style::c(p.panel_background)),
                }
                style::sunken(g, frame, &p, false);
                olo_kit::agent::record(
                    g,
                    olo_kit::agent::Node { role: "Image", name: Some(format!("Recording: {} (drag it out as the file)", a.name).into()), bounds: frame, enabled: true, ..Default::default() },
                );
                let close = Bounds::new(b.origin + close_at, size(px(close), px(close)));
                style::close_box(g, close, if a.close_lit { style::c(p.panel_text) } else { style::disabled_text(&p) });
            })
            .absolute()
            .left(px(0.0))
            .top(px(0.0))
            .size_full();
        div()
            .id("after")
            .size_full()
            .relative()
            // (Its time starts again whenever the pointer comes or goes.)
            .on_hover(cx.listener(|a: &mut After, inside: &bool, cx| {
                a.hovered = *inside;
                a.hold(cx);
            }))
            .child(back)
            // The picture: pressed and moved, the file dragged out.
            .child(
                div()
                    .id("picture")
                    .absolute()
                    .left(frame_at.x)
                    .top(frame_at.y)
                    .w(px(pw))
                    .h(px(ph))
                    .on_mouse_down(MouseButton::Left, cx.listener(|a: &mut After, e: &MouseDownEvent, _| a.press = Some(e.position)))
                    .moves_matter_while_pressed()
                    .on_mouse_move(cx.listener(|a: &mut After, e: &MouseMoveEvent, cx| {
                        let Some(at) = a.press else { return };
                        if e.pressed_button != Some(MouseButton::Left) {
                            a.press = None;
                        } else if f32::from((e.position - at).x).hypot(f32::from((e.position - at).y)) >= DRAG_AFTER {
                            a.drag(cx);
                        }
                    })),
            )
            .child(
                div()
                    .absolute()
                    .left(card_at.x + px(m::CARD_INSET))
                    .top(px(text_y))
                    .w(px(PICTURE_W - close - m::TIGHT))
                    .flex()
                    .flex_col()
                    .child(div().h(px(m::LABEL_ROW)).child(StringView::new(self.name.clone()).color(ink.text)))
                    .child(div().h(px(m::LABEL_ROW)).child(StringView::new(self.details.clone()).dim(true))),
            )
            .child(
                div()
                    .id("close")
                    .absolute()
                    .left(close_at.x)
                    .top(close_at.y)
                    .w(px(close))
                    .h(px(close))
                    .child(canvas(|g, b| olo_kit::agent::record(g, olo_kit::agent::Node::button("Close", b))).size_full())
                    .on_hover(cx.listener(|a: &mut After, inside: &bool, cx| {
                        a.close_lit = *inside;
                        cx.invalidate();
                    }))
                    .on_click(|_: &ClickEvent, _| olo_kit::quit()),
            )
            .child(
                div()
                    .absolute()
                    .left(card_at.x + px(m::CARD_INSET))
                    .top(px(text_y + 2.0 * m::LABEL_ROW + m::TIGHT))
                    .flex()
                    .gap(px(m::TIGHT))
                    .child(Button::new("Show in Tracker").small().on_click(cx.listener(|a: &mut After, _: &ClickEvent, _| a.show())))
                    .child(Button::new("Copy").small().on_click(cx.listener(|a: &mut After, _: &ClickEvent, cx| a.copy(cx))))
                    .child(Button::new("Edit").small().on_click(cx.listener(|a: &mut After, _: &ClickEvent, _| a.edit())))
                    .child(Button::new("Delete").small().on_click(cx.listener(|a: &mut After, _: &ClickEvent, _| a.delete()))),
            )
    }
}

/// `--after PATH`: the card, till it goes.
pub fn run(args: &[String]) -> std::process::ExitCode {
    let Some(path) = args.iter().position(|a| a == "--after").and_then(|i| args.get(i + 1)).map(PathBuf::from) else {
        eprintln!("olo-screenshot: --after takes the recording's path");
        return std::process::ExitCode::from(2);
    };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let input = olo_preview::input::Input::file(&path);
    // What it is (quick: its index), here; its picture on a worker.
    let gif = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("gif"));
    let (seconds, width, height) = if gif {
        let dims = image::ImageReader::open(&path).ok().and_then(|r| r.with_guessed_format().ok()).and_then(|r| r.into_dimensions().ok()).unwrap_or((0, 0));
        (None, dims.0, dims.1)
    } else {
        match olo_preview::video::probe(&input) {
            Ok(info) => (info.duration, info.width, info.height),
            Err(_) => (None, 0, 0),
        }
    };
    let mut details = Vec::new();
    if let Some(s) = seconds {
        details.push(length(s));
    }
    if width > 0 {
        details.push(format!("{width} × {height}"));
    }
    details.push(bytes(size));
    let aspect = if height > 0 { width as f32 / height as f32 } else { 16.0 / 10.0 };
    olo_kit::run(APP_ID, move |_| {
        let (cw, ch) = card_size(aspect);
        let options = WindowOptions::new("Recording Saved", cw + 2.0 * SHADOW, ch + 2.0 * SHADOW).no_panel().layer(NAMESPACE, LayerShellLayer::Top, (false, true, true, false)).layer_options(|l| {
            // (The card `MARGIN` from the edges: its shadow's room inside.)
            l.margin = (0, 0, (MARGIN - SHADOW * 1.4).max(0.0).round() as i32, (MARGIN - SHADOW).round() as i32);
            l.keyboard = LayerKeyboard::None;
        });
        let shown = path.clone();
        let window = olo_kit::open_window(options, move |cx| {
            let mut after = After {
                path: shown,
                name,
                details: details.join(" · "),
                aspect,
                original: None,
                picture: None,
                asked: None,
                close_lit: false,
                hovered: false,
                leaving: 0,
                press: None,
                size: (0.0, 0.0),
                holding: false,
            };
            after.hold(cx);
            after
        });
        // The picture: a moment in (past a countdown's last frame).
        let to = window.messenger();
        std::thread::spawn(move || {
            let picture = if gif {
                image::open(&path).ok().map(|i| i.into_rgba8())
            } else {
                let at = seconds.map_or(0.0, |s| (s * 0.1).min(1.0));
                olo_preview::video::picture_at(&input, at).ok().and_then(|b| image::RgbaImage::from_raw(b.size[0] as u32, b.size[1] as u32, b.rgba.clone()))
            };
            match picture {
                Some(p) => to.send(Msg::Original(Arc::new(p))),
                None => to.send(Msg::Picture(None)),
            };
        });
    });
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_how_long_and_how_big() {
        assert_eq!(length(4.4), "0:04");
        assert_eq!(length(61.6), "1:02");
        assert_eq!(length(3725.0), "1:02:05");
        assert_eq!(bytes(900), "900 bytes");
        assert_eq!(bytes(52 * 1024), "52 KB");
        assert_eq!(bytes(4_400_000), "4.2 MB");
    }

    /// The picture's frame follows the video's shape: as wide as it can
    /// be, a tall one held to `PICTURE_MOST_H` and narrower.
    #[test]
    fn the_picture_frame_follows_the_video() {
        assert_eq!(picture_size(16.0 / 10.0), (256.0, 160.0));
        assert_eq!(picture_size(16.0 / 9.0), (280.0, 158.0));
        let (w, h) = picture_size(9.0 / 16.0);
        assert_eq!(h, PICTURE_MOST_H);
        assert_eq!(w, 90.0);
        // Nothing known: a screen's shape.
        assert_eq!(picture_size(f32::NAN), (256.0, 160.0));
    }
}
