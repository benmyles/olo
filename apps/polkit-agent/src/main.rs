//! tabula-polkit-agent: asks for a password when a program needs an
//! administrator's permission (setting the clock, installing software, …).
//!
//! It registers with polkitd as the authentication agent for this login
//! session and stays in the background; each request opens a BAlert-style
//! window with polkit's message and a password field. The PAM conversation
//! runs in polkit's own helper (`tabula_system::polkit::Helper`), so with
//! Touch ID set up in PAM the window first says to touch the sensor, then
//! asks for the password if that doesn't work.
//!
//! ```text
//! tabula-polkit-agent                   the session's agent (started by tabula-session)
//! tabula-polkit-agent --self-test ACTION  serve only this process and ask
//!                                       polkitd about ACTION (changes nothing)
//! ```

use std::collections::HashMap;

use tabula_kit::{
    App, Cx, WindowId, WindowOptions,
    egui::{self, Key, Rect, pos2, vec2},
    instance::{self, Launch},
    metrics as m,
    style::{self, c},
    widgets::{self, Button, MenuField, TextControl},
};
use tabula_system::polkit::{Agent, Event, Helper, HelperEvent, Request, Subject};

const APP_ID: &str = "tabula.polkit-agent";
const WIDTH: u32 = 420;
const ICON: &str = "authentication";

enum Msg {
    Polkit(Event),
    Helper(WindowId, u64, HelperEvent),
    Registered(Result<(), String>),
}

struct Auth {
    request: Option<Request>,
    cookie: String,
    message: String,
    action_id: String,
    users: Vec<String>,
    user: usize,
    helper: Option<Helper>,
    /// Which run of the helper is current; events from older runs are
    /// ignored.
    run: u64,
    /// PAM is waiting for an answer: (prompt, echo).
    prompt: Option<(String, bool)>,
    answer: String,
    /// The latest message from PAM ("Place your finger on the reader").
    info: String,
    error: String,
    /// An answer is with PAM.
    checking: bool,
}

impl Auth {
    fn clear_answer(&mut self) {
        // Don't leave the password lying in freed memory.
        // SAFETY: zero bytes keep the string valid UTF-8.
        unsafe { self.answer.as_bytes_mut().fill(0) };
        self.answer.clear();
    }
}

struct AgentApp {
    windows: HashMap<WindowId, Auth>,
    /// `--self-test`: quit once its one request is done.
    self_test: bool,
}

impl AgentApp {
    fn begin(&mut self, cx: &mut Cx, request: Request) {
        let h = widgets::alert_height(&request.message) + 70;
        let id = cx.open_window(WindowOptions::new("Authenticate", WIDTH, h).min_size(WIDTH, h).max_size(WIDTH, h));
        let mut auth = Auth {
            cookie: request.cookie.clone(),
            message: request.message.clone(),
            action_id: request.action_id.clone(),
            users: request.users.clone(),
            user: 0,
            request: Some(request),
            helper: None,
            run: 0,
            prompt: None,
            answer: String::new(),
            info: String::new(),
            error: String::new(),
            checking: false,
        };
        if auth.users.is_empty() {
            auth.error = "Nobody is allowed to authorize this.".into();
        }
        self.windows.insert(id, auth);
        self.start_helper(cx, id);
        cx.activate(id);
    }

    /// (Re)start the helper for the chosen user.
    fn start_helper(&mut self, cx: &mut Cx, id: WindowId) {
        let Some(a) = self.windows.get_mut(&id) else { return };
        a.helper = None;
        a.prompt = None;
        a.checking = false;
        a.info.clear();
        a.run += 1;
        let Some(user) = a.users.get(a.user) else { return };
        let (proxy, run) = (cx.proxy(), a.run);
        match Helper::start(user, &a.cookie, move |e| proxy.send(Msg::Helper(id, run, e))) {
            Ok(h) => a.helper = Some(h),
            Err(e) => {
                tracing::warn!("polkit helper: {e}");
                a.error = "The authentication helper couldn't be started.".into();
            }
        }
    }

    fn helper_event(&mut self, cx: &mut Cx, id: WindowId, run: u64, e: HelperEvent) {
        let Some(a) = self.windows.get_mut(&id) else { return };
        if run != a.run {
            return;
        }
        match e {
            HelperEvent::Prompt { text, echo } => {
                a.checking = false;
                a.prompt = Some((text, echo));
            }
            HelperEvent::Info(t) => a.info = t,
            HelperEvent::Error(t) => a.error = t,
            HelperEvent::Done(true) => {
                if let Some(r) = a.request.take() {
                    r.finish(true);
                }
                self.close(cx, id);
            }
            HelperEvent::Done(false) => {
                a.clear_answer();
                if a.error.is_empty() || a.checking {
                    a.error = "Sorry, that didn't work. Please try again.".into();
                }
                // Another go, as GNOME and KDE allow.
                self.start_helper(cx, id);
            }
        }
        cx.redraw(id);
    }

    fn submit(&mut self, id: WindowId) {
        let Some(a) = self.windows.get_mut(&id) else { return };
        if a.prompt.is_none() || a.checking {
            return;
        }
        let r = a.helper.as_mut().map(|h| h.respond(&a.answer));
        a.clear_answer();
        a.prompt = None;
        match r {
            Some(Ok(())) => {
                a.checking = true;
                a.error.clear();
                a.info.clear();
            }
            _ => a.error = "The authentication helper stopped.".into(),
        }
    }

    /// Close a window; an unanswered request is cancelled.
    fn close(&mut self, cx: &mut Cx, id: WindowId) {
        if let Some(mut a) = self.windows.remove(&id) {
            a.clear_answer();
        }
        cx.close_window(id);
        if self.self_test && self.windows.is_empty() {
            cx.quit();
        }
    }
}

impl App for AgentApp {
    fn ui(&mut self, cx: &mut Cx, win: WindowId, ui: &mut egui::Ui) {
        let Some(a) = self.windows.get_mut(&win) else { return };
        let p = style::palette(ui.ctx());
        let full = ui.available_rect_before_wrap();
        let col = widgets::alert_frame(ui, ICON);
        let mut y = col.top();
        style::label(ui.painter(), col.left(), y + style::PLAIN_SIZE / 2.0 + 2.0, egui::Align::Min, "Authentication Required", style::bold_font(), c(p.panel_text));
        y += m::LABEL_ROW + m::TIGHT;
        let mut job = egui::text::LayoutJob::single_section(a.message.clone(), egui::TextFormat::simple(style::plain_font(), c(p.panel_text)));
        job.wrap.max_width = col.width();
        let g = ui.painter().layout_job(job);
        let text_h = g.size().y;
        let pos = style::text_origin(ui.ctx(), &g, col.left(), y + style::PLAIN_SIZE / 2.0 + 2.0, style::PLAIN_SIZE);
        ui.painter().galley(pos, g, c(p.panel_text));
        let message = Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), text_h));
        ui.interact(message, ui.scope_id().with("message"), egui::Sense::hover()).on_hover_text(&a.action_id);
        y += text_h + m::SPACING;

        let mut submit = false;
        let mut user_changed = false;
        let form = Rect::from_min_max(pos2(col.left(), y), pos2(col.right(), col.bottom() - m::CONTROL_HEIGHT - m::SPACING));
        ui.scope_builder(egui::UiBuilder::new().max_rect(form).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
            ui.spacing_mut().item_spacing.y = m::SPACING;
            let divider = 72.0;
            if a.users.len() > 1 {
                let names: Vec<&str> = a.users.iter().map(String::as_str).collect();
                let before = a.user;
                ui.add(MenuField::new(("identity", win), &mut a.user, &names).label("Identity:").divider(divider).width(form.width()));
                user_changed = a.user != before;
            }
            let label = a.prompt.as_ref().map_or("Password:".to_owned(), |(t, _)| {
                let t = t.trim();
                if t.ends_with(':') { t.to_owned() } else { format!("{t}:") }
            });
            let ready = a.prompt.is_some() && !a.checking;
            let echo = a.prompt.as_ref().is_some_and(|(_, e)| *e);
            let r = ui
                .add_enabled_ui(ready, |ui| {
                    ui.add(TextControl::new(&mut a.answer).label(&label).divider(divider).password(!echo).autofocus(ready).width(form.width()))
                })
                .inner;
            if ready && !r.has_focus() && !r.lost_focus() {
                r.request_focus();
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                submit = true;
            }
            let (note, color) = if a.checking {
                ("Checking…".to_owned(), style::disabled_text(&p))
            } else if !a.error.is_empty() {
                (a.error.clone(), c(p.failure))
            } else {
                (a.info.clone(), style::disabled_text(&p))
            };
            if !note.is_empty() {
                let r = ui.allocate_exact_size(vec2(form.width(), m::LABEL_ROW), egui::Sense::hover()).0;
                style::label(ui.painter(), r.left(), r.center().y, egui::Align::Min, note, style::plain_font(), color);
            }
        });

        let row = Rect::from_min_max(pos2(col.left(), col.bottom() - m::CONTROL_HEIGHT), pos2(full.right() - m::WINDOW_INSET, col.bottom()));
        let mut cancel = ui.input(|i| i.key_pressed(Key::Escape));
        ui.scope_builder(egui::UiBuilder::new().max_rect(row), |ui| {
            widgets::button_row(ui, |ui| {
                let can = a.prompt.is_some() && !a.checking && !a.answer.is_empty();
                if ui.add_enabled_ui(can, |ui| ui.add(Button::new("Authenticate").default(true))).inner.clicked() {
                    submit = true;
                }
                if ui.add(Button::new("Cancel")).clicked() {
                    cancel = true;
                }
            });
        });
        if cancel {
            self.close(cx, win);
            return;
        }
        if user_changed {
            if let Some(a) = self.windows.get_mut(&win) {
                a.error.clear();
            }
            self.start_helper(cx, win);
        }
        if submit {
            self.submit(win);
        }
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        self.close(cx, window);
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        let message = match message.downcast::<Launch>() {
            // A second start: we are already serving the session.
            Ok(_) => return,
            Err(other) => other,
        };
        let Ok(m) = message.downcast::<Msg>() else { return };
        match *m {
            Msg::Polkit(Event::Begin(r)) => self.begin(cx, r),
            Msg::Polkit(Event::Cancel(cookie)) => {
                let ids: Vec<WindowId> = self.windows.iter().filter(|(_, a)| a.cookie == cookie).map(|(id, _)| *id).collect();
                for id in ids {
                    self.close(cx, id);
                }
            }
            Msg::Helper(id, run, e) => self.helper_event(cx, id, run, e),
            Msg::Registered(Err(e)) => {
                eprintln!("tabula-polkit-agent: couldn't register with polkit: {e}");
                cx.quit();
            }
            Msg::Registered(Ok(())) => {}
        }
    }
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let self_test = match args.as_slice() {
        [flag, action] if flag == "--self-test" => Some(action.clone()),
        [] => None,
        _ => {
            eprintln!("usage: tabula-polkit-agent [--self-test ACTION]");
            return std::process::ExitCode::FAILURE;
        }
    };
    // Only the real session's agent may speak for the login session: a
    // test or nested session must never take over the user's prompts.
    if self_test.is_none() && std::env::var_os("TABULA_REAL_SESSION").is_none() {
        eprintln!("tabula-polkit-agent: not in a real Tabula session (use --self-test ACTION to try it)");
        return std::process::ExitCode::FAILURE;
    }
    if self_test.is_none() && instance::forward(APP_ID, &args) {
        return std::process::ExitCode::SUCCESS;
    }
    tabula_kit::run(APP_ID, |cx| {
        cx.stay_resident();
        if self_test.is_none() {
            instance::listen(APP_ID, cx.proxy());
        }
        let proxy = cx.proxy();
        let action = self_test.clone();
        std::thread::Builder::new()
            .name("polkit-agent".into())
            .spawn(move || {
                let subject = match &action {
                    Some(_) => Subject::process(std::process::id()).ok_or_else(|| "no /proc entry".to_owned()),
                    None => tabula_system::polkit::own_session().map(Subject::Session).map_err(|e| e.to_string()),
                };
                let on = {
                    let proxy = proxy.clone();
                    move |e| proxy.send(Msg::Polkit(e))
                };
                let agent = match subject.and_then(|s| Agent::register(s, on).map_err(|e| e.to_string())) {
                    Ok(agent) => agent,
                    Err(e) => {
                        proxy.send(Msg::Registered(Err(e)));
                        return;
                    }
                };
                proxy.send(Msg::Registered(Ok(())));
                if let Some(action) = action {
                    match tabula_system::polkit::check_interactive(&action) {
                        Ok(ok) => println!("self-test: {action}: {}", if ok { "authorized" } else { "not authorized" }),
                        Err(e) => println!("self-test: {action}: {e}"),
                    }
                    drop(agent);
                    return;
                }
                // Serve until the app exits.
                loop {
                    std::thread::park();
                }
            })
            .expect("thread");
        AgentApp { windows: HashMap::new(), self_test: self_test.is_some() }
    });
    std::process::ExitCode::SUCCESS
}
