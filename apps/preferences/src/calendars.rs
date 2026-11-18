//! Integrations ▸ Calendars: the accounts whose calendars Calendar shows
//! together and writes to (`olo_calendar::account`; Ben, 2026-11-17: "add
//! calendar support to prefs -> integrations and support any number of
//! google calendars (eg from multiple google accounts) as well as any
//! number of fastmail calendars"). Fastmail's (and any other CalDAV
//! server's) is added with its address and an app password; Google's by
//! signing in to Google in the browser, with the person's own OAuth
//! client (the one DiskBackup's Google Drive set-up made serves here too:
//! it's found by itself; otherwise its ID and secret are asked for).
//! Whatever is typed is tried first, on a worker (signed in, its
//! calendars listed): then what signs it in is kept in the login keyring
//! and the account in `calendar-accounts.json`, which Calendar follows.
//! As the panel opens each is tried again, and says how it stands.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

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

/// Where the fields begin, past the longest of their labels.
const LABELS: f32 = 100.0;

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

/// A Google client to sign in with, already on this computer or not.
#[derive(Clone, Debug, Default, PartialEq)]
enum Client {
    #[default]
    Looking,
    Found(olo_calendar::account::GoogleClient),
    None,
}

pub struct State {
    /// The accounts set up and how each stands (None: not read yet).
    accounts: Arc<Mutex<Option<Vec<(Account, Stands)>>>>,
    /// Whose kind is being added, while one is.
    adding: Option<Provider>,
    trying: Arc<Mutex<Adding>>,
    client: Arc<Mutex<Client>>,
    /// Stops a sign-in that waits for the browser.
    cancel: Arc<AtomicBool>,
    address: Child<TextField>,
    password: Child<TextField>,
    server: Child<TextField>,
    client_id: Child<TextField>,
    client_secret: Child<TextField>,
    proxy: olo_kit::Proxy,
}

/// The Calendars part of the Integrations panel's state, in its view.
fn state_of(v: &mut PanelView) -> Option<&mut State> {
    crate::integrations::state_of(v).map(|i| &mut i.calendars)
}

/// What went wrong with an account, in words.
fn said(provider: Provider, e: &dav::Error) -> String {
    match (provider, e) {
        (Provider::Google, dav::Error::SignIn) => "Google signed you in but wouldn’t give its calendars: the CalDAV API must be turned on in the client’s Google Cloud project (Turn On the CalDAV API…), or the sign-in was ended: sign in again.".into(),
        (Provider::Fastmail, dav::Error::SignIn) => "Fastmail didn’t take the address and app password (it needs one with access to calendars).".into(),
        (_, dav::Error::SignIn) => "The server didn’t take the name and password.".into(),
        (_, e) => {
            let mut s = e.to_string();
            if let Some(first) = s.chars().next() {
                s.replace_range(..first.len_utf8(), &first.to_uppercase().to_string());
            }
            format!("{s}.")
        }
    }
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
        let st = State {
            accounts: Arc::default(),
            adding: None,
            trying: Arc::default(),
            client: Arc::default(),
            cancel: Arc::default(),
            address: field(cx, "you@example.com", false),
            password: field(cx, "xxxx xxxx xxxx xxxx", true),
            server: field(cx, "https://cloud.example.org/remote.php/dav/", false),
            client_id: field(cx, "….apps.googleusercontent.com", false),
            client_secret: field(cx, "GOCSPX-…", true),
            proxy: olo_kit::proxy(),
        };
        // (Tests, the layout lint, never read the keyring or the network:
        // an account of each kind, one that no longer signs in.)
        if cfg!(test) {
            *st.accounts.lock().unwrap() = Some(vec![
                (Account { provider: Provider::Google, address: "ada@example.com".into(), server: String::new() }, Stands::Connected(4)),
                (Account { provider: Provider::Fastmail, address: "ada@example.org".into(), server: String::new() }, Stands::Failed(said(Provider::Fastmail, &dav::Error::SignIn))),
            ]);
            *st.client.lock().unwrap() = Client::None;
        } else {
            st.check();
        }
        st
    }

    /// Read the accounts, and try each, on a worker.
    fn check(&self) {
        let (accounts, proxy) = (self.accounts.clone(), self.proxy.clone());
        std::thread::spawn(move || {
            let listed = account::accounts();
            *accounts.lock().unwrap() = Some(listed.iter().map(|a| (a.clone(), Stands::Checking)).collect());
            proxy.send(Msg::Redraw);
            for a in listed {
                let now = match account::secret(&a) {
                    None => Stands::Failed("What signs it in isn’t in the keyring: remove it and add it again.".into()),
                    Some(secret) => match account::check(&a, &secret) {
                        Ok(calendars) => Stands::Connected(calendars.len()),
                        Err(e) => Stands::Failed(said(a.provider, &e)),
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

    /// Start adding an account of `provider`'s; for Google, a client
    /// already on this computer is looked for (the keyring: a worker).
    fn start_adding(&mut self, provider: Provider) {
        self.adding = Some(provider);
        *self.trying.lock().unwrap() = Adding::Typing;
        if provider == Provider::Google && !cfg!(test) {
            *self.client.lock().unwrap() = Client::Looking;
            let (client, proxy) = (self.client.clone(), self.proxy.clone());
            std::thread::spawn(move || {
                *client.lock().unwrap() = match account::known_google_client() {
                    Some(found) => Client::Found(found),
                    None => Client::None,
                };
                proxy.send(Msg::Redraw);
            });
        }
    }

    /// Whether what's typed is enough to try.
    fn ready(&self, cx: &Cx<PanelView>) -> bool {
        let typed = |f: &Child<TextField>| !f.read(cx).text().trim().is_empty();
        match self.adding {
            None => false,
            Some(Provider::Google) => match &*self.client.lock().unwrap() {
                Client::Looking => false,
                Client::Found(_) => true,
                Client::None => typed(&self.client_id) && typed(&self.client_secret),
            },
            Some(Provider::Fastmail) => self.address.read(cx).text().contains('@') && typed(&self.password),
            Some(Provider::Other) => typed(&self.server) && typed(&self.address) && typed(&self.password),
        }
    }

    /// Try what's typed (for Google: sign in, in the browser), and keep
    /// the account if its calendars can be listed.
    fn connect(&mut self, cx: &mut Cx<PanelView>) {
        let Some(provider) = self.adding else { return };
        if !self.ready(cx) || matches!(*self.trying.lock().unwrap(), Adding::Trying(_)) {
            return;
        }
        let read = |f: &Child<TextField>| f.read(cx).text().trim().to_owned();
        let (address, password, server) = (read(&self.address), self.password.read(cx).text().to_owned(), read(&self.server));
        let client = match &*self.client.lock().unwrap() {
            Client::Found(c) => c.clone(),
            _ => olo_calendar::account::GoogleClient { id: read(&self.client_id), secret: read(&self.client_secret) },
        };
        let waiting = if provider == Provider::Google { "Waiting for you to sign in to Google in the browser…".to_owned() } else { format!("Signing in to {}…", provider.name()) };
        *self.trying.lock().unwrap() = Adding::Trying(waiting);
        self.cancel.store(false, Ordering::Relaxed);
        let (accounts, trying, proxy, cancel) = (self.accounts.clone(), self.trying.clone(), self.proxy.clone(), self.cancel.clone());
        std::thread::spawn(move || {
            let signed: Result<(Account, Secret), String> = match provider {
                Provider::Google => account::google_sign_in(&client, &cancel, account::open_consent),
                Provider::Fastmail => Ok((Account { provider, address: address.to_lowercase(), server: String::new() }, Secret::Password(password.trim().to_owned()))),
                Provider::Other => {
                    let server = if server.contains("://") { server } else { format!("https://{server}") };
                    Ok((Account { provider, address, server }, Secret::Password(password.trim().to_owned())))
                }
            };
            let now = match signed {
                Err(why) if why == "stopped" => Adding::Typing,
                Err(why) => Adding::Failed(sentence(&why)),
                Ok((account, secret)) => {
                    *trying.lock().unwrap() = Adding::Trying("Looking for its calendars…".into());
                    proxy.send(Msg::Redraw);
                    match account::check(&account, &secret) {
                        Err(e) => Adding::Failed(said(provider, &e)),
                        Ok(calendars) => match account::keep_secret(&account, &secret) {
                            Err(e) => Adding::Failed(format!("What signs it in couldn’t be kept in the keyring ({e}).")),
                            Ok(()) => {
                                let mut listed = account::accounts();
                                listed.retain(|a| a.id() != account.id());
                                listed.push(account.clone());
                                match account::save_accounts(&listed) {
                                    Err(e) => Adding::Failed(format!("The account couldn’t be saved ({e}).")),
                                    Ok(()) => {
                                        let mut a = accounts.lock().unwrap();
                                        let list = a.get_or_insert_with(Vec::new);
                                        list.retain(|(b, _)| b.id() != account.id());
                                        list.push((account, Stands::Connected(calendars.len())));
                                        Adding::Done
                                    }
                                }
                            }
                        },
                    }
                }
            };
            *trying.lock().unwrap() = now;
            proxy.send(Msg::Redraw);
        });
    }

    /// Forget an account: what signs it in out of the keyring, itself
    /// off the list (Calendar lets go of what it kept of its calendars).
    fn remove(&self, id: String) {
        let (accounts, proxy) = (self.accounts.clone(), self.proxy.clone());
        if let Some(list) = accounts.lock().unwrap().as_mut() {
            list.retain(|(a, _)| a.id() != id);
        }
        std::thread::spawn(move || {
            let mut listed = account::accounts();
            for gone in listed.iter().filter(|a| a.id() == id) {
                account::forget_secret(gone);
            }
            listed.retain(|a| a.id() != id);
            if let Err(e) = account::save_accounts(&listed) {
                tracing::warn!("calendar accounts: couldn’t save ({e})");
            }
            proxy.send(Msg::Redraw);
        });
    }

    fn stop_adding(&mut self, cx: &mut Cx<PanelView>) {
        // (A sign-in waiting for the browser is let go.)
        self.cancel.store(true, Ordering::Relaxed);
        self.adding = None;
        *self.trying.lock().unwrap() = Adding::Typing;
        for secret in [&self.password, &self.client_secret] {
            secret.update(cx, |f, cx| f.clear_secret(cx));
        }
        for plain in [&self.address, &self.server, &self.client_id] {
            plain.update(cx, |f, cx| f.set_text("", cx));
        }
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

fn open(url: &'static str) -> impl Fn(&olo_kit::ClickEvent, &mut olo_kit::WindowCx) + 'static {
    move |_, _| {
        let _ = olo_xdg::open::open(&olo_xdg::open::Target::Url(url.into()));
    }
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
        Some(l) if l.is_empty() => "No accounts connected.".to_owned(),
        Some(l) if l.iter().any(|(_, s)| *s == Stands::Checking) => "Checking…".to_owned(),
        Some(l) => match (l.iter().filter(|(_, s)| matches!(s, Stands::Connected(_))).count(), l.len()) {
            (1, 1) => "Connected.".to_owned(),
            (c, n) if c == n => format!("All {n} connected."),
            (c, n) => format!("{c} of {n} connected."),
        },
    };
    let mut items: Vec<Element> =
        vec![service("datetime", "Calendar shows these accounts’ calendars together, each in its colour, and writes what you change back to where it lives.", status).into_element()];
    for (account, stands) in listed.iter().flatten() {
        let how = match stands {
            Stands::Checking => "checking…".to_owned(),
            Stands::Connected(1) => "connected, 1 calendar".to_owned(),
            Stands::Connected(n) => format!("connected, {n} calendars"),
            Stands::Failed(why) => why.clone(),
        };
        let id = account.id();
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
                        .child(StringView::new(account.address.clone()).wrap(true))
                        .child(StringView::new(format!("{} · {how}", account.provider.name())).dim(true).wrap(true)),
                )
                .child(Button::new("Remove").id(format!("calendar-remove-{id}")).on_click(act(ui, move |st, _| st.remove(id.clone()))))
                .into_element(),
        );
    }
    if let Some(provider) = st.adding {
        let found = st.client.lock().unwrap().clone();
        let mut row: Vec<Element> = Vec::new();
        // (A link under the words, where a third button wouldn't fit beside the two.)
        let mut link: Option<(&'static str, &'static str)> = None;
        let how = match provider {
            Provider::Google => {
                match &found {
                    Client::Looking => {}
                    Client::Found(_) => {}
                    Client::None => {
                        items.push(TextControl::new(ui.cx, &st.client_id).label("Client ID:").divider(LABELS).into_element());
                        items.push(TextControl::new(ui.cx, &st.client_secret).label("Client secret:").divider(LABELS).into_element());
                    }
                }
                link = Some(("Turn On the CalDAV API…", account::GOOGLE_CALDAV_API));
                match found {
                    Client::Looking => "Looking for a Google client on this computer…".to_owned(),
                    Client::Found(_) => "Olo signs in to Google with the client of your own that’s already on this computer (DiskBackup’s Google Drive made it, or another calendar account did). The CalDAV API must be turned on in that client’s Google Cloud project. Then sign in: Google asks in your browser whether Olo may see and change your calendars.".to_owned(),
                    Client::None => "Olo signs in to Google with an OAuth client of your own: a Desktop app’s, made in a Google Cloud project of yours (DiskBackup’s Google Drive set-up makes one, step by step, and it serves here too), with the CalDAV API turned on in that project. Give its ID and secret, then sign in: Google asks in your browser whether Olo may see and change your calendars.".to_owned(),
                }
            }
            Provider::Fastmail => {
                items.push(TextControl::new(ui.cx, &st.address).label("Address:").divider(LABELS).into_element());
                items.push(TextControl::new(ui.cx, &st.password).label("App password:").divider(LABELS).into_element());
                row.push(Button::new("Make an App Password…").on_click(open(account::FASTMAIL_APP_PASSWORDS)).into_element());
                "Your Fastmail address and an app password for it with access to calendars (Settings ▸ Privacy & Security ▸ Manage app passwords and access).".to_owned()
            }
            Provider::Other => {
                items.push(TextControl::new(ui.cx, &st.server).label("Server:").divider(LABELS).into_element());
                items.push(TextControl::new(ui.cx, &st.address).label("Name:").divider(LABELS).into_element());
                items.push(TextControl::new(ui.cx, &st.password).label("Password:").divider(LABELS).into_element());
                "Any CalDAV server (Nextcloud, iCloud with an app-specific password, a server of your own): its address, and the name and password it signs you in with.".to_owned()
            }
        };
        items.push(StringView::new(format!("{how} What signs you in is kept in your login keyring.")).dim(true).wrap(true).into_element());
        if let Some((label, url)) = link {
            items.push(div().flex().child(olo_kit::widgets::LinkButton::new(label).id("calendars-link").on_click(open(url))).into_element());
        }
        match &trying {
            Adding::Trying(what) => items.push(StringView::new(what.clone()).wrap(true).into_element()),
            Adding::Failed(why) => items.push(StringView::new(why.clone()).wrap(true).into_element()),
            _ => {}
        }
        let label = if provider == Provider::Google { "Sign In with Google" } else { "Connect" };
        let ready = st.ready(ui.cx) && !busy;
        let mut all = vec![
            Button::new(label).default(true).enabled(ready).on_click(act(ui, |st, cx| st.connect(cx))).into_element(),
            // (Cancel also lets go of a sign-in waiting for the browser.)
            Button::new("Cancel").on_click(act(ui, |st, cx| st.stop_adding(cx))).into_element(),
        ];
        all.extend(row);
        items.push(buttons(all).into_element());
    } else {
        let mut row = Vec::new();
        if listed.as_ref().is_some_and(|l| !l.is_empty()) {
            let checking = listed.iter().flatten().any(|(_, s)| *s == Stands::Checking);
            row.push(Button::new("Check Again").id("calendars-check").enabled(!checking).on_click(act(ui, |st, _| st.check())).into_element());
        }
        // One button for the three kinds (three, with Check Again, ran
        // out of the box): its menu says which.
        let fields = (st.address.downgrade(), st.server.downgrade());
        let choose = std::rc::Rc::new(ui.on_view(move |v: &mut PanelView, kind: &str, cx| {
            let Some(provider) = Provider::ALL.into_iter().find(|p| p.name() == kind) else { return };
            if let Some(st) = state_of(v) {
                st.start_adding(provider);
            }
            match provider {
                Provider::Google => {}
                Provider::Fastmail => fields.0.focus(cx),
                Provider::Other => fields.1.focus(cx),
            }
            cx.notify();
        }));
        row.push(
            Button::new("Add an Account…")
                .id("calendars-add")
                .on_click(move |_: &olo_kit::ClickEvent, cx| {
                    let menu = olo_kit::Menu::new("Add an Account")
                        .item("Google…", Provider::Google.name())
                        .item("Fastmail…", Provider::Fastmail.name())
                        .item("Another CalDAV Server…", Provider::Other.name());
                    let (at, choose) = (cx.listener_bounds(), choose.clone());
                    olo_kit::menu::popup(cx, menu, at, olo_kit::Corner::BottomLeft, move |kind, cx| choose(kind, cx));
                })
                .into_element(),
        );
        // (The first given is rightmost.)
        row.reverse();
        items.push(buttons(row).into_element());
    }
    ui.add(BBox::new("Calendars").children(items));
}
