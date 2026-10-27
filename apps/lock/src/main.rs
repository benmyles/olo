//! The lock screen. app_server starts it (on a socket of its own, so only
//! this process can lock and unlock) when the screen locks; it covers every
//! output, shows the time, and asks for the password or a fingerprint.
//! Unlocking ends the session lock and the process.
//!
//! Both checks run on worker threads: PAM for the password (which sleeps on
//! a failure) and fprintd for Touch ID, listening the whole time.

use std::time::Duration;

use olo_kit::{
    App, Cx, SessionLock, WindowId,
    egui::{self, Align2, Color32, FontId, Key, Rect, Sense, pos2, vec2},
    icons, metrics as m,
    style::{self, c},
    widgets::TextControl,
};
use olo_system::{auth, fingerprint};

const APP_ID: &str = "olo.lock";
const PANEL: egui::Vec2 = vec2(340.0, 150.0);

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

struct Lock {
    windows: Vec<WindowId>,
    user: String,
    name: String,
    password: String,
    status: Status,
    /// What the fingerprint sensor last said (None: not offered).
    finger: Option<String>,
    finger_stop: fingerprint::Stop,
    proxy: olo_kit::Proxy,
    unlocking: bool,
    /// Wrong attempts in a row (the field shakes on each).
    shake: Option<std::time::Instant>,
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
    const MONTHS: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let date = format!(
        "{}, {} {}",
        DAYS[tm.wday.clamp(0, 6) as usize],
        MONTHS[tm.mon.clamp(0, 11) as usize],
        tm.mday
    );
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
        let mut c = CTm {
            tm_sec: 0,
            tm_min: 0,
            tm_hour: 0,
            tm_mday: 1,
            tm_mon: 0,
            tm_year: 70,
            tm_wday: 4,
            tm_yday: 0,
            tm_isdst: 0,
            tm_gmtoff: 0,
            tm_zone: std::ptr::null(),
        };
        // SAFETY: both pointers are valid for the call; localtime_r is reentrant.
        if unsafe { localtime_r(&t, &mut c) }.is_null() {
            return;
        }
        *out = Tm { min: c.tm_min, hour: c.tm_hour, mday: c.tm_mday, mon: c.tm_mon, wday: c.tm_wday };
    }
}

impl Lock {
    fn start_fingerprint(&self) {
        let (user, stop, proxy) = (self.user.clone(), self.finger_stop.clone(), self.proxy.clone());
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

    fn submit(&mut self) {
        if self.status == Status::Checking || self.password.is_empty() {
            return;
        }
        self.status = Status::Checking;
        let (user, password, proxy) = (self.user.clone(), std::mem::take(&mut self.password), self.proxy.clone());
        std::thread::Builder::new()
            .name("pam".into())
            .spawn(move || proxy.send(Msg::Password(auth::check_password(&user, &password))))
            .expect("thread");
    }

    fn unlock(&mut self, cx: &mut Cx) {
        if self.unlocking {
            return;
        }
        self.unlocking = true;
        self.finger_stop.stop();
        cx.unlock_session();
        cx.quit();
    }

    fn redraw(&self, cx: &mut Cx) {
        for &w in &self.windows {
            cx.redraw(w);
        }
    }

    /// The time, centred in the top third.
    fn draw_clock(&self, ui: &egui::Ui, full: Rect) {
        let (time, date) = clock_text();
        let painter = ui.painter();
        let white = Color32::from_white_alpha(235);
        let y = full.top() + full.height() * 0.22;
        painter.text(pos2(full.center().x, y), Align2::CENTER_CENTER, time, FontId::new(72.0, style::bold()), white);
        painter.text(pos2(full.center().x, y + 56.0), Align2::CENTER_CENTER, date, FontId::new(20.0, style::bold()), white);
    }

    fn draw_panel(&mut self, cx: &mut Cx, ui: &mut egui::Ui, full: Rect) {
        let p = style::palette(ui.ctx());
        let mut rect = Rect::from_center_size(full.center() + vec2(0.0, 40.0), PANEL);
        // A wrong password shakes the panel, as macOS does.
        if let Some(t) = self.shake {
            let e = t.elapsed().as_secs_f32();
            if e < 0.4 {
                rect = rect.translate(vec2((e * 50.0).sin() * 10.0 * (1.0 - e / 0.4), 0.0));
                ui.ctx().request_repaint();
            } else {
                self.shake = None;
            }
        }
        let painter = ui.painter();
        painter.rect_filled(rect.expand(1.0), 0.0, c(p.frame_focused[0]));
        painter.rect_filled(rect, 0.0, c(p.panel_background));
        style::raised(painter, rect, &p);

        let inset = m::WINDOW_INSET;
        let icon = Rect::from_min_size(rect.min + vec2(inset, inset), vec2(64.0, 64.0));
        icons::paint(painter, icon, "people");
        let text_x = icon.right() + inset;
        painter.text(
            pos2(text_x, icon.top() + 8.0),
            Align2::LEFT_TOP,
            &self.name,
            FontId::new(16.0, style::bold()),
            c(p.panel_text),
        );
        let mut y = icon.top() + 30.0;
        if self.name != self.user {
            painter.text(pos2(text_x, y), Align2::LEFT_TOP, &self.user, style::plain_font(), style::disabled_text(&p));
            y += 18.0;
        }
        painter.text(pos2(text_x, y), Align2::LEFT_TOP, "This computer is locked.", style::plain_font(), c(p.panel_text));

        let field = Rect::from_min_size(
            pos2(rect.left() + inset, icon.bottom() + inset),
            vec2(rect.width() - 2.0 * inset, m::CONTROL_HEIGHT),
        );
        let checking = self.status == Status::Checking;
        let resp = ui
            .scope_builder(egui::UiBuilder::new().max_rect(field), |ui| {
                ui.add_enabled(
                    !checking,
                    TextControl::new(&mut self.password).password(true).autofocus(true).hint("Password"),
                )
            })
            .inner;
        // A single-line field gives up focus on Enter; take it straight
        // back, so typing always lands in the password.
        if !checking && resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            self.submit();
        }
        if !checking && !resp.has_focus() {
            resp.request_focus();
        }
        if resp.changed() && matches!(self.status, Status::Error(_)) {
            self.status = Status::Idle;
        }

        let (text, color) = match &self.status {
            Status::Checking => ("Checking…".to_owned(), style::disabled_text(&p)),
            Status::Error(e) => (e.clone(), c(p.failure)),
            Status::Idle => match &self.finger {
                Some(s) => (s.clone(), style::disabled_text(&p)),
                None => ("Enter your password".to_owned(), style::disabled_text(&p)),
            },
        };
        ui.painter().text(
            pos2(rect.center().x, field.bottom() + inset + 4.0),
            Align2::CENTER_CENTER,
            text,
            style::plain_font(),
            color,
        );
        let _ = cx;
    }
}

impl App for Lock {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui) {
        let full = ui.max_rect();
        // The desktop colour, dimmed: the screen is clearly locked but
        // still Olo's.
        ui.painter().rect_filled(full, 0.0, Color32::from_rgb(24, 46, 70));
        let _ = ui.interact(full, ui.make_persistent_id("lock-bg"), Sense::click());
        self.draw_clock(ui, full);
        if self.windows.first() == Some(&window) {
            self.draw_panel(cx, ui, full);
        }
        // The clock ticks on the minute.
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
        ui.ctx().request_repaint_after(Duration::from_secs(60 - secs % 60));
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        let message = match message.downcast::<SessionLock>() {
            Ok(lock) => {
                match *lock {
                    SessionLock::Locked(windows) => {
                        let first = self.windows.is_empty();
                        self.windows = windows;
                        if first {
                            self.start_fingerprint();
                        }
                    }
                    // Refused (another lock screen?) or ended: nothing to do.
                    SessionLock::Finished => cx.quit(),
                }
                self.redraw(cx);
                return;
            }
            Err(m) => m,
        };
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Password(Ok(())) => self.unlock(cx),
            Msg::Password(Err(e)) => {
                self.status = Status::Error(match e {
                    auth::AuthError::Denied(_) => "Incorrect password".into(),
                    auth::AuthError::Unavailable(m) => format!("Can't check passwords: {m}"),
                });
                self.shake = Some(std::time::Instant::now());
            }
            Msg::Finger(fingerprint::Event::Match) => self.unlock(cx),
            Msg::Finger(fingerprint::Event::Ready { swipe }) => {
                self.finger = Some(if swipe { "Swipe your finger or enter your password" } else { "Touch ID or enter your password" }.into());
            }
            Msg::Finger(fingerprint::Event::Retry(s)) => self.finger = Some(s),
            Msg::Finger(fingerprint::Event::NoMatch) => {
                self.finger = Some("Fingerprint not recognised".into());
                self.shake = Some(std::time::Instant::now());
            }
            Msg::Finger(fingerprint::Event::Failed(_)) | Msg::NoFinger => self.finger = None,
        }
        self.redraw(cx);
    }
}

fn main() {
    // A session or install from before the rename (Tabula) carries on.
    olo_kit::legacy::adopt();
    olo_kit::run(APP_ID, |cx| {
        let user = auth::current_user().unwrap_or_else(|| "user".into());
        let name = auth::full_name(&user).unwrap_or_else(|| user.clone());
        cx.lock_session();
        Lock {
            windows: Vec::new(),
            name,
            user,
            password: String::new(),
            status: Status::Idle,
            finger: None,
            finger_stop: fingerprint::Stop::default(),
            proxy: cx.proxy(),
            unlocking: false,
            shake: None,
        }
    });
}
