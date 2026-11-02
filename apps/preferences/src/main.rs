//! Preferences: one small window per panel, as BeOS had a preference app
//! per area, all in one process. `olo-preferences <panel>` opens a panel
//! (Deskbar's Preferences menu does this); with no argument a launcher
//! window lists them all.
//!
//! Settings that live in app_server (appearance, keyboard, touchpad,
//! workspaces, displays) are read and written over its IPC: a watcher
//! thread keeps our copy current and a writer thread sends changes,
//! newest first, so dragging a slider never blocks the UI.

#[cfg(test)]
mod lint;
mod about;
mod ai;
mod appearance;
mod backup;
mod bluetooth;
mod datetime;
mod display;
mod form;
mod integrations;
mod keyboard;
mod network;
mod notifications;
mod panel;
mod performance;
mod power;
mod screensaver;
mod security;
mod server;
mod shortcuts;
mod sound;
mod telegram;
mod terminal;
mod themes;
mod touchid;
mod touchpad;
mod tracker;
mod windows;
mod workspaces;

use olo_protocol::{OutputInfo, PowerStatus, Settings};
use olo_ui::{
    WindowOptions,
    gpui::{
        AnyWindowHandle, AppContext as _, Bounds, Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Pixels, Render, SharedString, Styled as _,
        Window, canvas, div, point, px, size,
    },
    icons,
    instance::{self, Launch},
    metrics as m, style, theme,
};

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
    /// app_server's power state (caffeinated, brightness), once known.
    pub power: Option<PowerStatus>,
    pub server: server::Server,
    windows: Vec<Shown>,
}

/// A window open: a panel's (and its view), or the launcher.
struct Shown {
    panel: Option<Panel>,
    handle: AnyWindowHandle,
    view: Option<Entity<PanelView>>,
}

impl Prefs {
    /// Changes the settings: app_server is sent them, and every window
    /// showing them is drawn again. (Not connected yet: nothing to change.)
    pub fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Settings)) {
        let Some(s) = self.settings.as_mut() else { return };
        let before = s.clone();
        f(s);
        if *s != before {
            self.server.set_settings(s.clone());
            cx.notify();
        }
    }

    fn open(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(panel) = Panel::from_arg(name) else {
            tracing::warn!("no preference panel called {name}");
            return;
        };
        // One window per panel: bring an open one forward.
        if let Some(w) = self.windows.iter().find(|w| w.panel == Some(panel)) {
            let _ = w.handle.update(cx, |_, window, _| window.activate_window());
            return;
        }
        let (w, h) = panel.size();
        let (w, h) = (w as f32, h as f32);
        let prefs = cx.entity();
        let opened = self.settings.clone().unwrap_or_default();
        let options = WindowOptions { fixed: true, ..WindowOptions::new(panel.title(), w, h).min_size(w, h) };
        // (Once this update is over: the window draws as it opens, and its
        // panel reads the settings.)
        cx.defer(move |cx| {
            let made = std::rc::Rc::new(std::cell::RefCell::new(None));
            let out = made.clone();
            let p = prefs.clone();
            let Some(handle) = olo_ui::open_window(cx, options, move |window, cx| {
                let view = cx.new(|cx| PanelView::new(p, panel, opened, window, cx));
                *out.borrow_mut() = Some(view.clone());
                view
            }) else {
                return;
            };
            let view = made.borrow_mut().take();
            prefs.update(cx, |p, cx| p.register(Some(panel), handle.into(), view, cx));
        });
    }

    fn open_launcher(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.windows.iter().find(|w| w.panel.is_none()) {
            let _ = w.handle.update(cx, |_, window, _| window.activate_window());
            return;
        }
        let rows = Panel::ALL.len().div_ceil(LAUNCHER_COLUMNS) as f32;
        let w = 2.0 * m::WINDOW_INSET + LAUNCHER_COLUMNS as f32 * LAUNCHER_CELL.0;
        let h = 2.0 * m::WINDOW_INSET + rows * LAUNCHER_CELL.1;
        let prefs = cx.entity();
        let options = WindowOptions { fixed: true, ..WindowOptions::new("Preferences", w, h).min_size(w, h).floating() };
        cx.defer(move |cx| {
            let p = prefs.clone();
            let Some(handle) = olo_ui::open_window(cx, options, move |_, cx| cx.new(|_| Launcher { prefs: p })) else { return };
            prefs.update(cx, |p, cx| p.register(None, handle.into(), None, cx));
        });
    }

    /// Follows the window: when it closes it leaves the list, and the
    /// app ends with the last.
    fn register(&mut self, panel: Option<Panel>, handle: AnyWindowHandle, view: Option<Entity<PanelView>>, cx: &mut Context<Self>) {
        let me = cx.weak_entity();
        let _ = handle.update(cx, |_, window, cx| {
            window.on_window_should_close(cx, move |_, cx| {
                if let Some(p) = me.upgrade() {
                    p.update(cx, |p, cx| {
                        p.windows.retain(|w| w.handle != handle);
                        if p.windows.is_empty() {
                            cx.quit();
                        }
                    });
                }
                true
            })
        });
        self.windows.push(Shown { panel, handle, view });
    }

    fn handle_args(&mut self, args: &[String], cx: &mut Context<Self>) {
        let (files, panels): (Vec<&String>, Vec<&String>) =
            args.iter().filter(|a| !a.starts_with('-')).partition(|a| [backup::EXTENSION, backup::OLD_EXTENSION].iter().any(|x| a.ends_with(&format!(".{x}"))));
        if panels.is_empty() && files.is_empty() {
            self.open_launcher(cx);
        }
        for p in panels {
            self.open(p, cx);
        }
        // A backup opened from Tracker: what it holds, ready to import
        // (once the panel's window, opened after this update, is there).
        if let Some(file) = files.last() {
            self.open("backup", cx);
            let path = std::path::PathBuf::from(file.as_str());
            let me = cx.entity();
            cx.defer(move |cx| {
                let view = me.read(cx).windows.iter().find(|w| w.panel == Some(Panel::Backup)).and_then(|w| w.view.clone());
                if let Some(view) = view {
                    view.update(cx, |v, cx| {
                        if let panel::State::Backup(b) = &mut v.state {
                            b.inspect(path);
                        }
                        cx.notify();
                    });
                }
            });
        }
    }

    fn message(&mut self, msg: Msg, cx: &mut Context<Self>) {
        match msg {
            Msg::Settings(s) => {
                // The first copy is also what Revert goes back to.
                if self.settings.is_none() {
                    for w in &self.windows {
                        if let Some(view) = &w.view {
                            view.update(cx, |v, _| v.opened = (*s).clone());
                        }
                    }
                }
                self.settings = Some(*s);
            }
            Msg::Outputs(o) => self.outputs = o,
            Msg::Power(p) => self.power = Some(p),
            Msg::Redraw => {}
        }
        cx.notify();
    }

    fn save_state(&self) -> Option<String> {
        let open: Vec<&str> = self.windows.iter().map(|w| w.panel.map_or("", Panel::arg)).collect();
        serde_json::to_string(&open).ok()
    }
}

/// A panel's window: the panel, the settings as they were when it opened
/// (for Revert), and what it keeps of its own.
pub struct PanelView {
    pub prefs: Entity<Prefs>,
    pub panel: Panel,
    pub opened: Settings,
    pub state: panel::State,
    pub fields: Fields,
}

impl PanelView {
    fn new(prefs: Entity<Prefs>, panel: Panel, opened: Settings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&prefs, |_, _, cx| cx.notify()).detach();
        let state = panel::State::new(panel, window, cx);
        PanelView { prefs, panel, opened, state, fields: Fields::default() }
    }
}

impl Render for PanelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        panel::render(self, window, cx)
    }
}

const LAUNCHER_COLUMNS: usize = 5;
const LAUNCHER_CELL: (f32, f32) = (104.0, 64.0);

/// The launcher: every panel's icon and name; a click opens it.
struct Launcher {
    prefs: Entity<Prefs>,
}

impl Render for Launcher {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut grid = div().relative().size_full();
        for (i, panel) in Panel::ALL.iter().copied().enumerate() {
            let (col, row) = (i % LAUNCHER_COLUMNS, i / LAUNCHER_COLUMNS);
            let (x, y) = (m::WINDOW_INSET + col as f32 * LAUNCHER_CELL.0, m::WINDOW_INSET + row as f32 * LAUNCHER_CELL.1);
            let prefs = self.prefs.clone();
            let cell = div()
                .id(SharedString::from(panel.arg()))
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(LAUNCHER_CELL.0))
                .h(px(LAUNCHER_CELL.1))
                .child(
                    canvas(
                        |_, _, _| {},
                        move |b: Bounds<Pixels>, _, window, cx| {
                            let p = theme::palette(cx);
                            let hovered = b.contains(&window.mouse_position());
                            if hovered {
                                let shade: Hsla = style::shade(&p, p.panel_background, 1.08);
                                window.paint_quad(olo_ui::gpui::fill(style::inset(window, b, 2.0), shade).corner_radii(px(3.0)));
                            }
                            let icon = Bounds { origin: point(b.center().x - px(16.0), b.top() + px(4.0)), size: size(px(32.0), px(32.0)) };
                            icons::paint(window, icon, panel.icon());
                            style::label(window, cx, b.center().x, icon.bottom() + px(12.0), style::Align::Center, panel.title(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::c(p.panel_text));
                            olo_ui::agent::record(window, cx, olo_ui::agent::Node { role: "Button", name: Some(panel.title().into()), bounds: b, enabled: true, clickable: true, ..Default::default() });
                        },
                    )
                    .size_full(),
                )
                .on_mouse_move(|_, window, _| window.refresh())
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    prefs.update(cx, |p, cx| p.open(panel.arg(), cx));
                });
            grid = grid.child(cell);
        }
        grid
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
                && let Ok(mut c) = olo_protocol::Client::connect()
            {
                let _ = c.request(&olo_protocol::Request::SetSettings { settings });
            }
        }),
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, file] = &args[..]
        && let Some(code) = backup_cli(flag, std::path::Path::new(file))
    {
        std::process::exit(code);
    }
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_ui::run(APP_ID, move |cx| {
        instance::listen(APP_ID, olo_ui::proxy(cx));
        let server = server::Server::start(olo_ui::proxy(cx));
        let prefs = cx.new(|_| Prefs { settings: None, outputs: Vec::new(), power: None, server, windows: Vec::new() });
        let restored = olo_ui::restored_state().and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()).filter(|o| !o.is_empty());
        prefs.update(cx, |p, cx| match restored {
            Some(open) => {
                for name in open {
                    if name.is_empty() {
                        p.open_launcher(cx);
                    } else {
                        p.open(&name, cx);
                    }
                }
            }
            None => p.handle_args(&args, cx),
        });
        let messages = prefs.clone();
        olo_ui::message_handler(cx, move |message, cx| {
            let message = match message.downcast::<Launch>() {
                Ok(launch) => return messages.update(cx, |p, cx| p.handle_args(&launch.args, cx)),
                Err(m) => m,
            };
            if let Ok(msg) = message.downcast::<Msg>() {
                messages.update(cx, |p, cx| p.message(*msg, cx));
            }
        });
        let saving = prefs.clone();
        olo_ui::on_upgrade(cx, move |cx| saving.read(cx).save_state());
    });
}
