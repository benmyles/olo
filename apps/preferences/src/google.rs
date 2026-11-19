//! Integrations ▸ Google: the Google accounts Olo is connected to, and
//! which of Mail, Calendar and DiskBackup use each (Ben, 2026-11-18:
//! "for each google workspace account you add you setup the "Olo"
//! google app … and that should handle DiskBackup, Email, Calendar,
//! anything else we add later … for each google workspace we should
//! have it be e.g. "Olo (ben@shiftupai.com)" … add as many google
//! workspaces as we want in prefs -> integrations and then choose which
//! ones are enabled for mail, calendar, diskbackup in like a grid").
//!
//! `olo_google::accounts` is the list and the sign-ins; this is what
//! the person sees of it:
//!
//! - a grid: each account, a tick for each service (one that its
//!   sign-in wasn't allowed can't be ticked: Sign In Again), Remove;
//! - Add a Google Account…: an agent makes the account's own app,
//!   "Olo (its address)", in Google Cloud and signs Olo in with it
//!   (`olo_google::setup`, on an agent screen, with the account's login
//!   from Passwords; it finishes with `olo-preferences
//!   --google-setup-done`, [`setup_done`]); or Set Up by Hand…: the
//!   steps, and the client's ID and secret, then Sign In with Google in
//!   the person's browser;
//! - DiskBackup's own app from before (it backed up to an account's
//!   Drive with an app it made itself, which alone sees what it wrote
//!   there): Bring It Into Olo… has the agent bring that same app up to
//!   date, so nothing of the backups moves.
//!
//! Ticking DiskBackup for an account backs up to its Drive (DiskBackup
//! does it: `olo-diskbackup --set-up-drive-account`); unticking stops
//! that (what's there stays).

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use olo_google::{
    accounts::{self, Account, Service},
    setup::{self, Task},
};
use olo_keychain::Shareable;
use olo_kit::{
    Child, Cx, Element, IntoElement, ParentElement as _, SharedString, Styled as _, div, metrics as m, px,
    widgets::{BBox, Button, CheckBox, LinkButton, MenuField, StringView, TextControl, TextField, button_width},
};
use olo_sync::Mutex;

use crate::{
    Msg, PanelView,
    form::Ui,
    integrations::{buttons, service},
};

/// How an account's sign-in stands.
#[derive(Clone, Debug, PartialEq)]
enum Stands {
    Checking,
    Signed,
    Failed(String),
}

/// Passwords' Google logins, for the agent to sign in with.
#[derive(Clone, Debug, Default, PartialEq)]
enum Logins {
    #[default]
    Reading,
    Read(Vec<Shareable>),
    Failed(String),
}

/// Adding an account (or bringing DiskBackup's app in): with the agent,
/// or by hand.
#[derive(Clone, Debug, PartialEq)]
enum Adding {
    Agent(Task),
    ByHand,
}

/// What's under way, or how it went.
#[derive(Clone, Debug, Default, PartialEq)]
enum Doing {
    #[default]
    Nothing,
    Busy(String),
    Said(String),
    /// The agent at work: what's said, and what it waits for: that
    /// account to be listed (bringing DiskBackup's app in), or any not
    /// listed when it started (a new one).
    Agent {
        said: String,
        account: Option<String>,
        known: Vec<String>,
    },
    /// The agent's account came: the message goes, and the adding with it.
    Arrived,
    Failed(String),
}

pub struct State {
    /// The accounts and how each stands (None: not read yet).
    accounts: Arc<Mutex<Option<Vec<(Account, Stands)>>>>,
    /// DiskBackup backs up to this account's Drive with its own app
    /// from before, and the account isn't connected: (address, client id).
    legacy: Arc<Mutex<Option<(String, String)>>>,
    /// The account DiskBackup backs up to, if Google Drive.
    drive: Arc<Mutex<Option<String>>>,
    adding: Option<Adding>,
    logins: Arc<Mutex<Logins>>,
    login: usize,
    doing: Arc<Mutex<Doing>>,
    /// Stops a sign-in waiting for the browser.
    cancel: Arc<AtomicBool>,
    client_id: Child<TextField>,
    client_secret: Child<TextField>,
    proxy: olo_kit::Proxy,
    /// Followed: the list changes (an agent finished, Calendar's or Mail's
    /// box unticked one), and is read again.
    _watch: Option<olo_kit::watch::Watch>,
}

/// The Google part of the Integrations panel's state, in its view.
fn state_of(v: &mut PanelView) -> Option<&mut State> {
    crate::integrations::state_of(v).map(|i| &mut i.google)
}

/// `service` ticked or not for `address`, the list saved (Mail's and
/// Calendar's boxes untick through this too). Blocking (a small file).
pub fn tick(address: &str, service: Service, on: bool) {
    let mut all = accounts::accounts();
    accounts::tick(&mut all, address, service, on);
    if let Err(e) = accounts::save(&all) {
        tracing::warn!("google accounts: couldn’t save ({e})");
    }
}

/// The OAuth client DiskBackup signed in to Google Drive with before
/// Olo's Google accounts (`olo_system::secrets::DISKBACKUP_GOOGLE`), and
/// the account it backs up to. Blocking (the keyring).
pub fn legacy_client() -> Option<olo_google::Client> {
    // (`OLO_DISKBACKUP_GOOGLE`: a test's, as DiskBackup takes it.)
    let kept = match std::env::var("OLO_DISKBACKUP_GOOGLE") {
        Ok(t) => t.into_bytes(),
        Err(_) => olo_system::secrets::lookup(olo_system::secrets::DISKBACKUP_GOOGLE.attributes).ok()??,
    };
    let v: serde_json::Value = serde_json::from_slice(&kept).ok()?;
    serde_json::from_value(v.get("client")?.clone()).ok()
}

/// The account DiskBackup backs up to, if it's a Google Drive.
fn diskbackup_account() -> Option<String> {
    match olo_protocol::diskbackup::Settings::load().target {
        olo_protocol::diskbackup::Target::GoogleDrive { account, .. } => Some(account),
        _ => None,
    }
}

impl State {
    pub fn new(cx: &mut Cx<PanelView>) -> State {
        let field = |cx: &mut Cx<PanelView>, hint: &str, password: bool| {
            let hint = hint.to_owned();
            crate::form::text_field(cx, |f| f.hint(hint).password(password))
        };
        let client_id = field(cx, "….apps.googleusercontent.com", false);
        let client_secret = field(cx, "GOCSPX-…", true);
        let mut st = State {
            accounts: Arc::default(),
            legacy: Arc::default(),
            drive: Arc::default(),
            adding: None,
            logins: Arc::default(),
            login: 0,
            doing: Arc::default(),
            cancel: Arc::default(),
            client_id,
            client_secret,
            proxy: olo_kit::proxy(),
            _watch: None,
        };
        // (Tests, the layout lint, never read the keyring or the network:
        // two accounts, one used for Mail and Calendar, the other for
        // Calendar and DiskBackup, whose sign-in wasn't allowed Mail.)
        if cfg!(test) {
            let all: Vec<String> = Service::ALL.iter().map(|s| s.scope().to_owned()).collect();
            *st.accounts.lock().unwrap() = Some(vec![
                (Account { address: "ada@example.com".into(), services: vec![Service::Mail, Service::Calendar], scopes: all.clone() }, Stands::Signed),
                (
                    Account { address: "ada@example.org".into(), services: vec![Service::Calendar, Service::DiskBackup], scopes: all.into_iter().filter(|s| s != Service::Mail.scope()).collect() },
                    Stands::Signed,
                ),
            ]);
            *st.drive.lock().unwrap() = Some("ada@example.org".into());
            *st.legacy.lock().unwrap() = Some(("ada.lovelace@analytical-engines.example.net".into(), "123.apps.googleusercontent.com".into()));
        } else {
            st.check();
            let (proxy, accounts, legacy, drive, doing) = (st.proxy.clone(), st.accounts.clone(), st.legacy.clone(), st.drive.clone(), st.doing.clone());
            st._watch = olo_kit::watch::watch([accounts::path()], move |_| {
                let (accounts, legacy, drive, doing, proxy) = (accounts.clone(), legacy.clone(), drive.clone(), doing.clone(), proxy.clone());
                std::thread::spawn(move || read(&accounts, &legacy, &drive, &doing, &proxy, false));
            });
        }
        st
    }

    /// Read the accounts, and try each one's sign-in, on a worker.
    fn check(&self) {
        let (accounts, legacy, drive, doing, proxy) = (self.accounts.clone(), self.legacy.clone(), self.drive.clone(), self.doing.clone(), self.proxy.clone());
        std::thread::spawn(move || read(&accounts, &legacy, &drive, &doing, &proxy, true));
    }

    /// Start adding (the agent's task, or by hand). The agent needs a
    /// login from Passwords: read on a worker (Passwords may ask to be
    /// unlocked).
    fn start(&mut self, adding: Adding) {
        *self.doing.lock().unwrap() = Doing::Nothing;
        self.cancel.store(true, Ordering::Relaxed);
        if matches!(adding, Adding::Agent(_)) {
            *self.logins.lock().unwrap() = Logins::Reading;
            let (logins, proxy) = (self.logins.clone(), self.proxy.clone());
            std::thread::spawn(move || {
                *logins.lock().unwrap() = match setup::google_logins() {
                    Ok(found) => Logins::Read(found),
                    Err(e) => Logins::Failed(e),
                };
                proxy.send(Msg::Redraw);
            });
        }
        self.login = 0;
        self.adding = Some(adding);
    }

    /// The agent started on its task, with the login chosen, on a worker.
    fn start_agent(&mut self, task: Task, claude: olo_protocol::ClaudeCode) {
        let Logins::Read(found) = self.logins.lock().unwrap().clone() else { return };
        let Some(login) = found.get(self.login).cloned() else { return };
        *self.doing.lock().unwrap() = Doing::Busy("Starting the agent…".into());
        let account = match &task {
            Task::Extend { address, .. } => Some(address.clone()),
            Task::Make => None,
        };
        let known: Vec<String> = self.accounts.lock().unwrap().iter().flatten().map(|(a, _)| a.address.clone()).collect();
        let (doing, proxy) = (self.doing.clone(), self.proxy.clone());
        std::thread::spawn(move || {
            *doing.lock().unwrap() = match setup::start(&login, &task, &claude) {
                Ok(screen) => Doing::Agent {
                    said: format!(
                        "The agent is at work in a Terminal window, on its agent screen “{screen}” (Agent Screens shows it): it makes the account’s app in Google Cloud and signs Olo in with it. The account shows here when it’s done; you can talk to the agent in Terminal meanwhile."
                    ),
                    account,
                    known,
                },
                Err(e) => Doing::Failed(sentence(&format!("the agent didn’t start: {e}"))),
            };
            proxy.send(Msg::Redraw);
        });
    }

    /// Sign in with `client` in the person's browser (by hand; again, for
    /// an account whose sign-in ended or wasn't allowed everything). With
    /// no client kept for it (its sign-in gone from the keyring), the
    /// by-hand form, saying where its client is.
    fn sign_in(&mut self, client: Option<olo_google::Client>, address: Option<String>) {
        let Some(client) = client else {
            self.start(Adding::ByHand);
            let whose = address.map_or_else(|| "the account’s".to_owned(), |a| format!("{a}’s"));
            *self.doing.lock().unwrap() = Doing::Failed(format!(
                "Olo doesn’t have {whose} app on this computer: give its client’s ID and secret (Google Cloud ▸ its Olo project ▸ Clients), or Cancel and Add a Google Account… to have the agent do it."
            ));
            return;
        };
        *self.doing.lock().unwrap() = Doing::Busy("Waiting for you to sign in to Google in the browser…".into());
        self.cancel.store(false, Ordering::Relaxed);
        let (doing, proxy, cancel, accounts_, legacy, drive) = (self.doing.clone(), self.proxy.clone(), self.cancel.clone(), self.accounts.clone(), self.legacy.clone(), self.drive.clone());
        std::thread::spawn(move || {
            let signed = accounts::sign_in(&client, address.as_deref(), &cancel, accounts::open_consent);
            let now = match signed {
                Err(why) if why == "stopped" => Doing::Nothing,
                Err(why) => Doing::Failed(sentence(&why)),
                Ok(signed) => match connect(&client, &signed) {
                    Ok(said) => Doing::Said(said),
                    Err(why) => Doing::Failed(sentence(&why)),
                },
            };
            *doing.lock().unwrap() = now;
            read(&accounts_, &legacy, &drive, &doing, &proxy, true);
        });
    }

    /// The client typed (by hand).
    fn typed_client(&self, cx: &Cx<PanelView>) -> olo_google::Client {
        olo_google::Client { id: self.client_id.read(cx).text().trim().to_owned(), secret: self.client_secret.read(cx).text().trim().to_owned() }
    }

    /// `service` for `address` ticked or not: DiskBackup is moved there
    /// (or stopped) by DiskBackup itself, first.
    fn set(&mut self, address: String, service: Service, on: bool) {
        // (At once on screen; the file follows.)
        if let Some(list) = self.accounts.lock().unwrap().as_mut() {
            let mut plain: Vec<Account> = list.iter().map(|(a, _)| a.clone()).collect();
            accounts::tick(&mut plain, &address, service, on);
            for ((a, _), b) in list.iter_mut().zip(plain) {
                *a = b;
            }
        }
        let (doing, proxy, drive) = (self.doing.clone(), self.proxy.clone(), self.drive.clone());
        if service != Service::DiskBackup {
            std::thread::spawn(move || tick(&address, service, on));
            return;
        }
        *doing.lock().unwrap() =
            Doing::Busy(if on { format!("DiskBackup is setting up its backups in {address}’s Google Drive…") } else { "DiskBackup is stopping its backups to Google Drive…".to_owned() });
        std::thread::spawn(move || {
            let args: Vec<&str> = if on { vec!["--set-up-drive-account", &address] } else { vec!["--disconnect-drive"] };
            let now = match crate::ask_program("olo-diskbackup", &args) {
                Ok(out) => {
                    tick(&address, Service::DiskBackup, on);
                    *drive.lock().unwrap() = on.then(|| address.clone());
                    Doing::Said(sentence(out.lines().last().unwrap_or(if on { "DiskBackup backs up there now" } else { "DiskBackup no longer backs up to Google Drive" })))
                }
                Err(why) => {
                    // (As it was: the file was never changed.)
                    Doing::Failed(sentence(&format!("DiskBackup couldn’t: {why}")))
                }
            };
            *doing.lock().unwrap() = now;
            proxy.send(Msg::Redraw);
        });
    }

    /// An account disconnected from Olo: what signs it in forgotten, off
    /// the list (Mail and Calendar let go of it; DiskBackup stops backing
    /// up to its Drive first, if it does).
    fn remove(&mut self, address: String) {
        let (accounts_, doing, proxy, drive) = (self.accounts.clone(), self.doing.clone(), self.proxy.clone(), self.drive.clone());
        let backs_up = drive.lock().unwrap().as_deref().is_some_and(|d| d.eq_ignore_ascii_case(&address));
        std::thread::spawn(move || {
            if backs_up && let Err(why) = crate::ask_program("olo-diskbackup", &["--disconnect-drive"]) {
                *doing.lock().unwrap() = Doing::Failed(sentence(&format!("DiskBackup backs up to its Drive and couldn’t stop: {why}")));
                proxy.send(Msg::Redraw);
                return;
            }
            let now = match accounts::remove(&address) {
                Ok(()) => Doing::Said(format!("{address} is disconnected from Olo. Its app stays in its Google Cloud project, to connect it again with.")),
                Err(e) => Doing::Failed(format!("The list of Google accounts couldn’t be saved ({e}).")),
            };
            if let Some(list) = accounts_.lock().unwrap().as_mut() {
                list.retain(|(a, _)| !a.address.eq_ignore_ascii_case(&address));
            }
            *doing.lock().unwrap() = now;
            proxy.send(Msg::Redraw);
        });
    }

    fn stop_adding(&mut self, cx: &mut Cx<PanelView>) {
        self.cancel.store(true, Ordering::Relaxed);
        self.adding = None;
        self.client_secret.update(cx, |f, cx| f.clear_secret(cx));
        self.client_id.update(cx, |f, cx| f.set_text("", cx));
    }
}

/// The accounts read (each one's sign-in tried, with `check`), and
/// whether DiskBackup has an app of its own from before.
fn read(
    accounts_: &Arc<Mutex<Option<Vec<(Account, Stands)>>>>,
    legacy: &Arc<Mutex<Option<(String, String)>>>,
    drive: &Arc<Mutex<Option<String>>>,
    doing: &Arc<Mutex<Doing>>,
    proxy: &olo_kit::Proxy,
    check: bool,
) {
    let listed = accounts::accounts();
    // (The account the agent was making, or bringing in, is here: its
    // message goes.)
    {
        let mut d = doing.lock().unwrap();
        if let Doing::Agent { account, known, .. } = &*d {
            let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
            let came = listed.iter().any(|l| match account {
                Some(wanted) => same(&l.address, wanted),
                None => !known.iter().any(|k| same(k, &l.address)),
            });
            if came {
                *d = Doing::Arrived;
            }
        }
    }
    {
        let mut held = accounts_.lock().unwrap();
        let before = held.take().unwrap_or_default();
        *held = Some(
            listed
                .iter()
                .map(|a| {
                    let was = before.iter().find(|(b, _)| b.address == a.address).map(|(_, s)| s.clone());
                    (a.clone(), if check { Stands::Checking } else { was.unwrap_or(Stands::Checking) })
                })
                .collect(),
        );
    }
    let backs_up = diskbackup_account();
    *drive.lock().unwrap() = backs_up.clone();
    *legacy.lock().unwrap() = backs_up.filter(|a| !listed.iter().any(|l| l.address.eq_ignore_ascii_case(a))).and_then(|a| legacy_client().map(|c| (a, c.id)));
    proxy.send(Msg::Redraw);
    let unchecked: Vec<Account> = accounts_.lock().unwrap().iter().flatten().filter(|(_, s)| *s == Stands::Checking).map(|(a, _)| a.clone()).collect();
    for a in unchecked {
        let now = match accounts::token(&a.address, true, "Olo Preferences") {
            Ok(_) => Stands::Signed,
            Err(accounts::NoToken::NotConnected) => Stands::Failed("Its sign-in isn’t in the keyring: sign in again.".into()),
            Err(accounts::NoToken::Network(e)) => Stands::Failed(sentence(&e)),
            Err(accounts::NoToken::Refused(_)) => Stands::Failed("Google no longer takes its sign-in: sign in again.".into()),
        };
        if let Some(list) = accounts_.lock().unwrap().as_mut()
            && let Some(entry) = list.iter_mut().find(|(b, _)| b.address == a.address)
        {
            entry.1 = now;
        }
        proxy.send(Msg::Redraw);
    }
}

/// Signed in with `client`: the account listed (DiskBackup's column
/// ticked where DiskBackup backs up to its Drive already), and what it
/// may be used for said. Blocking.
fn connect(client: &olo_google::Client, signed: &olo_google::SignedIn) -> Result<String, String> {
    let account = accounts::connected(client, signed)?;
    if diskbackup_account().is_some_and(|d| d.eq_ignore_ascii_case(&account.address)) {
        tick(&account.address, Service::DiskBackup, true);
    }
    let missing: Vec<&str> = Service::ALL.iter().filter(|s| !account.allowed(**s)).map(|s| s.name()).collect();
    Ok(match missing.as_slice() {
        [] => format!("Connected {}.", account.address),
        some => format!("Connected {}, but its sign-in wasn’t allowed {}: Sign In Again and allow everything it asks.", account.address, some.join(" or ")),
    })
}

/// What went wrong, as a sentence.
fn sentence(why: &str) -> String {
    let mut s = why.trim().to_owned();
    if let Some(first) = s.chars().next() {
        s.replace_range(..first.len_utf8(), &first.to_uppercase().to_string());
    }
    if !s.ends_with('.') && !s.ends_with('…') && !s.ends_with('?') {
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

/// A service's column: wide enough for its name.
const COLUMN: f32 = 76.0;

pub fn ui(ui: &mut Ui, st: &mut State) {
    {
        let mut d = st.doing.lock().unwrap();
        if *d == Doing::Arrived {
            *d = Doing::Nothing;
            if matches!(st.adding, Some(Adding::Agent(_))) {
                st.adding = None;
            }
        }
    }
    let listed = st.accounts.lock().unwrap().clone();
    let doing = st.doing.lock().unwrap().clone();
    let busy = matches!(doing, Doing::Busy(_));
    let status = crate::integrations::how_many(listed.as_ref().map(|l| (l.iter().any(|(_, s)| *s == Stands::Checking), l.iter().filter(|(_, s)| *s == Stands::Signed).count(), l.len())));
    let mut items: Vec<Element> = vec![
        service(
            "people",
            "Mail, Calendar and DiskBackup sign in to Google through an app of each account’s own, “Olo (its address)”, in its Google Cloud project: an agent makes it for you. Tick what each account is used for.",
            status,
        )
        .into_element(),
    ];
    // The grid: each account, what it's used for, and its buttons; and
    // the account DiskBackup backs up to with its own app from before, as
    // a row of it.
    let wide = button_width("Sign In Again", false) + m::SPACING + button_width("Remove", false);
    let row = || div().flex().flex_row().items_center().gap(px(m::SPACING));
    let legacy = st.legacy.lock().unwrap().clone();
    let none = Vec::new();
    let list = listed.as_ref().unwrap_or(&none);
    if !list.is_empty() || legacy.is_some() {
        let mut head = row().child(div().flex_1().min_w_0().child(StringView::new("Account").dim(true)));
        for s in Service::ALL {
            head = head.child(div().w(px(COLUMN)).flex_shrink_0().flex().justify_center().child(StringView::new(s.name()).dim(true)));
        }
        items.push(head.child(div().w(px(wide)).flex_shrink_0()).into_element());
        for (account, stands) in list {
            let how = match stands {
                Stands::Checking => "checking…".to_owned(),
                Stands::Signed => "signed in".to_owned(),
                Stands::Failed(why) => why.clone(),
            };
            let not_allowed: Vec<&str> = Service::ALL.iter().filter(|s| !account.allowed(**s)).map(|s| s.name()).collect();
            let how = if not_allowed.is_empty() || *stands != Stands::Signed { how } else { format!("{how}; not allowed {} (sign in again)", not_allowed.join(", ")) };
            let mut r = row().child(div().flex_1().min_w_0().flex().flex_col().child(StringView::new(account.address.clone()).cut(true)).child(StringView::new(how).small(true).dim(true).wrap(true)));
            for s in Service::ALL {
                let address = account.address.clone();
                let tick = CheckBox::new(format!("{} for {}", s.name(), account.address), account.ticked(s) && account.allowed(s))
                    .mark_only()
                    .id(format!("google-{}-{}", s.name(), account.address))
                    .enabled(account.allowed(s) && !busy)
                    .on_toggle(ui.on_view(move |v: &mut PanelView, on: &bool, cx| {
                        if let Some(st) = state_of(v) {
                            st.set(address.clone(), s, *on);
                        }
                        cx.notify();
                    }));
                r = r.child(div().w(px(COLUMN)).flex_shrink_0().flex().justify_center().child(tick));
            }
            let mut acts = div().w(px(wide)).flex_shrink_0().flex().flex_row().justify_end().gap(px(m::SPACING));
            if *stands != Stands::Signed && *stands != Stands::Checking || !not_allowed.is_empty() {
                let address = account.address.clone();
                acts = acts.child(Button::new("Sign In Again").id(format!("google-again-{address}")).enabled(!busy).on_click(act(ui, move |st, _| {
                    let client = accounts::secret(&address).map(|s| s.client);
                    st.sign_in(client, Some(address.clone()));
                })));
            }
            let address = account.address.clone();
            acts = acts.child(Button::new("Remove").id(format!("google-remove-{address}")).enabled(!busy).on_click(act(ui, move |st, _| st.remove(address.clone()))));
            items.push(r.child(acts).into_element());
        }
        // (Its DiskBackup tick shown as it is: the app is DiskBackup's.)
        if let Some((address, client_id)) = &legacy {
            let mut r = row().child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(StringView::new(address.clone()).cut(true))
                    .child(StringView::new("DiskBackup’s own app, from before. Bring it in for Mail and Calendar too; the backups stay put.").small(true).dim(true).wrap(true)),
            );
            for s in Service::ALL {
                let tick = CheckBox::new(format!("{} for {}", s.name(), address), s == Service::DiskBackup).mark_only().id(format!("google-{}-{}", s.name(), address)).enabled(false);
                r = r.child(div().w(px(COLUMN)).flex_shrink_0().flex().justify_center().child(tick));
            }
            let (a, c) = (address.clone(), client_id.clone());
            let bring = Button::new("Bring It Into Olo…")
                .id("google-bring-in")
                .enabled(!busy && st.adding.is_none())
                .on_click(act(ui, move |st, _| st.start(Adding::Agent(Task::Extend { address: a.clone(), client_id: c.clone() }))));
            items.push(r.child(div().w(px(wide)).flex_shrink_0().flex().flex_row().justify_end().child(bring)).into_element());
        }
    }
    match st.adding.clone() {
        Some(Adding::Agent(task)) => {
            let logins = st.logins.lock().unwrap().clone();
            match &logins {
                Logins::Reading => items.push(StringView::new("Reading your logins in Passwords…").wrap(true).into_element()),
                Logins::Failed(e) => items.push(StringView::new(sentence(&format!("Passwords couldn’t give its logins: {e}"))).wrap(true).into_element()),
                Logins::Read(found) if found.is_empty() => {
                    items.push(StringView::new("Passwords has no login for a Google account: add the account’s login there first, or set the app up by hand.").wrap(true).into_element())
                }
                Logins::Read(found) => {
                    let names: Vec<SharedString> = found.iter().map(|l| SharedString::from(setup::label(l))).collect();
                    let pick = ui.on_view(|v: &mut PanelView, i: &usize, cx| {
                        if let Some(st) = state_of(v) {
                            st.login = *i;
                        }
                        cx.notify();
                    });
                    items.push(div().flex().child(MenuField::new("google-login", names, st.login.min(found.len() - 1)).label("Google account’s login:").enabled(!busy).on_change(pick)).into_element());
                }
            }
            let what = match &task {
                Task::Make => {
                    "An agent (Claude Code, in a Terminal window: Preferences ▸ AI) signs in to Google Cloud as that account on an agent screen of its own, with the login shared from Passwords (it never sees the password), makes the account’s app there, and signs Olo in with it. You follow along, and can talk to it there."
                }
                Task::Extend { .. } => {
                    "An agent (Claude Code, in a Terminal window) signs in to Google Cloud as that account on an agent screen of its own, with the login shared from Passwords (it never sees the password), brings DiskBackup’s app up to date, and signs Olo in with it. DiskBackup goes on backing up throughout."
                }
            };
            items.push(StringView::new(what).dim(true).wrap(true).into_element());
            say(&mut items, &doing);
            let started = matches!(doing, Doing::Agent { .. });
            let ready = matches!(&logins, Logins::Read(l) if !l.is_empty()) && !busy && !started;
            let claude = ui.s.ai.claude_code.clone();
            let mut row = vec![
                Button::new("Start the Agent").default(true).enabled(ready).on_click(act(ui, move |st, _| st.start_agent(task.clone(), claude.clone()))).into_element(),
                Button::new(if started { "Done" } else { "Cancel" }).on_click(act(ui, |st, cx| st.stop_adding(cx))).into_element(),
            ];
            if !started {
                row.push(Button::new("Set Up by Hand…").id("google-by-hand").enabled(!busy).on_click(act(ui, |st, _| st.start(Adding::ByHand))).into_element());
            }
            items.push(buttons(row).into_element());
        }
        Some(Adding::ByHand) => {
            items.push(StringView::new("In Google Cloud, signed in as the account, make its app, “Olo (its address)”:").dim(true).wrap(true).into_element());
            // (A step a line, indented under what they're for.)
            let steps = setup::steps("Olo (its address)").iter().enumerate().map(|(i, (title, _, _))| StringView::new(format!("{}. {title}", i + 1)).dim(true).into_element()).collect::<Vec<_>>();
            items.push(div().pl(px(m::SPACING)).flex().flex_col().children(steps).into_element());
            items.push(
                StringView::new(
                    "Then its client’s ID and secret here, and sign in: Google asks in your browser whether Olo may read your mail, use your calendars and keep DiskBackup’s files in your Drive.",
                )
                .dim(true)
                .wrap(true)
                .into_element(),
            );
            items.push(
                div()
                    .flex()
                    .child(LinkButton::new("Google Cloud Console…").id("google-console").on_click(|_, _| {
                        let _ = olo_xdg::open::open(&olo_xdg::open::Target::Url(setup::NEW_PROJECT.into()));
                    }))
                    .into_element(),
            );
            let divider = crate::form::divider_for(&["Client ID:", "Client secret:"]);
            items.push(TextControl::new(ui.cx, &st.client_id).label("Client ID:").divider(divider).into_element());
            items.push(TextControl::new(ui.cx, &st.client_secret).label("Client secret:").divider(divider).into_element());
            say(&mut items, &doing);
            let ready = st.typed_client(ui.cx).check().is_ok() && !busy;
            items.push(
                buttons(vec![
                    Button::new("Sign In with Google")
                        .default(true)
                        .enabled(ready)
                        .on_click(act(ui, |st, cx| {
                            let client = st.typed_client(cx);
                            st.sign_in(Some(client), None);
                        }))
                        .into_element(),
                    // (Cancel also lets go of a sign-in waiting for the browser.)
                    Button::new("Cancel").on_click(act(ui, |st, cx| st.stop_adding(cx))).into_element(),
                ])
                .into_element(),
            );
        }
        None => {
            say(&mut items, &doing);
            let mut row = Vec::new();
            if listed.as_ref().is_some_and(|l| !l.is_empty()) {
                let checking = listed.iter().flatten().any(|(_, s)| *s == Stands::Checking);
                row.push(Button::new("Check Again").id("google-check").enabled(!checking && !busy).on_click(act(ui, |st, _| st.check())).into_element());
            }
            row.push(Button::new("Add a Google Account…").id("google-add").enabled(!busy).on_click(act(ui, |st, _| st.start(Adding::Agent(Task::Make)))).into_element());
            // (The first given is rightmost.)
            row.reverse();
            items.push(buttons(row).into_element());
        }
    }
    ui.add(BBox::new("Google").children(items));
}

/// What's under way, or how it went, under the rest.
fn say(items: &mut Vec<Element>, doing: &Doing) {
    match doing {
        Doing::Nothing | Doing::Arrived => {}
        Doing::Busy(what) | Doing::Said(what) | Doing::Agent { said: what, .. } => items.push(StringView::new(what.clone()).wrap(true).into_element()),
        Doing::Failed(why) => items.push(StringView::new(why.clone()).wrap(true).color(olo_kit::style::c(olo_kit::theme::palette().failure)).into_element()),
    }
}

/// `olo-preferences --google-setup-done [CLIENT_FILE] [--account ADDRESS]
/// [--screen NAME]`: what the agent runs when the account's app is
/// made (`olo_google::setup`): Olo signed in with its client (Google's
/// consent page opened on the agent's screen, for it to allow; with no
/// screen, in the person's browser), the account listed, and each of
/// Mail, Calendar and DiskBackup tried with it, said on standard
/// output. `--account`: an account whose app is there already (one
/// connected before, or DiskBackup's own from before), signed in again
/// with that app's client. The exit status says whether it's connected.
pub fn setup_done(args: &[String]) -> i32 {
    match setup_done_with(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("olo-preferences: {e}");
            1
        }
    }
}

fn setup_done_with(args: &[String]) -> Result<(), String> {
    let (mut file, mut account, mut screen) = (None::<String>, None::<String>, None::<String>);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--account" => account = it.next().cloned(),
            // (An empty name is none: the person's browser.)
            "--screen" => screen = it.next().cloned().filter(|s| !s.trim().is_empty()),
            f if !f.starts_with('-') && file.is_none() => file = Some(f.to_owned()),
            other => return Err(format!("{other}? --google-setup-done [CLIENT_FILE] [--account ADDRESS] [--screen NAME]")),
        }
    }
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let client = match (&file, &account) {
        (Some(f), _) => {
            let path = f.strip_prefix("~/").map_or_else(|| std::path::PathBuf::from(f), |rest| home.join(rest));
            olo_google::Client::from_json(&std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?)?
        }
        (None, Some(a)) => accounts::secret(a).map(|s| s.client).or_else(legacy_client).ok_or_else(|| format!("no app is known for {a} here: give its client's file (Download JSON)"))?,
        (None, None) => return Err("give the client's file (Download JSON), or --account ADDRESS for one whose app Olo knows".into()),
    };
    client.check()?;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(20 * 60));
            cancel.store(true, Ordering::Relaxed);
        });
    }
    let signed = accounts::sign_in(&client, account.as_deref(), &cancel, |url| match &screen {
        Some(s) => {
            println!("Google's consent page is open on screen “{s}”: choose the account (sign in if asked), allow everything it asks, then Continue. Olo waits for it.\n\n  {url}\n");
            if !setup::open_on_screen(s, url) {
                println!("(It didn't open there: open the address above on the screen.)");
            }
        }
        None => {
            println!("Sign in to Google in the browser (Olo opened it), or open this:\n\n  {url}\n");
            accounts::open_consent(url);
        }
    })?;
    if let Some(a) = &account
        && !signed.email.eq_ignore_ascii_case(a)
    {
        return Err(format!("that signed in as {}, not {a}: run it again and choose {a}", signed.email));
    }
    let said = connect(&client, &signed)?;
    println!("{said}");
    let address = signed.email.to_lowercase();
    let listed = accounts::accounts();
    let Some(now) = listed.iter().find(|a| a.address == address) else { return Err("it isn't listed: the list couldn't be read back".into()) };
    // Each service tried.
    let mail = olo_mail::Account { provider: olo_mail::Provider::Gmail, address: address.clone() };
    let mail_said = if !now.allowed(Service::Mail) {
        "not allowed (sign in again, and allow it)".to_owned()
    } else {
        match accounts::token(&address, true, "Olo").map_err(|e| e.to_string()).and_then(|t| olo_mail::check(&mail, &olo_mail::Auth::Token(t))) {
            Ok(()) => format!("its inbox opened{}", if now.ticked(Service::Mail) { "" } else { " (not ticked)" }),
            Err(e) => e,
        }
    };
    println!("Mail: {mail_said}.");
    let calendar_said = if !now.allowed(Service::Calendar) {
        "not allowed (sign in again, and allow it)".to_owned()
    } else {
        let cal = olo_calendar::account::Account { provider: olo_calendar::account::Provider::Google, address: address.clone(), server: String::new() };
        let secret = olo_calendar::account::Secret::Google { client: client.clone(), refresh_token: signed.refresh_token.clone() };
        match olo_calendar::account::check(&cal, &secret) {
            Ok(found) => format!("{} calendar{} found{}", found.len(), if found.len() == 1 { "" } else { "s" }, if now.ticked(Service::Calendar) { "" } else { " (not ticked)" }),
            Err(olo_calendar::dav::Error::SignIn) => "Google wouldn't give its calendars: turn the CalDAV API on in the app's project, then run this again".to_owned(),
            Err(e) => e.to_string(),
        }
    };
    println!("Calendar: {calendar_said}.");
    let drive_said = match olo_protocol::diskbackup::Settings::load().target {
        olo_protocol::diskbackup::Target::GoogleDrive { account: backs_up, folder } if backs_up.eq_ignore_ascii_case(&address) => match accounts::sees_drive_file(&address, &folder) {
            Ok(true) => "its backups in Drive are seen by this app: DiskBackup goes on as it was".to_owned(),
            Ok(false) => {
                "this app doesn't see the backups DiskBackup wrote to Drive: they were made by another project's app. Use that project's client (its app brought up to date), not a new one".to_owned()
            }
            Err(e) => e,
        },
        _ if now.allowed(Service::DiskBackup) => "allowed (tick it in Preferences ▸ Integrations ▸ Google to back up to this Drive)".to_owned(),
        _ => "not allowed (sign in again, and allow it)".to_owned(),
    };
    println!("DiskBackup: {drive_said}.");
    println!("Connected {address}: Preferences ▸ Integrations ▸ Google lists it.");
    Ok(())
}
