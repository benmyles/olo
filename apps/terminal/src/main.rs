//! Terminal: a shell in a window, BeOS style (one process, any number of
//! windows), on alacritty's terminal emulator. GPU-drawn through the kit,
//! with Tabula's own light and dark colour schemes.
//!
//! The shells run in tabula-termd (termd.rs), so they outlive Terminal:
//! quit it, or have it crash or upgrade, and the next Terminal opens a
//! window on each shell as it was, scrollback and all.

mod client;
mod colors;
mod daemon;
mod find;
mod graphics;
mod io;
mod links;
mod session;
mod view;
#[allow(dead_code)]
mod wire;

use std::{collections::HashMap, path::PathBuf};

use alacritty_terminal::grid::Dimensions;
use tabula_kit::{
    App, Cx, Menu, WindowId, WindowOptions,
    egui::{self, Key},
    instance::{self, Launch},
    menu::{self, MenuAnchor, MenuRequest},
    metrics as m,
    widgets::{self, Button, CheckBox, TextControl},
};

use colors::Scheme;
use session::{Message, Session, Size, Start, TermEvent};
use view::{GlyphCache, Metrics, Mouse};

const APP_ID: &str = "tabula.terminal";
/// BeOS Terminal opened at 80×25; wider is more useful on today's screens.
const DEFAULT_SIZE: (usize, usize) = (120, 25);
pub const TEXT_SIZES: [f32; 9] = [9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 16.0, 18.0, 22.0];
const DEFAULT_TEXT_SIZE: f32 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
enum SchemeChoice {
    /// Follow the desktop's light/dark appearance.
    #[default]
    Auto,
    Light,
    Dark,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Settings {
    text_size: f32,
    scheme: SchemeChoice,
    cols: usize,
    rows: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { text_size: DEFAULT_TEXT_SIZE, scheme: SchemeChoice::Auto, cols: DEFAULT_SIZE.0, rows: DEFAULT_SIZE.1 }
    }
}

fn settings_path() -> PathBuf {
    tabula_kit::config_dir().join("terminal.json")
}

impl Settings {
    fn load() -> Settings {
        std::fs::read_to_string(settings_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save(&self) {
        let path = settings_path();
        let text = serde_json::to_string_pretty(self).unwrap_or_default();
        // Off the UI thread: the disk can be slow.
        std::thread::spawn(move || {
            if let Some(d) = path.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let tmp = path.with_extension("json.new");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        });
    }
}

struct TermWin {
    session: Session,
    mouse: Mouse,
    glyphs: GlyphCache,
    /// Size to snap the window to once real font metrics are known.
    snap_to: Option<(usize, usize)>,
    shown_title: String,
    /// A visual bell: when it rang.
    bell: Option<std::time::Instant>,
    /// The program's exit status, once it has exited (window closes).
    focused: bool,
}

enum Win {
    Term(Box<TermWin>),
    /// "Close this window?" for a window still running something.
    ConfirmClose { target: WindowId, program: String },
    /// The Find window for a terminal window.
    Find { target: WindowId },
    /// Terminal ▸ Terminal Daemon…
    Daemon(Box<daemon::DaemonWin>),
    /// "Start tabula-termd over?" for the Terminal Daemon window.
    ConfirmStartOver { target: WindowId },
}

struct Terminal {
    link: client::Link,
    windows: HashMap<WindowId, Win>,
    settings: Settings,
    metrics: Option<Metrics>,
    dark: bool,
    search: find::Search,
    /// tabula-termd was started over: when its shells' windows have all
    /// closed, open a new one rather than quitting.
    respawn: bool,
}

/// Estimated content size for `cols`×`rows` before fonts are loaded
/// (corrected after the first frame).
fn estimate(settings: &Settings, cols: usize, rows: usize) -> (u32, u32) {
    let s = settings.text_size;
    let cell_w = (s * 0.6 * 2.0).ceil() / 2.0;
    let cell_h = (s * 1.32 * 2.0).ceil() / 2.0;
    (
        (cols as f32 * cell_w + 2.0 * view::PAD).ceil() as u32,
        (rows as f32 * cell_h + 2.0 * view::PAD).ceil() as u32 + menu::BAR_HEIGHT as u32,
    )
}

/// What an upgraded Terminal gets from its previous self: which shell
/// was in each window (tabula-termd has the rest).
#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
    windows: Vec<SavedWindow>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedWindow {
    shell: u64,
    size: (u32, u32),
}

/// What a Terminal from before tabula-termd handed on: the shells
/// themselves, which go to tabula-termd now.
#[derive(serde::Deserialize)]
struct Legacy {
    windows: Vec<LegacyWindow>,
}

#[derive(serde::Deserialize)]
struct LegacyWindow {
    fd: i32,
    pid: i32,
    size: (u32, u32),
    cols: usize,
    rows: usize,
    title: Option<String>,
    contents: String,
}

impl Terminal {
    fn scheme(&self) -> Scheme {
        match self.settings.scheme {
            SchemeChoice::Auto => Scheme::for_mode(self.dark),
            SchemeChoice::Light => Scheme::light(),
            SchemeChoice::Dark => Scheme::dark(),
        }
    }

    fn open(&mut self, cx: &mut Cx, cwd: Option<PathBuf>, command: Option<Vec<String>>) {
        let (cols, rows) = (self.settings.cols, self.settings.rows);
        let options = self.window_options(cols, rows);
        self.add(cx, options, Size { cols, rows }, Some((cols, rows)), Start::Spawn { command, cwd });
    }

    /// A window on a shell tabula-termd already has (sized as it was, if
    /// we know).
    fn reopen(&mut self, cx: &mut Cx, shell: &wire::ShellInfo, size: Option<(u32, u32)>) {
        let (cols, rows) = (shell.grid.cols.max(1) as usize, shell.grid.rows.max(1) as usize);
        let mut options = self.window_options(cols, rows);
        if let Some(size) = size {
            options.size = size;
        }
        if let Some(t) = &shell.title {
            options.title = t.clone();
        }
        let snap = size.is_none().then_some((cols, rows));
        self.add(cx, options, Size { cols, rows }, snap, Start::Attach(shell.id));
    }

    fn add(&mut self, cx: &mut Cx, options: WindowOptions, size: Size, snap_to: Option<(usize, usize)>, start: Start) {
        let id = cx.open_window(options);
        let cell = self.metrics.map(|m| m.device_cell()).unwrap_or((15, 36));
        let session = Session::start(id, cx.proxy(), &self.link, size, cell, self.scheme(), start);
        self.windows.insert(
            id,
            Win::Term(Box::new(TermWin {
                session,
                mouse: Mouse::default(),
                glyphs: GlyphCache::default(),
                snap_to,
                shown_title: String::new(),
                bell: None,
                focused: true,
            })),
        );
    }

    fn window_options(&self, cols: usize, rows: usize) -> WindowOptions {
        let (w, h) = match self.metrics {
            Some(m) => {
                let (w, h) = m.content_size(cols, rows);
                (w, h + menu::BAR_HEIGHT as u32)
            }
            None => estimate(&self.settings, cols, rows),
        };
        let mut o = WindowOptions::new("Terminal", w, h).min_size(200, 120).raw_keys();
        o.panel = false;
        o
    }

    fn handle_launch(&mut self, cx: &mut Cx, launch: Launch) {
        let mut cwd = launch.cwd;
        let mut command = None;
        let mut args = launch.args.into_iter();
        while let Some(a) = args.next() {
            match a.as_str() {
                "-w" | "--working-directory" => cwd = args.next().map(PathBuf::from),
                "-e" | "--" => {
                    command = Some(args.by_ref().collect::<Vec<_>>());
                }
                _ => {}
            }
        }
        self.open(cx, cwd, command.filter(|c: &Vec<String>| !c.is_empty()));
    }

    /// Open windows on the shells left in tabula-termd: those this
    /// Terminal had before it upgraded (`saved`), and any no other Terminal
    /// is showing. True if there were any.
    fn reattach(&mut self, cx: &mut Cx, shells: Vec<wire::ShellInfo>, saved: Option<&Saved>) -> bool {
        let ours = std::process::id() as i32;
        // SAFETY: signal 0 only checks the process exists.
        let free = |s: &wire::ShellInfo| s.client.is_none_or(|p| p == ours || unsafe { libc::kill(p, 0) } != 0);
        let mut any = false;
        for shell in shells.iter().filter(|s| free(s)) {
            let size = saved.and_then(|s| s.windows.iter().find(|w| w.shell == shell.id)).map(|w| w.size);
            self.reopen(cx, shell, size);
            any = true;
        }
        any
    }

    /// Hand the shells a Terminal from before tabula-termd left us over
    /// to it, each in its window as before.
    fn adopt(&mut self, cx: &mut Cx, legacy: Legacy) -> bool {
        let mut any = false;
        for w in legacy.windows {
            // SAFETY: our previous self passed this descriptor to us alone.
            if unsafe { libc::fcntl(w.fd, libc::F_GETFD) } < 0 {
                continue;
            }
            // SAFETY: as above; it's ours now.
            let pty = unsafe { std::os::fd::FromRawFd::from_raw_fd(w.fd) };
            let mut options = self.window_options(w.cols, w.rows);
            options.size = w.size;
            let size = Size { cols: w.cols.max(1), rows: w.rows.max(1) };
            self.add(cx, options, size, None, Start::Adopt { pty, pid: w.pid, title: w.title, replay: w.contents });
            any = true;
        }
        any
    }

    fn set_text_size(&mut self, cx: &mut Cx, size: f32) {
        self.settings.text_size = size;
        self.settings.save();
        self.metrics = None;
        // Keep each window's grid size: resize the windows to fit.
        for (id, w) in &mut self.windows {
            if let Win::Term(t) = w {
                t.snap_to = Some((t.session.size.cols, t.session.size.rows));
                cx.redraw(*id);
            }
        }
    }

    fn set_grid(&mut self, cx: &mut Cx, win: WindowId, cols: usize, rows: usize) {
        if let Some(Win::Term(t)) = self.windows.get_mut(&win) {
            t.snap_to = Some((cols, rows));
            cx.redraw(win);
        }
        self.settings.cols = cols;
        self.settings.rows = rows;
        self.settings.save();
    }

    fn term_ui(&mut self, cx: &mut Cx, win: WindowId, ui: &mut egui::Ui) {
        let ppp = ui.ctx().pixels_per_point();
        let size = self.settings.text_size;
        let metrics = match self.metrics {
            Some(m) if m.size == size && (m.cell.x * ppp).fract() == 0.0 => m,
            _ => {
                let m = Metrics::new(ui.ctx(), size);
                self.metrics = Some(m);
                m
            }
        };
        let scheme = self.scheme();
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return };
        let focused = ui.input(|i| i.focused);
        if focused != t.focused {
            t.focused = focused;
            let mut term = t.session.term.lock();
            term.is_focused = focused;
            if term.mode().contains(alacritty_terminal::term::TermMode::FOCUS_IN_OUT) {
                drop(term);
                t.session.write(if focused { b"\x1b[I".to_vec() } else { b"\x1b[O".to_vec() });
            }
        }
        t.session.woke();
        {
            let mut current = t.session.shared.scheme.lock().unwrap();
            if *current != scheme {
                *current = scheme.clone();
            }
        }
        if let Some((cols, rows)) = t.snap_to.take() {
            let (w, h) = metrics.content_size(cols, rows);
            let want = (w, h + menu::BAR_HEIGHT as u32);
            if cx.window_size(win) != Some(want) {
                cx.resize(win, want);
            }
        }
        let session_cwd = {
            let shared = t.session.shared.clone();
            move || {
                let pid = shared.foreground().or(shared.shell())?;
                std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
            }
        };
        let out = view::show(ui, &mut t.session, &scheme, &metrics, &mut t.glyphs, &mut t.mouse, focused, &session_cwd);
        if let Some(target) = &out.open {
            links::open(target);
        }
        t.session.resize(out.size, metrics.device_cell());
        if let Some(text) = out.copy {
            cx.copy(text);
        }
        if out.paste {
            cx.paste(win);
        }
        // Visual bell: a brief flash of the frame.
        if let Some(at) = t.bell {
            let age = at.elapsed().as_secs_f32();
            if age < 0.15 {
                let r = ui.max_rect();
                let a = ((1.0 - age / 0.15) * 60.0) as u8;
                ui.painter().rect_filled(r, 0.0, egui::Color32::from_rgba_unmultiplied(128, 128, 128, a));
                ui.ctx().request_repaint();
            } else {
                t.bell = None;
            }
        }
        // The title: the program's own, else what's running.
        let title = t
            .session
            .title
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| t.session.foreground())
            .unwrap_or_else(|| "Terminal".into());
        if title != t.shown_title {
            t.shown_title = title.clone();
            cx.set_title(win, title);
        }
        if let Some(pos) = out.context_menu {
            let has_selection = t.session.term.lock().selection.as_ref().is_some_and(|s| !s.is_empty());
            let menu = Menu::new("")
                .item("Copy", "copy", None)
                .enabled(has_selection)
                .item("Paste", "paste", None)
                .separator()
                .item("Select All", "select-all", None)
                .item("Clear Scrollback", "clear", None);
            menu::request_popup(
                ui.ctx(),
                MenuRequest { menu, source: ui.make_persistent_id("context"), anchor: MenuAnchor::At(pos), selected: None },
            );
        }
        // Foreground program changes don't produce output: look again now
        // and then for the title.
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }

    fn close(&mut self, cx: &mut Cx, win: WindowId) {
        if let Some(Win::Term(t)) = self.windows.remove(&win) {
            t.session.hang_up();
        }
        cx.close_window(win);
        let finds: Vec<WindowId> =
            self.windows.iter().filter(|(_, w)| matches!(w, Win::Find { target } if *target == win)).map(|(&id, _)| id).collect();
        for id in finds {
            self.windows.remove(&id);
            cx.close_window(id);
        }
        if !self.windows.values().any(|w| matches!(w, Win::Term(_))) {
            if std::mem::take(&mut self.respawn) {
                self.open(cx, None, None);
                return;
            }
            self.link.bye();
            cx.quit();
        }
    }
}

const START_OVER: &str = "Starting tabula-termd over ends every shell in it, and whatever is running in them. Start over?";

const FIND_W: f32 = 340.0;
const FIND_H: f32 = 2.0 * m::WINDOW_INSET + m::CONTROL_HEIGHT + m::SPACING + 4.0 * m::MARK_ROW + 3.0 * m::TIGHT + m::SPACING + m::CONTROL_HEIGHT;

impl Terminal {
    fn open_find(&mut self, cx: &mut Cx, target: WindowId) {
        if let Some((&id, _)) = self.windows.iter().find(|(_, w)| matches!(w, Win::Find { target: t } if *t == target)) {
            cx.activate(id);
            return;
        }
        let (w, h) = (FIND_W as u32, FIND_H as u32);
        let id = cx.open_window(WindowOptions::new("Find", w, h).min_size(w, h).max_size(900, h));
        self.windows.insert(id, Win::Find { target });
    }

    /// Search `win`'s scrollback; the visual bell says there's no match.
    fn find_in(&mut self, cx: &mut Cx, win: WindowId) {
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return };
        let found = find::find(&mut t.session.term.lock(), &self.search);
        if !found {
            t.bell = Some(std::time::Instant::now());
        }
        cx.redraw(win);
    }

    fn find_ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui, target: WindowId) {
        let full = ui.max_rect().shrink(m::WINDOW_INSET);
        let s = &mut self.search;
        ui.scope_builder(egui::UiBuilder::new().max_rect(full).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
            ui.spacing_mut().item_spacing.y = m::TIGHT;
            ui.add(TextControl::new(&mut s.text).label("Find:").divider(40.0).autofocus(true));
            ui.add_space(m::SPACING - m::TIGHT);
            ui.add(CheckBox::new(&mut s.case, "Case-sensitive"));
            ui.add(CheckBox::new(&mut s.words, "Whole words only"));
            ui.add(CheckBox::new(&mut s.wrap, "Wrap-around search"));
            ui.add(CheckBox::new(&mut s.backwards, "Search backwards"));
        });
        let bw = m::BUTTON_MIN_WIDTH;
        let y = full.bottom() - m::CONTROL_HEIGHT;
        let go_r = egui::Rect::from_min_size(egui::pos2(full.right() - bw, y), egui::vec2(bw, m::CONTROL_HEIGHT));
        let cancel_r = go_r.translate(egui::vec2(-(bw + m::SPACING + 2.0), 0.0));
        let cancel = ui.put(cancel_r, Button::new("Cancel")).clicked() || ui.input(|i| i.key_pressed(Key::Escape));
        let ready = !s.text.is_empty();
        let go = ui.add_enabled_ui(ready, |ui| ui.put(go_r, Button::new("Find").default(true))).inner.clicked()
            || ready && ui.input(|i| i.key_pressed(Key::Enter));
        if go {
            self.find_in(cx, target);
        }
        if go || cancel {
            self.windows.remove(&window);
            cx.close_window(window);
            if go {
                cx.activate(target);
            }
        }
    }
}

impl App for Terminal {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui) {
        match self.windows.get(&window) {
            Some(Win::Term(_)) => self.term_ui(cx, window, ui),
            Some(Win::ConfirmClose { target, program }) => {
                let (target, message) = (*target, format!("“{program}” is still running in this window. Close it anyway?"));
                let Some(choice) = widgets::alert(ui, "terminal", &message, &["Cancel", "Close"]) else {
                    return;
                };
                self.windows.remove(&window);
                cx.close_window(window);
                if choice == 1 {
                    self.close(cx, target);
                }
            }
            Some(&Win::Find { target }) => self.find_ui(cx, window, ui, target),
            Some(Win::Daemon(_)) => {
                let Some(Win::Daemon(d)) = self.windows.get_mut(&window) else { return };
                match d.ui(cx, ui) {
                    Some(daemon::Action::ConfirmStartOver) => {
                        let id = cx.open_window(WindowOptions::alert("Terminal", START_OVER));
                        self.windows.insert(id, Win::ConfirmStartOver { target: window });
                    }
                    Some(daemon::Action::Show(shell)) => {
                        let shown = self.windows.iter().find(|(_, w)| matches!(w, Win::Term(t) if t.session.id == shell.id)).map(|(&id, _)| id);
                        match shown {
                            Some(id) => cx.activate(id),
                            None => self.reopen(cx, &shell, None),
                        }
                    }
                    None => {}
                }
            }
            Some(&Win::ConfirmStartOver { target }) => {
                let Some(choice) = widgets::alert(ui, "terminal", START_OVER, &["Cancel", "Start Over"]) else {
                    return;
                };
                self.windows.remove(&window);
                cx.close_window(window);
                if choice == 1
                    && let Some(Win::Daemon(d)) = self.windows.get_mut(&target)
                {
                    self.respawn = true;
                    d.start_over(cx.proxy());
                }
            }
            None => {}
        }
    }

    fn menu_bar(&mut self, window: WindowId) -> Option<Vec<Menu>> {
        let Some(Win::Term(t)) = self.windows.get(&window) else { return None };
        let has_selection = t.session.term.lock().selection.as_ref().is_some_and(|s| !s.is_empty());
        let size = (t.session.size.cols, t.session.size.rows);
        let mut window_size = Menu::new("Window Size");
        for (c, r) in [(80, 24), (80, 25), (80, 40), (120, 25), (120, 40), (132, 25), (132, 50)] {
            window_size = window_size.item(format!("{c} × {r}"), format!("grid-{c}x{r}"), None).checked(size == (c, r));
        }
        let mut text = Menu::new("Text Size")
            .item_shift("Bigger", "bigger", Key::Equals)
            .item_shift("Smaller", "smaller", Key::Minus)
            .item_shift("Default", "default-size", Key::Num0)
            .separator();
        for s in TEXT_SIZES {
            text = text.item(format!("{s}"), format!("size-{s}"), None).checked(self.settings.text_size == s);
        }
        let scheme = Menu::new("Colors")
            .item("Match Appearance", "scheme-auto", None)
            .checked(self.settings.scheme == SchemeChoice::Auto)
            .item("Tabula Light", "scheme-light", None)
            .checked(self.settings.scheme == SchemeChoice::Light)
            .item("Tabula Dark", "scheme-dark", None)
            .checked(self.settings.scheme == SchemeChoice::Dark);
        Some(vec![
            Menu::new("Terminal")
                .item_shift("New Terminal", "new", Key::N)
                .separator()
                .item("Terminal Daemon…", "daemon", None)
                .separator()
                .item_shift("Close", "close", Key::W)
                .item_shift("Quit", "quit", Key::Q),
            Menu::new("Edit")
                .item_shift("Copy", "copy", Key::C)
                .enabled(has_selection)
                .item_shift("Paste", "paste", Key::V)
                .separator()
                .item_shift("Select All", "select-all", Key::A)
                .separator()
                .item_shift("Find…", "find", Key::F)
                .item_shift("Find Again", "find-again", Key::G)
                .enabled(!self.search.text.is_empty())
                .item_shift("Find Selection", "find-selection", Key::E)
                .enabled(has_selection)
                .separator()
                .item_shift("Clear Scrollback", "clear", Key::K),
            Menu::new("Settings").submenu(window_size).submenu(text).submenu(scheme),
        ])
    }

    fn command(&mut self, cx: &mut Cx, window: WindowId, command: &str) {
        let Some(Win::Term(t)) = self.windows.get_mut(&window) else { return };
        match command {
            "new" => {
                let cwd = t.session.cwd();
                self.open(cx, cwd, None);
            }
            "close" => self.close_requested(cx, window),
            "quit" => {
                // The shells stay in tabula-termd for next time.
                self.link.bye();
                for id in self.windows.keys().copied().collect::<Vec<_>>() {
                    cx.close_window(id);
                }
                self.windows.clear();
                cx.quit();
            }
            "copy" => {
                let text = t.session.term.lock().selection_to_string();
                if let Some(text) = text {
                    cx.copy(text);
                }
            }
            "paste" => cx.paste(window),
            "select-all" => {
                use alacritty_terminal::{
                    index::{Column, Line, Point, Side},
                    selection::{Selection, SelectionType},
                };
                let mut term = t.session.term.lock();
                let top = Point::new(Line(-(term.history_size() as i32)), Column(0));
                let bottom = Point::new(Line(term.screen_lines() as i32 - 1), term.last_column());
                let mut sel = Selection::new(SelectionType::Simple, top, Side::Left);
                sel.update(bottom, Side::Right);
                term.selection = Some(sel);
                drop(term);
                cx.redraw(window);
            }
            "find" => self.open_find(cx, window),
            "daemon" => {
                if let Some((&id, _)) = self.windows.iter().find(|(_, w)| matches!(w, Win::Daemon(_))) {
                    cx.activate(id);
                } else {
                    let (id, d) = daemon::open(cx);
                    self.windows.insert(id, Win::Daemon(Box::new(d)));
                }
            }
            "find-again" => self.find_in(cx, window),
            "find-selection" => {
                let text = t.session.term.lock().selection_to_string();
                if let Some(text) = text {
                    // One line: a match never crosses one.
                    self.search.text = text.lines().next().unwrap_or_default().to_owned();
                    self.find_in(cx, window);
                }
            }
            "clear" => {
                t.session.clear_history();
                cx.redraw(window);
            }
            "bigger" | "smaller" | "default-size" => {
                let cur = self.settings.text_size;
                let next = match command {
                    "bigger" => TEXT_SIZES.iter().copied().find(|&s| s > cur).unwrap_or(cur),
                    "smaller" => TEXT_SIZES.iter().rev().copied().find(|&s| s < cur).unwrap_or(cur),
                    _ => DEFAULT_TEXT_SIZE,
                };
                self.set_text_size(cx, next);
            }
            c if c.starts_with("size-") => {
                if let Ok(s) = c[5..].parse::<f32>() {
                    self.set_text_size(cx, s);
                }
            }
            c if c.starts_with("grid-") => {
                if let Some((cols, rows)) = c[5..].split_once('x')
                    && let (Ok(cols), Ok(rows)) = (cols.parse(), rows.parse())
                {
                    self.set_grid(cx, window, cols, rows);
                }
            }
            "scheme-auto" | "scheme-light" | "scheme-dark" => {
                self.settings.scheme = match command {
                    "scheme-light" => SchemeChoice::Light,
                    "scheme-dark" => SchemeChoice::Dark,
                    _ => SchemeChoice::Auto,
                };
                self.settings.save();
                cx.redraw_all();
            }
            _ => {}
        }
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        match self.windows.get(&window) {
            Some(Win::Term(t)) => {
                if let Some(program) = t.session.foreground().filter(|_| t.session.busy()) {
                    let message = format!("“{program}” is still running in this window. Close it anyway?");
                    let id = cx.open_window(WindowOptions::alert("Terminal", &message));
                    self.windows.insert(id, Win::ConfirmClose { target: window, program });
                } else {
                    self.close(cx, window);
                }
            }
            Some(Win::ConfirmClose { .. } | Win::Find { .. } | Win::Daemon(_) | Win::ConfirmStartOver { .. }) => {
                self.windows.remove(&window);
                cx.close_window(window);
            }
            None => cx.close_window(window),
        }
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        let message = match message.downcast::<Launch>() {
            Ok(launch) => return self.handle_launch(cx, *launch),
            Err(m) => m,
        };
        let message = match message.downcast::<daemon::Report>() {
            Ok(report) => {
                for w in self.windows.values_mut() {
                    if let Win::Daemon(d) = w {
                        d.report(*report);
                        break;
                    }
                }
                cx.redraw_all();
                return;
            }
            Err(m) => m,
        };
        let Ok(msg) = message.downcast::<Message>() else { return };
        let Message(win, event) = *msg;
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return };
        match event {
            TermEvent::Wakeup => cx.redraw(win),
            TermEvent::Title(title) => {
                t.session.title = title;
                cx.redraw(win);
            }
            TermEvent::Bell => {
                t.bell = Some(std::time::Instant::now());
                cx.redraw(win);
            }
            TermEvent::Clipboard(text) => cx.copy(text),
                        TermEvent::Exited(code) => {
                t.session.exited = Some(code);
                // As on BeOS: the window goes when its shell does.
                self.close(cx, win);
            }
        }
    }

    fn theme_changed(&mut self, cx: &mut Cx, palette: &tabula_kit::theme::Palette) {
        self.dark = palette.mode == tabula_kit::theme::ThemeMode::Dark;
        cx.redraw_all();
    }

    fn save_state(&mut self, cx: &Cx) -> Option<String> {
        let mut ids: Vec<WindowId> = self.windows.keys().copied().collect();
        ids.sort();
        let windows = ids
            .into_iter()
            .filter_map(|id| match self.windows.get(&id) {
                Some(Win::Term(t)) if t.session.exited.is_none() => {
                    Some(SavedWindow { shell: t.session.id, size: cx.window_size(id).unwrap_or((640, 400)) })
                }
                _ => None,
            })
            .collect();
        serde_json::to_string(&Saved { windows }).ok()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    let (link, shells) = match client::connect() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("tabula-terminal: can't reach tabula-termd: {e}");
            std::process::exit(1);
        }
    };
    tabula_kit::run(APP_ID, move |cx| {
        instance::listen(APP_ID, cx.proxy());
        let mut t = Terminal {
            link,
            windows: HashMap::new(),
            settings: Settings::load(),
            metrics: None,
            dark: cx.palette().mode == tabula_kit::theme::ThemeMode::Dark,
            search: find::Search::default(),
            respawn: false,
        };
        let state = cx.restored_state();
        let saved = state.as_deref().and_then(|s| serde_json::from_str::<Saved>(s).ok());
        let legacy = state.as_deref().filter(|_| saved.is_none()).and_then(|s| serde_json::from_str::<Legacy>(s).ok());
        let adopted = legacy.is_some_and(|l| t.adopt(cx, l));
        let reattached = t.reattach(cx, shells, saved.as_ref()) || adopted;
        // Asked for a new window (or there's nothing to show): open one.
        if !reattached || (state.is_none() && !args.is_empty()) {
            t.handle_launch(cx, Launch { args, cwd: std::env::current_dir().ok() });
        }
        t
    });
}
