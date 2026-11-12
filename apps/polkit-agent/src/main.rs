//! olo-polkit-agent: asks for a password when a program needs an
//! administrator's permission (setting the clock, installing software, …),
//! and when the login keyring needs its password (keyring.rs).
//!
//! It registers with polkitd as the authentication agent for this login
//! session and stays in the background; each request opens a BAlert-style
//! window with polkit's message and a password field. The PAM conversation
//! runs in polkit's own helper (`olo_system::polkit::Helper`), which goes
//! one way at a time (the face, Touch ID, the password); so once it
//! listens for a finger, a second helper starts, whose PAM finds the
//! sensor taken and asks for the password at once (`second`): the finger,
//! typing and a security key all answer, the first to succeed counting.
//! A touch of a security key set up (`olo_fido`) answers for the person's
//! own identity: the password it keeps goes to whichever helper asks.
//!
//! ```text
//! olo-polkit-agent                   the session's agent (started by olo-session)
//! olo-polkit-agent --self-test ACTION  serve only this process and ask
//!                                       polkitd about ACTION (changes nothing)
//! olo-polkit-agent --keyring-test BUS  only the keyring's prompts, on the
//!                                       D-Bus at address BUS (a test's own)
//! ```
//!
//! Each request's window is on a looper of its own and keeps its own
//! conversation; the app's looper keeps which windows there are and hands
//! each what polkit, its helpers and the keyring say.
//!
//! The keyring's prompts: gnome-keyring-daemon asks for passwords through
//! `org.gnome.keyring.SystemPrompter` on the session bus, which this agent
//! owns (`olo_system::keyring_prompt`), so they come up here rather
//! than in GTK's gcr-prompter.

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use olo_kit::{
    Child, Cx, Element, FocusHandle, InteractiveElement as _, IntoElement as _, KeyDownEvent, ParentElement as _, SharedString, Styled as _, View, WindowHandle, WindowId, WindowOptions, div,
    instance::{self, Launch},
    metrics as m, px, tip,
    widgets::{self, AlertFrame, Button, Conversation, ConversationField, MenuField, TextField, TextFieldEvent, button_row, paragraph, paragraph_height, text_field},
};
use olo_system::{
    keyring_prompt::{self, Prompter},
    polkit::{Agent, Event, Helper, HelperEvent, Request, Subject},
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
fn height(message: &str, identities: bool) -> f32 {
    let fixed = 2.0 * m::WINDOW_INSET + m::LABEL_ROW + m::TIGHT + 3.0 * m::SPACING + 2.0 * m::CONTROL_HEIGHT + m::LABEL_ROW;
    let menu = if identities { m::CONTROL_HEIGHT + m::SPACING } else { 0.0 };
    (fixed + menu + paragraph_height(message, COLUMN)).ceil()
}

pub enum Msg {
    Polkit(Event),
    Helper(WindowId, u64, HelperEvent),
    Registered(Result<(), String>),
    Keyring(keyring_prompt::Event),
    Prompter(Result<Arc<Prompter>, String>),
    /// The self-test's question is answered.
    Checked,
    /// A security key set up waits for a touch, for this window.
    KeyWaiting(WindowId),
    /// The key's password for this window, or why not.
    Key(WindowId, Result<zeroize::Zeroizing<String>, String>),
    /// A window of ours is closing.
    Closed(WindowId),
}

/// The keyring's prompter, once it serves (a prompt's window answers
/// through it).
pub static PROMPTER: OnceLock<Arc<Prompter>> = OnceLock::new();

/// The agent, on the app's looper: its windows (one for each request and
/// keyring prompt).
#[derive(Default)]
pub struct Agents {
    /// Each request's window, and polkit's cookie for it.
    auths: HashMap<WindowId, (String, WindowHandle<Auth>)>,
    /// Each keyring prompt's window, and the prompt's id.
    pub asks: HashMap<WindowId, (u64, WindowHandle<keyring::Ask>)>,
    /// `--self-test`: quit once its one request is done.
    self_test: bool,
}

/// What a request's window is told.
pub enum AuthMsg {
    /// It has opened: its helper starts.
    Start,
    Helper(u64, HelperEvent),
    KeyWaiting,
    Key(Result<zeroize::Zeroizing<String>, String>),
    /// Polkit took the request back.
    Cancelled,
    /// Another identity was chosen.
    Identity(usize),
}

/// One request's window.
pub struct Auth {
    request: Option<Request>,
    cookie: String,
    message: SharedString,
    action_id: SharedString,
    users: Vec<SharedString>,
    user: usize,
    helper: Option<Helper>,
    /// The second helper, asking for the password while the first listens
    /// for a finger (`listening`), and whether the prompt answered is its.
    second: Option<Helper>,
    listening: bool,
    asking_second: bool,
    /// Each helper's run (`run` counts them); events from older runs are
    /// ignored.
    run: u64,
    first_run: u64,
    second_run: u64,
    /// A security key waits for a touch.
    key_waiting: bool,
    /// The error showing is the finger's (pam_fprintd's, "Failed to match
    /// fingerprint"), to go when it stops listening.
    finger_error: bool,
    conv: Conversation,
    field: Child<TextField>,
    focus: FocusHandle,
    /// A touch of a security key waited for (the person's own identity
    /// only: the password it keeps is theirs), and its answer till PAM
    /// asks.
    key: Option<olo_fido::Touch>,
    key_password: Option<zeroize::Zeroizing<String>>,
}

impl Auth {
    fn new(request: Request, cx: &mut Cx<Self>) -> Auth {
        let field = text_field(cx, |f| f.name("Password").password(true));
        cx.subscribe(&field, |a: &mut Auth, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Entered => a.submit(cx),
            _ => cx.invalidate(),
        });
        let mut conv = Conversation::default();
        if request.users.is_empty() {
            conv.error = "Nobody is allowed to authorize this.".into();
        }
        cx.send(AuthMsg::Start);
        Auth {
            cookie: request.cookie.clone(),
            message: request.message.clone().into(),
            action_id: request.action_id.clone().into(),
            users: request.users.iter().map(|u| u.clone().into()).collect(),
            user: 0,
            request: Some(request),
            helper: None,
            second: None,
            listening: false,
            asking_second: false,
            run: 0,
            first_run: 0,
            second_run: 0,
            key_waiting: false,
            finger_error: false,
            conv,
            field,
            focus: FocusHandle::new(),
            key: None,
            key_password: None,
        }
    }

    /// Wait for a touch of a key set up, while the identity chosen is the
    /// person's own (and not where nobody sits at the computer).
    fn wait_for_key(&mut self, cx: &mut Cx<Self>) {
        self.key = None;
        self.key_waiting = false;
        if !olo_system::env::key_here() {
            return;
        }
        let Some(me) = olo_system::auth::current_user() else { return };
        if self.users.get(self.user).map(|u| u.as_ref()) != Some(me.as_str()) {
            return;
        }
        let id = cx.window_id();
        let (p, proxy) = (olo_kit::proxy(), olo_kit::proxy());
        self.key = Some(olo_fido::touch(me, move || p.send(Msg::KeyWaiting(id)), move |r| proxy.send(Msg::Key(id, r.map_err(|e| e.to_string())))));
    }

    /// Say how the window can be answered (the finger alongside, a key,
    /// typing), unless PAM said something of its own.
    fn tell_ways(&mut self) {
        if self.conv.checking || !(self.conv.info.is_empty() || widgets::WAYS_IN.contains(&self.conv.info.as_str())) {
            return;
        }
        self.conv.info = widgets::ways_in(self.listening, self.key_waiting, self.conv.prompt.is_some()).unwrap_or_default().to_owned();
    }

    fn key_said(&mut self, r: Result<zeroize::Zeroizing<String>, String>, cx: &mut Cx<Self>) {
        self.key = None;
        self.key_waiting = false;
        if widgets::WAYS_IN.contains(&self.conv.info.as_str()) {
            self.conv.info.clear();
            self.tell_ways();
        }
        match r {
            Ok(password) => {
                self.key_password = Some(password);
                self.answer_with_key(cx);
            }
            Err(why) => self.conv.info = format!("{why} Type your password."),
        }
    }

    /// The key's password to the helper, if PAM is asking for a password
    /// and the key gave one (once).
    fn answer_with_key(&mut self, cx: &mut Cx<Self>) {
        let hidden = self.conv.prompt.as_ref().is_some_and(|(_, echo)| !echo);
        if !self.conv.ready() || !hidden {
            return;
        }
        let Some(password) = self.key_password.take() else { return };
        let r = self.asking().map(|h| h.respond(&password));
        self.field.update(cx, |f, cx| f.clear_secret(cx));
        self.conv.sent();
        if !matches!(r, Some(Ok(()))) {
            self.conv.checking = false;
            self.conv.error = "The authentication helper stopped.".into();
        }
        self.sync_field(cx);
    }

    /// The helper whose prompt is answered.
    fn asking(&mut self) -> Option<&mut Helper> {
        if self.asking_second { self.second.as_mut() } else { self.helper.as_mut() }
    }

    /// A helper for the chosen user, its events tagged with a new run.
    fn helper(&mut self, cx: &mut Cx<Self>) -> Option<(Helper, u64)> {
        let user = self.users.get(self.user)?.clone();
        self.run += 1;
        let (proxy, run, id) = (olo_kit::proxy(), self.run, cx.window_id());
        match Helper::start(&user, &self.cookie, move |e| proxy.send(Msg::Helper(id, run, e))) {
            Ok(h) => Some((h, run)),
            Err(e) => {
                tracing::warn!("polkit helper: {e}");
                self.conv.error = "The authentication helper couldn't be started.".into();
                None
            }
        }
    }

    /// (Re)starts the helper for the chosen user (the second, if there
    /// was one, goes).
    fn start_helper(&mut self, cx: &mut Cx<Self>) {
        self.helper = None;
        self.second = None;
        self.listening = false;
        self.asking_second = false;
        self.conv.prompt = None;
        self.conv.checking = false;
        self.conv.info.clear();
        self.sync_field(cx);
        if let Some((h, run)) = self.helper(cx) {
            self.helper = Some(h);
            self.first_run = run;
        }
        self.tell_ways();
    }

    /// The second helper: the first listens for a finger (pam_fprintd has
    /// the sensor), so the second's PAM finds it taken and asks for the
    /// password at once.
    fn start_second(&mut self, cx: &mut Cx<Self>) {
        self.second = None;
        self.asking_second = false;
        if let Some((h, run)) = self.helper(cx) {
            self.second = Some(h);
            self.second_run = run;
        }
    }

    /// The first helper is done with the finger (nothing matched, or the
    /// sensor gave up) and would ask for the password itself: the second
    /// asks instead, so the first goes.
    fn finger_over(&mut self) {
        self.helper = None;
        self.listening = false;
        if std::mem::take(&mut self.finger_error) {
            self.conv.error.clear();
        }
        if olo_system::fingerprint::listening(&self.conv.info) {
            self.conv.info.clear();
        }
        self.tell_ways();
    }

    /// The field takes typing while PAM waits for it, showing bullets
    /// unless PAM echoes; it has the keyboard then.
    fn sync_field(&mut self, cx: &mut Cx<Self>) {
        let (ready, echo) = (self.conv.ready(), self.conv.prompt.as_ref().is_some_and(|(_, e)| *e));
        self.field.update(cx, |f, cx| {
            f.set_enabled(ready, cx);
            f.set_password(!echo, cx);
        });
        if ready {
            self.field.focus(cx);
        }
    }

    fn helper_event(&mut self, run: u64, e: HelperEvent, cx: &mut Cx<Self>) {
        let first = run == self.first_run && self.helper.is_some();
        let second = run == self.second_run && self.second.is_some();
        if !first && !second {
            return;
        }
        match e {
            // Past the finger, the first would ask too: the second does.
            HelperEvent::Prompt { .. } | HelperEvent::Done(false) if first && self.second.is_some() => self.finger_over(),
            HelperEvent::Prompt { text, echo } => {
                self.asking_second = second;
                self.conv.ask(text, echo);
                self.tell_ways();
                self.sync_field(cx);
                // (The key answered before PAM asked.)
                self.answer_with_key(cx);
            }
            HelperEvent::Info(t) => {
                if first && self.second.is_none() && olo_system::fingerprint::listening(&t) {
                    // pam_fprintd has the sensor: ask for the password
                    // alongside.
                    self.listening = true;
                    self.start_second(cx);
                }
                if self.listening && olo_system::fingerprint::listening(&t) {
                    self.conv.error.clear();
                    self.conv.info.clear();
                    self.tell_ways();
                } else {
                    self.conv.info = t;
                }
            }
            HelperEvent::Error(t) => {
                self.finger_error = first && self.second.is_some();
                self.conv.error = t;
            }
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
                if second && self.helper.is_some() {
                    // Another go at the password, the finger still
                    // listened for.
                    self.conv.prompt = None;
                    self.conv.checking = false;
                    self.sync_field(cx);
                    self.start_second(cx);
                } else {
                    // Another go, as GNOME and KDE allow.
                    self.start_helper(cx);
                }
            }
        }
    }

    fn can_submit(&self, cx: &Cx<Self>) -> bool {
        self.conv.ready() && !self.field.read(cx).text().is_empty()
    }

    fn submit(&mut self, cx: &mut Cx<Self>) {
        if !self.can_submit(cx) {
            return;
        }
        let answer = self.field.read(cx).text().to_owned();
        let r = self.asking().map(|h| h.respond(&answer));
        drop(zeroed(answer));
        self.field.update(cx, |f, cx| f.clear_secret(cx));
        self.conv.sent();
        if !matches!(r, Some(Ok(()))) {
            self.conv.checking = false;
            self.conv.error = "The authentication helper stopped.".into();
        }
        self.sync_field(cx);
        cx.invalidate();
    }

    /// What it was in the middle of goes (an unanswered request is
    /// cancelled as it's dropped).
    fn let_go(&mut self, cx: &mut Cx<Self>) {
        self.key = None;
        self.helper = None;
        self.second = None;
        self.key_password = None;
        self.field.update(cx, |f, cx| f.clear_secret(cx));
    }

    /// Closes the window; an unanswered request is cancelled (dropped).
    fn close(&mut self, cx: &mut Cx<Self>) {
        self.let_go(cx);
        cx.close_window();
    }
}

/// A copy of a password, its bytes zeroed before it goes.
fn zeroed(mut s: String) -> String {
    // SAFETY: zero bytes keep the string valid UTF-8.
    unsafe { s.as_bytes_mut().fill(0) };
    s.clear();
    s
}

impl View for Auth {
    type Message = AuthMsg;

    fn message(&mut self, message: AuthMsg, cx: &mut Cx<Self>) {
        match message {
            AuthMsg::Start => {
                cx.activate_window();
                self.start_helper(cx);
                self.wait_for_key(cx);
            }
            AuthMsg::Helper(run, e) => self.helper_event(run, e, cx),
            AuthMsg::KeyWaiting => {
                self.key_waiting = true;
                self.tell_ways();
            }
            AuthMsg::Key(r) => self.key_said(r, cx),
            AuthMsg::Cancelled => return self.close(cx),
            AuthMsg::Identity(i) => {
                self.user = i;
                self.conv.error.clear();
                self.key_password = None;
                self.start_helper(cx);
                self.wait_for_key(cx);
            }
        }
        cx.invalidate();
    }

    /// Closed from its tab: as its Cancel.
    fn close_requested(&mut self, cx: &mut Cx<Self>) -> bool {
        self.let_go(cx);
        true
    }

    /// (However it closes, the agent forgets it.)
    fn window_closing(&mut self, cx: &mut Cx<Self>) {
        olo_kit::proxy().send(Msg::Closed(cx.window_id()));
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let mut form = div().flex().flex_col().gap(px(m::SPACING));
        if self.users.len() > 1 {
            form = form.child(MenuField::new("identity", self.users.clone(), self.user).label("Identity:").divider(DIVIDER).width(COLUMN).on_change(cx.msg_with(|i: &usize| AuthMsg::Identity(*i))));
        }
        form = form.child(ConversationField::new(cx, &self.conv, &self.field, DIVIDER));
        let can = self.can_submit(cx);
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|a: &mut Self, e: &KeyDownEvent, cx| {
                if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
                    a.close(cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                AlertFrame::new(ICON)
                    .child(widgets::alert_title("Authentication Required"))
                    .child(div().h(px(m::TIGHT)))
                    .child(tip::on(paragraph(self.message.clone()), "message", self.action_id.clone()))
                    .child(div().h(px(m::SPACING)))
                    .child(form)
                    .child(div().flex_1())
                    .child(
                        button_row()
                            .child(Button::new("Cancel").on_click(cx.listener(|a: &mut Self, _, cx| a.close(cx))))
                            .child(Button::new("Authenticate").default(true).enabled(can).on_click(cx.listener(|a: &mut Self, _, cx| a.submit(cx)))),
                    ),
            )
            .into_element()
    }
}

/// Opens a window for polkit's request.
fn begin(agents: &mut Agents, request: Request) {
    let h = height(&request.message, request.users.len() > 1);
    let cookie = request.cookie.clone();
    let handle = olo_kit::open_window(WindowOptions::alert("Authenticate", WIDTH, h), move |cx| Auth::new(request, cx));
    agents.auths.insert(handle.id(), (cookie, handle));
}

fn message(agents: &mut Agents, message: Box<dyn std::any::Any + Send>) {
    let message = match message.downcast::<Launch>() {
        // A second start: we are already serving the session.
        Ok(_) => return,
        Err(other) => other,
    };
    let Ok(m) = message.downcast::<Msg>() else { return };
    let to_auth = |agents: &Agents, id: WindowId, m: AuthMsg| {
        if let Some((_, window)) = agents.auths.get(&id) {
            window.messenger().send(m);
        }
    };
    match *m {
        Msg::Polkit(Event::Begin(r)) => begin(agents, r),
        Msg::Polkit(Event::Cancel(cookie)) => {
            for (c, window) in agents.auths.values() {
                if *c == cookie {
                    window.messenger().send(AuthMsg::Cancelled);
                }
            }
        }
        Msg::Helper(id, run, e) => to_auth(agents, id, AuthMsg::Helper(run, e)),
        Msg::Registered(Err(e)) => {
            eprintln!("olo-polkit-agent: couldn't register with polkit: {e}");
            olo_kit::quit();
        }
        Msg::Registered(Ok(())) => {}
        Msg::Keyring(e) => keyring::event(agents, e),
        Msg::Prompter(Ok(p)) => {
            let _ = PROMPTER.set(p);
        }
        Msg::Prompter(Err(e)) => eprintln!("olo-polkit-agent: couldn't serve the keyring's prompts: {e}"),
        Msg::KeyWaiting(id) => to_auth(agents, id, AuthMsg::KeyWaiting),
        Msg::Key(id, r) => to_auth(agents, id, AuthMsg::Key(r)),
        Msg::Closed(id) => {
            agents.auths.remove(&id);
            agents.asks.remove(&id);
            // (The self-test ends with its one request.)
            if agents.self_test && agents.auths.is_empty() {
                olo_kit::quit();
            }
        }
        // (Answered without asking, or after its window closed.)
        Msg::Checked => {
            if agents.auths.is_empty() {
                olo_kit::quit();
            }
        }
    }
}

fn main() -> std::process::ExitCode {
    olo_crash::install();
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
    if self_test.is_none() && !keyring_test && !olo_xdg::session::real() {
        eprintln!("olo-polkit-agent: not in a real Olo session (use --self-test ACTION to try it)");
        return std::process::ExitCode::FAILURE;
    }
    if self_test.is_none() && !keyring_test && instance::forward(APP_ID, &args) {
        return std::process::ExitCode::SUCCESS;
    }
    olo_kit::run(APP_ID, move |app| {
        olo_kit::stay_resident();
        let mut agents = Agents { self_test: self_test.is_some(), ..Default::default() };
        olo_kit::message_handler(app, move |m, _| message(&mut agents, m));
        if self_test.is_none() && !keyring_test {
            instance::listen(APP_ID, olo_kit::proxy());
        }
        if self_test.is_none() {
            let proxy = olo_kit::proxy();
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
        let proxy = olo_kit::proxy();
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
