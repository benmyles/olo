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
//! it is saved, the file's path goes on the clipboard, and it is told of
//! in a notification in the screen's lower right, its picture to drag
//! out as the file, open, copy, show in Tracker or delete (saved.rs; not
//! with `--no-notification`, a script's, nor for the Screenshot portal's
//! `--for-app`).

mod backdrop;
mod bubble;
mod capture;
mod controls;
mod inset;
mod library;
mod recording;
mod region;
mod saved;
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
    widgets::{Button, CheckBox, MenuField, StringView, TextControl, TextField, TextFieldEvent, button_width, open_alert, separator, text_field},
};
use serde::{Deserialize, Serialize};

const APP_ID: &str = "olo.screenshot";

/// The window's width; it is as tall as its form needs (`Layout::height`).
const WIDTH: f32 = 580.0;
/// The preview's share of the window, beside the form.
const PREVIEW_W: f32 = 240.0;
/// The preview's height over the form, in a window too narrow for the
/// two side by side: as wide as the window at the picture's shape, so
/// tall at least and at most.
const PREVIEW_OVER_LEAST: f32 = 120.0;
/// A screen's shape, before a picture's taken (16 by 10).
const DEFAULT_ASPECT: f32 = 1.6;
const PREVIEW_OVER_MOST: f32 = 260.0;
/// Labels' column in the form.
const DIVIDER: f32 = 66.0;
/// The least room the form takes beside the preview (its fields a
/// popup's longest choice wide): narrower, the preview goes over it.
const FORM_LEAST: f32 = 214.0;
/// The buttons' widths: Copy to Clipboard and New Screenshot (Record…
/// and Save are `BUTTON_MIN_WIDTH`).
const COPY_W: f32 = 130.0;
const NEW_W: f32 = 120.0;

enum Msg {
    /// A picture, or why not.
    Taken(Result<Shot, String>),
    /// The picture made small for the preview: which picture, the size in
    /// device pixels it was made for, and it (None: it couldn't be).
    Preview(u64, (u32, u32), Option<Arc<RenderImage>>),
    /// Saved: where, and the file written (its notification's receipt).
    Saved(Result<(PathBuf, olo_protocol::notifications::FileId), String>),
    Copied(Result<Vec<u8>, String>),
    /// The first free name in a folder.
    Named(PathBuf, String),
    /// A recording asked for from the window started (it goes on by
    /// itself: we're done), or why not.
    Recording(Result<(), String>),
    /// The recording settings Record…'s ticks show.
    Recordings(olo_protocol::ScreenRecordingSettings, Encoders),
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

#[derive(Default)]
struct Encoders {
    nvidia: bool,
    va_h264: bool,
    va_av1: bool,
}

impl Encoders {
    fn read() -> Self {
        let va = olo_video::vaapi::abilities();
        Self {
            nvidia: olo_system::gpus::list().iter().any(|g| g.vendor == 0x10de) && olo_video::nvenc::available(),
            va_h264: va.iter().any(|a| a.h264.is_some()),
            va_av1: va.iter().any(|a| a.av1.is_some()),
        }
    }

    fn supports(&self, encoder: &str, codec: &str) -> bool {
        match encoder {
            "cpu" => codec == "h264",
            "vaapi" => codec == "h264" && self.va_h264 || codec == "av1" && self.va_av1,
            "nvidia" => self.nvidia,
            _ => codec == "h264" || self.nvidia || codec == "av1" && self.va_av1,
        }
    }
}

struct Screenshot {
    settings: Settings,
    shot: Option<Shot>,
    /// Counts the pictures taken, so a late preview of an old one is
    /// dropped.
    taken: u64,
    /// The preview: the picture it's of (`taken`), its size in device
    /// pixels and the image. (A new picture the same size is another
    /// preview: New Screenshot kept showing the one before.)
    preview: Option<(u64, (u32, u32), Arc<RenderImage>)>,
    /// Which preview is wanted, and which is being made ([`Previews`]).
    previews: Previews,
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
    encoders: Encoders,
    save_recording: std::sync::mpsc::Sender<olo_protocol::ScreenRecordingSettings>,
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
    for p in bgra.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(w.max(1), h.max(1), bgra)?;
    Some(Arc::new(RenderImage::new([image::Frame::new(buffer)])))
}

/// A preview: the picture it's of and its size in device pixels.
type Wanted = (u64, (u32, u32));

/// Which preview is wanted now, and its making: one worker at a time, so
/// sizes passed through on the way (a window being resized) aren't each
/// made, and one made late never takes the place of the one wanted.
#[derive(Default)]
struct Previews {
    /// The one wanted now: the one drawn, and the one made next.
    asked: Option<Wanted>,
    /// A worker is making one.
    making: bool,
    /// One that couldn't be made: not asked for again.
    unmade: Option<Wanted>,
}

impl Previews {
    /// `wanted` is wanted now (`have`, the one made last): Some where a
    /// worker should make it now; None where it's made, or one is being
    /// made (the next made for what's wanted then, as that one's in), or
    /// it couldn't be.
    fn want(&mut self, wanted: Wanted, have: Option<Wanted>) -> Option<Wanted> {
        self.asked = Some(wanted);
        if have == Some(wanted) || self.making || self.unmade == Some(wanted) {
            return None;
        }
        self.making = true;
        Some(wanted)
    }

    /// The worker's `made` is in (`ok`: it could be): whether it's kept,
    /// the one wanted now.
    fn made(&mut self, made: Wanted, ok: bool) -> bool {
        self.making = false;
        if !ok {
            self.unmade = Some(made);
            return false;
        }
        self.asked == Some(made)
    }
}

/// The window's parts (logical points, in the window).
struct Layout {
    /// The content's width it's laid out for.
    width: f32,
    /// Where the preview may go.
    area: Bounds<Pixels>,
    /// The preview beside the form (else over it).
    beside: bool,
    /// The form's rows ([`form_rows`]), its left and width.
    rows: [(f32, f32); 8],
    form_x: f32,
    form_w: f32,
    /// The buttons' widths ([`button_widths`]), and how many rows they
    /// take: one, two by two, or one a row.
    widths: [f32; 4],
    button_rows: usize,
    /// The first row of buttons' top, and the last's: the same where all
    /// four fit on one.
    buttons_y: f32,
    last_row: f32,
    /// The window's content's height: the form, then the buttons a
    /// spacing below it (a fixed height let "Save in:" go under them).
    height: f32,
}

/// The buttons' least widths, their words' in the language shown (the
/// kit measures them so: Copy to Clipboard, Record…, New Screenshot,
/// Save), the first and third no narrower than the design's own.
fn button_widths() -> [f32; 4] {
    [button_width("Copy to Clipboard", false).max(COPY_W), button_width("Record…", false), button_width("New Screenshot", false).max(NEW_W), button_width("Save", false)]
}

/// The window's parts at content width `width`: its own width
/// (`WIDTH`), or what tiling holds it at; the picture `aspect` (its width
/// over its height) and the buttons `widths` ([`button_widths`]). The
/// preview beside the form where the form has its least room there, its
/// first check box level with the preview's top; else over it, as wide
/// as the window at the picture's shape. The buttons on one row where
/// they fit; else two by two in columns, Save last at the right; else
/// one a row (words longer than English's). Rows of buttons a spacing
/// apart: room for Save's ring. (Laid out for `WIDTH` whatever its
/// width, a third of the smallest screen had Save and the form's fields
/// past the window's edge: QA, 2026-11-28.)
fn layout(width: f32, aspect: f32, widths: [f32; 4]) -> Layout {
    let over_h = ((width - 2.0 * m::WINDOW_INSET) / aspect.max(0.1)).clamp(PREVIEW_OVER_LEAST, PREVIEW_OVER_MOST).round();
    layout_over(width, over_h, widths)
}

/// [`layout`] with the preview over the form `over_h` tall (where it's
/// over it).
fn layout_over(width: f32, over_h: f32, widths: [f32; 4]) -> Layout {
    let inset = m::WINDOW_INSET;
    let beside = width - inset - PREVIEW_W - inset - m::SPACING - inset >= FORM_LEAST;
    let (form_x, form_top) = if beside { (inset + PREVIEW_W + inset + m::SPACING, inset - (m::MARK_ROW - m::MARK_SIZE) / 2.0) } else { (inset, inset + over_h + m::TIGHT + m::LABEL_ROW + m::SPACING) };
    let rows = form_rows(form_top);
    let (last_top, last_h) = rows[7];
    let buttons_y = last_top + last_h + m::SPACING;
    let [copy, record, new, save] = widths;
    let column = ((width - 2.0 * inset - m::SPACING) / 2.0).floor();
    // (Save's ring takes room on one row too, after its face: inside the
    // window's inset. Between it and New Screenshot, a spacing face to
    // face as between any two, its ring drawn into it, as the kit's
    // alerts have it.)
    let button_rows = if 2.0 * inset + copy + record + new + save + 3.0 * m::SPACING + DEFAULT_RING <= width {
        1
    } else if copy.max(new) <= column && record.max(save + 2.0 * DEFAULT_RING) <= column {
        2
    } else {
        4
    };
    let last_row = buttons_y + (button_rows - 1) as f32 * (m::CONTROL_HEIGHT + m::SPACING);
    let height = last_row + m::CONTROL_HEIGHT + inset;
    let area = if beside {
        let top_h = buttons_y - inset - inset;
        Bounds { origin: point(px(inset), px(inset)), size: size(px(PREVIEW_W), px(top_h - m::LABEL_ROW - m::TIGHT)) }
    } else {
        Bounds { origin: point(px(inset), px(inset)), size: size(px(width - 2.0 * inset), px(over_h)) }
    };
    Layout { width, area, beside, rows, form_x, form_w: width - inset - form_x, widths, button_rows, buttons_y, last_row, height }
}

/// Held by tiling `width` by `height`: its own layout where it fits;
/// else its preview (over the form) shorter by what's over, to its least;
/// else, taller even so, laid out beside the scroll bar the Frame shows
/// then, its preview at its least. (Laid out narrower whenever the wider
/// was taller, a height the narrower then fitted showed no bar and 14
/// points empty at its right: Neon's review, 2026-11-28.)
fn layout_held(width: f32, height: f32, aspect: f32, widths: [f32; 4]) -> Layout {
    let l = layout(width, aspect, widths);
    if l.height <= height + 0.5 {
        return l;
    }
    if !l.beside {
        let over = (f32::from(l.area.size.height) - (l.height - height)).floor();
        if over >= PREVIEW_OVER_LEAST {
            return layout_over(width, over, widths);
        }
    }
    layout_over(width - olo_kit::widgets::SCROLL_BAR, PREVIEW_OVER_LEAST, widths)
}

/// The width it lays out at in a window `vw` wide: its own; less the scroll
/// bar where the Frame, scrolling it (taller than the screen), widened
/// the window by it. (Laid out at the widened width, Save's ring ran into
/// the bar: Neon's review, 2026-11-28.)
fn unheld_width(vw: f32) -> f32 {
    if vw >= WIDTH + olo_kit::widgets::SCROLL_BAR - 0.5 { vw - olo_kit::widgets::SCROLL_BAR } else { vw }
}

/// A default button's ring, outside its face: Save's face is set in by it
/// so the ring ends where the fields over it do, and under Record…'s
/// edges in two columns (Opus's and Neon's reviews, 2026-11-28).
const DEFAULT_RING: f32 = 3.0;

impl Layout {
    /// Held `extra` taller than it by tiling: its buttons at the window's
    /// foot, a window's inset over it, as a BeOS dialog's are (they
    /// floated mid-window, a third of it empty under them: Opus's review,
    /// 2026-11-28).
    fn pinned(mut self, extra: f32) -> Layout {
        if extra > 0.5 {
            self.buttons_y += extra;
            self.last_row += extra;
            self.height += extra;
        }
        self
    }

    /// The buttons' places (left, top) and widths: Copy to Clipboard,
    /// Record…, New Screenshot, Save. On one row, Copy and Record… at the
    /// left and the others at the right; two by two, in two columns as
    /// wide as the window allows, edge to edge with the form, Save at the
    /// bottom right; or one a row, as wide as the form.
    fn buttons(&self) -> [(f32, f32, f32); 4] {
        let inset = m::WINDOW_INSET;
        let [copy, record, new, save] = self.widths;
        match self.button_rows {
            1 => {
                let save_x = self.width - inset - DEFAULT_RING - save;
                let new_x = save_x - m::SPACING - new;
                [(inset, self.buttons_y, copy), (inset + copy + m::SPACING, self.buttons_y, record), (new_x, self.last_row, new), (save_x, self.last_row, save)]
            }
            2 => {
                let column = ((self.width - 2.0 * inset - m::SPACING) / 2.0).floor();
                let right = self.width - inset - column;
                [(inset, self.buttons_y, column), (right, self.buttons_y, column), (inset, self.last_row, column), (right + DEFAULT_RING, self.last_row, column - 2.0 * DEFAULT_RING)]
            }
            _ => {
                let w = self.width - 2.0 * inset;
                let row = |i: f32| self.buttons_y + i * (m::CONTROL_HEIGHT + m::SPACING);
                [(inset, row(0.0), w), (inset, row(1.0), w), (inset, row(2.0), w), (inset + DEFAULT_RING, row(3.0), w - 2.0 * DEFAULT_RING)]
            }
        }
    }
}

/// The form's rows: each one's top and height, from the window's top,
/// the first at `top`. Check boxes, then the delay; a rule; the name,
/// the format and the folder.
fn form_rows(top: f32) -> [(f32, f32); 8] {
    let mut y = top;
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
        let (save_recording, updates) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let options = olo_kit::settings::current().map(|s| s.screen_recording).unwrap_or_default();
            to.send(Msg::Recordings(options, Encoders::read()));
            while let Ok(mut options) = updates.recv() {
                while let Ok(new) = updates.try_recv() {
                    options = new;
                }
                if let Ok(mut c) = olo_protocol::Client::connect() {
                    let _ = c.request(&olo_protocol::Request::SetRecordingOptions { options });
                }
            }
        });
        // (Shown, or said, once the window is up.)
        cx.send(Msg::Taken(first));
        Screenshot {
            settings,
            shot: None,
            taken: 0,
            preview: None,
            previews: Previews::default(),
            name,
            delay,
            proposed: String::new(),
            busy: Some(Busy::Taking),
            note: String::new(),
            folders,
            holding: false,
            recordings: Default::default(),
            encoders: Default::default(),
            save_recording,
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
            c if c.starts_with("encoder-") => {
                r.encoder = c["encoder-".len()..].into();
                if !self.encoders.supports(&r.encoder, &r.codec) {
                    r.codec = if self.encoders.supports(&r.encoder, "h264") { "h264" } else { "av1" }.into();
                }
            }
            c if c.starts_with("codec-") => r.codec = c["codec-".len()..].into(),
            c if c.starts_with("countdown-") => r.countdown = c["countdown-".len()..].parse().unwrap_or(3),
            _ => return,
        }
        let _ = self.save_recording.send(r.clone());
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
                    .submenu(
                        [("auto", "Automatic"), ("vaapi", "Integrated GPU (VA-API)"), ("nvidia", "NVIDIA GPU (NVENC)"), ("cpu", "Processor")]
                            .into_iter()
                            .fold(Menu::new("Encode With"), |m, (id, label)| {
                                m.item(label, format!("encoder-{id}")).checked(r.encoder == id).enabled(self.encoders.supports(id, "h264") || self.encoders.supports(id, "av1"))
                            }),
                    )
                    .submenu(
                        [("h264", "H.264"), ("hevc", "HEVC"), ("av1", "AV1")]
                            .into_iter()
                            .fold(Menu::new("Video Format"), |m, (id, label)| m.item(label, format!("codec-{id}")).checked(r.codec == id).enabled(self.encoders.supports(&r.encoder, id))),
                    )
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
        if command.starts_with("encoder-") || command.starts_with("codec-") {
            self.toggle(command);
            cx.invalidate();
            return;
        }
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
        let (encoder, codec) = (self.recordings.encoder.clone(), self.recordings.codec.clone());
        cx.work(
            move || {
                let active = capture::step_aside()?;
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                let mut cmd = std::process::Command::new(exe);
                cmd.args(["--toggle-record", "--encoder", &encoder, "--codec", &codec]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null());
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
        cx.work(move || capture::save(&shot, &path, format).map(|id| (path, id)), Msg::Saved);
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

    /// The picture's shape, its width over its height (before one's
    /// taken, a screen's).
    fn aspect(&self) -> f32 {
        self.shot.as_ref().map_or(DEFAULT_ASPECT, |s| s.image.width().max(1) as f32 / s.image.height().max(1) as f32)
    }

    /// The preview's frame for the picture, on device pixels at `scale`,
    /// in layout `l`: its outer bounds (the sunken frame two lines round
    /// it) and the picture's size inside, in device pixels.
    fn frame(&self, scale: f32, l: &Layout) -> (Bounds<Pixels>, Option<(u32, u32)>) {
        let area = l.area;
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
        if l.beside
            && let Some(r) = l.rows.iter().map(|(t, h)| t + h / 2.0).find(|r| (caption - m::LABEL_ROW / 2.0..caption).contains(r))
        {
            ph = (ph - ((caption - r) * scale).ceil()).max(1.0);
            pw = (w * ph / h).round().max(1.0);
        }
        let left = dev(f32::from(area.left())) + edge + ((iw - pw) / 2.0).round();
        let outer = Bounds::from_corners(point(px((left - edge) / scale), px((top - edge) / scale)), point(px((left + pw + edge) / scale), px((top + ph + edge) / scale)));
        (outer, Some((pw as u32, ph as u32)))
    }

    /// Asks for a preview the size the frame wants now (once per picture
    /// and size: a new scale, a new picture).
    fn want_preview(&mut self, scale: f32, l: &Layout, cx: &mut Cx<Self>) {
        let (_, Some(want)) = self.frame(scale, l) else { return };
        let wanted = (self.taken, want);
        let Some(shot) = self.shot.clone() else { return };
        let have = self.preview.as_ref().map(|(of, s, _)| (*of, *s));
        if self.previews.want(wanted, have).is_none() {
            return;
        }
        let to = cx.messenger();
        std::thread::spawn(move || {
            to.send(Msg::Preview(wanted.0, wanted.1, preview_of(&shot.image, wanted.1.0, wanted.1.1)));
        });
    }

    /// The preview to draw: the one wanted now only (one of another size
    /// would be scaled, never drawn: Astra's review, 2026-11-28, an older
    /// size made late stayed for good).
    fn shown_preview(&self) -> Option<Arc<RenderImage>> {
        self.preview.as_ref().filter(|(of, s, _)| Some((*of, *s)) == self.previews.asked).map(|(_, _, image)| image.clone())
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
                // Kept where it's the one wanted now; else the next is made
                // for what is (the body asks, drawn again below).
                let kept = self.previews.made((taken, size), image.is_some());
                if let Some(image) = image.filter(|_| kept) {
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
                    Ok((path, id)) => {
                        self.holding = true;
                        cx.close_window();
                        let _ = std::fs::remove_file(capture::scratch_path());
                        std::thread::spawn(move || {
                            // (Told of once the path is on the clipboard,
                            // or with no clipboard at all, as `saved`:
                            // what it says made first, here.)
                            let mut prepared = saved::prepare(&path, saved::Identity::Written(id), saved::Kind::Picture, None);
                            let mut notes = None;
                            let mut tell = || {
                                if let Some(p) = prepared.take() {
                                    notes = Some(p.send());
                                }
                            };
                            if let Err(e) = held_clipboard::hold_then(held_clipboard::text(&path.display().to_string()), &mut tell) {
                                tracing::warn!("couldn't put where it went on the clipboard: {e}");
                            }
                            tell();
                            saved::sent(notes.as_ref());
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
            Msg::Recordings(r, encoders) => {
                self.recordings = r;
                self.encoders = encoders;
            }
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
        // (Its own width, or what tiling holds it at.)
        // (Its own width, or what tiling holds it at: held shorter than
        // it, the Frame scrolls it, its bar taking `SCROLL_BAR` of the
        // width, laid out for what's left (its controls ran under the bar:
        // Neon's review, 2026-11-28); held taller, its buttons at the foot.)
        let vs = cx.viewport_size();
        let (vw, vh) = (f32::from(vs.width), f32::from(vs.height));
        let held = cx.held_state().held();
        let (aspect, widths) = (self.aspect(), button_widths());
        let l = if held { layout_held(vw, vh, aspect, widths) } else { layout(unheld_width(vw), aspect, widths) };
        let extra = vh - l.height;
        let l = if held { l.pinned(extra) } else { l };
        self.want_preview(scale, &l, cx);
        let rows = l.rows;
        let at = |x: f32, y: f32, w: f32| div().absolute().left(px(x)).top(px(y)).w(px(w));

        // The preview, framed at the picture's shape, its size under it.
        let (frame, _) = self.frame(scale, &l);
        let preview = self.shown_preview();
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
        let caption_y = rows.iter().map(|(top, h)| top + h / 2.0).find(|r| l.beside && (0.0..=m::LABEL_ROW / 2.0).contains(&(r - caption_y))).unwrap_or(caption_y);
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

        // Buttons ([`Layout::buttons`]).
        let idle = self.busy.is_none() && self.shot.is_some();
        let can_save = self.can_save(cx);
        let [copy, record, new, save] = l.buttons();
        let buttons = div()
            .child(at(copy.0, copy.1, copy.2).child(Button::new("Copy to Clipboard").min_width(copy.2).enabled(idle).on_click(cx.listener(|s: &mut Screenshot, _: &ClickEvent, cx| s.copy(cx)))))
            .child(at(record.0, record.1, record.2).child(Button::new("Record…").id("record").min_width(record.2).enabled(self.busy.is_none()).on_click(cx.listener(
                |_: &mut Screenshot, _: &ClickEvent, cx| {
                    let at = cx.listener_bounds();
                    cx.send(Msg::RecordMenu(at));
                },
            ))))
            .child(
                at(new.0, new.1, new.2).child(Button::new("New Screenshot").min_width(new.2).enabled(self.busy.is_none()).on_click(cx.listener(|s: &mut Screenshot, _: &ClickEvent, cx| s.take(cx)))),
            )
            // (Return saves: Save is the default button.)
            .child(at(save.0, save.1, save.2).child(Button::new("Save").default(true).min_width(save.2).enabled(can_save).on_click(cx.listener(|s: &mut Screenshot, _: &ClickEvent, cx| s.save(cx)))));

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
        // (Under the first free name, never over a file: two shortcuts at
        // once each keep their own picture.)
        let r = capture::save_new(&shot, &folder, settings.format);
        let _ = std::fs::remove_file(&shot.png);
        r
    });
    match result {
        Ok((path, id)) => saved(&path, id),
        Err(e) => {
            eprintln!("olo-screenshot: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Saved with no window: say where, and put that on the clipboard, held
/// until something else is copied (not with `--for-app`: the Screenshot
/// portal's, which hands the file to the app that asked). It's told of
/// in a notification once the path is on the clipboard (before, a Copy
/// pressed at once was overwritten by the path), its actions coming
/// back here while this holds it (saved.rs), and sent before this ends.
/// What it says is made first (the file `id` kept as Screenshot's, read
/// for its size): the clipboard, ready, only sends it.
fn saved(path: &Path, id: olo_protocol::notifications::FileId) -> std::process::ExitCode {
    let said = path.display().to_string();
    println!("{said}");
    if std::env::args().any(|a| a == "--for-app") {
        return std::process::ExitCode::SUCCESS;
    }
    let mut prepared = saved::prepare(path, saved::Identity::Written(id), saved::Kind::Picture, None);
    let mut notes = None;
    let mut tell = || {
        if let Some(p) = prepared.take() {
            notes = Some(p.send());
        }
    };
    if let Err(e) = held_clipboard::hold_then(held_clipboard::text(&said), &mut tell) {
        eprintln!("olo-screenshot: couldn't copy where it went: {e}");
    }
    // (No clipboard to hold: told all the same.)
    tell();
    saved::sent(notes.as_ref());
    std::process::ExitCode::SUCCESS
}

/// `--after PATH [--picture]`: what Screenshot asked of itself before a
/// saved file was told of in a notification (a recorder started before
/// an upgrade may still ask it): its notification. Whoever asks names
/// the file, so nothing is kept as Screenshot's here: its actions are
/// offered only if Screenshot already kept that very file; otherwise
/// it's shown, with nothing to do to it.
fn after(args: &[String]) -> std::process::ExitCode {
    let Some(path) = args.iter().position(|a| a == "--after").and_then(|i| args.get(i + 1)).map(PathBuf::from) else {
        eprintln!("olo-screenshot: --after takes the recording's or the picture's path");
        return std::process::ExitCode::from(2);
    };
    let kind = if args.iter().any(|a| a == "--picture") { saved::Kind::Picture } else { saved::Kind::Recording };
    let notes = saved::tell(&path, saved::Identity::Named, kind, None);
    saved::sent(notes.as_ref());
    std::process::ExitCode::SUCCESS
}

fn main() -> std::process::ExitCode {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // What a saved file's notification does (saved.rs), here or once
    // the program that told of it has gone: `--saved ACTION PATH
    // RECEIPT`, first and alone (what it's given is a notification's,
    // which anyone may send: nothing in it is read as another option).
    if args.first().is_some_and(|a| a == "--saved") {
        std::process::exit(saved::act_on(&args));
    }
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
    if args.iter().any(|a| a == "--saved") {
        eprintln!("olo-screenshot: --saved comes first, with what to do, the file's path and its receipt");
        return std::process::ExitCode::from(2);
    }
    if args.iter().any(|a| a == "--after") {
        return after(&args);
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
            Ok(Some((path, id, _))) => saved(&path, id),
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
                let mut options = WindowOptions::new("Screenshot", WIDTH, layout(WIDTH, DEFAULT_ASPECT, button_widths()).height).fits_content();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Held by tiling: its whole width where its preview can give way
    /// (349 × 565, a 16:10 picture: no bar and no 14 points empty);
    /// beside the bar only where even its least preview is too tall.
    /// Let be, but scrolled by the Frame (widened by the bar): laid out
    /// without the bar's width.
    #[test]
    fn held_or_scrolled_its_width_is_what_shows() {
        let l = layout_held(349.0, 565.0, 1.6, ENGLISH);
        assert_eq!(l.width, 349.0, "no phantom gutter");
        assert!(l.height <= 565.5, "it fits: no scroll bar");
        assert!(f32::from(l.area.size.height) >= PREVIEW_OVER_LEAST);
        let tall = layout_held(349.0, 760.0, 1.6, ENGLISH);
        assert_eq!((tall.width, tall.height), (layout(349.0, 1.6, ENGLISH).width, layout(349.0, 1.6, ENGLISH).height), "room to spare: as it is");
        let short = layout_held(349.0, 300.0, 1.6, ENGLISH);
        assert_eq!(short.width, 349.0 - olo_kit::widgets::SCROLL_BAR, "scrolled: beside the bar");
        assert!(short.height > 300.0);
        assert_eq!(unheld_width(WIDTH), WIDTH);
        assert_eq!(unheld_width(WIDTH + olo_kit::widgets::SCROLL_BAR), WIDTH);
    }

    /// The preview wanted, then another while the first is being made,
    /// then the first again: the first, made, is kept and drawn; nothing
    /// else is made meanwhile, and one made for a size not wanted any
    /// more is never kept.
    #[test]
    fn the_preview_wanted_is_the_one_kept() {
        let (a, b) = ((1, (400, 300)), (1, (300, 200)));
        let mut q = Previews::default();
        assert_eq!(q.want(a, None), Some(a), "A made");
        assert_eq!(q.want(b, None), None, "B waits: A is being made");
        assert_eq!(q.want(a, None), None, "back to A: still being made");
        assert!(q.made(a, true), "A in, A wanted: kept");
        assert_eq!(q.want(a, Some(a)), None, "made already");
        // A size wanted, then left: the one made late isn't kept, and the
        // one wanted then is made next.
        assert_eq!(q.want(b, Some(a)), Some(b));
        assert_eq!(q.want(a, Some(a)), None);
        assert!(!q.made(b, true), "B, made late, not kept");
        // One that couldn't be made isn't asked for again.
        assert_eq!(q.want(b, Some(a)), Some(b));
        assert!(!q.made(b, false));
        assert_eq!(q.want(b, Some(a)), None);
    }

    /// English's buttons ([`button_widths`] at their least), and words
    /// longer than English's (German's Copy to Clipboard is 210 points).
    const ENGLISH: [f32; 4] = [130.0, 80.0, 120.0, 80.0];
    const LONG: [f32; 4] = [210.0, 96.0, 150.0, 80.0];

    /// At its own width: the preview beside the form, the first check
    /// box's top level with the preview's, the buttons on one row.
    #[test]
    fn its_own_width_is_as_it_was() {
        let l = layout(WIDTH, DEFAULT_ASPECT, ENGLISH);
        assert!(l.beside && l.button_rows == 1);
        assert_eq!((f32::from(l.area.origin.x), f32::from(l.area.origin.y), f32::from(l.area.size.width), f32::from(l.area.size.height)), (16.0, 16.0, 240.0, 204.0));
        assert_eq!(l.rows[0].0 + (m::MARK_ROW - m::MARK_SIZE) / 2.0, f32::from(l.area.origin.y), "the check box level with the preview");
        assert_eq!((l.form_x, l.form_w, l.buttons_y, l.last_row, l.height), (284.0, 280.0, 258.0, 258.0, 298.0));
    }

    /// Held narrower by tiling (a quarter, 530 points; a third, 349, the
    /// narrowest on the smallest screen; and narrower), everything is
    /// inside the window: the form's fields, the preview and the four
    /// buttons, each its words' width at least, Save at the bottom right,
    /// no two over each other, rows of them a spacing apart (room for
    /// Save's ring), in columns or one a row.
    /// On one row, the buttons a spacing apart face to face, Save's ring
    /// drawn into its gap as the kit's alerts and Doctor's footer have
    /// it. (Its ring added to the gap, the review saw them 12 and 15 pt
    /// apart in a quarter, an error: tilecheck, 2026-11-28.)
    #[test]
    fn one_rows_buttons_are_evenly_apart() {
        let mut one_row = 0;
        for widths in [ENGLISH, LONG] {
            for width in [WIDTH, 590.0, 530.0, 500.0, 478.0] {
                let l = layout(width, 1.38, widths);
                if l.button_rows != 1 {
                    continue;
                }
                one_row += 1;
                let [copy, record, new, save] = l.buttons();
                assert_eq!(record.0 - (copy.0 + copy.2), m::SPACING, "{width} {widths:?}: Copy to Record…");
                assert_eq!(save.0 - (new.0 + new.2), m::SPACING, "{width} {widths:?}: New Screenshot to Save");
                assert!(new.0 - (record.0 + record.2) >= m::SPACING, "{width} {widths:?}: Record… to New Screenshot");
            }
        }
        assert!(one_row >= 3, "a quarter and wider on one row");
    }

    #[test]
    fn everything_fits_a_quarter_and_a_third() {
        let inset = m::WINDOW_INSET;
        for widths in [ENGLISH, LONG] {
            for width in [WIDTH, 530.0, 500.0, 478.0, 420.0, 349.0, 335.0, 317.0, 304.0] {
                let l = layout(width, 1.38, widths);
                assert!(l.form_x >= inset && l.form_x + l.form_w <= width - inset + 0.01, "{width}: the form");
                assert!(l.form_w >= FORM_LEAST.min(width - 2.0 * inset), "{width}: the form's room");
                assert!(f32::from(l.area.right()) <= width - inset, "{width}: the preview");
                assert!(l.beside || f32::from(l.area.bottom()) + m::TIGHT + m::LABEL_ROW <= l.rows[0].0, "{width}: the caption over the form");
                let b = l.buttons();
                for (i, (x, y, w)) in b.iter().enumerate() {
                    assert!(*x >= inset && x + w <= width - inset + 0.01, "{width} {widths:?}: button {i} at {x}, {w} wide");
                    assert!(*w >= widths[i], "{width} {widths:?}: button {i} narrower than its words");
                    assert!(*y >= l.rows[7].0 + l.rows[7].1 + m::SPACING, "{width}: button {i} under the form");
                }
                assert_eq!(b[3].0 + b[3].2, width - inset - DEFAULT_RING, "{width}: Save's ring at the right");
                assert_eq!(b[3].1, l.last_row, "{width}: Save on the last row");
                for i in 0..4 {
                    for j in i + 1..4 {
                        let ((xi, yi, wi), (xj, yj, wj)) = (b[i], b[j]);
                        let apart = yi + m::CONTROL_HEIGHT + m::SPACING <= yj || yj + m::CONTROL_HEIGHT + m::SPACING <= yi || xi + wi + m::SPACING <= xj || xj + wj + m::SPACING <= xi;
                        assert!(apart, "{width} {widths:?}: buttons {i} and {j} too close");
                    }
                }
                assert_eq!(l.height, l.last_row + m::CONTROL_HEIGHT + inset);
            }
        }
        let third = layout(349.0, 1.38, ENGLISH);
        assert!(!third.beside && third.button_rows == 2, "a third: the preview over the form, two rows of buttons");
        assert_eq!(f32::from(third.area.size.width), 317.0, "the preview as wide as the window");
        assert_eq!(layout(349.0, 1.38, LONG).button_rows, 4, "longer words: one a row");
        assert!(layout(530.0, 1.38, ENGLISH).beside && layout(530.0, 1.38, ENGLISH).button_rows == 1, "a quarter: as wide");
        // Held taller (tiling): the buttons at the foot, a window's inset over it.
        let pinned = layout(349.0, 1.38, ENGLISH).pinned(200.0);
        assert_eq!(pinned.last_row - third.last_row, 200.0);
        assert_eq!(pinned.height, third.height + 200.0);
    }
}
