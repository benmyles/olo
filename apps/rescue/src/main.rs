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
//!
//! Each display's surface is a window on a looper of its own. What
//! outlives them is the program's, on its own looper: the displays there
//! are, which has Rescue, and the terminal's shell (`term::Session`), so
//! Show Rescue Here moves Rescue with its shell as it was.

mod procs;
mod term;

use std::collections::HashMap;

use olo_kit::{
    AppCx, Child, Cx, Element, IntoElement as _, LayerKeyboard, LayerShellLayer, ParentElement as _, Proxy, Styled as _, View, WindowHandle, WindowOptions, canvas, div, icons, metrics as m, px,
    style, theme,
    widgets::{BBox, Button, Column, ColumnListView, ListModel, StringView, heading, open_alert},
};
use olo_protocol::{Client, Event, EventKind, OutputInfo, Reply, Request};

const TITLE: &str = "Olo Rescue";
/// The processes' part, under the terminal (ten rows or so).
const PROCESSES_HEIGHT: f32 = 300.0;

/// What the connection to app_server brings.
enum Server {
    Outputs(Vec<OutputInfo>),
    /// Rescue's over (or app_server has gone): quit.
    Over,
}

/// A display's surface asks for Rescue to show on it.
struct ShowHere(String);

/// The program's own, on its looper: what outlives any display's
/// surface.
struct Rescue {
    session: term::Session,
    /// The processes as last found (a surface that takes Rescue over
    /// starts with them).
    processes: Vec<procs::Proc>,
    outputs: Vec<OutputInfo>,
    /// The display Rescue shows on (None: the main one).
    here: Option<String>,
    windows: HashMap<String, WindowHandle<Screen>>,
}

impl Rescue {
    /// The display Rescue shows on.
    fn main_display(&self) -> Option<String> {
        let named = self.here.as_ref().filter(|h| self.outputs.iter().any(|o| &o.name == *h));
        named.cloned().or_else(|| self.outputs.iter().find(|o| o.main).or(self.outputs.first()).map(|o| o.name.clone()))
    }

    /// Where Rescue is, as another display's surface says it.
    fn there(&self) -> String {
        self.main_display().and_then(|n| self.outputs.iter().find(|o| o.name == n)).map_or_else(
            || "another display".to_owned(),
            // (Its number too, among displays of one name.)
            |o| match o.number.filter(|_| self.outputs.iter().filter(|x| x.display_name() == o.display_name()).count() > 1) {
                Some(n) => format!("{} ({})", o.display_name(), olo_protocol::display_label(n)),
                None => o.display_name(),
            },
        )
    }

    /// A surface on each display (each display's as it comes and goes).
    /// If the one that had Rescue has gone, every surface is made again
    /// for what it is now.
    fn cover(&mut self) {
        let names: Vec<String> = self.outputs.iter().filter(|o| o.mirror_of.is_none()).map(|o| o.name.clone()).collect();
        self.windows.retain(|name, w| {
            let keep = names.contains(name) && w.is_open();
            if !keep {
                w.close();
            }
            keep
        });
        let main = self.main_display();
        let there = self.there();
        for name in names {
            if let Some(w) = self.windows.get(&name) {
                w.messenger().send(ScreenMsg::There(there.clone()));
                continue;
            }
            let is_main = main.as_deref() == Some(name.as_str());
            let output = name.clone();
            let options = WindowOptions::new(TITLE, 1.0, 1.0).layer(olo_protocol::rescue::NAMESPACE, LayerShellLayer::Bottom, (true, true, true, true)).layer_options(|l| {
                // All of it, Deskbar's room too (it's put away).
                l.exclusive_zone = -1;
                l.output = Some(output.clone());
                // Only Rescue's own takes the keyboard (app_server gives
                // it it as it shows): the others' button is for the
                // pointer.
                l.keyboard = if is_main { LayerKeyboard::OnDemand } else { LayerKeyboard::None };
            });
            let (session, processes, there) = (self.session.clone(), self.processes.clone(), there.clone());
            let window = olo_kit::open_window(options, move |cx| Screen::new(output, is_main.then_some((session, processes)), there, cx));
            self.windows.insert(name, window);
        }
    }

    /// The surface that has Rescue now.
    fn main_window(&self) -> Option<&WindowHandle<Screen>> {
        self.main_display().and_then(|name| self.windows.get(&name))
    }

    fn to_main(&self, message: ScreenMsg) {
        if let Some(w) = self.main_window() {
            w.messenger().send(message);
        }
    }

    fn show_here(&mut self, output: String) {
        let was = self.main_display();
        self.here = Some(output);
        if self.main_display() == was {
            return;
        }
        // (Each surface made again for what it is now.)
        for (_, w) in self.windows.drain() {
            w.close();
        }
        self.cover();
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

/// What a display's surface is told, and what its controls say.
#[derive(Clone)]
enum ScreenMsg {
    /// The shell printed.
    Woke,
    /// The shell ended.
    Exited,
    Found(Vec<procs::Proc>),
    Note((String, bool)),
    /// Where Rescue is now (for a surface that hasn't it).
    There(String),
    Leave,
    ShowHere,
    OpenPanel(Option<&'static str>),
    OpenChosenPanel,
    ResetAsked,
    Reset(Option<usize>),
    LogOutAsked,
    LogOut(Option<usize>),
    RestartAsked,
    Restart(Option<usize>),
    End(bool),
    /// A list's selection changed (its buttons follow).
    Selected,
}

/// What the surface that has Rescue shows.
struct Main {
    session: term::Session,
    terminal: Child<term::Terminal>,
    processes: Vec<procs::Proc>,
    process_list: ListModel,
    panels: ListModel,
    /// What the last thing done came to: its words, and whether it went wrong.
    note: Option<(String, bool)>,
}

/// One display's surface.
struct Screen {
    output: String,
    /// Rescue, if this display has it.
    main: Option<Main>,
    /// Where Rescue is, if it hasn't.
    there: String,
}

impl Screen {
    fn new(output: String, main: Option<(term::Session, Vec<procs::Proc>)>, there: String, cx: &mut Cx<Self>) -> Screen {
        let main = main.map(|(session, processes)| {
            let terminal = cx.new_view({
                let session = session.clone();
                move |_| term::Terminal::new(session)
            });
            // Typing is the shell's from the start.
            let focus = terminal.read(cx).focus.clone();
            focus.focus(cx);
            Main { session, terminal, processes, process_list: ListModel::new(false), panels: ListModel::new(false), note: None }
        });
        Screen { output, main, there }
    }

    /// The displays' saved settings put back to Olo's (brightness kept),
    /// for when Rescue ends.
    fn reset_displays(&mut self) {
        let proxy = olo_kit::proxy();
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
        if let Some(main) = &mut self.main {
            main.note = Some(("Resetting the displays' settings…".into(), false));
        }
    }
}

impl Main {
    fn open_panel(&mut self, arg: Option<&str>) {
        let args: Vec<&str> = arg.into_iter().collect();
        self.note = Some(match start("olo-preferences", &args) {
            Ok(()) => (format!("Preferences opens{}; what you change there applies when you leave Rescue.", arg.map(|a| format!(" ({a})")).unwrap_or_default()), false),
            Err(e) => (format!("Preferences didn't open: {e}"), true),
        });
    }

    fn chosen_panel(&self) -> Option<&'static str> {
        self.panels.state().single().and_then(|i| olo_xdg::apps::PREFERENCE_PANELS.get(i)).map(|p| p.0)
    }

    fn end_selected(&mut self, force: bool) {
        let Some(p) = self.process_list.state().single().and_then(|i| self.processes.get(i)).cloned() else { return };
        self.note = Some(match procs::end(p.pid, force) {
            Ok(()) if force => (format!("{} ({}) was made to quit.", p.name, p.pid), false),
            Ok(()) => (format!("{} ({}) was asked to quit.", p.name, p.pid), false),
            Err(e) => (format!("{} ({}) wasn't ended: {e}", p.name, p.pid), true),
        });
    }

    /// The processes as just found: the selection stays with its process
    /// as the order changes.
    fn found(&mut self, found: Vec<procs::Proc>) {
        let pid = self.process_list.state().single().and_then(|i| self.processes.get(i)).map(|p| p.pid);
        self.processes = found;
        if let Some(at) = pid.and_then(|pid| self.processes.iter().position(|p| p.pid == pid)) {
            self.process_list.state_mut().select(at);
        }
    }
}

impl View for Screen {
    type Message = ScreenMsg;

    fn message(&mut self, message: ScreenMsg, cx: &mut Cx<Self>) {
        match (message, &mut self.main) {
            (ScreenMsg::There(there), _) => self.there = there,
            // Rescue over: app_server puts everything back, and ends us.
            (ScreenMsg::Leave, _) => ask(Request::Rescue { on: false }),
            (ScreenMsg::ShowHere, _) => olo_kit::proxy().send(ShowHere(self.output.clone())),
            (ScreenMsg::Woke, Some(main)) => return main.terminal.update(cx, |t, cx| t.woke(cx)),
            (ScreenMsg::Exited, Some(_)) => {}
            (ScreenMsg::Found(found), Some(main)) => main.found(found),
            (ScreenMsg::Note(note), Some(main)) => main.note = Some(note),
            (ScreenMsg::OpenPanel(arg), Some(main)) => main.open_panel(arg),
            (ScreenMsg::OpenChosenPanel, Some(main)) => {
                let arg = main.chosen_panel();
                main.open_panel(arg);
            }
            (ScreenMsg::ResetAsked, Some(_)) => {
                let answer = cx.msg_with(|choice: &Option<usize>| ScreenMsg::Reset(*choice));
                open_alert(
                    cx,
                    "Reset Display Settings",
                    "warning",
                    "Every display goes back to Olo's own resolution, scale, arrangement and colours when you leave Rescue. Its brightness stays as it is.",
                    &["Cancel", "Reset"],
                    answer,
                );
                return;
            }
            (ScreenMsg::Reset(Some(1)), Some(_)) => self.reset_displays(),
            (ScreenMsg::LogOutAsked, Some(_)) => {
                let answer = cx.msg_with(|choice: &Option<usize>| ScreenMsg::LogOut(*choice));
                open_alert(cx, "Log Out", "log-out", "Every program ends, and you're back at the login screen.", &["Cancel", "Log Out"], answer);
                return;
            }
            (ScreenMsg::LogOut(Some(1)), Some(_)) => {
                let _ = start("olo-session", &["quit"]);
            }
            (ScreenMsg::RestartAsked, Some(_)) => {
                let answer = cx.msg_with(|choice: &Option<usize>| ScreenMsg::Restart(*choice));
                open_alert(cx, "Restart", "restart", "The computer restarts now. Anything not saved is lost.", &["Cancel", "Restart"], answer);
                return;
            }
            (ScreenMsg::Restart(Some(1)), Some(_)) => {
                std::thread::spawn(|| {
                    if let Err(e) = olo_system::power::reboot() {
                        tracing::warn!("restart: {e}");
                    }
                });
            }
            (ScreenMsg::End(force), Some(main)) => main.end_selected(force),
            _ => {}
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        if self.main.is_some() { self.rescue(cx) } else { self.elsewhere(cx) }
    }
}

/// An icon, `size` points, on whole device pixels.
fn icon(name: &'static str, size: f32) -> Element {
    canvas(move |g, b| {
        let at = style::snap_centred(g, b.center(), size, size);
        icons::paint(g, at, name);
    })
    .paint_key(style::key((name, size.to_bits())))
    .size(px(size))
    .flex_shrink_0()
}

impl Screen {
    /// Another display's: where Rescue is, and Show Rescue Here.
    fn elsewhere(&self, cx: &mut Cx<Self>) -> Element {
        div().size_full().flex().items_center().justify_center().child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(m::SPACING))
                .child(icon("warning", 32.0))
                .child(StringView::new(TITLE).bold(true))
                .child(StringView::new(format!("Rescue is on {}.", self.there)).dim(true))
                .child(Button::new("Show Rescue Here").id("show-here").on_click(cx.msg(ScreenMsg::ShowHere))),
        )
    }

    fn rescue(&self, cx: &mut Cx<Self>) -> Element {
        let Some(main) = &self.main else { return div() };
        let p = theme::palette();
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
            .child(Button::new("Leave Rescue").id("leave").default(true).on_click(cx.msg(ScreenMsg::Leave)));
        let note = main.note.clone().map(|(text, bad)| {
            let v = StringView::new(text).wrap(true);
            if bad { v.color(style::c(p.failure)) } else { v.dim(true) }
        });

        // The left: the displays, Preferences, the desktop.
        let displays = BBox::new("Displays")
            .child(StringView::new("Each display is at its own best resolution and scale now: no rotation, mirroring, HDR or VRR, and night light off. Yours come back as you leave.").wrap(true))
            .child(div().flex().child(Button::new("Reset Display Settings…").id("reset-displays").on_click(cx.msg(ScreenMsg::ResetAsked))));
        let panels = olo_xdg::apps::PREFERENCE_PANELS;
        let panel_list = ColumnListView::new(cx, "rescue-panels", &main.panels, vec![Column::new("Panel", 240.0)], panels.len(), move |_: &Screen, cell, g| {
            if let Some((_, name, icon)) = panels.get(cell.row) {
                cell.icon_text(g, icon, "preferences", *name);
            }
        })
        .row_key(|_, row| row as u64)
        .no_header()
        .type_select(move |_, row| panels.get(row).map_or("", |p| p.1).into())
        .on_select(cx.msg(ScreenMsg::Selected))
        .on_open(cx.msg_with(move |row: &usize| ScreenMsg::OpenPanel(panels.get(*row).map(|p| p.0))));
        let chosen = main.chosen_panel();
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
                    .child(Button::new("Open").id("open-panel").enabled(chosen.is_some()).on_click(cx.msg(ScreenMsg::OpenChosenPanel)))
                    .child(Button::new("All Preferences").id("open-preferences").on_click(cx.msg(ScreenMsg::OpenPanel(None)))),
            );
        let desktop = BBox::new("Desktop").child(StringView::new("Start Olo over (Rescue stays on), log out, or restart the computer.").wrap(true)).child(
            div()
                .flex()
                .gap(px(m::TIGHT))
                .child(Button::new("Start Over").id("start-over").on_click(|_, _| {
                    let _ = start("olo-session", &["reload"]);
                }))
                .child(Button::new("Log Out…").id("log-out").on_click(cx.msg(ScreenMsg::LogOutAsked)))
                .child(Button::new("Restart…").id("restart").on_click(cx.msg(ScreenMsg::RestartAsked))),
        );
        let left = div().w(px(300.0)).flex_shrink_0().flex().flex_col().gap(px(m::SPACING)).child(displays).child(preferences).child(desktop);

        // The right: the terminal, the processes.
        let mut shell = div().flex_1().min_h_0().flex().flex_col().gap(px(m::TIGHT)).child(heading("Terminal"));
        if let Some(e) = &main.session.error {
            shell = shell.child(StringView::new(e.clone()).color(style::c(p.failure)));
        } else if main.session.exited() {
            shell = shell.child(StringView::new("The shell has ended.").dim(true));
        }
        let shell = shell.child(div().flex_1().min_h_0().child(main.terminal.element()));
        // (The name last, as top has it: it takes what's left.)
        let columns = vec![Column::new("ID", 70.0).right(), Column::new("CPU", 60.0).right(), Column::new("Memory", 80.0).right(), Column::new("Process", 260.0)];
        fn processes_of(screen: &Screen) -> &[procs::Proc] {
            screen.main.as_ref().map_or(&[][..], |main| &main.processes[..])
        }
        let list = ColumnListView::new(cx, "rescue-processes", &main.process_list, columns, main.processes.len(), move |screen: &Screen, cell, g| {
            let Some(p) = processes_of(screen).get(cell.row) else { return };
            let text = match cell.column {
                0 => p.pid.to_string(),
                1 => format!("{:.0}%", p.cpu),
                2 => procs::memory(p.memory),
                _ => p.name.clone(),
            };
            cell.text(g, text);
        })
        // (A row is drawn again when what it says changes: every two
        // seconds, most rows' CPU.)
        .row_key(move |screen, row| processes_of(screen).get(row).map_or(0, |p| style::key((p.pid, &p.name, (p.cpu.round() as i32), procs::memory(p.memory)))))
        .empty_text("Looking…")
        .type_select(move |screen, row| processes_of(screen).get(row).map_or(String::new(), |p| p.name.clone()).into())
        .on_select(cx.msg(ScreenMsg::Selected));
        let selected = main.process_list.state().single().is_some();
        let processes = div().h(px(PROCESSES_HEIGHT)).flex_shrink_0().flex().flex_col().gap(px(m::TIGHT)).child(heading("Processes")).child(div().flex_1().min_h_0().child(list)).child(
            div()
                .flex()
                .items_center()
                .gap(px(m::SPACING))
                .child(div().flex_1().min_w_0().children(note))
                .child(Button::new("End").id("end").enabled(selected).on_click(cx.msg(ScreenMsg::End(false))))
                .child(Button::new("Force End").id("force-end").enabled(selected).on_click(cx.msg(ScreenMsg::End(true)))),
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
    olo_crash::install();
    // Above everything, before any thread (they inherit it): what's left
    // when something has taken the computer.
    if let Err(e) = olo_system::priority::raise_self(olo_protocol::performance::RESCUE_NICE) {
        eprintln!("olo-rescue stays at its nice: {e}");
    }
    olo_kit::run(olo_protocol::rescue::APP_ID, |app: &mut AppCx| {
        let proxy = olo_kit::proxy();
        let mut rescue = Rescue { session: term::Session::new(proxy.clone()), processes: Vec::new(), outputs: Vec::new(), here: None, windows: HashMap::new() };
        // (No windows until the displays are known: it stays up.)
        olo_kit::stay_resident();
        olo_kit::message_handler(app, move |message, _| {
            let message = match message.downcast::<term::Wake>() {
                Ok(_) => {
                    // (No surface has Rescue yet: the one that comes
                    // draws what's printed, and is told of what's printed
                    // after.)
                    if rescue.main_window().is_none() {
                        rescue.session.shown();
                    }
                    return rescue.to_main(ScreenMsg::Woke);
                }
                Err(m) => m,
            };
            let message = match message.downcast::<term::Exited>() {
                Ok(_) => {
                    rescue.session.set_exited();
                    return rescue.to_main(ScreenMsg::Exited);
                }
                Err(m) => m,
            };
            let message = match message.downcast::<Server>() {
                Ok(s) => {
                    return match *s {
                        Server::Outputs(outputs) => {
                            rescue.outputs = outputs;
                            rescue.cover();
                        }
                        Server::Over => olo_kit::quit(),
                    };
                }
                Err(m) => m,
            };
            let message = match message.downcast::<procs::Found>() {
                Ok(found) => {
                    rescue.processes = found.0;
                    return rescue.to_main(ScreenMsg::Found(rescue.processes.clone()));
                }
                Err(m) => m,
            };
            let message = match message.downcast::<ShowHere>() {
                Ok(here) => return rescue.show_here(here.0),
                Err(m) => m,
            };
            if let Ok(done) = message.downcast::<Done>() {
                rescue.to_main(ScreenMsg::Note(done.0));
            }
        });
        watch_server(proxy.clone());
        procs::watch(proxy);
    });
}
