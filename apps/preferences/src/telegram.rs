//! Integrations ▸ Telegram: the API ID and hash a Telegram app signs in
//! with (the person's own, from my.telegram.org), and signing in (a code
//! sent to their phone or Telegram, or a QR code scanned with Telegram on
//! a phone, then the two-step password if the account has one) or out.
//! The session goes to the login keyring (`olo_tg::account`); Telegram
//! (`olo-telegram`), if it's running, is told to use it at once. All of
//! it talks to the keyring and Telegram on workers.

use olo_sync::Mutex;
use std::sync::Arc;

use olo_tg::{
    account::{self, Api},
    signin::{SignIn, Step, Update},
};
use olo_ui::{
    gpui::{AnyElement, App, Context, Entity, IntoElement, ParentElement as _, Styled as _, Window, canvas, div, point, px, size},
    metrics as m, style, theme,
    widgets::{BBox, Button, StringView, TextControl, TextField, TextFieldEvent},
};

use crate::{
    Msg, PanelView,
    form::{Ui, divider_for},
    integrations::{buttons, service},
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

pub struct State {
    known: Arc<Mutex<Known>>,
    /// Typing an API ID and hash.
    api_id: Entity<TextField>,
    api_hash: Entity<TextField>,
    editing_api: bool,
    /// Signing in: the flow, where it is, and what's typed.
    signin: Option<SignIn>,
    step: Arc<Mutex<Option<Update>>>,
    phone: Entity<TextField>,
    code: Entity<TextField>,
    password: Entity<TextField>,
    problem: Option<String>,
    proxy: olo_ui::Proxy,
    /// A frame is due (the QR code changes now and then).
    ticking: bool,
}

/// The Telegram part of the Integrations panel's state, in its view.
fn state_of(v: &mut PanelView) -> Option<&mut State> {
    crate::integrations::state_of(v).map(|i| &mut i.telegram)
}

impl State {
    pub fn new(cx: &mut Context<PanelView>) -> State {
        let field = |cx: &mut Context<PanelView>, hint: &str, password: bool| {
            let hint = hint.to_owned();
            let f = olo_ui::widgets::text_field(cx, |f| f.hint(hint).password(password));
            // Return does what the step's default button does.
            cx.subscribe(&f, |v, _, e: &TextFieldEvent, cx| match e {
                TextFieldEvent::Entered => {
                    if let Some(st) = state_of(v) {
                        st.enter(cx);
                    }
                    cx.notify();
                }
                TextFieldEvent::Changed => cx.notify(),
                _ => {}
            })
            .detach();
            f
        };
        let api_id = field(cx, "1234567", false);
        let api_hash = field(cx, "32 letters and digits", true);
        let phone = field(cx, "+1 555 123 4567", false);
        let code = field(cx, "12345", false);
        let password = field(cx, "", true);
        let mut st =
            State { known: Arc::default(), api_id, api_hash, editing_api: false, signin: None, step: Arc::default(), phone, code, password, problem: None, proxy: olo_ui::proxy(cx), ticking: false };
        // (Tests, the layout lint, never read the keyring.)
        if cfg!(test) {
            *st.known.lock().unwrap() = Known::NoApi;
        } else {
            st.check();
        }
        st
    }
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
    if olo_ui::instance::running("olo.telegram") {
        olo_ui::instance::forward("olo.telegram", &["--reconnect".to_owned()]);
    }
}

impl State {
    fn check(&mut self) {
        let (out, proxy) = (self.known.clone(), self.proxy.clone());
        std::thread::spawn(move || {
            *out.lock().unwrap() = status();
            proxy.send(Msg::Redraw);
        });
    }

    fn save_api(&mut self, cx: &mut Context<PanelView>) {
        let Ok(id) = self.api_id.read(cx).text().trim().parse::<i32>() else {
            self.problem = Some("The API ID is a number.".into());
            return;
        };
        let api = Api { id, hash: self.api_hash.read(cx).text().trim().to_owned() };
        if api.hash.len() != 32 || !api.hash.chars().all(|c| c.is_ascii_hexdigit()) {
            self.problem = Some("The API hash is 32 letters and digits (0–9, a–f).".into());
            return;
        }
        self.problem = None;
        self.editing_api = false;
        self.api_hash.update(cx, |f, cx| f.clear_secret(cx));
        let (out, proxy) = (self.known.clone(), self.proxy.clone());
        *out.lock().unwrap() = Known::Checking;
        std::thread::spawn(move || {
            *out.lock().unwrap() = match account::save_api(&api) {
                Ok(()) => status(),
                Err(e) => Known::Failed(format!("The API ID couldn’t be kept in the keyring ({e}).")),
            };
            proxy.send(Msg::Redraw);
        });
    }

    fn start_signin(&mut self, api: Api, cx: &mut Context<PanelView>) {
        let (step, proxy) = (self.step.clone(), self.proxy.clone());
        *step.lock().unwrap() = Some(Update { step: Step::Starting, problem: None });
        self.code.update(cx, |f, cx| f.set_text("", cx));
        self.password.update(cx, |f, cx| f.clear_secret(cx));
        self.problem = None;
        self.signin = Some(SignIn::start(api, move |u| {
            *step.lock().unwrap() = Some(u);
            proxy.send(Msg::Redraw);
        }));
    }

    fn end_signin(&mut self) {
        self.signin = None;
        *self.step.lock().unwrap() = None;
        self.check();
    }

    fn sign_out(&mut self) {
        let (out, proxy) = (self.known.clone(), self.proxy.clone());
        *out.lock().unwrap() = Known::Checking;
        std::thread::spawn(move || {
            let r = olo_tg::signin::sign_out();
            *out.lock().unwrap() = match r {
                Ok(()) => status(),
                Err(e) => Known::Failed(format!("Couldn’t sign out ({e}).")),
            };
            tell_telegram();
            proxy.send(Msg::Redraw);
        });
    }

    /// Return in a field: the step's default button, if it can be used.
    fn enter(&mut self, cx: &mut Context<PanelView>) {
        let step = self.step.lock().unwrap().clone().map(|u| u.step);
        match (&step, &self.signin) {
            (Some(Step::Phone), Some(s)) if self.phone_ready(cx) => s.phone(self.phone.read(cx).text()),
            (Some(Step::Code { .. }), Some(s)) if self.code_ready(cx) => s.code(self.code.read(cx).text()),
            (Some(Step::Password { .. }), Some(s)) if !self.password.read(cx).text().is_empty() => {
                s.password(self.password.read(cx).text());
                self.password.update(cx, |f, cx| f.clear_secret(cx));
            }
            (None, _) if self.editing_api || *self.known.lock().unwrap() == Known::NoApi => self.save_api(cx),
            _ => {}
        }
    }

    fn phone_ready(&self, cx: &App) -> bool {
        self.phone.read(cx).text().chars().filter(char::is_ascii_digit).count() >= 6
    }

    fn code_ready(&self, cx: &App) -> bool {
        self.code.read(cx).text().chars().filter(char::is_ascii_digit).count() >= 4
    }
}

/// `url` as a QR code in `r`, black on white with a quiet zone, each
/// module whole device pixels.
fn qr(window: &mut Window, r: olo_ui::gpui::Bounds<olo_ui::gpui::Pixels>, url: &str) {
    window.paint_quad(olo_ui::gpui::fill(r, olo_ui::gpui::white()).corner_radii(px(8.0)));
    let Ok(code) = qrcode::QrCode::new(url.as_bytes()) else { return };
    let n = code.width();
    let colors = code.to_colors();
    let s = window.scale_factor();
    let module = (((f32::from(r.size.width) - 16.0) / n as f32) * s).floor().max(1.0) / s;
    let total = module * n as f32;
    let origin = point(px(((f32::from(r.center().x) - total / 2.0) * s).round() / s), px(((f32::from(r.center().y) - total / 2.0) * s).round() / s));
    for y in 0..n {
        for x in 0..n {
            if colors[y * n + x] == qrcode::Color::Dark {
                let b = olo_ui::gpui::Bounds { origin: point(origin.x + px(x as f32 * module), origin.y + px(y as f32 * module)), size: size(px(module), px(module)) };
                style::fill_rect(window, b, olo_ui::gpui::black());
            }
        }
    }
}

/// Something done to the Telegram part's state from a button.
fn act(ui: &Ui, f: impl Fn(&mut State, &mut Context<PanelView>) + 'static) -> impl Fn(&mut Window, &mut App) + 'static {
    ui.on_click_view(move |v, _, cx| {
        if let Some(st) = state_of(v) {
            f(st, cx);
        }
        cx.notify();
    })
}

/// Cancel: the sign-in given up.
fn cancel(ui: &Ui) -> impl Fn(&mut Window, &mut App) + 'static {
    act(ui, |st, _| st.end_signin())
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let known = st.known.lock().unwrap().clone();
    let step = st.step.lock().unwrap().clone();
    if let Some(Update { step: Step::Done(_), .. }) = &step {
        tell_telegram();
        st.end_signin();
    }
    let failure = style::c(theme::palette(ui.cx).failure);
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
    let mut items: Vec<AnyElement> = vec![
        service("telegram", "Telegram (in Applications) shows your chats and messages, and notifies you of new ones. Olo keeps the sign-in in your login keyring.", status).into_any_element(),
        div().h(px(m::TIGHT)).into_any_element(),
    ];
    let problem_text = |p: &Option<String>| p.as_ref().map(|p| StringView::new(p.clone()).color(failure).wrap(true).into_any_element());

    // Signing in.
    if let Some(Update { step, problem }) = &step
        && st.signin.is_some()
    {
        let problem = problem.clone().or(st.problem.clone());
        match step {
            Step::Starting | Step::Working => {
                items.push(StringView::new("Asking Telegram…").dim(true).into_any_element());
                items.push(buttons(vec![Button::new("Cancel").on_click(cancel(ui)).into_any_element()]).into_any_element());
            }
            Step::Phone => {
                items.push(TextControl::new(st.phone.clone()).label("Phone number:").into_any_element());
                items.extend(problem_text(&problem));
                let ready = st.phone_ready(ui.cx);
                let send = act(ui, |st, cx| {
                    if let Some(s) = &st.signin {
                        s.phone(st.phone.read(cx).text());
                    }
                });
                let qr_code = act(ui, |st, _| {
                    if let Some(s) = &st.signin {
                        s.qr();
                    }
                });
                items.push(
                    buttons(vec![
                        Button::new("Send Code").default(true).enabled(ready).on_click(send).into_any_element(),
                        Button::new("Use a QR Code").on_click(qr_code).into_any_element(),
                        Button::new("Cancel").on_click(cancel(ui)).into_any_element(),
                    ])
                    .into_any_element(),
                );
            }
            Step::Code { .. } => {
                items.push(StringView::new("Telegram sent a code to Telegram on your other devices (or by text message). Type it here.").dim(true).wrap(true).into_any_element());
                items.push(TextControl::new(st.code.clone()).label("Code:").into_any_element());
                items.extend(problem_text(&problem));
                let ready = st.code_ready(ui.cx);
                let sign_in = act(ui, |st, cx| {
                    if let Some(s) = &st.signin {
                        s.code(st.code.read(cx).text());
                    }
                });
                items.push(
                    buttons(vec![Button::new("Sign In").default(true).enabled(ready).on_click(sign_in).into_any_element(), Button::new("Cancel").on_click(cancel(ui)).into_any_element()])
                        .into_any_element(),
                );
            }
            Step::Password { hint } => {
                items.push(StringView::new("This account has a two-step password.").dim(true).wrap(true).into_any_element());
                let h = hint.clone().map(|h| format!("Hint: {h}")).unwrap_or_default();
                st.password.update(ui.cx, |f, cx| f.set_hint(h, cx));
                items.push(TextControl::new(st.password.clone()).label("Password:").into_any_element());
                items.extend(problem_text(&problem));
                let ready = !st.password.read(ui.cx).text().is_empty();
                let sign_in = act(ui, |st, cx| {
                    if let Some(s) = &st.signin {
                        s.password(st.password.read(cx).text());
                    }
                    st.password.update(cx, |f, cx| f.clear_secret(cx));
                });
                items.push(
                    buttons(vec![Button::new("Sign In").default(true).enabled(ready).on_click(sign_in).into_any_element(), Button::new("Cancel").on_click(cancel(ui)).into_any_element()])
                        .into_any_element(),
                );
            }
            Step::Qr { url } => {
                items.push(StringView::new("On your phone, open Telegram ▸ Settings ▸ Devices ▸ Link Desktop Device, and point it at this code.").dim(true).wrap(true).into_any_element());
                let url = url.clone();
                items.push(
                    div()
                        .flex()
                        .justify_center()
                        .child(
                            canvas(
                                |_, _, _| {},
                                move |b, _, window, cx| {
                                    qr(window, b, &url);
                                    if !url.is_empty() {
                                        olo_ui::agent::record(window, cx, olo_ui::agent::Node { role: "Image", name: Some(format!("QR code: {url}").into()), bounds: b, ..Default::default() });
                                    }
                                },
                            )
                            .w(px(188.0))
                            .h(px(188.0)),
                        )
                        .h(px(196.0))
                        .into_any_element(),
                );
                items.extend(problem_text(&problem));
                let phone = act(ui, |st, _| *st.step.lock().unwrap() = Some(Update { step: Step::Phone, problem: None }));
                items.push(buttons(vec![Button::new("Use a Phone Number").on_click(phone).into_any_element(), Button::new("Cancel").on_click(cancel(ui)).into_any_element()]).into_any_element());
                // The code changes now and then.
                if !st.ticking {
                    st.ticking = true;
                    ui.cx
                        .spawn(async move |view, cx| {
                            cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                            let _ = view.update(cx, |v, cx| {
                                if let Some(st) = state_of(v) {
                                    st.ticking = false;
                                }
                                cx.notify();
                            });
                        })
                        .detach();
                }
            }
            Step::Done(_) => {}
            Step::Failed(why) => {
                items.push(StringView::new(why.clone()).color(failure).wrap(true).into_any_element());
                items.push(buttons(vec![Button::new("OK").default(true).on_click(cancel(ui)).into_any_element()]).into_any_element());
            }
        }
        ui.add(BBox::new("Telegram").children(items));
        return;
    }

    // The API ID and hash.
    if st.editing_api || known == Known::NoApi {
        items.push(
            StringView::new("Make one at my.telegram.org ▸ API development tools (sign in there with your phone number), then copy its App api_id and api_hash here.")
                .dim(true)
                .wrap(true)
                .into_any_element(),
        );
        let divider = divider_for(ui.window, &["API ID:", "API hash:"]);
        items.push(TextControl::new(st.api_id.clone()).label("API ID:").divider(divider).into_any_element());
        items.push(TextControl::new(st.api_hash.clone()).label("API hash:").divider(divider).into_any_element());
        items.extend(problem_text(&st.problem));
        let ready = !st.api_id.read(ui.cx).text().trim().is_empty() && !st.api_hash.read(ui.cx).text().trim().is_empty();
        let mut row = vec![Button::new("Save").default(true).enabled(ready).on_click(act(ui, |st, cx| st.save_api(cx))).into_any_element()];
        if known != Known::NoApi {
            row.push(
                Button::new("Cancel")
                    .on_click(act(ui, |st, _| {
                        st.editing_api = false;
                        st.problem = None;
                    }))
                    .into_any_element(),
            );
        }
        row.push(
            Button::new("Open my.telegram.org")
                .on_click(|_, _| {
                    let _ = olo_xdg::open::open(&olo_xdg::open::Target::Url(MY_TELEGRAM.into()));
                })
                .into_any_element(),
        );
        items.push(buttons(row).into_any_element());
        ui.add(BBox::new("Telegram").children(items));
        return;
    }
    let row: Vec<AnyElement> = match &known {
        Known::SignedOut(api) => {
            let (a, b) = (api.clone(), api.clone());
            vec![
                Button::new("Sign In…").default(true).on_click(act(ui, move |st, cx| st.start_signin(a.clone(), cx))).into_any_element(),
                Button::new("Change API ID…")
                    .on_click(act(ui, move |st, cx| {
                        st.api_id.update(cx, |f, cx| f.set_text(b.id.to_string(), cx));
                        st.api_hash.update(cx, |f, cx| f.clear_secret(cx));
                        st.editing_api = true;
                    }))
                    .into_any_element(),
            ]
        }
        Known::SignedIn(..) => vec![
            Button::new("Open Telegram")
                .on_click(|_, _| {
                    let _ = olo_xdg::open::spawn_app(&["olo-telegram".into()], "olo.telegram");
                })
                .into_any_element(),
            Button::new("Sign Out").on_click(act(ui, |st, _| st.sign_out())).into_any_element(),
        ],
        Known::Failed(_) => vec![Button::new("Check Again").on_click(act(ui, |st, _| st.check())).into_any_element()],
        _ => Vec::new(),
    };
    items.push(buttons(row).into_any_element());
    ui.add(BBox::new("Telegram").children(items));
}
