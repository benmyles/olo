//! Screenshot, after Haiku's: takes a picture of the screen as it starts,
//! shows it, and offers to save it (any of several formats, into a folder
//! of your choice), copy it, or take a new one, of the whole screen or the
//! active window, with or without its border and the pointer, after a
//! delay. Our own window steps aside while a picture is taken.
//!
//! `--silent [--window [ID]]` (⌘⇧3, Print Screen, Alt+Print Screen) saves one
//! straight into the folder, as `screenshotN`, with no window; `--region`
//! (⌘⇧4) lets you drag out part of the screen first (region.rs). However
//! it is saved, the file's path goes on the clipboard.

mod capture;
mod region;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use capture::{Format, Settings, Shot};
use serde::{Deserialize, Serialize};
use olo_kit::{
    App, Cx, WindowId, WindowOptions,
    egui::{self, Color32, ColorImage, Rect, TextureHandle, TextureOptions, pos2, vec2},
    file_panel::{FilePanel, PANEL_SIZE, PanelResult},
    instance::{self, Launch},
    metrics as m,
    style,
    widgets::{self, Button, CheckBox, MenuField, StringView, TextControl},
};

const APP_ID: &str = "olo.screenshot";

const SIZE: (u32, u32) = (580, 276);
/// The preview's share of the window.
const PREVIEW_W: f32 = 240.0;
/// Labels' column in the form.
const DIVIDER: f32 = 66.0;
/// The largest preview texture side, in pixels (twice the preview, for 2×).
const PREVIEW_PX: u32 = 2 * PREVIEW_W as u32;

enum Msg {
    /// A picture (and its preview), or why not.
    Taken(Result<(Shot, ColorImage), String>),
    Saved(Result<PathBuf, String>),
    Copied(Result<Arc<[u8]>, String>),
    /// The first free name in a folder.
    Named(PathBuf, String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Busy {
    Taking,
    Saving,
    Copying,
}

enum Win {
    Main,
    Folder(FilePanel),
    Error(String),
}

struct Screenshot {
    settings: Settings,
    windows: HashMap<WindowId, Win>,
    main: Option<WindowId>,
    shot: Option<Shot>,
    preview: Option<ColorImage>,
    texture: Option<TextureHandle>,
    name: String,
    /// The name as we proposed it; typing over it keeps the user's.
    proposed: String,
    delay: String,
    busy: Option<Busy>,
    /// What happened last ("Copied to the clipboard"), under the preview.
    note: String,
    /// Folders offered in "Save in:" besides the chosen one.
    folders: Vec<PathBuf>,
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

/// The preview texture: no bigger than the preview needs, made on the
/// worker that took the picture.
fn preview_of(image: &image::RgbaImage) -> ColorImage {
    let (w, h) = (image.width().max(1), image.height().max(1));
    let k = (PREVIEW_PX as f32 / w.max(h) as f32).min(1.0);
    let (pw, ph) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
    let small = if k < 1.0 { image::imageops::thumbnail(image, pw, ph) } else { image.clone() };
    ColorImage::from_rgba_unmultiplied([small.width() as usize, small.height() as usize], small.as_raw())
}

impl Screenshot {
    fn new(settings: Settings) -> Self {
        let mut folders = Vec::new();
        for f in [Some(capture::home()), olo_kit::user_dir("DESKTOP"), olo_kit::user_dir("PICTURES")].into_iter().flatten() {
            if !folders.contains(&f) {
                folders.push(f);
            }
        }
        let delay = settings.delay.to_string();
        Screenshot {
            settings,
            windows: HashMap::new(),
            main: None,
            shot: None,
            preview: None,
            texture: None,
            name: String::new(),
            proposed: String::new(),
            delay,
            busy: None,
            note: String::new(),
            folders,
        }
    }

    /// Take a picture on a worker; our window steps aside if it is up.
    fn take(&mut self, cx: &mut Cx) {
        self.busy = Some(Busy::Taking);
        self.note.clear();
        let settings = self.settings.clone();
        let hide = self.main.is_some();
        let proxy = cx.proxy();
        std::thread::spawn(move || {
            let r = capture::take(&settings, hide).map(|shot| {
                let preview = preview_of(&shot.image);
                (shot, preview)
            });
            proxy.send(Msg::Taken(r));
        });
    }

    /// Show a picture taken earlier (after an in-place reload).
    fn reload(&mut self, cx: &mut Cx, png: PathBuf) {
        self.busy = Some(Busy::Taking);
        let proxy = cx.proxy();
        std::thread::spawn(move || {
            let r = image::open(&png).map_err(|e| e.to_string()).map(|img| {
                let image = Arc::new(img.into_rgba8());
                let preview = preview_of(&image);
                (Shot { png, image }, preview)
            });
            proxy.send(Msg::Taken(r));
        });
    }

    /// Find the next free name in the folder, off the UI thread.
    fn propose_name(&self, cx: &mut Cx) {
        let folder = self.settings.folder();
        let proxy = cx.proxy();
        std::thread::spawn(move || {
            let name = capture::next_name(&folder);
            proxy.send(Msg::Named(folder, name));
        });
    }

    fn open_main(&mut self, cx: &mut Cx) {
        if let Some(id) = self.main {
            cx.activate(id);
            return;
        }
        let id = cx.open_window(WindowOptions::new("Screenshot", SIZE.0, SIZE.1).min_size(SIZE.0, SIZE.1).max_size(SIZE.0, SIZE.1));
        self.windows.insert(id, Win::Main);
        self.main = Some(id);
    }

    fn show_error(&mut self, cx: &mut Cx, message: String) {
        let id = cx.open_window(WindowOptions::alert("Screenshot", &message));
        self.windows.insert(id, Win::Error(message));
    }

    fn save(&mut self, cx: &mut Cx) {
        let Some(shot) = self.shot.clone() else { return };
        let path = self.settings.folder().join(capture::file_name(&self.name, self.settings.format));
        let format = self.settings.format;
        self.busy = Some(Busy::Saving);
        let proxy = cx.proxy();
        std::thread::spawn(move || {
            let r = capture::save(&shot, &path, format).map(|()| path);
            proxy.send(Msg::Saved(r));
        });
    }

    fn copy(&mut self, cx: &mut Cx) {
        let Some(shot) = self.shot.clone() else { return };
        self.busy = Some(Busy::Copying);
        let proxy = cx.proxy();
        std::thread::spawn(move || {
            let r = std::fs::read(&shot.png).map(Arc::from).map_err(|e| e.to_string());
            proxy.send(Msg::Copied(r));
        });
    }

    fn settings_changed(&mut self) {
        self.settings.store();
    }

    fn main_ui(&mut self, cx: &mut Cx, ui: &mut egui::Ui) {
        let p = style::palette(ui.ctx());
        if let Some(image) = self.preview.take() {
            self.texture = Some(ui.ctx().load_texture("screenshot", image, TextureOptions::LINEAR));
        }
        let full = ui.max_rect().shrink(m::WINDOW_INSET);
        let buttons = Rect::from_min_max(pos2(full.left(), full.bottom() - m::CONTROL_HEIGHT), full.max);
        let top = Rect::from_min_max(full.min, pos2(full.right(), buttons.top() - m::WINDOW_INSET));

        // The preview, framed at the picture's shape.
        let area = Rect::from_min_size(top.min, vec2(PREVIEW_W, top.height() - m::LABEL_ROW - m::TIGHT));
        let inner = area.shrink(2.0);
        let frame = match (&self.texture, &self.shot) {
            (Some(_), Some(shot)) => {
                let (w, h) = (shot.image.width().max(1) as f32, shot.image.height().max(1) as f32);
                let k = (inner.width() / w).min(inner.height() / h);
                let size = vec2((w * k).round(), (h * k).round());
                Rect::from_min_size(pos2(inner.center().x - (size.x / 2.0).round(), inner.top()), size).expand(2.0)
            }
            _ => area,
        };
        if let Some(t) = &self.texture {
            ui.painter().image(t.id(), frame.shrink(2.0), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        style::sunken(ui.painter(), frame, &p, false);
        let caption = match (self.busy, &self.shot) {
            (Some(Busy::Taking), _) => "Taking a screenshot…".to_owned(),
            (Some(Busy::Saving), _) => "Saving…".to_owned(),
            (Some(Busy::Copying), _) => "Copying…".to_owned(),
            (None, _) if !self.note.is_empty() => self.note.clone(),
            (None, Some(s)) => format!("{} × {} pixels", s.image.width(), s.image.height()),
            (None, None) => String::new(),
        };
        // Under the picture, so where it falls follows the picture's
        // shape: painted once the form beside it is laid out.
        let caption_y = frame.bottom() + m::TIGHT + m::LABEL_ROW / 2.0;
        let mut rows = Vec::new();

        // The form.
        let form = Rect::from_min_max(pos2(area.right() + m::WINDOW_INSET + m::SPACING, top.top()), top.max);
        let mut changed = false;
        let mut choose_folder = false;
        ui.scope_builder(egui::UiBuilder::new().max_rect(form).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
            ui.spacing_mut().item_spacing.y = m::TIGHT;
            changed |= ui.add(CheckBox::new(&mut self.settings.window, "Capture active window")).changed();
            changed |= ui.add_enabled(self.settings.window, CheckBox::new(&mut self.settings.border, "Include window border")).changed();
            changed |= ui.add(CheckBox::new(&mut self.settings.pointer, "Include mouse pointer")).changed();
            ui.add_space(m::TIGHT);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = m::LABEL_GAP;
                let r = ui.add(TextControl::new(&mut self.delay).label("Delay:").divider(DIVIDER).width(DIVIDER + 44.0));
                if r.changed() {
                    self.delay.retain(|c| c.is_ascii_digit());
                    self.delay.truncate(2);
                    let d = self.delay.parse::<u32>().unwrap_or(0).min(60);
                    if d != self.settings.delay {
                        self.settings.delay = d;
                        changed = true;
                    }
                }
                ui.add(StringView::new("seconds"));
                rows.push(r.rect.center().y);
            });
            ui.add_space(m::SPACING);
            widgets::separator(ui);
            ui.add_space(m::SPACING);
            let width = form.width();
            rows.push(ui.add(TextControl::new(&mut self.name).label("Name:").divider(DIVIDER).width(width)).rect.center().y);

            let labels: Vec<&str> = Format::ALL.iter().map(|f| f.label()).collect();
            let mut fi = Format::ALL.iter().position(|f| *f == self.settings.format).unwrap_or(0);
            let r = ui.add(MenuField::new("format", &mut fi, &labels).label("Save as:").divider(DIVIDER).width(width));
            rows.push(r.rect.center().y);
            if r.changed() {
                self.settings.format = Format::ALL[fi];
                changed = true;
            }

            // The chosen folder first if it isn't a usual one, then
            // Choose folder….
            let current = self.settings.folder();
            let mut folders = self.folders.clone();
            if !folders.contains(&current) {
                folders.insert(0, current.clone());
            }
            let mut names: Vec<String> = folders.iter().map(|f| folder_label(f)).collect();
            names.push("Choose folder…".into());
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            let mut di = folders.iter().position(|f| *f == current).unwrap_or(0);
            let r = ui.add(MenuField::new("folder", &mut di, &names).label("Save in:").divider(DIVIDER).width(width));
            rows.push(r.rect.center().y);
            if r.changed() {
                match folders.get(di) {
                    Some(f) => {
                        self.settings.folder = Some(f.clone());
                        changed = true;
                    }
                    None => choose_folder = true,
                }
            }
        });
        // Almost level with a row of the form, it's lined up with it (a
        // little further from the picture); else it's on a grid of its
        // own, level with the form only at some shapes.
        let y = rows.iter().copied().find(|r| (0.0..=m::LABEL_ROW / 2.0).contains(&(r - caption_y))).unwrap_or(caption_y);
        olo_kit::layout_review::own_grid(ui, Rect::from_center_size(pos2(area.center().x, y), vec2(area.width(), m::LABEL_ROW)));
        style::label(ui.painter(), area.center().x, y, egui::Align::Center, &caption, style::plain_font(), style::disabled_text(&p));
        if changed {
            self.settings_changed();
            // Keep a proposed name current; a typed one stays.
            if self.name == self.proposed {
                self.propose_name(cx);
            }
        }
        if choose_folder {
            let id = cx.open_window(WindowOptions::new("Choose a Folder", PANEL_SIZE.0, PANEL_SIZE.1).min_size(400, 300).floating());
            self.windows.insert(id, Win::Folder(FilePanel::folder(self.settings.folder())));
        }

        // Buttons.
        let idle = self.busy.is_none() && self.shot.is_some();
        let copy_r = Rect::from_min_size(buttons.min, vec2(130.0, m::CONTROL_HEIGHT));
        if ui.add_enabled_ui(idle, |ui| ui.put(copy_r, Button::new("Copy to clipboard"))).inner.clicked() {
            self.copy(cx);
        }
        let bw = m::BUTTON_MIN_WIDTH;
        let save_r = Rect::from_min_size(pos2(buttons.right() - bw, buttons.top()), vec2(bw, m::CONTROL_HEIGHT));
        let new_r = Rect::from_min_size(pos2(save_r.left() - m::SPACING - 2.0 - 120.0, buttons.top()), vec2(120.0, m::CONTROL_HEIGHT));
        if ui.add_enabled_ui(self.busy.is_none(), |ui| ui.put(new_r, Button::new("New screenshot"))).inner.clicked() {
            self.take(cx);
        }
        let can_save = idle && !self.name.trim().is_empty();
        let save = ui.add_enabled_ui(can_save, |ui| ui.put(save_r, Button::new("Save").default(true))).inner;
        let enter = can_save && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if save.clicked() || enter {
            self.save(cx);
        }
    }
}

impl App for Screenshot {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui) {
        match self.windows.get_mut(&window) {
            Some(Win::Main) => self.main_ui(cx, ui),
            Some(Win::Folder(panel)) => {
                if let Some(result) = panel.ui(ui) {
                    if let PanelResult::Chosen(dir) = result {
                        self.settings.folder = Some(dir);
                        self.settings_changed();
                        if self.name == self.proposed {
                            self.propose_name(cx);
                        }
                    }
                    self.windows.remove(&window);
                    cx.close_window(window);
                    if let Some(main) = self.main {
                        cx.redraw(main);
                    }
                }
            }
            Some(Win::Error(message)) => {
                let message = message.clone();
                if widgets::alert(ui, "warning", &message, &["OK"]).is_some() {
                    self.windows.remove(&window);
                    cx.close_window(window);
                    if self.main.is_none() && self.windows.is_empty() {
                        cx.quit();
                    }
                }
            }
            None => {}
        }
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        if Some(window) == self.main {
            // Quitting (the kit stays on for the clipboard if need be).
            cx.quit();
        } else {
            self.windows.remove(&window);
            cx.close_window(window);
        }
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        if message.is::<Launch>() {
            // Launched again: bring the window forward.
            self.open_main(cx);
            return;
        }
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Taken(r) => {
                self.busy = None;
                match r {
                    Ok((shot, preview)) => {
                        self.shot = Some(shot);
                        self.preview = Some(preview);
                        if self.name.is_empty() || self.name == self.proposed {
                            self.propose_name(cx);
                        }
                    }
                    Err(e) => self.show_error(cx, format!("Couldn't take a screenshot.\n\n{e}")),
                }
                self.open_main(cx);
            }
            Msg::Named(folder, name) => {
                if folder == self.settings.folder() && (self.name.is_empty() || self.name == self.proposed) {
                    self.name = name.clone();
                    self.proposed = name;
                }
            }
            Msg::Saved(r) => {
                self.busy = None;
                match r {
                    // Done, as in Haiku, with where it went on the
                    // clipboard (the kit stays on to hand it out).
                    Ok(path) => {
                        cx.copy(path.display().to_string());
                        cx.quit();
                    }
                    Err(e) => self.show_error(cx, format!("Couldn't save the screenshot.\n\n{e}")),
                }
            }
            Msg::Copied(r) => {
                self.busy = None;
                match r {
                    Ok(png) => {
                        cx.copy_data(vec![("image/png".to_owned(), png)]);
                        self.note = "Copied to the clipboard".into();
                    }
                    Err(e) => self.show_error(cx, format!("Couldn't copy the screenshot.\n\n{e}")),
                }
            }
        }
        cx.redraw_all();
    }

    fn save_state(&mut self, _cx: &Cx) -> Option<String> {
        serde_json::to_string(&Saved { png: self.shot.as_ref().map(|s| s.png.clone()), name: self.name.clone() }).ok()
    }
}

impl Drop for Screenshot {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(capture::scratch_path());
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
    if let Err(e) = olo_kit::clipboard::hold(olo_kit::clipboard::text(&path)) {
        eprintln!("olo-screenshot: couldn't copy where it went: {e}");
    }
    std::process::ExitCode::SUCCESS
}

fn main() -> std::process::ExitCode {
    // A session or install from before the rename (Tabula) carries on.
    olo_kit::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
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
    olo_kit::run(APP_ID, |cx| {
        instance::listen(APP_ID, cx.proxy());
        let mut app = Screenshot::new(Settings::load());
        match cx.restored_state().and_then(|s| serde_json::from_str::<Saved>(&s).ok()) {
            Some(Saved { png: Some(png), name }) if png.is_file() => {
                app.name = name;
                app.reload(cx, png);
            }
            // The picture comes first, then the window (so it isn't in it).
            _ => app.take(cx),
        }
        app
    });
    std::process::ExitCode::SUCCESS
}
