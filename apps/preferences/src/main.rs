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
mod appearance;
mod display;
mod keyboard;
mod panel;
mod server;
mod sound;
mod touchpad;
mod workspaces;

use std::collections::BTreeMap;

use tabula_kit::{
    App, Cx, Menu, WindowId, WindowOptions,
    egui::{self, Rect, Sense, pos2, vec2},
    icons,
    instance::{self, Launch},
    metrics as m,
    style::{self, c},
};
use tabula_protocol::{OutputInfo, Settings};

use panel::Panel;

const APP_ID: &str = "tabula.preferences";

pub enum Msg {
    Settings(Box<Settings>),
    Outputs(Vec<OutputInfo>),
}

/// What a panel works with: the live settings and a way to change them.
pub struct Ctx<'a> {
    pub settings: &'a mut Settings,
    pub outputs: &'a [OutputInfo],
    /// The settings when the window opened, for Revert.
    pub opened: &'a Settings,
    pub server: &'a server::Server,
}

enum Win {
    Launcher,
    Panel { panel: Panel, opened: Box<Settings>, state: panel::State },
}

struct Prefs {
    windows: BTreeMap<WindowId, Win>,
    settings: Option<Settings>,
    outputs: Vec<OutputInfo>,
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

    fn handle_args(&mut self, cx: &mut Cx, args: &[String]) {
        let panels: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
        if panels.is_empty() {
            self.open_launcher(cx);
        }
        for p in panels {
            self.open(cx, p);
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

const LAUNCHER_COLUMNS: usize = 5;
const LAUNCHER_CELL: egui::Vec2 = egui::Vec2::new(104.0, 64.0);

impl App for Prefs {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui) {
        if matches!(self.windows.get(&window), Some(Win::Launcher)) {
            self.launcher_ui(cx, ui);
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
            let ctx = Ctx { settings: &mut scratch, outputs: &self.outputs, opened, server: &self.server };
            panel::ui(*panel, state, ui, ctx);
            return;
        };
        let before = settings.clone();
        let ctx = Ctx { settings, outputs: &self.outputs, opened, server: &self.server };
        panel::ui(*panel, state, ui, ctx);
        if *settings != before {
            self.server.set_settings(settings.clone());
        }
        let _ = cx;
    }

    fn menu_bar(&mut self, _window: WindowId) -> Option<Vec<Menu>> {
        None
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        self.windows.remove(&window);
        cx.close_window(window);
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
        }
        for &id in self.windows.keys() {
            cx.redraw(id);
        }
    }

    fn save_state(&mut self, _cx: &Cx) -> Option<String> {
        let open: Vec<&str> = self
            .windows
            .values()
            .map(|w| match w {
                Win::Launcher => "",
                Win::Panel { panel, .. } => panel.arg(),
            })
            .collect();
        serde_json::to_string(&open).ok()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    tabula_kit::run(APP_ID, |cx| {
        instance::listen(APP_ID, cx.proxy());
        let server = server::Server::start(cx.proxy());
        let mut prefs = Prefs { windows: BTreeMap::new(), settings: None, outputs: Vec::new(), server };
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
