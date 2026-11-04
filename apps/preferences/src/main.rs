//! Preferences: one small window per panel, as BeOS had a preference app
//! per area, all in one process. `olo-preferences <panel>` opens a panel
//! (Deskbar's Preferences menu does this); with no argument a launcher
//! window lists them all.
//!
//! Settings that live in app_server (appearance, keyboard, touchpad,
//! workspaces, displays) are read and written over its IPC: a watcher
//! thread keeps our copy current and a writer thread sends changes,
//! newest first, so dragging a slider never blocks the UI.

mod about;
mod ai;
mod airpods;
mod appearance;
mod backup;
mod bluetooth;
mod datetime;
mod default_apps;
mod display;
mod form;
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
    /// The displays have been heard of (a panel's window is sized to fit
    /// them: `fits`); panels asked for before then wait in `waiting`.
    outputs_known: bool,
    waiting: Vec<String>,
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

    /// The displays are known (or won't be): open the panels waiting.
    fn displays_known(&mut self, cx: &mut Context<Self>) {
        self.outputs_known = true;
        for name in std::mem::take(&mut self.waiting) {
            self.open(&name, cx);
        }
    }

    fn open(&mut self, name: &str, cx: &mut Context<Self>) {
        // Sized to the displays, so wait for app_server to say what they
        // are (milliseconds), or a moment at most (it isn't running).
        if !self.outputs_known {
            if self.waiting.is_empty() {
                cx.spawn(async |me, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
                    let _ = me.update(cx, |p, cx| p.displays_known(cx));
                })
                .detach();
            }
            self.waiting.push(name.to_owned());
            return;
        }
        let Some(panel) = Panel::from_arg(name) else {
            tracing::warn!("no preference panel called {name}");
            return;
        };
        // One window per panel: bring an open one forward.
        if let Some(w) = self.windows.iter().find(|w| w.panel == Some(panel)) {
            olo_ui::raise(Some(w.handle), cx);
            return;
        }
        let (w, h) = panel.size();
        let fit = fits(&self.outputs);
        // Too tall for the screen: as tall as fits, and wider by a scroll
        // bar, which it then has (panel::render).
        let (w, h) = if (h as f32) > fit { (w as f32 + olo_ui::widgets::SCROLL_BAR, fit) } else { (w as f32, h as f32) };
        let prefs = cx.entity();
        let opened = self.settings.clone().unwrap_or_default();
        let options = WindowOptions { fixed: true, ..WindowOptions::new(panel.title(), w, h).min_size(w, h).content_height(panel.size().1 as f32) };
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
            olo_ui::raise(Some(w.handle), cx);
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
            self.open_launcher(cx);
        }
        for p in &panels {
            self.open(p, cx);
        }
        if let (Some(name), Some(panel)) = (show, panels.last().and_then(|p| Panel::from_arg(p))) {
            // (Once the panel's window, opened after this update, has
            // drawn: what's lit is found in what it drew.)
            let me = cx.entity();
            cx.defer(move |cx| {
                let Some(handle) = me.read(cx).windows.iter().find(|w| w.panel == Some(panel)).map(|w| w.handle) else { return };
                let _ = handle.update(cx, |_, window, _| {
                    window.on_next_frame(move |window, cx| {
                        let handle = window.window_handle();
                        cx.defer(move |cx| {
                            olo_ui::spotlight::show(handle, tab.as_deref(), group.as_deref(), &name, cx);
                        });
                    });
                    // (A frame for it now: the window may have drawn already.)
                    window.refresh();
                });
            });
        }
        // A backup opened from Tracker: what it holds, ready to import
        // (once the panel's window, opened after this update, is there).
        // (Waiting for the panel made for it, or taken by the one there.)
        if let Some(file) = files.last() {
            backup::open_later(std::path::PathBuf::from(file.as_str()));
            self.open("backup", cx);
            let me = cx.entity();
            cx.defer(move |cx| {
                let view = me.read(cx).windows.iter().find(|w| w.panel == Some(Panel::Backup)).and_then(|w| w.view.clone());
                if let Some(view) = view {
                    view.update(cx, |v, cx| {
                        if let panel::State::Backup(b) = &mut v.state {
                            b.take_opened();
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
            Msg::Outputs(o) => {
                self.outputs = o;
                self.displays_known(cx);
            }
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

/// The tallest a panel's window may be: the shortest display's height
/// (in points, as it is turned) less room for the window's tab and the
/// edges, on the 2-point grid. A panel taller than that scrolls.
fn fits(outputs: &[olo_protocol::OutputInfo]) -> f32 {
    const ROOM: f32 = 64.0;
    outputs
        .iter()
        .filter(|o| o.mirror_of.is_none())
        .filter_map(|o| {
            let m = o.current_mode.as_ref()?;
            let tall = if o.rotation % 180 == 90 { m.width } else { m.height };
            Some(tall as f32 / o.scale.max(0.5) as f32)
        })
        .fold(f32::INFINITY, f32::min)
        .max(240.0)
        .sub_room(ROOM)
}

trait SubRoom {
    fn sub_room(self, room: f32) -> f32;
}

impl SubRoom for f32 {
    /// Less `room`, down to the 2-point grid (unbounded stays so).
    fn sub_room(self, room: f32) -> f32 {
        if self.is_finite() { ((self - room) / 2.0).floor() * 2.0 } else { self }
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
                            style::label(
                                window,
                                cx,
                                b.center().x,
                                icon.bottom() + px(12.0),
                                style::Align::Center,
                                panel.title(),
                                style::PLAIN_SIZE,
                                style::ui_font(FontWeight::NORMAL),
                                style::c(p.panel_text),
                            );
                            olo_ui::agent::record(
                                window,
                                cx,
                                olo_ui::agent::Node { role: "Button", name: Some(panel.title().into()), bounds: b, enabled: true, clickable: true, ..Default::default() },
                            );
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

/// `--export FILE [--sealed]`, `--inspect FILE` or `--import FILE`: a
/// backup without a window, for agents and scripts; the exit code, or
/// None for other flags. A sealed export (passwords and sign-ins with
/// it), and an import of one, read the backup's password from stdin.
fn backup_cli(flag: &str, file: &std::path::Path, sealed: bool) -> Option<i32> {
    use std::sync::atomic::AtomicBool;
    let no = AtomicBool::new(false);
    let show = |m: &backup::Manifest| {
        println!("made {}, {} files, {}", m.created, m.files.len(), backup::size_text(m.bytes()));
        for (app, files, bytes) in m.groups() {
            println!("  {app}: {files} {}, {}", if files == 1 { "file" } else { "files" }, backup::size_text(bytes));
        }
        if m.sealed.is_some() {
            println!("sealed, with {}", if m.secrets.is_empty() { "no passwords or sign-ins".to_string() } else { m.secrets.join(", ") });
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
            let how = if sealed { backup::Seal::Passphrase(passphrase()?) } else { backup::Seal::Open };
            backup::export(&backup::roots(), file, &how, &backup::Login, &backup::Origin::here(), &|_, _| {}, &no).map(|m| show(&m))
        })(),
        "--inspect" if !sealed => backup::inspect(file).map(|m| show(&m)),
        "--import" if !sealed => (|| {
            let words = if backup::inspect(file)?.needs_passphrase() { Some(passphrase()?) } else { None };
            backup::import(&backup::roots(), file, words.as_deref().map(|w| w.as_str()), &backup::Login, &backup::Origin::here(), &|_, _| {}, &no).map(|i| {
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, file, rest @ ..] = &args[..]
        && rest.iter().all(|a| a == "--sealed")
        && let Some(code) = backup_cli(flag, std::path::Path::new(file), !rest.is_empty())
    {
        std::process::exit(code);
    }
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_ui::run(APP_ID, move |cx| {
        instance::listen(APP_ID, olo_ui::proxy(cx));
        let server = server::Server::start(olo_ui::proxy(cx));
        let prefs = cx.new(|_| Prefs { settings: None, outputs: Vec::new(), power: None, server, windows: Vec::new(), outputs_known: false, waiting: Vec::new() });
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
