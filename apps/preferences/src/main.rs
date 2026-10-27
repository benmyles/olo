//! Preferences: one small window per panel, as BeOS had a preference app
//! per area, all in one process. `tabula-preferences <panel>` opens a panel
//! (Deskbar's Preferences menu does this); with no argument a launcher
//! window lists them all.
//!
//! Settings that live in app_server (appearance, keyboard, touchpad,
//! workspaces, displays) are read and written over its IPC: a watcher
//! thread keeps our copy current and a writer thread sends changes,
//! newest first, so dragging a slider never blocks the UI.

mod about;
mod ai;
mod appearance;
mod backup;
mod bluetooth;
mod datetime;
mod display;
mod keyboard;
mod network;
mod notifications;
mod panel;
mod power;
mod screensaver;
mod security;
mod server;
mod shortcuts;
mod sound;
mod touchid;
mod touchpad;
mod tracker;
mod workspaces;

use std::collections::BTreeMap;

use tabula_kit::{
    App, Cx, Menu, WindowId, WindowOptions,
    egui::{self, Rect, Sense, pos2, vec2},
    file_panel::{FilePanel, PANEL_SIZE, PanelResult},
    icons,
    instance::{self, Launch},
    metrics as m,
    style::{self, c},
};
use tabula_protocol::{OutputInfo, PowerStatus, Settings};

use panel::Panel;

const APP_ID: &str = "tabula.preferences";

pub enum Msg {
    Settings(Box<Settings>),
    Outputs(Vec<OutputInfo>),
    Power(PowerStatus),
    /// Something a panel shows changed (it reads it from the server).
    Redraw,
}

/// What a panel works with: the live settings and a way to change them.
pub struct Ctx<'a> {
    pub settings: &'a mut Settings,
    pub outputs: &'a [OutputInfo],
    /// app_server's power state (caffeinated, brightness), once known.
    pub power: Option<&'a PowerStatus>,
    /// The settings when the window opened, for Revert.
    pub opened: &'a Settings,
    pub server: &'a server::Server,
}

enum Win {
    Launcher,
    Panel { panel: Panel, opened: Box<Settings>, state: panel::State },
    /// Backgrounds' Choose… panel, for `owner`.
    Wallpaper { owner: WindowId, panel: Box<FilePanel> },
    /// Network's sheet for joining an enterprise network, over `owner`.
    Enterprise { owner: WindowId, form: Box<network::EnterpriseForm> },
    /// The sheet's file panel for a certificate or key.
    Cert { owner: WindowId, slot: network::CertSlot, panel: Box<FilePanel> },
    /// Backup's file panel, to export to or import from.
    Backup { owner: WindowId, ask: backup::Ask, panel: Box<FilePanel> },
}

struct Prefs {
    windows: BTreeMap<WindowId, Win>,
    settings: Option<Settings>,
    outputs: Vec<OutputInfo>,
    power: Option<PowerStatus>,
    server: server::Server,
}

impl Prefs {
    fn open(&mut self, cx: &mut Cx, name: &str) {
        let Some(panel) = Panel::from_arg(name) else {
            tracing::warn!("no preference panel called {name}");
            return;
        };
        // One window per panel: bring an open one forward.
        if let Some((&id, _)) =
            self.windows.iter().find(|(_, w)| matches!(w, Win::Panel { panel: p, .. } if *p == panel))
        {
            cx.activate(id);
            return;
        }
        let (w, h) = panel.size();
        let id = cx.open_window(WindowOptions::new(panel.title(), w, h).min_size(w, h).max_size(w, h));
        let opened = Box::new(self.settings.clone().unwrap_or_default());
        self.windows.insert(id, Win::Panel { panel, opened, state: panel::State::new(panel) });
    }

    fn open_launcher(&mut self, cx: &mut Cx) {
        if let Some((&id, _)) = self.windows.iter().find(|(_, w)| matches!(w, Win::Launcher)) {
            cx.activate(id);
            return;
        }
        let rows = Panel::ALL.len().div_ceil(LAUNCHER_COLUMNS) as f32;
        let w = 2.0 * m::WINDOW_INSET + LAUNCHER_COLUMNS as f32 * LAUNCHER_CELL.x;
        let h = 2.0 * m::WINDOW_INSET + rows * LAUNCHER_CELL.y;
        let id = cx.open_window(WindowOptions::new("Preferences", w as u32, h as u32).min_size(w as u32, h as u32));
        self.windows.insert(id, Win::Launcher);
    }

    /// Open (or bring forward) the wallpaper file panel, starting where the
    /// current picture is, else in the Pictures folder.
    fn choose_wallpaper(&mut self, cx: &mut Cx, owner: WindowId) {
        if let Some((&id, _)) = self.windows.iter().find(|(_, w)| matches!(w, Win::Wallpaper { .. })) {
            cx.activate(id);
            return;
        }
        let dir = self
            .settings
            .as_ref()
            .and_then(|s| s.wallpaper.as_deref())
            .and_then(|p| std::path::Path::new(p).parent().map(|d| d.to_owned()))
            .filter(|d| d.is_dir())
            .or_else(|| tabula_kit::user_dir("PICTURES"))
            .unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()));
        let (w, h) = PANEL_SIZE;
        let id = cx.open_window(WindowOptions::new("Choose a Wallpaper", w, h).min_size(400, 300).parent(owner));
        let panel = Box::new(FilePanel::open(dir).types(workspaces::WALLPAPER_TYPES));
        self.windows.insert(id, Win::Wallpaper { owner, panel });
    }

    /// The sheet for joining an enterprise network (one per network).
    fn join_enterprise(&mut self, cx: &mut Cx, owner: WindowId, device: String, net: tabula_system::network::Network) {
        if let Some((&id, _)) = self.windows.iter().find(|(_, w)| matches!(w, Win::Enterprise { form, .. } if form.ssid() == net.ssid)) {
            cx.activate(id);
            return;
        }
        let form = Box::new(network::EnterpriseForm::new(device, net));
        let (w, h) = (network::SHEET_WIDTH, form.height());
        let id = cx.open_window(WindowOptions::new(format!("Join “{}”", form.ssid()), w, h).min_size(w, 200).parent(owner));
        self.windows.insert(id, Win::Enterprise { owner, form });
    }

    /// Backup's file panel: where to export to, or what to import.
    fn backup_panel(&mut self, cx: &mut Cx, owner: WindowId, ask: backup::Ask) {
        if let Some((&id, _)) = self.windows.iter().find(|(_, w)| matches!(w, Win::Backup { .. })) {
            cx.activate(id);
            return;
        }
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let (title, panel) = match ask {
            backup::Ask::Export => ("Export Settings", FilePanel::save(home, backup::suggested_name()).types(&[backup::EXTENSION])),
            backup::Ask::Import => ("Import Settings", FilePanel::open(home).types(&[backup::EXTENSION])),
        };
        let (w, h) = PANEL_SIZE;
        let id = cx.open_window(WindowOptions::new(title, w, h).min_size(400, 300).parent(owner));
        self.windows.insert(id, Win::Backup { owner, ask, panel: Box::new(panel) });
    }

    fn handle_args(&mut self, cx: &mut Cx, args: &[String]) {
        let (files, panels): (Vec<&String>, Vec<&String>) =
            args.iter().filter(|a| !a.starts_with('-')).partition(|a| a.ends_with(&format!(".{}", backup::EXTENSION)));
        if panels.is_empty() && files.is_empty() {
            self.open_launcher(cx);
        }
        for p in panels {
            self.open(cx, p);
        }
        // A backup opened from Tracker: what it holds, ready to import.
        if let Some(file) = files.last() {
            self.open(cx, "backup");
            if let Some(Win::Panel { state: panel::State::Backup(b), .. }) =
                self.windows.values_mut().find(|w| matches!(w, Win::Panel { panel: Panel::Backup, .. }))
            {
                b.inspect(std::path::PathBuf::from(file.as_str()));
            }
        }
    }

    fn launcher_ui(&mut self, cx: &mut Cx, ui: &mut egui::Ui) {
        let p = style::palette(ui.ctx());
        let origin = ui.max_rect().min + vec2(m::WINDOW_INSET, m::WINDOW_INSET);
        let mut chosen = None;
        for (i, panel) in Panel::ALL.iter().enumerate() {
            let (col, row) = (i % LAUNCHER_COLUMNS, i / LAUNCHER_COLUMNS);
            let cell = Rect::from_min_size(
                origin + vec2(col as f32 * LAUNCHER_CELL.x, row as f32 * LAUNCHER_CELL.y),
                LAUNCHER_CELL,
            );
            let resp = ui.interact(cell, ui.make_persistent_id(("panel", i)), Sense::click());
            let icon = Rect::from_center_size(pos2(cell.center().x, cell.top() + 4.0 + 16.0), vec2(32.0, 32.0));
            if resp.hovered() {
                ui.painter().rect_filled(cell.shrink(2.0), 3.0, style::shade(&p, p.panel_background, 1.08));
            }
            icons::paint(ui.painter(), icon, panel.icon());
            style::label(
                ui.painter(),
                cell.center().x,
                icon.bottom() + 12.0,
                egui::Align::Center,
                panel.title(),
                style::plain_font(),
                c(p.panel_text),
            );
            if resp.clicked() {
                chosen = Some(*panel);
            }
        }
        if let Some(panel) = chosen {
            self.open(cx, panel.arg());
        }
    }
}

/// `--export FILE`, `--inspect FILE` or `--import FILE`: a backup without
/// a window, for agents and scripts; the exit code, or None for other flags.
fn backup_cli(flag: &str, file: &std::path::Path) -> Option<i32> {
    use std::sync::atomic::AtomicBool;
    let no = AtomicBool::new(false);
    let show = |m: &backup::Manifest| {
        println!("made {}, {} files, {}", m.created, m.files.len(), backup::size_text(m.bytes()));
        for (app, files, bytes) in m.groups() {
            println!("  {app}: {files} {}, {}", if files == 1 { "file" } else { "files" }, backup::size_text(bytes));
        }
    };
    let r = match flag {
        "--export" => backup::export(&backup::roots(), file, &|_, _| {}, &no).map(|m| show(&m)),
        "--inspect" => backup::inspect(file).map(|m| show(&m)),
        "--import" => backup::import(&backup::roots(), file, &|_, _| {}, &no).map(|i| {
            show(&i.manifest);
            println!("imported {} files", i.files);
            if let Some(p) = &i.before {
                println!("the settings as they were: {}", p.display());
            }
            // app_server keeps its own copy: hand it the backup's.
            if let Some(settings) = i.settings
                && let Ok(mut c) = tabula_protocol::Client::connect()
            {
                let _ = c.request(&tabula_protocol::Request::SetSettings { settings });
            }
        }),
        _ => return None,
    };
    Some(match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("tabula-preferences: {e}");
            1
        }
    })
}

const LAUNCHER_COLUMNS: usize = 5;
const LAUNCHER_CELL: egui::Vec2 = egui::Vec2::new(104.0, 64.0);

impl App for Prefs {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui) {
        if matches!(self.windows.get(&window), Some(Win::Launcher)) {
            self.launcher_ui(cx, ui);
            return;
        }
        if let Some(Win::Enterprise { form, .. }) = self.windows.get_mut(&window) {
            let before = form.height();
            let result = form.ui(ui);
            if form.height() != before {
                cx.resize(window, (network::SHEET_WIDTH, form.height()));
            }
            match result {
                Some(network::SheetResult::Close) => self.close_requested(cx, window),
                Some(network::SheetResult::Choose(slot)) => {
                    let (w, h) = PANEL_SIZE;
                    let id = cx.open_window(WindowOptions::new(slot.title(), w, h).min_size(400, 300).parent(window));
                    let dir = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
                    self.windows.insert(id, Win::Cert { owner: window, slot, panel: Box::new(FilePanel::open(dir).types(network::CERT_TYPES)) });
                }
                None => {}
            }
            return;
        }
        if let Some(Win::Cert { owner, slot, panel }) = self.windows.get_mut(&window) {
            let (owner, slot) = (*owner, *slot);
            if let Some(result) = panel.ui(ui) {
                if let (PanelResult::Chosen(path), Some(Win::Enterprise { form, .. })) = (result, self.windows.get_mut(&owner)) {
                    form.chosen(slot, path);
                }
                self.windows.remove(&window);
                cx.close_window(window);
                cx.redraw(owner);
            }
            return;
        }
        if let Some(Win::Backup { owner, ask, panel }) = self.windows.get_mut(&window) {
            let (owner, ask) = (*owner, *ask);
            if let Some(result) = panel.ui(ui) {
                if let (PanelResult::Chosen(path), Some(Win::Panel { state: panel::State::Backup(b), .. })) = (result, self.windows.get_mut(&owner)) {
                    match ask {
                        backup::Ask::Export => b.export_to(path),
                        backup::Ask::Import => b.inspect(path),
                    }
                }
                self.windows.remove(&window);
                cx.close_window(window);
                cx.activate(owner);
                cx.redraw(owner);
            }
            return;
        }
        if let Some(Win::Wallpaper { owner, panel }) = self.windows.get_mut(&window) {
            let owner = *owner;
            if let Some(result) = panel.ui(ui) {
                if let (PanelResult::Chosen(path), Some(s)) = (result, self.settings.as_mut()) {
                    s.wallpaper = Some(path.to_string_lossy().into_owned());
                    self.server.set_settings(s.clone());
                }
                self.windows.remove(&window);
                cx.close_window(window);
                cx.redraw(owner);
            }
            return;
        }
        let Some(Win::Panel { panel, opened, state }) = self.windows.get_mut(&window) else {
            return;
        };
        let Some(settings) = self.settings.as_mut() else {
            // Not connected (yet): panels that need app_server wait.
            if panel.needs_server() {
                let p = style::palette(ui.ctx());
                style::label(
                    ui.painter(),
                    ui.max_rect().center().x,
                    ui.max_rect().center().y,
                    egui::Align::Center,
                    "Waiting for app_server…",
                    style::plain_font(),
                    style::disabled_text(&p),
                );
                return;
            }
            let mut scratch = Settings::default();
            let ctx = Ctx { settings: &mut scratch, outputs: &self.outputs, power: self.power.as_ref(), opened, server: &self.server };
            panel::ui(*panel, state, ui, ctx);
            return;
        };
        let before = settings.clone();
        let ctx = Ctx { settings, outputs: &self.outputs, power: self.power.as_ref(), opened, server: &self.server };
        panel::ui(*panel, state, ui, ctx);
        if *settings != before {
            self.server.set_settings(settings.clone());
        }
        let enterprise = match state {
            panel::State::Network(n) => n.enterprise.take(),
            _ => None,
        };
        let ask = match state {
            panel::State::Backup(b) => b.ask.take(),
            _ => None,
        };
        if let panel::State::Backgrounds(b) = state
            && std::mem::take(&mut b.choose)
        {
            self.choose_wallpaper(cx, window);
        }
        if let Some((device, net)) = enterprise {
            self.join_enterprise(cx, window, device, net);
        }
        if let Some(ask) = ask {
            self.backup_panel(cx, window, ask);
        }
    }

    fn menu_bar(&mut self, _window: WindowId) -> Option<Vec<Menu>> {
        None
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        self.windows.remove(&window);
        cx.close_window(window);
        // A file panel goes with the window it was choosing for.
        let orphans: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|(_, w)| matches!(w, Win::Wallpaper { owner, .. } | Win::Enterprise { owner, .. } | Win::Cert { owner, .. } | Win::Backup { owner, .. } if *owner == window))
            .map(|(&id, _)| id)
            .collect();
        for id in orphans {
            // Their own sheets and panels go too.
            self.close_requested(cx, id);
        }
        if self.windows.is_empty() {
            cx.quit();
        }
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        let message = match message.downcast::<Launch>() {
            Ok(launch) => return self.handle_args(cx, &launch.args),
            Err(m) => m,
        };
        let Ok(msg) = message.downcast::<Msg>() else {
            return;
        };
        match *msg {
            Msg::Settings(s) => {
                // The first copy is also what Revert goes back to.
                if self.settings.is_none() {
                    for w in self.windows.values_mut() {
                        if let Win::Panel { opened, .. } = w {
                            *opened = s.clone();
                        }
                    }
                }
                self.settings = Some(*s);
            }
            Msg::Outputs(o) => self.outputs = o,
            Msg::Power(p) => self.power = Some(p),
            Msg::Redraw => {}
        }
        for &id in self.windows.keys() {
            cx.redraw(id);
        }
    }

    fn save_state(&mut self, _cx: &Cx) -> Option<String> {
        let open: Vec<&str> = self
            .windows
            .values()
            .filter_map(|w| match w {
                Win::Launcher => Some(""),
                Win::Panel { panel, .. } => Some(panel.arg()),
                Win::Wallpaper { .. } | Win::Enterprise { .. } | Win::Cert { .. } | Win::Backup { .. } => None,
            })
            .collect();
        serde_json::to_string(&open).ok()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, file] = &args[..]
        && let Some(code) = backup_cli(flag, std::path::Path::new(file))
    {
        std::process::exit(code);
    }
    if instance::forward(APP_ID, &args) {
        return;
    }
    tabula_kit::run(APP_ID, |cx| {
        instance::listen(APP_ID, cx.proxy());
        let server = server::Server::start(cx.proxy());
        let mut prefs = Prefs { windows: BTreeMap::new(), settings: None, outputs: Vec::new(), power: None, server };
        match cx.restored_state().and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()) {
            Some(open) if !open.is_empty() => {
                for name in open {
                    if name.is_empty() {
                        prefs.open_launcher(cx);
                    } else {
                        prefs.open(cx, &name);
                    }
                }
            }
            _ => prefs.handle_args(cx, &args),
        }
        prefs
    });
}
