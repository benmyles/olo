//! Preferences: one small window per panel, as BeOS had a preference app
//! per area, all in one process. `olo-preferences <panel>` opens a panel
//! (Deskbar's Preferences menu does this); with no argument a launcher
//! window lists them all.
//!
//! Settings that live in app_server (appearance, keyboard, touchpad,
//! workspaces, displays) are read and written over its IPC: a watcher
//! thread keeps our copy current and a writer thread sends changes,
//! newest first, so dragging a slider never blocks the UI.
//!
//! Each window is a view on a looper of its own (olo-kit). What they
//! share (the settings, the displays, the undo history, which windows
//! are open) is [`Prefs`], behind a lock ([`prefs`]): a window's view
//! takes it to say what it shows and to change a setting, never while
//! it's held, and lets go before it calls the kit.

mod about;
mod ai;
mod airpods;
mod appearance;
mod backup;
mod bluetooth;
mod complete;
mod datetime;
mod default_apps;
mod deskbar;
mod display;
mod face_unlock;
mod form;
mod gpus;
mod integrations;
mod keyboard;
#[cfg(test)]
mod lint;
mod network;
mod notifications;
mod palm_trainer;
mod panel;
mod performance;
mod power;
mod screensaver;
mod security;
mod security_keys;
mod server;
mod shortcuts;
mod sound;
mod telegram;
mod terminal;
mod themes;
mod touchid;
mod touchpad;
mod tracker;
mod typing_tutor;
mod undo;
mod voice_try;
mod windows;
mod workspaces;

use std::{
    sync::{Mutex, MutexGuard, OnceLock},
    time::Duration,
};

use olo_kit::{
    Bounds, Cx, Element, FontWeight, Hsla, InteractiveElement as _, MouseButton, ParentElement as _, Styled as _, View, WindowHandle, WindowOptions, canvas, div, fill, icons,
    instance::{self, Launch},
    metrics as m, point, px, size, style, theme,
};
use olo_protocol::{OutputInfo, PowerStatus, Settings};

use form::Fields;
use panel::Panel;

const APP_ID: &str = "olo.preferences";

pub enum Msg {
    Settings(Box<Settings>),
    Outputs(Vec<OutputInfo>),
    Power(PowerStatus),
    /// Something a panel shows changed (it reads it from the server).
    Redraw,
}

/// The settings, app_server's displays and power state, and the windows.
pub struct Prefs {
    pub settings: Option<Settings>,
    pub outputs: Vec<OutputInfo>,
    /// app_server told its displays (perhaps none).
    pub outputs_in: bool,
    /// Windows not shown until the settings and displays are in, briefly
    /// (`olo_kit::hold_first_show`).
    pub holds: Vec<olo_kit::FirstShowHold>,
    /// app_server's power state (caffeinated, brightness), once known.
    pub power: Option<PowerStatus>,
    pub server: server::Server,
    windows: Vec<Shown>,
    /// Edit ▸ Undo and Redo for the changes made here (`undo.rs`), for
    /// every panel: ⌘Z in any of them, the palette's and agents' Edit menu.
    pub history: olo_kit::undo::History<Vec<undo::Diff>>,
}

static PREFS: OnceLock<Mutex<Prefs>> = OnceLock::new();

/// What every window shares. Held briefly: to read what a window shows,
/// to change a setting. Never taken while it's held, nor held across a
/// call into the kit that waits on a window.
/// Have the Olo Doctor beside us do something that's the computer's
/// (`olo-doctor --fans cool`, `--scheduler lavd`: it asks for an
/// administrator's password through polkit, in the person's own
/// session) and wait for it: what it said at the end, or why it didn't.
/// Off the looper: it waits for the person.
pub fn ask_doctor(args: &[&str]) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = std::path::PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
    let doctor = exe.with_file_name("olo-doctor");
    let out = std::process::Command::new(&doctor).args(args).stdin(std::process::Stdio::null()).output().map_err(|e| format!("{}: {e}", doctor.display()))?;
    let last = |bytes: &[u8]| String::from_utf8_lossy(bytes).lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_default().trim().to_owned();
    if out.status.success() {
        return Ok(last(&out.stdout));
    }
    let why = last(&out.stderr);
    let why = why.strip_prefix("olo-doctor: ").unwrap_or(&why);
    // (As a sentence of its own.)
    let mut c = why.chars();
    Err(match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => "Olo Doctor couldn't set it.".to_owned(),
    })
}

pub fn prefs() -> MutexGuard<'static, Prefs> {
    PREFS.get().expect("Preferences is running").lock().unwrap_or_else(|e| e.into_inner())
}

/// The lint's shared state (its tests' windows are made on loopers of
/// their own, as the app's are).
#[cfg(test)]
pub(crate) fn set_for_tests(p: Prefs) {
    if let Err(p) = PREFS.set(Mutex::new(p)) {
        *prefs() = p.into_inner().unwrap_or_else(|e| e.into_inner());
    }
}

/// A window open: a panel's, or the launcher.
enum Shown {
    Panel(Panel, WindowHandle<PanelView>),
    Launcher(WindowHandle<Launcher>),
}

impl Shown {
    fn panel(&self) -> Option<Panel> {
        match self {
            Shown::Panel(p, _) => Some(*p),
            Shown::Launcher(_) => None,
        }
    }

    fn is_open(&self) -> bool {
        match self {
            Shown::Panel(_, w) => w.is_open(),
            Shown::Launcher(w) => w.is_open(),
        }
    }

    fn raise(&self) {
        match self {
            Shown::Panel(_, w) => olo_kit::raise(Some(w)),
            Shown::Launcher(w) => olo_kit::raise(Some(w)),
        };
    }

    /// It says what it shows again.
    fn redraw(&self) {
        match self {
            Shown::Panel(_, w) => w.messenger().send(PanelMsg::Redraw),
            Shown::Launcher(w) => w.messenger().send(()),
        };
    }
}

impl Prefs {
    /// Changes the settings: app_server is sent them, and every window
    /// showing them is drawn again. (Not connected yet: nothing to change.)
    pub fn edit(&mut self, f: impl FnOnce(&mut Settings)) {
        self.edit_as("", "Change", f);
    }

    /// `edit`, kept for Undo as `name` (Edit ▸ Undo “Volume”); with a
    /// `run` key, part of a run of it (a slider dragged, a field typed in:
    /// one step).
    pub fn edit_as(&mut self, run: &str, name: &str, f: impl FnOnce(&mut Settings)) {
        let Some(s) = self.settings.as_mut() else { return };
        let before = s.clone();
        f(s);
        if *s != before {
            let change = undo::diff(&before, s);
            if !change.is_empty() {
                if run.is_empty() {
                    self.history.done(name, change);
                } else {
                    self.history.done_merging(run, name, change, undo::merge);
                }
            }
            self.server.set_settings(s.clone());
            self.redraw();
        }
    }

    /// Edit ▸ Undo (or Redo): the last step's settings back as they were
    /// (or as they became), the rest as they are.
    pub fn undo_redo(&mut self, redo: bool) {
        let step = if redo { self.history.take_redo() } else { self.history.take_undo() };
        let Some(step) = step else { return };
        if let Some(s) = self.settings.as_mut()
            && let Some(new) = undo::apply(s, &step.change, redo)
        {
            *s = new;
            self.server.set_settings(s.clone());
        }
        let purpose = if redo { olo_kit::undo::Purpose::Redo } else { olo_kit::undo::Purpose::Undo };
        self.history.record(purpose, step.name, step.change);
        self.redraw();
    }

    /// Every window says what it shows again.
    pub fn redraw(&mut self) {
        self.windows.retain(Shown::is_open);
        for w in &self.windows {
            w.redraw();
        }
    }

    /// `panel`'s window, if it's open.
    fn window_of(&self, panel: Panel) -> Option<WindowHandle<PanelView>> {
        self.windows.iter().find_map(|w| match w {
            Shown::Panel(p, w) if *p == panel && w.is_open() => Some(w.clone()),
            _ => None,
        })
    }

    pub fn open(&mut self, name: &str) {
        let Some(panel) = Panel::from_arg(name) else {
            tracing::warn!("no preference panel called {name}");
            return;
        };
        self.windows.retain(Shown::is_open);
        // One window per panel: bring an open one forward.
        if let Some(w) = self.windows.iter().find(|w| w.panel() == Some(panel)) {
            w.raise();
            return;
        }
        // As tall as what it shows needs (its size at least), and no
        // taller than the screen, where it scrolls (`fits_content`).
        let (w, h) = panel.size();
        let opened = self.settings.clone().unwrap_or_default();
        let options = WindowOptions { fixed: true, ..WindowOptions::new(panel.title(), w as f32, h as f32).fits_content() };
        let window = olo_kit::open_window(options, move |cx| PanelView::new(panel, opened, cx));
        self.windows.push(Shown::Panel(panel, window));
    }

    fn open_launcher(&mut self) {
        self.windows.retain(Shown::is_open);
        if let Some(w) = self.windows.iter().find(|w| w.panel().is_none()) {
            w.raise();
            return;
        }
        let rows = Panel::ALL.len().div_ceil(LAUNCHER_COLUMNS) as f32;
        let w = 2.0 * m::WINDOW_INSET + LAUNCHER_COLUMNS as f32 * LAUNCHER_CELL.0;
        let h = 2.0 * m::WINDOW_INSET + rows * LAUNCHER_CELL.1;
        let options = WindowOptions { fixed: true, ..WindowOptions::new("Preferences", w, h).min_size(w, h).floating() };
        let window = olo_kit::open_window(options, |_| Launcher { hovered: None });
        self.windows.push(Shown::Launcher(window));
    }

    fn handle_args(&mut self, args: &[String]) {
        // A setting to show (the command palette's, `olo_xdg::settings`):
        // its panel opens with it lit, on its tab, in its box.
        let (mut show, mut tab, mut group, mut rest) = (None, None, None, Vec::new());
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--show" => show = it.next().cloned(),
                "--tab" => tab = it.next().cloned(),
                "--group" => group = it.next().cloned(),
                _ => rest.push(a.clone()),
            }
        }
        let (files, panels): (Vec<&String>, Vec<&String>) =
            rest.iter().filter(|a| !a.starts_with('-')).partition(|a| [backup::EXTENSION, backup::OLD_EXTENSION].iter().any(|x| a.ends_with(&format!(".{x}"))));
        if panels.is_empty() && files.is_empty() {
            self.open_launcher();
        }
        for p in &panels {
            self.open(p);
        }
        // A tab alone: the panel opens on it (Deskbar's Ethernet menu:
        // `network --tab Ethernet`; the GPUs panel's old names, Power's
        // Graphics tab).
        let last = panels.last().map(|p| p.as_str());
        let tab = tab
            .or_else(|| matches!(last, Some("gpus" | "gpu" | "graphics" | "nvidia")).then(|| "Graphics".to_owned()))
            // (Security's Passwords tab by its old names.)
            .or_else(|| matches!(last, Some("passwords" | "privacy")).then(|| "Passwords".to_owned()));
        // (Told once its view is made: its window's looper takes what
        // it's sent in turn.)
        if let Some(window) = last.and_then(Panel::from_arg).and_then(|panel| self.window_of(panel))
            && (tab.is_some() || show.is_some())
        {
            window.messenger().send(PanelMsg::Show { tab, group, name: show });
        }
        // A backup opened from Tracker: what it holds, ready to import
        // (waiting for the panel made for it, or taken by the one there).
        if let Some(file) = files.last() {
            backup::open_later(std::path::PathBuf::from(file.as_str()));
            self.open("backup");
            if let Some(window) = self.window_of(Panel::Backup) {
                window.messenger().send(PanelMsg::TakeOpened);
            }
        }
    }

    fn message(&mut self, msg: Msg) {
        match msg {
            Msg::Settings(s) => {
                // The first copy is also what Revert goes back to.
                if self.settings.is_none() {
                    for w in &self.windows {
                        if let Shown::Panel(_, w) = w {
                            w.messenger().send(PanelMsg::Opened(s.clone()));
                        }
                    }
                }
                self.settings = Some(*s);
            }
            Msg::Outputs(o) => {
                self.outputs = o;
                self.outputs_in = true;
            }
            Msg::Power(p) => self.power = Some(p),
            Msg::Redraw => {}
        }
        if self.settings.is_some() && self.outputs_in {
            self.holds.clear();
        }
        self.redraw();
    }

    fn save_state(&mut self) -> Option<String> {
        self.windows.retain(Shown::is_open);
        let open: Vec<&str> = self.windows.iter().map(|w| w.panel().map_or("", Panel::arg)).collect();
        serde_json::to_string(&open).ok()
    }
}

/// A panel's window: the panel, the settings as they were when it opened
/// (for Revert), and what it keeps of its own.
pub struct PanelView {
    pub panel: Panel,
    pub opened: Settings,
    pub state: panel::State,
    pub fields: Fields,
}

/// What a panel's window is told.
pub enum PanelMsg {
    /// What it shows changed (the settings, what a worker found).
    Redraw,
    /// The settings as app_server first said them: what Revert goes
    /// back to.
    Opened(Box<Settings>),
    /// On tab `tab`, with the setting called `name` lit, in its box.
    Show { tab: Option<String>, group: Option<String>, name: Option<String> },
    /// The setting is looked for once its tab has drawn (and again,
    /// while its panel waits for what it shows).
    Light { tab: Option<String>, group: Option<String>, name: String, tries: u8 },
    /// A backup opened from Tracker is waiting (`backup::open_later`).
    TakeOpened,
}

impl PanelView {
    pub(crate) fn new(panel: Panel, opened: Settings, cx: &mut Cx<Self>) -> Self {
        let state = panel::State::new(panel, cx);
        PanelView { panel, opened, state, fields: Fields::default() }
    }
}

impl View for PanelView {
    type Message = PanelMsg;

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        // Edit ▸ Undo and Redo for the command palette and agents (the
        // windows have no menu bar).
        let edit = prefs().history.edit_items(olo_kit::Menu::new("Edit"));
        olo_kit::menu::set_menus(cx, vec![edit]);
        panel::render(self, cx)
    }

    fn message(&mut self, message: PanelMsg, cx: &mut Cx<Self>) {
        match message {
            PanelMsg::Redraw => {}
            PanelMsg::Opened(s) => self.opened = *s,
            PanelMsg::Show { tab, group, name } => {
                // A tab alone: the panel opens on it, by its state (its
                // old names too: Power's Graphics, Security's Passwords).
                if let (Some(tab), None) = (&tab, &name) {
                    match &mut self.state {
                        panel::State::Network(st) => network::show_tab(st, tab),
                        panel::State::Power(st) => power::show_tab(st, tab),
                        panel::State::Performance(st) => performance::show_tab(st, tab),
                        panel::State::Security(st) => security::show_tab(st, tab),
                        _ => {}
                    }
                }
                if let Some(name) = name {
                    cx.after(LIGHT_AFTER, PanelMsg::Light { tab, group, name, tries: 20 });
                }
            }
            // (What's lit is found in what the window drew: its tab
            // chosen first, as a click would, if it isn't the one
            // showing; then the control, once that has drawn.)
            PanelMsg::Light { tab, group, name, tries } => {
                let on_tab = tab.as_deref().is_none_or(|t| t.is_empty() || !olo_kit::spotlight::show_tab(cx, t) || olo_kit::spotlight::tab_selected(cx, t));
                if !(on_tab && olo_kit::spotlight::show(cx, group.as_deref(), &name)) && tries > 0 {
                    cx.after(LIGHT_AFTER, PanelMsg::Light { tab, group, name, tries: tries - 1 });
                }
                return;
            }
            PanelMsg::TakeOpened => {
                if let panel::State::Backup(b) = &mut self.state {
                    b.take_opened();
                }
            }
        }
        cx.invalidate();
    }

    /// ⌘Z and ⇧⌘Z (a text field's own first), and the Edit menu's: the
    /// app's Undo and Redo.
    fn command(&mut self, command: &str, _: &mut Cx<Self>) -> bool {
        undo_command(command)
    }
}

/// How long after a panel is told to show a setting it looks for it (it
/// has drawn by then, on its tab), and how long between tries while it
/// isn't there yet (a panel waiting for app_server's settings).
const LIGHT_AFTER: Duration = Duration::from_millis(150);

fn undo_command(command: &str) -> bool {
    match command {
        "undo" | "redo" => {
            prefs().undo_redo(command == "redo");
            true
        }
        _ => false,
    }
}

const LAUNCHER_COLUMNS: usize = 5;
/// (Wide enough for the longest names, "Touchpad & Mouse" and "About Olo
/// Desktop": at 104 they ran past their cells, into the next.)
const LAUNCHER_CELL: (f32, f32) = (120.0, 64.0);

/// The launcher: every panel's icon and name; a click opens it.
struct Launcher {
    /// The cell the pointer is over.
    hovered: Option<usize>,
}

impl View for Launcher {
    type Message = ();

    fn message(&mut self, _: (), cx: &mut Cx<Self>) {
        cx.invalidate();
    }

    fn command(&mut self, command: &str, _: &mut Cx<Self>) -> bool {
        undo_command(command)
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let edit = prefs().history.edit_items(olo_kit::Menu::new("Edit"));
        olo_kit::menu::set_menus(cx, vec![edit]);
        let mut grid = div().relative().size_full();
        for (i, panel) in Panel::ALL.iter().copied().enumerate() {
            let (col, row) = (i % LAUNCHER_COLUMNS, i / LAUNCHER_COLUMNS);
            let (x, y) = (m::WINDOW_INSET + col as f32 * LAUNCHER_CELL.0, m::WINDOW_INSET + row as f32 * LAUNCHER_CELL.1);
            let hovered = self.hovered == Some(i);
            let cell = div()
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(LAUNCHER_CELL.0))
                .h(px(LAUNCHER_CELL.1))
                .child(
                    canvas(move |g, b| {
                        let p = theme::palette();
                        if hovered {
                            let shade: Hsla = style::shade(&p, p.panel_background, 1.08);
                            g.paint_quad(fill(style::inset(g, b, 2.0), shade).corner_radii(px(3.0)));
                        }
                        let icon = Bounds { origin: point(b.center().x - px(16.0), b.top() + px(4.0)), size: size(px(32.0), px(32.0)) };
                        icons::paint(g, icon, panel.icon());
                        style::label(g, b.center().x, icon.bottom() + px(12.0), style::Align::Center, panel.title(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::c(p.panel_text));
                        olo_kit::agent::record(g, olo_kit::agent::Node::button(panel.title(), b));
                        // (Each name centred under its icon: not lined up
                        // on the left with the one under it.)
                        olo_kit::review::own_grid(g, b);
                    })
                    .size_full(),
                )
                .on_hover(cx.listener(move |v: &mut Launcher, over: &bool, cx| {
                    let now = if *over {
                        Some(i)
                    } else if v.hovered == Some(i) {
                        None
                    } else {
                        v.hovered
                    };
                    if v.hovered != now {
                        v.hovered = now;
                        cx.invalidate();
                    }
                }))
                .on_mouse_down(MouseButton::Left, move |_, _| prefs().open(panel.arg()));
            grid = grid.child(cell);
        }
        grid
    }
}

/// A backup's options after its file: `--sealed`, and `--only "A,B"`
/// (parts by name, as `--inspect` lists them); None if anything else.
fn backup_options(rest: &[String]) -> Option<(bool, Option<Vec<String>>)> {
    let (mut sealed, mut only) = (false, None);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--sealed" => sealed = true,
            "--only" => only = Some(it.next()?.split(',').map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect()),
            _ => return None,
        }
    }
    Some((sealed, only))
}

/// `--export FILE [--sealed]`, `--inspect FILE` or `--import FILE`: a
/// backup without a window, for agents and scripts; the exit code, or
/// None for other flags. A sealed export (passwords and sign-ins with
/// it), and an import of one, read the backup's password from stdin.
/// `--only "Token Pool,Passwords"`: of an export or import, those parts
/// alone (the rest left out, or left as they are here).
fn backup_cli(flag: &str, file: &std::path::Path, sealed: bool, only: Option<Vec<String>>) -> Option<i32> {
    use std::sync::atomic::AtomicBool;
    let no = AtomicBool::new(false);
    let show = |m: &backup::Manifest| {
        println!("made {}, {} {}, {}", m.created, m.files.len(), if m.files.len() == 1 { "file" } else { "files" }, backup::size_text(m.bytes()));
        for (part, files, bytes) in m.parts() {
            match files {
                0 => println!("  {part}: sealed"),
                _ => println!("  {part}: {files} {}, {}", if files == 1 { "file" } else { "files" }, backup::size_text(bytes)),
            }
        }
        if m.sealed.is_some() {
            println!("sealed, with {}", if m.secrets.is_empty() { "no passwords or sign-ins".to_string() } else { m.secrets.join(", ") });
        }
    };
    let choose = |parts: Vec<(String, usize, u64)>| -> Result<backup::Choice, String> {
        match &only {
            Some(wanted) => backup::Choice::only(wanted, &parts.into_iter().map(|p| p.0).collect::<Vec<_>>()),
            None => Ok(backup::Choice::all()),
        }
    };
    let passphrase = || -> Result<zeroize::Zeroizing<String>, String> {
        let mut line = zeroize::Zeroizing::new(String::new());
        std::io::stdin().read_line(&mut line).map_err(|e| e.to_string())?;
        let words = zeroize::Zeroizing::new(line.trim_end_matches(['\n', '\r']).to_string());
        if words.is_empty() {
            return Err("the backup's password is read from stdin, and there was none".into());
        }
        Ok(words)
    };
    let r = match flag {
        "--export" => (|| {
            let choice = choose(backup::parts_here(&backup::roots(), &backup::Root))?;
            let how = if sealed { backup::Seal::Passphrase(passphrase()?) } else { backup::Seal::Open };
            backup::export_only(&backup::roots(), file, &how, &backup::Here::this(), &choice, &|_, _| {}, &no).map(|m| show(&m))
        })(),
        "--inspect" if !sealed && only.is_none() => backup::inspect(file).map(|m| show(&m)),
        "--import" if !sealed => (|| {
            let manifest = backup::inspect(file)?;
            let choice = choose(manifest.parts())?;
            let words = if manifest.needs_passphrase() { Some(passphrase()?) } else { None };
            backup::import_only(&backup::roots(), file, words.as_deref().map(|w| w.as_str()), &backup::Here::this(), &choice, &|_, _| {}, &no).map(|i| {
                show(&i.manifest);
                println!("imported {} {}, {} {}", i.files, if i.files == 1 { "file" } else { "files" }, i.secrets, if i.secrets == 1 { "password or sign-in" } else { "passwords and sign-ins" });
                for note in &i.notes {
                    println!("note: {note}");
                }
                if let Some(p) = &i.before {
                    println!("the settings as they were: {}", p.display());
                }
                // app_server keeps its own copy: hand it the backup's.
                if let Some(settings) = i.settings
                    && let Ok(mut c) = olo_protocol::Client::connect()
                {
                    let _ = c.request(&olo_protocol::Request::SetSettings { settings });
                }
            })
        })(),
        _ => return None,
    };
    Some(match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("olo-preferences: {e}");
            1
        }
    })
}

fn main() {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, file, rest @ ..] = &args[..]
        && let Some((sealed, only)) = backup_options(rest)
        && let Some(code) = backup_cli(flag, std::path::Path::new(file), sealed, only)
    {
        std::process::exit(code);
    }
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_kit::run(APP_ID, move |cx| {
        let proxy = olo_kit::proxy();
        instance::listen(APP_ID, proxy.clone());
        let server = server::Server::start(proxy);
        let _ = PREFS.set(Mutex::new(Prefs { settings: None, outputs: Vec::new(), outputs_in: false, holds: Vec::new(), power: None, server, windows: Vec::new(), history: Default::default() }));
        let restored = olo_kit::restored_state().and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()).filter(|o| !o.is_empty());
        {
            let mut p = prefs();
            match restored {
                Some(open) => {
                    for name in open {
                        if name.is_empty() {
                            p.open_launcher();
                        } else {
                            p.open(&name);
                        }
                    }
                }
                None => p.handle_args(&args),
            }
        }
        olo_kit::message_handler(cx, move |message, _| {
            let message = match message.downcast::<Launch>() {
                Ok(launch) => return prefs().handle_args(&launch.args),
                Err(m) => m,
            };
            if let Ok(msg) = message.downcast::<Msg>() {
                prefs().message(*msg);
            }
        });
        olo_kit::on_upgrade(|_| prefs().save_state());
    });
}
