//! Agent Screens, the window: the agents' screens in a list, the chosen
//! one live beside it (about eight frames a second, with the agent's
//! pointer), and Take Control to step in: the person's pointer and keys
//! over the picture then go to that screen (never to their own), until
//! they choose Stop Controlling.
//!
//! Frames come from a worker (the screen's app_server writes raw pixels,
//! read back here), and input goes out through another, so the window
//! never waits on a screen.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, Sender, channel},
    },
    time::{Duration, Instant},
};

use olo_kit::egui::{self, Align, Color32, ColorImage, Key, Pos2, Rect, TextureHandle, TextureOptions, Ui, pos2, vec2};
use serde::{Deserialize, Serialize};
use olo_kit::{
    App, Cx, Menu, Proxy, WindowId, WindowOptions,
    metrics as m, style,
    widgets::{self, Button, Cell, Column, ColumnListView, ListState},
};
use olo_protocol::{ImageFormat, SyntheticEvent};

use crate::screens::{self, Screen};

/// Frames a second, at most.
const FPS: f64 = 8.0;
const LIST_W: f32 = 180.0;

enum Msg {
    Screens(Vec<Screen>),
    /// Every screen to list: running, or stopped with a history kept;
    /// each with when it was last used, most recent first.
    Rows(Vec<Row>),
    Frame { name: String, image: ColorImage },
    Failed { name: String, message: String },
    Started(Result<Screen, String>),
    Stopped(Result<(), String>),
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    selected: Option<String>,
}

enum Win {
    Main,
    /// The screen's name, and the question.
    ConfirmStop(String, String),
    Alert(String),
    /// Where to save a screen's history as a video.
    ExportTo(Box<olo_kit::file_panel::FilePanel>),
}

pub struct Viewer {
    windows: HashMap<WindowId, Win>,
    main: Option<WindowId>,
    screens: Vec<Screen>,
    list: ListState,
    selected: Option<String>,
    /// Which screen the frame worker shows (shared with it).
    watching: Arc<Mutex<Option<String>>>,
    texture: Option<(String, TextureHandle)>,
    /// The newest frame, until the window draws it.
    frame: Option<(String, ColorImage)>,
    failure: Option<String>,
    controlling: bool,
    /// Modifiers held down on the screen while controlling.
    held: [bool; 3],
    input: Sender<(Screen, Vec<SyntheticEvent>)>,
    busy: bool,
    /// Showing the chosen screen's history rather than the screen live,
    /// and that history.
    history: bool,
    replay: Option<crate::replay::Replay>,
    rows: Vec<Row>,
}

/// A screen in the list.
#[derive(Debug, Clone)]
pub struct Row {
    name: String,
    last_used: Option<u64>,
}

/// The screens running (`running`) and those stopped with a history, by
/// last use, most recent first. Reads the histories: off the UI thread.
fn rows(running: &[Screen]) -> Vec<Row> {
    let own = std::env::var("OLO_SCREEN").ok();
    let mut names: Vec<String> = running.iter().map(|s| s.name.clone()).collect();
    let kept: Vec<String> = crate::history::names().into_iter().filter(|n| !names.contains(n) && Some(n) != own.as_ref()).collect();
    names.extend(kept);
    let mut rows: Vec<Row> = names
        .into_iter()
        .map(|name| {
            let last_used = crate::history::read(&name).iter().map(|a| a.at + a.ms).max();
            Row { name, last_used }
        })
        .collect();
    rows.sort_by_key(|r| (std::cmp::Reverse(r.last_used), r.name.clone()));
    rows
}

/// "3 h ago".
fn ago(ms: u64, now: u64) -> String {
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86_400),
    }
}

/// The screens to show: all but the one this viewer itself is on, if it
/// runs on one (it would show itself, endlessly).
fn listed() -> Vec<Screen> {
    let own = std::env::var("OLO_SCREEN").ok();
    screens::list().into_iter().filter(|s| Some(&s.name) != own.as_ref()).collect()
}

/// Follow the list of screens as their files come and go
/// (`olo_kit::watch`), and now and then for one whose session was killed
/// without a word (its file left behind).
fn watch_list(proxy: Proxy) {
    let last = Arc::new(Mutex::new(None::<Vec<Screen>>));
    let look = move || {
        let now = listed();
        let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
        if last.as_ref() != Some(&now) {
            proxy.send(Msg::Screens(now.clone()));
            *last = Some(now);
        }
    };
    look();
    let look = Arc::new(look);
    let on_change = look.clone();
    let dir = screens::dir();
    let followed = dir.clone();
    if let Some(w) = olo_kit::watch::watch([dir], move |changed| {
        // A screen's own file, not a viewer's frame passing through.
        let items: Vec<&std::path::PathBuf> = changed.iter().filter(|p| p.parent() == Some(followed.as_path())).collect();
        if items.is_empty() || items.iter().any(|p| p.extension().is_some_and(|e| e == "json")) {
            on_change();
        }
    }) {
        // For as long as the app runs.
        std::mem::forget(w);
    }
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(30));
            look();
        }
    });
}

/// Fetch frames of whichever screen is being watched.
fn watch_frames(proxy: Proxy, watching: Arc<Mutex<Option<String>>>) {
    std::thread::spawn(move || {
        // In a folder of their own: the screens' folder is followed for
        // screens coming and going, not for frames.
        let views = screens::dir().join("views");
        let path = views.join(format!("view-{}.rgba", std::process::id()));
        let _ = std::fs::create_dir_all(&views);
        loop {
            let began = Instant::now();
            let name = watching.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(name) = name {
                let frame = screens::get(&name).and_then(|s| {
                    s.picture(&path, None, true, ImageFormat::Rgba)?;
                    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                    decode(&bytes).ok_or_else(|| "the picture was cut short".to_owned())
                });
                match frame {
                    Ok(image) => proxy.send(Msg::Frame { name, image }),
                    Err(message) => proxy.send(Msg::Failed { name, message }),
                }
            }
            let period = Duration::from_secs_f64(1.0 / FPS);
            std::thread::sleep(period.saturating_sub(began.elapsed()));
        }
    });
}

/// Width, height (little-endian u32), then straight RGBA rows.
fn decode(bytes: &[u8]) -> Option<ColorImage> {
    let w = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?) as usize;
    let h = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
    let pixels = bytes.get(8..8 + w * h * 4)?;
    Some(ColorImage::from_rgba_unmultiplied([w, h], pixels))
}

/// Send the person's input on, in order, off the UI thread.
fn send_input() -> Sender<(Screen, Vec<SyntheticEvent>)> {
    let (tx, rx): (Sender<(Screen, Vec<SyntheticEvent>)>, Receiver<_>) = channel();
    std::thread::spawn(move || {
        for (screen, events) in rx {
            if let Err(e) = screen.input(events) {
                tracing::warn!("input to screen {}: {e}", screen.name);
            }
        }
    });
    tx
}

/// The evdev code of a key the person pressed (by its place on the
/// keyboard where egui knows it).
fn evdev(key: Key) -> Option<u32> {
    use Key::*;
    Some(match key {
        Escape => 1,
        Num1 => 2,
        Num2 => 3,
        Num3 => 4,
        Num4 => 5,
        Num5 => 6,
        Num6 => 7,
        Num7 => 8,
        Num8 => 9,
        Num9 => 10,
        Num0 => 11,
        Minus => 12,
        Equals | Plus => 13,
        Backspace => 14,
        Tab => 15,
        Q => 16,
        W => 17,
        E => 18,
        R => 19,
        T => 20,
        Y => 21,
        U => 22,
        I => 23,
        O => 24,
        P => 25,
        OpenBracket | OpenCurlyBracket => 26,
        CloseBracket | CloseCurlyBracket => 27,
        Enter => 28,
        A => 30,
        S => 31,
        D => 32,
        F => 33,
        G => 34,
        H => 35,
        J => 36,
        K => 37,
        L => 38,
        Semicolon | Colon => 39,
        Quote => 40,
        Backtick => 41,
        Backslash | Pipe => 43,
        Z => 44,
        X => 45,
        C => 46,
        V => 47,
        B => 48,
        N => 49,
        M => 50,
        Comma => 51,
        Period => 52,
        Slash | Questionmark => 53,
        Space => 57,
        F1 => 59,
        F2 => 60,
        F3 => 61,
        F4 => 62,
        F5 => 63,
        F6 => 64,
        F7 => 65,
        F8 => 66,
        F9 => 67,
        F10 => 68,
        F11 => 87,
        F12 => 88,
        Home => 102,
        ArrowUp => 103,
        PageUp => 104,
        ArrowLeft => 105,
        ArrowRight => 106,
        End => 107,
        ArrowDown => 108,
        PageDown => 109,
        Insert => 110,
        Delete => 111,
        _ => return None,
    })
}

impl Viewer {
    pub fn new(cx: &mut Cx) -> Viewer {
        let saved: Saved = cx.restored_state().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let watching = Arc::new(Mutex::new(None));
        watch_list(cx.proxy());
        watch_frames(cx.proxy(), watching.clone());
        let mut v = Viewer {
            windows: HashMap::new(),
            main: None,
            screens: listed(),
            list: ListState::default(),
            selected: saved.selected,
            watching,
            texture: None,
            frame: None,
            failure: None,
            controlling: false,
            held: [false; 3],
            input: send_input(),
            busy: false,
            history: false,
            replay: None,
            rows: Vec::new(),
        };
        if v.selected.as_ref().is_none_or(|s| !v.screens.iter().any(|x| &x.name == s)) {
            v.selected = v.screens.first().map(|s| s.name.clone());
        }
        v.watch();
        v.open_main(cx);
        v
    }

    pub fn open_main(&mut self, cx: &mut Cx) {
        if let Some(id) = self.main {
            cx.activate(id);
            return;
        }
        let id = cx.open_window(WindowOptions::new("Agent Screens", 980, 600).min_size(560, 360));
        self.windows.insert(id, Win::Main);
        self.main = Some(id);
    }

    /// Choose `name` (from the command line, or a forwarded start).
    pub fn show(&mut self, cx: &mut Cx, name: Option<String>) {
        if let Some(name) = name {
            self.select(Some(name));
        }
        self.open_main(cx);
    }

    fn screen(&self) -> Option<&Screen> {
        self.selected.as_ref().and_then(|n| self.screens.iter().find(|s| &s.name == n))
    }

    fn select(&mut self, name: Option<String>) {
        if self.selected != name {
            self.stop_controlling();
            self.selected = name;
            self.failure = None;
            self.watch();
        }
    }

    fn watch(&self) {
        *self.watching.lock().unwrap_or_else(|e| e.into_inner()) = self.selected.clone();
    }

    fn redraw(&self, cx: &mut Cx) {
        if let Some(id) = self.main {
            cx.redraw(id);
        }
    }

    fn stop_controlling(&mut self) {
        if !self.controlling {
            return;
        }
        // Nothing left held down on the screen.
        let up: Vec<SyntheticEvent> = [29, 42, 56]
            .iter()
            .zip(self.held)
            .filter(|(_, held)| *held)
            .map(|(&key, _)| SyntheticEvent::Key { key, pressed: false })
            .collect();
        if let Some(screen) = self.screen().cloned()
            && !up.is_empty()
        {
            let _ = self.input.send((screen, up));
        }
        self.held = [false; 3];
        self.controlling = false;
    }

    fn alert(&mut self, cx: &mut Cx, message: &str) {
        let id = cx.open_window(WindowOptions::alert("Agent Screens", message).parent_opt(self.main));
        self.windows.insert(id, Win::Alert(message.to_owned()));
    }

    fn new_screen(&mut self, cx: &mut Cx) {
        if self.busy {
            return;
        }
        self.busy = true;
        let proxy = cx.proxy();
        std::thread::spawn(move || proxy.send(Msg::Started(screens::start(&screens::free_name(), &screens::Options::default()))));
        self.redraw(cx);
    }

    fn main_ui(&mut self, cx: &mut Cx, ui: &mut Ui) {
        let p = style::palette(ui.ctx());
        let c = style::c;
        let full = ui.max_rect().shrink(m::WINDOW_INSET);
        if self.rows.is_empty() {
            let text = "No agent screens. An agent starts one with “olo-screen start”, or choose New Screen.";
            style::label(ui.painter(), full.center().x, full.center().y - m::CONTROL_HEIGHT, Align::Center, text, style::plain_font(), c(p.panel_text));
            let r = Rect::from_center_size(pos2(full.center().x, full.center().y + m::SPACING), vec2(120.0, m::CONTROL_HEIGHT));
            let label = if self.busy { "Starting…" } else { "New Screen" };
            if ui.add_enabled_ui(!self.busy, |ui| ui.put(r, Button::new(label))).inner.clicked() {
                self.new_screen(cx);
            }
            return;
        }

        // The screens, then the chosen one.
        let list_r = Rect::from_min_size(full.min, vec2(LIST_W, full.height()));
        let columns = [Column::new("Screen", LIST_W - 2.0)];
        let names: Vec<String> = self.rows.iter().map(|r| r.name.clone()).collect();
        let now = crate::history::now_ms();
        let sizes: Vec<String> = self
            .rows
            .iter()
            .map(|r| match self.screens.iter().find(|s| s.name == r.name) {
                Some(s) => format!("{} × {}", s.width, s.height),
                None => format!("stopped · {}", r.last_used.map_or_else(|| "never used".to_owned(), |t| ago(t, now))),
            })
            .collect();
        self.list.selected = self.selected.as_ref().and_then(|n| names.iter().position(|x| x == n)).into_iter().collect();
        let out = ui
            .scope_builder(egui::UiBuilder::new().max_rect(list_r), |ui| {
                ColumnListView::new("screens-list", &columns, names.len(), &mut self.list).show(ui, |cell: &Cell| {
                    let job = {
                        let mut job = egui::text::LayoutJob::default();
                        let p = style::palette(cell.painter.ctx());
                        job.append(&names[cell.row], 0.0, egui::TextFormat::simple(style::plain_font(), cell.text_color));
                        let dim = if cell.selected { cell.text_color } else { c(olo_theme::mix(p.list_item_text, p.list_background, 0.45)) };
                        job.append(&format!("  {}", sizes[cell.row]), 0.0, egui::TextFormat::simple(style::plain_font(), dim));
                        job
                    };
                    cell.text_job(job);
                })
            })
            .inner;
        if out.selection_changed {
            let name = self.list.single().and_then(|i| names.get(i)).cloned();
            self.select(name);
        }

        let right = Rect::from_min_max(pos2(list_r.right() + m::SPACING, full.top()), full.max);
        // Live or its history.
        let Some(name) = self.selected.clone() else { return };
        let tabs = Rect::from_min_size(right.min, vec2(160.0, m::CONTROL_HEIGHT));
        let (live_r, hist_r) = (Rect::from_min_size(tabs.min, vec2(80.0, tabs.height())), Rect::from_min_size(pos2(tabs.left() + 80.0, tabs.top()), vec2(80.0, tabs.height())));
        let running = self.screen().is_some();
        if ui.add_enabled_ui(running, |ui| ui.put(live_r, Button::new("Live").pushed(!self.history).min_width(80.0))).inner.clicked() {
            self.history = false;
        }
        if !running {
            self.history = true;
        }
        if ui.put(hist_r, Button::new("History").pushed(self.history).min_width(80.0)).clicked() && !self.history {
            self.history = true;
            self.stop_controlling();
        }
        let right = Rect::from_min_max(pos2(right.left(), tabs.bottom() + m::SPACING), right.max);
        if self.history {
            if self.replay.as_ref().is_none_or(|r| r.name != name) {
                self.replay = Some(crate::replay::Replay::new(&name, cx.proxy()));
            }
            let mut ask = None;
            if let Some(r) = self.replay.as_mut() {
                r.ui(ui, right);
                if std::mem::take(&mut r.want_export) {
                    ask = Some(r.name.clone());
                }
            }
            if let Some(name) = ask {
                let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
                let dir = [home.join("Videos"), home.clone()].into_iter().find(|d| d.is_dir()).unwrap_or(home);
                let panel = olo_kit::file_panel::FilePanel::save(dir, format!("{name}.mp4"));
                let (w, h) = olo_kit::file_panel::PANEL_SIZE;
                let id = cx.open_window(WindowOptions::new("Export Video", w, h).min_size(420, 300));
                self.windows.insert(id, Win::ExportTo(Box::new(panel)));
            }
            return;
        }
        let status_r = Rect::from_min_max(pos2(right.left(), right.bottom() - m::CONTROL_HEIGHT), right.max);
        let area = Rect::from_min_max(right.min, pos2(right.right(), status_r.top() - m::SPACING));
        let Some(screen) = self.screen().cloned() else { return };
        if let Some((name, image)) = self.frame.take() {
            match &mut self.texture {
                Some((n, tex)) if *n == name => tex.set(image, TextureOptions::LINEAR),
                _ => self.texture = Some((name.clone(), ui.ctx().load_texture(format!("screen-{name}"), image, TextureOptions::LINEAR))),
            }
        }

        // The picture, fitted, in a sunken frame.
        let aspect = screen.width as f32 / screen.height as f32;
        let (w, h) = if area.width() / area.height() > aspect { (area.height() * aspect, area.height()) } else { (area.width(), area.width() / aspect) };
        let pic = Rect::from_center_size(area.center(), vec2(w, h)).shrink(1.0);
        let painter = ui.painter();
        painter.rect_filled(pic.expand(1.0), 0.0, c(p.control_border));
        match &self.texture {
            Some((name, tex)) if *name == screen.name => {
                painter.image(tex.id(), pic, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            _ => {
                painter.rect_filled(pic, 0.0, c(p.document_background));
                let text = self.failure.as_deref().unwrap_or("Connecting…");
                style::label(painter, pic.center().x, pic.center().y, Align::Center, text, style::plain_font(), c(p.panel_text));
            }
        }
        if self.controlling {
            // A ring in the selection colour says the keys go there.
            painter.rect_stroke(pic.expand(2.0), 0.0, egui::Stroke::new(2.0, c(p.keyboard_navigation)), egui::StrokeKind::Middle);
        }
        let response = ui.allocate_rect(pic, egui::Sense::click_and_drag());
        if self.controlling {
            self.forward(ui, &screen, pic, &response);
        }

        // The status line: what is happening, and the control button.
        let windows = format!("{}  ·  {} × {}", screen.name, screen.width, screen.height);
        let what = if self.controlling { format!("{windows}  ·  You have control") } else { format!("{windows}  ·  Watching") };
        style::label(ui.painter(), pic.left() - 1.0, status_r.center().y, Align::Min, what, style::plain_font(), c(p.panel_text));
        let label = if self.controlling { "Stop Controlling" } else { "Take Control" };
        let b = Rect::from_min_size(pos2(pic.right() + 1.0 - 130.0, status_r.top()), vec2(130.0, m::CONTROL_HEIGHT));
        if ui.put(b, Button::new(label).min_width(130.0)).clicked() {
            if self.controlling {
                self.stop_controlling();
            } else {
                self.controlling = true;
                response.request_focus();
            }
        }
    }

    /// Press or let go of Ctrl, Shift and Alt on the screen to match
    /// `modifiers`.
    fn sync_modifiers(&mut self, modifiers: egui::Modifiers, out: &mut Vec<SyntheticEvent>) {
        let now = [modifiers.ctrl, modifiers.shift, modifiers.alt];
        for (i, key) in [29u32, 42, 56].into_iter().enumerate() {
            if now[i] != self.held[i] {
                out.push(SyntheticEvent::Key { key, pressed: now[i] });
                self.held[i] = now[i];
            }
        }
    }

    /// The person's pointer and keys, onto the screen.
    fn forward(&mut self, ui: &mut Ui, screen: &Screen, pic: Rect, response: &egui::Response) {
        let to_screen = |pos: Pos2| -> (f64, f64) {
            let x = ((pos.x - pic.left()) / pic.width()).clamp(0.0, 1.0) as f64 * screen.width as f64;
            let y = ((pos.y - pic.top()) / pic.height()).clamp(0.0, 1.0) as f64 * screen.height as f64;
            (x, y)
        };
        if response.hovered() || response.dragged() {
            response.request_focus();
        }
        let keys = response.has_focus();
        let (events, modifiers) = ui.input(|i| (i.events.clone(), i.modifiers));
        let mut out = Vec::new();
        let mut moved = None;
        for e in &events {
            match e {
                egui::Event::PointerMoved(pos) if pic.contains(*pos) || response.dragged() => moved = Some(to_screen(*pos)),
                egui::Event::PointerButton { pos, button, pressed, .. } if pic.contains(*pos) || !pressed => {
                    let button = match button {
                        egui::PointerButton::Primary => screens::LEFT,
                        egui::PointerButton::Secondary => screens::RIGHT,
                        egui::PointerButton::Middle => screens::MIDDLE,
                        _ => continue,
                    };
                    let (x, y) = to_screen(*pos);
                    moved = None;
                    out.push(SyntheticEvent::Move { x, y });
                    out.push(SyntheticEvent::Button { button, pressed: *pressed });
                }
                egui::Event::MouseWheel { unit, delta, .. } if response.hovered() => {
                    if let Some((x, y)) = moved.take() {
                        out.push(SyntheticEvent::Move { x, y });
                    }
                    match unit {
                        egui::MouseWheelUnit::Point => out.push(SyntheticEvent::Scroll { dx: -delta.x as f64, dy: -delta.y as f64 }),
                        _ => out.push(SyntheticEvent::Wheel { dx: -delta.x.signum() as i32, dy: -delta.y.signum() as i32 }),
                    }
                }
                egui::Event::Key { key, physical_key, pressed, modifiers, .. } if keys => {
                    if let Some(code) = evdev(physical_key.unwrap_or(*key)) {
                        // The modifiers as they were for this key (a
                        // frame can hold Shift going down and up again).
                        self.sync_modifiers(*modifiers, &mut out);
                        out.push(SyntheticEvent::Key { key: code, pressed: *pressed });
                    }
                }
                _ => {}
            }
        }
        if let Some((x, y)) = moved {
            out.insert(0, SyntheticEvent::Move { x, y });
        }
        // And as they are now, once the keys are through.
        if keys {
            self.sync_modifiers(modifiers, &mut out);
        }
        if !out.is_empty() {
            let _ = self.input.send((screen.clone(), out));
        }
    }
}

impl App for Viewer {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut Ui) {
        match self.windows.get(&window) {
            Some(Win::Main) => self.main_ui(cx, ui),
            Some(Win::ConfirmStop(name, message)) => {
                let (name, message) = (name.clone(), message.clone());
                let Some(choice) = widgets::alert(ui, "warning", &message, &["Cancel", "Stop Screen"]) else { return };
                self.windows.remove(&window);
                cx.close_window(window);
                if choice == 1 {
                    if self.selected.as_ref() == Some(&name) {
                        self.stop_controlling();
                    }
                    let proxy = cx.proxy();
                    std::thread::spawn(move || proxy.send(Msg::Stopped(screens::stop(&name))));
                }
            }
            Some(Win::ExportTo(_)) => {
                let Some(Win::ExportTo(panel)) = self.windows.get_mut(&window) else { return };
                let Some(result) = panel.ui(ui) else { return };
                self.windows.remove(&window);
                cx.close_window(window);
                if let (olo_kit::file_panel::PanelResult::Chosen(path), Some(r)) = (result, self.replay.as_mut()) {
                    r.start_export(path, cx.proxy());
                }
            }
            Some(Win::Alert(message)) => {
                let message = message.clone();
                if widgets::alert(ui, "stop", &message, &["OK"]).is_some() {
                    self.windows.remove(&window);
                    cx.close_window(window);
                }
            }
            None => {}
        }
    }

    fn menu_bar(&mut self, window: WindowId) -> Option<Vec<Menu>> {
        if !matches!(self.windows.get(&window), Some(Win::Main)) {
            return None;
        }
        let has = self.screen().is_some();
        Some(vec![
            Menu::new("Screen")
                .item("New Screen", "new", Some(Key::N))
                .enabled(!self.busy)
                .item("Stop Screen…", "stop", None)
                .enabled(has)
                .separator()
                .item("Quit", "quit", Some(Key::Q)),
            Menu::new("Control")
                .item(if self.controlling { "Stop Controlling" } else { "Take Control" }, "control", Some(Key::K))
                .enabled(has),
        ])
    }

    fn command(&mut self, cx: &mut Cx, _window: WindowId, command: &str) {
        match command {
            "new" => self.new_screen(cx),
            "stop" => {
                if let Some(name) = self.selected.clone() {
                    let message = format!("Stop the screen “{name}”? The agent's programs on it are closed, and anything it hasn't saved is lost.");
                    let id = cx.open_window(WindowOptions::alert("Stop Screen", &message).parent_opt(self.main));
                    self.windows.insert(id, Win::ConfirmStop(name, message));
                }
            }
            "control" => {
                if self.controlling {
                    self.stop_controlling();
                } else if self.screen().is_some() {
                    self.controlling = true;
                }
            }
            "quit" => cx.quit(),
            _ => {}
        }
        self.redraw(cx);
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        if Some(window) == self.main {
            self.stop_controlling();
            cx.quit();
        } else {
            self.windows.remove(&window);
            cx.close_window(window);
        }
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        // Another start: `olo-screen [watch NAME]`.
        if let Some(launch) = message.downcast_ref::<olo_kit::instance::Launch>() {
            self.show(cx, launch.args.get(1).cloned());
            return;
        }
        let message = match message.downcast::<crate::replay::ReplayMsg>() {
            Ok(m) => {
                if let Some(r) = self.replay.as_mut() {
                    r.message(*m);
                }
                if let Some(w) = self.main {
                    cx.redraw(w);
                }
                return;
            }
            Err(m) => m,
        };
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Screens(list) => {
                self.screens = list;
                let (proxy, running) = (cx.proxy(), self.screens.clone());
                std::thread::spawn(move || proxy.send(Msg::Rows(rows(&running))));
            }
            Msg::Rows(rows) => {
                self.rows = rows;
                if self.selected.as_ref().is_none_or(|n| !self.rows.iter().any(|r| &r.name == n)) {
                    let first = self.rows.first().map(|r| r.name.clone());
                    self.stop_controlling();
                    self.selected = None;
                    self.select(first);
                }
                // A stopped one has only its history.
                if self.screen().is_none() && self.selected.is_some() {
                    self.history = true;
                }
            }
            Msg::Frame { name, image } => {
                if self.selected.as_ref() == Some(&name) {
                    self.failure = None;
                    self.frame = Some((name, image));
                }
            }
            Msg::Failed { name, message } => {
                if self.selected.as_ref() == Some(&name) {
                    self.failure = Some(message);
                }
            }
            Msg::Started(r) => {
                self.busy = false;
                match r {
                    Ok(screen) => {
                        if !self.screens.contains(&screen) {
                            self.screens.push(screen.clone());
                        }
                        self.select(Some(screen.name));
                    }
                    Err(e) => self.alert(cx, &format!("The screen couldn't be started: {e}")),
                }
            }
            Msg::Stopped(r) => {
                if let Err(e) = r {
                    self.alert(cx, &e);
                }
                self.screens = listed();
                if self.screen().is_none() {
                    let first = self.screens.first().map(|s| s.name.clone());
                    self.selected = None;
                    self.select(first);
                }
            }
        }
        self.redraw(cx);
    }

    fn save_state(&mut self, _cx: &Cx) -> Option<String> {
        serde_json::to_string(&Saved { selected: self.selected.clone() }).ok()
    }
}
