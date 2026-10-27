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
}

/// The screens to show: all but the one this viewer itself is on, if it
/// runs on one (it would show itself, endlessly).
fn listed() -> Vec<Screen> {
    let own = std::env::var("OLO_SCREEN").ok();
    screens::list().into_iter().filter(|s| Some(&s.name) != own.as_ref()).collect()
}

/// Watch the list of screens (every second).
fn watch_list(proxy: Proxy) {
    std::thread::spawn(move || {
        let mut last = Vec::new();
        loop {
            let now = listed();
            if now != last {
                proxy.send(Msg::Screens(now.clone()));
                last = now;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

/// Fetch frames of whichever screen is being watched.
fn watch_frames(proxy: Proxy, watching: Arc<Mutex<Option<String>>>) {
    std::thread::spawn(move || {
        let path = screens::dir().join(format!("view-{}.rgba", std::process::id()));
        let _ = std::fs::create_dir_all(screens::dir());
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
        if self.screens.is_empty() {
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
        let names: Vec<String> = self.screens.iter().map(|s| s.name.clone()).collect();
        let sizes: Vec<String> = self.screens.iter().map(|s| format!("{} × {}", s.width, s.height)).collect();
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
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Screens(list) => {
                self.screens = list;
                if self.screen().is_none() {
                    let first = self.screens.first().map(|s| s.name.clone());
                    self.stop_controlling();
                    self.selected = None;
                    self.select(first);
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
