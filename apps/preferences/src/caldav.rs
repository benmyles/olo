//! Integrations ▸ CalDAV: the accounts on CalDAV servers other than
//! Google's and Fastmail's (Nextcloud, iCloud with an app-specific
//! password, a server of one's own) whose calendars Calendar shows with
//! theirs and writes to (Ben, 2026-11-18: "just add a CalDAV tab in
//! integrations and have them managed in there"). Each is added with its
//! server's address, a name and a password; what's typed is tried first,
//! on a worker (signed in, its calendars listed), then the password is
//! kept in the login keyring and the account in `calendar-accounts.json`
//! (`olo_calendar::account`), which Calendar follows. The tab follows
//! that file too, and tries each as the panel opens and on Check Again.
//! (Google's and Fastmail's accounts have tabs of their own.)

use std::sync::Arc;

use olo_calendar::{
    account::{self, Account, Provider, Secret},
    dav,
};
use olo_kit::{
    Child, Cx, Element, IntoElement, ParentElement as _, Styled as _, div, metrics as m, px,
    widgets::{BBox, Button, StringView, TextControl, TextField, TextFieldEvent},
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
    /// Signed in: how many calendars it has.
    Connected(usize),
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
    server: Child<TextField>,
    name: Child<TextField>,
    password: Child<TextField>,
    proxy: olo_kit::Proxy,
    /// Calendar's list of accounts, followed: one added or removed
    /// elsewhere shows at once.
    _watch: Option<olo_kit::watch::Watch>,
}

/// The CalDAV tab's part of the Integrations panel's state, in its view.
fn state_of(v: &mut PanelView) -> Option<&mut State> {
    crate::integrations::state_of(v).map(|i| &mut i.caldav)
}

/// What went wrong with an account, in words.
fn said(e: &dav::Error) -> String {
    match e {
        dav::Error::SignIn => "The server didn’t take the name and password.".into(),
        e => sentence(&e.to_string()),
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

/// Read the accounts on other servers, and try each, on a worker.
fn check(accounts: Listed, proxy: olo_kit::Proxy) {
    std::thread::spawn(move || {
        let listed: Vec<Account> = account::own_accounts().into_iter().filter(|a| a.provider == Provider::Other).collect();
        *accounts.lock().unwrap() = Some(listed.iter().map(|a| (a.clone(), Stands::Checking)).collect());
        proxy.send(Msg::Redraw);
        for a in listed {
            let now = match account::secret(&a) {
                None => Stands::Failed("Its password isn’t in the keyring: remove it and add it again.".into()),
                Some(secret) => match account::check(&a, &secret) {
                    Ok(calendars) => Stands::Connected(calendars.len()),
                    Err(e) => Stands::Failed(said(&e)),
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
        let mut st = State {
            accounts: Arc::default(),
            adding: false,
            trying: Arc::default(),
            server: field(cx, "https://cloud.example.org/remote.php/dav/", false),
            name: field(cx, "you", false),
            password: field(cx, "the server’s password for you", true),
            proxy: olo_kit::proxy(),
            _watch: None,
        };
        // (Tests, the layout lint, never read the keyring or the network.)
        if cfg!(test) {
            *st.accounts.lock().unwrap() = Some(vec![
                (Account { provider: Provider::Other, address: "ada".into(), server: "https://cloud.example.org/remote.php/dav/".into() }, Stands::Connected(2)),
                (Account { provider: Provider::Other, address: "ada@example.net".into(), server: "https://dav.example.net/".into() }, Stands::Failed(said(&dav::Error::SignIn))),
            ]);
        } else {
            st.check();
            let (accounts, proxy) = (st.accounts.clone(), st.proxy.clone());
            st._watch = olo_kit::watch::watch([account::accounts_path()], move |_| check(accounts.clone(), proxy.clone()));
        }
        st
    }

    fn check(&self) {
        check(self.accounts.clone(), self.proxy.clone());
    }

    /// Whether what's typed is enough to try.
    fn ready(&self, cx: &Cx<PanelView>) -> bool {
        let typed = |f: &Child<TextField>| !f.read(cx).text().trim().is_empty();
        self.adding && typed(&self.server) && typed(&self.name) && typed(&self.password)
    }

    /// Try what's typed, and keep the account if its calendars can be
    /// listed.
    fn connect(&mut self, cx: &mut Cx<PanelView>) {
        if !self.ready(cx) || matches!(*self.trying.lock().unwrap(), Adding::Trying(_)) {
            return;
        }
        let read = |f: &Child<TextField>| f.read(cx).text().trim().to_owned();
        let (server, name, password) = (read(&self.server), read(&self.name), self.password.read(cx).text().to_owned());
        *self.trying.lock().unwrap() = Adding::Trying("Looking for its calendars…".into());
        let (trying, proxy) = (self.trying.clone(), self.proxy.clone());
        std::thread::spawn(move || {
            let server = if server.contains("://") { server } else { format!("https://{server}") };
            let account = Account { provider: Provider::Other, address: name, server };
            let secret = Secret::Password(password.trim().to_owned());
            let now = match account::check(&account, &secret) {
                Err(e) => Adding::Failed(said(&e)),
                Ok(_) => match account::keep_secret(&account, &secret) {
                    Err(e) => Adding::Failed(format!("The password couldn’t be kept in the keyring ({e}).")),
                    Ok(()) => {
                        let mut listed = account::own_accounts();
                        listed.retain(|a| a.id() != account.id());
                        listed.push(account);
                        // (The tab hears its file change, and reads it again.)
                        match account::save_accounts(&listed) {
                            Err(e) => Adding::Failed(format!("The account couldn’t be saved ({e}).")),
                            Ok(()) => Adding::Done,
                        }
                    }
                },
            };
            *trying.lock().unwrap() = now;
            proxy.send(Msg::Redraw);
        });
    }

    /// Forget an account: its password out of the keyring, it off the
    /// list (Calendar lets go of what it kept of its calendars).
    fn remove(&self, gone: Account) {
        if let Some(list) = self.accounts.lock().unwrap().as_mut() {
            list.retain(|(a, _)| *a != gone);
        }
        std::thread::spawn(move || {
            account::forget_secret(&gone);
            let mut listed = account::own_accounts();
            listed.retain(|a| a.id() != gone.id());
            if let Err(e) = account::save_accounts(&listed) {
                tracing::warn!("calendar accounts: couldn’t save ({e})");
            }
        });
    }

    fn stop_adding(&mut self, cx: &mut Cx<PanelView>) {
        self.adding = false;
        *self.trying.lock().unwrap() = Adding::Typing;
        self.password.update(cx, |f, cx| f.clear_secret(cx));
        for plain in [&self.server, &self.name] {
            plain.update(cx, |f, cx| f.set_text("", cx));
        }
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

pub fn ui(ui: &mut Ui, st: &mut State) {
    if *st.trying.lock().unwrap() == Adding::Done {
        st.stop_adding(ui.cx);
    }
    let listed = st.accounts.lock().unwrap().clone();
    let trying = st.trying.lock().unwrap().clone();
    let busy = matches!(trying, Adding::Trying(_));
    let status = match &listed {
        None => "Checking…".to_owned(),
        Some(l) if l.is_empty() => "None connected to Olo yet.".to_owned(),
        Some(l) if l.iter().any(|(_, s)| *s == Stands::Checking) => "Checking…".to_owned(),
        Some(l) => match (l.iter().filter(|(_, s)| matches!(s, Stands::Connected(_))).count(), l.len()) {
            (1, 1) => "Connected.".to_owned(),
            (c, n) if c == n => format!("All {n} connected."),
            (c, n) => format!("{c} of {n} connected."),
        },
    };
    let mut items: Vec<Element> = vec![
        service(
            "calendar",
            "Calendar shows the calendars of your accounts on other CalDAV servers (Nextcloud, iCloud with an app-specific password, a server of your own) beside Google’s and Fastmail’s, and writes what you change back to them.",
            status,
        )
        .into_element(),
    ];
    for (account, stands) in listed.iter().flatten() {
        let how = match stands {
            Stands::Checking => "checking…".to_owned(),
            Stands::Connected(1) => "connected, 1 calendar".to_owned(),
            Stands::Connected(n) => format!("connected, {n} calendars"),
            Stands::Failed(why) => why.clone(),
        };
        let server = account.server.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_owned();
        let gone = account.clone();
        items.push(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(m::SPACING))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(StringView::new(format!("{} at {server}", account.address)).cut(true))
                        .child(StringView::new(how).small(true).dim(true).wrap(true)),
                )
                .child(Button::new("Remove").id(format!("caldav-remove-{}", account.id())).enabled(!busy).on_click(act(ui, move |st, _| st.remove(gone.clone()))))
                .into_element(),
        );
    }
    if st.adding {
        let divider = crate::form::divider_for(&["Server:", "Name:", "Password:"]);
        items.push(TextControl::new(ui.cx, &st.server).label("Server:").divider(divider).into_element());
        items.push(TextControl::new(ui.cx, &st.name).label("Name:").divider(divider).into_element());
        items.push(TextControl::new(ui.cx, &st.password).label("Password:").divider(divider).into_element());
        items.push(StringView::new("The server’s CalDAV address, and the name and password it signs you in with. The password is kept in your login keyring.").dim(true).wrap(true).into_element());
        match &trying {
            Adding::Trying(what) => items.push(StringView::new(what.clone()).wrap(true).into_element()),
            Adding::Failed(why) => items.push(StringView::new(why.clone()).wrap(true).into_element()),
            _ => {}
        }
        let ready = st.ready(ui.cx) && !busy;
        items.push(
            buttons(vec![
                Button::new("Connect").default(true).enabled(ready).on_click(act(ui, |st, cx| st.connect(cx))).into_element(),
                Button::new("Cancel").on_click(act(ui, |st, cx| st.stop_adding(cx))).into_element(),
            ])
            .into_element(),
        );
    } else {
        let mut row = Vec::new();
        if listed.as_ref().is_some_and(|l| !l.is_empty()) {
            let checking = listed.iter().flatten().any(|(_, s)| *s == Stands::Checking);
            row.push(Button::new("Check Again").id("caldav-check").enabled(!checking).on_click(act(ui, |st, _| st.check())).into_element());
        }
        let server = st.server.downgrade();
        row.push(
            Button::new("Add a CalDAV Account…")
                .id("caldav-add")
                .on_click(ui.on_click_view(move |v, cx| {
                    if let Some(st) = state_of(v) {
                        st.adding = true;
                        *st.trying.lock().unwrap() = Adding::Typing;
                    }
                    server.focus(cx);
                    cx.notify();
                }))
                .into_element(),
        );
        // (The first given is rightmost.)
        row.reverse();
        items.push(buttons(row).into_element());
    }
    ui.add(BBox::new("CalDAV").children(items));
}
