//! olo-polkit-agent: asks for a password when a program needs an
//! administrator's permission (setting the clock, installing software, …),
//! and when the login keyring needs its password (keyring.rs).
//!
//! It registers with polkitd as the authentication agent for this login
//! session and stays in the background; each request opens a BAlert-style
//! window with polkit's message and a password field. The PAM conversation
//! runs in polkit's own helper (`olo_system::polkit::Helper`), so with
//! Touch ID set up in PAM the window first says to touch the sensor, then
//! asks for the password if that doesn't work.
//!
//! ```text
//! olo-polkit-agent                   the session's agent (started by olo-session)
//! olo-polkit-agent --self-test ACTION  serve only this process and ask
//!                                       polkitd about ACTION (changes nothing)
//! olo-polkit-agent --keyring-test BUS  only the keyring's prompts, on the
//!                                       D-Bus at address BUS (a test's own)
//! ```
//!
//! The keyring's prompts: gnome-keyring-daemon asks for passwords through
//! `org.gnome.keyring.SystemPrompter` on the session bus, which this agent
//! owns (`olo_system::keyring_prompt`), so they come up here rather
//! than in GTK's gcr-prompter.

use std::{collections::HashMap, sync::Arc};

use olo_system::{
    keyring_prompt::{self, Prompter},
    polkit::{Agent, Event, Helper, HelperEvent, Request, Subject},
};
use olo_ui::{
    Frame, WindowOptions,
    gpui::{AnyWindowHandle, App, AppContext as _, Context, Entity, FocusHandle, Global, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, WindowHandle, WindowId, div, px},
    instance::{self, Launch},
    metrics as m,
    widgets::{self, AlertFrame, Button, Conversation, ConversationField, MenuField, TextField, TextFieldEvent, alert, button_row, paragraph, paragraph_height, text_field},
};

mod keyring;

const APP_ID: &str = "olo.polkit-agent";
pub const WIDTH: f32 = 420.0;
const ICON: &str = "authentication";
/// Text column: the alert's width less its stripe, icon and margins.
pub const COLUMN: f32 = WIDTH - widgets::ALERT_TEXT_X - m::WINDOW_INSET;
/// Where the fields begin after their labels.
pub const DIVIDER: f32 = 72.0;

/// The window's height: the title, `message`, an Identity menu when
/// more than one user may answer, the password and the note under it,
/// and the buttons.
fn height(cx: &App, message: &str, identities: bool) -> f32 {
    let fixed = 2.0 * m::WINDOW_INSET + m::LABEL_ROW + m::TIGHT + 3.0 * m::SPACING + 2.0 * m::CONTROL_HEIGHT + m::LABEL_ROW;
    let menu = if identities { m::CONTROL_HEIGHT + m::SPACING } else { 0.0 };
    (fixed + menu + paragraph_height(cx, message, COLUMN)).ceil()
}

enum Msg {
    Polkit(Event),
    Helper(WindowId, u64, HelperEvent),
    Registered(Result<(), String>),
    Keyring(keyring_prompt::Event),
    Prompter(Result<Arc<Prompter>, String>),
    /// The self-test's question is answered.
    Checked,
}

/// The agent: its windows (one for each request and keyring prompt).
#[derive(Default)]
struct Agents {
    auths: HashMap<WindowId, Entity<Auth>>,
    asks: HashMap<WindowId, Entity<keyring::Ask>>,
    prompter: Option<Arc<Prompter>>,
    /// `--self-test`: quit once its one request is done.
    self_test: bool,
}

impl Global for Agents {}

/// One request's window.
struct Auth {
    window: Option<AnyWindowHandle>,
    request: Option<Request>,
    cookie: String,
    message: SharedString,
    action_id: SharedString,
    users: Vec<SharedString>,
    user: usize,
    helper: Option<Helper>,
    /// Which run of the helper is current; events from older runs are
    /// ignored.
    run: u64,
    conv: Conversation,
    field: Entity<TextField>,
    focus: FocusHandle,
}

impl Auth {
    /// (Re)starts the helper for the chosen user.
    fn start_helper(&mut self, cx: &mut Context<Self>) {
        self.helper = None;
        self.conv.prompt = None;
        self.conv.checking = false;
        self.conv.info.clear();
        self.run += 1;
        self.sync_field(cx);
        let (Some(user), Some(window)) = (self.users.get(self.user), self.window) else { return };
        let (proxy, run, id) = (olo_ui::proxy(cx), self.run, window.window_id());
        match Helper::start(user, &self.cookie, move |e| proxy.send(Msg::Helper(id, run, e))) {
            Ok(h) => self.helper = Some(h),
            Err(e) => {
                tracing::warn!("polkit helper: {e}");
                self.conv.error = "The authentication helper couldn't be started.".into();
            }
        }
        cx.notify();
    }

    /// The field takes typing while PAM waits for it, showing bullets
    /// unless PAM echoes; it has the keyboard then.
    fn sync_field(&mut self, cx: &mut Context<Self>) {
        let (ready, echo) = (self.conv.ready(), self.conv.prompt.as_ref().is_some_and(|(_, e)| *e));
        self.field.update(cx, |f, cx| {
            f.set_enabled(ready, cx);
            f.set_password(!echo, cx);
        });
        if ready && let Some(window) = self.window {
            let focus = olo_ui::gpui::Focusable::focus_handle(&self.field, cx);
            let _ = window.update(cx, |_, window, _| window.focus(&focus));
        }
    }

    fn helper_event(&mut self, run: u64, e: HelperEvent, cx: &mut Context<Self>) {
        if run != self.run {
            return;
        }
        match e {
            HelperEvent::Prompt { text, echo } => {
                self.conv.ask(text, echo);
                self.sync_field(cx);
            }
            HelperEvent::Info(t) => self.conv.info = t,
            HelperEvent::Error(t) => self.conv.error = t,
            HelperEvent::Done(true) => {
                if let Some(r) = self.request.take() {
                    r.finish(true);
                }
                self.close(cx);
            }
            HelperEvent::Done(false) => {
                self.field.update(cx, |f, cx| f.clear_secret(cx));
                if self.conv.error.is_empty() || self.conv.checking {
                    self.conv.error = "Sorry, that didn't work. Please try again.".into();
                }
                // Another go, as GNOME and KDE allow.
                self.start_helper(cx);
            }
        }
        cx.notify();
    }

    fn can_submit(&self, cx: &App) -> bool {
        self.conv.ready() && !self.field.read(cx).text().is_empty()
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.can_submit(cx) {
            return;
        }
        let answer = self.field.read(cx).text().to_owned();
        let r = self.helper.as_mut().map(|h| h.respond(&answer));
        drop(zeroed(answer));
        self.field.update(cx, |f, cx| f.clear_secret(cx));
        self.conv.sent();
        if !matches!(r, Some(Ok(()))) {
            self.conv.checking = false;
            self.conv.error = "The authentication helper stopped.".into();
        }
        self.sync_field(cx);
        cx.notify();
    }

    /// Closes the window; an unanswered request is cancelled (dropped).
    fn close(&mut self, cx: &mut Context<Self>) {
        self.field.update(cx, |f, cx| f.clear_secret(cx));
        if let Some(window) = self.window.take() {
            forget(cx, window.window_id());
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
    }
}

/// A copy of a password, its bytes zeroed before it goes.
fn zeroed(mut s: String) -> String {
    // SAFETY: zero bytes keep the string valid UTF-8.
    unsafe { s.as_bytes_mut().fill(0) };
    s.clear();
    s
}

/// A window of ours closed: what was in it goes, and the self-test ends
/// with its one request.
fn forget(cx: &mut App, id: WindowId) {
    let agents = cx.default_global::<Agents>();
    agents.auths.remove(&id);
    agents.asks.remove(&id);
    if agents.self_test && agents.auths.is_empty() {
        cx.quit();
    }
}

impl Render for Auth {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let mut form = div().flex().flex_col().gap(px(m::SPACING));
        if self.users.len() > 1 {
            let view = view.clone();
            form = form.child(MenuField::new("identity", self.users.clone(), self.user).label("Identity:").divider(DIVIDER).width(COLUMN).on_change(move |i, _, cx| {
                view.update(cx, |a, cx| {
                    a.user = i;
                    a.conv.error.clear();
                    a.start_helper(cx);
                })
            }));
        }
        form = form.child(ConversationField::new(&self.conv, self.field.clone(), DIVIDER));
        let can = self.can_submit(cx);
        let (cancel, submit) = (view.clone(), view.clone());
        div()
            .size_full()
            .key_context(alert::CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|a, _: &alert::Cancel, _, cx| a.close(cx)))
            .on_action(cx.listener(|a, _: &alert::Default, _, cx| a.submit(cx)))
            .child(
                AlertFrame::new(ICON)
                    .child(widgets::alert_title("Authentication Required"))
                    .child(div().h(px(m::TIGHT)))
                    .child(paragraph(self.message.clone()).id("message").tooltip(widgets::tooltip(self.action_id.clone())))
                    .child(div().h(px(m::SPACING)))
                    .child(form)
                    .child(div().flex_1())
                    .child(
                        button_row()
                            .child(Button::new("Cancel").on_click(move |_, cx| cancel.update(cx, |a, cx| a.close(cx))))
                            .child(Button::new("Authenticate").default(true).enabled(can).on_click(move |_, cx| submit.update(cx, |a, cx| a.submit(cx)))),
                    ),
            )
    }
}

/// Opens a window for polkit's request.
fn begin(cx: &mut App, request: Request) {
    let h = height(cx, &request.message, request.users.len() > 1);
    let field = text_field(cx, |f| f.name("Password").password(true));
    let mut conv = Conversation::default();
    if request.users.is_empty() {
        conv.error = "Nobody is allowed to authorize this.".into();
    }
    let auth = cx.new(|cx| {
        cx.subscribe(&field, |a: &mut Auth, _, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Entered => a.submit(cx),
            _ => cx.notify(),
        })
        .detach();
        Auth {
            window: None,
            cookie: request.cookie.clone(),
            message: request.message.clone().into(),
            action_id: request.action_id.clone().into(),
            users: request.users.iter().map(|u| u.clone().into()).collect(),
            user: 0,
            request: Some(request),
            helper: None,
            run: 0,
            conv,
            field,
            focus: cx.focus_handle(),
        }
    });
    let Some(handle) = open(cx, "Authenticate", h, auth.clone()) else { return };
    let id = handle.window_id();
    cx.default_global::<Agents>().auths.insert(id, auth.clone());
    auth.update(cx, |a, cx| {
        a.window = Some(handle.into());
        a.start_helper(cx);
    });
}

/// Opens an alert window showing `view`, which has the keyboard; closing
/// it from its tab is as its Cancel.
pub fn open<V: Render + 'static>(cx: &mut App, title: &str, height: f32, view: Entity<V>) -> Option<WindowHandle<Frame>>
where
    V: Cancel,
{
    let v = view.clone();
    olo_ui::open_window(cx, WindowOptions::alert(title.to_owned(), WIDTH, height), move |window, cx| {
        window.activate_window();
        let closing = v.clone();
        window.on_window_should_close(cx, move |_, cx| closing.update(cx, |v, cx| v.cancel(cx)));
        v
    })
}

/// What closing an alert from its tab does: true if it may close now.
pub trait Cancel {
    fn cancel(&mut self, cx: &mut Context<Self>) -> bool
    where
        Self: Sized;
}

impl Cancel for Auth {
    fn cancel(&mut self, cx: &mut Context<Self>) -> bool {
        self.field.update(cx, |f, cx| f.clear_secret(cx));
        if let Some(w) = self.window.take() {
            forget(cx, w.window_id());
        }
        true
    }
}

fn message(message: Box<dyn std::any::Any + Send>, cx: &mut App) {
    let message = match message.downcast::<Launch>() {
        // A second start: we are already serving the session.
        Ok(_) => return,
        Err(other) => other,
    };
    let Ok(m) = message.downcast::<Msg>() else { return };
    match *m {
        Msg::Polkit(Event::Begin(r)) => begin(cx, r),
        Msg::Polkit(Event::Cancel(cookie)) => {
            let views: Vec<Entity<Auth>> = cx.default_global::<Agents>().auths.values().cloned().collect();
            for v in views {
                if v.read(cx).cookie == cookie {
                    v.update(cx, |a, cx| a.close(cx));
                }
            }
        }
        Msg::Helper(id, run, e) => {
            if let Some(v) = cx.default_global::<Agents>().auths.get(&id).cloned() {
                v.update(cx, |a, cx| a.helper_event(run, e, cx));
            }
        }
        Msg::Registered(Err(e)) => {
            eprintln!("olo-polkit-agent: couldn't register with polkit: {e}");
            cx.quit();
        }
        Msg::Registered(Ok(())) => {}
        Msg::Keyring(e) => keyring::event(cx, e),
        Msg::Prompter(Ok(p)) => cx.default_global::<Agents>().prompter = Some(p),
        Msg::Prompter(Err(e)) => eprintln!("olo-polkit-agent: couldn't serve the keyring's prompts: {e}"),
        // (Answered without asking, or after its window closed.)
        Msg::Checked => {
            if cx.default_global::<Agents>().auths.is_empty() {
                cx.quit();
            }
        }
    }
}

fn main() -> std::process::ExitCode {
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (self_test, keyring_test) = match args.as_slice() {
        [flag, action] if flag == "--self-test" => (Some(action.clone()), false),
        [flag, bus] if flag == "--keyring-test" => {
            // SAFETY: before any thread starts.
            unsafe { std::env::set_var("DBUS_SESSION_BUS_ADDRESS", bus) };
            (None, true)
        }
        [] => (None, false),
        _ => {
            eprintln!("usage: olo-polkit-agent [--self-test ACTION | --keyring-test BUS]");
            return std::process::ExitCode::FAILURE;
        }
    };
    // Only the real session's agent may speak for the login session: a
    // test or nested session must never take over the user's prompts.
    if self_test.is_none() && !keyring_test && std::env::var_os("OLO_REAL_SESSION").is_none() {
        eprintln!("olo-polkit-agent: not in a real Olo session (use --self-test ACTION to try it)");
        return std::process::ExitCode::FAILURE;
    }
    if self_test.is_none() && !keyring_test && instance::forward(APP_ID, &args) {
        return std::process::ExitCode::SUCCESS;
    }
    olo_ui::run(APP_ID, move |cx| {
        olo_ui::stay_resident(cx);
        cx.set_global(Agents { self_test: self_test.is_some(), ..Default::default() });
        olo_ui::message_handler(cx, message);
        if self_test.is_none() && !keyring_test {
            instance::listen(APP_ID, olo_ui::proxy(cx));
        }
        if self_test.is_none() {
            let proxy = olo_ui::proxy(cx);
            std::thread::Builder::new()
                .name("keyring-prompt".into())
                .spawn(move || {
                    let on = {
                        let proxy = proxy.clone();
                        move |e| proxy.send(Msg::Keyring(e))
                    };
                    proxy.send(Msg::Prompter(Prompter::serve(on).map(Arc::new).map_err(|e| e.to_string())));
                })
                .expect("thread");
        }
        if keyring_test {
            return;
        }
        let proxy = olo_ui::proxy(cx);
        let action = self_test.clone();
        std::thread::Builder::new()
            .name("polkit-agent".into())
            .spawn(move || {
                let subject = match &action {
                    Some(_) => Subject::process(std::process::id()).ok_or_else(|| "no /proc entry".to_owned()),
                    None => olo_system::polkit::own_session().map(Subject::Session).map_err(|e| e.to_string()),
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
                    match olo_system::polkit::check_interactive(&action) {
                        Ok(ok) => println!("self-test: {action}: {}", if ok { "authorized" } else { "not authorized" }),
                        Err(e) => println!("self-test: {action}: {e}"),
                    }
                    drop(agent);
                    proxy.send(Msg::Checked);
                    return;
                }
                // Serve until the app exits.
                loop {
                    std::thread::park();
                }
            })
            .expect("thread");
    });
    std::process::ExitCode::SUCCESS
}
