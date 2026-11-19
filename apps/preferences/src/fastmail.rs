//! Integrations ▸ Fastmail: the Fastmail accounts Olo is connected to, and
//! what each is used for, as the Google tab has its accounts (Ben,
//! 2026-11-18: "add a Fastmail tab modelled after google where you add
//! fastmail accounts and choose what they integrate with"). Each signs
//! in with an app password made for it in Fastmail, with access to mail
//! and calendars: one for both (`olo_fastmail`). An account being added
//! is tried for each first, on a worker, and ticked for what it signed
//! in to; a tick is tried before it's kept. The tab follows the list's
//! file: the accounts Mail and Calendar move here by themselves after
//! an upgrade (`olo_fastmail::adopt`) show as they come.

use std::sync::Arc;

use olo_fastmail::{self as fastmail, Account, Service};
use olo_kit::{
    Child, Cx, Element, IntoElement, ParentElement as _, Styled as _, div, metrics as m, px,
    widgets::{BBox, Button, CheckBox, StringView, TextControl, TextField, TextFieldEvent, button_width},
};
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
    /// Signed in to what it's ticked for: how many calendars, if Calendar.
    Signed(Option<usize>),
    Failed(String),
}

/// Adding one: being typed, being tried (what's going on), what was
/// wrong, or done.
#[derive(Clone, Debug, Default, PartialEq)]
enum Adding {
    #[default]
    Typing,
    Trying(String),
    Failed(String),
    Done,
}

type Listed = Arc<Mutex<Option<Vec<(Account, Stands)>>>>;

pub struct State {
    /// The accounts and how each stands (None: not read yet).
    accounts: Listed,
    adding: bool,
    trying: Arc<Mutex<Adding>>,
    address: Child<TextField>,
    password: Child<TextField>,
    proxy: olo_kit::Proxy,
    /// The list's file, followed.
    _watch: Option<olo_kit::watch::Watch>,
}

/// The Fastmail tab's part of the Integrations panel's state, in its view.
fn state_of(v: &mut PanelView) -> Option<&mut State> {
    crate::integrations::state_of(v).map(|i| &mut i.fastmail)
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

/// `address` signed in to `service` with `password`: how many calendars,
/// for Calendar; why not, in words. Blocking.
fn try_service(address: &str, password: &str, service: Service) -> Result<Option<usize>, String> {
    match service {
        Service::Mail => {
            let account = olo_mail::Account { provider: olo_mail::Provider::Fastmail, address: address.to_owned() };
            olo_mail::check(&account, password).map(|()| None).map_err(|e| format!("Mail: {}", sentence(&e)))
        }
        Service::Calendar => {
            use olo_calendar::account::{self, Provider, Secret};
            let account = account::Account { provider: Provider::Fastmail, address: address.to_owned(), server: String::new() };
            account::check(&account, &Secret::Password(password.to_owned())).map(|c| Some(c.len())).map_err(|e| match e {
                olo_calendar::dav::Error::SignIn => "Calendar: Fastmail didn’t take the app password for calendars (it needs access to them).".to_owned(),
                e => format!("Calendar: {}", sentence(&e.to_string())),
            })
        }
    }
}

/// How `account` stands: signed in to each thing it's ticked for.
/// Blocking.
fn stands(account: &Account) -> Stands {
    let Some(password) = fastmail::password(&account.address) else {
        return Stands::Failed("Its app password isn’t in the keyring: remove it and add it again.".into());
    };
    let mut calendars = None;
    for s in Service::ALL.into_iter().filter(|s| account.ticked(*s)) {
        match try_service(&account.address, &password, s) {
            Ok(n) => calendars = calendars.or(n),
            Err(why) => return Stands::Failed(why),
        }
    }
    Stands::Signed(calendars)
}

/// Read the accounts, and try each, on a worker. (Mail's and Calendar's
/// accounts are read first: whichever reads them first after an upgrade
/// moves their Fastmail ones here.)
fn check(accounts: Listed, proxy: olo_kit::Proxy) {
    std::thread::spawn(move || {
        let _ = olo_mail::accounts();
        let _ = olo_calendar::account::accounts();
        let listed = fastmail::accounts();
        *accounts.lock().unwrap() = Some(listed.iter().map(|a| (a.clone(), Stands::Checking)).collect());
        proxy.send(Msg::Redraw);
        for a in listed {
            let now = stands(&a);
            if let Some(list) = accounts.lock().unwrap().as_mut()
                && let Some(entry) = list.iter_mut().find(|(b, _)| *b == a)
            {
                entry.1 = now;
            }
            proxy.send(Msg::Redraw);
        }
    });
}

impl State {
    pub fn new(cx: &mut Cx<PanelView>) -> State {
        let field = |cx: &mut Cx<PanelView>, hint: &str, password: bool| {
            let hint = hint.to_owned();
            let f = crate::form::text_field(cx, |f| f.hint(hint).password(password));
            // Return adds it, as the default button does.
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
        let mut st = State {
            accounts: Arc::default(),
            adding: false,
            trying: Arc::default(),
            address: field(cx, "you@fastmail.com", false),
            password: field(cx, "xxxx xxxx xxxx xxxx", true),
            proxy: olo_kit::proxy(),
            _watch: None,
        };
        // (Tests, the layout lint, never read the keyring or the network:
        // one account used for both, one for mail that no longer signs in.)
        if cfg!(test) {
            *st.accounts.lock().unwrap() = Some(vec![
                (Account { address: "ada@example.org".into(), services: vec![Service::Mail, Service::Calendar] }, Stands::Signed(Some(3))),
                (
                    Account { address: "ada.lovelace@analytical-engines.example.net".into(), services: vec![Service::Mail] },
                    Stands::Failed("Mail: Fastmail didn’t take the address and app password.".into()),
                ),
            ]);
        } else {
            st.check();
            let (accounts, proxy) = (st.accounts.clone(), st.proxy.clone());
            st._watch = olo_kit::watch::watch([fastmail::path()], move |_| check(accounts.clone(), proxy.clone()));
        }
        st
    }

    fn check(&self) {
        check(self.accounts.clone(), self.proxy.clone());
    }

    /// `service` ticked or not for `address`. Ticked, it's tried with
    /// the account's password first, and kept only if it signs in (else
    /// the account says why); unticked, at once.
    fn set(&self, address: String, service: Service, on: bool) {
        let (accounts, proxy) = (self.accounts.clone(), self.proxy.clone());
        let mark = move |now: Stands, address: &str| {
            if let Some(list) = accounts.lock().unwrap().as_mut()
                && let Some(entry) = list.iter_mut().find(|(a, _)| a.address == address)
            {
                entry.1 = now;
            }
        };
        if on {
            mark(Stands::Checking, &address);
        }
        std::thread::spawn(move || {
            if on {
                let tried = match fastmail::password(&address) {
                    None => Err("Its app password isn’t in the keyring: remove it and add it again.".to_owned()),
                    Some(p) => try_service(&address, &p, service),
                };
                if let Err(why) = tried {
                    mark(Stands::Failed(why), &address);
                    proxy.send(Msg::Redraw);
                    return;
                }
            }
            // (The tab hears its file change, and reads it again.)
            if let Err(e) = fastmail::tick(&address, service, on) {
                tracing::warn!("fastmail accounts: couldn’t save ({e})");
            }
            proxy.send(Msg::Redraw);
        });
    }

    /// Try what's typed for Mail and for Calendar, and keep the account,
    /// ticked for what it signed in to.
    fn connect(&mut self, cx: &mut Cx<PanelView>) {
        let address = self.address.read(cx).text().trim().to_lowercase();
        let password = self.password.read(cx).text().trim().to_owned();
        if !address.contains('@') || password.is_empty() || matches!(*self.trying.lock().unwrap(), Adding::Trying(_)) {
            return;
        }
        *self.trying.lock().unwrap() = Adding::Trying("Signing in to Fastmail…".into());
        let (trying, proxy) = (self.trying.clone(), self.proxy.clone());
        std::thread::spawn(move || {
            let mut works = Vec::new();
            let mut why = Vec::new();
            for s in Service::ALL {
                match try_service(&address, &password, s) {
                    Ok(_) => works.push(s),
                    Err(e) => why.push(e),
                }
            }
            let now = if works.is_empty() {
                Adding::Failed(format!("Fastmail didn’t take the address and app password: {}", why.join(" ")))
            } else {
                // (The tab hears its file change, and reads it again.)
                match fastmail::add(&address, &password, &works) {
                    Ok(()) => Adding::Done,
                    Err(e) => Adding::Failed(sentence(&e)),
                }
            };
            *trying.lock().unwrap() = now;
            proxy.send(Msg::Redraw);
        });
    }

    /// Disconnect an account: off the list, its password forgotten.
    fn remove(&self, address: String) {
        if let Some(list) = self.accounts.lock().unwrap().as_mut() {
            list.retain(|(a, _)| a.address != address);
        }
        std::thread::spawn(move || {
            if let Err(e) = fastmail::remove(&address) {
                tracing::warn!("fastmail accounts: couldn’t save ({e})");
            }
        });
    }

    fn stop_adding(&mut self, cx: &mut Cx<PanelView>) {
        self.adding = false;
        *self.trying.lock().unwrap() = Adding::Typing;
        self.password.update(cx, |f, cx| f.clear_secret(cx));
        self.address.update(cx, |f, cx| f.set_text("", cx));
    }
}

fn act(ui: &Ui, f: impl Fn(&mut State, &mut Cx<PanelView>) + 'static) -> impl Fn(&olo_kit::ClickEvent, &mut olo_kit::WindowCx) + 'static {
    ui.on_click_view(move |v, cx| {
        if let Some(st) = state_of(v) {
            f(st, cx);
        }
        cx.notify();
    })
}

/// A tick's column's width, as Google's tab has it.
const COLUMN: f32 = 76.0;

pub fn ui(ui: &mut Ui, st: &mut State) {
    if *st.trying.lock().unwrap() == Adding::Done {
        st.stop_adding(ui.cx);
    }
    let listed = st.accounts.lock().unwrap().clone();
    let trying = st.trying.lock().unwrap().clone();
    let busy = matches!(trying, Adding::Trying(_));
    let status = crate::integrations::how_many(listed.as_ref().map(|l| (l.iter().any(|(_, s)| *s == Stands::Checking), l.iter().filter(|(_, s)| matches!(s, Stands::Signed(_))).count(), l.len())));
    let mut items: Vec<Element> = vec![
        service(
            "mail",
            "Mail and Calendar sign in to Fastmail with an app password of each account’s own, with access to mail and calendars. Tick what each account is used for: Mail looks in its new mail for one-time codes, Calendar shows its calendars.",
            status,
        )
        .into_element(),
    ];
    // The grid: each account, what it's used for, and its button.
    let wide = button_width("Remove", false);
    let row = || div().flex().flex_row().items_center().gap(px(m::SPACING));
    let none = Vec::new();
    let list = listed.as_ref().unwrap_or(&none);
    if !list.is_empty() {
        let mut head = row().child(div().flex_1().min_w_0().child(StringView::new("Account").dim(true)));
        for s in Service::ALL {
            head = head.child(div().w(px(COLUMN)).flex_shrink_0().flex().justify_center().child(StringView::new(s.name()).dim(true)));
        }
        items.push(head.child(div().w(px(wide)).flex_shrink_0()).into_element());
        for (account, stands) in list {
            let how = match stands {
                Stands::Checking => "checking…".to_owned(),
                Stands::Signed(Some(1)) => "signed in, 1 calendar".to_owned(),
                Stands::Signed(Some(n)) => format!("signed in, {n} calendars"),
                Stands::Signed(None) => "signed in".to_owned(),
                Stands::Failed(why) => why.clone(),
            };
            let mut r = row().child(div().flex_1().min_w_0().flex().flex_col().child(StringView::new(account.address.clone()).cut(true)).child(StringView::new(how).small(true).dim(true).wrap(true)));
            for s in Service::ALL {
                let address = account.address.clone();
                let tick = CheckBox::new(format!("{} for {}", s.name(), account.address), account.ticked(s))
                    .mark_only()
                    .id(format!("fastmail-{}-{}", s.name(), account.address))
                    .enabled(!busy && *stands != Stands::Checking)
                    .on_toggle(ui.on_view(move |v: &mut PanelView, on: &bool, cx| {
                        if let Some(st) = state_of(v) {
                            st.set(address.clone(), s, *on);
                        }
                        cx.notify();
                    }));
                r = r.child(div().w(px(COLUMN)).flex_shrink_0().flex().justify_center().child(tick));
            }
            let address = account.address.clone();
            let remove = Button::new("Remove").id(format!("fastmail-remove-{address}")).enabled(!busy).on_click(act(ui, move |st, _| st.remove(address.clone())));
            items.push(r.child(div().w(px(wide)).flex_shrink_0().flex().flex_row().justify_end().child(remove)).into_element());
        }
    }
    if st.adding {
        let divider = crate::form::divider_for(&["Address:", "App password:"]);
        items.push(TextControl::new(ui.cx, &st.address).label("Address:").divider(divider).into_element());
        items.push(TextControl::new(ui.cx, &st.password).label("App password:").divider(divider).into_element());
        items.push(
            StringView::new(
                "Make an app password for Olo in Fastmail (Settings ▸ Privacy & Security ▸ Manage app passwords and access) with access to mail and calendars. It’s kept in your login keyring.",
            )
            .dim(true)
            .wrap(true)
            .into_element(),
        );
        match &trying {
            Adding::Trying(what) => items.push(StringView::new(what.clone()).wrap(true).into_element()),
            Adding::Failed(why) => items.push(StringView::new(why.clone()).wrap(true).into_element()),
            _ => {}
        }
        let ready = !busy && st.address.read(ui.cx).text().contains('@') && !st.password.read(ui.cx).text().trim().is_empty();
        items.push(
            buttons(vec![
                Button::new("Add").default(true).enabled(ready).on_click(act(ui, |st, cx| st.connect(cx))).into_element(),
                Button::new("Cancel").on_click(act(ui, |st, cx| st.stop_adding(cx))).into_element(),
                Button::new("Make an App Password…")
                    .on_click(|_, _| {
                        let _ = olo_xdg::open::open(&olo_xdg::open::Target::Url(fastmail::APP_PASSWORDS.into()));
                    })
                    .into_element(),
            ])
            .into_element(),
        );
    } else {
        let mut row = Vec::new();
        if !list.is_empty() {
            let checking = list.iter().any(|(_, s)| *s == Stands::Checking);
            row.push(Button::new("Check Again").id("fastmail-check").enabled(!checking).on_click(act(ui, |st, _| st.check())).into_element());
        }
        let address = st.address.downgrade();
        row.push(
            Button::new("Add a Fastmail Account…")
                .id("fastmail-add")
                .on_click(ui.on_click_view(move |v, cx| {
                    if let Some(st) = state_of(v) {
                        st.adding = true;
                        *st.trying.lock().unwrap() = Adding::Typing;
                    }
                    address.focus(cx);
                    cx.notify();
                }))
                .into_element(),
        );
        // (The first given is rightmost.)
        row.reverse();
        items.push(buttons(row).into_element());
    }
    ui.add(BBox::new("Fastmail").children(items));
}
