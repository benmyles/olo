//! The lock screen. app_server starts it (on a socket of its own, so only
//! this process can lock and unlock) when the screen locks; it covers every
//! output, shows the time, and asks for the password or a fingerprint.
//! Unlocking ends the session lock and the process.
//!
//! Both checks run on worker threads: PAM for the password (which sleeps on
//! a failure) and fprintd for Touch ID, listening the whole time.
//!
//! The lock itself (who is asked, what was answered) is kept on the app's
//! looper, apart from any window: each output's window is on a looper of
//! its own, is told what to show, and says what was typed. The first
//! output covered has the panel; if its window goes (its display
//! unplugged), the next one has it.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

use olo_kit::{
    AppCx, Bounds, Child, Cx, Element, FontWeight, InteractiveElement as _, ParentElement as _, SessionLockEvent, Styled as _, View, WindowHandle, canvas, div, icons, metrics as m, point, px, size,
    style, theme,
    widgets::{TextControl, TextField, TextFieldEvent, text_field},
};
use olo_system::{auth, fingerprint};

const APP_ID: &str = "olo.lock";
const PANEL: (f32, f32) = (340.0, 150.0);
/// How long a wrong answer shakes the panel.
const SHAKE: f32 = 0.4;

enum Msg {
    Password(Result<(), auth::AuthError>),
    Finger(fingerprint::Event),
    /// The sensor isn't there or has no prints for this user.
    NoFinger,
    /// A security key set up is plugged in and waits for a touch.
    KeyWaiting,
    /// The key's password (checked as one typed), or why not.
    Key(olo_fido::Result<zeroize::Zeroizing<String>>),
    /// How a look at the camera went (face unlock).
    Face(olo_system::face::Look),
    /// A screen says: a password was entered.
    Submit(String),
    /// A screen says: the password field's text changed.
    Typed,
    /// A screen says: someone is there (a key, the pointer).
    Activity,
    /// The displays changed: a window covering one may have gone.
    Displays,
}

/// A look at the camera at most this often (someone being there, again
/// and again, doesn't keep its light on).
const FACE_EVERY: Duration = Duration::from_secs(5);

/// Waits for a key that end with nobody touching it, in a row, before
/// the lock screen stops asking it till someone is there (a key pressed,
/// the pointer moved): a key waiting all night blinks all night.
const KEY_TRIES: u32 = 3;

/// A screen says someone is there at most this often while the pointer
/// moves over it.
const ACTIVITY_EVERY: Duration = Duration::from_secs(1);

#[derive(PartialEq)]
enum Status {
    Idle,
    Checking,
    Error(String),
}

/// The one lock, on the app's looper.
struct Lock {
    user: String,
    status: Status,
    /// What the fingerprint sensor last said (None: not offered).
    finger: Option<String>,
    finger_stop: fingerprint::Stop,
    /// A touch of a security key waited for, what to say of it, and the
    /// waits in a row that ended untouched.
    key: Option<olo_fido::Touch>,
    key_note: Option<String>,
    key_tries: u32,
    /// A look going, and when the last began.
    face_looking: bool,
    face_last: Option<Instant>,
    unlocking: bool,
    /// Every output's window (the first that's open has the panel).
    screens: Vec<(String, WindowHandle<Screen>)>,
}

fn clock_text() -> (String, String) {
    let now = olo_kit::clock::now();
    (now.time(), now.date())
}

impl Lock {
    fn start_fingerprint(&self) {
        // Not in a test: `OLO_NO_FINGERPRINT`, a debug build checking
        // passwords with a private PAM service, or a session nobody sits
        // at, leaves the reader (the person's own) alone.
        if !olo_system::env::finger_here() || cfg!(debug_assertions) && std::env::var_os("OLO_PAM_CONFDIR").is_some() {
            return;
        }
        let (user, stop, proxy) = (self.user.clone(), self.finger_stop.clone(), olo_kit::proxy());
        std::thread::Builder::new()
            .name("fingerprint".into())
            .spawn(move || {
                match fingerprint::enrolled(&user) {
                    Ok(true) => {}
                    Ok(false) | Err(_) => {
                        proxy.send(Msg::NoFinger);
                        return;
                    }
                }
                let p = proxy.clone();
                match fingerprint::verify(&user, &stop, move |e| p.send(Msg::Finger(e))) {
                    Ok(_) => {}
                    Err(e) => {
                        tracing::info!("fingerprint: {e}");
                        proxy.send(Msg::NoFinger);
                    }
                }
            })
            .expect("thread");
    }

    /// Wait for a touch of a security key set up, unless already waiting.
    fn wait_for_key(&mut self) {
        if self.key.is_some() || self.unlocking || !olo_system::env::key_here() {
            return;
        }
        let (p, proxy) = (olo_kit::proxy(), olo_kit::proxy());
        self.key = Some(olo_fido::touch(self.user.clone(), move || p.send(Msg::KeyWaiting), move |r| proxy.send(Msg::Key(r))));
    }

    /// Someone is there (a key, the pointer): the key is asked again,
    /// and the camera looks (not more often than `FACE_EVERY`).
    fn activity(&mut self) {
        self.key_tries = 0;
        self.wait_for_key();
        self.look();
    }

    /// Look for this user's face (olo-faced), on a thread.
    fn look(&mut self) {
        if self.face_looking || self.unlocking || self.face_last.is_some_and(|t| t.elapsed() < FACE_EVERY) || !olo_system::env::face_here() {
            return;
        }
        self.face_looking = true;
        self.face_last = Some(Instant::now());
        let proxy = olo_kit::proxy();
        std::thread::Builder::new().name("face".into()).spawn(move || proxy.send(Msg::Face(olo_system::face::verify(3000)))).expect("thread");
    }

    /// Check `password` through PAM, on a thread.
    fn check(&mut self, password: String) {
        self.status = Status::Checking;
        let (user, proxy) = (self.user.clone(), olo_kit::proxy());
        std::thread::Builder::new()
            .name("pam".into())
            .spawn(move || {
                let mut password = password;
                let r = auth::check_password(&user, &password);
                // SAFETY: zero bytes keep the string valid UTF-8.
                unsafe { password.as_bytes_mut().fill(0) };
                proxy.send(Msg::Password(r));
            })
            .expect("thread");
    }

    fn unlock(&mut self) {
        if self.unlocking {
            return;
        }
        self.unlocking = true;
        self.finger_stop.stop();
        self.key = None;
        // (app_server has heard by the time this returns.)
        olo_kit::unlock_session();
        olo_kit::quit();
    }

    /// A wrong answer: the panel shakes.
    fn shake(&self) {
        if let Some((_, screen)) = self.screens.iter().find(|(_, s)| s.is_open()) {
            screen.messenger().send(ScreenMsg::Shake);
        }
    }

    fn message(&mut self, msg: Msg) {
        match msg {
            Msg::Password(Ok(())) => self.unlock(),
            Msg::Password(Err(e)) => {
                self.status = Status::Error(match e {
                    auth::AuthError::Denied(_) => "Incorrect password".into(),
                    auth::AuthError::Unavailable(m) => format!("Can't check passwords: {m}"),
                });
                self.shake();
            }
            Msg::Finger(fingerprint::Event::Match) => self.unlock(),
            Msg::Finger(fingerprint::Event::Ready { swipe }) => {
                self.finger = Some(if swipe { "Swipe your finger or enter your password" } else { "Touch ID or enter your password" }.into());
            }
            Msg::Finger(fingerprint::Event::Retry(s)) => self.finger = Some(s),
            Msg::Finger(fingerprint::Event::NoMatch) => {
                self.finger = Some("Fingerprint not recognised".into());
                self.shake();
            }
            Msg::Finger(fingerprint::Event::Failed(_)) | Msg::NoFinger => self.finger = None,
            Msg::KeyWaiting => self.key_note = Some("Touch your security key or enter your password".into()),
            Msg::Key(Ok(password)) => {
                self.key = None;
                self.key_note = None;
                if self.status != Status::Checking {
                    self.check(password.to_string());
                }
            }
            Msg::Key(Err(e)) => {
                self.key = None;
                self.key_note = None;
                let untouched = matches!(e, olo_fido::Error::Timeout | olo_fido::Error::Ctap(olo_fido::ctap::ERR_USER_ACTION_TIMEOUT));
                if untouched && self.key_tries + 1 < KEY_TRIES {
                    self.key_tries += 1;
                    self.wait_for_key();
                } else if !untouched {
                    tracing::info!("security key: {e}");
                }
            }
            Msg::Face(look) => {
                self.face_looking = false;
                match look {
                    olo_system::face::Look::Match => self.unlock(),
                    olo_system::face::Look::NotYou => self.key_note = Some("Your face wasn’t recognised.".into()),
                    _ => {}
                }
            }
            Msg::Submit(password) => {
                self.activity();
                if self.status == Status::Checking || password.is_empty() {
                    let mut password = password;
                    // SAFETY: zero bytes keep the string valid UTF-8.
                    unsafe { password.as_bytes_mut().fill(0) };
                } else {
                    self.check(password);
                }
            }
            Msg::Typed => {
                self.activity();
                if matches!(self.status, Status::Error(_)) {
                    self.status = Status::Idle;
                }
            }
            Msg::Activity => {
                if self.key.is_none() {
                    self.activity();
                }
            }
            Msg::Displays => {}
        }
        self.tell_screens();
    }

    /// What shows under the field.
    fn note(&self) -> (String, bool) {
        match &self.status {
            Status::Checking => ("Checking…".to_owned(), false),
            Status::Error(e) => (e.clone(), true),
            Status::Idle => (
                match (&self.finger, &self.key_note) {
                    (Some(f), Some(_)) if f == "Touch ID or enter your password" => "Touch ID, your security key, or your password".to_owned(),
                    (Some(f), _) => f.clone(),
                    (None, Some(k)) => k.clone(),
                    (None, None) => "Enter your password".to_owned(),
                },
                false,
            ),
        }
    }

    /// Every screen is told what it shows now: the first whose window is
    /// open the panel (with the note, and whether a password can be
    /// entered), the others none.
    fn tell_screens(&mut self) {
        if self.unlocking {
            return;
        }
        self.screens.retain(|(_, screen)| screen.is_open());
        let (note, error) = self.note();
        let checking = self.status == Status::Checking;
        for (i, (_, screen)) in self.screens.iter().enumerate() {
            screen.messenger().send(ScreenMsg::Shown(Shown { panel: i == 0, note: note.clone(), error, checking }));
        }
    }
}

/// What a screen shows of the lock.
#[derive(Clone, Default, PartialEq)]
struct Shown {
    /// It has the panel asking for the password.
    panel: bool,
    note: String,
    error: bool,
    /// A password is being checked: none can be entered.
    checking: bool,
}

enum ScreenMsg {
    Shown(Shown),
    /// A wrong answer just came (the panel shakes).
    Shake,
    /// The minute changed.
    Minute,
    /// The next frame of the panel's shake.
    Frame,
}

/// One output's window: the desktop's colour dimmed, the time, and on
/// the first output the panel asking for the password.
struct Screen {
    user: String,
    name: String,
    /// The first workspace's desktop colour (as the settings had it).
    desktop: [u8; 3],
    field: Child<TextField>,
    shown: Shown,
    shake: Option<Instant>,
    /// When it last said someone is there.
    told: Cell<Option<Instant>>,
}

impl Screen {
    fn new(user: String, name: String, desktop: [u8; 3], cx: &mut Cx<Self>) -> Screen {
        let field = text_field(cx, |f| f.name("Password").password(true).hint("Password"));
        cx.subscribe(&field, |screen: &mut Screen, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Entered => screen.submit(cx),
            TextFieldEvent::Changed => olo_kit::proxy().send(Msg::Typed),
            _ => olo_kit::proxy().send(Msg::Activity),
        });
        // The clock ticks on the minute.
        cx.after(olo_kit::clock::to_next_minute(), ScreenMsg::Minute);
        Screen { user, name, desktop, field, shown: Shown::default(), shake: None, told: Cell::new(None) }
    }

    /// The password entered goes to the lock, and leaves the field.
    fn submit(&mut self, cx: &mut Cx<Self>) {
        if self.shown.checking || self.field.read(cx).text().is_empty() {
            return;
        }
        let password = self.field.read(cx).text().to_owned();
        self.field.update(cx, |f, cx| {
            f.clear_secret(cx);
            f.set_enabled(false, cx);
        });
        self.shown.checking = true;
        olo_kit::proxy().send(Msg::Submit(password));
        cx.invalidate();
    }
}

impl View for Screen {
    type Message = ScreenMsg;

    fn message(&mut self, message: ScreenMsg, cx: &mut Cx<Self>) {
        match message {
            ScreenMsg::Shown(shown) => {
                if shown == self.shown {
                    return;
                }
                let enabled = !shown.checking;
                self.shown = shown;
                if self.field.read(cx).enabled() != enabled {
                    self.field.update(cx, |f, cx| f.set_enabled(enabled, cx));
                }
            }
            ScreenMsg::Shake => self.shake = Some(Instant::now()),
            ScreenMsg::Minute => cx.after(olo_kit::clock::to_next_minute(), ScreenMsg::Minute),
            ScreenMsg::Frame => {}
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let [r, g, b] = self.desktop;
        let (time, date) = clock_text();
        let backdrop = canvas({
            let (time, date) = (time.clone(), date.clone());
            move |g2, bounds| {
                let p = theme::palette();
                // The desktop colour under the look's scrim: the screen is
                // clearly locked but still the person's.
                let dimmed = olo_theme::mix([r, g, b, 255], [p.scrim[0], p.scrim[1], p.scrim[2], 255], 0.55);
                style::fill_rect(g2, bounds, style::c(dimmed));
                let ink = style::c(p.desktop_text);
                let x = bounds.center().x;
                let y = bounds.top() + bounds.size.height * 0.22;
                style::label(g2, x, y, style::Align::Center, time.clone(), 72.0, style::ui_font(FontWeight::BOLD), ink);
                style::label(g2, x, y + px(56.0), style::Align::Center, date.clone(), 20.0, style::ui_font(FontWeight::BOLD), ink);
            }
        })
        .paint_key(style::key((&time, &date, self.desktop)))
        .absolute()
        .inset_0();
        // Someone there: the key is asked again, if it stopped waiting.
        let mut root = div().relative().size_full().child(backdrop).on_mouse_move(cx.listener(|screen: &mut Screen, _, _| {
            if screen.told.get().is_none_or(|at| at.elapsed() >= ACTIVITY_EVERY) {
                screen.told.set(Some(Instant::now()));
                olo_kit::proxy().send(Msg::Activity);
            }
        }));
        if self.shown.panel {
            root = root.child(self.panel(cx));
            // Typing always lands in the password.
            if self.field.read(cx).enabled() && !self.field.is_focused(cx) {
                self.field.focus(cx);
            }
        }
        root
    }
}

impl Screen {
    /// The panel: the person's picture and name, the field, the note;
    /// shaken for a moment after a wrong answer, as macOS does.
    fn panel(&mut self, cx: &mut Cx<Self>) -> Element {
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
        let (name, user) = (self.name.clone(), self.user.clone());
        let (note, error) = (self.shown.note.clone(), self.shown.error);
        let inset = m::WINDOW_INSET;
        let key = style::key((&name, &user, &note, error));
        let face = canvas(move |g, b| {
            let p = theme::palette();
            let outer = Bounds::from_corners(b.origin - point(px(1.0), px(1.0)), b.bottom_right() + point(px(1.0), px(1.0)));
            style::fill_rect(g, outer, style::c(p.frame_focused[0]));
            style::fill_rect(g, b, style::c(p.panel_background));
            style::raised(g, b, &p);
            let icon = Bounds { origin: b.origin + point(px(inset), px(inset)), size: size(px(64.0), px(64.0)) };
            icons::paint(g, icon, "people");
            let text_x = icon.right() + px(inset);
            let ink = style::c(p.panel_text);
            style::label(g, text_x, icon.top() + px(16.0), style::Align::Left, name.clone(), 16.0, style::ui_font(FontWeight::BOLD), ink);
            let mut y = icon.top() + px(38.0);
            if name != user {
                style::label(g, text_x, y, style::Align::Left, user.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                y += px(18.0);
            }
            style::label(g, text_x, y, style::Align::Left, "This computer is locked.", style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), ink);
            let color = if error { style::c(p.failure) } else { style::disabled_text(&p) };
            let note_y = b.top() + px(inset + 64.0 + inset + m::CONTROL_HEIGHT + inset + 4.0);
            style::label(g, b.center().x, note_y, style::Align::Center, note.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), color);
        })
        .paint_key(key)
        .absolute()
        .inset_0();
        let field = div().absolute().left(px(inset)).right(px(inset)).top(px(inset + 64.0 + inset)).h(px(m::CONTROL_HEIGHT)).child(TextControl::new(cx, &self.field));
        // Centred, a little below the middle.
        div().absolute().inset_0().flex().items_center().justify_center().child(div().relative().left(px(dx)).top(px(40.0)).w(px(PANEL.0)).h(px(PANEL.1)).child(face).child(field).id("panel"))
    }
}

fn main() {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    olo_kit::run(APP_ID, |app: &mut AppCx| {
        let user = auth::current_user().unwrap_or_else(|| "user".into());
        let name = auth::full_name(&user).unwrap_or_else(|| user.clone());
        let desktop = olo_kit::settings::current().unwrap_or_default().workspace_color(0);
        let lock = Rc::new(RefCell::new(Lock {
            user: user.clone(),
            status: Status::Idle,
            finger: None,
            finger_stop: fingerprint::Stop::default(),
            key: None,
            key_note: None,
            key_tries: 0,
            face_looking: false,
            face_last: None,
            unlocking: false,
            screens: Vec::new(),
        }));
        let l = lock.clone();
        olo_kit::message_handler(app, move |m, _| {
            if let Ok(m) = m.downcast::<Msg>() {
                l.borrow_mut().message(*m);
            }
        });
        // (A display unplugged: its window goes, and the panel with it
        // if it had it. The next screen has it then.)
        let proxy = olo_kit::proxy();
        olo_kit::on_displays_changed(move || proxy.send(Msg::Displays));
        let events = lock.clone();
        let locked = olo_kit::lock_session(
            app,
            move |_, cx| Screen::new(user.clone(), name.clone(), desktop, cx),
            move |event, screens, _| match event {
                SessionLockEvent::Locked | SessionLockEvent::Outputs => {
                    let mut lock = events.borrow_mut();
                    lock.screens = screens.to_vec();
                    lock.tell_screens();
                    if event == SessionLockEvent::Locked {
                        lock.start_fingerprint();
                        lock.wait_for_key();
                    }
                }
                // Refused (another lock screen?) or ended: nothing to do.
                SessionLockEvent::Finished => olo_kit::quit(),
            },
        );
        if let Err(e) = locked {
            eprintln!("olo-lock: {e}");
            olo_kit::quit();
        }
    });
}
