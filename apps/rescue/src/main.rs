//! olo-rescue: Olo Rescue's screen (`olo_protocol::rescue`; app_server's
//! rescue.rs starts it when Ctrl+Shift+Super+Delete is pressed and ends
//! it as Rescue does). It asks rtkit for Rescue's priority before it
//! starts a thread, so everything it runs (its shell too) is above
//! whatever has taken the computer, and depends on nothing of the
//! person's session but app_server: no Deskbar, no olo-termd, no
//! settings of its own.
//!
//! It covers each display with a layer surface of its own (under the
//! windows, so what it opens, Preferences, shows over it): on the main
//! one (the built-in display, else the first), the terminal, the
//! processes to end, Preferences' panels, the displays' settings put
//! back to Olo's, and Leave Rescue; on the others, where it is and Show
//! Rescue Here.

mod procs;
mod term;

use std::{collections::HashMap, rc::Rc};

use olo_protocol::{Client, Event, EventKind, OutputInfo, Reply, Request};
use olo_ui::{
    Frame, Proxy, WindowOptions,
    gpui::{self, AppContext as _, Context, Entity, IntoElement, KeyBinding, LayerKeyboard, LayerShellLayer, NoAction, ParentElement as _, Render, Styled as _, Window, WindowHandle, canvas, div, px},
    icons, metrics as m, style, theme,
    widgets::{BBox, Button, Column, ColumnListView, ListModel, StringView, heading, open_alert},
};

const TITLE: &str = "Olo Rescue";
/// The processes' part, under the terminal (ten rows or so).
const PROCESSES_HEIGHT: f32 = 300.0;

/// What the connection to app_server brings.
enum Server {
    Outputs(Vec<OutputInfo>),
    /// Rescue's over (or app_server has gone): quit.
    Over,
}

struct Rescue {
    proxy: Proxy,
    terminal: Entity<term::Terminal>,
    processes: Vec<procs::Proc>,
    process_list: Entity<ListModel>,
    panels: Entity<ListModel>,
    outputs: Vec<OutputInfo>,
    /// The display Rescue shows on (None: the main one).
    here: Option<String>,
    windows: HashMap<String, WindowHandle<Frame>>,
    /// What the last thing done came to: its words, and whether it went wrong.
    note: Option<(String, bool)>,
}

/// One display's surface.
struct Screen {
    app: Entity<Rescue>,
    output: String,
}

impl Rescue {
    /// The display Rescue shows on.
    fn main_display(&self) -> Option<String> {
        let named = self.here.as_ref().filter(|h| self.outputs.iter().any(|o| &o.name == *h));
        named.cloned().or_else(|| self.outputs.iter().find(|o| o.main).or(self.outputs.first()).map(|o| o.name.clone()))
    }

    /// A surface on each display (each display's as it comes and goes).
    fn cover(&mut self, me: &Entity<Rescue>, cx: &mut gpui::App) {
        let names: Vec<String> = self.outputs.iter().filter(|o| o.mirror_of.is_none()).map(|o| o.name.clone()).collect();
        self.windows.retain(|name, w| {
            let keep = names.contains(name);
            if !keep {
                let _ = w.update(cx, |_, window, _| window.remove_window());
            }
            keep
        });
        for name in names {
            if self.windows.contains_key(&name) {
                continue;
            }
            let options = WindowOptions::new(TITLE, 1.0, 1.0).layer(olo_protocol::rescue::NAMESPACE, LayerShellLayer::Bottom, (true, true, true, true)).layer_options(|l| {
                // All of it, Deskbar's room too (it's put away).
                l.exclusive_zone = -1;
                l.output = Some(name.clone());
            });
            let (app, output) = (me.clone(), name.clone());
            let terminal = self.terminal.clone();
            let main = self.main_display().as_deref() == Some(name.as_str());
            // Only Rescue's own takes the keyboard (app_server gives it
            // it as it shows): the others' button is for the pointer.
            let options = options.layer_options(|l| l.keyboard = if main { LayerKeyboard::OnDemand } else { LayerKeyboard::None });
            if let Some(w) = olo_ui::open_window(cx, options, move |window, cx| {
                if main {
                    window.focus(&terminal.read(cx).focus);
                }
                cx.new(|cx| {
                    cx.observe(&app, |_, _, cx| cx.notify()).detach();
                    Screen { app, output }
                })
            }) {
                self.windows.insert(name, w);
            }
        }
    }

    fn server(&mut self, message: Server, me: &Entity<Rescue>, cx: &mut Context<Self>) {
        match message {
            Server::Outputs(outputs) => {
                self.outputs = outputs;
                self.cover(me, cx);
                cx.notify();
            }
            Server::Over => cx.quit(),
        }
    }

    /// Rescue over: app_server puts everything back, and ends us.
    fn leave(&mut self) {
        ask(Request::Rescue { on: false });
    }

    fn show_here(&mut self, output: String, me: &Entity<Rescue>, cx: &mut Context<Self>) {
        self.here = Some(output);
        // (Each surface drawn again for what it is now.)
        for (_, w) in self.windows.drain() {
            let _ = w.update(cx, |_, window, _| window.remove_window());
        }
        self.cover(me, cx);
        cx.notify();
    }

    fn open_panel(&mut self, arg: Option<&str>, cx: &mut Context<Self>) {
        let args: Vec<&str> = arg.into_iter().collect();
        self.note = Some(match start("olo-preferences", &args) {
            Ok(()) => (format!("Preferences opens{}; what you change there applies when you leave Rescue.", arg.map(|a| format!(" ({a})")).unwrap_or_default()), false),
            Err(e) => (format!("Preferences didn't open: {e}"), true),
        });
        cx.notify();
    }

    /// The displays' saved settings put back to Olo's (brightness kept),
    /// for when Rescue ends.
    fn reset_displays(&mut self, cx: &mut Context<Self>) {
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let mut c = Client::connect().map_err(|e| e.to_string())?;
                let Reply::Settings { mut settings } = c.request(&Request::GetSettings).map_err(|e| e.to_string())? else { return Err("app_server didn't say".to_owned()) };
                let brightness: Vec<(String, f32)> = settings.display.outputs.iter().filter_map(|(k, c)| Some((k.clone(), c.brightness?))).collect();
                settings.display = Default::default();
                for (k, b) in brightness {
                    settings.display.outputs.entry(k).or_default().brightness = Some(b);
                }
                c.request(&Request::SetSettings { settings }).map_err(|e| e.to_string())?;
                Ok(())
            })();
            proxy.send(Done(match result {
                Ok(()) => ("The displays' settings are Olo's own again, from when you leave Rescue.".into(), false),
                Err(e) => (format!("The displays' settings weren't reset: {e}"), true),
            }));
        });
        self.note = Some(("Resetting the displays' settings…".into(), false));
        cx.notify();
    }

    fn end_selected(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(p) = self.process_list.read(cx).state.single().and_then(|i| self.processes.get(i)).cloned() else { return };
        self.note = Some(match procs::end(p.pid, force) {
            Ok(()) if force => (format!("{} ({}) was made to quit.", p.name, p.pid), false),
            Ok(()) => (format!("{} ({}) was asked to quit.", p.name, p.pid), false),
            Err(e) => (format!("{} ({}) wasn't ended: {e}", p.name, p.pid), true),
        });
        cx.notify();
    }
}

/// What a thread did, for the note.
struct Done((String, bool));

/// A request to app_server on a thread of its own (the UI never waits).
fn ask(req: Request) {
    std::thread::spawn(move || {
        if let Err(e) = Client::connect().and_then(|mut c| c.request(&req)) {
            tracing::warn!(?req, "app_server: {e}");
        }
    });
}

/// One of Olo's programs, started on its own.
fn start(program: &str, args: &[&str]) -> Result<(), String> {
    let path = olo_xdg::find_program(program).ok_or_else(|| format!("{program} isn't installed"))?;
    let mut child = std::process::Command::new(path).args(args).spawn().map_err(|e| e.to_string())?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

impl Render for Screen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.read(cx);
        if app.main_display().as_deref() != Some(self.output.as_str()) {
            return self.elsewhere(cx).into_any_element();
        }
        self.rescue(cx).into_any_element()
    }
}

/// An icon, `size` points, on whole device pixels.
fn icon(name: &'static str, size: f32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            icons::paint(window, style::snap_centred(window, b.center(), size, size), name);
        },
    )
    .size(px(size))
    .flex_shrink_0()
}

impl Screen {
    /// Another display's: where Rescue is, and Show Rescue Here.
    fn elsewhere(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.read(cx);
        let there = app.main_display().and_then(|n| app.outputs.iter().find(|o| o.name == n)).map_or_else(
            || "another display".to_owned(),
            // (Its number too, among displays of one name.)
            |o| match o.number.filter(|_| app.outputs.iter().filter(|x| x.display_name() == o.display_name()).count() > 1) {
                Some(n) => format!("{} ({})", o.display_name(), olo_protocol::display_label(n)),
                None => o.display_name(),
            },
        );
        let (me, output) = (self.app.clone(), self.output.clone());
        div().size_full().flex().items_center().justify_center().child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(m::SPACING))
                .child(icon("warning", 32.0))
                .child(StringView::new(TITLE).bold(true))
                .child(StringView::new(format!("Rescue is on {there}.")).dim(true))
                .child(Button::new("Show Rescue Here").id("show-here").on_click(move |_, cx| {
                    let output = output.clone();
                    me.update(cx, |r, cx| {
                        let me = cx.entity();
                        r.show_here(output, &me, cx);
                    });
                })),
        )
    }

    fn rescue(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        let me = self.app.clone();
        let app = self.app.read(cx);
        let leave = me.clone();
        let header = div()
            .flex()
            .items_center()
            .gap(px(m::SPACING + 2.0))
            .child(icon("warning", 32.0))
            .child(
                div().flex_1().min_w_0().flex().flex_col().child(StringView::new(TITLE).bold(true).height(m::LABEL_ROW + 2.0)).child(
                    StringView::new(format!(
                        "Your windows are put away and the displays, keyboard and pointer are on safe settings until you leave. {} brings Rescue back.",
                        olo_protocol::rescue::KEYS
                    ))
                    .dim(true)
                    .wrap(true),
                ),
            )
            .child(Button::new("Leave Rescue").id("leave").default(true).on_click(move |_, cx| leave.update(cx, |r, _| r.leave())));
        let note = app.note.clone().map(|(text, bad)| {
            let v = StringView::new(text).wrap(true);
            if bad { v.color(style::c(p.failure)) } else { v.dim(true) }
        });

        // The left: the displays, Preferences, the desktop.
        let reset = me.clone();
        let displays = BBox::new("Displays")
            .child(StringView::new("Each display is at its own best resolution and scale now: no rotation, mirroring, HDR or VRR, and night light off. Yours come back as you leave.").wrap(true))
            .child(div().flex().child(Button::new("Reset Display Settings…").id("reset-displays").on_click(move |_, cx| {
                let reset = reset.clone();
                open_alert(
                    cx,
                    "Reset Display Settings",
                    "warning",
                    "Every display goes back to Olo's own resolution, scale, arrangement and colours when you leave Rescue. Its brightness stays as it is.",
                    &["Cancel", "Reset"],
                    move |choice, cx| {
                        if choice == Some(1) {
                            reset.update(cx, |r, cx| r.reset_displays(cx));
                        }
                    },
                );
            })));
        let panels = olo_xdg::apps::PREFERENCE_PANELS;
        let (cell_panels, open_me) = (panels, me.clone());
        let panel_list = ColumnListView::new("rescue-panels", &app.panels, vec![Column::new("Panel", 240.0)], panels.len(), move |cell, window, cx| {
            if let Some((_, name, icon)) = cell_panels.get(cell.row) {
                cell.icon_text(window, cx, icon, "preferences", *name);
            }
        })
        .no_header()
        .type_select(move |row| panels.get(row).map_or("", |p| p.1).into())
        .on_open(move |row, _, cx| {
            let arg = panels.get(row).map(|p| p.0);
            open_me.update(cx, |r, cx| r.open_panel(arg, cx));
        });
        let (open_one, open_all) = (me.clone(), me.clone());
        let chosen = app.panels.read(cx).state.single().and_then(|i| panels.get(i)).map(|p| p.0);
        let preferences = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .child(heading("Preferences"))
            .child(StringView::new("What you change applies when you leave Rescue.").dim(true).wrap(true))
            .child(div().flex_1().min_h_0().child(panel_list))
            .child(
                div()
                    .flex()
                    .gap(px(m::SPACING))
                    .child(Button::new("Open").id("open-panel").enabled(chosen.is_some()).on_click(move |_, cx| open_one.update(cx, |r, cx| r.open_panel(chosen, cx))))
                    .child(Button::new("All Preferences").id("open-preferences").on_click(move |_, cx| open_all.update(cx, |r, cx| r.open_panel(None, cx)))),
            );
        let desktop = BBox::new("Desktop").child(StringView::new("Start Olo over (Rescue stays on), log out, or restart the computer.").wrap(true)).child(
            div()
                .flex()
                .gap(px(m::TIGHT))
                .child(Button::new("Start Over").id("start-over").on_click(|_, _| {
                    let _ = start("olo-session", &["reload"]);
                }))
                .child(Button::new("Log Out…").id("log-out").on_click(|_, cx| {
                    open_alert(cx, "Log Out", "log-out", "Every program ends, and you're back at the login screen.", &["Cancel", "Log Out"], |choice, _| {
                        if choice == Some(1) {
                            let _ = start("olo-session", &["quit"]);
                        }
                    });
                }))
                .child(Button::new("Restart…").id("restart").on_click(|_, cx| {
                    open_alert(cx, "Restart", "restart", "The computer restarts now. Anything not saved is lost.", &["Cancel", "Restart"], |choice, _| {
                        if choice == Some(1) {
                            std::thread::spawn(|| {
                                if let Err(e) = olo_system::power::reboot() {
                                    tracing::warn!("restart: {e}");
                                }
                            });
                        }
                    });
                })),
        );
        let left = div().w(px(300.0)).flex_shrink_0().flex().flex_col().gap(px(m::SPACING)).child(displays).child(preferences).child(desktop);

        // The right: the terminal, the processes.
        let terminal = app.terminal.clone();
        let mut shell = div().flex_1().min_h_0().flex().flex_col().gap(px(m::TIGHT)).child(heading("Terminal"));
        if let Some(e) = &terminal.read(cx).error {
            shell = shell.child(StringView::new(e.clone()).color(style::c(p.failure)));
        } else if terminal.read(cx).exited {
            shell = shell.child(StringView::new("The shell has ended.").dim(true));
        }
        let shell = shell.child(div().flex_1().min_h_0().child(terminal));
        let rows = Rc::new(app.processes.clone());
        let cell_rows = rows.clone();
        // (The name last, as top has it: it takes what's left.)
        let columns = vec![Column::new("ID", 70.0).right(), Column::new("CPU", 60.0).right(), Column::new("Memory", 80.0).right(), Column::new("Process", 260.0)];
        let list = ColumnListView::new("rescue-processes", &app.process_list, columns, rows.len(), move |cell, window, cx| {
            let Some(p) = cell_rows.get(cell.row) else { return };
            let text = match cell.column {
                0 => p.pid.to_string(),
                1 => format!("{:.0}%", p.cpu),
                2 => procs::memory(p.memory),
                _ => p.name.clone(),
            };
            cell.text(window, cx, text);
        })
        .empty_text("Looking…")
        .type_select({
            let rows = rows.clone();
            move |row| rows.get(row).map_or("", |p| p.name.as_str()).to_owned().into()
        });
        let selected = app.process_list.read(cx).state.single().is_some();
        let (end, force) = (me.clone(), me.clone());
        let processes = div().h(px(PROCESSES_HEIGHT)).flex_shrink_0().flex().flex_col().gap(px(m::TIGHT)).child(heading("Processes")).child(div().flex_1().min_h_0().child(list)).child(
            div()
                .flex()
                .items_center()
                .gap(px(m::SPACING))
                .child(div().flex_1().min_w_0().children(note))
                .child(Button::new("End").id("end").enabled(selected).on_click(move |_, cx| end.update(cx, |r, cx| r.end_selected(false, cx))))
                .child(Button::new("Force End").id("force-end").enabled(selected).on_click(move |_, cx| force.update(cx, |r, cx| r.end_selected(true, cx)))),
        );
        let right = div().flex_1().min_w_0().flex().flex_col().gap(px(m::SPACING)).child(shell).child(processes);
        div().size_full().flex().flex_col().gap(px(m::SPACING + 2.0)).p(px(m::WINDOW_INSET)).child(header).child(div().flex_1().min_h_0().flex().gap(px(m::SPACING + 4.0)).child(left).child(right))
    }
}

/// app_server's displays now and as they change, and Rescue's end, on a
/// thread of its own.
fn watch_server(proxy: Proxy) {
    let spawned = std::thread::Builder::new().name("rescue-server".into()).spawn(move || {
        let Ok(mut c) = Client::connect() else { return proxy.send(Server::Over) };
        if let Ok(Reply::Outputs { outputs }) = c.request(&Request::Outputs) {
            proxy.send(Server::Outputs(outputs));
        }
        if let Ok(events) = c.into_events_of(&[EventKind::Outputs, EventKind::Power]) {
            for e in events {
                match e {
                    Ok(Event::OutputsChanged { outputs }) => proxy.send(Server::Outputs(outputs)),
                    Ok(Event::PowerChanged { status }) if !status.rescue => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        }
        // Rescue's over, or app_server's gone (a new one starts its own).
        proxy.send(Server::Over);
    });
    if let Err(e) = spawned {
        tracing::error!("Rescue can't follow app_server: {e}");
    }
}

fn main() {
    // Above everything, before any thread (they inherit it): what's left
    // when something has taken the computer.
    if let Err(e) = olo_system::priority::raise_self(olo_protocol::performance::RESCUE_NICE) {
        eprintln!("olo-rescue stays at its nice: {e}");
    }
    olo_ui::run(olo_protocol::rescue::APP_ID, |cx| {
        // Tab is the shell's in the terminal.
        cx.bind_keys([KeyBinding::new("tab", NoAction, Some(term::CONTEXT)), KeyBinding::new("shift-tab", NoAction, Some(term::CONTEXT))]);
        let proxy = olo_ui::proxy(cx);
        let terminal = cx.new(|cx| term::Terminal::new(proxy.clone(), cx));
        let (process_list, panels) = (ListModel::new(cx, false), ListModel::new(cx, false));
        let app =
            cx.new(|_| Rescue { proxy: proxy.clone(), terminal: terminal.clone(), processes: Vec::new(), process_list, panels, outputs: Vec::new(), here: None, windows: HashMap::new(), note: None });
        // (No windows until the displays are known: it stays up.)
        olo_ui::stay_resident(cx);
        let messages = app.clone();
        olo_ui::message_handler(cx, move |message, cx| {
            let message = match message.downcast::<term::Wake>() {
                Ok(_) => return terminal.update(cx, |t, cx| t.woke(cx)),
                Err(m) => m,
            };
            let message = match message.downcast::<term::Exited>() {
                Ok(_) => {
                    return terminal.update(cx, |t, cx| {
                        t.exited = true;
                        cx.notify();
                    });
                }
                Err(m) => m,
            };
            let message = match message.downcast::<Server>() {
                Ok(s) => {
                    let me = messages.clone();
                    return messages.update(cx, |r, cx| r.server(*s, &me, cx));
                }
                Err(m) => m,
            };
            let message = match message.downcast::<procs::Found>() {
                Ok(found) => {
                    return messages.update(cx, |r, cx| {
                        // The selection stays with its process as the order changes.
                        let pid = r.process_list.read(cx).state.single().and_then(|i| r.processes.get(i)).map(|p| p.pid);
                        r.processes = found.0;
                        if let Some(at) = pid.and_then(|pid| r.processes.iter().position(|p| p.pid == pid)) {
                            r.process_list.update(cx, |l, _| l.state.select(at));
                        }
                        cx.notify();
                    });
                }
                Err(m) => m,
            };
            if let Ok(done) = message.downcast::<Done>() {
                messages.update(cx, |r, cx| {
                    r.note = Some(done.0);
                    cx.notify();
                });
            }
        });
        watch_server(proxy.clone());
        procs::watch(proxy);
    });
}
