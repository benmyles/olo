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

use olo_ui::{
    WindowOptions,
    gpui::{
        self, AppContext as _, Context, Corners, InteractiveElement as _, IntoElement, LayerKeyboard, LayerShellLayer, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels, Point,
        Render, RenderImage, StatefulInteractiveElement as _, Styled as _, Task, Window, canvas, div, point, px, size,
    },
    metrics as m, style, theme,
    widgets::{
        StringView,
        buttons::{Button, SMALL_BUTTON_HEIGHT},
    },
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
    /// Something else is on the clipboard: nothing more to serve.
    Released,
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
    dropped: Vec<Arc<RenderImage>>,
    close_lit: bool,
    /// Its going, once the pointer has left it.
    leaving: Option<Task<()>>,
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
fn to_trash(path: &Path) -> std::io::Result<()> {
    let (target, info) = olo_xdg::trash::trash_slot(path)?;
    let moved = std::fs::rename(path, &target).or_else(|_| std::fs::copy(path, &target).and_then(|_| std::fs::remove_file(path)));
    if moved.is_err() {
        let _ = std::fs::remove_file(&info);
    }
    moved
}

impl After {
    fn message(&mut self, m: Msg, window: &mut Window, cx: &mut Context<Self>) {
        match m {
            Msg::Picture(made) => {
                self.asked = None;
                self.dropped.extend(std::mem::replace(&mut self.picture, made).map(|(_, i)| i));
                cx.notify();
            }
            Msg::Original(original) => {
                self.original = Some(original);
                self.ask_picture(window.scale_factor(), cx);
            }
            Msg::Released => cx.quit(),
        }
    }

    /// The picture made for `scale` on a worker (exact device pixels).
    fn ask_picture(&mut self, scale: f32, cx: &mut Context<Self>) {
        let Some(original) = self.original.clone() else { return };
        if self.asked == Some(scale) || self.picture.as_ref().is_some_and(|(s, _)| *s == scale) {
            return;
        }
        self.asked = Some(scale);
        let (w, h) = picture_size(self.aspect);
        // (Inside the frame's 2-point bevel.)
        let (dw, dh) = (((w - 4.0) * scale).round() as u32, ((h - 4.0) * scale).round() as u32);
        let proxy = olo_ui::proxy(cx);
        std::thread::spawn(move || proxy.send(Msg::Picture(crate::preview_of(&original, dw, dh).map(|i| (scale, i)))));
    }

    /// It goes `STAYS` after the pointer left: asked then whether the
    /// pointer is over it (a drag out of it takes the pointer without its
    /// leaving being told), and again later if it is.
    fn hold(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.leaving = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(STAYS).await;
            let _ = this.update_in(cx, |a, window, cx| {
                if a.holding {
                    return;
                }
                if window.is_window_hovered() {
                    a.hold(window, cx);
                } else {
                    cx.quit();
                }
            });
        }));
    }

    fn show(&mut self, cx: &mut Context<Self>) {
        let _ = olo_ui::tracker::open(&self.path);
        cx.quit();
    }

    /// The file on the clipboard (as a file manager copies it); the window
    /// goes, the process serving it till something else is copied.
    fn copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.holding = true;
        // (No window from now on, and still running.)
        olo_ui::stay_resident(cx);
        let clip = olo_ui::clipboard::files(std::slice::from_ref(&self.path), false);
        let proxy = olo_ui::proxy(cx);
        std::thread::spawn(move || {
            if let Err(e) = olo_ui::clipboard::hold(clip) {
                eprintln!("olo-screenshot: couldn't copy the recording: {e}");
            }
            proxy.send(Msg::Released);
        });
        window.remove_window();
    }

    /// Edit: the recording in the Viewer's editor (`olo-viewer --edit`).
    fn edit(&mut self, cx: &mut Context<Self>) {
        match olo_xdg::find_program("olo-viewer") {
            Some(viewer) => {
                let argv = [viewer.into_os_string(), "--edit".into(), self.path.clone().into_os_string()];
                if let Err(e) = olo_xdg::open::spawn(&argv) {
                    eprintln!("olo-screenshot: couldn't open the editor: {e}");
                }
            }
            None => eprintln!("olo-screenshot: no Viewer to edit it in"),
        }
        cx.quit();
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = to_trash(&self.path) {
            eprintln!("olo-screenshot: couldn't move {} to the Trash: {e}", self.path.display());
        }
        cx.quit();
    }

    /// A press on the picture moved far enough: the file dragged out.
    fn drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.press = None;
        let mut drag = olo_ui::dnd::files(std::slice::from_ref(&self.path), true, false);
        drag.icon = olo_ui::dnd::picture(window, cx, "video-x-generic", 48.0, Some(&self.name), point(px(24.0), px(24.0)));
        olo_ui::dnd::start(window, cx, drag, |action, cx| {
            // Dropped somewhere: done with.
            if action.is_some() {
                cx.quit();
            }
        });
    }
}

impl Render for After {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        let scale = window.scale_factor();
        self.ask_picture(scale, cx);
        let (cw, ch) = card_size(self.aspect);
        let (w, h) = (cw + 2.0 * SHADOW, ch + 2.0 * SHADOW);
        if (w, h) != self.size {
            self.size = (w, h);
            window.defer(cx, move |window, _| window.resize(size(px(w), px(h))));
        }
        let (pw, ph) = picture_size(self.aspect);
        let picture = self.picture.as_ref().filter(|(s, _)| *s == scale).map(|(_, i)| i.clone());
        let dropped = std::mem::take(&mut self.dropped);
        let (name, close_lit) = (self.name.clone(), self.close_lit);
        let me = cx.entity();
        let card_at = point(px(SHADOW), px(SHADOW * 0.6));
        let frame_at = card_at + point(px(m::CARD_INSET + (PICTURE_W - pw) / 2.0), px(m::CARD_INSET));
        let text_y = SHADOW * 0.6 + m::CARD_INSET + ph + m::TIGHT;
        let close = 16.0;
        let close_at = point(card_at.x + px(cw - m::CARD_INSET - close + 4.0), px(text_y + (m::LABEL_ROW - close) / 2.0));
        let ink = style::card_ink(&p);
        let (hover_me, press_me, move_me, close_me, lit_me) = (me.clone(), me.clone(), me.clone(), me.clone(), me.clone());
        let action = |f: fn(&mut After, &mut Window, &mut Context<After>)| {
            let me = me.clone();
            move |window: &mut Window, cx: &mut gpui::App| me.update(cx, |a, cx| f(a, window, cx))
        };
        div()
            .id("after")
            .size_full()
            .relative()
            // (Its time starts again whenever the pointer comes or goes.)
            .on_hover(move |_, window, cx| hover_me.update(cx, |a, cx| a.hold(window, cx)))
            .child(
                canvas(
                    |_, _, _| {},
                    move |b, _, window, cx| {
                        let p = theme::palette(cx);
                        for image in dropped {
                            let _ = window.drop_image(image);
                        }
                        let card = gpui::Bounds::new(b.origin + card_at, size(px(cw), px(ch)));
                        // Over whatever is on the screen: a shadow that
                        // lifts it, then the card's own.
                        let radius = Corners::all(px(m::CARD_RADIUS));
                        window.paint_shadows(
                            card,
                            radius,
                            &[gpui::BoxShadow { color: gpui::hsla(0.0, 0.0, 0.0, 0.28), offset: point(px(0.0), px(3.0)), blur_radius: px(SHADOW * 0.7), spread_radius: px(0.0) }],
                        );
                        style::card(window, card, &p, false, false);
                        let frame = style::snap(window, gpui::Bounds::new(b.origin + frame_at, size(px(pw), px(ph))));
                        match &picture {
                            Some(image) => {
                                let inner = style::inset(window, frame, 2.0);
                                let _ = window.paint_image(inner, Corners::default(), image.clone(), 0, false);
                            }
                            None => style::fill_rect(window, frame, style::c(p.panel_background)),
                        }
                        style::sunken(window, frame, &p, false);
                        olo_ui::agent::record(
                            window,
                            cx,
                            olo_ui::agent::Node { role: "Image", name: Some(format!("Recording: {name} (drag it out as the file)").into()), bounds: frame, enabled: true, ..Default::default() },
                        );
                        let close = gpui::Bounds::new(b.origin + close_at, size(px(close), px(close)));
                        style::close_box(window, close, if close_lit { style::c(p.panel_text) } else { style::disabled_text(&p) });
                    },
                )
                .absolute()
                .size_full(),
            )
            // The picture: pressed and moved, the file dragged out.
            .child(
                div()
                    .id("picture")
                    .absolute()
                    .left(frame_at.x)
                    .top(frame_at.y)
                    .w(px(pw))
                    .h(px(ph))
                    .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, _, cx| press_me.update(cx, |a, _| a.press = Some(e.position)))
                    .on_mouse_move(move |e: &MouseMoveEvent, window, cx| {
                        move_me.update(cx, |a, cx| {
                            let Some(at) = a.press else { return };
                            if e.pressed_button != Some(MouseButton::Left) {
                                a.press = None;
                            } else if f32::from((e.position - at).x).hypot(f32::from((e.position - at).y)) >= DRAG_AFTER {
                                a.drag(window, cx);
                            }
                        })
                    }),
            )
            .child(
                div()
                    .absolute()
                    .left(card_at.x + px(m::CARD_INSET))
                    .top(px(text_y))
                    .w(px(PICTURE_W - close - m::TIGHT))
                    .flex()
                    .flex_col()
                    .child(div().h(px(m::LABEL_ROW)).text_color(ink.text).child(StringView::new(self.name.clone())))
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
                    .child(canvas(|_, _, _| {}, move |b, _, window, cx| olo_ui::agent::record(window, cx, olo_ui::agent::Node::button("Close", b))).size_full())
                    .on_hover(move |inside, _, cx| {
                        lit_me.update(cx, |a, cx| {
                            a.close_lit = *inside;
                            cx.notify();
                        })
                    })
                    .on_click(move |_, _, cx| close_me.update(cx, |_, cx| cx.quit())),
            )
            .child(
                div()
                    .absolute()
                    .left(card_at.x + px(m::CARD_INSET))
                    .top(px(text_y + 2.0 * m::LABEL_ROW + m::TIGHT))
                    .flex()
                    .gap(px(m::TIGHT))
                    .child(Button::new("Show in Tracker").small().on_click(action(|a, _, cx| a.show(cx))))
                    .child(Button::new("Copy").small().on_click(action(|a, window, cx| a.copy(window, cx))))
                    .child(Button::new("Edit").small().on_click(action(|a, _, cx| a.edit(cx))))
                    .child(Button::new("Delete").small().on_click(action(|a, _, cx| a.delete(cx)))),
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
    olo_ui::run(APP_ID, move |cx| {
        let (cw, ch) = card_size(aspect);
        let options = WindowOptions::new("Recording Saved", cw + 2.0 * SHADOW, ch + 2.0 * SHADOW).no_panel().layer(NAMESPACE, LayerShellLayer::Top, (false, true, true, false)).layer_options(|l| {
            // (The card `MARGIN` from the edges: its shadow's room inside.)
            l.margin = (0, 0, (MARGIN - SHADOW * 1.4).max(0.0).round() as i32, (MARGIN - SHADOW).round() as i32);
            l.keyboard = LayerKeyboard::None;
        });
        let after = cx.new(|_| After {
            path: path.clone(),
            name,
            details: details.join(" · "),
            aspect,
            original: None,
            picture: None,
            asked: None,
            dropped: Vec::new(),
            close_lit: false,
            leaving: None,
            press: None,
            size: (0.0, 0.0),
            holding: false,
        });
        let view = after.clone();
        let handle = olo_ui::open_window(cx, options, move |window, cx| {
            view.update(cx, |a, cx| a.hold(window, cx));
            view
        });
        let messages = after.clone();
        olo_ui::message_handler(cx, move |m, cx| {
            let Ok(m) = m.downcast::<Msg>() else { return };
            // (Copied, the window has gone: only the clipboard's end comes.)
            if matches!(*m, Msg::Released) {
                cx.quit();
                return;
            }
            if let Some(h) = handle {
                let _ = h.update(cx, |_, window, cx| messages.update(cx, |a, cx| a.message(*m, window, cx)));
            }
        });
        // The picture: a moment in (past a countdown's last frame).
        let proxy = olo_ui::proxy(cx);
        std::thread::spawn(move || {
            let picture = if gif {
                image::open(&path).ok().map(|i| i.into_rgba8())
            } else {
                let at = seconds.map_or(0.0, |s| (s * 0.1).min(1.0));
                olo_preview::video::picture_at(&input, at).ok().and_then(|b| image::RgbaImage::from_raw(b.size[0] as u32, b.size[1] as u32, b.rgba.clone()))
            };
            match picture {
                Some(p) => proxy.send(Msg::Original(Arc::new(p))),
                None => proxy.send(Msg::Picture(None)),
            }
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
