//! Agent Screens, the window: the agents' screens in a sidebar (running,
//! then stopped ones with a history kept), and the one chosen under a
//! header saying what it is and how it's doing, in three tabs: Live (the
//! screen as it is, about eight frames a second with the agent's pointer,
//! and Take Control to step in: the person's pointer and keys over the
//! picture then go to that screen, never to their own, until they choose
//! Stop Controlling), History (`replay`: its actions as they come, to
//! search, filter and play back) and Stats (`stats`).
//!
//! Frames come from a worker (the screen's app_server writes raw pixels,
//! read back here), input goes out through another and the history is
//! followed by a third, so the window never waits on a screen or a disk.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, Sender, channel},
    },
    time::{Duration, Instant},
};

use olo_kit::egui::{self, Align, ColorImage, FontId, Key, Pos2, Rect, TextureHandle, TextureOptions, Ui, pos2, vec2};
use serde::{Deserialize, Serialize};
use olo_kit::{
    App, Cx, Menu, Proxy, WindowId, WindowOptions, icons,
    metrics as m,
    sidebar::{self, Sidebar},
    style::{self, c},
    widgets::{self, Button, EmptyState, ScrollView, TabView},
};
use olo_protocol::{ImageFormat, SyntheticEvent};

use crate::{
    replay::{self, Replay},
    screens::{self, Screen},
    stats,
};

/// Frames a second, at most.
const FPS: f64 = 8.0;
/// The sidebar's width, less in a small window.
const SIDEBAR_W: f32 = 196.0;
const SIDEBAR_MIN_W: f32 = 160.0;
const HEADER_H: f32 = 40.0;

enum Msg {
    Screens(Vec<Screen>),
    /// Every screen to list: running, or stopped with a history kept;
    /// each with when it was last used, most recent first.
    Rows(Vec<Row>),
    Frame { name: String, image: ColorImage },
    Failed { name: String, message: String },
    Started(Result<Screen, String>),
    Stopped(Result<(), String>),
    Cleared(Result<(), String>),
}

/// The chosen screen's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
enum Tab {
    #[default]
    Live,
    History,
    Stats,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Live => "Live",
            Tab::History => "History",
            Tab::Stats => "Stats",
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    selected: Option<String>,
    #[serde(default)]
    tab: Tab,
}

enum Win {
    Main,
    /// What to do once asked: stop the screen, or clear its history; the
    /// screen's name, and the question.
    ConfirmStop(String, String),
    ConfirmClear(String, String),
    Alert(String),
    /// Where to save a screen's history as a video.
    ExportTo(Box<olo_kit::file_panel::FilePanel>),
    /// A video saved there, to show.
    Exported(PathBuf, String),
}

pub struct Viewer {
    windows: HashMap<WindowId, Win>,
    main: Option<WindowId>,
    screens: Vec<Screen>,
    selected: Option<String>,
    tab: Tab,
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
    /// The chosen screen's history, followed as it grows.
    replay: Option<Replay>,
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

/// How long ago, as a sidebar says it: "now", "5 min", "3 h", "2 d".
fn ago_short(ms: u64, now: u64) -> String {
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{} min", s / 60),
        3600..86_400 => format!("{} h", s / 3600),
        _ => format!("{} d", s / 86_400),
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
            selected: saved.selected,
            tab: saved.tab,
            watching,
            texture: None,
            frame: None,
            failure: None,
            controlling: false,
            held: [false; 3],
            input: send_input(),
            busy: false,
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
        let id = cx.open_window(WindowOptions::new("Agent Screens", 1000, 640).min_size(660, 420));
        self.windows.insert(id, Win::Main);
        self.main = Some(id);
    }

    /// Choose `name` (from the command line, or a forwarded start).
    pub fn show(&mut self, cx: &mut Cx, name: Option<String>) {
        if let Some(name) = name {
            self.select(Some(name));
            self.set_tab(Tab::Live);
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

    /// Point the frame worker at the chosen screen while its Live tab
    /// shows (it pulls nothing otherwise).
    fn watch(&self) {
        let live = (self.shown_tab() == Tab::Live).then(|| self.selected.clone()).flatten();
        let mut watching = self.watching.lock().unwrap_or_else(|e| e.into_inner());
        if *watching != live {
            *watching = live;
        }
    }

    fn redraw(&self, cx: &mut Cx) {
        if let Some(id) = self.main {
            cx.redraw(id);
        }
    }

    /// The tab showing: Live only for a running screen.
    fn shown_tab(&self) -> Tab {
        if self.tab == Tab::Live && self.screen().is_none() { Tab::History } else { self.tab }
    }

    fn set_tab(&mut self, tab: Tab) {
        if tab != Tab::Live {
            self.stop_controlling();
        }
        self.tab = tab;
        self.watch();
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

    fn ask_stop(&mut self, cx: &mut Cx) {
        if let Some(name) = self.screen().map(|s| s.name.clone()) {
            let message = format!("Stop the screen “{name}”? The agent’s programs on it are closed, and anything it hasn’t saved is lost. Its history is kept.");
            let id = cx.open_window(WindowOptions::alert("Stop Screen", &message).parent_opt(self.main));
            self.windows.insert(id, Win::ConfirmStop(name, message));
        }
    }

    fn ask_clear(&mut self, cx: &mut Cx) {
        if let Some(name) = self.selected.clone().filter(|_| self.screen().is_none()) {
            let message = format!("Clear the history of “{name}”? Its actions and the frames kept with them are deleted. This can’t be undone.");
            let id = cx.open_window(WindowOptions::alert("Clear History", &message).parent_opt(self.main));
            self.windows.insert(id, Win::ConfirmClear(name, message));
        }
    }

    fn ask_export(&mut self, cx: &mut Cx) {
        let Some(name) = self.replay.as_ref().filter(|r| r.loaded() && !r.exporting() && !r.actions().is_empty()).map(|r| r.name.clone()) else { return };
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let dir = [home.join("Videos"), home.clone()].into_iter().find(|d| d.is_dir()).unwrap_or(home);
        let panel = olo_kit::file_panel::FilePanel::save(dir, format!("{name}.mp4"));
        let (w, h) = olo_kit::file_panel::PANEL_SIZE;
        let id = cx.open_window(WindowOptions::new("Export Video", w, h).min_size(420, 300).parent_opt(self.main));
        self.windows.insert(id, Win::ExportTo(Box::new(panel)));
    }

    fn main_ui(&mut self, cx: &mut Cx, ui: &mut Ui) {
        // The person's input, for a screen they control, before the tabs
        // take theirs (⌃Tab goes to the screen then).
        let events = if self.controlling { ui.input(|i| i.events.clone()) } else { Vec::new() };
        let full = ui.max_rect();
        if self.rows.is_empty() {
            let area = full.shrink(m::WINDOW_INSET);
            let text = "No agent screens yet. An agent starts one with “olo-screen start”, or start one yourself.";
            let r = Rect::from_min_max(area.min, pos2(area.right(), area.center().y + m::CONTROL_HEIGHT));
            ui.put(r, EmptyState::new(text).icon("agent-screens"));
            let b = Rect::from_min_size(pos2(area.center().x - 60.0, r.bottom() + m::SPACING), vec2(120.0, m::CONTROL_HEIGHT));
            let label = if self.busy { "Starting…" } else { "New Screen" };
            if ui.add_enabled_ui(!self.busy, |ui| ui.put(b, Button::new(label).min_width(120.0))).inner.clicked() {
                self.new_screen(cx);
            }
            return;
        }
        let side_w = (full.width() * 0.2).round().clamp(SIDEBAR_MIN_W, SIDEBAR_W);
        let side = Rect::from_min_size(full.min, vec2(side_w, full.height()));
        self.sidebar_ui(cx, ui, side);
        let content = Rect::from_min_max(pos2(side.right(), full.top()), full.max).shrink(m::WINDOW_INSET);
        let Some(name) = self.selected.clone() else { return };
        if self.replay.as_ref().is_none_or(|r| r.name != name) {
            self.replay = Some(Replay::new(&name, cx.proxy()));
        }
        let header = Rect::from_min_size(content.min, vec2(content.width(), HEADER_H));
        self.header_ui(cx, ui, header, &name);
        let below = Rect::from_min_max(pos2(content.left(), header.bottom() + m::SPACING), content.max);
        let running = self.screen().is_some();
        let tabs: &[Tab] = if running { &[Tab::Live, Tab::History, Tab::Stats] } else { &[Tab::History, Tab::Stats] };
        let labels: Vec<&str> = tabs.iter().map(|t| t.label()).collect();
        let shown = self.shown_tab();
        let mut i = tabs.iter().position(|t| *t == shown).unwrap_or(0);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(below));
        TabView::new(&mut i, &labels).show(&mut child, |ui, i| {
            let area = ui.max_rect();
            match tabs[i] {
                Tab::Live => self.live_ui(ui, area, &events),
                Tab::History => {
                    if let Some(r) = self.replay.as_mut() {
                        r.ui(ui, area, running);
                    }
                }
                Tab::Stats => {
                    if let Some(r) = self.replay.as_mut() {
                        stats::ui(ui, area, r, running);
                    }
                }
            }
        });
        if tabs[i] != shown {
            self.set_tab(tabs[i]);
        }
        if let Some(r) = self.replay.as_mut()
            && let Some(result) = r.take_exported()
        {
            match result {
                Ok(path) => {
                    let message = format!("The video “{}” is saved.", path.file_name().and_then(|n| n.to_str()).unwrap_or(""));
                    let id = cx.open_window(WindowOptions::alert("Export Video", &message).parent_opt(self.main));
                    self.windows.insert(id, Win::Exported(path, message));
                }
                Err(e) if e == "cancelled" => {}
                Err(e) => self.alert(cx, &format!("The video couldn’t be made: {e}")),
            }
        }
        // "2 s ago" moves on.
        if running {
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }
    }

    /// The screens: running, then stopped ones with a history; New Screen
    /// under them.
    fn sidebar_ui(&mut self, cx: &mut Cx, ui: &mut Ui, side: Rect) {
        let p = style::palette(ui.ctx());
        let now = crate::history::now_ms();
        let footer_h = m::CONTROL_HEIGHT + 2.0 * m::SPACING;
        let list = Rect::from_min_max(side.min, pos2(side.right(), side.bottom() - footer_h));
        let footer = Rect::from_min_max(pos2(side.left(), list.bottom()), side.max);
        // The chosen one's last use as it happens; the others' as listed.
        let latest = self.replay.as_ref().and_then(|r| r.actions().last().map(|a| (r.name.clone(), a.at + a.ms)));
        let last_used = |r: &Row| match &latest {
            Some((n, t)) if *n == r.name => Some(*t),
            _ => r.last_used,
        };
        let running = |n: &str| self.screens.iter().any(|s| s.name == n);
        let groups: [(&str, Vec<&Row>); 2] =
            [("Running", self.rows.iter().filter(|r| running(&r.name)).collect()), ("Stopped", self.rows.iter().filter(|r| !running(&r.name)).collect())];
        let values: Vec<Vec<String>> = groups.iter().map(|(_, rs)| rs.iter().map(|r| last_used(r).map(|t| ago_short(t, now)).unwrap_or_default()).collect()).collect();
        let mut items = Vec::new();
        let mut names: Vec<Option<&str>> = Vec::new();
        for ((heading, rs), values) in groups.iter().zip(&values) {
            if rs.is_empty() {
                continue;
            }
            items.push(sidebar::Row::heading(heading));
            names.push(None);
            for (r, v) in rs.iter().zip(values) {
                let row = sidebar::Row::new("agent-screens", &r.name);
                items.push(if v.is_empty() { row } else { row.value(v) });
                names.push(Some(&r.name));
            }
        }
        let first = names.iter().position(Option::is_some).unwrap_or(0);
        let mut chosen = names.iter().position(|n| *n == self.selected.as_deref()).unwrap_or(first);
        let before = chosen;
        let height = sidebar::height(&items);
        if height <= list.height() {
            Sidebar::new("screens", &items, &mut chosen).show(ui, list);
        } else {
            ui.painter().rect_filled(list, 0.0, style::shade(&p, p.panel_background, 1.04));
            ScrollView::new("screens-scroll").border(false).show_in(ui, list, |ui, _| {
                let r = Rect::from_min_size(ui.max_rect().min, vec2(ui.max_rect().width(), height));
                Sidebar::new("screens", &items, &mut chosen).show(ui, r);
                ui.allocate_rect(r, egui::Sense::hover());
            });
        }
        let picked = (chosen != before).then(|| names.get(chosen).copied().flatten().map(str::to_owned)).flatten();
        // New Screen, on the sidebar's ground.
        let painter = ui.painter();
        painter.rect_filled(footer, 0.0, style::shade(&p, p.panel_background, 1.04));
        style::vline_left_of(painter, footer.right(), footer.top(), footer.bottom(), style::shade(&p, p.panel_background, 1.25));
        style::hline(painter, footer.left(), footer.right() - 1.0, footer.top(), style::shade(&p, p.panel_background, 1.18));
        let b = Rect::from_min_size(pos2(footer.left() + m::WINDOW_INSET, footer.top() + m::SPACING), vec2(footer.width() - 2.0 * m::WINDOW_INSET, m::CONTROL_HEIGHT));
        let label = if self.busy { "Starting…" } else { "New Screen" };
        if ui.add_enabled_ui(!self.busy, |ui| ui.put(b, Button::new(label))).inner.clicked() {
            self.new_screen(cx);
        }
        if let Some(name) = picked {
            self.select(Some(name));
        }
    }

    /// The chosen screen's name, how it's doing, and what can be done
    /// with it.
    fn header_ui(&mut self, cx: &mut Cx, ui: &mut Ui, r: Rect, name: &str) {
        let p = style::palette(ui.ctx());
        let now = crate::history::now_ms();
        let screen = self.screen().cloned();
        let icon = style::snap(ui.ctx(), Rect::from_min_size(pos2(r.left(), r.center().y - 16.0), vec2(32.0, 32.0)));
        icons::paint(ui.painter(), icon, "agent-screens");
        // The buttons, from the right.
        let mut right = r.right();
        let mut button = |ui: &mut Ui, label: &str, enabled: bool| {
            let w = style::label_width(ui.ctx(), label, style::plain_font()).ceil() + 2.0 * m::CONTROL_PAD;
            let w = w.max(96.0);
            let b = Rect::from_min_size(pos2(right - w, r.center().y - m::CONTROL_HEIGHT / 2.0), vec2(w, m::CONTROL_HEIGHT));
            right = b.left() - m::SPACING;
            ui.add_enabled_ui(enabled, |ui| ui.put(b, Button::new(label).min_width(w))).inner.clicked()
        };
        let can_export = self.replay.as_ref().is_some_and(|r| r.loaded() && !r.exporting() && !r.actions().is_empty());
        if screen.is_some() {
            if button(ui, "Stop Screen…", true) {
                self.ask_stop(cx);
            }
        } else if button(ui, "Clear History…", true) {
            self.ask_clear(cx);
        }
        if button(ui, "Export Video…", can_export) {
            self.ask_export(cx);
        }
        // The name, and a line on how it is.
        let x = icon.right() + m::SPACING;
        let w = (right - m::SPACING - x).max(0.0);
        let title = FontId::new(15.0, style::bold());
        style::label_fit(ui.painter(), x, r.top() + 11.0, Align::Min, name, title, c(p.panel_text), w);
        let actions = self.replay.as_ref().map(|r| r.actions()).unwrap_or_default();
        let mut parts = Vec::new();
        let dot = if let Some(s) = &screen {
            parts.push("Running".to_owned());
            parts.push(format!("{} × {}", s.width, s.height));
            if let Some(t) = self.replay.as_ref().and_then(Replay::started) {
                parts.push(format!("up {}", stats::span(now.saturating_sub(t))));
            }
            c(p.success)
        } else {
            parts.push("Stopped".to_owned());
            style::disabled_text(&p)
        };
        if !actions.is_empty() {
            parts.push(format!("{} action{}", style::thousands(actions.len()), if actions.len() == 1 { "" } else { "s" }));
        }
        if let Some(t) = actions.last().map(|a| a.at + a.ms) {
            parts.push(format!("last {}", stats::ago(t, now)));
        }
        let line_y = r.top() + 29.0;
        let d = style::snap_centred(ui.ctx(), pos2(x + 4.0, line_y), vec2(8.0, 8.0));
        ui.painter().circle_filled(d.center(), 4.0, dot);
        style::label_fit(ui.painter(), x + 14.0, line_y, Align::Min, &parts.join("  ·  "), style::plain_font(), style::disabled_text(&p), (w - 14.0).max(0.0));
    }

    /// The screen as it is now, and Take Control.
    fn live_ui(&mut self, ui: &mut Ui, area: Rect, events: &[egui::Event]) {
        let p = style::palette(ui.ctx());
        let Some(screen) = self.screen().cloned() else { return };
        if let Some((name, image)) = self.frame.take() {
            match &mut self.texture {
                Some((n, tex)) if *n == name => tex.set(image, TextureOptions::LINEAR),
                _ => self.texture = Some((name.clone(), ui.ctx().load_texture(format!("screen-{name}"), image, TextureOptions::LINEAR))),
            }
        }
        let bar = Rect::from_min_max(pos2(area.left(), area.bottom() - m::CONTROL_HEIGHT), area.max);
        let well = Rect::from_min_max(area.min, pos2(area.right(), bar.top() - m::SPACING));
        let tex = self.texture.as_ref().filter(|(n, _)| *n == screen.name).map(|(_, t)| t);
        let text = match tex {
            Some(_) => None,
            None => Some(self.failure.as_deref().unwrap_or("Connecting…")),
        };
        let pic = replay::picture_well(ui, well, Some(vec2(screen.width as f32, screen.height as f32)), tex, text);
        let target = pic.unwrap_or(well);
        if self.controlling {
            // A ring in the selection colour says the keys go there.
            ui.painter().rect_stroke(target.expand(2.0), 0.0, egui::Stroke::new(2.0, c(p.keyboard_navigation)), egui::StrokeKind::Middle);
        }
        let response = ui.allocate_rect(target, egui::Sense::click_and_drag());
        if self.controlling {
            self.forward(ui, &screen, target, &response, events);
        }

        // What is going on, and the control button.
        let label = if self.controlling { "Stop Controlling" } else { "Take Control" };
        let b = Rect::from_min_size(pos2(bar.right() - 140.0, bar.top()), vec2(140.0, m::CONTROL_HEIGHT));
        if ui.put(b, Button::new(label).min_width(140.0).pushed(self.controlling)).clicked() {
            if self.controlling {
                self.stop_controlling();
            } else {
                self.controlling = true;
                response.request_focus();
            }
        }
        let now = crate::history::now_ms();
        let (text, color) = if self.controlling {
            ("You have control: your pointer and keys go to this screen.".to_owned(), c(p.panel_text))
        } else {
            let latest = self.replay.as_ref().and_then(|r| r.actions().last());
            match latest {
                Some(a) => {
                    let what = replay::command(a);
                    let mark = if a.error.is_some() { "  ✗" } else { "" };
                    (format!("Latest: {what}{mark}  ·  {}", stats::ago(a.at + a.ms, now)), style::disabled_text(&p))
                }
                None => ("Watching. Nothing has been done on it yet.".to_owned(), style::disabled_text(&p)),
            }
        };
        let w = (b.left() - m::SPACING - bar.left()).max(0.0);
        style::label_fit(ui.painter(), bar.left(), bar.center().y, Align::Min, &text, style::plain_font(), color, w);
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

    /// The person's pointer and keys (`events`, this frame's), onto the
    /// screen.
    fn forward(&mut self, ui: &mut Ui, screen: &Screen, pic: Rect, response: &egui::Response, events: &[egui::Event]) {
        let to_screen = |pos: Pos2| -> (f64, f64) {
            let x = ((pos.x - pic.left()) / pic.width()).clamp(0.0, 1.0) as f64 * screen.width as f64;
            let y = ((pos.y - pic.top()) / pic.height()).clamp(0.0, 1.0) as f64 * screen.height as f64;
            (x, y)
        };
        if response.hovered() || response.dragged() {
            response.request_focus();
        }
        let keys = response.has_focus();
        let modifiers = ui.input(|i| i.modifiers);
        let mut out = Vec::new();
        let mut moved = None;
        for e in events {
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
            Some(Win::ConfirmClear(name, message)) => {
                let (name, message) = (name.clone(), message.clone());
                let Some(choice) = widgets::alert(ui, "warning", &message, &["Cancel", "Clear History"]) else { return };
                self.windows.remove(&window);
                cx.close_window(window);
                if choice == 1 {
                    let proxy = cx.proxy();
                    std::thread::spawn(move || proxy.send(Msg::Cleared(crate::history::clear(&name))));
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
            Some(Win::Exported(path, message)) => {
                let (path, message) = (path.clone(), message.clone());
                let Some(choice) = widgets::alert(ui, "video", &message, &[olo_kit::tracker::LABEL, "OK"]) else { return };
                self.windows.remove(&window);
                cx.close_window(window);
                if choice == 0
                    && let Err(e) = olo_kit::tracker::open(&path)
                {
                    self.alert(cx, &format!("Tracker couldn’t show it: {e}"));
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
        let running = self.screen().is_some();
        let chosen = self.selected.is_some();
        let tab = self.shown_tab();
        let history = tab == Tab::History && self.replay.as_ref().is_some_and(|r| r.loaded() && !r.actions().is_empty());
        let can_export = self.replay.as_ref().is_some_and(|r| r.loaded() && !r.exporting() && !r.actions().is_empty());
        let playing = self.replay.as_ref().is_some_and(Replay::playing);
        Some(vec![
            Menu::new("Screen")
                .item("New Screen", "new", Some(Key::N))
                .enabled(!self.busy)
                .item("Stop Screen…", "stop", None)
                .enabled(running)
                .item("Clear History…", "clear", None)
                .enabled(chosen && !running)
                .separator()
                .item("Export Video…", "export", Some(Key::E))
                .enabled(can_export)
                .separator()
                .item("Quit", "quit", Some(Key::Q)),
            Menu::new("View")
                .item("Live", "tab-live", Some(Key::Num1))
                .enabled(running)
                .checked(tab == Tab::Live)
                .item("History", "tab-history", Some(Key::Num2))
                .enabled(chosen)
                .checked(tab == Tab::History)
                .item("Stats", "tab-stats", Some(Key::Num3))
                .enabled(chosen)
                .checked(tab == Tab::Stats)
                .separator()
                .item("Find", "find", Some(Key::F))
                .enabled(chosen),
            Menu::new("Playback")
                .item(if playing { "Pause" } else { "Play" }, "play", None)
                .enabled(history)
                .item("Previous Action", "previous", None)
                .enabled(history)
                .item("Next Action", "next", None)
                .enabled(history)
                .separator()
                .item(if running { "Go Live" } else { "Go to Latest" }, "latest", None)
                .enabled(history),
            Menu::new("Control")
                .item(if self.controlling { "Stop Controlling" } else { "Take Control" }, "control", Some(Key::K))
                .enabled(running),
        ])
    }

    fn command(&mut self, cx: &mut Cx, _window: WindowId, command: &str) {
        match command {
            "new" => self.new_screen(cx),
            "stop" => self.ask_stop(cx),
            "clear" => self.ask_clear(cx),
            "export" => self.ask_export(cx),
            "tab-live" if self.screen().is_some() => self.set_tab(Tab::Live),
            "tab-history" => self.set_tab(Tab::History),
            "tab-stats" => self.set_tab(Tab::Stats),
            "find" => {
                self.set_tab(Tab::History);
                if let Some(r) = self.replay.as_mut() {
                    r.focus_search();
                }
            }
            "play" | "previous" | "next" | "latest" => {
                if let Some(r) = self.replay.as_mut() {
                    match command {
                        "play" => r.play_pause(),
                        "previous" => r.step(-1),
                        "next" => r.step(1),
                        _ => r.latest(),
                    }
                }
            }
            "control" => {
                if self.controlling {
                    self.stop_controlling();
                } else if self.screen().is_some() {
                    self.set_tab(Tab::Live);
                    self.controlling = true;
                }
            }
            "quit" => cx.quit(),
            _ => {}
        }
        self.watch();
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
        let message = match message.downcast::<replay::ReplayMsg>() {
            Ok(m) => {
                if let Some(r) = self.replay.as_mut() {
                    r.message(*m);
                }
                self.redraw(cx);
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
                        self.set_tab(Tab::Live);
                    }
                    Err(e) => self.alert(cx, &format!("The screen couldn't be started: {e}")),
                }
            }
            Msg::Stopped(r) => {
                if let Err(e) = r {
                    self.alert(cx, &e);
                }
                self.screens = listed();
                let (proxy, running) = (cx.proxy(), self.screens.clone());
                std::thread::spawn(move || proxy.send(Msg::Rows(rows(&running))));
            }
            Msg::Cleared(r) => {
                if let Err(e) = r {
                    self.alert(cx, &e);
                }
                self.replay = None;
                let (proxy, running) = (cx.proxy(), self.screens.clone());
                std::thread::spawn(move || proxy.send(Msg::Rows(rows(&running))));
            }
        }
        // A screen come or gone changes what Live shows.
        self.watch();
        self.redraw(cx);
    }

    fn save_state(&mut self, _cx: &Cx) -> Option<String> {
        serde_json::to_string(&Saved { selected: self.selected.clone(), tab: self.tab }).ok()
    }
}
