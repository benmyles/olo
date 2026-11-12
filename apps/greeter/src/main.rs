//! olo-greeter: Olo's login screen, in GDM's place (see `olo_greet`).
//!
//! It runs in the login screen's own session (`olo-session --greeter`,
//! as the system's `olo-greeter` user), every display showing the
//! desktop's colour and the time, the first the panel: who (their
//! picture and name; a choice of people when there are several), their
//! password, what the ways in say (a fingerprint's "Place your finger on
//! the reader"), the session, and Restart, Shut Down and Sleep. It asks
//! nothing itself: the login daemon (`olo-login`) runs every way in at
//! once through PAM and says what each says, and the greeter shows it
//! and answers the password's question. Once the person is in it asks
//! for their session and quits, which ends its own session for theirs.
//!
//! What it knows (who, the daemon's connection, what was said) is kept
//! on the app's looper, apart from any window: each display's window is
//! on a looper of its own, the first is told what its panel shows and
//! says what was chosen and typed.

use std::{
    cell::RefCell,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    rc::Rc,
    time::Instant,
};

use olo_greet::{Event, Power, PromptKind, Request, Session, User, Way};
use olo_kit::{
    Bounds, Child, Cx, Element, FontWeight, IntoElement as _, LayerKeyboard, LayerShellLayer, ParentElement as _, SharedString, Styled as _, View, WindowHandle, WindowOptions, canvas, div, icons,
    metrics as m, point, px, size, style, theme,
    widgets::{Button, MenuField, TextControl, TextField, TextFieldEvent, text_field},
};
use serde::{Deserialize, Serialize};

const APP_ID: &str = "olo.greeter";
/// The panel's size (points).
const PANEL: (f32, f32) = (360.0, 160.0);
/// How long a wrong answer shakes the panel.
const SHAKE: f32 = 0.4;
/// A person's picture's side (points).
const PICTURE: f32 = 64.0;

enum Msg {
    Heard(Event),
    /// The daemon's connection ended (or never was).
    Gone(String),
    /// The panel says: this person was chosen.
    Choose(usize),
    /// The panel says: this session was chosen.
    Session(usize),
    /// The panel says: the password's question was answered.
    Submit(String),
    /// The panel says: the field's text changed.
    Typed,
    /// The panel says: Sleep, Restart or Shut Down was pressed.
    Power(Power),
}

/// Who logged in last, and to what (the greeter's own, kept between
/// logins).
#[derive(Default, Serialize, Deserialize)]
struct Last {
    user: Option<String>,
    session: Option<String>,
}

fn last_path() -> PathBuf {
    olo_xdg::state_dir().join("greeter.json")
}

/// The login, on the app's looper.
struct Greeter {
    to_daemon: Option<UnixStream>,
    users: Vec<User>,
    chosen: Option<usize>,
    sessions: Vec<Session>,
    session: usize,
    /// The password's question asked (the field takes an answer).
    asking: bool,
    checking: bool,
    /// What the ways in last said (and whether it's something wrong).
    note: Option<(String, bool)>,
    /// In: their session starting.
    entering: bool,
    last: Last,
    /// The first display's window: the panel's.
    panel: Option<WindowHandle<Screen>>,
}

impl Greeter {
    fn ask(&mut self, r: &Request) {
        if let Some(s) = &mut self.to_daemon {
            let line = serde_json::to_string(r).unwrap_or_default();
            if writeln!(s, "{line}").and_then(|_| s.flush()).is_err() {
                self.to_daemon = None;
            }
        }
    }

    fn user(&self) -> Option<&User> {
        self.chosen.and_then(|i| self.users.get(i))
    }

    fn to_panel(&self, message: ScreenMsg) {
        if let Some(panel) = &self.panel {
            panel.messenger().send(message);
        }
    }

    /// `i` chosen: every way in begins for them.
    fn choose(&mut self, i: usize) {
        let Some(name) = self.users.get(i).map(|u| u.name.clone()) else { return };
        self.chosen = Some(i);
        self.asking = false;
        self.checking = false;
        self.note = None;
        self.to_panel(ScreenMsg::Forget);
        self.ask(&Request::Begin { user: name });
    }

    fn submit(&mut self, password: String) {
        if !self.asking || self.checking {
            let mut password = password;
            // SAFETY: zero bytes keep the string valid UTF-8.
            unsafe { password.as_bytes_mut().fill(0) };
            return;
        }
        self.checking = true;
        self.asking = false;
        self.ask(&Request::Answer { text: Some(password) });
    }

    fn heard(&mut self, msg: Msg) {
        match msg {
            Msg::Heard(Event::Users { users }) => {
                self.users = users;
                let last = self.last.user.clone();
                let i = last.and_then(|l| self.users.iter().position(|u| u.name == l)).unwrap_or(0);
                if !self.users.is_empty() {
                    self.choose(i);
                }
            }
            Msg::Heard(Event::Sessions { sessions }) => {
                self.sessions = sessions;
                let last = self.last.session.clone();
                self.session = last.and_then(|l| self.sessions.iter().position(|s| s.id == l)).unwrap_or(0);
            }
            Msg::Heard(Event::Prompt { by, kind, text }) => match (by, kind) {
                (Way::Password, PromptKind::Secret | PromptKind::Visible) => {
                    self.asking = true;
                    self.checking = false;
                    // ("Password:" is what the field says; anything else, said.)
                    let plain = text.trim().trim_end_matches(':').eq_ignore_ascii_case("password");
                    if !plain {
                        self.note = Some((text.trim().to_owned(), false));
                    }
                }
                (_, PromptKind::Error) => self.note = Some((text.trim().to_owned(), true)),
                (_, _) => self.note = Some((text.trim().to_owned(), false)),
            },
            Msg::Heard(Event::Failed { by, reason }) => {
                if by == Way::Password {
                    // Refused just after an answer: the password was wrong
                    // (PAM's own words for it vary with its modules);
                    // before any, something else is, said as PAM says it.
                    let refused = self.checking || reason.contains("Authentication failure");
                    self.checking = false;
                    self.to_panel(ScreenMsg::Shake);
                    self.note = Some((if refused { "Incorrect password".to_owned() } else { reason }, true));
                    // Asked again (every way, from the start).
                    if let Some(i) = self.chosen {
                        let note = self.note.clone();
                        self.choose(i);
                        self.note = note;
                    }
                }
            }
            Msg::Heard(Event::Authenticated { .. }) => {
                self.entering = true;
                self.note = Some(("Welcome".to_owned(), false));
                let session = self.sessions.get(self.session).map(|s| s.id.clone());
                self.last = Last { user: self.user().map(|u| u.name.clone()), session: session.clone() };
                if let Ok(text) = serde_json::to_string(&self.last) {
                    let _ = std::fs::create_dir_all(olo_xdg::state_dir());
                    let _ = std::fs::write(last_path(), text);
                }
                match session {
                    Some(id) => self.ask(&Request::Start { session: id }),
                    None => self.note = Some(("There's no session to start".to_owned(), true)),
                }
            }
            // Theirs starts as ours ends.
            Msg::Heard(Event::Started) => olo_kit::quit(),
            Msg::Heard(Event::Error { reason }) => {
                self.entering = false;
                self.note = Some((reason, true));
            }
            Msg::Gone(why) => {
                self.to_daemon = None;
                self.note = Some((format!("The login service isn't answering ({why})"), true));
            }
            Msg::Choose(i) => self.choose(i),
            Msg::Session(i) => self.session = i,
            Msg::Submit(password) => self.submit(password),
            Msg::Typed => {
                if self.note.as_ref().is_some_and(|(_, error)| *error) {
                    self.note = None;
                }
            }
            Msg::Power(action) => self.ask(&Request::Power { action }),
        }
        self.tell_panel();
    }

    /// What shows under the field.
    fn note(&self) -> (String, bool) {
        if let Some(n) = &self.note {
            return n.clone();
        }
        if self.checking {
            return ("Checking…".to_owned(), false);
        }
        if self.to_daemon.is_none() {
            return ("Connecting to the login service…".to_owned(), false);
        }
        ("Enter your password".to_owned(), false)
    }

    /// The panel is told what it shows now.
    fn tell_panel(&self) {
        let (note, error) = self.note();
        self.to_panel(ScreenMsg::Shown(Box::new(Shown {
            users: self.users.clone(),
            chosen: self.chosen,
            sessions: self.sessions.iter().map(|s| s.name.clone().into()).collect(),
            session: self.session,
            note,
            error,
            asking: self.asking && !self.checking,
        })));
    }
}

/// What the panel shows of the login.
#[derive(Clone, Default)]
struct Shown {
    users: Vec<User>,
    chosen: Option<usize>,
    sessions: Vec<SharedString>,
    session: usize,
    note: String,
    error: bool,
    /// The password's question is asked: the field takes an answer.
    asking: bool,
}

enum ScreenMsg {
    Shown(Box<Shown>),
    /// A wrong answer just came (the panel shakes).
    Shake,
    /// Whatever is in the field is forgotten, and it takes nothing till
    /// it's asked for again.
    Forget,
    /// The minute changed.
    Minute,
    /// The next frame of the panel's shake.
    Frame,
}

/// One display's window: the desktop's colour, the time; on the first,
/// the panel, the session and the power buttons.
struct Screen {
    desktop: [u8; 3],
    /// The panel's, on the first display.
    field: Option<Child<TextField>>,
    shown: Shown,
    shake: Option<Instant>,
}

impl Screen {
    fn new(desktop: [u8; 3], panel: bool, cx: &mut Cx<Self>) -> Screen {
        let field = panel.then(|| {
            let field = text_field(cx, |f| f.name("Password").password(true).hint("Password"));
            field.update(cx, |f, cx| f.set_enabled(false, cx));
            cx.subscribe(&field, |screen: &mut Screen, e: &TextFieldEvent, cx| match e {
                TextFieldEvent::Entered => screen.submit(cx),
                TextFieldEvent::Changed => olo_kit::proxy().send(Msg::Typed),
                _ => {}
            });
            field
        });
        // The clock ticks on the minute.
        cx.after(olo_kit::clock::to_next_minute(), ScreenMsg::Minute);
        Screen { desktop, field, shown: Shown::default(), shake: None }
    }

    /// The answer goes to the login, and leaves the field.
    fn submit(&mut self, cx: &mut Cx<Self>) {
        let Some(field) = &self.field else { return };
        if !self.shown.asking {
            return;
        }
        let password = field.read(cx).text().to_owned();
        field.update(cx, |f, cx| {
            f.clear_secret(cx);
            f.set_enabled(false, cx);
        });
        self.shown.asking = false;
        olo_kit::proxy().send(Msg::Submit(password));
        cx.invalidate();
    }

    fn user(&self) -> Option<&User> {
        self.shown.chosen.and_then(|i| self.shown.users.get(i))
    }
}

impl View for Screen {
    type Message = ScreenMsg;

    fn message(&mut self, message: ScreenMsg, cx: &mut Cx<Self>) {
        match message {
            ScreenMsg::Shown(shown) => {
                self.shown = *shown;
                if let Some(field) = &self.field
                    && field.read(cx).enabled() != self.shown.asking
                {
                    let asking = self.shown.asking;
                    field.update(cx, |f, cx| f.set_enabled(asking, cx));
                }
            }
            ScreenMsg::Forget => {
                self.shown.asking = false;
                if let Some(field) = &self.field {
                    field.update(cx, |f, cx| {
                        f.clear_secret(cx);
                        f.set_enabled(false, cx);
                    });
                }
            }
            ScreenMsg::Shake => self.shake = Some(Instant::now()),
            ScreenMsg::Minute => cx.after(olo_kit::clock::to_next_minute(), ScreenMsg::Minute),
            ScreenMsg::Frame => {}
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let [r, gr, b] = self.desktop;
        let now = olo_kit::clock::now();
        let (time, date) = (now.time(), now.date());
        let backdrop = canvas({
            let (time, date) = (time.clone(), date.clone());
            move |g, bounds| {
                let p = theme::palette();
                style::fill_rect(g, bounds, style::c([r, gr, b, 255]));
                let ink = style::c(p.desktop_text);
                let x = bounds.center().x;
                let y = bounds.top() + bounds.size.height * 0.22;
                style::label(g, x, y, style::Align::Center, time.clone(), 72.0, style::ui_font(FontWeight::BOLD), ink);
                style::label(g, x, y + px(56.0), style::Align::Center, date.clone(), 20.0, style::ui_font(FontWeight::BOLD), ink);
            }
        })
        .paint_key(style::key((&time, &date, self.desktop)))
        .absolute()
        .inset_0();
        let mut root = div().relative().size_full().child(backdrop);
        // (The field out of the screen while the panel is made round it.)
        if let Some(field) = self.field.take() {
            root = root.child(self.panel(&field, cx)).child(power_row(cx));
            if field.read(cx).enabled() && !field.is_focused(cx) {
                field.focus(cx);
            }
            self.field = Some(field);
        }
        root
    }
}

impl Screen {
    /// The panel: the person's picture and name (or a choice of people),
    /// their password, what the ways in say, the session; shaken a moment
    /// after a wrong answer.
    fn panel(&mut self, field: &Child<TextField>, cx: &mut Cx<Self>) -> Element {
        let mut dx = 0.0;
        if let Some(t) = self.shake {
            let e = t.elapsed().as_secs_f32();
            if e < SHAKE {
                dx = (e * 50.0).sin() * 10.0 * (1.0 - e / SHAKE);
                // (The next frame moves it on.)
                let me = cx.view_ref();
                cx.on_next_frame(move |cx| me.send(cx, ScreenMsg::Frame));
            } else {
                self.shake = None;
            }
        }
        let inset = m::WINDOW_INSET;
        let user = self.user().cloned();
        let several = self.shown.users.len() > 1;
        let (note, error) = (self.shown.note.clone(), self.shown.error);
        let s = cx.scale_factor();
        let picture = user.as_ref().and_then(|u| u.picture.clone());
        let face_key = style::key((user.as_ref().map(|u| (&u.full_name, &u.name)), several, &note, error));
        let face = canvas(move |g, b| {
            let p = theme::palette();
            let outer = Bounds::from_corners(b.origin - point(px(1.0), px(1.0)), b.bottom_right() + point(px(1.0), px(1.0)));
            style::fill_rect(g, outer, style::c(p.frame_focused[0]));
            style::fill_rect(g, b, style::c(p.panel_background));
            style::raised(g, b, &p);
            let well = Bounds { origin: b.origin + point(px(inset), px(inset)), size: size(px(PICTURE), px(PICTURE)) };
            let text_x = well.right() + px(inset);
            let ink = style::c(p.panel_text);
            if !several && let Some(u) = &user {
                style::label(g, text_x, well.top() + px(16.0), style::Align::Left, u.full_name.clone(), 16.0, style::ui_font(FontWeight::BOLD), ink);
                if u.full_name != u.name {
                    style::label(g, text_x, well.top() + px(38.0), style::Align::Left, u.name.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                }
            }
            let color = if error { style::c(p.failure) } else { style::disabled_text(&p) };
            let note_y = b.top() + px(inset + PICTURE + inset + m::CONTROL_HEIGHT + inset + 4.0);
            let at = style::label(g, b.center().x, note_y, style::Align::Center, note.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), color);
            g.agent(olo_kit::agent::Node::label(note.clone(), at));
        })
        .paint_key(face_key)
        .absolute()
        .inset_0();
        // Their picture in a sunken well, a texture pixel to a device pixel;
        // else a person.
        let portrait_key = style::key((&picture, s.to_bits()));
        let portrait = canvas(move |g, frame| {
            let p = theme::palette();
            let well = style::inset(g, frame, 2.0);
            style::fill_rect(g, well, style::c(p.document_background));
            style::sunken(g, frame, &p, false);
            let side = PICTURE - 8.0;
            match picture.as_ref().and_then(|path| olo_kit::pictures::thumbnail(g, path, (side * s).round() as u32)) {
                Some(img) => {
                    let isz = img.size(0);
                    let (w, h) = (isz.width.0 as f32 / s, isz.height.0 as f32 / s);
                    let at = style::snap_centred(g, well.center(), w.min(side), h.min(side));
                    let _ = g.paint_image(at, olo_kit::Corners::default(), img, 0, false);
                }
                None => {
                    let at = style::snap_centred(g, well.center(), side, side);
                    icons::paint(g, at, "people");
                }
            }
        })
        .paint_key(portrait_key)
        .absolute()
        .left(px(inset))
        .top(px(inset))
        .size(px(PICTURE));
        let mut card = div().relative().left(px(dx)).top(px(40.0)).w(px(PANEL.0)).h(px(PANEL.1)).child(face).child(portrait);
        // Several people: who, chosen from a menu beside the picture.
        if several {
            let names: Vec<SharedString> = self.shown.users.iter().map(|u| u.full_name.clone().into()).collect();
            let chooser = MenuField::new("who", names, self.shown.chosen.unwrap_or(0)).on_change(|i, _| olo_kit::proxy().send(Msg::Choose(*i)));
            card = card.child(div().absolute().left(px(inset + PICTURE + inset)).right(px(inset)).top(px(inset + (PICTURE - m::CONTROL_HEIGHT) / 2.0)).child(chooser));
        }
        let control = div().absolute().left(px(inset)).right(px(inset)).top(px(inset + PICTURE + inset)).h(px(m::CONTROL_HEIGHT)).child(TextControl::new(cx, field));
        card = card.child(control);
        let mut column = div().flex().flex_col().items_center().gap(px(m::SPACING)).child(card);
        // The session, where there's a choice.
        if self.shown.sessions.len() > 1 {
            let chooser = MenuField::new("session", self.shown.sessions.clone(), self.shown.session).label("Session:").on_change(|i, _| olo_kit::proxy().send(Msg::Session(*i)));
            column = column.child(div().relative().top(px(40.0)).w(px(PANEL.0)).flex().justify_center().child(chooser));
        }
        div().absolute().inset_0().flex().items_center().justify_center().child(column)
    }
}

/// Restart, Shut Down and Sleep, along the bottom.
fn power_row(_: &mut Cx<Screen>) -> Element {
    let button = |label: &'static str, action: Power| Button::new(label).on_click(move |_, _| olo_kit::proxy().send(Msg::Power(action)));
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom(px(m::WINDOW_INSET * 2.0))
        .flex()
        .justify_center()
        .gap(px(m::SPACING))
        .child(button("Sleep", Power::Suspend))
        .child(button("Restart", Power::Restart))
        .child(button("Shut Down", Power::PowerOff))
        .into_element()
}

/// The daemon's words, read on a thread and sent to the app.
fn listen(stream: UnixStream, proxy: olo_kit::Proxy) {
    std::thread::Builder::new()
        .name("login daemon".into())
        .spawn(move || {
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                match serde_json::from_str::<Event>(&line) {
                    Ok(e) => proxy.send(Msg::Heard(e)),
                    Err(e) => tracing::warn!("from the login daemon: {e}: {line}"),
                }
            }
            proxy.send(Msg::Gone("its connection ended".into()));
        })
        .expect("thread");
}

fn main() {
    olo_crash::install();
    olo_kit::run(APP_ID, |app| {
        let desktop = olo_kit::settings::current().unwrap_or_default().workspace_color(0);
        let last: Last = std::fs::read_to_string(last_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        // A window on each display, over everything, the first with the
        // keyboard and the panel.
        let mut outputs: Vec<Option<String>> = olo_kit::display_names().into_iter().flatten().map(Some).collect();
        if outputs.is_empty() {
            outputs.push(None);
        }
        let mut panel = None;
        for (i, output) in outputs.into_iter().enumerate() {
            let window = WindowOptions::new("Login", 1.0, 1.0).layer("login", LayerShellLayer::Overlay, (true, true, true, true)).no_panel().layer_options(|l| {
                l.exclusive_zone = -1;
                l.keyboard = if i == 0 { LayerKeyboard::Exclusive } else { LayerKeyboard::None };
                l.output = output;
            });
            let handle = olo_kit::open_window(window, move |cx| Screen::new(desktop, i == 0, cx));
            if i == 0 {
                panel = Some(handle);
            }
        }
        let greeter = Rc::new(RefCell::new(Greeter {
            to_daemon: None,
            users: Vec::new(),
            chosen: None,
            sessions: Vec::new(),
            session: 0,
            asking: false,
            checking: false,
            note: None,
            entering: false,
            last,
            panel,
        }));
        let proxy = olo_kit::proxy();
        match UnixStream::connect(olo_greet::socket_path()) {
            Ok(stream) => {
                if let Ok(reader) = stream.try_clone() {
                    listen(reader, proxy);
                }
                let mut g = greeter.borrow_mut();
                g.to_daemon = Some(stream);
                g.ask(&Request::Sessions);
                g.ask(&Request::Users);
                g.tell_panel();
            }
            Err(e) => proxy.send(Msg::Gone(e.to_string())),
        }
        olo_kit::message_handler(app, move |m, _| {
            if let Ok(m) = m.downcast::<Msg>() {
                greeter.borrow_mut().heard(*m);
            }
        });
    });
}
