//! Security ▸ Security Keys: a key (a YubiKey) set up so a touch of it
//! does what typing the account password does (`olo_fido`), and whether
//! a key's touches type its code (`SecurityKeySettings`).
//!
//! The keys plugged in are looked at on a worker (each asked what it is,
//! and whether it's one set up, without a touch), again whenever udev
//! says a HID device came or went (`olo_fido::hid::watch`: no polling).
//! Setting one up checks the password through PAM, then waits for the
//! key's two touches, either cancellable; a second key set up the same
//! way is a spare.

use olo_sync::Mutex;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc::{Sender, channel},
};

use olo_fido::{Device, Sealed};
use olo_ui::{
    gpui::{Context, Entity, IntoElement as _, ParentElement as _, SharedString, Styled as _, div, px},
    metrics as m,
    widgets::{Button, StatusBar, StringView, TextControl, TextField, TextFieldEvent},
};
use zeroize::Zeroizing;

use crate::{Msg, PanelView, form::Ui, integrations::buttons, panel};

enum Request {
    Look,
    SetUp(Device, Zeroizing<String>),
    Remove(String),
}

/// A key plugged in, as the box says it.
#[derive(Clone)]
struct Plugged {
    device: Device,
    /// What it is ("firmware 5.8.0"), or why it can't be used.
    about: Result<String, String>,
    set_up: bool,
}

struct Doing {
    message: String,
    /// Touches had (of two).
    touched: u32,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct Shared {
    /// None until first looked.
    plugged: Option<Vec<Plugged>>,
    set_up: Vec<Sealed>,
    doing: Option<Doing>,
    error: Option<String>,
    note: Option<String>,
}

struct Link {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Request>,
}

fn user() -> String {
    olo_system::auth::current_user().unwrap_or_default()
}

/// The worker (one for the app), telling the windows through `proxy`.
fn link(proxy: olo_ui::Proxy) -> &'static Link {
    static LINK: OnceLock<Link> = OnceLock::new();
    LINK.get_or_init(|| {
        let shared: Arc<Mutex<Shared>> = Default::default();
        let (tx, rx) = channel::<Request>();
        let sh = shared.clone();
        if cfg!(test) {
            // (The layout lint: one key, set up, none being set up.)
            let mut s = sh.lock().unwrap();
            let device = Device { path: "/dev/hidraw4".into(), name: "Yubico YubiKey OTP+FIDO+CCID".into(), vendor: 0x1050, product: 0x0407 };
            s.plugged = Some(vec![Plugged { device: device.clone(), about: Ok("firmware 5.8.0".into()), set_up: true }]);
            s.set_up = vec![Sealed {
                name: device.name,
                aaguid: String::new(),
                user: "ben".into(),
                added: 1_793_000_000,
                credential: String::new(),
                salt: String::new(),
                nonce: String::new(),
                sealed: String::new(),
            }];
            drop(s);
            return Link { shared, tx };
        }
        let again = tx.clone();
        let _ = olo_fido::hid::watch(move || {
            let _ = again.send(Request::Look);
        });
        let _ = tx.send(Request::Look);
        std::thread::Builder::new()
            .name("security-keys".into())
            .spawn(move || {
                let me = user();
                while let Ok(req) = rx.recv() {
                    match req {
                        Request::Look => {}
                        Request::SetUp(device, password) => set_up(&sh, &proxy, &me, &device, &password),
                        Request::Remove(credential) => {
                            let keys: Vec<Sealed> = olo_fido::seal::load(&me).into_iter().filter(|k| k.credential != credential).collect();
                            let mut s = sh.lock().unwrap();
                            match olo_fido::seal::save(&me, &keys) {
                                Ok(()) => s.note = Some("The key was removed: a touch of it no longer stands for your password.".into()),
                                Err(e) => s.error = Some(format!("The key couldn’t be removed ({e}).")),
                            }
                        }
                    }
                    // Look again after anything (a burst of plugging
                    // looked at once).
                    while rx.try_recv().is_ok_and(|r| matches!(r, Request::Look)) {}
                    let plugged = olo_fido::plugged(&me).into_iter().map(|p| Plugged { about: p.info.map(|i| about(&i)), device: p.device, set_up: p.set_up }).collect();
                    let set_up = olo_fido::seal::load(&me);
                    let mut s = sh.lock().unwrap();
                    s.plugged = Some(plugged);
                    s.set_up = set_up;
                    drop(s);
                    proxy.send(Msg::Redraw);
                }
            })
            .expect("thread");
        Link { shared, tx }
    })
}

/// What a key says of itself, in a few words.
fn about(i: &olo_fido::Info) -> String {
    let mut parts = Vec::new();
    if let Some(f) = i.firmware_text() {
        parts.push(format!("firmware {f}"));
    }
    if i.pin_set() {
        parts.push("a PIN set".into());
    }
    if !i.hmac_secret() {
        return format!("{}: it can’t keep a secret for Olo (no hmac-secret)", parts.join(", "));
    }
    parts.join(", ")
}

/// Set `device` up: the password checked, then two touches.
fn set_up(sh: &Arc<Mutex<Shared>>, proxy: &olo_ui::Proxy, me: &str, device: &Device, password: &str) {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut s = sh.lock().unwrap();
        (s.error, s.note) = (None, None);
        s.doing = Some(Doing { message: "Checking your password…".into(), touched: 0, cancel: cancel.clone() });
    }
    proxy.send(Msg::Redraw);
    let checked = olo_system::auth::check_password(me, password);
    if cancel.load(Ordering::Relaxed) {
        sh.lock().unwrap().doing = None;
        return;
    }
    if let Err(e) = checked {
        let mut s = sh.lock().unwrap();
        s.doing = None;
        s.error = Some(match e {
            olo_system::auth::AuthError::Denied(_) => "That password isn’t right.".into(),
            e => format!("Your password couldn’t be checked ({e})."),
        });
        return;
    }
    let asked = AtomicU32::new(0);
    let touch = || {
        let n = asked.fetch_add(1, Ordering::Relaxed);
        if let Some(d) = sh.lock().unwrap().doing.as_mut() {
            d.touched = n;
            d.message = if n == 0 { "Touch the key (its light blinks).".into() } else { "Touch it once more.".into() };
        }
        proxy.send(Msg::Redraw);
    };
    let made = olo_fido::set_up(device, me, password, &cancel, &touch);
    let mut s = sh.lock().unwrap();
    s.doing = None;
    match made {
        Ok(sealed) => {
            let mut keys = olo_fido::seal::load(me);
            keys.push(sealed);
            match olo_fido::seal::save(me, &keys) {
                Ok(()) => s.note = Some("The key is set up: a touch of it now does what typing your password does.".into()),
                Err(e) => s.error = Some(format!("The key couldn’t be kept ({e}).")),
            }
        }
        Err(olo_fido::Error::Cancelled) => {}
        Err(e) => s.error = Some(e.to_string()),
    }
}

pub struct State {
    link: &'static Link,
    /// The key being set up (its password being typed).
    setting_up: Option<Device>,
    password: Entity<TextField>,
}

impl State {
    pub fn new(cx: &mut Context<PanelView>) -> State {
        let password = olo_ui::widgets::text_field(cx, |f| f.password(true));
        cx.subscribe(&password, |v, _, e: &TextFieldEvent, cx| {
            if matches!(e, TextFieldEvent::Entered)
                && let Some(st) = state_of(v)
            {
                st.start(cx);
                cx.notify();
            }
        })
        .detach();
        State { link: link(olo_ui::proxy(cx)), setting_up: None, password }
    }

    /// Set the key chosen up with the password typed.
    fn start(&mut self, cx: &mut Context<PanelView>) {
        let typed = Zeroizing::new(self.password.read(cx).text().to_owned());
        let Some(device) = self.setting_up.take() else { return };
        if typed.is_empty() {
            self.setting_up = Some(device);
            return;
        }
        self.password.update(cx, |f, cx| f.clear_secret(cx));
        let _ = self.link.tx.send(Request::SetUp(device, typed));
    }
}

fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::Security(st) => Some(st),
        _ => None,
    }
}

/// Something done to the box's state from a button.
fn act(ui: &Ui, f: impl Fn(&mut State, &mut olo_ui::gpui::Window, &mut Context<PanelView>) + 'static) -> impl Fn(&mut olo_ui::gpui::Window, &mut olo_ui::gpui::App) + 'static {
    ui.on_click_view(move |v, window, cx| {
        if let Some(st) = state_of(v) {
            f(st, window, cx);
        }
        cx.notify();
    })
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let k = ui.s.security.keys.clone();
    let sh = st.link.shared.lock().unwrap();
    let plugged = sh.plugged.clone();
    let set_up = sh.set_up.clone();
    let doing = sh.doing.as_ref().map(|d| (d.message.clone(), d.touched, d.cancel.clone()));
    let (error, note) = (sh.error.clone(), sh.note.clone());
    drop(sh);
    let link = st.link;

    ui.bbox("Security Keys", |ui| {
        ui.note("A touch of a security key (a YubiKey) can stand for your password: sudo in Terminal, Passwords, administrator requests and the lock screen.");
        // The keys set up: each with whether it's plugged in, and Remove.
        for key in &set_up {
            let here = plugged.as_ref().is_some_and(|p| p.iter().any(|p| p.set_up && p.device.name == key.name));
            let when = jiff::Timestamp::from_second(key.added as i64).map(|t| t.to_zoned(jiff::tz::TimeZone::system()).strftime("%b %-d, %Y").to_string()).unwrap_or_default();
            let line = format!("{} · set up {when}{}", key_name(&key.name), if here { " · plugged in" } else { "" });
            let credential = key.credential.clone();
            let idle = doing.is_none() && st.setting_up.is_none();
            ui.add(div().flex().flex_row().items_center().gap(px(m::SPACING)).child(div().flex_1().min_w_0().child(StringView::new(line).wrap(true))).child(
                Button::new("Remove").enabled(idle).on_click(move |_, _| {
                    let _ = link.tx.send(Request::Remove(credential.clone()));
                }),
            ));
        }
        match &plugged {
            None => ui.note("Looking for security keys…"),
            Some(p) if p.is_empty() && set_up.is_empty() => ui.note("Plug in a security key to set it up."),
            Some(p) => {
                for key in p.iter().filter(|p| !p.set_up) {
                    match &key.about {
                        Err(why) => ui.add(StringView::new(format!("{}: {why}", key_name(&key.device.name))).wrap(true)),
                        Ok(about) => {
                            let what = if set_up.is_empty() { "" } else { " (a spare)" };
                            ui.add(StringView::new(format!("{}{what}, plugged in: {about}.", key_name(&key.device.name))).wrap(true));
                            if st.setting_up.is_none() && doing.is_none() {
                                let device = key.device.clone();
                                let field = st.password.clone();
                                ui.add(buttons(vec![
                                    Button::new("Set Up…")
                                        .on_click(act(ui, move |st, window, cx| {
                                            st.setting_up = Some(device.clone());
                                            field.update(cx, |f, _| f.focus(window));
                                        }))
                                        .into_any_element(),
                                ]));
                            }
                        }
                    }
                }
            }
        }
        if let Some(device) = &st.setting_up {
            ui.add(StringView::new(format!("To set up the {}, type your password, then touch the key twice.", key_name(&device.name))).wrap(true));
            ui.add(TextControl::new(st.password.clone()).label("Password:"));
            let typed = !st.password.read(ui.cx).text().is_empty();
            ui.add(buttons(vec![
                Button::new("Set Up").default(true).enabled(typed).on_click(act(ui, |st, _, cx| st.start(cx))).into_any_element(),
                Button::new("Cancel")
                    .on_click(act(ui, |st, _, cx| {
                        st.setting_up = None;
                        st.password.update(cx, |f, cx| f.clear_secret(cx));
                    }))
                    .into_any_element(),
            ]));
        }
        if let Some((message, touched, cancel)) = doing {
            ui.add(StatusBar::new(touched as f32 / 2.0).label(SharedString::from(message)).trailing(format!("{touched} of 2")));
            ui.add(buttons(vec![Button::new("Cancel").on_click(move |_, _| cancel.store(true, Ordering::Relaxed)).into_any_element()]));
        } else if let Some(e) = error {
            ui.add(StringView::new(e).wrap(true));
        } else if let Some(n) = note {
            ui.note(n);
        }
        ui.check("Three touches type a code", k.three_touches_type, |s, v| s.security.keys.three_touches_type = v);
        ui.note("A touch types nothing unless something is waiting for the key; three in a row type its one-time code where you are, for a web form Olo doesn’t know.");
    });
}

/// "YubiKey" for "Yubico YubiKey OTP+FIDO+CCID": the maker's name left out.
fn key_name(name: &str) -> String {
    let n = name.strip_prefix("Yubico ").unwrap_or(name);
    match n.split_once(' ') {
        Some((model, rest)) if rest.contains("FIDO") || rest.contains("OTP") || rest.contains("CCID") => model.to_owned(),
        _ => n.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn keys_named_plainly() {
        assert_eq!(super::key_name("Yubico YubiKey OTP+FIDO+CCID"), "YubiKey");
        assert_eq!(super::key_name("Yubico YubiKey FIDO+CCID"), "YubiKey");
        assert_eq!(super::key_name("Nitrokey Nitrokey 3"), "Nitrokey Nitrokey 3");
        assert_eq!(super::key_name("Olo Soft Key"), "Olo Soft Key");
    }
}
