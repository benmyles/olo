//! The lock screen. app_server starts it (on a socket of its own, so only
//! this process can lock and unlock) when the screen locks; it covers every
//! output, shows the time, and asks for the password or a fingerprint.
//! Unlocking ends the session lock and the process.
//!
//! Both checks run on worker threads: PAM for the password (which sleeps on
//! a failure) and fprintd for Touch ID, listening the whole time.

use std::time::Instant;

use olo_system::{auth, fingerprint};
use olo_ui::{
    gpui::{
        App, AppContext as _, Bounds, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render, SessionLockEvent, Styled as _, Window, canvas, div, point, px,
        size,
    },
    icons, metrics as m, style, theme,
    widgets::{TextControl, TextField, TextFieldEvent, text_field},
};

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
}

/// A look at the camera at most this often (someone being there, again
/// and again, doesn't keep its light on).
const FACE_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

/// Waits for a key that end with nobody touching it, in a row, before
/// the lock screen stops asking it till someone is there (a key pressed,
/// the pointer moved): a key waiting all night blinks all night.
const KEY_TRIES: u32 = 3;

#[derive(PartialEq)]
enum Status {
    Idle,
    Checking,
    Error(String),
}

/// What every output's window shows: one lock.
struct Lock {
    user: String,
    name: String,
    field: Entity<TextField>,
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
    /// A wrong answer just came (the panel shakes).
    shake: Option<Instant>,
    /// The first workspace's desktop colour (as the settings had it).
    desktop: [u8; 3],
    /// Which output's window has the panel (the first covered).
    primary: Option<String>,
}

fn clock_text() -> (String, String) {
    let now = olo_ui::clock::now();
    (now.time(), now.date())
}

impl Lock {
    fn start_fingerprint(&self, cx: &App) {
        // Not in a test: `OLO_NO_FINGERPRINT`, or a debug build checking
        // passwords with a private PAM service, leaves the reader (the
        // person's own) alone.
        if std::env::var_os("OLO_NO_FINGERPRINT").is_some() || cfg!(debug_assertions) && std::env::var_os("OLO_PAM_CONFDIR").is_some() {
            return;
        }
        let (user, stop, proxy) = (self.user.clone(), self.finger_stop.clone(), olo_ui::proxy(cx));
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
    fn wait_for_key(&mut self, cx: &App) {
        if self.key.is_some() || self.unlocking {
            return;
        }
        let (p, proxy) = (olo_ui::proxy(cx), olo_ui::proxy(cx));
        self.key = Some(olo_fido::touch(self.user.clone(), move || p.send(Msg::KeyWaiting), move |r| proxy.send(Msg::Key(r))));
    }

    /// Someone is there (a key, the pointer): the key is asked again,
    /// and the camera looks (not more often than `FACE_EVERY`).
    fn activity(&mut self, cx: &App) {
        self.key_tries = 0;
        self.wait_for_key(cx);
        self.look(cx);
    }

    /// Look for this user's face (olo-faced), on a thread.
    fn look(&mut self, cx: &App) {
        if self.face_looking || self.unlocking || self.face_last.is_some_and(|t| t.elapsed() < FACE_EVERY) {
            return;
        }
        self.face_looking = true;
        self.face_last = Some(Instant::now());
        let proxy = olo_ui::proxy(cx);
        std::thread::Builder::new().name("face".into()).spawn(move || proxy.send(Msg::Face(olo_system::face::verify(3000)))).expect("thread");
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.status == Status::Checking || self.field.read(cx).text().is_empty() {
            return;
        }
        let password = self.field.read(cx).text().to_owned();
        self.field.update(cx, |f, cx| {
            f.clear_secret(cx);
            f.set_enabled(false, cx);
        });
        self.check(password, cx);
    }

    /// Check `password` through PAM, on a thread.
    fn check(&mut self, password: String, cx: &mut Context<Self>) {
        self.status = Status::Checking;
        let (user, proxy) = (self.user.clone(), olo_ui::proxy(cx));
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
        cx.notify();
    }

    fn unlock(&mut self, cx: &mut Context<Self>) {
        if self.unlocking {
            return;
        }
        self.unlocking = true;
        self.finger_stop.stop();
        self.key = None;
        olo_ui::unlock_session();
        cx.quit();
    }

    fn message(&mut self, msg: Msg, cx: &mut Context<Self>) {
        match msg {
            Msg::Password(Ok(())) => self.unlock(cx),
            Msg::Password(Err(e)) => {
                self.status = Status::Error(match e {
                    auth::AuthError::Denied(_) => "Incorrect password".into(),
                    auth::AuthError::Unavailable(m) => format!("Can't check passwords: {m}"),
                });
                self.shake = Some(Instant::now());
                self.field.update(cx, |f, cx| f.set_enabled(true, cx));
            }
            Msg::Finger(fingerprint::Event::Match) => self.unlock(cx),
            Msg::Finger(fingerprint::Event::Ready { swipe }) => {
                self.finger = Some(if swipe { "Swipe your finger or enter your password" } else { "Touch ID or enter your password" }.into());
            }
            Msg::Finger(fingerprint::Event::Retry(s)) => self.finger = Some(s),
            Msg::Finger(fingerprint::Event::NoMatch) => {
                self.finger = Some("Fingerprint not recognised".into());
                self.shake = Some(Instant::now());
            }
            Msg::Finger(fingerprint::Event::Failed(_)) | Msg::NoFinger => self.finger = None,
            Msg::KeyWaiting => self.key_note = Some("Touch your security key or enter your password".into()),
            Msg::Key(Ok(password)) => {
                self.key = None;
                self.key_note = None;
                if self.status != Status::Checking {
                    self.check(password.to_string(), cx);
                }
            }
            Msg::Key(Err(e)) => {
                self.key = None;
                self.key_note = None;
                let untouched = matches!(e, olo_fido::Error::Timeout | olo_fido::Error::Ctap(olo_fido::ctap::ERR_USER_ACTION_TIMEOUT));
                if untouched && self.key_tries + 1 < KEY_TRIES {
                    self.key_tries += 1;
                    self.wait_for_key(cx);
                } else if !untouched {
                    tracing::info!("security key: {e}");
                }
            }
            Msg::Face(look) => {
                self.face_looking = false;
                match look {
                    olo_system::face::Look::Match => self.unlock(cx),
                    olo_system::face::Look::NotYou => self.key_note = Some("Your face wasn’t recognised.".into()),
                    _ => {}
                }
            }
        }
        cx.notify();
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
}

/// One output's window: the desktop's colour dimmed, the time, and on
/// the first output the panel asking for the password.
struct Screen {
    lock: Entity<Lock>,
    output: String,
}

impl Render for Screen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lock = self.lock.read(cx);
        let primary = lock.primary.as_deref() == Some(self.output.as_str());
        let [r, g, b] = lock.desktop;
        let backdrop = canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                let p = theme::palette(cx);
                // The desktop colour under the look's scrim: the screen is
                // clearly locked but still the person's.
                let dimmed = olo_theme::mix([r, g, b, 255], [p.scrim[0], p.scrim[1], p.scrim[2], 255], 0.55);
                style::fill_rect(window, bounds, style::c(dimmed));
                let (time, date) = clock_text();
                let ink = style::c(p.desktop_text);
                let x = bounds.center().x;
                let y = bounds.top() + bounds.size.height * 0.22;
                style::label(window, cx, x, y, style::Align::Center, time, 72.0, style::ui_font(FontWeight::BOLD), ink);
                style::label(window, cx, x, y + px(56.0), style::Align::Center, date, 20.0, style::ui_font(FontWeight::BOLD), ink);
            },
        )
        .absolute()
        .inset_0();
        // Someone there: the key is asked again, if it stopped waiting.
        let l = self.lock.clone();
        let mut root = div().relative().size_full().child(backdrop).on_mouse_move(move |_, _, cx| {
            if l.read(cx).key.is_none() {
                l.update(cx, |l, cx| l.activity(cx));
            }
        });
        if primary {
            root = root.child(panel(lock, window));
            // Typing always lands in the password.
            let field = lock.field.read(cx);
            if field.enabled() && !field.is_focused(window) {
                let handle = olo_ui::gpui::Focusable::focus_handle(&lock.field, cx);
                window.focus(&handle);
            }
        }
        root
    }
}

/// The panel: the person's picture and name, the field, the note; shaken
/// for a moment after a wrong answer, as macOS does.
fn panel(lock: &Lock, window: &mut Window) -> impl IntoElement {
    let mut dx = 0.0;
    if let Some(t) = lock.shake {
        let e = t.elapsed().as_secs_f32();
        if e < SHAKE {
            dx = (e * 50.0).sin() * 10.0 * (1.0 - e / SHAKE);
            window.request_animation_frame();
        }
    }
    let (name, user) = (lock.name.clone(), lock.user.clone());
    let (note, error) = lock.note();
    let inset = m::WINDOW_INSET;
    let face = canvas(
        |_, _, _| {},
        move |b, _, window, cx| {
            let p = theme::palette(cx);
            let outer = Bounds::from_corners(b.origin - point(px(1.0), px(1.0)), b.bottom_right() + point(px(1.0), px(1.0)));
            style::fill_rect(window, outer, style::c(p.frame_focused[0]));
            style::fill_rect(window, b, style::c(p.panel_background));
            style::raised(window, b, &p);
            let icon = Bounds { origin: b.origin + point(px(inset), px(inset)), size: size(px(64.0), px(64.0)) };
            icons::paint(window, icon, "people");
            let text_x = icon.right() + px(inset);
            let ink = style::c(p.panel_text);
            style::label(window, cx, text_x, icon.top() + px(16.0), style::Align::Left, name.clone(), 16.0, style::ui_font(FontWeight::BOLD), ink);
            let mut y = icon.top() + px(38.0);
            if name != user {
                style::label(window, cx, text_x, y, style::Align::Left, user.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                y += px(18.0);
            }
            style::label(window, cx, text_x, y, style::Align::Left, "This computer is locked.", style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), ink);
            let color = if error { style::c(p.failure) } else { style::disabled_text(&p) };
            let note_y = b.top() + px(inset + 64.0 + inset + m::CONTROL_HEIGHT + inset + 4.0);
            style::label(window, cx, b.center().x, note_y, style::Align::Center, note.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), color);
        },
    )
    .absolute()
    .inset_0();
    let field = div().absolute().left(px(inset)).right(px(inset)).top(px(inset + 64.0 + inset)).h(px(m::CONTROL_HEIGHT)).child(TextControl::new(lock.field.clone()));
    // Centred, a little below the middle.
    div().absolute().inset_0().flex().items_center().justify_center().child(div().relative().left(px(dx)).top(px(40.0)).w(px(PANEL.0)).h(px(PANEL.1)).child(face).child(field).id("panel"))
}

fn main() {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    olo_ui::run(APP_ID, |cx| {
        let user = auth::current_user().unwrap_or_else(|| "user".into());
        let name = auth::full_name(&user).unwrap_or_else(|| user.clone());
        let field = text_field(cx, |f| f.name("Password").password(true).hint("Password"));
        let desktop = olo_ui::settings::current().unwrap_or_default().workspace_color(0);
        let lock = cx.new(|cx| {
            cx.subscribe(&field, |l: &mut Lock, _, e: &TextFieldEvent, cx| {
                l.activity(cx);
                match e {
                    TextFieldEvent::Entered => l.submit(cx),
                    TextFieldEvent::Changed if matches!(l.status, Status::Error(_)) => {
                        l.status = Status::Idle;
                        cx.notify();
                    }
                    _ => {}
                }
            })
            .detach();
            Lock {
                user,
                name,
                field,
                status: Status::Idle,
                finger: None,
                finger_stop: fingerprint::Stop::default(),
                key: None,
                key_note: None,
                key_tries: 0,
                face_looking: false,
                face_last: None,
                unlocking: false,
                shake: None,
                desktop,
                primary: None,
            }
        });
        let l = lock.clone();
        olo_ui::message_handler(cx, move |m, cx| {
            if let Ok(m) = m.downcast::<Msg>() {
                l.update(cx, |l, cx| l.message(*m, cx));
            }
        });
        // The clock ticks on the minute.
        let l = lock.clone();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(olo_ui::clock::to_next_minute()).await;
                if l.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        let (l, events) = (lock.clone(), lock.clone());
        let locked = olo_ui::lock_session(
            cx,
            move |output, _, cx| {
                let lock = l.clone();
                lock.update(cx, |l, _| {
                    if l.primary.is_none() {
                        l.primary = Some(output.to_owned());
                    }
                });
                let screen = cx.new(|cx| {
                    cx.observe(&lock, |_, _, cx| cx.notify()).detach();
                    Screen { lock: lock.clone(), output: output.to_owned() }
                });
                screen
            },
            move |event, cx| match event {
                SessionLockEvent::Locked => events.update(cx, |l, cx| {
                    l.start_fingerprint(cx);
                    l.wait_for_key(cx);
                }),
                // Refused (another lock screen?) or ended: nothing to do.
                _ => cx.quit(),
            },
        );
        if let Err(e) = locked {
            eprintln!("olo-lock: {e}");
            cx.quit();
        }
    });
}
