//! Integrations ▸ Telegram: the API ID and hash a Telegram app signs in
//! with (the person's own, from my.telegram.org), and signing in (a code
//! sent to their phone or Telegram, or a QR code scanned with Telegram on
//! a phone, then the two-step password if the account has one) or out.
//! The session goes to the login keyring (`olo_tg::account`); Telegram
//! (`olo-telegram`), if it's running, is told to use it at once. All of
//! it talks to the keyring and Telegram on workers.

use olo_sync::Mutex;
use std::sync::Arc;

use olo_kit::{
    egui::{Color32, Context, Key, Rect, Sense, Ui, pos2, vec2},
    icons, metrics as m, style,
    widgets::{BBox, Button, StringView, TextControl},
};
use crate::integrations::buttons;
use olo_tg::{
    account::{self, Api},
    signin::{SignIn, Step, Update},
};

const MY_TELEGRAM: &str = "https://my.telegram.org/apps";

#[derive(Clone, Debug, Default, PartialEq)]
enum Known {
    #[default]
    Checking,
    /// No API ID kept.
    NoApi,
    SignedOut(Api),
    SignedIn(Api, olo_tg::Me),
    Failed(String),
}

#[derive(Default)]
pub struct State {
    known: Arc<Mutex<Known>>,
    started: bool,
    /// Typing an API ID and hash.
    api_id: String,
    api_hash: String,
    editing_api: bool,
    /// Signing in: the flow, where it is, and what's typed.
    signin: Option<SignIn>,
    step: Arc<Mutex<Option<Update>>>,
    phone: String,
    code: String,
    password: String,
    problem: Option<String>,
}

fn status() -> Known {
    match account::api() {
        Err(e) => Known::Failed(format!("The keyring couldn’t be read ({e}).")),
        Ok(None) => Known::NoApi,
        Ok(Some(api)) => match account::signed_in() {
            Ok(Some(me)) => Known::SignedIn(api, me),
            Ok(None) => Known::SignedOut(api),
            Err(e) => Known::Failed(format!("The keyring couldn’t be read ({e}).")),
        },
    }
}

/// Tell a running Telegram to use the account as it is now.
fn tell_telegram() {
    if olo_kit::instance::running("olo.telegram") {
        olo_kit::instance::forward("olo.telegram", &["--reconnect".to_owned()]);
    }
}

impl State {
    fn check(&mut self, ctx: &Context) {
        self.started = true;
        let (out, ctx) = (self.known.clone(), ctx.clone());
        std::thread::spawn(move || {
            *out.lock().unwrap() = status();
            ctx.request_repaint();
        });
    }

    fn save_api(&mut self, ctx: &Context) {
        let Ok(id) = self.api_id.trim().parse::<i32>() else {
            self.problem = Some("The API ID is a number.".into());
            return;
        };
        let api = Api { id, hash: self.api_hash.trim().to_owned() };
        if api.hash.len() != 32 || !api.hash.chars().all(|c| c.is_ascii_hexdigit()) {
            self.problem = Some("The API hash is 32 letters and digits (0–9, a–f).".into());
            return;
        }
        self.problem = None;
        self.editing_api = false;
        let (out, ctx) = (self.known.clone(), ctx.clone());
        *out.lock().unwrap() = Known::Checking;
        std::thread::spawn(move || {
            *out.lock().unwrap() = match account::save_api(&api) {
                Ok(()) => status(),
                Err(e) => Known::Failed(format!("The API ID couldn’t be kept in the keyring ({e}).")),
            };
            ctx.request_repaint();
        });
    }

    fn start_signin(&mut self, ctx: &Context, api: Api) {
        let (step, ctx2) = (self.step.clone(), ctx.clone());
        *step.lock().unwrap() = Some(Update { step: Step::Starting, problem: None });
        self.code.clear();
        self.password.clear();
        self.problem = None;
        self.signin = Some(SignIn::start(api, move |u| {
            *step.lock().unwrap() = Some(u);
            ctx2.request_repaint();
        }));
    }

    fn end_signin(&mut self, ctx: &Context) {
        self.signin = None;
        *self.step.lock().unwrap() = None;
        self.check(ctx);
    }

    fn sign_out(&mut self, ctx: &Context) {
        let (out, ctx) = (self.known.clone(), ctx.clone());
        *out.lock().unwrap() = Known::Checking;
        std::thread::spawn(move || {
            let r = olo_tg::signin::sign_out();
            *out.lock().unwrap() = match r {
                Ok(()) => status(),
                Err(e) => Known::Failed(format!("Couldn’t sign out ({e}).")),
            };
            tell_telegram();
            ctx.request_repaint();
        });
    }
}

/// `url` as a QR code in `r`, black on white with a quiet zone.
fn qr(ui: &Ui, r: Rect, url: &str) {
    let Ok(code) = qrcode::QrCode::new(url.as_bytes()) else { return };
    let n = code.width();
    let colors = code.to_colors();
    let p = ui.painter();
    p.rect_filled(r, 8.0, Color32::WHITE);
    let ppp = ui.ctx().pixels_per_point();
    // Whole device pixels per module, so its edges are crisp.
    let module = (((r.width() - 16.0) / n as f32) * ppp).floor().max(1.0) / ppp;
    let size = module * n as f32;
    let origin = pos2(((r.center().x - size / 2.0) * ppp).round() / ppp, ((r.center().y - size / 2.0) * ppp).round() / ppp);
    for y in 0..n {
        for x in 0..n {
            if colors[y * n + x] == qrcode::Color::Dark {
                p.rect_filled(Rect::from_min_size(origin + vec2(x as f32 * module, y as f32 * module), vec2(module, module)), 0.0, Color32::BLACK);
            }
        }
    }
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let ctx = ui.ctx().clone();
    if !st.started {
        st.check(&ctx);
    }
    let known = st.known.lock().unwrap().clone();
    let step = st.step.lock().unwrap().clone();
    if let Some(Update { step: Step::Done(_), .. }) = &step {
        tell_telegram();
        st.end_signin(&ctx);
    }
    BBox::new("Telegram").show(ui, |ui| {
        let status = match &known {
            Known::Checking => "Checking…".to_owned(),
            Known::NoApi => "Not set up: Telegram needs an API ID to sign in with.".to_owned(),
            Known::SignedOut(api) => format!("Not signed in. (API ID {}.)", api.id),
            Known::SignedIn(_, me) => match &me.phone {
                Some(p) => format!("Signed in as {} ({p}).", me.name),
                None => format!("Signed in as {}.", me.name),
            },
            Known::Failed(why) => why.clone(),
        };
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = m::SPACING;
            let (r, _) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::hover());
            icons::paint(ui.painter(), r, "telegram");
            ui.vertical(|ui| {
                ui.add(StringView::new("Telegram (in Applications) shows your chats and messages, and notifies you of new ones. Olo keeps the sign-in in your login keyring.").dim(true).wrap(true));
                ui.add(StringView::new(&status).wrap(true));
            });
        });
        ui.add_space(m::TIGHT);

        // Signing in.
        if let Some(Update { step, problem }) = &step
            && st.signin.is_some()
        {
            let problem = problem.clone().or(st.problem.clone());
            let enter = ui.input(|i| i.key_pressed(Key::Enter));
            match step {
                Step::Starting | Step::Working => {
                    ui.add(StringView::new("Asking Telegram…").dim(true));
                    buttons(ui, |ui| {
                        if ui.add(Button::new("Cancel")).clicked() {
                            st.end_signin(&ctx);
                        }
                    });
                }
                Step::Phone => {
                    ui.add(TextControl::new(&mut st.phone).label("Phone number:").hint("+1 555 123 4567").autofocus(true));
                    if let Some(p) = &problem {
                        ui.add(StringView::new(p).color(style::c(style::palette(ui.ctx()).failure)).wrap(true));
                    }
                    buttons(ui, |ui| {
                        let ready = st.phone.chars().filter(char::is_ascii_digit).count() >= 6;
                        if (ui.add_enabled(ready, Button::new("Send Code").default(true)).clicked() || enter && ready)
                            && let Some(s) = &st.signin
                        {
                            s.phone(&st.phone);
                        }
                        if ui.add(Button::new("Use a QR Code")).clicked()
                            && let Some(s) = &st.signin
                        {
                            s.qr();
                        }
                        if ui.add(Button::new("Cancel")).clicked() {
                            st.end_signin(&ctx);
                        }
                    });
                }
                Step::Code { .. } => {
                    ui.add(StringView::new("Telegram sent a code to Telegram on your other devices (or by text message). Type it here.").dim(true).wrap(true));
                    ui.add(TextControl::new(&mut st.code).label("Code:").hint("12345").autofocus(true));
                    if let Some(p) = &problem {
                        ui.add(StringView::new(p).color(style::c(style::palette(ui.ctx()).failure)).wrap(true));
                    }
                    buttons(ui, |ui| {
                        let ready = st.code.chars().filter(char::is_ascii_digit).count() >= 4;
                        if (ui.add_enabled(ready, Button::new("Sign In").default(true)).clicked() || enter && ready)
                            && let Some(s) = &st.signin
                        {
                            s.code(&st.code);
                        }
                        if ui.add(Button::new("Cancel")).clicked() {
                            st.end_signin(&ctx);
                        }
                    });
                }
                Step::Password { hint } => {
                    ui.add(StringView::new("This account has a two-step password.").dim(true).wrap(true));
                    let h = hint.clone().map(|h| format!("Hint: {h}")).unwrap_or_default();
                    ui.add(TextControl::new(&mut st.password).label("Password:").password(true).hint(&h).autofocus(true));
                    if let Some(p) = &problem {
                        ui.add(StringView::new(p).color(style::c(style::palette(ui.ctx()).failure)).wrap(true));
                    }
                    buttons(ui, |ui| {
                        let ready = !st.password.is_empty();
                        if (ui.add_enabled(ready, Button::new("Sign In").default(true)).clicked() || enter && ready)
                            && let Some(s) = &st.signin
                        {
                            s.password(&std::mem::take(&mut st.password));
                        }
                        if ui.add(Button::new("Cancel")).clicked() {
                            st.end_signin(&ctx);
                        }
                    });
                }
                Step::Qr { url } => {
                    ui.add(StringView::new("On your phone, open Telegram ▸ Settings ▸ Devices ▸ Link Desktop Device, and point it at this code.").dim(true).wrap(true));
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 196.0), Sense::hover());
                    let square = Rect::from_center_size(r.center(), vec2(188.0, 188.0));
                    if url.is_empty() {
                        ui.painter().rect_filled(square, 8.0, Color32::WHITE);
                    } else {
                        qr(ui, square, url);
                        style::describe_text(ui.ctx(), olo_kit::egui::accesskit::Role::Image, &format!("QR code: {url}"), square, None);
                    }
                    if let Some(p) = &problem {
                        ui.add(StringView::new(p).color(style::c(style::palette(ui.ctx()).failure)).wrap(true));
                    }
                    buttons(ui, |ui| {
                        if ui.add(Button::new("Use a Phone Number")).clicked() {
                            *st.step.lock().unwrap() = Some(Update { step: Step::Phone, problem: None });
                        }
                        if ui.add(Button::new("Cancel")).clicked() {
                            st.end_signin(&ctx);
                        }
                    });
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
                }
                Step::Done(_) => {}
                Step::Failed(why) => {
                    ui.add(StringView::new(why).color(style::c(style::palette(ui.ctx()).failure)).wrap(true));
                    buttons(ui, |ui| {
                        if ui.add(Button::new("OK").default(true)).clicked() {
                            st.end_signin(&ctx);
                        }
                    });
                }
            }
            return;
        }

        // The API ID and hash.
        if st.editing_api || known == Known::NoApi {
            ui.add(StringView::new("Make one at my.telegram.org ▸ API development tools (sign in there with your phone number), then copy its App api_id and api_hash here.").dim(true).wrap(true));
            let divider = olo_kit::widgets::divider_for(ui, &["API ID:", "API hash:"]);
            ui.add(TextControl::new(&mut st.api_id).label("API ID:").divider(divider).hint("1234567"));
            ui.add(TextControl::new(&mut st.api_hash).label("API hash:").divider(divider).password(true).hint("32 letters and digits"));
            if let Some(p) = &st.problem {
                ui.add(StringView::new(p).color(style::c(style::palette(ui.ctx()).failure)).wrap(true));
            }
            buttons(ui, |ui| {
                let ready = !st.api_id.trim().is_empty() && !st.api_hash.trim().is_empty();
                if ui.add_enabled(ready, Button::new("Save").default(true)).clicked() {
                    st.save_api(&ctx);
                }
                if known != Known::NoApi && ui.add(Button::new("Cancel")).clicked() {
                    st.editing_api = false;
                    st.problem = None;
                }
                if ui.add(Button::new("Open my.telegram.org")).clicked() {
                    let _ = olo_kit::open::open(&olo_kit::open::Target::Url(MY_TELEGRAM.into()));
                }
            });
            return;
        }
        buttons(ui, |ui| match &known {
            Known::SignedOut(api) => {
                if ui.add(Button::new("Sign In…").default(true)).clicked() {
                    st.start_signin(&ctx, api.clone());
                }
                if ui.add(Button::new("Change API ID…")).clicked() {
                    st.api_id = api.id.to_string();
                    st.api_hash.clear();
                    st.editing_api = true;
                }
            }
            Known::SignedIn(..) => {
                if ui.add(Button::new("Open Telegram")).clicked() {
                    let _ = olo_kit::spawn_detached("olo-telegram");
                }
                if ui.add(Button::new("Sign Out")).clicked() {
                    st.sign_out(&ctx);
                }
            }
            Known::Failed(_) => {
                if ui.add(Button::new("Check Again")).clicked() {
                    st.check(&ctx);
                }
            }
            _ => {}
        });
    });
}
