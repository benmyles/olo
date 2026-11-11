//! Token Pool's window (on olo-kit): the gateway, which clients use it,
//! each account's quota, usage and state, live, and adding accounts.
//! What outlives the windows is a [`Hub`] on the app's looper (the
//! gateway serves with no window open): which clients use the pool, a
//! sign-in being waited for, the last thing said. Each window is a view
//! on a looper of its own, reading the pool itself (`Shared`) and told
//! the rest by the hub ([`Told`]); what it's asked to do that outlives it
//! goes to the hub ([`Msg`]).

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use olo_kit::{
    AppCx, Bounds, Child, ClickEvent, Corner, Cx, Element, Hsla, Menu, MenuBar, ParentElement as _, Pixels, Proxy, ScrollHandle, Styled as _, View, WindowHandle, WindowOptions, div, menu,
    metrics as m, px, style, theme,
    widgets::{self, BBox, Button, CellGrid, CheckBox, GridCell, ScrollView, StatusBar, StringView, TextControl, TextField, TextFieldEvent, heading, separator, text_field},
};
use olo_token_pool::{
    clients,
    codex_daemon::{self, Followed},
    gateway::{Pool, Shared},
    import::{self, Found},
    model::{Account, Outcome, Provider, RECENT, Request, Window as QuotaWindow, now},
    schedule, store,
};

use crate::{
    PROVIDERS, client_index,
    clients_window::{ClientsMsg, ClientsWindow},
    clock, duration, plan_name, span, tokens,
};

/// How long a separate sign-in in Terminal is waited for.
const SIGN_IN_WAIT: Duration = Duration::from_secs(20 * 60);

/// What the hub is told: by worker threads, the gateway, a launch, and
/// the windows.
pub enum Msg {
    /// The pool changed (a request, a refresh, quota read).
    Changed,
    /// Which clients use the pool.
    Clients([bool; 2]),
    /// A client turned on or off: how it went, and whether that pointed
    /// it at the proxy (Some: and restarted Codex's service onto it) or
    /// only paused or resumed it (None: its next request).
    ClientSet(Provider, Result<(), String>, Option<bool>),
    /// A sign-in was read (for this account, when signing in again).
    SignedIn(Result<Found, String>, Option<String>),
    /// Something done in the background: what to say.
    Done(Result<String, String>),
    /// Asked over the gateway to open (`/pool/open`: a launch that found
    /// no copy taking launches).
    Open,
    /// A command from a window's menus ("clients", "import:claude",
    /// "separate:codex").
    Command(String),
    /// A window's check box: this client uses the pool, or doesn't.
    SetClient(Provider, bool),
    /// Sign in again for this account: in Terminal, or (true) with this
    /// computer's own sign-in.
    SignIn(Provider, String, bool),
    /// The sign-in being waited for is given up.
    CancelSignIn,
    /// A window closed by the person (true: Clients).
    Closed(bool),
    /// Every window closes (handed over to a new copy, which opens them).
    CloseWindows,
}

/// A separate sign-in going on in Terminal.
struct Waiting {
    provider: Provider,
    /// The account it's for (signing in again), else a new one.
    target: Option<String>,
    cancel: Arc<AtomicBool>,
}

/// What the hub keeps that the main window shows.
#[derive(Clone, Default, PartialEq)]
pub struct Told {
    /// Which clients use the pool (None: not read yet).
    clients: Option<[bool; 2]>,
    /// The last thing done or gone wrong (error?).
    note: Option<(String, bool)>,
    /// A separate sign-in being waited for: whose client, and the
    /// account it's for.
    waiting: Option<(Provider, Option<String>)>,
}

/// Which windows are open, for the upgrade's thread to read.
#[derive(Default)]
pub struct Open {
    main: AtomicBool,
    clients: AtomicBool,
}

impl Open {
    fn state(&self) -> String {
        serde_json::json!({"open": self.main.load(Ordering::Relaxed), "clients": self.clients.load(Ordering::Relaxed)}).to_string()
    }
}

/// The app's state, kept from start to end on the app's looper.
pub struct Hub {
    pub shared: Arc<Shared>,
    /// The clients' proxy's port (`intercept`).
    proxy_port: u16,
    proxy: Proxy,
    win: Option<WindowHandle<PoolWindow>>,
    /// The Clients window, while it's open.
    clients_win: Option<WindowHandle<ClientsWindow>>,
    clients: Option<[bool; 2]>,
    note: Option<(String, bool)>,
    waiting: Option<Waiting>,
    open: Arc<Open>,
}

impl Hub {
    pub fn new(shared: Arc<Shared>, proxy_port: u16, proxy: Proxy) -> Hub {
        Hub { shared, proxy_port, proxy, win: None, clients_win: None, clients: None, note: None, waiting: None, open: Arc::default() }
    }

    fn told(&self) -> Told {
        Told { clients: self.clients, note: self.note.clone(), waiting: self.waiting.as_ref().map(|w| (w.provider, w.target.clone())) }
    }

    /// The windows shown what the hub has now.
    fn tell(&self) {
        if let Some(w) = &self.win {
            w.messenger().send(WinMsg::Told(self.told()));
        }
        if let Some(w) = &self.clients_win {
            w.messenger().send(ClientsMsg::Uses(self.clients));
        }
    }

    /// Opens the window (or brings it to the front).
    pub fn open(&mut self) {
        let (shared, port, told) = (self.shared.clone(), self.proxy_port, self.told());
        let options = WindowOptions::new("Token Pool", 460.0, 560.0).min_size(380.0, 320.0).keep_size("main");
        olo_kit::open_or_raise(&mut self.win, options, move |cx| PoolWindow::new(shared, port, told, cx));
        self.open.main.store(true, Ordering::Relaxed);
        self.read_clients();
    }

    /// Opens the Clients window (or brings it to the front).
    pub fn open_clients(&mut self) {
        let (shared, port, uses) = (self.shared.clone(), self.proxy_port, self.clients);
        let options = WindowOptions::new("Clients", 480.0, 600.0).min_size(400.0, 420.0);
        olo_kit::open_or_raise(&mut self.clients_win, options, move |cx| ClientsWindow::new(shared, port, uses, cx));
        self.open.clients.store(true, Ordering::Relaxed);
        if self.clients.is_none() {
            self.read_clients();
        }
    }

    /// Reads which clients use the pool (their settings files) off the
    /// app's looper.
    fn read_clients(&self) {
        let (proxy, port) = (self.proxy.clone(), self.proxy_port);
        std::thread::spawn(move || proxy.send(Msg::Clients(PROVIDERS.map(|p| clients::enabled(p, port)))));
    }

    fn set_client(&mut self, p: Provider, on: bool) {
        if let Some(c) = self.clients.as_mut() {
            c[client_index(p)] = on;
        }
        // The proxy routes its requests so from the next on: the client
        // turned off passes through, with nothing restarted or cut.
        self.shared.set_paused(p, !on);
        let (proxy, port) = (self.proxy.clone(), self.proxy_port);
        std::thread::spawn(move || {
            let pointed = clients::pointed(p, port);
            let r = clients::set(p, on, port);
            // Pointed at the proxy only now: Codex's daemon restarted onto
            // it (its windows' requests from then on).
            let restarted = p == Provider::Codex && r.is_ok() && matches!(codex_daemon::follow(), Ok(Followed::Restarted(_)));
            proxy.send(Msg::ClientSet(p, r, if pointed { None } else { Some(restarted) }))
        });
    }

    /// Brings in this computer's sign-in for `p` (read off the app's
    /// looper), for the account `target` or as a new one.
    fn import_local(&mut self, p: Provider, target: Option<String>) {
        let proxy = self.proxy.clone();
        std::thread::spawn(move || proxy.send(Msg::SignedIn(import::this_computer(p), target)));
    }

    /// Signs `p`'s client in to an account in Terminal, apart from this
    /// computer's own sign-in, and takes it when it's done.
    fn sign_in_separately(&mut self, p: Provider, target: Option<String>) {
        if let Some(w) = self.waiting.take() {
            w.cancel.store(true, Ordering::Relaxed);
        }
        let dir = match import::separate_dir() {
            Ok(d) => d,
            Err(e) => return self.note = Some((e, true)),
        };
        let mut argv = vec!["olo-terminal".to_owned(), "-e".to_owned()];
        argv.extend(import::sign_in_command(p, &dir));
        let argv: Vec<std::ffi::OsString> = argv.into_iter().map(Into::into).collect();
        if let Err(e) = olo_xdg::open::spawn(&argv) {
            import::discard(&dir);
            return self.note = Some((format!("Couldn’t open Terminal: {e}"), true));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.waiting = Some(Waiting { provider: p, target: target.clone(), cancel: cancel.clone() });
        self.note = None;
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            let result = loop {
                std::thread::sleep(Duration::from_secs(2));
                if cancel.load(Ordering::Relaxed) {
                    break None;
                }
                if let Ok(found) = import::read_separate(p, &dir) {
                    break Some(Ok(found));
                }
                if start.elapsed() > SIGN_IN_WAIT {
                    break Some(Err("The sign-in in Terminal didn’t finish in time.".to_owned()));
                }
            };
            import::discard(&dir);
            if let Some(r) = result {
                proxy.send(Msg::SignedIn(r, target));
            }
        });
    }

    /// Takes a sign-in that was read: a new account, or a new sign-in for
    /// the account it belongs to.
    fn adopt(&mut self, found: Found, target: Option<String>) {
        let pool = self.shared.snapshot();
        let same = |a: &&Account| a.provider == found.provider && found.email.is_some() && a.email == found.email;
        let existing = target.or_else(|| pool.saved.accounts.iter().find(same).map(|a| a.id.clone()));
        if let Some(id) = existing {
            let name = pool.account(&id).map(|a| a.name.clone()).unwrap_or_default();
            self.shared.reauth(&id, found.credential);
            if let Some(a) = self.shared.pool.lock().unwrap().account_mut(&id) {
                if found.org.is_some() {
                    a.org = found.org;
                }
                if found.email.is_some() {
                    a.email = found.email;
                }
                if found.plan.is_some() {
                    a.plan = found.plan;
                }
            }
            self.note = Some((format!("{name} is signed in again."), false));
            return;
        }
        let mut a = Account::new(store::new_id(), found.name(), found.provider);
        a.email = found.email.clone();
        a.plan = found.plan.clone();
        a.org = found.org.clone();
        a.separate = self.waiting.is_some();
        let name = a.name.clone();
        self.note = Some(match self.shared.add(a, found.credential) {
            Ok(()) => (format!("{name} joined the pool."), false),
            Err(e) => (e, true),
        });
    }

    fn command(&mut self, command: &str) {
        if command == "clients" {
            return self.open_clients();
        }
        let provider = command.rsplit(':').next().and_then(Provider::parse);
        match (command.split(':').next(), provider) {
            (Some("import"), Some(p)) => self.import_local(p, None),
            (Some("separate"), Some(p)) => self.sign_in_separately(p, None),
            _ => {}
        }
    }

    /// A message from a worker thread, the gateway, a launch or a window.
    pub fn message(&mut self, message: Box<dyn std::any::Any + Send>) {
        if let Some(launch) = message.downcast_ref::<olo_kit::instance::Launch>() {
            if !launch.args.iter().any(|a| a == "--service") {
                self.open();
            }
            return;
        }
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Changed => {
                // (Each window reads the pool itself.)
                if let Some(w) = &self.win {
                    w.messenger().send(WinMsg::Changed);
                }
                if let Some(w) = &self.clients_win {
                    w.messenger().send(ClientsMsg::Changed);
                }
                return;
            }
            Msg::Open => {
                // (And this copy takes launches from now on, if none does.)
                olo_kit::instance::listen(crate::APP_ID, self.proxy.clone());
                self.open();
                return;
            }
            Msg::Clients(on) => self.clients = Some(on),
            Msg::ClientSet(prov, r, restarted) => {
                self.note = Some(match r {
                    Ok(()) => {
                        let on = self.clients.is_some_and(|c| c[client_index(prov)]);
                        // Codex's sessions run in its daemon, restarted onto
                        // the settings.
                        let when = match (prov, restarted) {
                            (_, None) => "from its next request",
                            (Provider::Codex, Some(true)) => "from now (its service restarted)",
                            (Provider::Codex, Some(false)) => "from now",
                            (Provider::Claude, Some(_)) => "from its next start",
                        };
                        let verb = if on { "uses the pool" } else { "is back to its own sign-in" };
                        (format!("{} {verb} {when}.", prov.client()), false)
                    }
                    Err(e) => (e, true),
                });
                self.read_clients();
            }
            Msg::Done(r) => {
                self.note = Some(match r {
                    Ok(m) => (m, false),
                    Err(e) => (e, true),
                })
            }
            Msg::SignedIn(r, target) => {
                match r {
                    Ok(found) => self.adopt(found, target),
                    Err(e) => self.note = Some((e, true)),
                }
                self.waiting = None;
            }
            Msg::Command(command) => self.command(&command),
            Msg::SetClient(p, on) => self.set_client(p, on),
            Msg::SignIn(p, id, local) => {
                if local {
                    self.import_local(p, Some(id));
                } else {
                    self.sign_in_separately(p, Some(id));
                }
            }
            Msg::CancelSignIn => {
                if let Some(w) = self.waiting.take() {
                    w.cancel.store(true, Ordering::Relaxed);
                }
            }
            // A window closed by the person: what it showed goes (the
            // gateway keeps serving).
            Msg::Closed(clients) => {
                if clients {
                    self.clients_win = None;
                    self.open.clients.store(false, Ordering::Relaxed);
                } else {
                    self.win = None;
                    self.open.main.store(false, Ordering::Relaxed);
                }
                return;
            }
            Msg::CloseWindows => {
                if let Some(w) = self.win.take() {
                    w.close();
                }
                if let Some(w) = self.clients_win.take() {
                    w.close();
                }
                return;
            }
        }
        self.tell();
    }
}

fn add_menu() -> Menu {
    let mut menu = Menu::new("Accounts");
    for (i, p) in PROVIDERS.into_iter().enumerate() {
        if i > 0 {
            menu = menu.separator();
        }
        let key = format!("{p:?}").to_lowercase();
        menu = menu.item(format!("Add This Computer’s {} Sign-In", p.client()), format!("import:{key}")).item(format!("Sign In to {} Separately…", p.client()), format!("separate:{key}"));
    }
    menu
}

/// The app's menu bar, on each of its windows (what's chosen goes to the
/// window's view: [`command`]).
pub fn menu_bar<V: View>(cx: &mut Cx<V>) -> MenuBar {
    let menus = vec![Menu::new("Pool").item("Read Quota Now", "refresh").key("r").item("Clients", "clients").key("l").separator().item("Close", "close").key("w"), add_menu()];
    MenuBar::new(cx, menus)
}

/// A command from the menu bar, in either window: Close closes the one
/// it's in, the rest is the app's.
pub fn command<V: View>(shared: &Arc<Shared>, command: &str, cx: &mut Cx<V>) -> bool {
    match command {
        "close" => cx.close_window(),
        "refresh" => shared.sync_soon(None),
        _ => olo_kit::proxy().send(Msg::Command(command.to_owned())),
    }
    true
}

/// How a provider's requests are marked in the heatmap.
fn mark(p: Provider) -> &'static str {
    match p {
        Provider::Claude => "C",
        Provider::Codex => "X",
    }
}

/// A finished request's colour: BeOS's green, yellow and red, grey for
/// one the client gave up on.
fn outcome_color(p: &olo_kit::palette::Palette, o: Outcome) -> Hsla {
    match o {
        Outcome::Completed => style::c(p.success),
        Outcome::RateLimited => style::c(p.tab_focused),
        Outcome::Failed => style::c(p.failure),
        Outcome::Cancelled => style::disabled_text(p),
    }
}

fn done_details(account: &str, r: &Request) -> String {
    let mut s = format!("{} · {} on {} · {} · {} · {}", r.outcome.label(), r.model, account, r.client, clock(r.at), duration(r.ms));
    if r.status != 0 && r.outcome != Outcome::Completed {
        s.push_str(&format!(" · HTTP {}", r.status));
    }
    if r.outcome == Outcome::Completed || r.usage.input_tokens + r.usage.output_tokens > 0 {
        s.push_str(&format!(" · {} in, {} out", tokens(r.usage.input_tokens), tokens(r.usage.output_tokens)));
    }
    s
}

/// One quota window: how much is used, and when it resets.
fn window_bar(p: &olo_kit::palette::Palette, w: &QuotaWindow, t: i64) -> StatusBar {
    let mut trailing = format!("{:.0}% used", w.used.clamp(0.0, 999.0));
    if let Some(r) = w.resets_at.filter(|&r| r > t) {
        trailing.push_str(&format!(" · resets {} ({})", clock(r), span(r - t)));
    } else if w.used <= 0.0 {
        // (Nothing used in it yet: it starts with the next request.)
        trailing.push_str(" · not started");
    }
    let label = if w.label.is_empty() { plan_name(w.name.trim_end_matches("_window")) } else { w.label.clone() };
    let mut bar = StatusBar::new((w.used / 100.0) as f32).label(label).trailing(trailing).bar_height(10.0);
    if w.exhausted(t) {
        bar = bar.color(style::c(p.failure));
    } else if w.used >= 80.0 {
        bar = bar.color(style::c(p.keyboard_navigation));
    }
    bar
}

/// A row of buttons at the right, one control tall.
fn row() -> Element {
    widgets::button_row().w_full().h(px(m::CONTROL_HEIGHT))
}

/// A request picked in the heatmap.
#[derive(Clone, PartialEq)]
pub enum Pick {
    /// Going on now (its id).
    Open(u64),
    /// One of an account's finished requests.
    Done(String, Request),
}

/// What Token Pool's window is told.
#[derive(Clone)]
pub enum WinMsg {
    /// The pool changed.
    Changed,
    /// What the hub keeps, now.
    Told(Told),
    /// Times count down (running requests' by the second).
    Tick,
    Pick(Pick),
    /// The Add Account button's menu, under it.
    AddMenu(Bounds<Pixels>),
    /// An account's Account button's menu, under it.
    AccountMenu(String, Bounds<Pixels>),
    /// What was chosen from an account's menu.
    Account(String, String),
    /// The rename ends: with the name typed, or as it was.
    Renamed(bool),
    /// Its removal is confirmed, or not.
    Remove(Option<String>),
    /// A reset credit is spent for it, or not.
    Reset(Option<String>),
}

/// Token Pool's window.
pub struct PoolWindow {
    shared: Arc<Shared>,
    proxy_port: u16,
    told: Told,
    scroll: ScrollHandle,
    /// The account being renamed, and its name being typed.
    renaming: Option<(String, Child<TextField>)>,
    /// The account whose removal is being confirmed.
    removing: Option<String>,
    /// The account a reset credit is being spent for (confirming).
    resetting: Option<String>,
    /// The request whose details are shown.
    picked: Option<Pick>,
}

impl PoolWindow {
    fn new(shared: Arc<Shared>, proxy_port: u16, told: Told, cx: &mut Cx<Self>) -> PoolWindow {
        let w = PoolWindow { shared, proxy_port, told, scroll: ScrollHandle::new(), renaming: None, removing: None, resetting: None, picked: None };
        w.tick(cx);
        w
    }

    /// Looks again in a second while requests run, else in twenty.
    fn tick(&self, cx: &mut Cx<Self>) {
        let busy = !self.shared.snapshot().open.is_empty();
        cx.after(Duration::from_secs(if busy { 1 } else { 20 }), WinMsg::Tick);
    }

    fn account_command(&mut self, id: &str, command: &str, cx: &mut Cx<Self>) {
        let Some(a) = self.shared.snapshot().account(id).cloned() else { return };
        match command {
            "rename" => {
                let field = text_field(cx, |f| f.name("Name").with_text(a.name.clone()));
                field.update(cx, |f, cx| f.caret_to_end(cx));
                field.focus(cx);
                // Return renames.
                cx.subscribe(&field, |_, e: &TextFieldEvent, cx| {
                    if matches!(e, TextFieldEvent::Entered) {
                        cx.send(WinMsg::Renamed(true));
                    }
                });
                self.renaming = Some((a.id, field));
            }
            "refresh" => self.shared.sync_soon(Some(a.id)),
            "sign-in" => olo_kit::proxy().send(Msg::SignIn(a.provider, a.id, false)),
            "sign-in-local" => olo_kit::proxy().send(Msg::SignIn(a.provider, a.id, true)),
            "remove" => self.removing = Some(a.id),
            "reset" => self.resetting = Some(a.id),
            _ => {}
        }
    }

    /// What the gateway is doing, the requests going on now, and which
    /// clients use the pool.
    fn header(&self, pool: &Pool, p: &olo_kit::palette::Palette, cx: &mut Cx<Self>) -> Element {
        let busy: u32 = pool.in_flight.values().sum();
        let first = match &pool.error {
            Some(e) => StringView::new(e.clone()).wrap(true).color(style::c(p.failure)),
            None => StringView::new(match busy {
                0 => format!("Listening on 127.0.0.1:{}", self.proxy_port),
                n => format!("Listening on 127.0.0.1:{} · {n} request{} now", self.proxy_port, if n == 1 { "" } else { "s" }),
            }),
        };
        // The requests going through now: a cell each, lit while it runs.
        let t = crate::now_ms();
        let cells: Vec<GridCell> = pool
            .open
            .iter()
            .map(|o| GridCell {
                color: Some(style::c(p.status_bar)),
                mark: Some(mark(o.provider).into()),
                name: format!("{} on {}, {}", o.model, o.account, duration((t - o.started).max(0) as u64)).into(),
                selected: self.picked == Some(Pick::Open(o.id)),
            })
            .collect();
        let ids: Vec<u64> = pool.open.iter().map(|o| o.id).collect();
        let grid = CellGrid::new(cells).rows(2).cell_size(14.0).fill().on_click(cx.msg_with(move |i: &usize| WinMsg::Pick(Pick::Open(ids[*i]))));
        // One line, always there (so nothing jumps): the picked request.
        let details = match &self.picked {
            Some(Pick::Open(id)) => {
                pool.open.iter().find(|o| o.id == *id).map(|o| format!("Running · {} on {} · {} · {} so far", o.model, o.account, o.client, duration((t - o.started).max(0) as u64)))
            }
            Some(Pick::Done(id, r)) => Some(done_details(&pool.account(id).map(|x| x.name.clone()).unwrap_or_default(), r)),
            None => None,
        };
        let text = details.unwrap_or_else(|| match pool.open.len() {
            0 => "No requests now · click a cell for its details".into(),
            _ => "Click a cell for its details".into(),
        });
        // (A row a control tall.)
        let mut checks = div().flex().items_center().h(px(m::CONTROL_HEIGHT)).gap(px(m::SPACING * 2.0));
        for prov in PROVIDERS {
            let on = self.told.clients.is_some_and(|c| c[client_index(prov)]);
            checks = checks.child(
                CheckBox::new(format!("{} uses the pool", prov.client()), on).enabled(self.told.clients.is_some()).on_toggle(move |on: &bool, _| olo_kit::proxy().send(Msg::SetClient(prov, *on))),
            );
        }
        div().flex().flex_col().gap(px(m::SPACING)).child(first).child(grid).child(StringView::new(text).dim(true)).child(checks)
    }

    /// An account's card: what it is, its quota, today's use, its latest
    /// requests, and what can be done with it.
    fn card(&self, pool: &Pool, account: &Account, p: &olo_kit::palette::Palette, cx: &mut Cx<Self>) -> BBox {
        let t = now();
        let title = if account.signed_out.is_some() {
            format!("{} — Signed Out", account.name)
        } else if !account.enabled {
            format!("{} — Paused", account.name)
        } else {
            account.name.clone()
        };
        let mut b = BBox::new(title);
        let mut facts: Vec<String> = Vec::new();
        if let Some(e) = &account.email {
            facts.push(e.clone());
        }
        if let Some(plan) = &account.plan {
            facts.push(format!("{} plan", plan_name(plan)));
        }
        let credits: u32 = account.credits.iter().map(|c| c.remaining).sum();
        if credits > 0 {
            facts.push(format!("{credits} reset credit{}", if credits == 1 { "" } else { "s" }));
        }
        if pool.busy(&account.id) {
            facts.push("answering now".into());
        } else if let Some(l) = account.last_used {
            facts.push(format!("used {} ago", span(t - l)));
        }
        if !facts.is_empty() {
            b = b.child(StringView::new(facts.join(" · ")).dim(true));
        }
        if let Some(why) = &account.signed_out {
            b = b.child(StringView::new(format!("Needs signing in again: {why}")).wrap(true).color(style::c(p.failure)));
        } else if let Some(until) = schedule::blocked_until(account, "", t) {
            b = b.child(StringView::new(format!("Out of quota until {} (in {})", clock(until), span(until - t))));
        }
        for w in &account.windows {
            b = b.child(window_bar(p, w, t));
        }
        if account.windows.is_empty() && account.signed_out.is_none() {
            b = b.child(StringView::new(if account.synced.is_some() { "No quota reported." } else { "Reading its quota…" }).dim(true));
        }
        let today = if account.today.0 == olo_token_pool::model::today() { account.today.1.clone() } else { Default::default() };
        b = b.child(StringView::new(format!(
            "Today: {} request{} · {} in · {} out",
            today.requests,
            if today.requests == 1 { "" } else { "s" },
            tokens(today.input_tokens),
            tokens(today.output_tokens)
        )));
        // Its latest requests, oldest first, coloured by how they ended.
        let cells: Vec<GridCell> = account
            .recent
            .iter()
            .map(|r| GridCell {
                color: Some(outcome_color(p, r.outcome)),
                mark: None,
                name: format!("{}: {} at {}", r.outcome.label(), r.model, clock(r.at)).into(),
                selected: self.picked == Some(Pick::Done(account.id.clone(), r.clone())),
            })
            .collect();
        let (id, recent) = (account.id.clone(), account.recent.clone());
        b = b.child(CellGrid::new(cells).cell_size(12.0).slots(RECENT).on_click(cx.msg_with(move |i: &usize| WinMsg::Pick(Pick::Done(id.clone(), recent[*i].clone())))));
        b.child(self.card_controls(account, cx))
    }

    fn card_controls(&self, account: &Account, cx: &mut Cx<Self>) -> Element {
        if let Some((_, field)) = self.renaming.as_ref().filter(|(id, _)| *id == account.id) {
            return div().flex().items_center().gap(px(m::SPACING)).child(TextControl::new(cx, field).width(180.0)).child(
                widgets::button_row().flex_1().child(Button::new("Cancel").on_click(cx.msg(WinMsg::Renamed(false)))).child(Button::new("Rename").default(true).on_click(cx.msg(WinMsg::Renamed(true)))),
            );
        }
        if self.resetting.as_deref() == Some(account.id.as_str()) {
            let when = account.credit().and_then(|c| c.expires_at).map(|e| format!(" (it expires {})", clock(e))).unwrap_or_default();
            return div()
                .flex()
                .flex_col()
                .gap(px(m::SPACING))
                .child(StringView::new(format!("Use one of its reset credits now{when}? Its quota starts again, and the credit is gone.")).wrap(true))
                .child(row().child(Button::new("Cancel").on_click(cx.msg(WinMsg::Reset(None)))).child(Button::new("Use Credit").on_click(cx.msg(WinMsg::Reset(Some(account.id.clone()))))));
        }
        if self.removing.as_deref() == Some(account.id.as_str()) {
            return div()
                .flex()
                .flex_col()
                .gap(px(m::SPACING))
                .child(StringView::new("Remove it from the pool? Its sign-in here is deleted; the account itself isn’t touched.").wrap(true))
                .child(row().child(Button::new("Cancel").on_click(cx.msg(WinMsg::Remove(None)))).child(Button::new("Remove").on_click(cx.msg(WinMsg::Remove(Some(account.id.clone()))))));
        }
        let (shared, id) = (self.shared.clone(), account.id.clone());
        // (The pool says it changed: the window shows it then.)
        let use_it = CheckBox::new("Use this account", account.enabled).id(format!("use-{}", account.id)).on_toggle(move |on: &bool, _| shared.set_enabled(&id, *on));
        let id = account.id.clone();
        let account_menu = Button::new("Account").id(format!("account-{}", account.id)).min_width(m::BUTTON_MIN_WIDTH).on_click(cx.listener(move |_, _: &ClickEvent, cx| {
            let at = cx.listener_bounds();
            cx.send(WinMsg::AccountMenu(id.clone(), at));
        }));
        div().flex().items_center().child(use_it).child(widgets::button_row().flex_1().child(account_menu))
    }
}

impl View for PoolWindow {
    type Message = WinMsg;

    fn message(&mut self, message: WinMsg, cx: &mut Cx<Self>) {
        match message {
            WinMsg::Changed => {}
            WinMsg::Told(told) => {
                if told == self.told {
                    return;
                }
                self.told = told;
            }
            WinMsg::Tick => self.tick(cx),
            WinMsg::Pick(pick) => self.picked = if self.picked.as_ref() == Some(&pick) { None } else { Some(pick) },
            WinMsg::AddMenu(at) => {
                let chosen = cx.msg_with(|command: &str| WinMsg::Account(String::new(), command.to_owned()));
                menu::popup(cx, add_menu(), at, Corner::BottomLeft, chosen);
                return;
            }
            WinMsg::AccountMenu(id, at) => {
                let Some(a) = self.shared.snapshot().account(&id).cloned() else { return };
                let menu = Menu::new("Account")
                    .item("Rename…", "rename")
                    .item("Read Quota Now", "refresh")
                    .item("Use a Reset Credit…", "reset")
                    .enabled(a.credit().is_some())
                    .separator()
                    .item("Sign In Again in Terminal…", "sign-in")
                    .item(format!("Use This Computer’s {} Sign-In", a.provider.client()), "sign-in-local")
                    .separator()
                    .item("Remove from Pool…", "remove");
                let chosen = cx.msg_with(move |command: &str| WinMsg::Account(id.clone(), command.to_owned()));
                menu::popup(cx, menu, at, Corner::BottomLeft, chosen);
                return;
            }
            // (Add Account's menu holds the app's commands.)
            WinMsg::Account(id, command) if id.is_empty() => olo_kit::proxy().send(Msg::Command(command)),
            WinMsg::Account(id, command) => self.account_command(&id, &command, cx),
            WinMsg::Renamed(rename) => {
                if let Some((id, field)) = self.renaming.take()
                    && rename
                {
                    let name = field.read(cx).text().trim().to_owned();
                    if !name.is_empty() {
                        self.shared.rename(&id, &name);
                    }
                }
            }
            WinMsg::Remove(id) => {
                if let Some(id) = id {
                    let name = self.shared.snapshot().account(&id).map(|a| a.name.clone()).unwrap_or_default();
                    olo_kit::proxy().send(Msg::Done(self.shared.remove(&id).map(|()| format!("{name} left the pool."))));
                }
                self.removing = None;
            }
            WinMsg::Reset(id) => {
                if let Some(id) = id {
                    let name = self.shared.snapshot().account(&id).map(|a| a.name.clone()).unwrap_or_default();
                    let (shared, proxy) = (self.shared.clone(), olo_kit::proxy());
                    self.shared.spawn(async move {
                        let r = shared.redeem(&id).await.map(|()| format!("{name}’s quota was reset."));
                        proxy.send(Msg::Done(r));
                    });
                }
                self.resetting = None;
            }
        }
        cx.invalidate();
    }

    fn command(&mut self, name: &str, cx: &mut Cx<Self>) -> bool {
        command(&self.shared, name, cx)
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        olo_kit::proxy().send(Msg::Closed(false));
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let p = &*p;
        let pool = self.shared.snapshot();
        let header = self.header(&pool, p, cx);
        let mut cards = div().flex().flex_col().gap(px(m::SPACING)).pb(px(m::SPACING));
        let mut any = false;
        for prov in PROVIDERS {
            let accounts: Vec<&Account> = pool.saved.accounts.iter().filter(|x| x.provider == prov).collect();
            if accounts.is_empty() {
                continue;
            }
            any = true;
            cards = cards.child(div().pb(px(m::TIGHT)).child(heading(prov.client())));
            for account in accounts {
                cards = cards.child(self.card(&pool, account, p, cx));
            }
        }
        if !any {
            let text = if pool.loaded {
                "No accounts yet. Add your subscriptions with Add Account: this computer’s own sign-in, or others signed in separately in Terminal. Then check the clients that should use the pool."
            } else {
                "Opening the pool’s accounts…"
            };
            cards = cards.child(StringView::new(text).wrap(true).dim(true));
        }
        // The bottom: what's happening, and adding accounts.
        let status = match (&self.told.waiting, &self.told.note) {
            (Some((provider, target)), _) => {
                let whose = target.as_deref().and_then(|id| pool.account(id)).map(|x| format!("{}’s", x.name)).unwrap_or_else(|| format!("the {}", provider.client()));
                Some((format!("Waiting for {whose} sign-in in Terminal…"), false))
            }
            (None, Some(n)) => Some(n.clone()),
            (None, None) => pool.served.back().map(|s| {
                let (r, u) = (&s.request, &s.request.usage);
                (format!("Last: {} via {} at {} · {} in, {} out", r.model, s.account, clock(r.at), tokens(u.input_tokens), tokens(u.output_tokens)), false)
            }),
        };
        let mut bottom = div().flex().items_center().gap(px(m::SPACING + 2.0)).h(px(m::CONTROL_HEIGHT));
        let mut note = div().flex_1().overflow_hidden();
        if let Some((text, error)) = status {
            let color = if error { style::c(p.failure) } else { style::disabled_text(p) };
            note = note.child(StringView::new(text).color(color));
        }
        bottom = bottom.child(note);
        if self.told.waiting.is_some() {
            bottom = bottom.child(Button::new("Cancel Sign-In").on_click(|_: &ClickEvent, _| olo_kit::proxy().send(Msg::CancelSignIn)));
        }
        bottom = bottom.child(Button::new("Add Account").on_click(cx.listener(|_, _: &ClickEvent, cx| {
            let at = cx.listener_bounds();
            cx.send(WinMsg::AddMenu(at));
        })));
        bottom = bottom.child(Button::new("Clients").on_click(|_: &ClickEvent, _| olo_kit::proxy().send(Msg::Command("clients".into()))));

        let bar = menu_bar(cx);
        div().size_full().flex().flex_col().child(bar).child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .gap(px(m::SPACING))
                .p(px(m::WINDOW_INSET))
                .child(header)
                .child(separator())
                .child(div().flex_1().min_h_0().child(ScrollView::new(&self.scroll).border(false).child(cards)))
                .child(separator())
                .child(bottom),
        )
    }
}

/// The app's state, kept by the app from start to end.
pub fn start(app: &mut AppCx, hub: Hub, service: bool) {
    let restored: Option<serde_json::Value> = olo_kit::restored_state().and_then(|s| serde_json::from_str(&s).ok());
    let open = match &restored {
        Some(r) => r["open"] == true,
        None => !service,
    };
    let clients = restored.as_ref().is_some_and(|r| r["clients"] == true);
    let (shared, flags, proxy) = (hub.shared.clone(), hub.open.clone(), hub.proxy.clone());
    let hub = Rc::new(RefCell::new(hub));
    if open {
        hub.borrow_mut().open();
    }
    if clients {
        hub.borrow_mut().open_clients();
    }
    olo_kit::message_handler(app, move |message, _| hub.borrow_mut().message(message));
    let saved = flags.clone();
    olo_kit::on_upgrade(move |_| Some(saved.state()));
    // A new olo-pool installed: a copy of the new version is started with
    // the listening sockets and takes every new connection at once, while
    // this one finishes what it has under way and then goes (`hand_over`).
    // If no copy can be started, the old way: take no more connections
    // (they wait for it), let what is under way finish (`drain`) and
    // become the new version with the sockets.
    let due = shared.clone();
    olo_kit::on_upgrade_due(move || {
        if hand_over(&due, &flags, &proxy) {
            return;
        }
        due.drain.begin();
        // Look again now and then (a tunnel falls idle with nothing to
        // say so).
        let weak = Arc::downgrade(&due);
        let _ = std::thread::Builder::new().name("pool-drain".into()).spawn(move || {
            while let Some(s) = weak.upgrade() {
                if !s.drain.draining() {
                    break;
                }
                // Long past when the new version should have started: it
                // won't (the kit carried on); take connections again.
                if s.drain.overdue() {
                    s.drain.end();
                    break;
                }
                s.nudge();
                drop(s);
                std::thread::sleep(Duration::from_millis(500));
            }
        });
    });
    // Requests or tunnels under way: the upgrade waits. Once not, the
    // listening sockets are readied for the new version. (Handed over to a
    // copy: always, as this one goes by itself once it has finished.)
    olo_kit::upgrade_waits_while(move || {
        if shared.drain.retired() {
            return true;
        }
        let b = shared.drain.busy();
        if !b && shared.drain.draining() {
            shared.drain.hand_over();
        }
        b
    });
}

/// The environment variable saying a copy runs under the keeper (the copy
/// olo-session started, which stays to wait for it: `main`'s `keeper`).
pub const KEPT_ENV: &str = "OLO_POOL_KEPT";
/// The environment variable that makes `main` the keeper.
pub const KEEPER_ENV: &str = "OLO_POOL_KEEPER";

/// The new version installed: started as a copy of its own, given the
/// listening sockets (the kernel hands each new connection to whichever
/// accepts, and this one stops), the windows (closed here, opened there
/// from the state handed over) and the app's launches; this one finishes
/// the requests it has under way, streams too, saving nothing but a
/// sign-in it had to renew, then goes: the copy olo-session started
/// becomes the keeper, a later one exits (its successor goes to the
/// keeper). Nobody waits and nothing is cut short. False if no copy could
/// be started (the old way, then).
fn hand_over(shared: &Arc<Shared>, open: &Open, proxy: &Proxy) -> bool {
    use std::os::unix::process::CommandExt as _;
    let state = open.state();
    let Some(listeners) = shared.drain.for_a_copy() else { return false };
    let Ok(exe) = std::env::current_exe() else { return false };
    let exe = std::path::PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(std::env::args_os().skip(1))
        .env(olo_token_pool::drain::LISTENERS_ENV, listeners)
        .env(olo_token_pool::drain::PREDECESSOR_ENV, std::process::id().to_string())
        .env(olo_xdg::NEW_INSTANCE, "1")
        .env(KEPT_ENV, "1");
    olo_kit::restore::hand_over(&mut cmd, Some(&state));
    // Launches go to the copy from now on (it listens as it starts).
    olo_kit::instance::close();
    let started = cmd.spawn();
    shared.drain.sealed();
    let child = match started {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("pool: couldn’t start the new version beside this one ({e}); upgrading the old way");
            olo_kit::instance::listen(crate::APP_ID, proxy.clone());
            return false;
        }
    };
    tracing::info!(pid = child.id(), "pool: the new version takes every new request; finishing what this one has");
    shared.drain.retire();
    proxy.send(Msg::CloseWindows);
    let weak = Arc::downgrade(shared);
    let kept = std::env::var_os(KEPT_ENV).is_some();
    let _ = std::thread::Builder::new().name("pool-handed-over".into()).spawn(move || {
        // (The kit says this one waits, `mark_waiting`, as it next asks
        // whether it's busy: after that, so what's said is unsaid last.)
        std::thread::sleep(Duration::from_millis(500));
        loop {
            match weak.upgrade() {
                Some(s) if s.drain.busy() => {
                    drop(s);
                    std::thread::sleep(Duration::from_millis(250));
                }
                _ => break,
            }
        }
        olo_kit::restore::mark_waiting(false);
        if kept {
            tracing::info!("pool: finished; the new version has it all");
            olo_kit::restore::mark_waiting(false);
            std::process::exit(0);
        }
        // The copy olo-session started: its pid stays, as the keeper.
        tracing::info!("pool: finished; staying as the keeper of the new version");
        let e = std::process::Command::new(&exe).env(KEEPER_ENV, "1").env_remove(KEPT_ENV).exec();
        tracing::warn!("pool: couldn’t become the keeper ({e}); ending");
        std::process::exit(0);
    });
    drop(child);
    true
}
