//! The lock screen. app_server starts it (on a socket of its own, so only
//! this process can lock and unlock) when the screen locks; it covers every
//! output, shows the time, and asks for the password or a fingerprint.
//! Unlocking ends the session lock and the process.
//!
//! Both checks run on worker threads: PAM for the password (which sleeps on
//! a failure) and fprintd for Touch ID, listening the whole time.

use std::time::{Duration, Instant};

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
}

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
    unlocking: bool,
    /// A wrong answer just came (the panel shakes).
    shake: Option<Instant>,
    /// The first workspace's desktop colour (as the settings had it).
    desktop: [u8; 3],
    /// Which output's window has the panel (the first covered).
    primary: Option<String>,
}

fn clock_text() -> (String, String) {
    // Local time without a tz database: the C library knows the zone.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    let mut tm: libc_tm::Tm = Default::default();
    libc_tm::localtime(now, &mut tm);
    let hour12 = match tm.hour % 12 {
        0 => 12,
        h => h,
    };
    let time = format!("{hour12}:{:02}", tm.min);
    const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let date = format!("{}, {} {}", DAYS[tm.wday.clamp(0, 6) as usize], MONTHS[tm.mon.clamp(0, 11) as usize], tm.mday);
    (time, date)
}

/// A thin wrapper over localtime_r (the kit has no calendar of its own).
mod libc_tm {
    #[derive(Default)]
    pub struct Tm {
        pub min: i32,
        pub hour: i32,
        pub mday: i32,
        pub mon: i32,
        pub wday: i32,
    }

    #[repr(C)]
    struct CTm {
        tm_sec: i32,
        tm_min: i32,
        tm_hour: i32,
        tm_mday: i32,
        tm_mon: i32,
        tm_year: i32,
        tm_wday: i32,
        tm_yday: i32,
        tm_isdst: i32,
        tm_gmtoff: i64,
        tm_zone: *const std::ffi::c_char,
    }

    unsafe extern "C" {
        fn localtime_r(t: *const i64, out: *mut CTm) -> *mut CTm;
    }

    pub fn localtime(t: i64, out: &mut Tm) {
        let mut c = CTm { tm_sec: 0, tm_min: 0, tm_hour: 0, tm_mday: 1, tm_mon: 0, tm_year: 70, tm_wday: 4, tm_yday: 0, tm_isdst: 0, tm_gmtoff: 0, tm_zone: std::ptr::null() };
        // SAFETY: both pointers are valid for the call; localtime_r is reentrant.
        if unsafe { localtime_r(&t, &mut c) }.is_null() {
            return;
        }
        *out = Tm { min: c.tm_min, hour: c.tm_hour, mday: c.tm_mday, mon: c.tm_mon, wday: c.tm_wday };
    }
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

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.status == Status::Checking || self.field.read(cx).text().is_empty() {
            return;
        }
        self.status = Status::Checking;
        let password = self.field.read(cx).text().to_owned();
        self.field.update(cx, |f, cx| {
            f.clear_secret(cx);
            f.set_enabled(false, cx);
        });
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
        }
        cx.notify();
    }

    /// What shows under the field.
    fn note(&self) -> (String, bool) {
        match &self.status {
            Status::Checking => ("Checking…".to_owned(), false),
            Status::Error(e) => (e.clone(), true),
            Status::Idle => (self.finger.clone().unwrap_or_else(|| "Enter your password".to_owned()), false),
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
        let mut root = div().relative().size_full().child(backdrop);
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
            cx.subscribe(&field, |l: &mut Lock, _, e: &TextFieldEvent, cx| match e {
                TextFieldEvent::Entered => l.submit(cx),
                TextFieldEvent::Changed if matches!(l.status, Status::Error(_)) => {
                    l.status = Status::Idle;
                    cx.notify();
                }
                _ => {}
            })
            .detach();
            Lock { user, name, field, status: Status::Idle, finger: None, finger_stop: fingerprint::Stop::default(), unlocking: false, shake: None, desktop, primary: None }
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
                let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
                cx.background_executor().timer(Duration::from_secs(60 - secs % 60)).await;
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
                SessionLockEvent::Locked => events.update(cx, |l, cx| l.start_fingerprint(cx)),
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
