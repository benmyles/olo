//! Screenshot, after Haiku's: takes a picture of the screen as it starts,
//! shows it, and offers to save it (any of several formats, into a folder
//! of your choice), copy it, or take a new one, of the whole screen or the
//! active window, with or without its border and the pointer, after a
//! delay. Our own window steps aside while a picture is taken. On the
//! GPUI kit (olo-ui).
//!
//! `--silent [--window [ID]]` (⌘⇧3, Print Screen, Alt+Print Screen) saves one
//! straight into the folder, as `screenshotN`, with no window; `--region`
//! (⌘⇧4) lets you drag out part of the screen first (region.rs). However
//! it is saved, the file's path goes on the clipboard.

mod capture;
mod region;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use capture::{Format, Settings, Shot};
use olo_ui::{
    Frame, WindowOptions, clipboard,
    file_panel::{self, FilePanel, PanelResult},
    gpui::{
        self, App, AppContext as _, Bounds, ClipboardEntry, ClipboardItem, Context, Corners, Entity, Image, ImageFormat, IntoElement, KeyDownEvent, InteractiveElement as _, ParentElement as _, Pixels, Render, RenderImage, SharedString, Styled as _, Window, WindowHandle, canvas, div, point, px, size,
    },
    instance::{self, Launch},
    metrics as m, style, theme,
    widgets::{Button, CheckBox, MenuField, StringView, TextControl, TextField, TextFieldEvent, open_alert, separator, text_field},
};
use serde::{Deserialize, Serialize};

const APP_ID: &str = "olo.screenshot";

const SIZE: (f32, f32) = (580.0, 276.0);
/// The preview's share of the window.
const PREVIEW_W: f32 = 240.0;
/// Labels' column in the form.
const DIVIDER: f32 = 66.0;

enum Msg {
    /// A picture, or why not.
    Taken(Result<Shot, String>),
    /// The picture made small for the preview: which picture, the size in
    /// device pixels it was made for, and it.
    Preview(u64, (u32, u32), Arc<RenderImage>),
    Saved(Result<PathBuf, String>),
    Copied(Result<Vec<u8>, String>),
    /// The first free name in a folder.
    Named(PathBuf, String),
    /// Something else is on the clipboard now: the path given out after
    /// saving isn't needed any more.
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Busy {
    Taking,
    Saving,
    Copying,
}

struct Screenshot {
    window: Option<WindowHandle<Frame>>,
    settings: Settings,
    shot: Option<Shot>,
    /// Counts the pictures taken, so a late preview of an old one is
    /// dropped.
    taken: u64,
    /// The preview: its size in device pixels and the image.
    preview: Option<((u32, u32), Arc<RenderImage>)>,
    /// The preview size last asked for (picture, size).
    asked: Option<(u64, (u32, u32))>,
    name: Entity<TextField>,
    delay: Entity<TextField>,
    /// The name as we proposed it; typing over it keeps the user's.
    proposed: String,
    busy: Option<Busy>,
    /// What happened last ("Copied to the clipboard"), under the preview.
    note: String,
    /// Folders offered in "Save in:" besides the chosen one.
    folders: Vec<PathBuf>,
    /// Saved: the window is gone and the app stays only to hand out
    /// where the picture went.
    holding: bool,
    /// The window's keys (Return saves) when no field has them.
    focus: gpui::FocusHandle,
}

#[derive(Serialize, Deserialize)]
struct Saved {
    /// The picture being shown (in the runtime dir; kept across a reload).
    png: Option<PathBuf>,
    name: String,
}

fn folder_label(path: &Path) -> String {
    if path == capture::home() {
        "Home".into()
    } else {
        path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
    }
}

/// `image` made `w` × `h` (device pixels) on a worker, area-averaged, as
/// GPUI draws it (BGRA): drawn one pixel to a device pixel, never scaled.
fn preview_of(image: &image::RgbaImage, w: u32, h: u32) -> Option<Arc<RenderImage>> {
    let small = image::imageops::thumbnail(image, w.max(1), h.max(1));
    let mut bgra = small.into_raw();
    for p in bgra.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(w.max(1), h.max(1), bgra)?;
    Some(Arc::new(RenderImage::new([image::Frame::new(buffer)])))
}

/// The window's parts (logical points, in the window).
struct Layout {
    /// Where the preview may go.
    area: Bounds<Pixels>,
    /// The form's left and width.
    form_x: f32,
    form_w: f32,
    /// The bottom row of buttons' top.
    buttons_y: f32,
}

fn layout() -> Layout {
    let (w, h) = SIZE;
    let inset = m::WINDOW_INSET;
    let buttons_y = h - inset - m::CONTROL_HEIGHT;
    let top_h = buttons_y - inset - inset;
    let area = Bounds { origin: point(px(inset), px(inset)), size: size(px(PREVIEW_W), px(top_h - m::LABEL_ROW - m::TIGHT)) };
    let form_x = inset + PREVIEW_W + inset + m::SPACING;
    Layout { area, form_x, form_w: w - inset - form_x, buttons_y }
}

/// The form's rows: each one's top and height, from the window's top.
/// Check boxes, then the delay; a rule; the name, the format and the
/// folder.
fn form_rows() -> [(f32, f32); 8] {
    let mut y = m::WINDOW_INSET;
    let mut rows = [(0.0, 0.0); 8];
    let heights = [m::MARK_ROW, m::MARK_ROW, m::MARK_ROW, m::CONTROL_HEIGHT, 6.0, m::CONTROL_HEIGHT, m::CONTROL_HEIGHT, m::CONTROL_HEIGHT];
    // The space before each row, after the gap between them all.
    let extra = [0.0, 0.0, 0.0, m::TIGHT, m::SPACING, m::SPACING, 0.0, 0.0];
    for i in 0..rows.len() {
        if i > 0 {
            y += m::TIGHT;
        }
        y += extra[i];
        rows[i] = (y, heights[i]);
        y += heights[i];
    }
    rows
}

impl Screenshot {
    fn new(settings: Settings, cx: &mut Context<Self>) -> Self {
        let mut folders = Vec::new();
        for f in [Some(capture::home()), olo_xdg::user_dir("DESKTOP"), olo_xdg::user_dir("PICTURES")].into_iter().flatten() {
            if !folders.contains(&f) {
                folders.push(f);
            }
        }
        let name = text_field(cx, |f| f.name("Name"));
        let delay = text_field(cx, |f| f.name("Delay"));
        delay.update(cx, |f, cx| f.set_text(settings.delay.to_string(), cx));
        cx.subscribe(&name, |s, _, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Entered => s.save(cx),
            _ => cx.notify(),
        })
        .detach();
        cx.subscribe(&delay, |s, field, e: &TextFieldEvent, cx| {
            if let TextFieldEvent::Changed = e {
                // Two digits at most, a minute at most.
                let text = field.read(cx).text().to_owned();
                let digits: String = text.chars().filter(char::is_ascii_digit).take(2).collect();
                if digits != text {
                    field.update(cx, |f, cx| f.set_text(digits.clone(), cx));
                }
                let d = digits.parse::<u32>().unwrap_or(0).min(60);
                if d != s.settings.delay {
                    s.settings.delay = d;
                    s.settings_changed(cx);
                }
            }
        })
        .detach();
        Screenshot { window: None, settings, shot: None, taken: 0, preview: None, asked: None, name, delay, proposed: String::new(), busy: None, note: String::new(), folders, holding: false, focus: cx.focus_handle() }
    }

    fn name_text(&self, cx: &App) -> String {
        self.name.read(cx).text().to_owned()
    }

    /// Take a picture on a worker; our window steps aside if it is up.
    fn take(&mut self, cx: &mut Context<Self>) {
        self.busy = Some(Busy::Taking);
        self.note.clear();
        let settings = self.settings.clone();
        let hide = self.window.is_some();
        let proxy = olo_ui::proxy(cx);
        std::thread::spawn(move || proxy.send(Msg::Taken(capture::take(&settings, hide))));
        cx.notify();
    }

    /// Show a picture taken earlier (after an in-place reload).
    fn reload(&mut self, png: PathBuf, cx: &mut Context<Self>) {
        self.busy = Some(Busy::Taking);
        let proxy = olo_ui::proxy(cx);
        std::thread::spawn(move || {
            let r = image::open(&png).map_err(|e| e.to_string()).map(|img| Shot { png, image: Arc::new(img.into_rgba8()) });
            proxy.send(Msg::Taken(r));
        });
    }

    /// Find the next free name in the folder, off the UI thread.
    fn propose_name(&self, cx: &mut Context<Self>) {
        let folder = self.settings.folder();
        let proxy = olo_ui::proxy(cx);
        std::thread::spawn(move || {
            let name = capture::next_name(&folder);
            proxy.send(Msg::Named(folder, name));
        });
    }

    fn open_main(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.window
            && w.update(cx, |_, window, _| window.activate_window()).is_ok()
        {
            return;
        }
        let me = cx.entity();
        let focus = self.focus.clone();
        cx.defer(move |cx| {
            let view = me.clone();
            let mut options = WindowOptions::new("Screenshot", SIZE.0, SIZE.1);
            options.fixed = true;
            let w = olo_ui::open_window(cx, options, move |window, _| {
                window.focus(&focus);
                view
            });
            me.update(cx, |s, _| s.window = w);
        });
    }

    fn show_error(&self, message: String, cx: &mut Context<Self>) {
        let me = cx.entity();
        cx.defer(move |cx| {
            let _ = me;
            open_alert(cx, "Screenshot", "warning", &message, &["OK"], |_, _| {});
        });
    }

    fn can_save(&self, cx: &App) -> bool {
        self.busy.is_none() && self.shot.is_some() && !self.name_text(cx).trim().is_empty()
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if !self.can_save(cx) {
            return;
        }
        let Some(shot) = self.shot.clone() else { return };
        let path = self.settings.folder().join(capture::file_name(&self.name_text(cx), self.settings.format));
        let format = self.settings.format;
        self.busy = Some(Busy::Saving);
        let proxy = olo_ui::proxy(cx);
        std::thread::spawn(move || {
            let r = capture::save(&shot, &path, format).map(|()| path);
            proxy.send(Msg::Saved(r));
        });
        cx.notify();
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        let Some(shot) = self.shot.clone() else { return };
        self.busy = Some(Busy::Copying);
        let proxy = olo_ui::proxy(cx);
        std::thread::spawn(move || {
            let r = std::fs::read(&shot.png).map_err(|e| e.to_string());
            proxy.send(Msg::Copied(r));
        });
        cx.notify();
    }

    fn settings_changed(&mut self, cx: &mut Context<Self>) {
        self.settings.store();
        // Keep a proposed name current; a typed one stays.
        if self.name_text(cx) == self.proposed {
            self.propose_name(cx);
        }
        cx.notify();
    }

    fn choose_folder(&mut self, cx: &mut Context<Self>) {
        let me = cx.entity();
        let start = self.settings.folder();
        cx.defer(move |cx| {
            file_panel::show(cx, "Choose a Folder", FilePanel::folder(start), move |a, cx| {
                if let PanelResult::Chosen(dir) = a.result {
                    me.update(cx, |s, cx| {
                        s.settings.folder = Some(dir);
                        s.settings_changed(cx);
                    });
                }
            });
        });
    }

    /// The preview's frame for the picture, on device pixels at `scale`:
    /// its outer bounds (the sunken frame two lines round it) and the
    /// picture's size inside, in device pixels.
    fn frame(&self, scale: f32) -> (Bounds<Pixels>, Option<(u32, u32)>) {
        let area = layout().area;
        let Some(shot) = &self.shot else { return (area, None) };
        let dev = |v: f32| (v * scale).round();
        // The sunken frame's two lines, in device pixels (as style::inset
        // takes them off).
        let edge = 2.0 * style::line_pixels(scale);
        // The space inside the frame, in device pixels.
        let (iw, ih) = (dev(f32::from(area.size.width)) - 2.0 * edge, dev(f32::from(area.size.height)) - 2.0 * edge);
        let (w, h) = (shot.image.width().max(1) as f32, shot.image.height().max(1) as f32);
        let k = (iw / w).min(ih / h);
        let (pw, ph) = ((w * k).round().max(1.0), (h * k).round().max(1.0));
        let left = dev(f32::from(area.left())) + edge + ((iw - pw) / 2.0).round();
        let top = dev(f32::from(area.top())) + edge;
        let outer = Bounds::from_corners(point(px((left - edge) / scale), px((top - edge) / scale)), point(px((left + pw + edge) / scale), px((top + ph + edge) / scale)));
        (outer, Some((pw as u32, ph as u32)))
    }

    /// Asks for a preview the size the frame wants now (once per picture
    /// and size: a new scale, a new picture).
    fn want_preview(&mut self, scale: f32, cx: &mut Context<Self>) {
        let (_, Some(want)) = self.frame(scale) else { return };
        if self.preview.as_ref().is_some_and(|(s, _)| *s == want) || self.asked == Some((self.taken, want)) {
            return;
        }
        let Some(shot) = self.shot.clone() else { return };
        self.asked = Some((self.taken, want));
        let (taken, proxy) = (self.taken, olo_ui::proxy(cx));
        std::thread::spawn(move || {
            if let Some(image) = preview_of(&shot.image, want.0, want.1) {
                proxy.send(Msg::Preview(taken, want, image));
            }
        });
    }

    fn message(&mut self, message: Box<dyn std::any::Any + Send>, cx: &mut Context<Self>) {
        if message.is::<Launch>() {
            // Launched again: bring the window forward.
            return self.open_main(cx);
        }
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Taken(r) => {
                self.busy = None;
                match r {
                    Ok(shot) => {
                        self.shot = Some(shot);
                        self.taken += 1;
                        let name = self.name_text(cx);
                        if name.is_empty() || name == self.proposed {
                            self.propose_name(cx);
                        }
                    }
                    Err(e) => self.show_error(format!("Couldn't take a screenshot.\n\n{e}"), cx),
                }
                self.open_main(cx);
            }
            Msg::Preview(taken, size, image) => {
                if taken == self.taken {
                    let old = self.preview.replace((size, image));
                    // The old one's texture goes.
                    if let (Some((_, old)), Some(w)) = (old, self.window) {
                        let _ = w.update(cx, |_, window, _| window.drop_image(old));
                    }
                }
            }
            Msg::Named(folder, name) => {
                let typed = self.name_text(cx);
                if folder == self.settings.folder() && (typed.is_empty() || typed == self.proposed) {
                    self.name.update(cx, |f, cx| f.set_text(name.clone(), cx));
                    self.proposed = name;
                }
            }
            Msg::Saved(r) => {
                self.busy = None;
                match r {
                    // Done, as in Haiku: the window goes, and where the
                    // picture went is on the clipboard (held, with no
                    // window, until something else is copied).
                    Ok(path) => {
                        self.holding = true;
                        if let Some(w) = self.window.take() {
                            cx.defer(move |cx| {
                                let _ = w.update(cx, |_, window, _| window.remove_window());
                            });
                        }
                        let _ = std::fs::remove_file(capture::scratch_path());
                        let proxy = olo_ui::proxy(cx);
                        std::thread::spawn(move || {
                            if let Err(e) = clipboard::hold(clipboard::text(&path.display().to_string())) {
                                tracing::warn!("couldn't put where it went on the clipboard: {e}");
                            }
                            proxy.send(Msg::Released);
                        });
                    }
                    Err(e) => self.show_error(format!("Couldn't save the screenshot.\n\n{e}"), cx),
                }
            }
            Msg::Copied(r) => {
                self.busy = None;
                match r {
                    Ok(png) => {
                        cx.write_to_clipboard(ClipboardItem::from_entries(vec![ClipboardEntry::Image(Image::from_bytes(ImageFormat::Png, png))]));
                        self.note = "Copied to the clipboard".into();
                    }
                    Err(e) => self.show_error(format!("Couldn't copy the screenshot.\n\n{e}"), cx),
                }
            }
            Msg::Released => cx.quit(),
        }
        cx.notify();
    }

    fn save_state(&self, cx: &App) -> Option<String> {
        serde_json::to_string(&Saved { png: self.shot.as_ref().map(|s| s.png.clone()), name: self.name_text(cx) }).ok()
    }
}

impl Render for Screenshot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = window.scale_factor();
        self.want_preview(scale, cx);
        let me = cx.entity();
        let l = layout();
        let rows = form_rows();
        let at = |x: f32, y: f32, w: f32| div().absolute().left(px(x)).top(px(y)).w(px(w));

        // The preview, framed at the picture's shape, its size under it.
        let (frame, _) = self.frame(scale);
        let preview = self.preview.as_ref().map(|(_, image)| image.clone());
        let caption = match (self.busy, &self.shot) {
            (Some(Busy::Taking), _) => "Taking a screenshot…".to_owned(),
            (Some(Busy::Saving), _) => "Saving…".to_owned(),
            (Some(Busy::Copying), _) => "Copying…".to_owned(),
            (None, _) if !self.note.is_empty() => self.note.clone(),
            (None, Some(s)) => format!("{} × {} pixels", s.image.width(), s.image.height()),
            (None, None) => String::new(),
        };
        // Almost level with a row of the form, it's lined up with it (a
        // little further from the picture); else it's on a grid of its
        // own, level with the form only at some shapes.
        let caption_y = f32::from(frame.bottom()) + m::TIGHT + m::LABEL_ROW / 2.0;
        let caption_y = rows.iter().map(|(top, h)| top + h / 2.0).find(|r| (0.0..=m::LABEL_ROW / 2.0).contains(&(r - caption_y))).unwrap_or(caption_y);
        let area = l.area;
        let picture = canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                let p = theme::palette(cx);
                if let Some(image) = &preview {
                    let inner = style::inset(window, frame, 2.0);
                    let _ = window.paint_image(inner, Corners::default(), image.clone(), 0, false);
                }
                style::sunken(window, frame, &p, false);
                let font = style::ui_font(gpui::FontWeight::NORMAL);
                style::label(window, cx, area.center().x, px(caption_y), style::Align::Center, caption.clone(), style::PLAIN_SIZE, font, style::disabled_text(&p));
            },
        )
        .absolute()
        .inset_0();

        // The form.
        let check = |label: &'static str, on: bool, enabled: bool, set: fn(&mut Settings, bool)| {
            let me = me.clone();
            CheckBox::new(label, on).enabled(enabled).on_toggle(move |v, _, cx| {
                me.update(cx, |s, cx| {
                    set(&mut s.settings, v);
                    s.settings_changed(cx);
                })
            })
        };
        let labels: Vec<SharedString> = Format::ALL.iter().map(|f| SharedString::from(f.label())).collect();
        let fi = Format::ALL.iter().position(|f| *f == self.settings.format).unwrap_or(0);
        // The chosen folder first if it isn't a usual one, then Choose
        // folder….
        let current = self.settings.folder();
        let mut folders = self.folders.clone();
        if !folders.contains(&current) {
            folders.insert(0, current.clone());
        }
        let mut names: Vec<SharedString> = folders.iter().map(|f| folder_label(f).into()).collect();
        names.push("Choose folder…".into());
        let di = folders.iter().position(|f| *f == current).unwrap_or(0);
        let (fx, fw) = (l.form_x, l.form_w);
        let (fm, dm) = (me.clone(), me.clone());
        let form = div()
            .child(at(fx, rows[0].0, fw).child(check("Capture active window", self.settings.window, true, |s, v| s.window = v)))
            .child(at(fx, rows[1].0, fw).child(check("Include window border", self.settings.border, self.settings.window, |s, v| s.border = v)))
            .child(at(fx, rows[2].0, fw).child(check("Include mouse pointer", self.settings.pointer, true, |s, v| s.pointer = v)))
            .child(at(fx, rows[3].0, fw).flex().items_center().gap(px(m::LABEL_GAP)).child(TextControl::new(self.delay.clone()).label("Delay:").divider(DIVIDER).width(DIVIDER + 44.0)).child(StringView::new("seconds")))
            .child(at(fx, rows[4].0, fw).child(separator()))
            .child(at(fx, rows[5].0, fw).child(TextControl::new(self.name.clone()).label("Name:").divider(DIVIDER).width(fw)))
            .child(at(fx, rows[6].0, fw).child(MenuField::new("format", labels, fi).label("Save as:").divider(DIVIDER).width(fw).on_change(move |i, _, cx| {
                fm.update(cx, |s, cx| {
                    s.settings.format = Format::ALL[i];
                    s.settings_changed(cx);
                })
            })))
            .child(at(fx, rows[7].0, fw).child(MenuField::new("folder", names, di).label("Save in:").divider(DIVIDER).width(fw).on_change(move |i, _, cx| {
                dm.update(cx, |s, cx| match folders.get(i) {
                    Some(f) => {
                        s.settings.folder = Some(f.clone());
                        s.settings_changed(cx);
                    }
                    None => s.choose_folder(cx),
                })
            })));

        // Buttons.
        let idle = self.busy.is_none() && self.shot.is_some();
        let can_save = self.can_save(cx);
        let (copy, new, save) = (me.clone(), me.clone(), me.clone());
        let (w, _) = SIZE;
        let bw = m::BUTTON_MIN_WIDTH;
        let save_x = w - m::WINDOW_INSET - bw;
        let new_x = save_x - m::SPACING - 2.0 - 120.0;
        let buttons = div()
            .child(at(m::WINDOW_INSET, l.buttons_y, 130.0).child(Button::new("Copy to clipboard").min_width(130.0).enabled(idle).on_click(move |_, cx| copy.update(cx, |s, cx| s.copy(cx)))))
            .child(at(new_x, l.buttons_y, 120.0).child(Button::new("New screenshot").min_width(120.0).enabled(self.busy.is_none()).on_click(move |_, cx| new.update(cx, |s, cx| s.take(cx)))))
            .child(at(save_x, l.buttons_y, bw).child(Button::new("Save").default(true).min_width(bw).enabled(can_save).on_click(move |_, cx| save.update(cx, |s, cx| s.save(cx)))));

        let enter = me.clone();
        div().size_full().relative().track_focus(&self.focus).child(picture).child(form).child(buttons).on_key_down(move |e: &KeyDownEvent, _, cx| {
            if e.keystroke.key == "enter" {
                enter.update(cx, |s, cx| s.save(cx));
            }
        })
    }
}

/// ⌘⇧3 and friends: one picture, saved, no window.
fn silent(window: bool, window_id: Option<olo_protocol::WindowId>) -> std::process::ExitCode {
    let mut settings = Settings::load();
    settings.window = window;
    settings.window_id = window_id;
    settings.delay = 0;
    let result = capture::take(&settings, false).and_then(|shot| {
        let folder = settings.folder();
        let path = folder.join(capture::file_name(&capture::next_name(&folder), settings.format));
        let r = capture::save(&shot, &path, settings.format).map(|()| path);
        let _ = std::fs::remove_file(&shot.png);
        r
    });
    match result {
        Ok(path) => saved(&path),
        Err(e) => {
            eprintln!("olo-screenshot: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Saved with no window: say where, and put that on the clipboard, held
/// until something else is copied (not with `--for-app`: the Screenshot
/// portal's, which hands the file to the app that asked).
fn saved(path: &Path) -> std::process::ExitCode {
    let path = path.display().to_string();
    println!("{path}");
    if std::env::args().any(|a| a == "--for-app") {
        return std::process::ExitCode::SUCCESS;
    }
    if let Err(e) = clipboard::hold(clipboard::text(&path)) {
        eprintln!("olo-screenshot: couldn't copy where it went: {e}");
    }
    std::process::ExitCode::SUCCESS
}

fn main() -> std::process::ExitCode {
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The Screenshot portal's PickColor: "R G B" (0–255), nothing if
    // cancelled.
    if args.iter().any(|a| a == "--pick-color") {
        return match region::pick_color() {
            Ok(Some([r, g, b])) => {
                println!("{r} {g} {b}");
                std::process::ExitCode::SUCCESS
            }
            Ok(None) => std::process::ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("olo-screenshot: {e}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if args.iter().any(|a| a == "--region") {
        return match region::run() {
            Ok(Some(path)) => saved(&path),
            Ok(None) => std::process::ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("olo-screenshot: {e}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if args.iter().any(|a| a == "--silent") {
        // `--window` alone: the active window; `--window ID`: that one.
        let at = args.iter().position(|a| a == "--window");
        let id = at.and_then(|i| args.get(i + 1)).and_then(|a| a.parse().ok());
        return silent(at.is_some(), id);
    }
    if instance::forward(APP_ID, &args) {
        return std::process::ExitCode::SUCCESS;
    }
    std::thread::spawn(capture::sweep);
    olo_ui::run(APP_ID, |cx| {
        instance::listen(APP_ID, olo_ui::proxy(cx));
        let app = cx.new(|cx| Screenshot::new(Settings::load(), cx));
        let restored = olo_ui::restored_state().and_then(|s| serde_json::from_str::<Saved>(&s).ok());
        app.update(cx, |s, cx| match restored {
            Some(Saved { png: Some(png), name }) if png.is_file() => {
                s.name.update(cx, |f, cx| f.set_text(name, cx));
                s.reload(png, cx);
            }
            // The picture comes first, then the window (so it isn't in it).
            _ => s.take(cx),
        });
        let messages = app.clone();
        olo_ui::message_handler(cx, move |message, cx| messages.update(cx, |s, cx| s.message(message, cx)));
        let saving = app.clone();
        olo_ui::on_upgrade(cx, move |cx| saving.read(cx).save_state(cx));
        // The picture app_server wrote for us goes with us.
        cx.on_app_quit(|_| {
            let _ = std::fs::remove_file(capture::scratch_path());
            async {}
        })
        .detach();
        // It stays with no window while the first picture is taken (the
        // window opens after) and after saving (holding the clipboard);
        // otherwise its last window closing ends it.
        olo_ui::stay_resident(cx);
        cx.on_window_closed(move |cx| {
            let s = app.read(cx);
            if cx.windows().is_empty() && !s.holding && s.busy != Some(Busy::Taking) {
                cx.quit();
            }
        })
        .detach();
    });
    std::process::ExitCode::SUCCESS
}
