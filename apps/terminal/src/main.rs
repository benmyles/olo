//! Terminal: a shell in a window, BeOS style (one process, any number of
//! windows), on alacritty's terminal emulator. GPU-drawn through the kit,
//! with Olo's own light and dark colour schemes and Catppuccin's
//! (colors.rs), one for each appearance. A window can hold
//! several shells as tabs, and be a named, coloured group of them that
//! can be saved and opened again (groups.rs). Tabs running a coding agent
//! show what it's doing, and say when it needs you (agent.rs).
//!
//! The shells run in olo-termd (termd.rs), so they outlive Terminal:
//! quit it, or have it crash or upgrade, and the next Terminal opens a
//! window on each shell as it was, scrollback and all.

mod agent;
mod client;
mod colors;
mod daemon;
mod find;
mod graphics;
mod groups;
mod io;
mod links;
mod session;
mod view;
#[allow(dead_code)]
mod wire;

use std::{collections::HashMap, path::PathBuf, time::Instant};

use alacritty_terminal::grid::Dimensions;
use olo_kit::{
    App, Cx, Menu, WindowId, WindowOptions,
    egui::{self, Key},
    icons,
    instance::{self, Launch},
    menu::{self, MenuAnchor, MenuRequest},
    metrics as m, style,
    widgets::{self, Button, CheckBox, StripAction, StripTab, TabStrip, TextControl},
};

use agent::{Agent, State};
use colors::{Scheme, SchemeId};
use groups::Group;
use olo_kit::notify::{Note, NoteEvent, NoteId, NoteKind, Notifier};
use session::{Message, Session, Size, Start, TermEvent};
use view::{GlyphCache, Metrics, Mouse};

const APP_ID: &str = "olo.terminal";
/// BeOS Terminal opened at 80×25; wider is more useful on today's screens.
const DEFAULT_SIZE: (usize, usize) = (120, 25);
pub const TEXT_SIZES: [f32; 9] = [9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 16.0, 18.0, 22.0];
const DEFAULT_TEXT_SIZE: f32 = 12.0;
/// The tab strip, shown once a window has more than one tab.
const STRIP: f32 = widgets::STRIP_HEIGHT;
/// The title bar over each pane in Show All Tabs.
const PANE_HEAD: f32 = 22.0;

/// Show All Tabs' panes for `n` tabs in `area`: as many columns as keep
/// each pane nearest a terminal's usual shape, the last row's spread
/// across the width; a line apart.
fn tile(area: egui::Rect, n: usize) -> Vec<egui::Rect> {
    let shape = |cols: usize| {
        let rows = n.div_ceil(cols);
        let aspect = (area.width() / cols as f32) / (area.height() / rows as f32);
        (aspect / 1.8).ln().abs()
    };
    let cols = (1..=n).min_by(|&a, &b| shape(a).total_cmp(&shape(b))).unwrap_or(1);
    let rows = n.div_ceil(cols);
    let h = ((area.height() + 1.0) / rows as f32).floor();
    let mut out = Vec::with_capacity(n);
    for r in 0..rows {
        let in_row = if r + 1 == rows { n - cols * (rows - 1) } else { cols };
        let w = ((area.width() + 1.0) / in_row as f32).floor();
        for c in 0..in_row {
            let x0 = area.left() + c as f32 * w;
            let y0 = area.top() + r as f32 * h;
            let x1 = if c + 1 == in_row { area.right() } else { x0 + w - 1.0 };
            let y1 = if r + 1 == rows { area.bottom() } else { y0 + h - 1.0 };
            out.push(egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1)));
        }
    }
    out
}

/// A pane's title bar in Show All Tabs: its tab's icon and title (cut
/// short with "…"), the one typed into lit as the strip's selected tab
/// is and underlined in the group's colour (or the focus blue); a line
/// around each pane keeps them apart.
fn pane_head(ui: &egui::Ui, pane: egui::Rect, title: &str, icon: &dyn Fn(&egui::Painter, egui::Rect), active: bool, band: Option<egui::Color32>) {
    let p = style::palette(ui.ctx());
    let painter = ui.painter();
    let bg = p.panel_background;
    let well = style::shade(&p, bg, 1.07);
    let edge = style::shade(&p, bg, 1.3);
    let head = egui::Rect::from_min_size(pane.min, egui::vec2(pane.width(), PANE_HEAD));
    painter.rect_filled(head, 0.0, if active { style::c(bg) } else { well });
    style::hline(painter, head.left(), head.right(), head.bottom() - 1.0, edge);
    // Lines between panes (the window's frame does the outer edges).
    style::vline(painter, pane.right(), pane.top(), pane.bottom() + 1.0, edge);
    style::hline(painter, pane.left(), pane.right() + 1.0, pane.bottom(), edge);
    if active {
        let mark = band.unwrap_or(style::c(p.keyboard_navigation));
        painter.rect_filled(egui::Rect::from_min_max(egui::pos2(head.left(), head.bottom() - 2.0), head.max), 0.0, mark);
    }
    let cy = ((head.top() + head.bottom() - 1.0) / 2.0).round();
    let at = style::snap(ui.ctx(), egui::Rect::from_min_size(egui::pos2(head.left() + m::SPACING, cy - 8.0), egui::vec2(16.0, 16.0)));
    icon(painter, at);
    let x = at.right() + m::MARK_GAP;
    let font = if active { style::bold_font() } else { style::plain_font() };
    let text = style::c(p.panel_text);
    let color = if active { text } else { text.lerp_to_gamma(well, 0.3) };
    style::label_fit(painter, x, cy, egui::Align::Min, title, font, color, head.right() - m::SPACING - x);
}

/// Which of the two schemes (`Settings::light_colors`, `dark_colors`)
/// Terminal draws with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
enum SchemeChoice {
    /// Follow the desktop's light/dark appearance.
    #[default]
    Auto,
    /// Always the light-mode one.
    Light,
    /// Always the dark-mode one.
    Dark,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Settings {
    text_size: f32,
    scheme: SchemeChoice,
    cols: usize,
    rows: usize,
    /// The colours in light mode, and in dark mode.
    #[serde(default = "SchemeId::light")]
    light_colors: SchemeId,
    #[serde(default = "SchemeId::dark")]
    dark_colors: SchemeId,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            text_size: DEFAULT_TEXT_SIZE,
            scheme: SchemeChoice::Auto,
            cols: DEFAULT_SIZE.0,
            rows: DEFAULT_SIZE.1,
            light_colors: SchemeId::light(),
            dark_colors: SchemeId::dark(),
        }
    }
}

fn settings_path() -> PathBuf {
    olo_kit::config_dir().join("terminal.json")
}

impl Settings {
    fn load() -> Settings {
        std::fs::read_to_string(settings_path()).ok().map(|t| Settings::parse(&t)).unwrap_or_default()
    }

    fn parse(text: &str) -> Settings {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else { return Settings::default() };
        // From before there was a choice for each appearance, Light and
        // Dark were Olo Light and Olo Dark, always: still so.
        let before = value.get("light_colors").is_none() && value.get("dark_colors").is_none();
        let mut s: Settings = serde_json::from_value(value).unwrap_or_default();
        if before {
            match s.scheme {
                SchemeChoice::Light => s.light_colors = SchemeId::OloLight,
                SchemeChoice::Dark => s.dark_colors = SchemeId::OloDark,
                SchemeChoice::Auto => {}
            }
        }
        s
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

/// One shell, in a tab of a window.
struct Tab {
    session: Session,
    mouse: Mouse,
    /// What its tab says: the program's own title, else what's running.
    title: String,
    /// A visual bell: when it rang.
    bell: Option<Instant>,
    /// The bell rang while another tab was showing.
    rang: bool,
    /// The coding agent running in it, and what it's doing.
    agent: Option<(Agent, State)>,
    /// Its agent stopped (done, or asking) while no one was looking.
    attention: bool,
}

impl Tab {
    fn new(session: Session) -> Tab {
        Tab { session, mouse: Mouse::default(), title: "Terminal".into(), bell: None, rang: false, agent: None, attention: false }
    }

    /// What it's doing, for its tab's name to screen readers and agents.
    fn status(&self) -> Option<String> {
        match self.agent {
            Some((agent, state)) => Some(format!("{}: {}", agent.name(), state.describe())),
            None => self.rang.then(|| "bell rang".into()),
        }
    }

    fn current_title(&self) -> String {
        self.session.title.clone().filter(|s| !s.trim().is_empty()).or_else(|| self.session.foreground()).unwrap_or_else(|| "Terminal".into())
    }

    /// Tell the program whether it has the keyboard, if it asked to know.
    fn report_focus(&self, focused: bool) {
        let mut term = self.session.term.lock();
        term.is_focused = focused;
        if term.mode().contains(alacritty_terminal::term::TermMode::FOCUS_IN_OUT) {
            drop(term);
            self.session.write(if focused { b"\x1b[I".to_vec() } else { b"\x1b[O".to_vec() });
        }
    }
}

struct TermWin {
    tabs: Vec<Tab>,
    active: usize,
    glyphs: GlyphCache,
    /// Size to snap the window to once real font metrics are known.
    snap_to: Option<(usize, usize)>,
    shown_title: String,
    focused: bool,
    group: Option<Group>,
    /// Show All Tabs: every tab at once, tiled in the window.
    tiled: bool,
}

impl TermWin {
    fn new(tabs: Vec<Tab>, active: usize, snap_to: Option<(usize, usize)>, group: Option<Group>) -> TermWin {
        let active = active.min(tabs.len().saturating_sub(1));
        TermWin { active, tabs, glyphs: GlyphCache::default(), snap_to, shown_title: String::new(), focused: true, group, tiled: false }
    }

    fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    fn grid(&self) -> (usize, usize) {
        let s = self.tab().session.size;
        (s.cols, s.rows)
    }

    /// The tab strip shows: several tabs, or a group (its name leads it).
    fn strip(&self) -> bool {
        self.tabs.len() > 1 || self.group.is_some()
    }

    /// Group or not. The window keeps its size as the strip comes or
    /// goes (the grid gives up or gets back a row or so): it doesn't grow
    /// past the screen, or out of the size it was zoomed or tiled to.
    fn set_group(&mut self, group: Option<Group>) {
        self.group = group;
    }

    fn position(&self, shell: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.session.id == shell)
    }

    fn select(&mut self, i: usize) {
        if i >= self.tabs.len() || i == self.active {
            return;
        }
        if self.focused {
            self.tab().report_focus(false);
        }
        self.active = i;
        self.tabs[i].rang = false;
        self.tabs[i].attention = false;
        if self.focused {
            self.tabs[i].report_focus(true);
        }
    }

    /// Take tab `i` out; the one after it (or else before) shows.
    fn take(&mut self, i: usize) -> Tab {
        let was_active = i == self.active;
        let tab = self.tabs.remove(i);
        if self.active > i {
            self.active -= 1;
        }
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
        if was_active && let Some(t) = self.tabs.get_mut(self.active) {
            t.rang = false;
            if self.focused {
                t.report_focus(true);
            }
        }
        // The strip can go with the second tab; the window stays as it is.
        tab
    }
}

enum Win {
    Term(Box<TermWin>),
    /// "Close this tab (or window)?" for one still running something.
    ConfirmClose { target: WindowId, shell: Option<u64>, message: String },
    /// The Find window for a terminal window.
    Find { target: WindowId },
    /// Terminal ▸ Terminal Daemon…
    Daemon(Box<daemon::DaemonWin>),
    /// "Start olo-termd over?" for the Terminal Daemon window.
    ConfirmStartOver { target: WindowId },
    /// Name (or rename) a window's group and choose its colour.
    NameGroup { target: WindowId, name: String, color: usize },
}

struct Terminal {
    link: client::Link,
    windows: HashMap<WindowId, Win>,
    settings: Settings,
    metrics: Option<Metrics>,
    dark: bool,
    search: find::Search,
    /// olo-termd was started over: when its shells' windows have all
    /// closed, open a new one rather than quitting.
    respawn: bool,
    /// The terminal window last in front, for new tabs from outside.
    front: Option<WindowId>,
    /// Groups saved to open again (groups.rs).
    saved_groups: Vec<groups::Saved>,
    agents: agent::Watcher,
    notifier: Notifier,
    /// The notification each shell's agent last put up.
    notes: HashMap<u64, NoteId>,
}

/// Room above the grid: the menu bar, and the tab strip if shown.
fn chrome(strip: bool) -> u32 {
    menu::BAR_HEIGHT as u32 + if strip { STRIP as u32 } else { 0 }
}

/// Estimated content size for `cols`×`rows` before fonts are loaded
/// (corrected after the first frame).
fn estimate(settings: &Settings, cols: usize, rows: usize, strip: bool) -> (u32, u32) {
    let s = settings.text_size;
    let cell_w = (s * 0.6 * 2.0).ceil() / 2.0;
    let cell_h = (s * 1.32 * 2.0).ceil() / 2.0;
    (
        (cols as f32 * cell_w + 2.0 * view::PAD.x).ceil() as u32,
        (rows as f32 * cell_h + 2.0 * view::PAD.y).ceil() as u32 + chrome(strip),
    )
}

/// What an upgraded Terminal gets from its previous self: which shells
/// were in each window (olo-termd has the rest).
#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
    windows: Vec<SavedWindow>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedWindow {
    /// The shell showing (all there was before tabs).
    shell: u64,
    /// Every tab's shell, in order.
    #[serde(default)]
    tabs: Vec<u64>,
    #[serde(default)]
    active: usize,
    size: (u32, u32),
    #[serde(default)]
    group: Option<Group>,
    #[serde(default)]
    tiled: bool,
}

/// What a Terminal from before olo-termd handed on: the shells
/// themselves, which go to olo-termd now.
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
        let s = &self.settings;
        let dark = match s.scheme {
            SchemeChoice::Auto => self.dark,
            SchemeChoice::Light => false,
            SchemeChoice::Dark => true,
        };
        if dark { s.dark_colors.scheme() } else { s.light_colors.scheme() }
    }

    fn open(&mut self, cx: &mut Cx, cwd: Option<PathBuf>, command: Option<Vec<String>>) {
        let (cols, rows) = (self.settings.cols, self.settings.rows);
        let options = self.window_options(cols, rows, false);
        self.add(cx, options, Size { cols, rows }, Some((cols, rows)), vec![Start::Spawn { command, cwd }], 0, None);
    }

    /// A window on shells olo-termd already has (sized as it was, if
    /// we know), one tab each.
    fn reopen(&mut self, cx: &mut Cx, shells: &[&wire::ShellInfo], size: Option<(u32, u32)>, active: usize, group: Option<Group>) -> WindowId {
        let active = active.min(shells.len() - 1);
        let shown = shells[active];
        let (cols, rows) = (shown.grid.cols.max(1) as usize, shown.grid.rows.max(1) as usize);
        let mut options = self.window_options(cols, rows, shells.len() > 1 || group.is_some());
        if let Some(size) = size {
            options.size = size;
        }
        if let Some(t) = &shown.title {
            options.title = t.clone();
        }
        let snap = size.is_none().then_some((cols, rows));
        let starts = shells.iter().map(|s| Start::Attach(s.id)).collect();
        self.add(cx, options, Size { cols, rows }, snap, starts, active, group)
    }

    fn start(&self, cx: &Cx, size: Size, start: Start) -> Tab {
        let cell = self.metrics.map(|m| m.device_cell()).unwrap_or((15, 36));
        let session = Session::start(cx.proxy(), &self.link, size, cell, self.scheme(), start);
        self.agents.watch(session.id, &session.term, &session.shared);
        Tab::new(session)
    }

    #[allow(clippy::too_many_arguments)]
    fn add(&mut self, cx: &mut Cx, options: WindowOptions, size: Size, snap_to: Option<(usize, usize)>, starts: Vec<Start>, active: usize, group: Option<Group>) -> WindowId {
        let id = cx.open_window(options);
        let tabs = starts.into_iter().map(|s| self.start(cx, size, s)).collect();
        self.windows.insert(id, Win::Term(Box::new(TermWin::new(tabs, active, snap_to, group))));
        id
    }

    /// A new tab in `win`, after the one showing, and show it.
    fn add_tab(&mut self, cx: &mut Cx, win: WindowId, start: Start) {
        let Some(Win::Term(t)) = self.windows.get(&win) else { return };
        let (cols, rows) = t.grid();
        let tab = self.start(cx, Size { cols, rows }, start);
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return };
        let at = t.active + 1;
        // The strip comes in the window as it is (see `set_group`).
        t.tabs.insert(at, tab);
        t.select(at);
        cx.redraw(win);
    }

    fn window_options(&self, cols: usize, rows: usize, strip: bool) -> WindowOptions {
        let (w, h) = match self.metrics {
            Some(m) => {
                let (w, h) = m.content_size(cols, rows);
                (w, h + chrome(strip))
            }
            None => estimate(&self.settings, cols, rows, strip),
        };
        let mut o = WindowOptions::new("Terminal", w, h).min_size(200, 120).raw_keys();
        o.panel = false;
        o
    }

    fn handle_launch(&mut self, cx: &mut Cx, launch: Launch) {
        let mut cwd = launch.cwd;
        let mut command = None;
        let mut tab = false;
        let mut args = launch.args.into_iter();
        while let Some(a) = args.next() {
            match a.as_str() {
                "-w" | "--working-directory" => cwd = args.next().map(PathBuf::from),
                "--tab" => tab = true,
                "-e" | "--" => {
                    command = Some(args.by_ref().collect::<Vec<_>>());
                }
                _ => {}
            }
        }
        let command = command.filter(|c: &Vec<String>| !c.is_empty());
        // `--tab`: in the window last in front, if there is one.
        let front = self.front.filter(|w| matches!(self.windows.get(w), Some(Win::Term(_)))).or_else(|| {
            let mut terms: Vec<WindowId> = self.windows.iter().filter(|(_, w)| matches!(w, Win::Term(_))).map(|(&id, _)| id).collect();
            terms.sort();
            terms.last().copied()
        });
        match front.filter(|_| tab) {
            Some(win) => {
                self.add_tab(cx, win, Start::Spawn { command, cwd });
                cx.activate(win);
            }
            None => self.open(cx, cwd, command),
        }
    }

    /// Open windows on the shells left in olo-termd: those this
    /// Terminal had before it upgraded (`saved`, with their tabs as they
    /// were), and any no other Terminal is showing. True if there were any.
    fn reattach(&mut self, cx: &mut Cx, shells: Vec<wire::ShellInfo>, saved: Option<&Saved>) -> bool {
        let ours = std::process::id() as i32;
        // SAFETY: signal 0 only checks the process exists.
        let free = |s: &wire::ShellInfo| s.client.is_none_or(|p| p == ours || unsafe { libc::kill(p, 0) } != 0);
        let free: Vec<&wire::ShellInfo> = shells.iter().filter(|s| free(s)).collect();
        let mut used = std::collections::HashSet::new();
        for w in saved.map(|s| s.windows.as_slice()).unwrap_or_default() {
            let ids = if w.tabs.is_empty() { vec![w.shell] } else { w.tabs.clone() };
            let shown = ids.get(w.active).copied();
            let mine: Vec<&wire::ShellInfo> = ids.iter().filter_map(|id| free.iter().find(|s| s.id == *id && !used.contains(id)).copied()).collect();
            if mine.is_empty() {
                continue;
            }
            used.extend(mine.iter().map(|s| s.id));
            let active = mine.iter().position(|s| Some(s.id) == shown).unwrap_or(0);
            let id = self.reopen(cx, &mine, Some(w.size), active, w.group.clone());
            if let Some(Win::Term(t)) = self.windows.get_mut(&id) {
                t.tiled = w.tiled;
            }
        }
        let rest: Vec<&wire::ShellInfo> = free.iter().filter(|s| !used.contains(&s.id)).copied().collect();
        for shell in &rest {
            self.reopen(cx, &[shell], None, 0, None);
        }
        !used.is_empty() || !rest.is_empty()
    }

    /// Hand the shells a Terminal from before olo-termd left us over
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
            let mut options = self.window_options(w.cols, w.rows, false);
            options.size = w.size;
            let size = Size { cols: w.cols.max(1), rows: w.rows.max(1) };
            self.add(cx, options, size, None, vec![Start::Adopt { pty, pid: w.pid, title: w.title, replay: w.contents }], 0, None);
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
                t.snap_to = Some(t.grid());
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

    /// The window and place of the tab running `shell`.
    fn find_shell(&self, shell: u64) -> Option<(WindowId, usize)> {
        self.windows.iter().find_map(|(&id, w)| match w {
            Win::Term(t) => t.position(shell).map(|i| (id, i)),
            _ => None,
        })
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
        // Files dropped on it are typed as their paths (quoted for the
        // shell, as macOS's Terminal does), text as if pasted.
        if let Some(h) = olo_kit::dnd::hover(ui.ctx()) {
            let mime = if h.has_files() { Some("text/uri-list") } else { h.text_type() };
            if let Some(mime) = mime.filter(|_| h.actions.copy) {
                olo_kit::dnd::accept(ui.ctx(), "term", mime, olo_kit::dnd::Action::Copy);
                olo_kit::dnd::outline(ui, ui.max_rect());
            }
        }
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return };
        let focused = ui.input(|i| i.focused);
        if focused {
            self.front = Some(win);
        }
        if focused != t.focused {
            t.focused = focused;
            t.tab().report_focus(focused);
        }
        if focused {
            t.tab_mut().attention = false;
        }
        // Ctrl+Tab and Ctrl+Page Up/Down (and so ⌥Tab) go through the
        // tabs, as in other terminals; the shell never sees them (nor
        // their releases).
        let step = olo_kit::widgets::tab_keys(ui.ctx());
        if step != 0 && t.tabs.len() > 1 {
            let n = t.tabs.len() as i32;
            t.select((t.active as i32 + step).rem_euclid(n) as usize);
        }
        let full = ui.max_rect();
        let mut area = full;
        let mut action = None;
        if t.strip() {
            let strip = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), STRIP));
            area.min.y = strip.max.y;
            let icons: Vec<_> = t.tabs.iter().map(|tab| tab_icon(tab.rang, tab.agent, tab.attention)).collect();
            let statuses: Vec<Option<String>> = t.tabs.iter().map(Tab::status).collect();
            if t.tabs.iter().any(|x| matches!(x.agent, Some((_, State::Working)))) {
                // The spinner turns.
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            }
            let color = t.group.as_ref().map(|g| groups::color(g.color, self.dark));
            let items: Vec<StripTab> = t
                .tabs
                .iter()
                .zip(&icons)
                .zip(&statuses)
                .map(|((tab, icon), status)| StripTab { label: &tab.title, icon: Some(icon), color, status: status.as_deref() })
                .collect();
            let mut bar = TabStrip::new(("terminal-tabs", win), &items, t.active);
            if let (Some(g), Some(color)) = (&t.group, color) {
                bar = bar.group(&g.name, color);
            }
            if t.tiled && t.tabs.len() > 1 {
                // Each pane has its own head.
                bar = bar.closed();
            } else {
                // The selected tab opens into its screen.
                let (back, fore) = view::screen_colors(&t.tab().session, &scheme);
                bar = bar.opens_into(back, fore);
            }
            action = bar.show(ui, strip);
        }
        t.tab().session.woke();
        {
            let mut current = t.tab().session.shared.scheme.lock().unwrap();
            if *current != scheme {
                *current = scheme.clone();
            }
        }
        if let Some((cols, rows)) = t.snap_to.take().filter(|_| !(t.tiled && t.tabs.len() > 1)) {
            let (w, h) = metrics.content_size(cols, rows);
            let want = (w, h + chrome(t.strip()));
            if cx.window_size(win) != Some(want) {
                cx.resize(win, want);
            }
        }
        let TermWin { tabs, active, glyphs, group, tiled, .. } = &mut **t;
        let session_cwd = |tab: &Tab| {
            let shared = tab.session.shared.clone();
            move || {
                let pid = shared.foreground().or(shared.shell())?;
                std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
            }
        };
        // Show All Tabs: every tab in a pane of its own, the one typed
        // into marked; else only the one showing.
        let panes: Vec<(usize, egui::Rect)> = if *tiled && tabs.len() > 1 {
            tile(area, tabs.len()).into_iter().enumerate().collect()
        } else {
            vec![(*active, area)]
        };
        let several = panes.len() > 1;
        let band = group.as_ref().map(|g| groups::color(g.color, self.dark));
        let mut out: Option<view::Output> = None;
        let mut picked = None;
        for &(i, rect) in &panes {
            let tab = &mut tabs[i];
            let mut screen = rect;
            if several {
                screen.min.y += PANE_HEAD;
                let icon = tab_icon(tab.rang, tab.agent, tab.attention);
                pane_head(ui, rect, &tab.title, &icon, i == *active, band);
                let pressed = ui.input(|inp| inp.pointer.any_pressed() && inp.pointer.interact_pos().is_some_and(|p| rect.contains(p)));
                if pressed && i != *active {
                    picked = Some(i);
                }
            }
            let cwd = session_cwd(tab);
            let o = ui
                .scope_builder(egui::UiBuilder::new().max_rect(screen).id_salt(("pane", tab.session.id)), |ui| {
                    view::show(ui, &mut tab.session, &scheme, &metrics, glyphs, &mut tab.mouse, focused && i == *active, &cwd)
                })
                .inner;
            if several {
                tab.session.resize(o.size, metrics.device_cell());
                if i != *active {
                    // The panes not typed into, dimmed a little.
                    let (back, _) = view::screen_colors(&tab.session, &scheme);
                    ui.painter().rect_filled(screen, 0.0, back.gamma_multiply(0.3));
                }
            }
            match &mut out {
                Some(first) => first.merge(o),
                None => out = Some(o),
            }
        }
        let out = out.expect("a pane");
        if !several {
            // Every tab has the window's grid, so a program in one that
            // isn't showing still lays itself out right.
            for tab in tabs.iter_mut() {
                tab.session.resize(out.size, metrics.device_cell());
            }
        }
        if let Some(i) = picked {
            t.select(i);
            cx.redraw(win);
        }
        let TermWin { tabs, active, group, .. } = &mut **t;
        if let Some(target) = &out.open {
            links::open(target);
        }
        let tab = &mut tabs[*active];
        if let Some(text) = out.copy {
            cx.copy(text);
        }
        if out.paste {
            cx.paste(win);
        }
        // Visual bell: a brief flash of the frame.
        if let Some(at) = tab.bell {
            let age = at.elapsed().as_secs_f32();
            if age < 0.15 {
                let a = ((1.0 - age / 0.15) * 60.0) as u8;
                ui.painter().rect_filled(area, 0.0, egui::Color32::from_rgba_unmultiplied(128, 128, 128, a));
                ui.ctx().request_repaint();
            } else {
                tab.bell = None;
            }
        }
        // Titles: each tab's, and the window's is the one showing (after
        // its group's name).
        for tab in tabs.iter_mut() {
            tab.title = tab.current_title();
        }
        let title = match group {
            Some(g) => format!("{} — {}", g.name, tabs[*active].title),
            None => tabs[*active].title.clone(),
        };
        if title != t.shown_title {
            t.shown_title = title.clone();
            cx.set_title(win, title);
        }
        if let Some(pos) = out.context_menu {
            let has_selection = t.tab().session.term.lock().selection.as_ref().is_some_and(|s| !s.is_empty());
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
        match action {
            Some(StripAction::Select(i)) => {
                t.select(i);
                cx.redraw(win);
            }
            Some(StripAction::Close(i)) => {
                let shell = t.tabs[i].session.id;
                self.close_tab_requested(cx, win, shell);
            }
            Some(StripAction::New) => self.command(cx, win, "new-tab"),
            Some(StripAction::Move { from, to }) => {
                let shown = t.tab().session.id;
                let tab = t.tabs.remove(from);
                t.tabs.insert(to.min(t.tabs.len()), tab);
                t.active = t.position(shown).unwrap_or(0);
                cx.redraw(win);
            }
            Some(StripAction::Group(pos)) => {
                let menu = Menu::new("")
                    .item("Rename Group…", "name-group", None)
                    .item("Save Group", "save-group", None)
                    .item("Ungroup", "ungroup", None)
                    .separator()
                    .item("Show All Tabs", "tile-tabs", None)
                    .checked(t.tiled);
                menu::request_popup(
                    ui.ctx(),
                    MenuRequest { menu, source: ui.make_persistent_id("group-menu"), anchor: MenuAnchor::At(pos), selected: None },
                );
            }
            Some(StripAction::Menu(i, pos)) => {
                let shell = t.tabs[i].session.id;
                let menu = Menu::new("")
                    .item("New Tab", "new-tab", None)
                    .separator()
                    .item("Move to New Window", format!("tab-detach:{shell}"), None)
                    .separator()
                    .item("Close Other Tabs", format!("tab-close-others:{shell}"), None)
                    .item_shift("Close Tab", format!("tab-close:{shell}"), Key::W);
                menu::request_popup(
                    ui.ctx(),
                    MenuRequest { menu, source: ui.make_persistent_id("tab-menu"), anchor: MenuAnchor::At(pos), selected: None },
                );
            }
            None => {}
        }
        // Foreground program changes don't produce output: look again now
        // and then for the title.
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }

    fn close(&mut self, cx: &mut Cx, win: WindowId) {
        if let Some(Win::Term(t)) = self.windows.remove(&win) {
            for tab in &t.tabs {
                tab.session.hang_up();
            }
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

    /// Close the tab running `shell` (hanging it up); the window goes
    /// with its last tab.
    fn close_tab(&mut self, cx: &mut Cx, win: WindowId, shell: u64) {
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return };
        let Some(i) = t.position(shell) else { return };
        if t.tabs.len() == 1 {
            return self.close(cx, win);
        }
        t.take(i).session.hang_up();
        cx.redraw(win);
    }

    /// Close a tab, asking first if something is running in it.
    fn close_tab_requested(&mut self, cx: &mut Cx, win: WindowId, shell: u64) {
        let Some(Win::Term(t)) = self.windows.get(&win) else { return };
        let Some(i) = t.position(shell) else { return };
        let session = &t.tabs[i].session;
        match session.foreground().filter(|_| session.busy()) {
            Some(program) => {
                let place = if t.tabs.len() > 1 { "tab" } else { "window" };
                let message = format!("“{program}” is still running in this {place}. Close it anyway?");
                let id = cx.open_window(WindowOptions::alert("Terminal", &message));
                self.windows.insert(id, Win::ConfirmClose { target: win, shell: Some(shell), message });
            }
            None => self.close_tab(cx, win, shell),
        }
    }

    /// Tab `shell` into a window of its own.
    fn detach(&mut self, cx: &mut Cx, win: WindowId, shell: u64) {
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return };
        let Some(i) = t.position(shell).filter(|_| t.tabs.len() > 1) else { return };
        let mut tab = t.take(i);
        cx.redraw(win);
        let (cols, rows) = (tab.session.size.cols, tab.session.size.rows);
        let mut options = self.window_options(cols, rows, false);
        options.title = tab.title.clone();
        tab.rang = false;
        let id = cx.open_window(options);
        self.windows.insert(id, Win::Term(Box::new(TermWin::new(vec![tab], 0, Some((cols, rows)), None))));
    }
}

/// A tab's icon: Terminal's, with a badge at its corner: a turning arc
/// while an agent works, an orange "!" when it needs an answer, a green
/// dot when it's done and no one has looked, a blue dot for an unseen bell.
fn tab_icon(rang: bool, agent: Option<(Agent, State)>, attention: bool) -> impl Fn(&egui::Painter, egui::Rect) {
    move |painter, r| {
        icons::paint(painter, r, "terminal");
        let p = style::palette(painter.ctx());
        let dark = p.mode == olo_kit::theme::ThemeMode::Dark;
        let at = egui::pos2(r.right() - 1.5, r.top() + 3.5);
        let back = style::c(p.panel_background);
        match agent {
            Some((_, State::Working)) => {
                painter.circle_filled(at, 5.0, back);
                let turn = painter.ctx().input(|i| i.time) as f32 * 5.0;
                let arc: Vec<egui::Pos2> = (0..=16)
                    .map(|k| {
                        let a = turn + k as f32 * (1.5 * std::f32::consts::PI / 16.0);
                        at + egui::vec2(a.cos(), a.sin()) * 3.0
                    })
                    .collect();
                painter.add(egui::Shape::line(arc, egui::Stroke::new(1.5, style::c(p.keyboard_navigation))));
            }
            Some((_, State::Blocked)) => {
                painter.circle_filled(at, 5.5, back);
                painter.circle_filled(at, 4.5, groups::color(1, dark));
                let white = egui::Color32::WHITE;
                painter.rect_filled(egui::Rect::from_center_size(at - egui::vec2(0.0, 1.0), egui::vec2(1.5, 4.0)), 0.0, white);
                painter.rect_filled(egui::Rect::from_center_size(at + egui::vec2(0.0, 2.25), egui::vec2(1.5, 1.5)), 0.0, white);
            }
            Some((_, State::Idle)) if attention => {
                painter.circle_filled(at, 4.0, back);
                painter.circle_filled(at, 3.0, style::c(p.success));
            }
            _ if rang => {
                painter.circle_filled(at, 3.5, back);
                painter.circle_filled(at, 2.5, style::c(p.keyboard_navigation));
            }
            _ => {}
        }
    }
}

const START_OVER: &str = "Starting olo-termd over ends every shell in it, and whatever is running in them. Start over?";

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

    /// Search `win`'s scrollback (its tab showing); the visual bell says
    /// there's no match.
    fn find_in(&mut self, cx: &mut Cx, win: WindowId) {
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return };
        let found = find::find(&mut t.tab().session.term.lock(), &self.search);
        if !found {
            t.tab_mut().bell = Some(Instant::now());
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

impl Terminal {
    /// A tab's agent started, stopped or moved on: show it, and say so
    /// when it now needs the person and they aren't looking at it.
    fn agent_changed(&mut self, cx: &mut Cx, agent::Report(shell, now): agent::Report) {
        let Some((win, i)) = self.find_shell(shell) else { return };
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return };
        let seen = t.focused && i == t.active;
        let tab = &mut t.tabs[i];
        let before = tab.agent;
        tab.agent = now;
        cx.redraw(win);
        let wants = match (before, now) {
            (_, Some((agent, State::Blocked))) => Some((agent, State::Blocked)),
            (Some((_, State::Working | State::Blocked)), Some((agent, State::Idle))) => Some((agent, State::Idle)),
            _ => None,
        };
        let Some((agent, state)) = wants.filter(|_| !seen) else {
            // Answered or moving again: the note has had its day.
            if let Some(id) = self.notes.remove(&shell) {
                self.notifier.close(id);
            }
            return;
        };
        tab.attention = true;
        let place = match &t.group {
            Some(g) => format!("{} — {}", g.name, tab.title),
            None => tab.title.clone(),
        };
        let folder = tab.session.cwd().and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()));
        let body = match folder {
            Some(f) => format!("In “{place}”, in {f}."),
            None => format!("In “{place}”."),
        };
        let (summary, kind) = match state {
            State::Blocked => (format!("{} needs your answer", agent.name()), NoteKind::Important),
            _ => (format!("{} is waiting for you", agent.name()), NoteKind::Information),
        };
        let note = Note::new(summary).body(body).kind(kind).action("default", "Show");
        let id = self.notifier.show(self.notes.get(&shell).copied(), note);
        self.notes.insert(shell, id);
    }
}

const NAME_W: f32 = 340.0;
const NAME_H: f32 = 2.0 * m::WINDOW_INSET + m::CONTROL_HEIGHT + 2.0 * m::SPACING + SWATCH + m::SPACING + m::CONTROL_HEIGHT;
const SWATCH: f32 = 22.0;

impl Terminal {
    /// Saved group `i`: its window if one is open, else a new one with
    /// a shell in each tab's folder.
    fn open_group(&mut self, cx: &mut Cx, i: usize) {
        let Some(saved) = self.saved_groups.get(i).cloned() else { return };
        let open = self.windows.iter().find(|(_, w)| matches!(w, Win::Term(x) if x.group.as_ref().is_some_and(|g| g.name == saved.group.name)));
        if let Some((&id, _)) = open {
            return cx.activate(id);
        }
        let (cols, rows) = (self.settings.cols, self.settings.rows);
        let options = self.window_options(cols, rows, true);
        let mut starts: Vec<Start> = saved.tabs.iter().map(|t| Start::Spawn { command: None, cwd: t.cwd.clone().filter(|d| d.is_dir()) }).collect();
        if starts.is_empty() {
            starts.push(Start::Spawn { command: None, cwd: None });
        }
        self.add(cx, options, Size { cols, rows }, Some((cols, rows)), starts, saved.active, Some(saved.group));
    }

    /// The Group window: a name and a colour for `target`'s group.
    fn name_group_ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui) {
        let Some(Win::NameGroup { target, name, color }) = self.windows.get_mut(&window) else { return };
        let target = *target;
        let full = ui.max_rect().shrink(m::WINDOW_INSET);
        ui.scope_builder(egui::UiBuilder::new().max_rect(full).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
            ui.add(TextControl::new(name).label("Name:").divider(50.0).autofocus(true));
        });
        // The colours, as swatches; the chosen one ringed.
        let p = style::palette(ui.ctx());
        let top = full.top() + m::CONTROL_HEIGHT + 2.0 * m::SPACING;
        style::label(ui.painter(), full.left(), top + SWATCH / 2.0, egui::Align::Min, "Color:", style::plain_font(), style::c(p.panel_text));
        for i in 0..groups::COLORS.len() {
            let r = egui::Rect::from_min_size(egui::pos2(full.left() + 50.0 + i as f32 * (SWATCH + m::SPACING), top), egui::vec2(SWATCH, SWATCH));
            let resp = ui.interact(r, ui.make_persistent_id(("swatch", i)), egui::Sense::click());
            resp.widget_info(|| egui::WidgetInfo { selected: Some(*color == i), ..egui::WidgetInfo::labeled(egui::Role::RadioButton, true, groups::COLORS[i].0) });
            if resp.clicked() {
                *color = i;
            }
            if *color == i {
                ui.painter().circle_stroke(r.center(), SWATCH / 2.0, egui::Stroke::new(2.0, style::c(p.keyboard_navigation)));
            }
            ui.painter().circle_filled(r.center(), SWATCH / 2.0 - 3.0, groups::color(i, self.dark));
        }
        let bw = m::BUTTON_MIN_WIDTH;
        let y = full.bottom() - m::CONTROL_HEIGHT;
        let ok_r = egui::Rect::from_min_size(egui::pos2(full.right() - bw, y), egui::vec2(bw, m::CONTROL_HEIGHT));
        let cancel_r = ok_r.translate(egui::vec2(-(bw + m::SPACING + 2.0), 0.0));
        let cancel = ui.put(cancel_r, Button::new("Cancel")).clicked() || ui.input(|i| i.key_pressed(Key::Escape));
        let ready = !name.trim().is_empty();
        let ok = ui.add_enabled_ui(ready, |ui| ui.put(ok_r, Button::new("OK").default(true))).inner.clicked()
            || ready && ui.input(|i| i.key_pressed(Key::Enter));
        if !(ok || cancel) {
            return;
        }
        let group = Group { name: name.trim().to_owned(), color: *color };
        self.windows.remove(&window);
        cx.close_window(window);
        if ok && let Some(Win::Term(t)) = self.windows.get_mut(&target) {
            // A saved group renamed here keeps its saved tabs under the new name.
            if let Some(old) = t.group.as_ref()
                && let Some(s) = self.saved_groups.iter_mut().find(|s| s.group.name == old.name)
            {
                s.group = group.clone();
                groups::store(&self.saved_groups);
            }
            t.set_group(Some(group));
            cx.redraw(target);
            cx.activate(target);
        }
    }
}

impl App for Terminal {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui) {
        match self.windows.get(&window) {
            Some(Win::Term(_)) => self.term_ui(cx, window, ui),
            Some(Win::ConfirmClose { target, shell, message }) => {
                let (target, shell) = (*target, *shell);
                let Some(choice) = widgets::alert(ui, "terminal", &message.clone(), &["Cancel", "Close"]) else {
                    return;
                };
                self.windows.remove(&window);
                cx.close_window(window);
                if choice == 1 {
                    match shell {
                        Some(shell) => self.close_tab(cx, target, shell),
                        None => self.close(cx, target),
                    }
                }
            }
            Some(&Win::Find { target }) => self.find_ui(cx, window, ui, target),
            Some(Win::NameGroup { .. }) => self.name_group_ui(cx, window, ui),
            Some(Win::Daemon(_)) => {
                let Some(Win::Daemon(d)) = self.windows.get_mut(&window) else { return };
                match d.ui(cx, ui) {
                    Some(daemon::Action::ConfirmStartOver) => {
                        let id = cx.open_window(WindowOptions::alert("Terminal", START_OVER));
                        self.windows.insert(id, Win::ConfirmStartOver { target: window });
                    }
                    Some(daemon::Action::Show(shell)) => match self.find_shell(shell.id) {
                        Some((id, i)) => {
                            if let Some(Win::Term(t)) = self.windows.get_mut(&id) {
                                t.select(i);
                            }
                            cx.activate(id);
                            cx.redraw(id);
                        }
                        None => {
                            self.reopen(cx, &[&shell], None, 0, None);
                        }
                    },
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

    fn dropped(&mut self, cx: &mut Cx, window: WindowId, drop: olo_kit::dnd::Drop) {
        let Some(Win::Term(t)) = self.windows.get(&window) else { return };
        let paths = drop.paths();
        let text = if drop.mime == "text/uri-list" && !paths.is_empty() {
            paths.iter().map(|p| shell_quote(&p.to_string_lossy())).collect::<Vec<_>>().join(" ") + " "
        } else {
            drop.text()
        };
        let session = &t.tab().session;
        let mode = *session.term.lock().mode();
        view::paste(session, &text, mode);
        cx.activate(window);
    }

    fn menu_bar(&mut self, window: WindowId) -> Option<Vec<Menu>> {
        let Some(Win::Term(t)) = self.windows.get(&window) else { return None };
        let has_selection = t.tab().session.term.lock().selection.as_ref().is_some_and(|s| !s.is_empty());
        let size = t.grid();
        let several = t.tabs.len() > 1;
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
        let pick = |title: &str, ids: [SchemeId; 3], chosen: SchemeId| {
            ids.iter().fold(Menu::new(title), |m, &id| m.item(id.name(), id.command(), None).checked(id == chosen))
        };
        let scheme = Menu::new("Colors")
            .item("Match Appearance", "scheme-auto", None)
            .checked(self.settings.scheme == SchemeChoice::Auto)
            .item("Always Light", "scheme-light", None)
            .checked(self.settings.scheme == SchemeChoice::Light)
            .item("Always Dark", "scheme-dark", None)
            .checked(self.settings.scheme == SchemeChoice::Dark)
            .separator()
            .submenu(pick("Light Colors", SchemeId::LIGHT, self.settings.light_colors))
            .submenu(pick("Dark Colors", SchemeId::DARK, self.settings.dark_colors));
        const NUMBERS: [Key; 9] = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
        let grouped = t.group.is_some();
        let mut open_group = Menu::new("Open Saved Group");
        let mut forget_group = Menu::new("Forget Saved Group");
        for (i, g) in self.saved_groups.iter().enumerate() {
            let open = self.windows.values().any(|w| matches!(w, Win::Term(x) if x.group.as_ref().is_some_and(|x| x.name == g.group.name)));
            open_group = open_group.item(&g.group.name, format!("open-group:{i}"), None).checked(open);
            forget_group = forget_group.item(&g.group.name, format!("forget-group:{i}"), None);
        }
        if self.saved_groups.is_empty() {
            open_group = open_group.item("No Saved Groups", "none", None).enabled(false);
            forget_group = forget_group.item("No Saved Groups", "none", None).enabled(false);
        }
        let group = Menu::new("Group")
            .item(if grouped { "Rename Group…" } else { "Name Group…" }, "name-group", None)
            .item("Save Group", "save-group", None)
            .enabled(grouped)
            .item("Ungroup", "ungroup", None)
            .enabled(grouped)
            .separator()
            .submenu(open_group)
            .submenu(forget_group);
        let mut tabs = Menu::new("Tabs")
            .item_shift("Previous Tab", "prev-tab", Key::OpenBracket)
            .enabled(several)
            .item_shift("Next Tab", "next-tab", Key::CloseBracket)
            .enabled(several)
            .separator()
            .item("Move Tab to New Window", "detach", None)
            .enabled(several)
            .separator()
            .submenu(group)
            .item_shift("Show All Tabs", "tile-tabs", Key::D)
            .enabled(several)
            .checked(t.tiled && several)
            .separator();
        for (i, tab) in t.tabs.iter().enumerate() {
            let label = match (tab.agent, tab.rang) {
                (Some((agent, state)), _) => format!("{} — {} {}", tab.title, agent.name(), state.describe()),
                (None, true) => format!("{} (bell)", tab.title),
                (None, false) => tab.title.clone(),
            };
            tabs = match NUMBERS.get(i) {
                Some(&key) => tabs.item_shift(label, format!("tab:{i}"), key),
                None => tabs.item(label, format!("tab:{i}"), None),
            }
            .checked(i == t.active);
        }
        // (Their keys are view::SHORTCUTS: Ctrl+Shift with any other key
        // goes to the shell.)
        Some(vec![
            Menu::new("Terminal")
                .item_shift("New Terminal", "new", Key::N)
                .item_shift("New Tab", "new-tab", Key::T)
                .separator()
                .item("Terminal Daemon…", "daemon", None)
                .separator()
                .item_shift(if several { "Close Tab" } else { "Close" }, "close", Key::W)
                .item("Close Window", "close-window", None)
                .enabled(several)
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
            tabs,
            Menu::new("Settings").submenu(window_size).submenu(text).submenu(scheme),
        ])
    }

    fn command(&mut self, cx: &mut Cx, window: WindowId, command: &str) {
        let Some(Win::Term(t)) = self.windows.get_mut(&window) else { return };
        match command {
            "new" => {
                let cwd = t.tab().session.cwd();
                self.open(cx, cwd, None);
            }
            "new-tab" => {
                let cwd = t.tab().session.cwd();
                self.add_tab(cx, window, Start::Spawn { command: None, cwd });
            }
            "close" => {
                let shell = t.tab().session.id;
                self.close_tab_requested(cx, window, shell);
            }
            "close-window" => self.close_requested(cx, window),
            "next-tab" | "prev-tab" => {
                let n = t.tabs.len();
                let i = if command == "next-tab" { (t.active + 1) % n } else { (t.active + n - 1) % n };
                t.select(i);
                cx.redraw(window);
            }
            "detach" => {
                let shell = t.tab().session.id;
                self.detach(cx, window, shell);
            }
            "name-group" => {
                let (name, color) = match &t.group {
                    Some(g) => (g.name.clone(), g.color),
                    None => {
                        let groups: Vec<&Group> = self.windows.values().filter_map(|w| if let Win::Term(x) = w { x.group.as_ref() } else { None }).collect();
                        let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).chain(self.saved_groups.iter().map(|g| g.group.name.as_str())).collect();
                        let colors: Vec<usize> = groups.iter().map(|g| g.color).collect();
                        (groups::new_name(&names), groups::new_color(&colors))
                    }
                };
                if let Some((&id, _)) = self.windows.iter().find(|(_, w)| matches!(w, Win::NameGroup { target, .. } if *target == window)) {
                    cx.activate(id);
                } else {
                    let id = cx.open_window(WindowOptions::new("Group", NAME_W as u32, NAME_H as u32).min_size(NAME_W as u32, NAME_H as u32).max_size(NAME_W as u32, NAME_H as u32));
                    self.windows.insert(id, Win::NameGroup { target: window, name, color });
                }
            }
            "ungroup" => {
                t.set_group(None);
                cx.redraw(window);
            }
            "save-group" => {
                if let Some(group) = t.group.clone() {
                    let tabs = t.tabs.iter().map(|x| groups::SavedTab { cwd: x.session.cwd(), title: x.title.clone() }).collect();
                    let saved = groups::Saved { group, tabs, active: t.active };
                    match self.saved_groups.iter_mut().find(|g| g.group.name == saved.group.name) {
                        Some(g) => *g = saved,
                        None => self.saved_groups.push(saved),
                    }
                    self.saved_groups.sort_by_key(|g| g.group.name.to_lowercase());
                    groups::store(&self.saved_groups);
                }
            }
            "tile-tabs" => {
                if let Some(Win::Term(t)) = self.windows.get_mut(&window) {
                    t.tiled = !t.tiled;
                    if !t.tiled {
                        // Back to one at a time: each tab takes the window's grid.
                        t.snap_to = None;
                    }
                    cx.redraw(window);
                }
            }
            "quit" => {
                // The shells stay in olo-termd for next time.
                self.link.bye();
                for id in self.windows.keys().copied().collect::<Vec<_>>() {
                    cx.close_window(id);
                }
                self.windows.clear();
                cx.quit();
            }
            "copy" => {
                let text = t.tab().session.term.lock().selection_to_string();
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
                let mut term = t.tab().session.term.lock();
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
                let text = t.tab().session.term.lock().selection_to_string();
                if let Some(text) = text {
                    // One line: a match never crosses one.
                    self.search.text = text.lines().next().unwrap_or_default().to_owned();
                    self.find_in(cx, window);
                }
            }
            "clear" => {
                t.tab().session.clear_history();
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
            c if let Some(id) = SchemeId::from_command(c) => {
                if SchemeId::DARK.contains(&id) {
                    self.settings.dark_colors = id;
                } else {
                    self.settings.light_colors = id;
                }
                self.settings.save();
                cx.redraw_all();
            }
            c => {
                if let Some(i) = c.strip_prefix("tab:").and_then(|n| n.parse().ok()) {
                    t.select(i);
                    cx.redraw(window);
                } else if let Some(shell) = c.strip_prefix("tab-close:").and_then(|n| n.parse().ok()) {
                    self.close_tab_requested(cx, window, shell);
                } else if let Some(shell) = c.strip_prefix("tab-detach:").and_then(|n| n.parse().ok()) {
                    self.detach(cx, window, shell);
                } else if let Some(i) = c.strip_prefix("open-group:").and_then(|n| n.parse::<usize>().ok()) {
                    self.open_group(cx, i);
                } else if let Some(i) = c.strip_prefix("forget-group:").and_then(|n| n.parse::<usize>().ok()) {
                    if i < self.saved_groups.len() {
                        self.saved_groups.remove(i);
                        groups::store(&self.saved_groups);
                    }
                } else if let Some(keep) = c.strip_prefix("tab-close-others:").and_then(|n| n.parse::<u64>().ok()) {
                    let others: Vec<u64> = t.tabs.iter().map(|x| x.session.id).filter(|&s| s != keep).collect();
                    for shell in others {
                        self.close_tab_requested(cx, window, shell);
                    }
                }
            }
        }
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        match self.windows.get(&window) {
            Some(Win::Term(t)) => {
                let busy: Vec<String> = t.tabs.iter().filter(|x| x.session.busy()).filter_map(|x| x.session.foreground()).collect();
                let message = match busy.as_slice() {
                    [] => return self.close(cx, window),
                    [one] => format!("“{one}” is still running in this window. Close it anyway?"),
                    [first, rest @ ..] => format!(
                        "“{first}” and {} other program{} are still running in this window. Close it anyway?",
                        rest.len(),
                        if rest.len() == 1 { "" } else { "s" }
                    ),
                };
                let id = cx.open_window(WindowOptions::alert("Terminal", &message));
                self.windows.insert(id, Win::ConfirmClose { target: window, shell: None, message });
            }
            Some(Win::ConfirmClose { .. } | Win::Find { .. } | Win::Daemon(_) | Win::ConfirmStartOver { .. } | Win::NameGroup { .. }) => {
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
        let message = match message.downcast::<agent::Report>() {
            Ok(report) => return self.agent_changed(cx, *report),
            Err(m) => m,
        };
        let message = match message.downcast::<NoteEvent>() {
            Ok(event) => {
                if let NoteEvent::Action { id, token, .. } = *event
                    && let Some((&shell, _)) = self.notes.iter().find(|(_, n)| **n == id)
                    && let Some((win, i)) = self.find_shell(shell)
                {
                    if let Some(Win::Term(t)) = self.windows.get_mut(&win) {
                        t.select(i);
                    }
                    cx.activate_with(win, token);
                    cx.redraw(win);
                }
                return;
            }
            Err(m) => m,
        };
        let Ok(msg) = message.downcast::<Message>() else { return };
        let Message(shell, event) = *msg;
        let Some((win, i)) = self.find_shell(shell) else { return };
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return };
        let showing = i == t.active;
        let tab = &mut t.tabs[i];
        match event {
            // A tab that isn't showing is drawn (and woken again) when it is.
            TermEvent::Wakeup => {
                if showing {
                    cx.redraw(win);
                }
            }
            TermEvent::Title(title) => {
                tab.session.title = title;
                cx.redraw(win);
            }
            TermEvent::Bell => {
                if showing {
                    tab.bell = Some(Instant::now());
                } else {
                    tab.rang = true;
                }
                cx.redraw(win);
            }
            TermEvent::Clipboard(text) => cx.copy(text),
            TermEvent::Exited(code) => {
                tab.session.exited = Some(code);
                // As on BeOS: the window goes when its (last) shell does.
                self.close_tab(cx, win, shell);
            }
        }
    }

    fn theme_changed(&mut self, cx: &mut Cx, palette: &olo_kit::theme::Palette) {
        self.dark = palette.mode == olo_kit::theme::ThemeMode::Dark;
        cx.redraw_all();
    }

    fn save_state(&mut self, cx: &Cx) -> Option<String> {
        let mut ids: Vec<WindowId> = self.windows.keys().copied().collect();
        ids.sort();
        let windows = ids
            .into_iter()
            .filter_map(|id| match self.windows.get(&id) {
                Some(Win::Term(t)) => {
                    let tabs: Vec<u64> = t.tabs.iter().filter(|x| x.session.exited.is_none()).map(|x| x.session.id).collect();
                    let shell = t.tab().session.id;
                    let active = tabs.iter().position(|&s| s == shell).unwrap_or(0);
                    (!tabs.is_empty()).then(|| SavedWindow {
                        shell: tabs[active],
                        tabs,
                        active,
                        size: cx.window_size(id).unwrap_or((640, 400)),
                        group: t.group.clone(),
                        tiled: t.tiled,
                    })
                }
                _ => None,
            })
            .collect();
        serde_json::to_string(&Saved { windows }).ok()
    }
}

fn main() {
    // A session or install from before the rename (Tabula) carries on.
    olo_kit::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    let (link, shells) = match client::connect() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("olo-terminal: can't reach olo-termd: {e}");
            std::process::exit(1);
        }
    };
    olo_kit::run(APP_ID, move |cx| {
        instance::listen(APP_ID, cx.proxy());
        let mut t = Terminal {
            link,
            windows: HashMap::new(),
            settings: Settings::load(),
            metrics: None,
            dark: cx.palette().mode == olo_kit::theme::ThemeMode::Dark,
            search: find::Search::default(),
            respawn: false,
            front: None,
            saved_groups: groups::load(),
            agents: agent::Watcher::start(cx.proxy()),
            notifier: Notifier::new(cx.proxy(), "Terminal", "olo-terminal"),
            notes: HashMap::new(),
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

/// `s` as one shell word: as it is when that is safe, else single-quoted.
fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"/-._+,:@%=".contains(&b)) {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod drop_tests {
    #[test]
    fn shell_quote() {
        assert_eq!(super::shell_quote("/tmp/a.png"), "/tmp/a.png");
        assert_eq!(super::shell_quote("/tmp/My Photo.png"), "'/tmp/My Photo.png'");
        assert_eq!(super::shell_quote("/tmp/it's"), "'/tmp/it'\\''s'");
    }
}

#[cfg(test)]
mod settings_tests {
    use super::{SchemeChoice, SchemeId, Settings};

    #[test]
    fn colours_for_each_appearance() {
        // New: Catppuccin Frappé in light mode, Macchiato in dark.
        let s = Settings::parse("{}");
        assert_eq!((s.light_colors, s.dark_colors), (SchemeId::Frappe, SchemeId::Macchiato));
        let old = r#"{"text_size":12.0,"cols":120,"rows":25,"scheme":"#;
        // Saved before the choice: matching the appearance takes the new
        // defaults, while one who picked Olo Light (or Dark) keeps it.
        let s = Settings::parse(&format!("{old}\"Auto\"}}"));
        assert_eq!((s.scheme, s.light_colors, s.dark_colors), (SchemeChoice::Auto, SchemeId::Frappe, SchemeId::Macchiato));
        let s = Settings::parse(&format!("{old}\"Light\"}}"));
        assert_eq!((s.scheme, s.light_colors), (SchemeChoice::Light, SchemeId::OloLight));
        let s = Settings::parse(&format!("{old}\"Dark\"}}"));
        assert_eq!((s.scheme, s.dark_colors), (SchemeChoice::Dark, SchemeId::OloDark));
        // Saved since: as saved.
        let s = Settings::parse(&format!("{old}\"Light\",\"light_colors\":\"latte\",\"dark_colors\":\"mocha\"}}"));
        assert_eq!((s.light_colors, s.dark_colors), (SchemeId::Latte, SchemeId::Mocha));
        let round = Settings::parse(&serde_json::to_string(&s).unwrap());
        assert_eq!((round.scheme, round.light_colors, round.dark_colors), (s.scheme, s.light_colors, s.dark_colors));
    }
}
