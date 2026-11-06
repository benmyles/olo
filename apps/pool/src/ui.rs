//! Token Pool's window (on olo-ui): the gateway, which clients use it,
//! each account's quota, usage and state, live, and adding accounts.
//! The app's state is a [`PoolApp`] entity that lives while the app does
//! (the gateway serves with no window open); the window draws from it.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use olo_token_pool::{
    clients,
    codex_daemon::{self, Followed},
    gateway::{Pool, Shared},
    import::{self, Found},
    model::{Account, Outcome, Provider, RECENT, Request, Window as QuotaWindow, now},
    running::Running,
    schedule, store,
};
use olo_ui::{
    Frame, Menu, MenuBar, Proxy, WindowOptions,
    gpui::{self, App, AppContext as _, Context, Corner, Entity, Hsla, IntoElement, ParentElement as _, Render, ScrollHandle, Styled as _, Subscription, Task, Window, WindowHandle, div, px},
    menu, metrics as m, style, theme,
    widgets::{self, BBox, Button, CellGrid, CheckBox, GridCell, ScrollView, StatusBar, StringView, TextControl, TextField, TextFieldEvent, heading, separator},
};

use crate::{PROVIDERS, client_index, clients_window::ClientsWindow, clock, duration, plan_name, span, tokens};

/// How long a separate sign-in in Terminal is waited for.
const SIGN_IN_WAIT: Duration = Duration::from_secs(20 * 60);

pub enum Msg {
    /// The pool changed (a request, a refresh, quota read).
    Changed,
    /// Which clients use the pool.
    Clients([bool; 2]),
    /// A client turned on or off: how it went, and whether that pointed
    /// it at the proxy (Some: and restarted Codex's service onto it) or
    /// only paused or resumed it (None: its next request).
    ClientSet(Provider, Result<(), String>, Option<bool>),
    /// What's running, for the Clients window.
    Running(Box<Running>),
    /// Codex's service restarted from the Clients window (its new pid).
    Restarted(Result<i32, String>),
    /// A sign-in was read (for this account, when signing in again).
    SignedIn(Result<Found, String>, Option<String>),
    /// Something done in the background: what to say.
    Done(Result<String, String>),
}

/// A separate sign-in going on in Terminal.
struct Waiting {
    provider: Provider,
    /// The account it's for (signing in again), else a new one.
    target: Option<String>,
    cancel: Arc<AtomicBool>,
}

/// A request picked in the heatmap.
#[derive(Clone, PartialEq)]
enum Pick {
    /// Going on now (its id).
    Open(u64),
    /// One of an account's finished requests.
    Done(String, Request),
}

pub struct PoolApp {
    pub shared: Arc<Shared>,
    /// The clients' proxy's port (`intercept`).
    proxy_port: u16,
    proxy: Proxy,
    win: Option<WindowHandle<Frame>>,
    /// The Clients window, while it's open.
    clients_win: Option<(WindowHandle<Frame>, Entity<ClientsWindow>)>,
    /// Which clients use the pool (None: not read yet).
    clients: Option<[bool; 2]>,
    /// The last thing done or gone wrong (error?).
    note: Option<(String, bool)>,
    waiting: Option<Waiting>,
    /// The account being renamed, and its name being typed.
    renaming: Option<(String, Entity<TextField>, Subscription)>,
    /// The account whose removal is being confirmed.
    removing: Option<String>,
    /// The account a reset credit is being spent for (confirming).
    resetting: Option<String>,
    /// The request whose details are shown.
    picked: Option<Pick>,
}

impl PoolApp {
    pub fn new(shared: Arc<Shared>, proxy_port: u16, proxy: Proxy) -> PoolApp {
        PoolApp { shared, proxy_port, proxy, win: None, clients_win: None, clients: None, note: None, waiting: None, renaming: None, removing: None, resetting: None, picked: None }
    }

    pub fn is_open(&self) -> bool {
        self.win.is_some()
    }

    pub fn clients_open(&self) -> bool {
        self.clients_win.is_some()
    }

    /// Opens the window (or brings it to the front).
    pub fn open(&mut self, cx: &mut Context<Self>) {
        let app = cx.entity();
        let options = WindowOptions::new("Token Pool", 460.0, 560.0).min_size(380.0, 320.0).keep_size("main");
        olo_ui::open_or_raise(self.win, cx, options, move |_, cx| cx.new(|cx| PoolWindow::new(app, cx)), |a, w| a.win = w);
        self.read_clients();
    }

    /// Opens the Clients window (or brings it to the front).
    pub fn open_clients(&mut self, cx: &mut Context<Self>) {
        if let Some((w, _)) = &self.clients_win
            && olo_ui::raise(Some(*w), cx)
        {
            return;
        }
        let app = cx.entity();
        let (proxy, port, shared) = (self.proxy.clone(), self.proxy_port, self.shared.clone());
        cx.defer(move |cx| {
            let view = cx.new(|cx| ClientsWindow::new(app.clone(), shared, port, move |r| proxy.send(Msg::Running(Box::new(r))), cx));
            let shown = view.clone();
            let options = WindowOptions::new("Clients", 480.0, 600.0).min_size(400.0, 420.0);
            let handle = olo_ui::open_window(cx, options, move |_, _| shown);
            app.update(cx, |a, _| a.clients_win = handle.map(|h| (h, view)));
        });
        if self.clients.is_none() {
            self.read_clients();
        }
    }

    /// Reads which clients use the pool (their settings files) off the
    /// UI thread.
    fn read_clients(&self) {
        let (proxy, port) = (self.proxy.clone(), self.proxy_port);
        std::thread::spawn(move || proxy.send(Msg::Clients(PROVIDERS.map(|p| clients::enabled(p, port)))));
    }

    fn set_client(&mut self, p: Provider, on: bool, cx: &mut Context<Self>) {
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
        cx.notify();
    }

    /// Brings in this computer's sign-in for `p` (read off the UI thread).
    fn import_local(&mut self, p: Provider) {
        let proxy = self.proxy.clone();
        std::thread::spawn(move || proxy.send(Msg::SignedIn(import::this_computer(p), None)));
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

    fn command(&mut self, command: &str, cx: &mut Context<Self>) {
        match command {
            "refresh" => self.shared.sync_soon(None),
            "clients" => self.open_clients(cx),
            "close" => {
                if let Some(w) = self.win.take() {
                    cx.close_window(w);
                }
            }
            _ => self.add_command(command),
        }
        cx.notify();
    }

    fn add_command(&mut self, cmd: &str) {
        let provider = cmd.rsplit(':').next().and_then(Provider::parse);
        match (cmd.split(':').next(), provider) {
            (Some("import"), Some(p)) => self.import_local(p),
            (Some("separate"), Some(p)) => self.sign_in_separately(p, None),
            _ => {}
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

    fn account_command(&mut self, id: &str, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.shared.snapshot().account(id).cloned() else { return };
        match command {
            "rename" => {
                let field = widgets::text_field(cx, |f| f.name("Name"));
                field.update(cx, |f, cx| {
                    f.set_text(a.name.clone(), cx);
                    f.caret_to_end(cx);
                });
                let focus = gpui::Focusable::focus_handle(field.read(cx), cx);
                window.focus(&focus);
                // Return renames.
                let sub = cx.subscribe(&field, |a, _, e: &TextFieldEvent, cx| {
                    if matches!(e, TextFieldEvent::Entered) {
                        a.finish_rename(true, cx);
                    }
                });
                self.renaming = Some((a.id, field, sub));
            }
            "refresh" => self.shared.sync_soon(Some(a.id)),
            "sign-in" => self.sign_in_separately(a.provider, Some(a.id)),
            "sign-in-local" => {
                let proxy = self.proxy.clone();
                std::thread::spawn(move || proxy.send(Msg::SignedIn(import::this_computer(a.provider), Some(a.id))));
            }
            "remove" => self.removing = Some(a.id),
            "reset" => self.resetting = Some(a.id),
            _ => {}
        }
        cx.notify();
    }

    fn finish_rename(&mut self, rename: bool, cx: &mut Context<Self>) {
        if let Some((id, field, _)) = self.renaming.take()
            && rename
        {
            let name = field.read(cx).text().trim().to_owned();
            if !name.is_empty() {
                self.shared.rename(&id, &name);
            }
        }
        cx.notify();
    }

    fn pick(&mut self, pick: Pick, cx: &mut Context<Self>) {
        self.picked = if self.picked.as_ref() == Some(&pick) { None } else { Some(pick) };
        cx.notify();
    }

    /// A message from a worker thread, the gateway or a launch.
    pub fn message(&mut self, message: Box<dyn std::any::Any + Send>, cx: &mut Context<Self>) {
        if let Some(launch) = message.downcast_ref::<olo_ui::instance::Launch>() {
            if !launch.args.iter().any(|a| a == "--service") {
                self.open(cx);
            }
            return;
        }
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Changed => {}
            Msg::Clients(on) => self.clients = Some(on),
            Msg::Running(r) => {
                if let Some((_, w)) = &self.clients_win {
                    w.update(cx, |w, cx| {
                        w.found(*r);
                        cx.notify();
                    });
                }
                return;
            }
            Msg::Restarted(r) => {
                if let Some((_, w)) = &self.clients_win {
                    w.update(cx, |w, cx| {
                        w.restarted(r);
                        cx.notify();
                    });
                }
                return;
            }
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
        }
        cx.notify();
    }

    /// Every window closed (handed over to a new copy, which opens them).
    fn close_windows(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.win.take() {
            cx.close_window(w);
        }
        if let Some((w, _)) = self.clients_win.take() {
            cx.close_window(w);
        }
    }

    /// A window closed by the person: what it showed goes (the gateway
    /// keeps serving).
    fn closed(&mut self, clients: bool) {
        if clients {
            if let Some((_, w)) = self.clients_win.take() {
                drop(w);
            }
        } else {
            self.win = None;
            self.renaming = None;
            self.removing = None;
        }
    }

    pub fn restart_codex(&self) {
        let proxy = self.proxy.clone();
        std::thread::spawn(move || proxy.send(Msg::Restarted(codex_daemon::restart_now().map_err(|e| e.to_string()))));
    }

    pub fn clients(&self) -> Option<[bool; 2]> {
        self.clients
    }
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
fn outcome_color(p: &olo_ui::palette::Palette, o: Outcome) -> Hsla {
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
fn window_bar(p: &olo_ui::palette::Palette, w: &QuotaWindow, t: i64) -> StatusBar {
    let mut trailing = format!("{:.0}% used", w.used.clamp(0.0, 999.0));
    if let Some(r) = w.resets_at.filter(|&r| r > t) {
        trailing.push_str(&format!(" · resets {} ({})", clock(r), span(r - t)));
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
fn row() -> gpui::Div {
    widgets::button_row().w_full().h(px(m::CONTROL_HEIGHT))
}

/// Token Pool's window.
pub struct PoolWindow {
    app: Entity<PoolApp>,
    scroll: ScrollHandle,
    _observe: Subscription,
    _released: Subscription,
    /// Times count down (running requests' by the second).
    _tick: Task<()>,
}

impl PoolWindow {
    fn new(app: Entity<PoolApp>, cx: &mut Context<Self>) -> PoolWindow {
        let observe = cx.observe(&app, |_, _, cx| cx.notify());
        let a = app.clone();
        let released = cx.on_release(move |_, cx| a.update(cx, |a, _| a.closed(false)));
        let tick = cx.spawn(async move |this, cx| {
            loop {
                let busy = this.update(cx, |w, cx| !w.app.read(cx).shared.snapshot().open.is_empty()).unwrap_or(false);
                cx.background_executor().timer(Duration::from_secs(if busy { 1 } else { 20 })).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
        });
        PoolWindow { app, scroll: ScrollHandle::new(), _observe: observe, _released: released, _tick: tick }
    }
}

impl Render for PoolWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.clone();
        let p = theme::palette(cx);
        let a = self.app.read(cx);
        let pool = a.shared.snapshot();
        let header = header(a, &pool, &p, &app);
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
                cards = cards.child(card(a, &pool, account, &p, &app));
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
        let status = match (&a.waiting, &a.note) {
            (Some(w), _) => {
                let whose = w.target.as_deref().and_then(|id| pool.account(id)).map(|x| format!("{}’s", x.name)).unwrap_or_else(|| format!("the {}", w.provider.client()));
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
            let color = if error { style::c(p.failure) } else { style::disabled_text(&p) };
            note = note.child(StringView::new(text).color(color));
        }
        bottom = bottom.child(note);
        if a.waiting.is_some() {
            let app = app.clone();
            bottom = bottom.child(Button::new("Cancel Sign-In").on_click(move |_, cx| {
                app.update(cx, |a, cx| {
                    if let Some(w) = a.waiting.take() {
                        w.cancel.store(true, Ordering::Relaxed);
                    }
                    cx.notify();
                })
            }));
        }
        let add = app.clone();
        bottom = bottom.child(Button::new("Add Account").on_click_at(move |b, window, cx| {
            let add = add.clone();
            menu::popup(window, cx, PoolApp::add_menu(), b, Corner::BottomLeft, move |cmd, _, cx| add.update(cx, |a, cx| a.command(cmd, cx)));
        }));
        let clients = app.clone();
        bottom = bottom.child(Button::new("Clients").on_click(move |_, cx| clients.update(cx, |a, cx| a.open_clients(cx))));

        let bar = menu_bar(&app);
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
                .child(div().flex_1().min_h_0().child(ScrollView::new("pool-accounts", &self.scroll).border(false).child(cards)))
                .child(separator())
                .child(bottom),
        )
    }
}

/// The app's menu bar, on each of its windows; Close closes the one
/// it's in.
pub fn menu_bar(app: &Entity<PoolApp>) -> MenuBar {
    let menus = vec![Menu::new("Pool").item("Read Quota Now", "refresh").key("r").item("Clients", "clients").key("l").separator().item("Close", "close").key("w"), PoolApp::add_menu()];
    let app = app.clone();
    MenuBar::new(menus, move |cmd, window, cx| match cmd {
        "close" => window.remove_window(),
        _ => app.update(cx, |a, cx| a.command(cmd, cx)),
    })
}

/// What the gateway is doing, the requests going on now, and which
/// clients use the pool.
fn header(a: &PoolApp, pool: &Pool, p: &olo_ui::palette::Palette, app: &Entity<PoolApp>) -> gpui::Div {
    let busy: u32 = pool.in_flight.values().sum();
    let first = match &pool.error {
        Some(e) => StringView::new(e.clone()).wrap(true).color(style::c(p.failure)),
        None => StringView::new(match busy {
            0 => format!("Listening on 127.0.0.1:{}", a.proxy_port),
            n => format!("Listening on 127.0.0.1:{} · {n} request{} now", a.proxy_port, if n == 1 { "" } else { "s" }),
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
            selected: a.picked == Some(Pick::Open(o.id)),
        })
        .collect();
    let ids: Vec<u64> = pool.open.iter().map(|o| o.id).collect();
    let picker = app.clone();
    let grid = CellGrid::new(cells).rows(2).cell_size(14.0).fill().on_click(move |i, _, cx| picker.update(cx, |a, cx| a.pick(Pick::Open(ids[i]), cx)));
    // One line, always there (so nothing jumps): the picked request.
    let details = match &a.picked {
        Some(Pick::Open(id)) => pool.open.iter().find(|o| o.id == *id).map(|o| format!("Running · {} on {} · {} · {} so far", o.model, o.account, o.client, duration((t - o.started).max(0) as u64))),
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
        let on = a.clients.is_some_and(|c| c[client_index(prov)]);
        let app = app.clone();
        checks =
            checks.child(CheckBox::new(format!("{} uses the pool", prov.client()), on).enabled(a.clients.is_some()).on_toggle(move |on, _, cx| app.update(cx, |a, cx| a.set_client(prov, on, cx))));
    }
    div().flex().flex_col().gap(px(m::SPACING)).child(first).child(grid).child(StringView::new(text).dim(true)).child(checks)
}

/// An account's card: what it is, its quota, today's use, its latest
/// requests, and what can be done with it.
fn card(a: &PoolApp, pool: &Pool, account: &Account, p: &olo_ui::palette::Palette, app: &Entity<PoolApp>) -> BBox {
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
    b = b.child(StringView::new(format!("Today: {} request{} · {} in · {} out", today.requests, if today.requests == 1 { "" } else { "s" }, tokens(today.input_tokens), tokens(today.output_tokens))));
    // Its latest requests, oldest first, coloured by how they ended.
    let cells: Vec<GridCell> = account
        .recent
        .iter()
        .map(|r| GridCell {
            color: Some(outcome_color(p, r.outcome)),
            mark: None,
            name: format!("{}: {} at {}", r.outcome.label(), r.model, clock(r.at)).into(),
            selected: a.picked == Some(Pick::Done(account.id.clone(), r.clone())),
        })
        .collect();
    let (picker, id, recent) = (app.clone(), account.id.clone(), account.recent.clone());
    b = b.child(CellGrid::new(cells).cell_size(12.0).slots(RECENT).on_click(move |i, _, cx| picker.update(cx, |a, cx| a.pick(Pick::Done(id.clone(), recent[i].clone()), cx))));
    b.child(card_controls(a, account, app))
}

fn card_controls(a: &PoolApp, account: &Account, app: &Entity<PoolApp>) -> gpui::AnyElement {
    if let Some((_, field, _)) = a.renaming.as_ref().filter(|(id, _, _)| *id == account.id) {
        let (ok, cancel) = (app.clone(), app.clone());
        return div()
            .flex()
            .items_center()
            .gap(px(m::SPACING))
            .child(TextControl::new(field.clone()).width(180.0))
            .child(
                widgets::button_row()
                    .flex_1()
                    .child(Button::new("Cancel").on_click(move |_, cx| cancel.update(cx, |a, cx| a.finish_rename(false, cx))))
                    .child(Button::new("Rename").default(true).on_click(move |_, cx| ok.update(cx, |a, cx| a.finish_rename(true, cx)))),
            )
            .into_any_element();
    }
    if a.resetting.as_deref() == Some(account.id.as_str()) {
        let when = account.credit().and_then(|c| c.expires_at).map(|e| format!(" (it expires {})", clock(e))).unwrap_or_default();
        let (use_it, cancel, id, name) = (app.clone(), app.clone(), account.id.clone(), account.name.clone());
        return div()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(StringView::new(format!("Use one of its reset credits now{when}? Its quota starts again, and the credit is gone.")).wrap(true))
            .child(
                row()
                    .child(Button::new("Cancel").on_click(move |_, cx| {
                        cancel.update(cx, |a, cx| {
                            a.resetting = None;
                            cx.notify();
                        })
                    }))
                    .child(Button::new("Use Credit").on_click(move |_, cx| {
                        let (id, name) = (id.clone(), name.clone());
                        use_it.update(cx, |a, cx| {
                            let (shared, proxy) = (a.shared.clone(), a.proxy.clone());
                            a.shared.spawn(async move {
                                let r = shared.redeem(&id).await.map(|()| format!("{name}’s quota was reset."));
                                proxy.send(Msg::Done(r));
                            });
                            a.resetting = None;
                            cx.notify();
                        })
                    })),
            )
            .into_any_element();
    }
    if a.removing.as_deref() == Some(account.id.as_str()) {
        let (remove, cancel, id, name) = (app.clone(), app.clone(), account.id.clone(), account.name.clone());
        return div()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(StringView::new("Remove it from the pool? Its sign-in here is deleted; the account itself isn’t touched.").wrap(true))
            .child(
                row()
                    .child(Button::new("Cancel").on_click(move |_, cx| {
                        cancel.update(cx, |a, cx| {
                            a.removing = None;
                            cx.notify();
                        })
                    }))
                    .child(Button::new("Remove").on_click(move |_, cx| {
                        remove.update(cx, |a, cx| {
                            a.note = Some(match a.shared.remove(&id) {
                                Ok(()) => (format!("{name} left the pool."), false),
                                Err(e) => (e, true),
                            });
                            a.removing = None;
                            cx.notify();
                        })
                    })),
            )
            .into_any_element();
    }
    let (toggle, id) = (app.clone(), account.id.clone());
    let use_it = CheckBox::new("Use this account", account.enabled).id(format!("use-{}", account.id)).on_toggle(move |on, _, cx| {
        toggle.update(cx, |a, cx| {
            a.shared.set_enabled(&id, on);
            cx.notify();
        })
    });
    let (menu_app, id, credit, provider) = (app.clone(), account.id.clone(), account.credit().is_some(), account.provider);
    let account_menu = Button::new("Account").id(format!("account-{}", account.id)).min_width(m::BUTTON_MIN_WIDTH).on_click_at(move |b, window, cx| {
        let menu = Menu::new("Account")
            .item("Rename…", "rename")
            .item("Read Quota Now", "refresh")
            .item("Use a Reset Credit…", "reset")
            .enabled(credit)
            .separator()
            .item("Sign In Again in Terminal…", "sign-in")
            .item(format!("Use This Computer’s {} Sign-In", provider.client()), "sign-in-local")
            .separator()
            .item("Remove from Pool…", "remove");
        let (app, id) = (menu_app.clone(), id.clone());
        menu::popup(window, cx, menu, b, Corner::BottomLeft, move |cmd, window, cx| app.update(cx, |a, cx| a.account_command(&id, cmd, window, cx)));
    });
    div().flex().items_center().child(use_it).child(widgets::button_row().flex_1().child(account_menu)).into_any_element()
}

/// The app's state, kept by the app from start to end.
pub fn start(cx: &mut App, app: PoolApp, service: bool) -> Entity<PoolApp> {
    let app = cx.new(|_| app);
    let restored: Option<serde_json::Value> = olo_ui::restored_state().and_then(|s| serde_json::from_str(&s).ok());
    let open = match &restored {
        Some(r) => r["open"] == true,
        None => !service,
    };
    let clients = restored.as_ref().is_some_and(|r| r["clients"] == true);
    app.update(cx, |a, cx| {
        if open {
            a.open(cx);
        }
        if clients {
            a.open_clients(cx);
        }
    });
    let messages = app.clone();
    olo_ui::message_handler(cx, move |message, cx| messages.update(cx, |a, cx| a.message(message, cx)));
    let saved = app.clone();
    olo_ui::on_upgrade(cx, move |cx| {
        let a = saved.read(cx);
        Some(serde_json::json!({"open": a.is_open(), "clients": a.clients_open()}).to_string())
    });
    // A new olo-pool installed: a copy of the new version is started with
    // the listening sockets and takes every new connection at once, while
    // this one finishes what it has under way and then goes (`hand_over`).
    // If no copy can be started, the old way: take no more connections
    // (they wait for it), let what is under way finish (`drain`) and
    // become the new version with the sockets.
    let due = app.clone();
    olo_ui::on_upgrade_due(cx, move |cx| {
        if hand_over(&due, cx) {
            return;
        }
        let shared = due.read(cx).shared.clone();
        shared.drain.begin();
        // Look again now and then (a tunnel falls idle with nothing to
        // say so).
        let weak = Arc::downgrade(&shared);
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
    let busy = app.clone();
    olo_ui::upgrade_waits_while(cx, move |cx| {
        let shared = &busy.read(cx).shared;
        if shared.drain.retired() {
            return true;
        }
        let b = shared.drain.busy();
        if !b && shared.drain.draining() {
            shared.drain.hand_over();
        }
        b
    });
    app
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
fn hand_over(app: &Entity<PoolApp>, cx: &mut App) -> bool {
    use std::os::unix::process::CommandExt as _;
    let (shared, state) = {
        let a = app.read(cx);
        (a.shared.clone(), serde_json::json!({"open": a.is_open(), "clients": a.clients_open()}).to_string())
    };
    let Some(listeners) = shared.drain.for_a_copy() else { return false };
    let Ok(exe) = std::env::current_exe() else { return false };
    let exe = std::path::PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(std::env::args_os().skip(1))
        .env(olo_token_pool::drain::LISTENERS_ENV, listeners)
        .env(olo_token_pool::drain::PREDECESSOR_ENV, std::process::id().to_string())
        .env(olo_xdg::NEW_INSTANCE, "1")
        .env(KEPT_ENV, "1");
    olo_ui::restore::hand_over(&mut cmd, Some(&state));
    // Launches go to the copy from now on (it listens as it starts).
    olo_ui::instance::close();
    let started = cmd.spawn();
    shared.drain.sealed();
    let child = match started {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("pool: couldn’t start the new version beside this one ({e}); upgrading the old way");
            olo_ui::instance::listen(crate::APP_ID, olo_ui::proxy(cx));
            return false;
        }
    };
    tracing::info!(pid = child.id(), "pool: the new version takes every new request; finishing what this one has");
    shared.drain.retire();
    app.update(cx, |a, cx| a.close_windows(cx));
    let weak = Arc::downgrade(&shared);
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
        olo_ui::restore::mark_waiting(false);
        if kept {
            tracing::info!("pool: finished; the new version has it all");
            olo_ui::restore::mark_waiting(false);
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

/// The Clients window closed: what it showed goes.
pub fn clients_closed(app: &Entity<PoolApp>, cx: &mut App) {
    app.update(cx, |a, _| a.closed(true));
}
