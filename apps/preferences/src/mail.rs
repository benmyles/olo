//! Integrations ▸ Mail: the mail accounts Olo looks in for one-time codes
//! sent by mail (`olo_mail`; Ben, 2026-11-17: "support for connecting any
//! number of gmail (free or workspace gmail) accounts as well as fastmail
//! accounts"). A Google account's is its account connected to Olo with
//! Mail ticked (the Google box, `crate::google`: signed in through its
//! own app). A Fastmail one is added here with its address and an app
//! password, which is tried first (signed in, its inbox opened) on a
//! worker: then the password is kept in the login keyring and the
//! address in `mail.json` (Gmail accounts added that way before go on
//! as they were). As the panel opens each is tried again, and says how
//! it stands. Which are looked in, and when: Security ▸ Passwords
//! (`phone::codes`).

use std::sync::Arc;

use olo_kit::{
    Child, Cx, Element, IntoElement, ParentElement as _, Styled as _, div, metrics as m, px,
    widgets::{BBox, Button, StringView, TextControl, TextField, TextFieldEvent},
};
use olo_mail::{Account, Provider};
use olo_sync::Mutex;

use crate::{
    Msg, PanelView,
    form::Ui,
    integrations::{buttons, service},
};

/// How an account stands.
#[derive(Clone, Debug, PartialEq)]
enum Stands {
    Checking,
    Connected,
    Failed(String),
}

/// Adding one: being typed, being tried, what was wrong, or done.
#[derive(Clone, Debug, Default, PartialEq)]
enum Adding {
    #[default]
    Typing,
    Trying,
    Failed(String),
    Done,
}

pub struct State {
    /// The accounts set up and how each stands (None: not read yet).
    accounts: Arc<Mutex<Option<Vec<(Account, Stands)>>>>,
    /// The provider of the one being added, while it is.
    adding: Option<Provider>,
    trying: Arc<Mutex<Adding>>,
    /// The addresses that are Google accounts connected to Olo (signed in
    /// through their own app, not an app password).
    google: Arc<Mutex<Vec<String>>>,
    address: Child<TextField>,
    password: Child<TextField>,
    proxy: olo_kit::Proxy,
}

/// The Mail part of the Integrations panel's state, in its view.
fn state_of(v: &mut PanelView) -> Option<&mut State> {
    crate::integrations::state_of(v).map(|i| &mut i.mail)
}

impl State {
    pub fn new(cx: &mut Cx<PanelView>) -> State {
        let field = |cx: &mut Cx<PanelView>, hint: &str, password: bool| {
            let hint = hint.to_owned();
            let f = crate::form::text_field(cx, |f| f.hint(hint).password(password));
            // Return connects, as the default button does.
            cx.subscribe(&f, |v, e: &TextFieldEvent, cx| {
                if matches!(e, TextFieldEvent::Entered)
                    && let Some(st) = state_of(v)
                {
                    st.connect(cx);
                    cx.notify();
                }
            });
            f
        };
        let address = field(cx, "you@example.com", false);
        let password = field(cx, "xxxx xxxx xxxx xxxx", true);
        let st = State { accounts: Arc::default(), adding: None, trying: Arc::default(), google: Arc::default(), address, password, proxy: olo_kit::proxy() };
        // (Tests, the layout lint, never read the keyring or the network:
        // an account of each kind, one that no longer signs in.)
        if cfg!(test) {
            *st.accounts.lock().unwrap() = Some(vec![
                (Account { provider: Provider::Gmail, address: "ada@example.com".into() }, Stands::Connected),
                (Account { provider: Provider::Fastmail, address: "ada@example.org".into() }, Stands::Failed("Fastmail didn’t take the address and app password.".into())),
            ]);
        } else {
            st.check();
        }
        st
    }

    /// Read the accounts, and try each, on a worker.
    fn check(&self) {
        let (accounts, google, proxy) = (self.accounts.clone(), self.google.clone(), self.proxy.clone());
        std::thread::spawn(move || {
            let listed = olo_mail::accounts();
            *google.lock().unwrap() = olo_google::accounts::serving(olo_google::accounts::Service::Mail).into_iter().map(|a| a.address).collect();
            *accounts.lock().unwrap() = Some(listed.iter().map(|a| (a.clone(), Stands::Checking)).collect());
            proxy.send(Msg::Redraw);
            for a in listed {
                let now = match olo_mail::auth_of(&a) {
                    Err(why) if why.starts_with("no app password") => Stands::Failed("Its app password isn’t in the keyring: remove it and add it again.".into()),
                    Err(why) => Stands::Failed(sentence(&why)),
                    Ok(auth) => match olo_mail::check(&a, &auth) {
                        Ok(()) => Stands::Connected,
                        Err(why) => Stands::Failed(sentence(&why)),
                    },
                };
                if let Some(list) = accounts.lock().unwrap().as_mut()
                    && let Some(entry) = list.iter_mut().find(|(b, _)| *b == a)
                {
                    entry.1 = now;
                }
                proxy.send(Msg::Redraw);
            }
        });
    }

    /// Try the address and app password typed, and keep them if they sign
    /// in.
    fn connect(&mut self, cx: &mut Cx<PanelView>) {
        let Some(provider) = self.adding else { return };
        let address = self.address.read(cx).text().trim().to_owned();
        let password = self.password.read(cx).text().to_owned();
        if !address.contains('@') || password.trim().is_empty() || *self.trying.lock().unwrap() == Adding::Trying {
            return;
        }
        *self.trying.lock().unwrap() = Adding::Trying;
        let (accounts, trying, proxy) = (self.accounts.clone(), self.trying.clone(), self.proxy.clone());
        std::thread::spawn(move || {
            let account = Account { provider, address: address.to_lowercase() };
            let now = match olo_mail::check(&account, &password) {
                Err(why) => Adding::Failed(sentence(&why)),
                Ok(()) => match olo_mail::keep_password(&account.address, password.trim()) {
                    Err(e) => Adding::Failed(format!("The app password couldn’t be kept in the keyring ({e}).")),
                    Ok(()) => {
                        let mut listed = olo_mail::own_accounts();
                        listed.retain(|a| a.address != account.address);
                        listed.push(account.clone());
                        match olo_mail::save_accounts(&listed) {
                            Err(e) => Adding::Failed(format!("The account couldn’t be saved ({e}).")),
                            Ok(()) => {
                                let mut a = accounts.lock().unwrap();
                                let list = a.get_or_insert_with(Vec::new);
                                list.retain(|(b, _)| b.address != account.address);
                                list.push((account, Stands::Connected));
                                Adding::Done
                            }
                        }
                    }
                },
            };
            *trying.lock().unwrap() = now;
            proxy.send(Msg::Redraw);
        });
    }

    /// Forget an account: its password out of the keyring, its address
    /// off the list. A Google account stays connected to Olo: its mail
    /// is no longer looked in (its Mail unticked under Google).
    fn remove(&self, address: String) {
        let (accounts, proxy) = (self.accounts.clone(), self.proxy.clone());
        if let Some(list) = accounts.lock().unwrap().as_mut() {
            list.retain(|(a, _)| a.address != address);
        }
        let through_google = self.google.lock().unwrap().iter().any(|g| g.eq_ignore_ascii_case(&address));
        std::thread::spawn(move || {
            if through_google {
                crate::google::tick(&address, olo_google::accounts::Service::Mail, false);
                proxy.send(Msg::Redraw);
                return;
            }
            olo_mail::forget_password(&address);
            let mut listed = olo_mail::own_accounts();
            listed.retain(|a| a.address != address);
            if let Err(e) = olo_mail::save_accounts(&listed) {
                tracing::warn!("mail accounts: couldn’t save ({e})");
            }
            proxy.send(Msg::Redraw);
        });
    }

    fn stop_adding(&mut self, cx: &mut Cx<PanelView>) {
        self.adding = None;
        *self.trying.lock().unwrap() = Adding::Typing;
        self.password.update(cx, |f, cx| f.clear_secret(cx));
        self.address.update(cx, |f, cx| f.set_text("", cx));
    }
}

/// What went wrong, as a sentence.
fn sentence(why: &str) -> String {
    let mut s = why.trim().to_owned();
    if let Some(first) = s.chars().next() {
        s.replace_range(..first.len_utf8(), &first.to_uppercase().to_string());
    }
    if !s.ends_with('.') {
        s.push('.');
    }
    s
}

fn act(ui: &Ui, f: impl Fn(&mut State, &mut Cx<PanelView>) + 'static) -> impl Fn(&olo_kit::ClickEvent, &mut olo_kit::WindowCx) + 'static {
    ui.on_click_view(move |v, cx| {
        if let Some(st) = state_of(v) {
            f(st, cx);
        }
        cx.notify();
    })
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    if *st.trying.lock().unwrap() == Adding::Done {
        st.stop_adding(ui.cx);
    }
    let listed = st.accounts.lock().unwrap().clone();
    let trying = st.trying.lock().unwrap().clone();
    let status = match &listed {
        None => "Checking…".to_owned(),
        Some(l) if l.is_empty() => "No accounts connected.".to_owned(),
        Some(l) if l.iter().any(|(_, s)| *s == Stands::Checking) => "Checking…".to_owned(),
        Some(l) => match (l.iter().filter(|(_, s)| *s == Stands::Connected).count(), l.len()) {
            (1, 1) => "Connected.".to_owned(),
            (c, n) if c == n => format!("All {n} connected."),
            (c, n) => format!("{c} of {n} connected."),
        },
    };
    let mut items: Vec<Element> = vec![
        service("mail", "While a page waits for a one-time code sent by mail, Olo looks for it in these accounts’ new mail for a minute (which ones: Security ▸ Passwords). Google accounts: tick Mail for them under Google.", status).into_element(),
    ];
    let google = st.google.lock().unwrap().clone();
    for (account, stands) in listed.iter().flatten() {
        let how = match stands {
            Stands::Checking => "checking…".to_owned(),
            Stands::Connected => "connected".to_owned(),
            Stands::Failed(why) => why.clone(),
        };
        let address = account.address.clone();
        items.push(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(m::SPACING))
                .child(div().flex_1().min_w_0().flex().flex_col().child(StringView::new(account.address.clone()).wrap(true)).child(
                    StringView::new(format!("{} · {how}", if google.iter().any(|g| g.eq_ignore_ascii_case(&account.address)) { "Google" } else { account.provider.name() })).dim(true).wrap(true),
                ))
                .child(Button::new("Remove").id(format!("mail-remove-{address}")).on_click(act(ui, move |st, _| st.remove(address.clone()))))
                .into_element(),
        );
    }
    if let Some(provider) = st.adding {
        items.push(TextControl::new(ui.cx, &st.address).label("Address:").into_element());
        items.push(TextControl::new(ui.cx, &st.password).label("App password:").into_element());
        let how = match provider {
            Provider::Gmail => {
                "Your Google account’s address (Gmail, or a Workspace one) and an app password for it, made in your Google Account’s security settings with 2-Step Verification on (a Workspace’s administrator may have turned them off)."
            }
            Provider::Fastmail => "Your Fastmail address and an app password for it with mail access (Settings ▸ Privacy & Security ▸ Manage app passwords and access; not on the Basic plan).",
        };
        items.push(StringView::new(format!("{how} Olo keeps it in your login keyring and only reads the newest messages, for codes.")).dim(true).wrap(true).into_element());
        match &trying {
            Adding::Trying => items.push(StringView::new(format!("Signing in to {}…", provider.name())).wrap(true).into_element()),
            Adding::Failed(why) => items.push(StringView::new(why.clone()).wrap(true).into_element()),
            _ => {}
        }
        let ready = st.address.read(ui.cx).text().contains('@') && !st.password.read(ui.cx).text().trim().is_empty() && trying != Adding::Trying;
        items.push(
            buttons(vec![
                Button::new("Connect").default(true).enabled(ready).on_click(act(ui, |st, cx| st.connect(cx))).into_element(),
                Button::new("Cancel").enabled(trying != Adding::Trying).on_click(act(ui, |st, cx| st.stop_adding(cx))).into_element(),
                Button::new("Make an App Password…")
                    .on_click(move |_, _| {
                        let _ = olo_xdg::open::open(&olo_xdg::open::Target::Url(provider.app_passwords().into()));
                    })
                    .into_element(),
            ])
            .into_element(),
        );
    } else {
        let mut row = Vec::new();
        if listed.as_ref().is_some_and(|l| !l.is_empty()) {
            let checking = listed.iter().flatten().any(|(_, s)| *s == Stands::Checking);
            row.push(Button::new("Check Again").id("mail-check").enabled(!checking).on_click(act(ui, |st, _| st.check())).into_element());
        }
        // (Gmail's: under Google, with Mail ticked.)
        for provider in [Provider::Fastmail] {
            let field = st.address.downgrade();
            row.push(
                Button::new(format!("Add {}…", provider.name()))
                    .on_click(ui.on_click_view(move |v, cx| {
                        if let Some(st) = state_of(v) {
                            st.adding = Some(provider);
                            *st.trying.lock().unwrap() = Adding::Typing;
                        }
                        field.focus(cx);
                        cx.notify();
                    }))
                    .into_element(),
            );
        }
        // (The first given is rightmost.)
        row.reverse();
        items.push(buttons(row).into_element());
    }
    ui.add(BBox::new("Mail").children(items));
}
