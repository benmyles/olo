//! Screenshot, after Haiku's: takes a picture of the screen as it starts,
//! shows it, and offers to save it (any of several formats, into a folder
//! of your choice), copy it, or take a new one, of the whole screen or the
//! active window, with or without its border and the pointer, after a
//! delay. Our own window steps aside while a picture is taken. The first
//! picture is taken before there is a window (so it isn't in it): the
//! app's looper opens the window with it.
//!
//! `--silent [--window [ID]]` (⌘⇧3, Print Screen, Alt+Print Screen) saves one
//! straight into the folder, as `screenshotN`, with no window; `--region`
//! (⌘⇧4) lets you drag out part of the screen first (region.rs). However
//! it is saved, the file's path goes on the clipboard.

mod after;
mod backdrop;
mod bubble;
mod capture;
mod controls;
mod inset;
mod library;
mod recording;
mod region;
mod sound;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use capture::{Format, Settings, Shot};
use olo_kit::{
    Bounds, Child, ClickEvent, ClipboardEntry, ClipboardItem, Corner, Corners, Cx, Element, FontWeight, Image, ImageFormat, ParentElement as _, Pixels, RenderImage, SharedString, Styled as _, View,
    WindowHandle, WindowOptions, canvas, clipboard, div,
    file_panel::{self, FilePanel, PanelResult},
    held_clipboard,
    instance::{self, Launch},
    menu::{self, Menu},
    metrics as m, point, px, size, style, theme,
    widgets::{Button, CheckBox, MenuField, StringView, TextControl, TextField, TextFieldEvent, open_alert, separator, text_field},
};
use serde::{Deserialize, Serialize};

const APP_ID: &str = "olo.screenshot";

/// The window's width; it is as tall as its form needs (`Layout::height`).
const WIDTH: f32 = 580.0;
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
    /// A recording asked for from the window started (it goes on by
    /// itself: we're done), or why not.
    Recording(Result<(), String>),
    /// The recording settings Record…'s ticks show.
    Recordings(olo_protocol::ScreenRecordingSettings),
    /// The folder chosen in the panel.
    Folder(PathBuf),
    /// Record…'s menu, under the button.
    RecordMenu(Bounds<Pixels>),
    /// What was chosen from it.
    Record(String),
}

/// What the window's Record… records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Record {
    Screen,
    /// The active window (the one behind ours), else the screen.
    Window,
    /// Dragged out first.
    Region,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Busy {
    Taking,
    Saving,
    Copying,
}

struct Screenshot {
    settings: Settings,
    shot: Option<Shot>,
    /// Counts the pictures taken, so a late preview of an old one is
    /// dropped.
    taken: u64,
    /// The preview: the picture it's of (`taken`), its size in device
    /// pixels and the image. (A new picture the same size is another
    /// preview: New screenshot kept showing the one before.)
    preview: Option<(u64, (u32, u32), Arc<RenderImage>)>,
    /// The preview size last asked for (picture, size).
    asked: Option<(u64, (u32, u32))>,
    name: Child<TextField>,
    delay: Child<TextField>,
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
    /// The recording settings, for Record…'s ticks (the sound, GIF,
    /// clicks and keys shown).
    recordings: olo_protocol::ScreenRecordingSettings,
}

#[derive(Serialize, Deserialize)]
struct Saved {
    /// The picture being shown (in the runtime dir; kept across a reload).
    png: Option<PathBuf>,
    name: String,
}

fn folder_label(path: &Path) -> String {
    if path == capture::home() { "Home".into() } else { path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned()) }
}

/// `image` made `w` × `h` (device pixels) on a worker, area-averaged, as
/// it's drawn (BGRA): drawn one pixel to a device pixel, never scaled.
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
    /// The window's content's height: the form, then the buttons a
    /// spacing below it (a fixed height let "Save in:" go under them).
    height: f32,
}

fn layout() -> Layout {
    let w = WIDTH;
    let inset = m::WINDOW_INSET;
    let (last_top, last_h) = form_rows()[7];
    let buttons_y = last_top + last_h + m::SPACING;
    let height = buttons_y + m::CONTROL_HEIGHT + inset;
    let top_h = buttons_y - inset - inset;
    let area = Bounds { origin: point(px(inset), px(inset)), size: size(px(PREVIEW_W), px(top_h - m::LABEL_ROW - m::TIGHT)) };
    let form_x = inset + PREVIEW_W + inset + m::SPACING;
    Layout { area, form_x, form_w: w - inset - form_x, buttons_y, height }
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
    /// The window, showing the first picture taken (or saying why not).
    fn new(settings: Settings, first: Result<Shot, String>, name: Option<String>, cx: &mut Cx<Self>) -> Self {
        let mut folders = Vec::new();
        for f in [Some(capture::home()), olo_xdg::user_dir("DESKTOP"), olo_xdg::user_dir("PICTURES")].into_iter().flatten() {
            if !folders.contains(&f) {
                folders.push(f);
            }
        }
        let name = text_field(cx, |f| f.name("Name").with_text(name.unwrap_or_default()));
        let delay = text_field(cx, |f| f.name("Delay").with_text(settings.delay.to_string()));
        cx.subscribe(&name, |s: &mut Screenshot, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Entered => s.save(cx),
            _ => cx.invalidate(),
        });
        cx.subscribe(&delay, |s: &mut Screenshot, e: &TextFieldEvent, cx| {
            if let TextFieldEvent::Changed = e {
                // Two digits at most, a minute at most.
                let text = s.delay.read(cx).text().to_owned();
                let digits: String = text.chars().filter(char::is_ascii_digit).take(2).collect();
                if digits != text {
                    let shown = digits.clone();
                    s.delay.update(cx, |f, cx| f.set_text(shown, cx));
                }
                let d = digits.parse::<u32>().unwrap_or(0).min(60);
                if d != s.settings.delay {
                    s.settings.delay = d;
                    s.settings_changed(cx);
                }
            }
        });
        // What Record…'s ticks show, read off the window's looper.
        let to = cx.messenger();
        std::thread::spawn(move || {
            if let Some(s) = olo_kit::settings::current() {
                to.send(Msg::Recordings(s.screen_recording));
            }
        });
        // (Shown, or said, once the window is up.)
        cx.send(Msg::Taken(first));
        Screenshot {
            settings,
            shot: None,
            taken: 0,
            preview: None,
            asked: None,
            name,
            delay,
            proposed: String::new(),
            busy: Some(Busy::Taking),
            note: String::new(),
            folders,
            holding: false,
            recordings: Default::default(),
        }
    }

    /// One of Record…'s ticks turned over: kept in the settings (off the
    /// window's looper).
    fn toggle(&mut self, what: &str) {
        let r = &mut self.recordings;
        match what {
            "sound-computer" | "sound-microphone" => {
                let (mut computer, mut mic) = sound_parts(&r.sound);
                if what == "sound-microphone" {
                    mic = !mic;
                } else {
                    computer = !computer;
                }
                r.sound = match (computer, mic) {
                    (true, true) => "both",
                    (true, false) => "computer",
                    (false, true) => "microphone",
                    (false, false) => "none",
                }
                .into();
            }
            "gif" => r.gif = !r.gif,
            "show-clicks" => r.show_clicks = !r.show_clicks,
            "show-keys" => r.show_keys = !r.show_keys,
            "background" => r.background = !r.background,
            "pointer-size" => r.pointer_size = if r.pointer_size > 1.0 { 1.0 } else { 1.5 },
            "smooth-pointer" => r.smooth_pointer = !r.smooth_pointer,
            "zoom-follow" => r.zoom_follow = !r.zoom_follow,
            "camera" => r.camera = if r.camera.is_empty() { "auto".into() } else { String::new() },
            c if c.starts_with("countdown-") => r.countdown = c["countdown-".len()..].parse().unwrap_or(3),
            _ => return,
        }
        let mine = r.clone();
        std::thread::spawn(move || {
            let Some(mut settings) = olo_kit::settings::current() else { return };
            // (The encoder is Preferences', left as it is there.)
            settings.screen_recording = olo_protocol::ScreenRecordingSettings { encoder: settings.screen_recording.encoder, ..mine };
            if let Ok(mut c) = olo_protocol::Client::connect() {
                let _ = c.request(&olo_protocol::Request::SetSettings { settings });
            }
        });
    }

    fn name_text(&self, cx: &Cx<Self>) -> String {
        self.name.read(cx).text().to_owned()
    }

    /// Take a picture on a worker; our window steps aside (and comes back).
    fn take(&mut self, cx: &mut Cx<Self>) {
        self.busy = Some(Busy::Taking);
        self.note.clear();
        let settings = self.settings.clone();
        cx.work(move || capture::take(&settings, true), Msg::Taken);
        cx.invalidate();
    }

    /// Find the next free name in the folder, off the window's looper.
    fn propose_name(&self, cx: &mut Cx<Self>) {
        let folder = self.settings.folder();
        cx.work(
            move || {
                let name = capture::next_name(&folder);
                (folder, name)
            },
            |(folder, name)| Msg::Named(folder, name),
        );
    }

    /// Record…'s menu: what to record, or (one going on this display)
    /// Pause, Resume and Stop.
    fn record_menu(&self) -> Menu {
        match olo_protocol::recording::going() {
            Some(g) => {
                let menu = Menu::new("Record");
                let menu = if g.paused { menu.item("Resume Recording", "resume") } else { menu.item("Pause Recording", "pause") };
                menu.item("Stop Recording", "stop")
            }
            None => {
                let r = &self.recordings;
                let (computer, mic) = sound_parts(&r.sound);
                Menu::new("Record")
                    .item("Record Screen", "screen")
                    .item("Record Active Window", "window")
                    .item("Record Region…", "region")
                    .separator()
                    .item("Record the Computer's Sound", "sound-computer")
                    .checked(computer)
                    .enabled(!r.gif)
                    .item("Record the Microphone", "sound-microphone")
                    .checked(mic)
                    .enabled(!r.gif)
                    .separator()
                    .item("Show Clicks", "show-clicks")
                    .checked(r.show_clicks)
                    .item("Show Shortcuts", "show-keys")
                    .checked(r.show_keys)
                    .item("Larger Pointer", "pointer-size")
                    .checked(r.pointer_size > 1.0)
                    .item("Smooth Pointer", "smooth-pointer")
                    .checked(r.smooth_pointer)
                    .item("Zoom on Clicks", "zoom-follow")
                    .checked(r.zoom_follow)
                    .item("Show Camera", "camera")
                    .checked(!r.camera.is_empty())
                    .item("Window on a Background", "background")
                    .checked(r.background)
                    .separator()
                    .item("Save as GIF", "gif")
                    .checked(r.gif)
                    .submenu(
                        [(0, "None"), (3, "3 Seconds"), (5, "5 Seconds"), (10, "10 Seconds")]
                            .into_iter()
                            .fold(Menu::new("Countdown"), |m, (n, label)| m.item(label, format!("countdown-{n}")).checked(r.countdown == n)),
                    )
                    .separator()
                    .item("Recordings…", "library")
            }
        }
    }

    fn record_command(&mut self, command: &str, cx: &mut Cx<Self>) {
        let what = match command {
            "sound-computer" | "sound-microphone" | "gif" | "show-clicks" | "show-keys" | "background" | "pointer-size" | "smooth-pointer" | "zoom-follow" | "camera" | "countdown-0"
            | "countdown-3" | "countdown-5" | "countdown-10" => {
                self.toggle(command);
                cx.invalidate();
                return;
            }
            // The recordings made (library.rs, a window of its own).
            "library" => {
                if let Ok(exe) = std::env::current_exe() {
                    let _ = olo_xdg::open::spawn(&[exe.into_os_string(), "--library".into()]);
                }
                return;
            }
            "screen" => Record::Screen,
            "window" => Record::Window,
            "region" => Record::Region,
            // The recorder's own --pause, --resume and --stop.
            flag => {
                let flag = format!("--{flag}");
                std::thread::spawn(move || {
                    if let Ok(exe) = std::env::current_exe() {
                        let _ = std::process::Command::new(exe).arg(flag).stdout(std::process::Stdio::null()).status();
                    }
                });
                return;
            }
        };
        self.record(what, cx);
    }

    /// Record `what` (the pointer in it as the window has it): our window
    /// steps aside, the recorder starts on its own (Deskbar's record
    /// light, ⌘⇧6 or Stop Recording finish it, and it tells when it's
    /// saved), and we're done.
    fn record(&mut self, what: Record, cx: &mut Cx<Self>) {
        let pointer = self.settings.pointer;
        cx.work(
            move || {
                let active = capture::step_aside()?;
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                let mut cmd = std::process::Command::new(exe);
                cmd.arg("--toggle-record").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null());
                match (what, active) {
                    (Record::Window, Some(id)) => {
                        cmd.args(["--window", &id.to_string()]);
                    }
                    (Record::Region, _) => {
                        cmd.arg("--region");
                    }
                    _ => {}
                }
                if !pointer {
                    cmd.arg("--no-pointer");
                }
                cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
            },
            Msg::Recording,
        );
    }

    fn show_error(&self, message: String, cx: &mut Cx<Self>) {
        open_alert(cx, "Screenshot", "warning", &message, &["OK"], |_, _| {});
    }

    fn can_save(&self, cx: &Cx<Self>) -> bool {
        self.busy.is_none() && self.shot.is_some() && !self.name_text(cx).trim().is_empty()
    }

    fn save(&mut self, cx: &mut Cx<Self>) {
        if !self.can_save(cx) {
            return;
        }
        let Some(shot) = self.shot.clone() else { return };
        let path = self.settings.folder().join(capture::file_name(&self.name_text(cx), self.settings.format));
        let format = self.settings.format;
        self.busy = Some(Busy::Saving);
        cx.work(move || capture::save(&shot, &path, format).map(|()| path), Msg::Saved);
        cx.invalidate();
    }

    fn copy(&mut self, cx: &mut Cx<Self>) {
        let Some(shot) = self.shot.clone() else { return };
        self.busy = Some(Busy::Copying);
        cx.work(move || std::fs::read(&shot.png).map_err(|e| e.to_string()), Msg::Copied);
        cx.invalidate();
    }

    fn settings_changed(&mut self, cx: &mut Cx<Self>) {
        self.settings.store();
        // Keep a proposed name current; a typed one stays.
        if self.name_text(cx) == self.proposed {
            self.propose_name(cx);
        }
        cx.invalidate();
    }

    fn choose_folder(&mut self, cx: &mut Cx<Self>) {
        // (The panel answers on its own looper: it tells this window.)
        let to = cx.messenger();
        file_panel::show("Choose a Folder", FilePanel::folder(self.settings.folder()), move |a| {
            if let PanelResult::Chosen(dir) = a.result {
                to.send(Msg::Folder(dir));
            }
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
        let (mut pw, mut ph) = ((w * k).round().max(1.0), (h * k).round().max(1.0));
        let top = dev(f32::from(area.top())) + edge;
        // Its caption a few points under the middle of a row of the form
        // beside it: the picture a little smaller, so the caption is level
        // with that row (`body` lines it up with one just below).
        let caption = (top + ph + edge) / scale + m::TIGHT + m::LABEL_ROW / 2.0;
        if let Some(r) = form_rows().iter().map(|(t, h)| t + h / 2.0).find(|r| (caption - m::LABEL_ROW / 2.0..caption).contains(r)) {
            ph = (ph - ((caption - r) * scale).ceil()).max(1.0);
            pw = (w * ph / h).round().max(1.0);
        }
        let left = dev(f32::from(area.left())) + edge + ((iw - pw) / 2.0).round();
        let outer = Bounds::from_corners(point(px((left - edge) / scale), px((top - edge) / scale)), point(px((left + pw + edge) / scale), px((top + ph + edge) / scale)));
        (outer, Some((pw as u32, ph as u32)))
    }

    /// Asks for a preview the size the frame wants now (once per picture
    /// and size: a new scale, a new picture).
    fn want_preview(&mut self, scale: f32, cx: &mut Cx<Self>) {
        let (_, Some(want)) = self.frame(scale) else { return };
        if self.preview.as_ref().is_some_and(|(of, s, _)| *of == self.taken && *s == want) || self.asked == Some((self.taken, want)) {
            return;
        }
        let Some(shot) = self.shot.clone() else { return };
        self.asked = Some((self.taken, want));
        let (taken, to) = (self.taken, cx.messenger());
        std::thread::spawn(move || {
            if let Some(image) = preview_of(&shot.image, want.0, want.1) {
                to.send(Msg::Preview(taken, want, image));
            }
        });
    }
}

impl View for Screenshot {
    type Message = Msg;

    fn message(&mut self, msg: Msg, cx: &mut Cx<Self>) {
        match msg {
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
            }
            Msg::Preview(taken, size, image) => {
                if taken == self.taken {
                    self.preview = Some((taken, size, image));
                }
            }
            Msg::Named(folder, name) => {
                let typed = self.name_text(cx);
                if folder == self.settings.folder() && (typed.is_empty() || typed == self.proposed) {
                    let shown = name.clone();
                    self.name.update(cx, |f, cx| f.set_text(shown, cx));
                    self.proposed = name;
                }
            }
            Msg::Saved(r) => {
                self.busy = None;
                match r {
                    // Done, as in Haiku: the window goes, and where the
                    // picture went is on the clipboard (held, with no
                    // window, until something else is copied; then the
                    // app ends).
                    Ok(path) => {
                        self.holding = true;
                        cx.close_window();
                        let _ = std::fs::remove_file(capture::scratch_path());
                        std::thread::spawn(move || {
                            if let Err(e) = held_clipboard::hold(held_clipboard::text(&path.display().to_string())) {
                                tracing::warn!("couldn't put where it went on the clipboard: {e}");
                            }
                            olo_kit::quit();
                        });
                    }
                    Err(e) => self.show_error(format!("Couldn't save the screenshot.\n\n{e}"), cx),
                }
            }
            Msg::Copied(r) => {
                self.busy = None;
                match r {
                    Ok(png) => {
                        clipboard::write(ClipboardItem::from_entries(vec![ClipboardEntry::Image(Image::from_bytes(ImageFormat::Png, png))]));
                        self.note = "Copied to the clipboard".into();
                    }
                    Err(e) => self.show_error(format!("Couldn't copy the screenshot.\n\n{e}"), cx),
                }
            }
            Msg::Recording(Ok(())) => {
                let _ = std::fs::remove_file(capture::scratch_path());
                olo_kit::quit();
            }
            Msg::Recording(Err(e)) => {
                cx.activate_window();
                self.show_error(format!("Couldn't start recording.\n\n{e}"), cx);
            }
            Msg::Recordings(r) => self.recordings = r,
            Msg::Folder(dir) => {
                self.settings.folder = Some(dir);
                self.settings_changed(cx);
            }
            Msg::RecordMenu(at) => {
                let chosen = cx.msg_with(|command: &str| Msg::Record(command.to_owned()));
                menu::popup(cx, self.record_menu(), at, Corner::BottomLeft, chosen);
            }
            Msg::Record(command) => self.record_command(&command, cx),
        }
        cx.invalidate();
    }

    /// The window closed by the person: the app ends (saved, it stays to
    /// hand out where the picture went).
    fn window_closing(&mut self, _: &mut Cx<Self>) {
        if !self.holding {
            olo_kit::quit();
        }
    }

    fn save_state(&mut self, cx: &mut Cx<Self>) -> Option<String> {
        serde_json::to_string(&Saved { png: self.shot.as_ref().map(|s| s.png.clone()), name: self.name_text(cx) }).ok()
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let scale = cx.scale_factor();
        self.want_preview(scale, cx);
        let l = layout();
        let rows = form_rows();
        let at = |x: f32, y: f32, w: f32| div().absolute().left(px(x)).top(px(y)).w(px(w));

        // The preview, framed at the picture's shape, its size under it.
        let (frame, _) = self.frame(scale);
        let preview = self.preview.as_ref().map(|(_, _, image)| image.clone());
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
        let look = style::key((
            self.taken,
            self.preview.as_ref().map(|(of, size, _)| (*of, *size)),
            &caption,
            f32::from(frame.origin.x).to_bits(),
            f32::from(frame.size.width).to_bits(),
            f32::from(frame.size.height).to_bits(),
        ));
        let picture = canvas(move |g, _| {
            let p = theme::palette();
            if let Some(image) = &preview {
                let inner = style::inset(g, frame, 2.0);
                let _ = g.paint_image(inner, Corners::default(), image.clone(), 0, false);
            }
            style::sunken(g, frame, &p, false);
            let font = style::ui_font(FontWeight::NORMAL);
            style::label(g, area.center().x, px(caption_y), style::Align::Center, caption.clone(), style::PLAIN_SIZE, font, style::disabled_text(&p));
        })
        .paint_key(look)
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .size_full();

        // The form.
        let check = |label: &'static str, on: bool, enabled: bool, set: fn(&mut Settings, bool)| {
            CheckBox::new(label, on).enabled(enabled).on_toggle(cx.listener(move |s: &mut Screenshot, v: &bool, cx| {
                set(&mut s.settings, *v);
                s.settings_changed(cx);
            }))
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
        let form = div()
            .child(at(fx, rows[0].0, fw).child(check("Capture active window", self.settings.window, true, |s, v| s.window = v)))
            .child(at(fx, rows[1].0, fw).child(check("Include window border", self.settings.border, self.settings.window, |s, v| s.border = v)))
            .child(at(fx, rows[2].0, fw).child(check("Include mouse pointer", self.settings.pointer, true, |s, v| s.pointer = v)))
            .child(
                at(fx, rows[3].0, fw)
                    .flex()
                    .items_center()
                    .gap(px(m::LABEL_GAP))
                    .child(TextControl::new(cx, &self.delay).label("Delay:").divider(DIVIDER).width(DIVIDER + 44.0))
                    .child(StringView::new("seconds")),
            )
            .child(at(fx, rows[4].0, fw).child(separator()))
            .child(at(fx, rows[5].0, fw).child(TextControl::new(cx, &self.name).label("Name:").divider(DIVIDER).width(fw)))
            .child(at(fx, rows[6].0, fw).child(MenuField::new("format", labels, fi).label("Save as:").divider(DIVIDER).width(fw).on_change(cx.listener(|s: &mut Screenshot, i: &usize, cx| {
                s.settings.format = Format::ALL[*i];
                s.settings_changed(cx);
            }))))
            .child(at(fx, rows[7].0, fw).child(MenuField::new("folder", names, di).label("Save in:").divider(DIVIDER).width(fw).on_change(cx.listener(move |s: &mut Screenshot, i: &usize, cx| {
                match folders.get(*i) {
                    Some(f) => {
                        s.settings.folder = Some(f.clone());
                        s.settings_changed(cx);
                    }
                    None => s.choose_folder(cx),
                }
            }))));

        // Buttons.
        let idle = self.busy.is_none() && self.shot.is_some();
        let can_save = self.can_save(cx);
        let w = WIDTH;
        let bw = m::BUTTON_MIN_WIDTH;
        let save_x = w - m::WINDOW_INSET - bw;
        let new_x = save_x - m::SPACING - 120.0;
        let buttons = div()
            .child(
                at(m::WINDOW_INSET, l.buttons_y, 130.0)
                    .child(Button::new("Copy to clipboard").min_width(130.0).enabled(idle).on_click(cx.listener(|s: &mut Screenshot, _: &ClickEvent, cx| s.copy(cx)))),
            )
            .child(at(m::WINDOW_INSET + 130.0 + m::SPACING, l.buttons_y, bw).child(Button::new("Record…").id("record").min_width(bw).enabled(self.busy.is_none()).on_click(cx.listener(
                |_: &mut Screenshot, _: &ClickEvent, cx| {
                    let at = cx.listener_bounds();
                    cx.send(Msg::RecordMenu(at));
                },
            ))))
            .child(
                at(new_x, l.buttons_y, 120.0)
                    .child(Button::new("New screenshot").min_width(120.0).enabled(self.busy.is_none()).on_click(cx.listener(|s: &mut Screenshot, _: &ClickEvent, cx| s.take(cx)))),
            )
            // (Return saves: Save is the default button.)
            .child(at(save_x, l.buttons_y, bw).child(Button::new("Save").default(true).min_width(bw).enabled(can_save).on_click(cx.listener(|s: &mut Screenshot, _: &ClickEvent, cx| s.save(cx)))));

        div().w_full().h(px(l.height)).relative().child(picture).child(form).child(buttons)
    }
}

/// What the app's looper is told: the first picture (the window opens
/// with it), and launches.
enum AppMsg {
    First(Result<Shot, String>, Option<String>),
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
    if let Err(e) = held_clipboard::hold(held_clipboard::text(&path)) {
        eprintln!("olo-screenshot: couldn't copy where it went: {e}");
    }
    std::process::ExitCode::SUCCESS
}

fn main() -> std::process::ExitCode {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Recording the screen (recording.rs): no window, for people's
    // shortcuts and agents alike.
    if args.iter().any(|a| a == "--record") {
        std::process::exit(recording::run(&args));
    }
    if args.iter().any(|a| a == "--stop") {
        std::process::exit(recording::stop());
    }
    if args.iter().any(|a| a == "--gif") {
        std::process::exit(recording::gif(&args));
    }
    if args.iter().any(|a| a == "--export") {
        std::process::exit(recording::export(&args));
    }
    if args.iter().any(|a| a == "--trim") {
        std::process::exit(recording::trim(&args));
    }
    if args.iter().any(|a| a == "--toggle-record") {
        std::process::exit(recording::toggle(&args));
    }
    if args.iter().any(|a| a == "--pause") {
        std::process::exit(recording::pause(true));
    }
    if args.iter().any(|a| a == "--discard") {
        std::process::exit(recording::discard());
    }
    if args.iter().any(|a| a == "--restart") {
        std::process::exit(recording::restart());
    }
    if args.iter().any(|a| a == "--controls") {
        return controls::run();
    }
    if args.iter().any(|a| a == "--after") {
        return after::run(&args);
    }
    if args.iter().any(|a| a == "--library") {
        return library::run(&args);
    }
    if args.iter().any(|a| a == "--recordings") {
        std::process::exit(library::print());
    }
    if args.iter().any(|a| a == "--resume") {
        std::process::exit(recording::pause(false));
    }
    if args.iter().any(|a| a == "--recording") {
        std::process::exit(recording::status());
    }
    if args.iter().any(|a| a == "--zoom-to") {
        std::process::exit(recording::zoom_to(&args));
    }
    if args.iter().any(|a| a == "--mark") {
        std::process::exit(recording::mark(&args));
    }
    if args.iter().any(|a| a == "--caption") && !args.iter().any(|a| a == "--record") {
        std::process::exit(recording::caption(&args));
    }
    if args.iter().any(|a| a == "--camera-bubble") {
        std::process::exit(bubble::run(&args));
    }
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
    olo_kit::run(APP_ID, |app| {
        let proxy = olo_kit::proxy();
        instance::listen(APP_ID, proxy.clone());
        // It stays with no window while the first picture is taken (the
        // window opens after, so it isn't in it) and after saving
        // (holding the clipboard); its window closed otherwise ends it.
        olo_kit::stay_resident();
        let restored = olo_kit::restored_state().and_then(|s| serde_json::from_str::<Saved>(&s).ok());
        std::thread::spawn(move || match restored {
            // The picture shown before an in-place reload.
            Some(Saved { png: Some(png), name }) if png.is_file() => {
                let r = image::open(&png).map_err(|e| e.to_string()).map(|img| Shot { png, image: Arc::new(img.into_rgba8()) });
                proxy.send(AppMsg::First(r, Some(name)));
            }
            _ => proxy.send(AppMsg::First(capture::take(&Settings::load(), false), None)),
        });
        let mut window: Option<WindowHandle<Screenshot>> = None;
        olo_kit::message_handler(app, move |message, _| {
            // Launched again: the window comes forward.
            if message.is::<Launch>() {
                olo_kit::raise(window.as_ref());
            } else if let Ok(first) = message.downcast::<AppMsg>() {
                let AppMsg::First(shot, name) = *first;
                let mut options = WindowOptions::new("Screenshot", WIDTH, layout().height).fits_content();
                options.fixed = true;
                window = Some(olo_kit::open_window(options, move |cx| Screenshot::new(Settings::load(), shot, name, cx)));
            }
        });
    });
    // The picture app_server wrote for us goes with us.
    let _ = std::fs::remove_file(capture::scratch_path());
    std::process::ExitCode::SUCCESS
}

/// The computer's sound and the microphone, as a sound setting has them.
fn sound_parts(sound: &str) -> (bool, bool) {
    match sound {
        "both" => (true, true),
        "microphone" => (false, true),
        "none" => (false, false),
        _ => (true, false),
    }
}
