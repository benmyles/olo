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

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::Instant,
};

use olo_greet::{Event, Power, PromptKind, Request, Session, User, Way};
use olo_ui::{
    WindowOptions,
    gpui::{App, AppContext as _, Bounds, Context, Entity, FontWeight, IntoElement, LayerKeyboard, LayerShellLayer, ParentElement as _, Render, Styled as _, Window, canvas, div, point, px, size},
    icons, metrics as m, style, theme,
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

struct Greeter {
    to_daemon: Option<UnixStream>,
    users: Vec<User>,
    chosen: Option<usize>,
    sessions: Vec<Session>,
    session: usize,
    field: Entity<TextField>,
    /// The password's question asked (the field takes an answer).
    asking: bool,
    checking: bool,
    /// What the ways in last said (and whether it's something wrong).
    note: Option<(String, bool)>,
    /// In: their session starting.
    entering: bool,
    shake: Option<Instant>,
    desktop: [u8; 3],
    /// Which display's window has the panel (the first).
    primary: Option<String>,
    last: Last,
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

    /// `i` chosen: every way in begins for them.
    fn choose(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(name) = self.users.get(i).map(|u| u.name.clone()) else { return };
        self.chosen = Some(i);
        self.asking = false;
        self.checking = false;
        self.note = None;
        self.field.update(cx, |f, cx| {
            f.clear_secret(cx);
            f.set_enabled(false, cx);
        });
        self.ask(&Request::Begin { user: name });
        cx.notify();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.asking || self.checking {
            return;
        }
        let password = self.field.read(cx).text().to_owned();
        self.field.update(cx, |f, cx| {
            f.clear_secret(cx);
            f.set_enabled(false, cx);
        });
        self.checking = true;
        self.asking = false;
        self.ask(&Request::Answer { text: Some(password) });
        cx.notify();
    }

    fn heard(&mut self, msg: Msg, cx: &mut Context<Self>) {
        match msg {
            Msg::Heard(Event::Users { users }) => {
                self.users = users;
                let last = self.last.user.clone();
                let i = last.and_then(|l| self.users.iter().position(|u| u.name == l)).unwrap_or(0);
                if !self.users.is_empty() {
                    self.choose(i, cx);
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
                    self.field.update(cx, |f, cx| f.set_enabled(true, cx));
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
                    self.shake = Some(Instant::now());
                    self.note = Some((if refused { "Incorrect password".to_owned() } else { reason }, true));
                    // Asked again (every way, from the start).
                    if let Some(i) = self.chosen {
                        let note = self.note.clone();
                        self.choose(i, cx);
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
            Msg::Heard(Event::Started) => cx.quit(),
            Msg::Heard(Event::Error { reason }) => {
                self.entering = false;
                self.note = Some((reason, true));
            }
            Msg::Gone(why) => {
                self.to_daemon = None;
                self.note = Some((format!("The login service isn't answering ({why})"), true));
            }
        }
        cx.notify();
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
}

/// One display's window: the desktop's colour, the time; on the first,
/// the panel, the session and the power buttons.
struct Screen {
    greeter: Entity<Greeter>,
    output: Option<String>,
}

impl Render for Screen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let g = self.greeter.read(cx);
        let primary = g.primary == self.output || g.primary.is_none();
        let [r, gr, b] = g.desktop;
        let backdrop = canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                let p = theme::palette(cx);
                style::fill_rect(window, bounds, style::c([r, gr, b, 255]));
                let now = olo_ui::clock::now();
                let ink = style::c(p.desktop_text);
                let x = bounds.center().x;
                let y = bounds.top() + bounds.size.height * 0.22;
                style::label(window, cx, x, y, style::Align::Center, now.time(), 72.0, style::ui_font(FontWeight::BOLD), ink);
                style::label(window, cx, x, y + px(56.0), style::Align::Center, now.date(), 20.0, style::ui_font(FontWeight::BOLD), ink);
            },
        )
        .absolute()
        .inset_0();
        let mut root = div().relative().size_full().child(backdrop);
        if primary {
            root = root.child(panel(&self.greeter, g, window, cx)).child(power_row(&self.greeter));
            let field = g.field.read(cx);
            if field.enabled() && !field.is_focused(window) {
                let handle = olo_ui::gpui::Focusable::focus_handle(&g.field, cx);
                window.focus(&handle);
            }
        }
        root
    }
}

/// The panel: the person's picture and name (or a choice of people),
/// their password, what the ways in say, the session; shaken a moment
/// after a wrong answer.
fn panel(greeter: &Entity<Greeter>, g: &Greeter, window: &mut Window, cx: &App) -> impl IntoElement {
    let mut dx = 0.0;
    if let Some(t) = g.shake {
        let e = t.elapsed().as_secs_f32();
        if e < SHAKE {
            dx = (e * 50.0).sin() * 10.0 * (1.0 - e / SHAKE);
            window.request_animation_frame();
        }
    }
    let inset = m::WINDOW_INSET;
    let user = g.user().cloned();
    let several = g.users.len() > 1;
    let (note, error) = g.note();
    let s = window.scale_factor();
    let picture = user.as_ref().and_then(|u| u.picture.clone());
    let face = canvas(
        |_, _, _| {},
        move |b, _, window, cx| {
            let p = theme::palette(cx);
            let outer = Bounds::from_corners(b.origin - point(px(1.0), px(1.0)), b.bottom_right() + point(px(1.0), px(1.0)));
            style::fill_rect(window, outer, style::c(p.frame_focused[0]));
            style::fill_rect(window, b, style::c(p.panel_background));
            style::raised(window, b, &p);
            let well = Bounds { origin: b.origin + point(px(inset), px(inset)), size: size(px(PICTURE), px(PICTURE)) };
            let text_x = well.right() + px(inset);
            let ink = style::c(p.panel_text);
            if !several && let Some(u) = &user {
                style::label(window, cx, text_x, well.top() + px(16.0), style::Align::Left, u.full_name.clone(), 16.0, style::ui_font(FontWeight::BOLD), ink);
                if u.full_name != u.name {
                    style::label(window, cx, text_x, well.top() + px(38.0), style::Align::Left, u.name.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                }
            }
            let color = if error { style::c(p.failure) } else { style::disabled_text(&p) };
            let note_y = b.top() + px(inset + PICTURE + inset + m::CONTROL_HEIGHT + inset + 4.0);
            let at = style::label(window, cx, b.center().x, note_y, style::Align::Center, note.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), color);
            olo_ui::agent::record(window, cx, olo_ui::agent::Node::label(note.clone(), at));
        },
    )
    .absolute()
    .inset_0();
    // Their picture in a sunken well, a texture pixel to a device pixel;
    // else a person.
    let portrait = canvas(
        |_, _, _| {},
        move |frame, _, window, cx| {
            let p = theme::palette(cx);
            let well = style::inset(window, frame, 2.0);
            style::fill_rect(window, well, style::c(p.document_background));
            style::sunken(window, frame, &p, false);
            let side = PICTURE - 8.0;
            match picture.as_ref().and_then(|path| olo_ui::pictures::thumbnail(cx, path, (side * s).round() as u32)) {
                Some(img) => {
                    let isz = img.size(0);
                    let (w, h) = (isz.width.0 as f32 / s, isz.height.0 as f32 / s);
                    let at = style::snap_centred(window, well.center(), w.min(side), h.min(side));
                    let _ = window.paint_image(at, olo_ui::gpui::Corners::default(), img, 0, false);
                }
                None => {
                    icons::paint(window, style::snap_centred(window, well.center(), side, side), "people");
                }
            }
        },
    )
    .absolute()
    .left(px(inset))
    .top(px(inset))
    .size(px(PICTURE));
    let mut card = div().relative().left(px(dx)).top(px(40.0)).w(px(PANEL.0)).h(px(PANEL.1)).child(face).child(portrait);
    // Several people: who, chosen from a menu beside the picture.
    if several {
        let names: Vec<olo_ui::gpui::SharedString> = g.users.iter().map(|u| u.full_name.clone().into()).collect();
        let me = greeter.clone();
        let chooser = MenuField::new("who", names, g.chosen.unwrap_or(0)).on_change(move |i, _, cx| me.update(cx, |g, cx| g.choose(i, cx)));
        card = card.child(div().absolute().left(px(inset + PICTURE + inset)).right(px(inset)).top(px(inset + (PICTURE - m::CONTROL_HEIGHT) / 2.0)).child(chooser));
    }
    let field = div().absolute().left(px(inset)).right(px(inset)).top(px(inset + PICTURE + inset)).h(px(m::CONTROL_HEIGHT)).child(TextControl::new(g.field.clone()));
    card = card.child(field);
    let mut column = div().flex().flex_col().items_center().gap(px(m::SPACING)).child(card);
    // The session, where there's a choice.
    if g.sessions.len() > 1 {
        let names: Vec<olo_ui::gpui::SharedString> = g.sessions.iter().map(|s| s.name.clone().into()).collect();
        let me = greeter.clone();
        let chooser = MenuField::new("session", names, g.session).label("Session:").on_change(move |i, _, cx| {
            me.update(cx, |g, cx| {
                g.session = i;
                cx.notify();
            })
        });
        column = column.child(div().relative().top(px(40.0)).w(px(PANEL.0)).flex().justify_center().child(chooser));
    }
    let _ = cx;
    div().absolute().inset_0().flex().items_center().justify_center().child(column)
}

/// Restart, Shut Down and Sleep, along the bottom.
fn power_row(greeter: &Entity<Greeter>) -> impl IntoElement {
    let button = |label: &'static str, action: Power| {
        let me = greeter.clone();
        Button::new(label).on_click(move |_, cx| me.update(cx, |g, _| g.ask(&Request::Power { action })))
    };
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
}

/// The daemon's words, read on a thread and sent to the app.
fn listen(stream: UnixStream, proxy: olo_ui::Proxy) {
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
    olo_ui::run(APP_ID, |cx| {
        let field = text_field(cx, |f| f.name("Password").password(true).hint("Password"));
        field.update(cx, |f, cx| f.set_enabled(false, cx));
        let desktop = olo_ui::settings::current().unwrap_or_default().workspace_color(0);
        let last: Last = std::fs::read_to_string(last_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let to_daemon = UnixStream::connect(olo_greet::socket_path());
        let proxy = olo_ui::proxy(cx);
        let greeter = cx.new(|cx| {
            cx.subscribe(&field, |g: &mut Greeter, _, e: &TextFieldEvent, cx| match e {
                TextFieldEvent::Entered => g.submit(cx),
                TextFieldEvent::Changed if g.note.as_ref().is_some_and(|(_, error)| *error) => {
                    g.note = None;
                    cx.notify();
                }
                _ => {}
            })
            .detach();
            Greeter {
                to_daemon: None,
                users: Vec::new(),
                chosen: None,
                sessions: Vec::new(),
                session: 0,
                field,
                asking: false,
                checking: false,
                note: None,
                entering: false,
                shake: None,
                desktop,
                primary: None,
                last,
            }
        });
        match to_daemon {
            Ok(stream) => {
                if let Ok(reader) = stream.try_clone() {
                    listen(reader, proxy);
                }
                greeter.update(cx, |g, _| {
                    g.to_daemon = Some(stream);
                    g.ask(&Request::Sessions);
                    g.ask(&Request::Users);
                });
            }
            Err(e) => proxy.send(Msg::Gone(e.to_string())),
        }
        let g = greeter.clone();
        olo_ui::message_handler(cx, move |m, cx| {
            if let Ok(m) = m.downcast::<Msg>() {
                g.update(cx, |g, cx| g.heard(*m, cx));
            }
        });
        // The clock ticks on the minute.
        let g = greeter.clone();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(olo_ui::clock::to_next_minute()).await;
                if g.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        // A window on each display, over everything, the first with the
        // keyboard and the panel.
        let mut outputs: Vec<Option<String>> = cx.displays().iter().filter_map(|d| d.name()).map(Some).collect();
        if outputs.is_empty() {
            outputs.push(None);
        }
        greeter.update(cx, |g, _| g.primary = outputs[0].clone());
        for (i, output) in outputs.into_iter().enumerate() {
            let greeter = greeter.clone();
            let window = WindowOptions::new("Login", 1.0, 1.0).layer("login", LayerShellLayer::Overlay, (true, true, true, true)).no_panel().layer_options(|l| {
                l.exclusive_zone = -1;
                l.keyboard = if i == 0 { LayerKeyboard::Exclusive } else { LayerKeyboard::None };
                l.output = output.clone();
            });
            olo_ui::open_window(cx, window, move |_, cx| {
                cx.new(|cx| {
                    cx.observe(&greeter, |_, _, cx| cx.notify()).detach();
                    Screen { greeter, output }
                })
            });
        }
    });
}
